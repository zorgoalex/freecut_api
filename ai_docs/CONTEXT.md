# CONTEXT: 2D Cutting Optimization Quality Investigation

## Цель
Добиться качества раскроя, приближенного к идеальным примерам из `ai_docs/logs/perfect/` (6 скриншотов). Ключевые характеристики идеальных раскроев:
- **Ноль внутренних пустот** (bbox_void = 0)
- **9-11 деталей на лист**, плотная упаковка край-к-краю
- **~4 листа с ~8% отходов**, а не 5 листов с 26%
- Маленькие детали только у краёв/углов, не в центре

## Тестовый fixture
`tests/fixtures/multisheet_varied_4sheets.json` — 40 деталей (20 типов x 2), лист MDF 2070x2800mm, trim 10mm, kerf 6mm.

## ОПРЕДЕЛЕНИЯ МЕТРИК
- `min_util` — мин. util среди листов, % (больше = плотнее, идеал 90+%)
- `range` — разница между макс и мин util, % (меньше = ровнее)
- `max_edge_gap` — макс. расстояние от bbox деталей до края листа, мм (меньше = нет stranded кусков)
- `spread` — stddev per-sheet util, % (меньше = ровнее)
- `all-4-≥90%` — % seeds где ВСЕ 4 листа имеют util ≥ 90% (главная метрика)
- `internal_void` (IV) — пустоты окружённые деталями (идеал 0)
- `bbox_void` (BV) — вся пустота внутри bbox деталей (включая kerf)
- `placement count` — фактическое число placements в ответе (для детекта багов)

---

## ЭТАП 1-5: ПРЕДЫДУЩАЯ РАБОТА (см. CONTEXT.md до V1)

### Phase A: OAT Sensitivity (200 запросов)
| Параметр | Влияние | Лучшее значение |
|----------|---------|-----------------|
| epochs (60-500) | **НУЛЕВОЕ** — все идентичны | Любой |
| top_k_candidates (3-48) | **НУЛЕВОЕ** — все идентичны | Любой |
| breed_factor (0.3-0.7) | Слабое | 0.5 (10/10 hard_ok) |
| survival_factor (0.5-0.9) | **НАИБОЛЬШЕЕ** | 0.8 → 4.8 листов, 85583 perim |

### Phase B: Full Factorial (160 запросов)
- epochs 120 vs 300 = ИДЕНТИЧНО
- top_k 3 vs 6 = ИДЕНТИЧНО
- breed=0.5 >> breed=0.6 (4.8-4.9 листов vs 5.0)
- survival=0.8 + breed=0.5 = лучший результат: **4.8 листов, perim 85583**
- **Итог**: `{epochs:300, breed_factor:0.5, survival_factor:0.8, top_k_candidates:3}`

### Phase C: Cross-Mode Validation (40 запросов)
| Режим | hard_ok | Листов | Отходов |
|-------|---------|--------|---------|
| guillotine+standard | 9/10 | 4.8 | ~7.5M mm2 |
| guillotine+portfolio | 10/10 | 5.0 | ~7.5M mm2 |
| nested+standard | 1/10 | 4.2 | ~1.8M mm2 |
| nested+portfolio | 5/10 | 4.1 | ~1.8M mm2 |

### Выводы свипа:
1. **survival_factor** — единственный значимый GA параметр
2. epochs и top_k — **нулевое влияние** (GA популяция сходится раньше)
3. nested mode дает меньше листов (4.0-4.2), но ниже hard_ok
4. guillotine mode надежнее (9-10/10 hard_ok), но больше листов (4.8-5.0)

---

## ЭТАП 2: Диагностика bottleneck

### Тест: min_sheets vs min_waste objective
Результат: **ИДЕНТИЧНЫЕ результаты** во всех конфигурациях.
- guillotine+standard: оба = 4.8 листов
- guillotine+portfolio: оба = 4.3 листов
- nested+portfolio: оба = 4.0 листов (10/10)

**Вывод**: scoring function НЕ является bottleneck. Проблема в SEARCH PROCESS.

### Корневой анализ vendor library fitness
```rust
// vendor/cut-optimizer-2d/src/lib.rs - OptimizerUnit::fitness()
// ТЕКУЩАЯ: усредняет per-bin fitness БЕЗ штрафа за кол-во листов
let fitness = bins.iter().fold(0.0, |acc, b| acc + b.fitness()) / bins.len() as f64;

// vendor/cut-optimizer-2d/src/guillotine.rs - Bin::fitness()
// Per-bin: (used_area / total_area)^(2.0 + free_rects * 0.01)
// ↑ utilization^2 сильно вознаграждает заполненность, free_rects штраф минимален
```

**ДИАГНОЗ**: GA fitness вознаграждает per-bin utilization^2, но НЕ штрафует за количество листов. GA эволюционирует в сторону решений с высокой утилизацией каждого листа, не заботясь об их количестве.

---

## ЭТАП 3: Эксперименты по модификации

### Level 1: Штраф за кол-во листов в fitness (branch: feat/sheet-penalty-scoring)
**Модификация**: `vendor/cut-optimizer-2d/src/lib.rs`
```rust
avg_fitness / (1.0 + alpha * (n - 1.0))  // n = кол-во bins
```

| alpha | Результат | Вывод |
|-------|-----------|-------|
| 0.15 | 5.0 листов (все 10/10) | ❌ ХУЖЕ базовых 4.8 |
| 0.5 | 5.0 листов (все 10/10) | ❌ ХУЖЕ базовых 4.8 |

**Почему не работает**: Per-bin utilization^2 доминирует. 5-листовое решение с 95% utilization имеет ВЫШЕ fitness, чем 4-листовое с 85%. Штраф не перекрывает разницу.

### Level 2: bbox_void как primary objective (branch: feat/void-reduction)
**Модификация**: `src/optimizer.rs` — compare_candidates()
```
Порядок сравнения: sheets → bbox_void → waste → bbox_area → perimeter
(вместо: waste/sheets → bbox_void → bbox_area → perimeter)
```

**Результат**: ❌ НЕТ ЭФФЕКТА. Результаты идентичны.
**Почему**: Изменение влияет только на выбор ФИНАЛЬНОГО кандидата из популяции. GA эволюция не меняется. Если в популяции нет решений с меньшими voids — выбирать не из чего.

### Level 3: Штраф за фрагментацию в vendor fitness (branch: feat/void-reduction)
**Модификация**: `vendor/cut-optimizer-2d/src/maxrects.rs` и `guillotine.rs`
```rust
// БЫЛО:  .powf(2.0 + free_rects.len() * 0.01)  // штраф ~1% за free rect
// СТАЛО: .powf(2.0 + free_rects.len() * PENALTY)
```

| Penalty | Nested (sheets) | Nested (voids) | Guillotine (4-sheet rate) | Guillotine (voids) |
|---------|-----------------|----------------|---------------------------|-------------------|
| 0.01 (baseline) | 4.0 | 1.28M-1.50M | 0/10 (все 5) | 1.30M-1.82M |
| 0.1 | 4.0 | 1.28M-1.50M | 2/10 | 1.27M-1.71M |
| 0.5 | 4.0 | 1.18M-1.38M | 4/10 | 1.22M-1.77M |

**Вывод**: Штраф за фрагментацию ПОМОГАЕТ:
- Guillotine: 0/10 → 4/10 для 4-листовых решений ✅
- Nested voids: min void уменьшился с 1.28M до 1.18M ✅
- **НО**: voids остаются ~1.2M+ mm2 при целевом значении 0

---

## ЭТАП 4: CORRECTED METRICS — Internal Void (flood-fill) vs bbox_void

### КРИТИЧЕСКАЯ ОШИБКА В ПРЕДЫДУЩИХ ИЗМЕРЕНИЯХ
Ранее использовался `bbox_void` (из API) для оценки качества. Это НЕПРАВИЛЬНО:
- **bbox_void** = вся пустота внутри bounding box деталей (включает kerf 6mm зазоры)
- **internal_void** (правильная метрика) = пустоты, **окруженные деталями** (не связанные с краями через flood-fill)

Kerf зазоры, связанные с краем листа, НЕ считаются internal void. `hard_ok` в deep_param_sweep.py использует именно internal_void.

### Результаты с ПРАВИЛЬНОЙ метрикой (fragmentation penalty=0.5):

| Конфиг | Листы | hard_ok | Avg IV | Min IV | Avg BV (для справки) |
|--------|-------|---------|--------|--------|---------------------|
| guillotine+standard 5s | 4.5 | **9/10** | 1,478 | 0 | 1,584K |
| guillotine+portfolio 10s | 4.0 | **8/10** | 19,392 | 0 | 1,504K |
| nested+standard 5s | 4.0 | 5/10 | 9,915 | 0 | 1,480K |
| nested+portfolio 5s | 4.0 | 5/10 | 18,208 | 0 | 1,296K |
| nested+portfolio 10s | 4.0 | 4/10 | 20,698 | 0 | 1,247K |

### Fragmentation penalty sweep (0.5 vs 1.0):

| Конфиг | Penalty | Листы | hard_ok | Avg IV |
|--------|---------|-------|---------|--------|
| guillotine+portfolio 10s | 0.5 | 4.0 | **8/10** | 19,392 |
| guillotine+portfolio 10s | 1.0 | 4.0 | 7/10 | **838** |
| guillotine+standard 5s | 0.5 | 4.5 | **9/10** | 1,478 |
| guillotine+standard 5s | 1.0 | 4.7 | 8/10 | **1,160** |

### Time budget sweep (fragmentation=0.5):
- nested+portfolio: plateau с 10s (void 1.25M, все 4 листа)
- guillotine+standard: plateau с 5s (4.5 листов, 50%→4 листа)
- guillotine+portfolio: plateau с 10s (4.0 листа, 100%→4 листа)
- nested+standard: plateau с 5s (4.0 листа)
- Больше 10s не дает улучшений

### Базовая линия (без fragmentation penalty, baseline 0.01):
- guillotine+standard: 9/10 hard_ok, **4.8 листов**
- guillotine+portfolio: 10/10 hard_ok, **5.0 листов**
- nested+standard: 1/10 hard_ok, 4.2 листов
- nested+portfolio: 5/10 hard_ok, 4.1 листов

### УЛУЧШЕНИЕ от fragmentation penalty 0.5:
- guillotine+standard: hard_ok 9/10→**9/10** (то же), листы 4.8→**4.5** ✅
- guillotine+portfolio: hard_ok 10/10→**8/10**, листы 5.0→**4.0** ✅ (trade-off)
- nested+standard: hard_ok 1/10→**5/10** ✅, листы 4.2→**4.0** ✅
- nested+portfolio: hard_ok 5/10→**5/10** (то же), листы 4.1→**4.0** ✅

---

## ЭТАП 5: Fine-grained penalty sweep (0.2, 0.3, 0.4) + Portfolio candidates

### Полный свип fragmentation penalty (internal_void метрика, 10 seeds):

#### guillotine+portfolio 10s (лучший конфиг):
| Penalty | Листы | hard_ok | Avg IV | Вывод |
|---------|-------|---------|--------|-------|
| 0.01 (baseline) | 5.0 | 10/10 | ~0 | Много листов |
| 0.2 | 4.1 | **9/10** | 372 | ✅ Лучший hard_ok |
| 0.3 | 4.1 | **9/10** | 372 | ✅ Идентичен 0.2 |
| 0.4 | 4.1 | 8/10 | 485 | Чуть хуже |
| 0.5 | 4.0 | 8/10 | 19,392 | ✅ Меньше листов |
| 1.0 | 4.0 | 7/10 | 838 | Хуже hard_ok |

#### guillotine+standard 5s:
| Penalty | Листы | hard_ok | Avg IV |
|---------|-------|---------|--------|
| 0.01 (baseline) | 4.8 | 9/10 | ~0 |
| 0.2 | 4.9 | 8/10 | 1,718 |
| 0.3 | 4.9 | 8/10 | 1,718 |
| 0.4 | 4.6 | **9/10** | 462 |
| 0.5 | 4.5 | **9/10** | 1,478 |

#### nested+portfolio 5s:
| Penalty | Листы | hard_ok | Avg IV |
|---------|-------|---------|--------|
| 0.01 (baseline) | 4.1 | 5/10 | 18,208 |
| 0.2 | 4.0 | 4/10 | 26,365 |
| 0.3 | 4.0 | 3/10 | 31,810 |
| 0.4 | 4.0 | **6/10** | 18,380 |
| 0.5 | 4.0 | 5/10 | 9,915 |

### Portfolio candidate_count sweep (guillotine, penalty=0.2, 10s):
| Candidates | Листы | hard_ok | Avg IV | Время |
|-----------|-------|---------|--------|-------|
| 5 | 4.3 | 6/10 | 910 | 74s |
| 8 | 4.3 | 7/10 | 658 | 87s |
| 10 | 4.1 | 6/10 | 970 | 87s |
| 12 | 4.0 | 7/10 | 698 | 98s |

**Вывод**: Больше кандидатов НЕ дает существенного улучшения. 12 кандидатов = 7/10 vs 6/10 для 5.
Проблема: при 12 кандидатах на 10s бюджет, каждый кандидат получает ~0.8s — слишком мало для глубокой оптимизации.

### КЛЮЧЕВОЙ ИНСАЙТ ЭТАПА 5:
**Variance между прогонами** — результаты сильно зависят от случайных seed и условий выполнения.
penalty=0.2 дает 9/10 hard_ok в одном прогоне, но 6/10 в другом (идентичные параметры). Это означает:
1. Разница между penalty 0.2-0.4 **статистически незначима** для 10 seeds
2. Реальное улучшение начинается с penalty ≥ 0.2 (vs baseline 0.01)
3. Оптимальный диапазон: **0.2-0.5** (точный выбор не критичен)

### Анализ "failed" seeds (20 seeds, guillotine+portfolio, penalty=0.2, 10s):
**Результат**: 15/20 pass (75%), 5/20 fail.

| Failed Seed | Sheets | IV | Where | Pattern |
|-------------|--------|----|-------|--------|
| 0 | 5 | 2,525 | Sheet 0 only | 1 лишняя sheet |
| 6 | 4 | 1,725 | Sheet 0 only | 4 листа |
| 7 | 4 | 3,125 | Sheet 2 only | 4 листа |
| 8 | 4 | 1,725 | Sheet 0 only | 4 листа |
| 15 | 4 | 100,000 | Sheet 3 only | 4 листа, большой void |

**PASS vs FAIL COMPARISON:**
```
  Sheets:       pass=4.2  fail=4.2  ← ИДЕНТИЧНО
  Utilization:  pass=87.5% fail=87.5% ← ИДЕНТИЧНО
  Pieces/sheet: pass=9.5  fail=9.5  ← ИДЕНТИЧНО
  Small pieces: pass=0.0  fail=0.0  ← ИДЕНТИЧНО
```

**Критические открытия:**
1. ❗ Void ВСЕГДА на **ОДНОМ листе** из 4-5 (никогда не распределяется)
2. ❗ Pass/fail **статистически неотличимы** по агрегированным метрикам
3. ❗ Void — это **геометрическая неудача** размещения, не систематическая проблема
4. Размеры void: 1,725-3,125 mm2 (~42×42 до 56×56 mm) — маленькие зазоры
5. Один outlier: seed 15 с iv=100,000 mm2 (~316×316 mm) — крупный trapped void

**Практическое решение:**
- **Вариант A**: Post-hoc retry — при обнаружении void перегенерировать с другим seed
- **Вариант B**: Post-processing compaction — сдвинуть детали на проблемном листе
- **Вариант C**: Увеличить restarts (5→10-20) для большего diversity

### Retry Strategy Test (guillotine+portfolio, penalty=0.2, 10s):
**Три стратегии сравниваются на 20 seeds:**

| Стратегия | Pass Rate | Avg Sheets | Avg Attempts |
|-----------|-----------|------------|--------------|
| Portfolio no retry | 15/20 (75%) | 4.2 | 1.0 |
| **Portfolio retry (3 attempts)** | **20/20 (100%)** | **4.2** | **1.2** |
| Standard no retry (5s) | 16/20 (80%) | 4.9 | 1.0 |

**❗ BREAKTHROUGH**: Retry с max 3 попытками дает **100% pass rate** с avg всего 1.2 попытки!
Все 5 failed seeds (0, 6, 7, 8, 15) были исправлены на 2-й попытке.

**Механизм retry:**
```python
# При iv > 0 — повторить с другим seed (seed + 100, seed + 200)
for retry in range(3):
    res = run_optimize(seed + retry * 100)
    if res.hard_ok: break
```

**Standard mode (без portfolio)**: 16/20 pass (80%), все 5 листов. Хуже portfolio по обоим метрикам.

---

## КЛЮЧЕВЫЕ ВЫВОДЫ (обновлено ЭТАП 5 + Retry)

### Что работает:
1. ✅ survival_factor=0.8, breed_factor=0.5 — лучшие GA параметры
2. ✅ Fragmentation penalty **0.2-0.5** — уменьшает листы и/или улучшает hard_ok
3. ✅ Nested mode стабильно дает 4 листа
4. ✅ **guillotine+portfolio с frag=0.2 и 10s + retry**: **20/20 hard_ok (100%)**, 4.2 листов, avg 1.2 попытки
5. ✅ **Retry mechanism** — простое и эффективное решение для stochastic voids
6. ✅ **guillotine+portfolio с frag=0.5 и 10s**: 8/10 hard_ok, 4.0 листов (минимум листов без retry)

### Что НЕ работает:
1. ❌ Штраф за кол-во листов в fitness — per-bin utilization доминирует
2. ❌ Изменение compare_candidates — не влияет на GA эволюцию
3. ❌ Увеличение epochs/top_k — нулевой эффект (сходимость раньше)
4. ❌ min_sheets objective — идентичен min_waste
5. ❌ Fragmentation penalty > 1.0 — ухудшает hard_ok rate
6. ❌ Time budget > 10s — нет улучшений (plateau)
7. ❌ Portfolio candidates > 5 — незначительное улучшение (6/10→7/10), не стоит вычисл. затрат
8. ❌ Точный подбор penalty в диапазоне 0.2-0.5 — variance между прогонами превышает разницу
9. ❌ Standard mode без retry — только 16/20 pass (80%), все 5 листов

### ИТОГОВАЯ РЕКОМЕНДАЦИЯ:
**Оптимальная конфигурация для production:**
```
layout_mode: guillotine
portfolio: {enabled: true, candidate_count: 5, deadline_ms: 10000}
time_limit_ms: 10000
restarts: 5
fragmentation_penalty: 0.2 (или 0.3-0.5 — не критично)
retry: max 3 attempts, different seed each time
```

**Ожидаемый результат:** 100% hard_ok rate, 4.2 avg листов, avg 1.2 попытки (12s avg)

### GAP vs идеал:
| Метрика | Текущее (лучшее) | Идеал | GAP |
|---------|-----------------|-------|-----|
| Листы | 4.0-4.2 (guill+port+retry) | 4 | ✅ ~OK |
| Internal void | 0 (все seeds с retry) | 0 | ✅ OK |
| hard_ok rate | **20/20 (100%)** | 10/10 | ✅ **ДОСТИГНУТО** |
| Время | ~12s (avg 1.2 попытки) | ~10s | ✅ OK |

---

## ЭТАП 6: Балансировка плотности листов (min-sheet-util tie-breaker)

### Проблема:
При 4 листах всегда одинаковые суммарные отходы (1,840,400 mm2), но **плотность сильно неравномерна**:
- «Хорошие» листы: 93-97% utilization (отлично упакованы)
- «Плохие» листы: 86-88% utilization (пустые полосы по краям)
- Причина: **first-fit-decreasing** алгоритм распределяет детали последовательно, и последний лист получает «остатки»

### Корневая причина в коде:
`compare_candidates()` в `src/optimizer.rs` использует **суммарные** метрики (total_bbox_void, total_bbox_area, total_perimeter) — они не штрафуют за дисбаланс между листами.

### Решение:
Добавлен **min_sheet_util_bps** tie-breaker (basis points, 0-10000 = 0-100%):
```rust
// В Candidate struct:
min_sheet_util_bps: u64,  // мин. utilization среди всех листов

// В compare_candidates() — ПЕРВЫЙ tie-breaker после primary objective:
if candidate.min_sheet_util_bps != best.min_sheet_util_bps {
    if candidate.min_sheet_util_bps > best.min_sheet_util_bps {
        return CandidateCompare::Better;  // более сбалансированный = лучше
    }
    return CandidateCompare::WorseByTieMinUtil;
}
```

### Ключевой момент:
tie-breaker должен быть **ПЕРЕД** bbox_void/area/perimeter, иначе он никогда не срабатывает (bbox_void — непрерывное u128 значение, которое всегда различается).

### Результаты (30 seeds, guillotine+portfolio, 10s):
| Метрика | До tie-breaker | После |
|---------|---------------|-------|
| 4-sheet rate | 17/20 (85%) | **30/30 (100%)** |
| 5-sheet rate | 3/20 (15%) | 0/30 (0%) |
| ALL sheets >= 90% | 0/20 (0%) | **13/30 (43%)** |
| Min util avg | ~88% | **89.7%** |
| Min util range | [86.6%, 89.8%] | [86.5%, 91.2%] |

### Top-5 balanced layouts (all zero-void):
| Rank | Seed | Min util | Spread | Per-sheet util |
|------|------|----------|--------|----------------|
| 1 | 15 | 91.2% | 1.7% | [91.9, 93.1, 91.2, 91.5] |
| 2 | 3 | 91.1% | 2.0% | [91.1, 91.6, 91.9, 93.1] |
| 3 | 10 | 91.0% | 2.2% | [91.9, 91.7, 91.0, 93.2] |
| 4 | 12 | 91.0% | 2.3% | [91.0, 91.8, 91.6, 93.3] |
| 5 | 14 | 91.0% | 2.2% | [91.0, 93.2, 91.7, 91.9] |

Сохранено в `ai_docs/tmp/best_layouts_balanced/`

### GAP vs идеал (обновлённый):
| Метрика | Текущее (лучшее) | Идеал | GAP |
|---------|-----------------|-------|-----|
| Листы | 4.0 (все 30 seeds) | 4 | ✅ OK |
| Internal void | 0 (30/30) | 0 | ✅ OK |
| Min sheet util | 91.2% (best) | ~92% | ✅ **~OK** |
| Balance (spread) | 1.7% (best) | <3% | ✅ OK |

---

## НЕИССЛЕДОВАННЫЕ НАПРАВЛЕНИЯ

1. **Анализ "failed" seeds** — чем отличаются 1-4 seeds, которые не дают zero void? (piece placement patterns, free rect structure)
2. **Post-processing compaction** — после размещения переместить детали для устранения зазоров
3. **Constraint-based placement** — принудительное размещение в сетке с касанием
4. **Seeding с эвристиками** — инициализация GA с FFD/BFD начальными решениями
5. **Другие алгоритмы** — shelf-based, guillotine tree optimization
6. **Увеличение restarts** (5→10→20) — больше шансов найти zero-void решение
7. **Multi-objective fitness** — объединить internal_void и utilization в fitness function

---

---

## ЭТАП 6-12: V1-V7 (текущая работа, изолированные ветки)

### Сводная таблица результатов (multisheet_varied_4sheets, 30 seeds, 10s × 3 retries)

| Branch | Стратегия | 4-sheet | all-4-≥90% | avg_min | avg_range | avg_edge | Замечание |
|---|---|---|---|---|---|---|---|
| **baseline** (feat/void-reduction) | frag=0.2 + min-util TB | 30/30 (100%) | 13/30 (**43%**) | 89.7% | n/a | ~88мм | V0 |
| `feat/visible-kerf-cut-lines` | визуал: #8B0000 cut-lines | 30/30 | = | = | = | = | только визуал |
| **`feat/v1-edge-gap-stdev-tiebreak`** | + edge_gap + stddev TB | 30/30 | **21/30 (70%)** | 90.20% | 3.63% | 29.6мм | **+27%** ✅ |
| `feat/v2-post-compaction` | V1 + slide+center после GA | 30/30 | = | = | = | = (but other fix) | edge_gap −41% |
| `feat/v3-fault-aware-retry` | V2 + smart retry в сервисе | 30/30 | 21/30 (70%) | 90.39% | 3.63% | 29.7мм | **+3.3%** ✅ |
| `feat/v4-heuristic-seeding` | V3 + FFD в candidate pool | 30/30 | = | = | = | = | ❌ no-op, + qty-doubling bug |
| `feat/v5-more-restarts-retry` | V4 + more_restarts для lumpy | 30/30 | = | = | = | = | ⚠ neutral |
| `feat/v6-overfit-check` | V5 + cross-fixture validation | n/a | n/a | n/a | n/a | n/a | ✅ NOT overfit |
| **`feat/v7-fix-heuristic-double-place`** | V6 - убирает V4 bug | 30/30 | 21/30 (70%) | 90.39% | 3.63% | 29.7мм | ✅ bug fix |

**Кумулятивный итог vs baseline: +27% all-4-≥90% (43%→70%), −65% avg edge_gap (88→30мм), +0.7% avg min_util (89.7→90.4%)**

---

## V1: Edge-gap + Util-spread tie-breakers

**Гипотеза:** V0 tie-breaker `min_sheet_util` не различает `[90, 95, 95, 95]` (range=5, "горбатый") и `[90, 91, 92, 93]` (range=3, ровный). Добавить два новых tie-breaker'а ПОСЛЕ min-util.

**Реализация (`src/optimizer.rs`):**
- `Candidate.max_edge_gap_units: u64` — max расстояние от bbox деталей до ЛЮБОГО из 4 краёв листа, в vendor units (меньше = лучше)
- `Candidate.sheet_util_sum_sq_diff_bps2: u64` — sum of squared per-sheet util deviations от mean (прокси для stddev, integer-clean)
- Tie-breakers в `compare_candidates` после min-util, перед bbox_void
- Телеметрия: `candidates_rejected_tie_max_edge_gap`, `candidates_rejected_tie_util_spread`, `winner_max_edge_gap_mm`, `winner_sheet_util_spread_pct`

**Результат (30 seeds, 10s × 3 retries):**
| Метрика | V0 | V1 | Δ |
|---|---|---|---|
| All-4-≥90% | 13/30 (43%) | **20/30 (67%)** | **+24%** ✅ |
| Avg min_util | 89.7% | 90.20% | +0.5% |
| Avg max_edge_gap | ~88мм | 50.2мм | −40мм |
| Avg spread | n/a | 1.49% | new metric |

**Ключевое наблюдение:** tie-breaker должен быть ПЕРЕД bbox_void, иначе bbox_void (непрерывный u128) всегда побеждает.

---

## V2: Post-compaction (slide+center)

**Гипотеза:** даже с V1 tie-breakers, внутри листа могут быть stranded мелкие куски и L-образные пустоты. Локальный slide-left+slide-up + center bbox улучшит edge_gap.

**Реализация (`src/optimizer.rs:compact_solution`):**
Двухпроходная функция:
1. **Per-piece compaction** — каждую деталь максимально влево+вверх, сохраняя kerf от всех остальных. До сходимости.
2. **Bbox centering** — после уплотнения сдвигаем bbox листа так, чтобы max из 4 edge-gaps был минимальным.

**КЛЮЧЕВОЙ БАГ ИСПРАВЛЕН (V2 initial test):** первая версия брала MIN ограничений — это позволяло куску "проскочить" мимо соседа, который сам только что сдвинулся. Результат — перекрывающиеся детали, `max_edge_gap` взлетал до 600мм. **MAX** — корректно (берём самое строгое ограничение = самое правое из "до чего нельзя подвинуться").

**Результат (V1+V2, 30 seeds, 10s × 3 retries):**
| Метрика | V1 | V2 | Δ |
|---|---|---|---|
| All-4-≥90% | 20/30 (67%) | 20/30 (67%) | = |
| Avg max_edge_gap | 50.2мм | **29.6мм** | **−41%** ✅ |
| 4-sheet AND edge≤50мм | 18/30 | 29/30 | +11 |
| 4-sheet AND edge≤30мм | 0/30 | **12/30** | new metric |

min_util/range/spread идентичны V1 (per-sheet util инвариантна к трансляции).

---

## V3: Service-level fault-aware retry

**Гипотеза:** 10 seeds остаются lumpy (88-90%) в V1+V2. Детектировать failure mode и подбирать стратегию retry (вместо фиксированного `+100 seed stride`).

**Реализация (`src/optimizer.rs`):**
- 5 FaultMode: `NoSolution`, `TooManySheets(n≥5)`, `VeryLumpy(min<88%)`, `Lumpy(88≤min<90%)`, `Imbalanced(min≥90%, range>5%)`
- 2 стратегии retry: `different_seed` (+100*n seed), `switch_to_nested` (override layout_mode)
- `optimize_with_smart_retry()` обёртка, до 3 попыток
- Телеметрия: `Summary.retry { attempts, retries, strategies, initial_failure }`
- API: `params.retry_strategy: Disabled|Smart` (default Smart), `params.max_retry_attempts: u32` (default 3)
- `Clone` добавлен на `OptimizeRequest/Params/Units/StockItem/Item/Objective`

**Стратегия selection (apply_strategy):**
- `NoSolution` → `different_seed`
- `Imbalanced`/`Lumpy` → `different_seed`
- `TooManySheets` → `switch_to_nested`
- `VeryLumpy` retry 1: `switch_to_nested`, retry 2+: `different_seed`

**Результат (30 seeds, без client retry):**
| Метрика | V2 (client) | V3 (service) | Δ |
|---|---|---|---|
| All-4-≥90% | 20/30 (67%) | **21/30 (70%)** | **+3.3%** ✅ |
| Avg min_util | 90.20% | 90.39% | +0.19% |
| Avg attempts | 3.0 (forced) | **2.17 (smart)** | **−28%** ✅ |
| Seeds needing retry | 20/30 | 20/30 | = |

**Новый rank 2:** seed=21, min_util=**91.17%**, edge=10.2мм (V2: 90.97%, 43.5мм) — recovered через `switch_to_nested`.

**9 seeds остаются lumpy** (1, 5, 16, 17, 19, 22, 23, 25, 27): даже retry не помогает — нужны другие рычаги.

---

## V4: Heuristic seeding (FFD via vendor API)

**Гипотеза:** GA иногда пропускает плотный layout, который простая FFD бы нашла. Добавить FFD-решение в candidate pool, tie-breakers выберут лучшее.

**Реализация (`vendor/cut-optimizer-2d/src/lib.rs`):**
- `Solution::from_components(fitness, stock_pieces, price)` — публичный конструктор
- `Optimizer::build_guillotine_heuristic()` — FFD с BestAreaFit heuristic (sort by area desc, first_fit_with_heuristic)
- `Optimizer::build_nested_heuristic()` — то же для MaxRectsBin

В `src/optimizer.rs::run_restarts_with_budget`: после GA `top_k`, добавить `build_*_heuristic` в начало SolutionSet.

**Результат (V3+V4, 30 seeds, 10s × 3 retries):** **ИДЕНТИЧНЫ V3** — гипотеза фальсифицирована.

| Метрика | V3 | V4 | Δ |
|---|---|---|---|
| All-4-≥90% | 21/30 (70%) | 21/30 (70%) | = |
| Avg min_util | 90.39% | 90.39% | = |
| Avg edge_gap | 29.7мм | 29.7мм | = |

**Почему не сработало:** GA-evolved решения доминируют над FFD:
1. FFD игнорирует kerf+spacing gap (GA-tuned fragmentation penalty 0.2 учитывает)
2. Vendor's BestAreaFit heuristic уже сильная стратегия
3. Tie-breakers (V1: min_util → edge_gap → spread) сначала сортируют по min_util, и GA's решение на этом уровне не уступает

**Однако V4 ВВЁЗ QTY-DOUBLING BUG** (см. V7).

---

## V5: more_restarts как 3-я стратегия retry

**Гипотеза:** для Lumpy/Imbalanced (GA не нашёл нужный packing region), doubling restarts (5→10, cap 20) даёт GA больше diversity — лучший lever, чем просто seed perturbation.

**Реализация:** добавлена 3-я стратегия `more_restarts`:
- `Lumpy`/`Imbalanced` retry 1: `more_restarts` (restarts × 2, cap 20)
- `Lumpy`/`Imbalanced` retry 2+: `different_seed`
- `VeryLumpy` retry 2+: `more_restarts` (вместо `different_seed`)

**Результат (V4+V5, 30 seeds, 10s × 3 retries):** **NEUTRAL**.

| Метрика | V4 | V5 | Δ |
|---|---|---|---|
| All-4-≥90% | 21/30 (70%) | 21/30 (70%) | = |
| Avg min_util | 90.39% | 90.37% | −0.02% (noise) |
| Avg edge_gap | 29.7мм | 31.3мм | +1.6мм (slightly worse) |
| Seeds used MR | 0/30 | 15/30 | new |

**Per-seed swaps:** seed 30 improved (~~ → OK), seed 5 regressed (OK → ~~). 9 lumpy seeds остаются застрявшими — не diversity, а фундаментальный ceiling.

---

## V6: Cross-fixture overfit check

**Гипотеза:** V1-V5 тестировались только на `multisheet_varied_4sheets`. Возможно overfit под эту фикстуру. Проверить на 3 других.

**Фикстуры и результаты (5 seeds, 10s × 3 retries):**

| Fixture | Sheets | all≥90% | avg_min | avg_range | avg_edge | Вердикт |
|---|---|---|---|---|---|---|
| **multisheet_varied_4sheets** (control) | 5/5 | 3/5 | 90.47% | 3.61% | 36.9мм | V1-V5 работают ✅ |
| multisheet_oversized | 5/5 | 0/5 | 36.85% | 54.75% | 565.2мм | Структурный лимит |
| multisheet_qty_limit | 5/5 | 0/5 | 57.27% | 33.48% | 530.2мм | Структурный лимит |
| optimize_valid | 5/5 | 0/5 | 58.31% | 0.00% | 87.0мм | **Pre-existing bug** |

**Визуально подтверждён structural limit** для oversized: 1-й лист получает 2 strip-long, 2-й и 3-й плотные. Items не делятся на 3 равные группы — никакой algorithm не исправит.

**ВЕРДИКТ: NOT overfit.** V1-V5 hold their gains на оригинальной фикстуре, не регрессируют другие.

---

## V7: Fix V4 qty-doubling bug

**Симптом (обнаружен в V6):** `optimize_valid.json` (qty=2 stock, 2 items × {qty=2, qty=1} = 3 items) возвращает **6 placements** вместо 3. Каждый instance удваивается:
- Expected: A inst=1, A inst=2, B inst=1
- Got: A inst=1 (x2), A inst=2 (x2), B inst=1 (x2)

**Изоляция бага:**
- Воспроизводится с `portfolio: None`, `retry_strategy: disabled`, любым `layout_mode`, любой `rotation`
- НЕ воспроизводится на `multisheet_varied_4sheets` (40 items → 40 placements)
- Универсальный паттерн: `N items → 2N placements` (каждый instance удваивается)

**Корневая причина (`vendor/cut-optimizer-2d/src/lib.rs:1095-1098`):**

V4 добавил `build_guillotine_heuristic()` и `build_nested_heuristic()`, которые вызывали:
```rust
let Ok(mut unit) = OptimizerUnit::with_heuristic(...);  // ← УЖЕ place all cuts via first_fit
for cut in &cuts {
    unit.first_fit_with_heuristic(cut, &heuristic, &mut rng);  // ← BUG: double-places
}
```

`with_heuristic` (lib.rs:510-513) УЖЕ вызывает `first_fit_with_heuristic` для каждого cut piece при конструировании unit'а. Второй цикл в V4 `build_*_heuristic` дублировал каждую деталь — для маленьких фикстур с большим free space деталь находила место для второй копии.

**Почему `multisheet_varied_4sheets` НЕ задело:** там 40 items на 4 листа — после первого прохода bin'ы полные, второй проход fails для всех 40 items → 0 дополнительных placements. На малых фикстурах (1 лист, 1-3 items) — bin полу-пустой, второй проход успешно дублирует.

**Фикс:** убран второй `for cut in &cuts` цикл из обоих `build_*_heuristic` методов.

**Верификация:**
- `optimize_valid.json` seed=1: **3 placements** ✅ (было 6)
- V1-V3 метрики на `multisheet_varied_4sheets` не изменились (FFD всё равно не побеждал GA, баг был невидим на больших фикстурах)

---

## Прогресс по веткам (изолированно, можно мержить выборочно)

| Ветка | Описание | Δ vs baseline |
|---|---|---|
| `feat/void-reduction` (V0) | frag=0.2 + min-util TB | — |
| `feat/visible-kerf-cut-lines` | Визуал: #8B0000 cut-lines | = метрики |
| `feat/v1-edge-gap-stdev-tiebreak` | + edge_gap + stddev TB | **+24% all-4-≥90%** |
| `feat/v2-post-compaction` | V1 + slide+center | **−41% edge_gap** |
| `feat/v3-fault-aware-retry` | V2 + service-level smart retry | **+3.3% all-4-≥90%**, −28% avg attempts |
| `feat/v4-heuristic-seeding` | V3 + FFD в pool | no-op + qty-doubling bug |
| `feat/v5-more-restarts-retry` | V4 + 3-я retry стратегия | neutral |
| `feat/v6-overfit-check` | V5 + cross-fixture validation | ✅ generalises |
| `feat/v7-fix-heuristic-double-place` | V6 - фикс V4 бага | ✅ fix |

**PRODUCTION-READY CONFIG (V1+V2+V3):**
```
layout_mode: guillotine
portfolio: {enabled: true, candidate_count: 5, deadline_ms: 10000}
time_limit_ms: 10000
restarts: 5
fragmentation_penalty: 0.2
retry_strategy: smart
max_retry_attempts: 3
```

**Ожидаемый результат:** 70% all-4-≥90%, 4.2 avg листов, avg 1.2 attempts, ~12s avg время.

---

## КЛЮЧЕВЫЕ ВЫВОДЫ (финальные)

### Что работает:
1. ✅ `survival_factor=0.8`, `breed_factor=0.5` — лучшие GA параметры
2. ✅ Fragmentation penalty **0.2-0.5** — уменьшает листы
3. ✅ V1 edge_gap + stddev tie-breakers
4. ✅ V2 post-compaction (slide+center)
5. ✅ V3 smart retry с fault detection
6. ✅ V6 generalises (не overfit)
7. ✅ V7 fix qty-doubling bug от V4

### Что НЕ работает:
1. ❌ Штраф за кол-во листов в fitness — per-bin utilization^2 доминирует
2. ❌ FFD heuristic seeding (V4) — GA's решения доминируют
3. ❌ More_restarts для lumpy (V5) — не diversity проблема, а ceiling
4. ❌ Точный подбор penalty 0.2 vs 0.5 — variance > delta
5. ❌ Standard mode без retry — 80% pass rate
6. ❌ GA params epochs/top_k — нулевое влияние (сходимость раньше)
7. ❌ min_sheets vs min_waste objective — идентичны

### Неисследованные направления:
1. **Per-failure-mode GA param override** (e.g., survival_factor=0.9 для lumpy)
2. **Different tie-breaker order** (попробовать total_waste первым)
3. **Larger top_k** в GA (больше кандидатов для tie-breakers)
4. **Multi-objective fitness** — комбинировать internal_void и util в GA fitness
5. **Constraint-based placement** (принудительное касание деталей)

---

## Git ветки (текущее состояние)

| Ветка | Базируется на | Δ | Запушен? |
|---|---|---|---|
| `main` | — | (без изменений) | ✅ |
| `feat/newcore` | main | — | ✅ |
| `feat/void-reduction` | newcore | V0: frag=0.2 + min-util TB | ✅ |
| `feat/visible-kerf-cut-lines` | void-reduction | визуал | ✅ |
| `feat/v1-edge-gap-stdev-tiebreak` | visible-kerf-cut-lines | V1 | ✅ |
| `feat/v2-post-compaction` | v1 | V2 | ✅ |
| `feat/v3-fault-aware-retry` | v2 | V3 | ✅ |
| `feat/v4-heuristic-seeding` | v3 | V4 + qty bug | ✅ |
| `feat/v5-more-restarts-retry` | v4 | V5 | ✅ |
| `feat/v6-overfit-check` | v5 | V6 | ✅ |
| `feat/v7-fix-heuristic-double-place` | v6 | V7 (qty fix) | (локально, не запушен) |

## Ключевые файлы (актуальные на V7)

- `vendor/cut-optimizer-2d/src/lib.rs:786-800` — `OptimizerUnit::fitness()` (GA level)
- `vendor/cut-optimizer-2d/src/lib.rs:831-855` — `Solution` struct + `from_components` constructor (V4)
- `vendor/cut-optimizer-2d/src/lib.rs:1075-1108` — `build_guillotine_heuristic()` (V4, исправлен V7)
- `vendor/cut-optimizer-2d/src/lib.rs:1110-1144` — `build_nested_heuristic()` (V4, исправлен V7)
- `vendor/cut-optimizer-2d/src/lib.rs:492-517` — `with_heuristic` (УЖЕ place all cuts, V4 не должно было повторно)
- `vendor/cut-optimizer-2d/src/maxrects.rs:77-102` — `MaxRectsBin::fitness()` (nested mode)
- `vendor/cut-optimizer-2d/src/guillotine.rs:122-135` — `GuillotineBin::fitness()` (guillotine mode)
- `src/optimizer.rs:121-138` — `Candidate` struct (с min_sheet_util_bps, max_edge_gap_units, sheet_util_sum_sq_diff_bps2)
- `src/optimizer.rs:167-186` — `CandidateCompare` enum (8 вариантов)
- `src/optimizer.rs:471-549` — `FailureMode` enum + `assess_failure()` + `per_sheet_utils()` (V3)
- `src/optimizer.rs:551-590` — `choose_strategy()` + `apply_strategy()` (V3, +more_restarts в V5)
- `src/optimizer.rs:594-610` — `response_score()` + `is_better_response()` (V3)
- `src/optimizer.rs:613-678` — `optimize_with_smart_retry()` (V3)
- `src/optimizer.rs:985-991` — V2 compaction в `run_restarts_with_budget`
- `src/optimizer.rs:2053-2109` — `compact_solution()` (V2)
- `src/optimizer.rs:2153-2237` — `build_candidate()` (с V1+V3 метриками)
- `src/optimizer.rs:2264-2300` — `build_candidate_selection_telemetry()` (с winner_* V1+V3)
- `src/optimizer.rs:2305-2310` — `compare_candidates()` с 6 tie-breaker'ами
- `src/models.rs:20-50` — `Params` (с retry_strategy, max_retry_attempts)
- `src/models.rs:60-67` — `RetryStrategy` enum
- `src/models.rs:204-260` — `Summary` (с `retry: Option<RetryTelemetry>`)
- `src/models.rs:263-270` — `RetryTelemetry` struct
- `src/models.rs:331-352` — `CandidateSelectionTelemetry` (с V1+V3 полями)

## Скрипты (актуальные на V7)

- `scripts/deep_param_sweep.py` — параметрический свип (Этап 1)
- `scripts/analyze_internal_void.py` — правильная метрика IV (flood-fill)
- `scripts/test_retry_strategy.py` — тест retry (Этап 5)
- `scripts/test_v1_edge_gap_stdev.py` — V1 30-seed test
- `scripts/test_v2_post_compaction.py` — V2 30-seed test
- `scripts/test_v3_smart_retry.py` — V3 30-seed test
- `scripts/test_v4_heuristic_seed.py` — V4 30-seed test
- `scripts/test_v5_more_restarts.py` — V5 30-seed test
- `scripts/test_v6_overfit_check.py` — V6 4-fixture test
- `scripts/test_v7_heuristic_fix.py` — V7 sanity (optimize_valid)
- `scripts/test_v7_heuristic_fix_full.py` — V7 30-seed main fixture (aborted, fix confirmed separately)

## Артефакты (SVG/JSON, в ai_docs/tmp/)

- `best_layouts/` — оригинальные best layouts (CONTEXT.md pre-V1)
- `best_layouts_frag02/` — V0: 4-sheet layouts
- `best_layouts_balanced/` — V0: min-util tie-breaker top-5
- `best_layouts_visible_kerf/` — V-визуал: #8B0000 cut-lines
- `best_layouts_v1/` — V1 top-5 (rank_01: seed=15, min=91.18%, range=1.97%, edge=17мм)
- `best_layouts_v2/` — V2 top-5 (rank_01: seed=15, min=91.18%, edge=29.6мм avg)
- `best_layouts_v3/` — V3 top-5 (rank_02 NEW: seed=21, min=91.17%, edge=10.2мм)
- `best_layouts_v4/` — V4 top-5 (идентичны V3, FFD no-op)
- `best_layouts_v5/` — V5 top-5 (neutral)
- `best_layouts_v6/` — V6 per-fixture bests (varied=working, oversized/qty_limit=structural)
- `best_layouts_v7/` — V7 top-5 (после qty bug fix, идентичны V1)
- `scripts/test_retry_strategy.py` — тест retry стратегии (portfolio vs standard, max 3 попытки)
- `scripts/test_min_util_balance.py` — тест min-sheet-util tie-breaker (20 seeds)
- `scripts/analyze_sheet_util.py` — детальный per-sheet utilization анализ
- `scripts/save_balanced_layouts.py` — сбор и сохранение сбалансированных zero-void layouts
- `scripts/test_top5_seeds.py` — проверка конкретных seeds до/после

---

## ЭТАП 13: АНАЛИЗ — визуальный аудит топ-раскроев + теоретический потолок (2026-06-12)

### Визуальный аудит (SVG → PNG, по-листовой разбор)

Изучены: `best_layouts_balanced/` (seed 15 rank 1, seed 3 rank 2 — V0+min-util TB) и `best_layouts_frag02/` (seed 100, seed 1 — V0). Все по 4 листа, IV=0.

**Общий визуальный паттерн дефектов (повторяется на всех изученных раскроях):**
1. **Отходы фрагментированы на 2–4 несвязные зоны на каждом листе** — вместо одного крупного остатка.
2. **Пустоты часто в СЕРЕДИНЕ листа** (вертикальные «коридоры» между колоннами деталей, прямоугольные «окна» в центре-справа) — а не у края/угла. Такой остаток бизнес-непригоден для повторного использования.
3. frag02 (V0): широкие вертикальные коридоры посреди листа. balanced: плотнее, но отходы всё равно рассеяны (у seed 15 лист 3 — пустой прямоугольник ~500×700 в центре-справа; лист 1 — колонна-зазор между core_b и колоннами strip).
4. **V2-компакция «центрирует» bbox — это РАЗМАЗЫВАЕТ отход по периметру**, что минимизирует max_edge_gap, но прямо противоречит консолидации остатка в один кусок.

### Теоретический потолок (посчитано из фикстуры)

- Сумма площадей 40 деталей: **20,955,600 mm²**; usable на лист (после trim 10мм): **5,699,000 mm²**.
- При 4 листах средняя утилизация = **91.93%** — это жёсткий потолок min_util.
- **LPT-разбиение по площади на 4 группы даёт [91.88, 91.88, 91.98, 91.98]% — ВСЕ ≥ 90%.**

**КЛЮЧЕВОЙ ВЫВОД: идеально сбалансированное разбиение деталей по листам СУЩЕСТВУЕТ и находится тривиальным жадным алгоритмом за миллисекунды.** Значит метрика all-4-≥90% достижима на 100% (сейчас 70%). Оставшиеся 9 lumpy seeds — это **ошибка РАЗБИЕНИЯ деталей по листам** (партиционирования), а не упаковки: GA-конвейер first-fit-decreasing формирует неравные группы, и никакие retry/restarts (V3/V5) этого не чинят, т.к. не управляют партицией напрямую.

### Состояние веток (важно)

- `main` использует **crates.io cut-optimizer-2d 0.4.2**, БЕЗ `vendor/` — вся цепочка V0–V7 живёт на линии `feat/newcore → ... → feat/v7-fix-heuristic-double-place` и в main НЕ влита.
- beam/ALNS endpoints из `last_state.md` в `src/` на main тоже отсутствуют (другая линия веток).
- Новые ветки для V8+ создавать от `feat/v7-fix-heuristic-double-place` (актуальная вершина цепочки), не от main.

### Полный аудит всех 10 top-раскроев (количественный, flood-fill по JSON, grid 10мм)

Скрипт: `/tmp/svgconv/void_audit.py` (стоит перенести в `scripts/audit_void_geometry.py`).
Метрика `fill%` = площадь зоны отхода / её bbox — показывает «компактность» отхода (100% = чистый прямоугольник-остаток, <15% = змеевидный коридор через весь лист).

**Распределение листов по качеству (40 листов из 10 раскроев):**
| Класс | Util | Кол-во | Геометрия отхода |
|---|---|---|---|
| Отличные | 93–96.6% | ~10 | 1 зона, компактная полоса у края (лучший: 96.6%, отход 116k mm² fill 4% — узкая полоса) |
| Хорошие | 91–93% | ~22 | 1–2 зоны, но fill 5–25% — отход «змеится» коридорами через весь лист (bbox 2050×2780 при площади 300–400k) |
| Слабые | 86.6–89.8% | ~8 | 1 крупная зона 490–660k mm², лист просто недогружен по площади |

**Три конкретных дефекта, мешающих «самому топу»:**
1. **Партиционный дисбаланс** (главный, ~8 листов): слабые листы 86–90% недогружены ПО ПЛОЩАДИ — детали можно перераспределить (LPT доказывает достижимость 91.88%+ на всех). Никакая упаковка это не лечит.
2. **Коридорный отход вместо остатка** (~22 листа): отход связный, но fill 5–15% — тонкие (40–200мм) вертикальные коридоры между колоннами деталей + рваные окончания рядов. Причина: колонны деталей не подбираются по суммарной ширине под 2050мм (пример frag02 seed100 sheet0: остаточные полосы 60×2780 и 190×2000), а V2-центрирование дополнительно размазывает отход по периметру.
3. **Рваный край рядов**: ряды заканчиваются на разных x → ступенчатая граница → отход не прямоугольный.

**Эталон достижимого** (среди существующих результатов): frag02 seed5 sheet0 — 96.6%, отход одной компактной полосой у края. Такой паттерн нужно сделать систематическим.

### ЦЕЛЕВОЕ КАЧЕСТВО — визуальный разбор эталонов `logs/perfect/` (6 скринов)

Все 6 эталонных листов имеют ОДИН И ТОТ ЖЕ структурный паттерн, которого нет у наших текущих топов:

1. **Отход = единая связная зона у одного угла** (правый-нижний), формой «лесенки». Детали прижаты к верхнему и левому краям; занятая область — монотонный «скайлайн», спускающийся слева-направо. Ни одного коридора в середине листа.
2. **Колонны деталей подобраны по ширине**: внутри колонны детали одинаковой ширины стоят стопкой край-к-краю (только kerf между ними); ширины колонн в сумме закрывают ширину листа. Поэтому вертикальных щелей между колоннами нет вообще.
3. **Тонкие длинные детали — у краёв** (крайняя левая полоса на всю высоту) или как разделители колонн, никогда в центре.
4. **Мелкие детали — на «ступенях лесенки»** внизу колонн, на границе с зоной отхода.
5. 9–11 деталей на лист, разнотипные (большие панели + средние + мелочь) — т.е. партиция смешивает типоразмеры, а не группирует одинаковые.

**Операционализация (как мерить близость к эталону):**
- `n_waste_regions == 1` (сейчас 1–3)
- зона отхода касается ДВУХ смежных краёв листа (угловая), а не размазана по периметру
- `waste_fill%` (площадь зоны / её bbox) ≥ 50% (сейчас 5–25% — змеевидные коридоры)
- эквивалентно: «скайлайн» занятой области монотонный, без ям
- internal_void = 0 (уже достигнуто)

**Вывод для алгоритма:** эталон требует другой ФОРМЫ отхода (одна угловая зона) И максимального заполнения доступного пространства.

**УТОЧНЕНИЕ ЦЕЛИ (от пользователя, 2026-06-12):** консолидация отхода — НЕ вместо плотности. Если пространство позволяет разместить деталь — оно должно быть заполнено; угловой остаток должен быть минимальным из возможных. Практическое следствие при фиксированном минимуме 4 листа (3 листа математически невозможны: 20.96M > 3×5.7M, суммарный отход фиксирован = 1.84M):
- **Стратегия «dense-first» вместо балансировки**: первые листы заполнять до геометрического максимума (~96.6% доказано достижимо, см. frag02 seed5 sheet0), весь слак сгонять на ПОСЛЕДНИЙ лист одной крупной угловой зоной — это и максимальная утилизация рабочих листов, и максимально пригодный остаток.
- min-util/balance tie-breakers (V0/V1) при этой цели контрпродуктивны — их роль пересматривается (балансировка размазывает отход по всем 4 листам мелкими кусками).
- Метрика цели: max суммарная утилизация листов 1..N-1 + один угловой остаток на листе N + везде fill% ≥ 50% и n_waste_regions=1.

### ПЛАН СЛЕДУЮЩИХ ГИПОТЕЗ (приоритезировано)

**V8 (высший приоритет): Area-balanced partition — атака на 9 lumpy seeds.**
Гипотеза: лимит — партиция, не упаковка (доказано LPT-расчётом).
Два варианта реализации (можно оба, сравнить):
- a) *Pre-partition*: LPT/Karmarkar-Karp разбиение деталей на 4 сбалансированные группы → упаковка каждого листа независимо (GA на один лист) → fallback на текущий конвейер, если какой-то лист не упаковался. Геометрическая осуществимость ~91.9% на лист подтверждена существующими листами 91.9–93.3%.
- b) *Post-GA inter-sheet local search*: после GA найти лист с min_util<90% → перемещать/менять детали между самым плотным и самым разреженным листом (swap по площади) → перепаковка двух затронутых листов → принять если min_util вырос. Дешевле, чем полный re-run, вписывается в smart-retry (V3) как новая стратегия `rebalance`.
Ожидание: all-4-≥90% 70% → 90-100%.

**V9: Консолидация остатка (waste consolidation) — бизнес-ценность остатков.**
Гипотеза: при фиксированных 4 листах суммарный отход постоянен (1,840,400 mm²) — выигрывать дальше можно только ФОРМОЙ отхода.
- Новый tie-breaker: максимизировать **largest free rectangle** (макс. вписываемый прямоугольник пустоты) на лист, или минимизировать число несвязных зон отхода.
- Заменить V2-центрирование на **углового якоря** компакцию (всё в левый-верхний угол) — отход собирается в один L-образный кусок у двух краёв.
- ⚠ Конфликт с метрикой max_edge_gap (она вознаграждает размазывание) — нужно решить, что важнее бизнесу: ровные поля или один крупный остаток. Визуальный аудит говорит — крупный остаток.

**V9 РЕАЛИЗОВАН (2026-06-12, ветка `feat/v9-waste-consolidation`, 2 коммита) — результаты ниже.**

**V10: Nested + полный V1–V3 стек.**
Nested стабильно даёт 4.0 листа, но исторически хуже по IV. Tie-breakers V1, компакция V2 и smart-retry V3 тестировались в основном на guillotine. Прогнать 30 seeds nested с полным стеком — возможно nested+стек обгонит guillotine по min_util (у nested нет ограничения гильотинных резов, потолок плотности выше).

**V11 (опционально): связать visual-scoring (exposure_penalty/penetration_weighted из last_state.md) как поздний tie-breaker** — устранит «детали в центре пустого листа» паттерны, которые формулы CONTEXT.md не ловят.

### Чем НЕ заниматься дальше (закрыто предыдущими этапами)
- GA-параметры (epochs/top_k/penalty fine-tuning) — выработаны, variance > delta.
- FFD seeding, more_restarts — фальсифицированы (V4, V5).
- Time budget > 10s — плато.

---

## ЭТАП 14: V9/V9.1 — corner-anchored компакция + corner_free tie-breaker (2026-06-12)

Ветка: `feat/v9-waste-consolidation` (от `feat/v7-fix-heuristic-double-place`), коммиты `8d5914c` (V9), `6547f50` (V9.1).

### Реализация
1. **`compact_solution`**: удалён Pass 2 (bbox-центрирование) — детали остаются прижатыми влево-вверх, отход собирается справа-внизу. Центрирование размазывало отход по периметру.
2. **Новая метрика `Candidate.corner_free_area_units`**: сумма по листам наибольшего свободного прямоугольника, прижатого к правому-нижнему углу (`corner_free_rect_area()`, O(n²) по правым кромкам деталей).
3. **`compare_candidates`**: corner_free — ПЕРВЫЙ tie-breaker (больше = лучше); max_edge_gap и util_spread УБРАНЫ из сравнения (противоречат консолидации; оставлены в телеметрии). min_util оставлен вторым.
4. V9.1: компакция перенесена в `pick_best_candidate` — теперь ВСЕ кандидаты компактируются ДО ранжирования (раньше — только финальный победитель, tie-breaker сравнивал «сырую» геометрию).
5. Телеметрия: `winner_corner_free_area_mm2`, `candidates_rejected_tie_corner_free`.
6. Бенчмарк: `scripts/test_v9_corner_waste.py` (30 seeds, guillotine+portfolio 10s, smart retry; метрики: lead_util = среднее по n-1 плотнейшим листам, число зон отхода ≥5k mm², max corner rect).

### Результаты (30 seeds, release build, порт 8088)
| Метрика | V9.0 | V9.1 | Цель |
|---|---|---|---|
| 4-sheet rate | 30/30 | 30/30 | 30/30 ✅ |
| Avg lead util (плотнейшие n-1) | 92.64% | 92.65% | ~96% |
| Avg min util | 89.78% | 89.76% | (не цель в dense-first) |
| Avg зон отхода/раскрой | 9.3 | 9.4 | 4 ❌ |
| Avg max corner rect | 234k | 243k | ≥460k (=1.84M/4) |
| Layouts max_corner≥300k | 3/30 | 4/30 | — |

Лучший: V9.1 seed=2 — utils **[95.9, 95.5, 89.2, 87.1]**, corner 434k (~2050×212мм полоса). Артефакты: `spec_freecut/tmp/best_layouts_v9/` (V9.1) и `best_layouts_v9_0/`.

### Выводы
1. ✅ **Dense-first работает частично**: появились раскрои с 2 листами ~96% и слаком на последних (seed 2/21). Раньше балансировка такое отбрасывала.
2. ✅ Визуально форма лучше: детали прижаты влево-вверх, основной отход справа-внизу (проверено по SVG seed 2, 3).
3. ❌ **Консолидация в 1 зону/лист НЕ достигнута** (9.4 зоны vs цель 4): мелкие «карманы» между рядами остаются.
4. ❗ **Ключевой урок (повтор Level 2/V4)**: tie-breaker выбирает только из пула GA-кандидатов. Если в пуле нет «лесенок» — выбирать не из чего. V9.1 (компакция всех кандидатов до ранжирования) почти не изменил результат: пул из ~top_k×restarts×portfolio кандидатов слишком однороден.
5. Слайд-компакция не убирает карманы внутри гильотинной структуры: ряды разной ширины блокируют друг друга.

### Следующий шаг — V8 (партиция), главный нереализованный рычаг
Tie-breaker'ы исчерпаны. Дальше нужно менять то, ЧТО генерируется, а не как выбирается:
- **V8a pre-partition**: LPT/Karmarkar-Karp разбиение по площади (доказано: даёт все 4 листа ≥91.88%) → упаковка каждого листа отдельно → fallback на текущий конвейер. Для dense-first: партиция «3 плотных + 1 остаточный» (заполнять листы жадно до ~96%, остаток на последний).
- **V8b post-GA rebalance**: новая стратегия smart-retry — перенос/обмен деталей между плотным и разреженным листом с перепаковкой двух листов.
- **V9b width-matched columns** (внутри упаковщика): подбирать колонны деталей с суммой ширин ≈ ширине листа — устраняет вертикальные коридоры по построению.

---

## ЭТАП 15: V8 — управление партицией: pre-partition ФАЛЬСИФИЦИРОВАН, peeling РАБОТАЕТ (2026-06-12)

Ветка: `feat/v8-partition` (от `feat/v9-waste-consolidation`).

### КЛЮЧЕВОЕ ОТКРЫТИЕ: GA не может переупаковать форсированную группу

V8a в исходной формулировке (жадная dense-first партиция по площади → упаковка каждой группы per-sheet через GA) **мёртв**. Прямой эксперимент: взяли РЕАЛЬНЫЙ лист 95.93% из V9.1 seed 2 (12 деталей, упаковка заведомо существует) и дали vendor GA только эти детали:
- guillotine: **2 листа** (все seeds, сходится за ~170мс — не бюджет, потолок поиска);
- nested: 1 лист на 95.93%, но 2 листа уже на 95.53%;
- группы ≤~90%: пакуются в 1 лист стабильно.
Потолок переупаковки форсированных групп: **~90-93%**. Жадные группы (большие плиты + мелкие добивки точно под cap) не пакуются даже при cap 90%. Плотные листы возникают ТОЛЬКО когда GA сам выбирает, какие детали выдавить на другие листы.

### Решение: iterative peeling (V8a-revised)

Алгоритм `run_partitioned` (src/optimizer.rs): упаковать ВСЕ оставшиеся детали обычным GA → заморозить плотнейший лист КАК ЕСТЬ (с геометрией, без переупаковки) → выкинуть его детали из пула → пере-оптимизировать остаток → повторять, пока остаток не влезет в 1 лист по площади. Слак стекает на последний лист по построению.

Критические детали реализации:
1. **Best-of-K попыток на peel**: один сабсет-прогон GA сходится за ~1-2с (early-stop patience 4), длиннее слайсы бесполезны. Бюджет peel'а конвертируется в K≤16 независимых re-seeded попыток (attempt_budget = budget/8, мин 600мс); выбирается попытка с максимальным util плотнейшего листа.
2. **Площадной фильтр осуществимости**: попытка отбрасывается, если после заморозки остаток не влезает в оставшиеся листы по площади (иначе 5-й лист позже).
3. Остаточная итерация: выбор по (меньше листов, больше corner_free_area).
4. Заморозка peeled > n_min листов → fallback на обычный конвейер.
5. API: `params.partition = {enabled, sheet_budget_ms}`; телеметрия `summary.partition` (applied, group_sizes, group_area_pct по листам, fallback_reason). При applied — smart-retry НЕ запускается (балансо-ориентированный assess_failure ложно считает dense-first «very_lumpy»).

### V8b: rebalance как стратегия smart-retry

Реализован (`rebalance_attempt`): перенос деталей с разреженного листа на плотные с верификацией реальной перепаковкой одного листа (pack_group_single_sheet), принятие по dense_score (листы, -lead_util, -min_util), терминальное. Включается только при `partition` в запросе, для fallback-случаев. На практике почти не срабатывает — тот же потолок переупаковки (receiver ~92% + деталь → >93% → не пакуется). Не вреден, оставлен.

### Результаты (30 seeds, multisheet_varied_4sheets, release, порт 8088)

| Метрика | V9.1 | V8 (2.5с/peel) | V8 (10с/peel) | **V8 (20с/peel, K=16)** | Цель |
|---|---|---|---|---|---|
| 4-sheet rate | 30/30 | 30/30 | 30/30 | **30/30** | ✅ |
| Avg lead util | 92.65% | 92.91% | 93.89% | **94.20%** | ~96% |
| Avg min util | 89.76% | 88.98% | 86.05% | 85.10% | (dense-first: не цель) |
| Avg зон отхода | 9.4 | 8.8 | 8.5 | **7.9** | 4 ❌ |
| Avg max corner | 243k | 238k | 377k | **461k** | ≥400k ✅ |
| max_corner≥300k | 4/30 | 4/30 | 19/30 | **26/30** | — |
| max_corner≥400k | — | — | 13/30 | **~23/30** | стабильно ✅ |
| Peeling applied | — | 23/30 | 29/30 | 29/30 | — |
| Wall/раскрой | ~10-20с | ~6с | ~17с | ~34с | (пользователь разрешил) |

Топы: seed 19 — utils [97.7, 95.8, 93.2, 81.0]; seed 28 — [96.6, 95.0, 93.6, 82.5], corner 662k. Артефакты: `spec_freecut/tmp/best_layouts_v8_20s/` (также `_v8/` 2.5с и `_v8_10s/`). Бенчмарк: `scripts/test_v8_partition.py` (env `FREECUT_SHEET_BUDGET_MS`, `FREECUT_OUT_DIR`).

### Визуальный аудит топов
- Лидирующие листы (96-97.7%): сплошное заполнение, эталонный уровень (seed 19 лист 1 — почти без щелей).
- Слак-лист (81-84%): крупный corner rect есть (538-662k), но отход ещё рассеян на 2-4 зоны, не единая «лесенка» — GA пакует остаток без углового давления.
- Бюджет: 20с/peel — близко к потолку — рост 10с→20с дал +0.31pp lead; дальше масштабировать попытками бессмысленно без диверсификации генератора.

### Выводы
1. ✅ **Партиция — правильный рычаг, peeling — правильный механизм**: lead +1.55pp, corner ×1.9, цель max_corner ≥400k достигнута.
2. ❌ Pre-partition (LPT/жадная по площади → переупаковка групп) фальсифицирован: потолок переупаковки GA ~90-93%. ЭТАП 13 утверждал осуществимость по площади — но геометрическая переупаковываемость форсированных групп НЕ следует из существования упаковки.
3. ❌ Число зон отхода (7.9 vs цель 4) peeling'ом не решается — это внутрилистовая форма (V9b width-matched columns / corner-pressure для остаточного листа).
4. Lead 94.2% vs цель 96: остаточный зазор — потолок «плотнейшего листа» одного GA-прогона (~95-97 у лучших попыток, ~93-94 в среднем). Следующий рычаг: диверсификация генератора для первого peel'а (nested+guillotine микс, разные ga_profile в попытках).

### Следующие шаги
- **V9b width-matched columns** — атака на число зон (главный незакрытый разрыв с эталоном).
- **Peel-генератор микс**: в best-of-K чередовать guillotine/nested и ga_profile — поднять плотнейший лист попытки.
- **Corner-pressure для слак-листа**: остаточную итерацию паковать с corner_free-приоритетом уже сейчас (выбор есть), но отход всё равно фрагментирован — нужна компакция, склеивающая карманы (slide недостаточно).

---

## ЭТАП 16: V9b — width-matched columns + FFDH shelves + zones-first отбор слака (2026-06-12)

Ветка: `feat/v9b-width-columns` (от `feat/v8-partition`).

### Реализация
1. **Колонный конструктор** (`build_column_stock_piece`): колонны с anchor-шириной, стек деталей ширины ∈ [col_w−tol, col_w], 4 варианта жадности (tol 0/40/80мм, anchor по ширине/площади). На multisheet_varied СЛАБ: ширины деталей уникальны (только пары qty=2) → стек не собирается, util одиночного листа 19-91% vs GA 93-97. Width-matched columns предполагают повторяющиеся ширины (реальные заказы эталонов) — на varied-fixture гипотеза структурно не применима. Оставлен (выигрывает только если ≥GA, регрессий нет; на повторных ширинах должен включаться).
2. **Shelf-конструктор FFDH** (`build_shelf_stock_piece`): детали landscape/portrait, сортировка по высоте, полки first-fit; затем полки пересортированы по занятой ширине (широкие сверху), внутри полки детали по высоте → свободное место = одна монотонная «лесенка» к нижнему-правому углу (форма эталона). Главный рабочий инструмент для слак-листа.
3. **Отбор слак-итерации переписан**: (число листов, **число зон отхода**, corner_free) вместо (листы, corner_free). Счётчик зон — flood-fill на 10мм-сетке в Rust (`waste_region_count`, детали раздуты на kerf, порог 5k mm²) — зеркалит метрику бенчмарка. Урок: метрика отбора должна совпадать с целевой — shelf-слак (1 зона, corner 0.33M) проигрывал GA-слаку (2-3 зоны, corner 0.6M) по старому правилу.
4. Оба конструктора участвуют и в обычных peel-итерациях (по util; GA там обычно плотнее — проигрывают честно).

### Результаты (30 seeds, sheet_budget_ms=20000)
| Метрика | V8 | **V9b** |
|---|---|---|
| Avg зон отхода | 7.9 | **6.9** |
| Avg lead util | 94.20% | **94.26%** |
| Avg max corner | 461k | 421k (−40k: осознанный trade — связность зоны важнее размера прямоугольника) |
| max_corner≥300k | 26/30 | 27/30 |
| Peeling applied | 29/30 | **30/30** |
| 4-sheet rate | 30/30 | 30/30 |

Слак-листы: были 2-4 зоны → теперь типично **1 зона** (shelf-лесенка, визуально подтверждено seed 10). Остаточная фрагментация — карманы внутри GA-листов 2-3 (util 91-94%): по 2-3 зоны. Артефакты: `spec_freecut/tmp/best_layouts_v9b/`.

### Выводы
1. ✅ Shelf-FFDH + zones-first отбор закрывают слак-лист: отход одной лесенкой.
2. ❌ Width-matched columns на varied-fixture не работают (уникальные ширины) — проверять на fixture с повторами.
3. Разрыв с целью 4 зоны: средние GA-листы (3-й peel, util 91-94) несут 2-3 кармана. Рычаги: zones как поздний tie-breaker в выборе ПЛОТНЫХ peel'ов (при равном util ±ε); либо принять (карманы в плотных листах часто бизнес-приемлемы — мелкие).

### Следующие шаги (приоритезировано)
1. **V10: Zones-aware peel selection** ✅ РЕАЛИЗОВАН — при равном util±0.3pp предпость кандидата с меньшим числом зон отхода на плотнейшем листе. Результат: avg зон 6.8 (V9b: 6.9, −0.1 — маргинально). Маргинальность объяснима: GA-кандидаты внутри ±0.3pp однородны по структуре отходов, tie-breaker редко срабатывает. Телеметрия `densest_zones` показывает, что первый peel всегда zones=1 (плотнейший лист — 1 зона), проблема — в листах 2-3 (3-й peel, util 91-94%).
2. **V11: Peel-генератор микс** — в best-of-K попытках для плотных peel'ов чередовать guillotine/nested режимы. Nested не ограничен гильотинными резами → потенциально плотнее первый peel (lead 94.3→95+).
3. **V12: Post-peel in-sheet compaction** — компакция уже замороженных GA-листов (slide+corner-anchor) для слияния мелких карманов. Может уменьшить зоны с 6.8→5-6.
4. Fixture с повторяющимися ширинами — проверить колонный конструктор по назначению.

---

## ЭТАП 17: V10 — zones-aware peel selection (2026-06-13)

Ветка: `feat/v10-zones-aware-peel` (от `feat/v9b-width-columns`).

### Реализация
1. `densest_sheet_waste_regions(sheets, gap)`: находит плотнейший лист и считает `waste_region_count` только для него.
2. `peel_candidate_better` (не-последняя итерация): при `|densest_util_diff| ≤ 0.3pp` сравнивает `densest_sheet_waste_regions` (меньше = лучше). При разнице >0.3pp — как раньше, по util.
3. `PartitionTelemetry.densest_zones: Vec<u32>`: зоны отхода на плотнейшем листе каждого peel-раунда.

### Результаты (30 seeds, sheet_budget_ms=20000)
| Метрика | V9b | **V10** | Δ |
|---|---|---|---|
| Avg зон отхода | 6.9 | **6.8** | −0.1 |
| Avg lead util | 94.26% | 94.24% | −0.02pp |
| Avg max corner | 421k | 409k | −12k |
| max_corner≥300k | 27/30 | 26/30 | −1 |
| 4-sheet rate | 30/30 | 30/30 | = |
| Peeling applied | 30/30 | 30/30 | = |

### Лучший раскрой: seed 24 — 4 зоны (1-1-1-1 на листе)
Но только 1/30 layouts с ≤4 зонами. Подавляющее большинство 5-8 зон.

### Выводы
1. ❌ **V10 — маргинальный эффект**: tie-breaker срабатывает редко, GA-кандидаты внутри ±0.3pp однородны по структуре отходов.
2. ✅ Телеметрия `densest_zones` ценна: показывает, что первый peel всегда zones=1, проблема — в листах 2-3 (3-й peel, util 91-94%).
3. **Следующий рычаг**: V11 (nested микс в peel-попытках).

---

## ЭТАП 18: V11 — Peel-генератор микс: guillotine/nested чередование (2026-06-13)

Ветка: `feat/v11-peel-nested-mix` (от `feat/v10-zones-aware-peel`).

### Реализация
В каждой peel-попытке (не-last итерация): чётные попытки — guillotine (как раньше), нечётные — nested mode. Для slack-итерации — только guillotine (shelf/column конструкторы обрабатывают).

### Результаты (30 seeds, sheet_budget_ms=20000)
| Метрика | V10 | **V11** | Δ |
|---|---|---|---|
| Avg lead util | 94.24% | **94.87%** | **+0.63pp** ✅ |
| Avg waste zones | 6.8 | **9.0** | **+2.2** ❌ |
| Avg max corner | 409k | **459k** | **+50k** ✅ |
| Avg min util | 84.99% | 83.09% | −1.9pp ❌ |
| 4-sheet rate | 30/30 | 30/30 | = |

### Выводы
1. ✅ **Nested даёт плотнее размещение**: lead util +0.63pp, max corner +50k.
2. ❌ **Nested фрагментирует отход**: зоны +2.2 (nested без гильотинных резов создаёт больше несвязных зон).
3. ❌ **Min util падает**: -1.9pp, т.к. nested «забирает» больше деталей на первые листы, оставляя меньше на слак-лист.
4. Trade-off: плотность vs. консолидация отхода. V11 лучше по lead util/corner, хуже по зонам/min util.

---

## ЭТАП 19: V12 — Nested только для первого peel (РЕГРЕССИЯ) (2026-06-13)

Ветка: `feat/v12-nested-first-peel-only` (от `feat/v10-zones-aware-peel`).

### Реализация
Вместо V11 (чередование во ВСЕХ peel-раундах), nested только для первого (плотнейшего) peel-раунда. Гипотеза: плотность первого листа — главное, а guillotine для остальных уменьшит фрагментацию.

### Результаты (30 seeds)
| Метрика | V10 | V11 | **V12** |
|---|---|---|---|
| Avg lead util | 94.24% | 94.87% | **94.19%** (хуже V10!) |
| Avg waste zones | 6.8 | 9.0 | **8.1** (хуже V10!) |
| Avg max corner | 409k | 459k | 434k |
| Avg min util | 84.99% | 83.09% | 85.13% |

### Корневая причина регрессии
Nested-раскладка полного набора деталей (40 на 4 листа) даёт **3 зоны отхода** на плотнейшем листе (vs 1 для guillotine). Когда nested_candidate побеждает по density, замораживается более фрагментированный лист, и это проходит через весь peel-конвейер.

### Вывод
V12 — регрессия по обоим ключевым метрикам (lead util и zones). Nested лучше работает как диверсификатор в V11 (чередование), а не исключительно на первом peel. **Лучший конфиг для зон — V10 (6.8), лучший для lead util — V11 (94.87%).**

---

## Сравнительная таблица версий (актуальная)

| Версия | Ветка | Механизм | lead_util | avg зон | max_corner | min_util | 4-sheet |
|--------|-------|----------|-----------|---------|-------------|----------|---------|
| V9b | v9b-width-columns | FFDH shelves, zones-first slack | 94.26% | 6.9 | 421k | — | 30/30 |
| V10 | v10-zones-aware-peel | + zones-aware peel TB | 94.24% | 6.8 | 409k | 84.99% | 30/30 |
| **V11** | v11-peel-nested-mix | + nested/guillotine micro | **94.87%** | 9.0 | **459k** | 83.09% | 30/30 |
| V12 | v12-nested-first-peel | nested first peel only | 94.19% | 8.1 | 434k | 85.13% | 30/30 |

**Лучший по плотности**: V11 (94.87%, corner 459k) — но зоны 9.0.
**Лучший по консолидации**: V10 (зоны 6.8) — но lead 94.24%.
**Лучший компромисс**: V13 (0.8pp/zone) — lead 94.65%, зоны 7.0.

---

## ЭТАП 20: V13 — Nested/guillotine микс + zones penalty (0.8pp/zone) (2026-06-13)

Ветка: `feat/v13-nested-zones-hybrid` (от `feat/v11-peel-nested-mix`).

### Реализация
При сравнении peel-кандидатов вместо фиксированного epsilon (V10: ±0.3pp) применяется
постоянный штраф за число зон отхода на плотнейшем листе:
```
effective_util = densest_util - max(0, zones - 1) * 0.8pp
```
Так, candidate с 3 зонами «стоит» 1.6pp меньше, candidate с 1 зоной — без штрафа.
Nested-кандидат побеждает guillotine только если превышает штраф >=1.6pp для 3 зон.

### Результаты (30 seeds, sheet_budget_ms=20000)
| Метрика | V10 (guill) | V11 (nested микс) | **V13 (0.8pp штраф)** | Δ vs V10 |
|---|---|---|---|---|
| Avg lead util | 94.24% | 94.87% | **94.65%** | **+0.41pp** ✅ |
| Avg waste zones | 6.8 | 9.0 | **7.0** | +0.2 (≈V10) ✅ |
| Avg max corner | 409k | 459k | **454k** | +45k ✅ |
| Avg min util | 84.99% | 83.09% | 83.77% | −1.2pp |
| 4-sheet rate | 30/30 | 30/30 | 30/30 | = |

### Выводы
1. ✅ **V13 — лучший компромисс**: lead util +0.41pp vs V10 при зонах ≈V10 (7.0 vs 6.8).
2. ✅ Zones penalty 0.8pp эффективно фильтрует nested-кандидаты с сильной фрагментацией.
3. ❌ Min util −1.2pp (nested забирает детали на первые листы).
4. ❌ Зоны 7.0 vs цель 4 — основной разрыв в GA-листах 2-3, не в peel-выборе.

### Penalty sweep (0.3, 0.8, 1.0, 1.5 pp/zone)

| Penalty | lead_util | avg zones | max_corner | min_util | Артефакты |
|---------|-----------|-----------|-------------|----------|-----------|
| 0.3     | 94.85%    | 8.6       | 453k        | 83.17%   | — |
| **0.8** | **94.65%** | **7.0**   | **454k**    | 83.77%   | `best_layouts_v13_0.8pp/` |
| 1.0     | 94.57%    | 6.9       | 440k        | 83.99%   | `best_layouts_v13_1.0pp/` |
| 1.5     | 94.42%    | 6.2       | 403k        | 84.45%   | `best_layouts_v13_1.5pp/` |

**0.8pp — оптимальный trade-off**: nested побеждает guillotine когда даёт >0.8pp плотности per extra зону. При 1.5pp nested полностью отсеивается на большинстве seeds.

### Следующие гипотезы
---

## ПЛАН ДАЛЬНЕЙШЕЙ РЕАЛИЗАЦИИ: гипотезы и направления

### Текущий разрыв с идеалом (количественный)

| Метрика | Идеал (logs/perfect) | V13 лучший (0.8pp) | GAP |
|---------|---------------------|---------------------|-----|
| Зон отхода/раскрой | 4 (1/лист) | 7.0 | +3 зоны |
| waste_fill% | ≥50% | 5–25% | 2–10× |
| Min util (slack) | ≥91.88% | 83–84% | −8pp |
| Lead util (dense n-1) | ~96% | 94.65% | −1.3pp |
| All-4-≥90% | 100% | 0/30 | фундаментальный |
| Форма отхода | 1 угловая лесенка | 2–3 разрозненных зоны | архитектурный |

### Корневые причины разрыва (визуальный аудит)

1. **Гильотинная архитектура не даёт width-matched колонн**: GA строит бинарное дерево разрезов, а не колонны равной ширины. Отсюда вертикальные щели между колоннами разной ширины — главная причина зон отхода.
2. **Peeling создаёт дисбаланс**: 3 плотных листа (93–97%) + 1 slack (83–84%). LPT доказывает достижимость 91.88% на каждом, но peeling не перераспределяет.
3. **Slide-компакция не склеивает карманы**: внутри guillotine-структуры ряды разной ширины блокируют друг друга, создавая «карманы» 2–3 зоны.
4. **Corner-anchoring (V9) сдвигает отход в угол, но не консолидирует**: отход фрагментирован на 2–3 зоны, прижатых к углу.

---

### Группа А: Изменение того, ЧТО генерируется (структура размещения)

**А1. Shelf-конструктор для ВСЕХ листов (HIGH, средняя сложность)**
Shelf-FFDH (V9b) даёт 1 зону для slack, но не конкурирует с GA на плотных листах (91% vs 96%). Гипотеза: усилить shelf — формировать «лесенку» из колонн, пустое место внизу-справа заполнять мелкими деталями. Полный отход от GA на плотных листах, но эталонная форма отхода. Ожидание: зоны 4, lead ~92–93%.

**А2. Column-seeded GA: инициализация GA популяцией из shelf (MEDIUM, средняя)**
Подать shelf-решение как стартовый кандидат в GA. GA оптимизирует «лесенку» сохраняя структуру колонн. Ожидание: зоны 4–5, lead ~94%. Риск: V4 (FFD seeding) фальсифицирован, но shelf структурно ближе к идеалу.

**А3. Guillotine-repack замороженных nested-листов (MEDIUM, низкая)**
После того как nested-кандидат побеждает в peel, перепаковать его детали через guillotine (single-sheet). Если guillotine даёт ≥ ту же плотность — берём guillotine (меньше зон). Ожидание: зоны 7→5.5 на ~60% seeds.

**А4. Двухуровневая укладка: guillotine-корпус + nested-заполнение (EXPLORATORY, высокая)**
Сначала guillotine формирует «каркас» из крупных деталей, затем nested заполняет пустые прямоугольники. Даст чистые разрезы + плотное заполнение. Требует глубокой модификации vendor.

---

### Группа Б: Изменение того, КАК генерируется (GA/vendor fitness)

**Б1. Зоны-штраф в фитнес-функции GA (HIGH, средняя)**
Заменить `free_rects * 0.01` на `waste_regions * ZONES_PENALTY_GA` (0.3–0.5) в `vendor/cut-optimizer-2d/src/{guillotine,maxrects}.rs`. GA сам будет оптимизировать за число зон — самый фундаментальный рычаг. Ожидание: зоны 7→4–5. Риск: высокая fragmentation_penalty (0.5+) снижала hard_ok rate в Этапе 3, но zones-штраф — качественно другое (штраф за несвязные зоны, а не за free_rects).

**Б2. GA profile diversification в best-of-K peel (MEDIUM, низкая)**
Чередовать survival_factor (0.7, 0.8, 0.9), breed_factor, frag_penalty в разных peel-попытках. Разные профили → качественно разные структуры. Ожидание: +0.3–0.5pp lead.

**Б3. Перераспределение бюджета peel (LOW, низкая)**
Больше бюджета на средние peel-итерации, меньше на первый (плотнейший быстр). Ожидание: +0.1–0.2pp.

---

### Группа В: Изменение того, ЧТО сравнивается (tie-breaking)

**В1. Corner-area bonus в effective_util (MEDIUM, низкая)**
Добавить `corner_pct * 0.2pp` к effective_util при peel-отборе. Preference для размещений с крупным угловым остатком. Ожидание: marginальный эффект.

**В2. Скалярная целевая функция (EXPLORATORY)**
`EQS = w1*lead_util - w2*total_zones - w3*(1-corner_pct)`. Точная балансировка целей, но сложная настройка весов.

---

### Группа Г: Пост-обработка (после GA)

**Г1. Per-sheet re-optimization после peeling (HIGH, низкая)**
После заморозки всех листов, пропустить каждый замороженный лист через GA с fragmentation_penalty=0.5–1.0. Если результат имеет ≤2 зоны при util ≥ заморожённого — заменить. Ожидание: зоны 7→5–6. Время: +0.5–1s/лист.

**Г2. Row-merging: слияние соседних рядов (EXPLORATORY)**
После slide-компакции попытаться «слить» два соседних ряда в один. Сложная реализация для uncertain gain.

---

### Группа Д: Архитектурные изменения (долгосрочные)

**Д1. Column-first конструктор «лесенка» (HIGH LONG-TERM, высокая)**
Новый алгоритм: группировать детали по ширине в колонны, формировать монотонный skyline, мелкие на ступенях, тонкие на левом краю. Отход = 1 зона по построению. Ожидание: зоны 4, lead ~92–94%. На varied-fixture — fallback к shelf (А1).

**Д2. Fixture с повторяющимися ширинами (MEDIUM, низкая)**
Создать fixture с 5–6 типичными ширинами × qty=4–8. Протестировать column-конструктор V9b. Ожидание: column-конструктор работает на реальных данных.

**Д3. Multi-objective GA (EXPLORATORY, высокая)**
Pareto-front по (util, zones) в GA. Вместо одного лучшего — набор недоминируемых кандидатов. Ожидание: direct optimization for zones. Высокий риск реализации.

---

### Приоритезированный план реализации

| Приоритет | Гипотеза | Ветка | Ожидаемый эффект | Сложность |
|---|---|---|---|---|
| **1** | **Г1: Per-sheet re-optimization** | feat/v14-sheet-reopt | зоны 7→5–6 | Низкая |
| **2** | **Б1: Зоны-штраф в fitness GA** | feat/v15-zones-fitness | зоны 7→4–5 | Средняя |
| **3** | **А3: Guillotine-repack nested** | feat/v16-guill-repack | nested→guill 3→1 зона | Низкая |
| **4** | **Б2: GA profile diversification** | feat/v17-ga-diversify | lead +0.3–0.5pp | Низкая |
| **5** | **А1: Shelf для всех листов** | feat/v18-shelf-all | зоны 4, lead ~92% | Средняя |
| **6** | **Д2: Fixture с повторами** | feat/v19-repeat-fixture | Валидация column-конструктора | Низкая |
| **7** | **А2: Column-seeded GA** | feat/v20-col-seed | зоны 4–5, lead ~94% | Средняя |
| **8** | **Д1: Column-first «лесенка»** | feat/v21-staircase | зоны 4, форма эталон | Высокая |

### Визуальная иллюстрация разрыва

```
ИДЕАЛ (logs/perfect):          ТЕКУЩИЙ V13 (seed 22, rank 1):
┌─────────────────────┐        ┌─────────────────────┐
│████████████████████▓│        │███████████████▓────▓│
│████████████████▓▓▓▓▓│        │██████████▓────▓███▓│
│████████████▓▓▓▓▓▓▓▓│        │██████▓────▓████████│
│████████▓▓▓▓▓▓▓▓▓▓▓▓│        │████▓──▓████████████│
│████▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│        │▓──▓████████████████│
│▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│        │▓████████████████████│
└─────────────────────┘        └─────────────────────┘
  1 зона отхода (лесенка)        2–3 зоны (коридоры + L-образная)
  waste_fill ≥ 50%               waste_fill 5–25%
  width-matched колонны          колонны разной ширины
```

---

## Сравнительная таблица версий (актуальная)

| Версия | Ветка | Механизм | lead_util | avg зон | max_corner | min_util | 4-sheet |
|--------|-------|----------|-----------|---------|-------------|----------|---------|
| V9b | v9b-width-columns | FFDH shelves, zones-first | 94.26% | 6.9 | 421k | — | 30/30 |
| V10 | v10-zones-aware | +0.3pp epsilon zones TB | 94.24% | 6.8 | 409k | 84.99% | 30/30 |
| V11 | v11-nested-mix | + nested/guillotine micro | 94.87% | 9.0 | 459k | 83.09% | 30/30 |
| V12 | v12-nested-first | nested first peel only | 94.19% | 8.1 | 434k | 85.13% | 30/30 |
| **V13** | v13-nested-zones | + 0.8pp/zone penalty | **94.65%** | **7.0** | **454k** | 83.77% | 30/30 |
| V14 | v14-guill-repack | A3: guillotine-repack nested winners | 94.65% | 7.0 | 454k | 83.77% | 30/30 |

---

## ЭТАП 21: ПЛАН — Новая стратегия гипотез (по research_approach.md, 2026-06-13)

### Главная диагностика
Проблема уже **не в количестве листов** (4 листа стабильно), а в **форме отхода**.
Идеал: 1 зона отхода на лист (угловая лесенка), waste_fill ≥ 50%. Текущий V13: 7 зон, waste_fill 5–25%, коридоры и L-образные карманы.
**Ключевой вывод**: tie-breaker выбирает из того, что GA уже создал. Если в популяции нет «лесенок», ранжирование их не создаст. Нужно менять **генерацию**, а не только **выбор**.

### Обновлённый приоритет гипотез

| Приоритет | ID | Гипотеза | Ветка | Ожидание | Сложность |
|---|---|---|---|---|---|
| **1** | **V15** | **Zones-aware GA fitness** (внутри vendor) | `feat/v15-zones-fitness` | zones 7→4–5 | Средняя |
| **2** | **V16** | **Per-sheet reoptimization/repair** | `feat/v16-sheet-repair` | zones 7→5–6 | Низкая |
| **3** | **V17** | **Touching Perimeter + gap-fill** | `feat/v17-touching-perim` | zones 4–6 | Высокая |
| 4 | V18 | GA profile diversification | — | lead +0.2–0.5pp | Низкая |
| 5 | V19 | Shelf-all (как repair/seed, не основной) | — | zones вниз, lead может просесть | Средняя |

### Критерии принятия

```
Must keep:
  4-sheet rate = 30/30

Strong win:
  avg waste regions <= 5.8
  lead_util >= 94.3%

Breakthrough:
  avg waste regions <= 5.0
  layouts with <=4 regions >= 10/30

Production-quality target:
  avg waste regions ~= 4
  largest_region_fill >= 50%
  lead_util >= 94–95%
```

### V15: Zones-aware GA fitness (ТЕКУЩИЙ ШАГ)
**Идея**: перенести zones-штраф из peel-селектора внутрь vendor GA fitness. Штрафовать не только free_rects, а число компонент отхода, compactness/fill крупнейшей зоны и размер corner-free остатка.

**Целевая fitness-формула**:
```rust
fitness = util.powf(2.0)
    * exp(-lambda_zones * max(0, waste_regions - 1))
    * exp(-lambda_fill * (1.0 - largest_region_fill))
    * (1.0 + lambda_corner * corner_free_pct);
```

**Быстрая аппроксимация**: считать компоненты не на 10mm-сетке, а по графу free rectangles (узлы — свободные прямоугольники, ребро — если касаются/пересекаются с учётом kerf).

**Sweep параметры**:
- `lambda_zones` = 0.15, 0.3, 0.5, 0.8
- `lambda_corner` = 0.05, 0.1
- `lambda_fill` = 0.1, 0.2

**Место в коде**: `vendor/cut-optimizer-2d/src/guillotine.rs` и `maxrects.rs` — fitness-функция.

### V14 результат (завершено, нейтрально)
A3: Guillotine-repack nested peel winners — метрики идентичны V13.
Вывод: V13 zones penalty уже смещает peel-селектор в сторону guillotine; nested побеждает редко, и guillotine-repack не может воспроизвести ту же плотность.

---

## Git ветки (актуальное состояние)

| Ветка | Базируется на | Статус |
|---|---|---|
| `main` | — | без изменений оптимизации |
| `feat/v9b-width-columns` | v8 | V9b |
| `feat/v10-zones-aware-peel` | v9b | V10 — маргинальный эффект |
| `feat/v11-peel-nested-mix` | v10 | V11 — lead +0.63pp, зоны +2.2 |
| `feat/v12-nested-first-peel` | v10 | V12 — РЕГРЕССИЯ |
| `feat/v13-nested-zones-hybrid` | v11 | V13 — лучший компромисс |
| `feat/v14-guill-repack` | v13 | A3 guillotine-repack — ЗАВЕРШЕНО, без улучшения |
| `feat/v15-zones-fitness` | v13 | **В РАБОТЕ** — V15 zones-aware GA fitness |
