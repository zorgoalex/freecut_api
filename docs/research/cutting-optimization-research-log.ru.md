# Журнал исследований оптимизации раскроя

<!-- research-log-sync-index
goal
base-fixture
metrics
old-metric-issues
quality-frame
history-to-v28
v29-v33-group-shift
v34-geometry-penalties
v39-group-shift-visual
v40-corridor-audit
v41-zone-penalty-profile-pool
workspace-state
research-approach-conclusions
next-hypotheses-v42-v53
working-rules
v55-v58-performance
v59-v61-next
v63-packingsolver
v64-constructive-portfolio
v65-cpsat-verifier
v66-sat-feasibility
v67-ladder-benchmark
v59-v61-productionization-a-cut-quality
v59-v61-productionization-b-async-postprocess
v72-remnant-telemetry
-->

Language: Russian.

Дата сжатия: 2026-06-16.
Целевой размер: до 70k символов.
Назначение: быстрый рабочий контекст для дальнейших исследований 2D раскроя без потери важных выводов последних экспериментов.

## Цель

Freecut — Rust HTTP service для 2D раскроя прямоугольных деталей на листах. Нужно добиться максимально плотной и практически полезной раскладки:

- минимум листов;
- максимум плотности на рабочих листах;
- минимум внутренних пустот между деталями;
- остаток должен быть цельным, пригодным к повторному использованию, лучше у края/угла;
- допустимы `guillotine` и `nested`, но сравнивать их нужно по одинаковым визуальным метрикам;
- все новые гипотезы тестировать в отдельных ветках от `main`;
- лучшие SVG/PNG складывать только в `ai_docs/tmp`, не в `C:/tmp`.

Главное уточнение пользователя: визуально качественный раскрой — это не просто меньше математических зон отхода. Цель — получить одну плотную группу деталей без внутренних коридоров, чтобы полезный остаток был не разорван, а вытеснен наружу, желательно к одному краю/углу. Если несколько крайних деталей можно сдвинуть группой к основной массе и убрать внутренний gap, это может быть ценнее, чем формальная привязка к краю листа.

## Базовый fixture и теоретический предел

Основной benchmark: `tests/fixtures/multisheet_varied_4sheets.json`.

Параметры:

- лист: 2070 x 2800 мм;
- trim: 10 мм со всех сторон;
- usable area: 2050 x 2780 = 5.699M мм²;
- суммарная площадь деталей: около 20.96M мм²;
- 3 листа невозможны по площади;
- 4 листа — математический минимум;
- суммарный отход при 4 листах фиксирован примерно 1.84M мм².

Значит, на этом fixture главная борьба уже не за количество листов, а за форму и пригодность остатка. Идеальная цель: 4 листа, на каждом листе 1 визуальная зона отхода, плотные листы около 94-96% util, slack распределён так, чтобы не создавать внутренних дыр.

## Основные метрики

Используемые обозначения:

- `sheets` — число использованных листов. Главная hard-метрика.
- `lead_util` — средняя утилизация всех листов, кроме самого слабого/slack. Для dense-first важнее обычного среднего.
- `min_util` — утилизация самого слабого листа. Полезна, но не должна вытеснять форму остатка.
- `waste_percent` — общий отход. На фиксированных 4 листах почти константа.
- `waste_regions_kerf` — число зон отхода с kerf/spacing inflation. Полезно для технологического зазора, плохо для визуальной оценки.
- `visual_waste_regions` — число зон отхода без kerf inflation или с очень мягкой inflation. Лучше совпадает с глазом.
- `per_sheet_zones` — зоны по каждому листу. Целевая форма: `[1,1,1,1]`.
- `max_corner_mm2` / `corner_free` — максимальный угловой прямоугольник. Полезно, но само по себе не гарантирует отсутствие внутренних коридоров.
- `bbox_void_area` / `bbox_area` — пустота внутри bbox деталей. Полезный сигнал для плотности группы, но может конфликтовать с остатком у края.
- `group_shift.closed_area` — площадь закрытых/сдвинутых коридоров. Это локальная метрика улучшения, а не полная метрика качества раскроя.
- `time_ms` — цена решения. Нужно считать отдельно для base, profile_pool, group_shift, rescue.

## Ошибки старых метрик

В ходе V29-V41 выяснилось, что несколько метрик систематически расходятся с визуальной оценкой.

1. `waste_region_count` с kerf inflation завышает фрагментацию.
   Если детали сдвинулись и визуально gap исчез, между ними всё равно остаётся kerf/spacing. Flood-fill с inflation может разрезать отход на дополнительные зоны и показать ухудшение. Это было главным источником неверного вывода "group_shift нейтрален/хуже".

2. `max_edge_gap` вреден как цель.
   Он поощряет размазывание пустоты по периметру, а бизнес-цель обратная: один крупный пригодный остаток.

3. Одно только `visual_waste_regions` тоже недостаточно.
   V41c показывает: candidate может получить 4 зоны вместо 5, но выглядеть рыхлее и терять lead_util. Значит, зоны должны быть combined metric, а не единственный winner rule.

4. Старый `corridor_score` оказался почти всегда около 1.0.
   Причина: guillotine naturally порождает длинные коридорные формы отхода. Метрика стала насыщенной и перестала различать хорошие/плохие случаи.

5. `compactness = largest_zone_area / bbox_area` и `largest_zone_fraction` слишком грубые.
   V39/V40 показали микроскопические изменения при визуально заметных локальных сдвигах. Они не видят, куда именно ушёл gap: внутрь группы или к краю.

6. `lead_util`/`min_util` без визуального guard приводит к плотному хаосу.
   Высокая плотность нужна, но не ценой внутренних дыр и разорванного остатка.

## Новая рамка оценки качества

Дальше метрики нужно строить в несколько уровней:

1. Hard objective:
   - нет unplaced;
   - минимальное число листов;
   - соблюдены kerf, spacing, trim, rotation, pattern constraints.

2. Density guard:
   - не принимать визуально красивый раскрой, если он заметно теряет плотность;
   - текущий рабочий guard: `max_lead_drop_pp = 0.8`, но V41c seed 13 показывает, что иногда 0.8-1.2pp визуально уже дорого.

3. Visual topology:
   - `per_sheet_zones <= 1` ideally;
   - считать без kerf inflation или с soft gap <= 1 мм;
   - отдельно считать площадь вторичных зон, а не только их количество.

4. Remnant usability:
   - большая boundary-connected зона лучше внутреннего "окна";
   - крупный corner/edge-connected rect лучше длинной щели;
   - secondary zones должны иметь минимальную площадь;
   - внутренние компоненты отхода должны штрафоваться сильнее крайних.

5. Cluster compactness:
   - `cluster_bbox_density = used_area / bbox(placements)`;
   - `internal_gap_area = bbox(placements) - used_area - boundary-connected free inside bbox`;
   - оценивать, можно ли сдвинуть группу деталей к углу без ухудшения constraints.

Кандидатная формула для следующего аудита, не final:

```text
visual_loss =
  1000 * extra_sheets
  + 80 * sum(max(0, per_sheet_zones - 1))
  + 60 * secondary_waste_area_ratio
  + 45 * internal_void_area_ratio
  + 25 * thin_internal_corridor_ratio
  + 20 * max(0, lead_drop_pp - lead_guard_pp)
  - 30 * largest_boundary_connected_waste_ratio
  - 20 * largest_corner_rect_ratio
  - 15 * cluster_bbox_density
```

Нужна калибровка на визуально размеченных SVG/PNG. Формула должна не заменять визуальный аудит, а быть проверена против него.

## Сжатая история до V28

V8: iterative peeling подтвердился.
Форсированная pre-partition по площади была фальсифицирована: геометрически группа может не перепаковаться даже при подходящей площади. Рабочий подход: GA сам выбирает плотнейший лист, он замораживается, остаток переоптимизируется. Это стабильно даёт 4 листа и dense-first поведение.

V9/V9.1: corner anchoring и `corner_free` tie-breaker были полезны как телеметрия, но не дали сильного улучшения. Главный урок: притягивать отход к углу недостаточно, нужно убирать внутренние карманы.

V9b: width-matched columns / FFDH shelves.
Shelf на slack-листе визуально даёт хорошую "лесенку" и одну зону отхода, но проигрывает GA на плотных листах. Полезен как repair/seed, но не как основной генератор для всех листов.

V10: zones-aware peel selection дал только маргинальный эффект.
Tie-breaker выбирает только из того, что уже сгенерировано. Если популяция не содержит хорошую структуру остатка, selection её не создаст.

V11/V12: nested/guillotine mix.
Nested может повысить плотность, но часто увеличивает фрагментацию отхода. Nested только на первом peel оказался регрессией. Вывод: nested нужен как candidate, но с сильным visual guard.

V13: nested/guillotine mix + zones penalty 0.8pp/zone.
Это был важный ранний компромисс: 30/30 в 4 листа, lead около 94.65%, но zones около 7. Главный разрыв до идеала был не в количестве листов, а в форме остатка.

V15-V17: zones-aware GA/vendor fitness.
Перенос zone pressure внутрь генерации был правильным направлением, но простые штрафы в GA быстро выходят на плато. Важный результат: `profile_pool` лучше одного фиксированного penalty.

V17c/V20/V22: profile_pool.
Пул профилей zone_penalty стал лучшим практическим механизмом: несколько кандидатов с разным pressure, затем выбор по качеству. V22 `[0.2,0.3,0.4,0.5]`, guard 0.8 стал сильным baseline.

V23-V27: visual audit, adaptive policies, presets.
Подтверждено: режимы должны быть preset-driven. Нельзя пытаться одним default одновременно максимизировать lead и визуальную форму. Нужны cheap/balanced/aggressive режимы, где aggressive платит временем за качество остатка.

V28: group-shift / anchor-attraction audit.
Найдено много opportunities, включая cases где нужно сдвигать не одну деталь, а группу. Это совпало с пользовательской гипотезой: крайние детали должны притягиваться к основной плотной группе, а не обязательно к краю листа.

## V29-V33: group_shift

V29: реализован opt-in `group_shift` postprocess.

Смысл:

- после основной раскладки искать коридоры/gaps;
- определять группу деталей по одну сторону gap;
- сдвигать группу целиком к anchor/group;
- принимать только feasible moves.

Quick run показал, что pass реально двигает детали и закрывает gaps. Визуально это выглядело лучше, особенно там, где одна деталь только переносит коридор дальше, а группа деталей выдавливает gap к краю.

V30: добавлена telemetry:

- `summary.group_shift.time_ms`;
- before/after opportunity metrics;
- closed corridor area;
- runtime был приемлемым на быстрых тестах.

V31: paired same-run benchmark + SVG diff artifacts.

Артефакты:

- `ai_docs/tmp/best_layouts_v31_paired_group_shift_diff_quick/seed_08_moves4_closed498280_before.svg`
- `ai_docs/tmp/best_layouts_v31_paired_group_shift_diff_quick/seed_08_moves4_closed498280_after.svg`
- `ai_docs/tmp/best_layouts_v31_paired_group_shift_diff_quick/seed_08_moves4_closed498280_diff.svg`

Визуальный вывод по seed 08: group_shift делает реальные локальные сдвиги и закрывает часть внутренних промежутков, но на total zones это может почти не влиять. Значит, оценивать его только через `waste_regions` нельзя.

V32: group_shift-aware profile_pool scoring.

Идея: включить residual/delta от group_shift в scoring pool. Результат на реальном fixture: 0/30 изменений winner. Причина: критерий не был связан с тем, что глаз считает улучшением, а profile_pool уже выбирал похожие candidates.

V33: anchor-component group_shift candidates.

Добавлены кандидаты не только от cutline side-groups, но и от anchor components. Synthetic case улучшился, production 30-seed benchmark почти не поменялся. V33 слит в `main` ранее: merge commit `c0d4656`.

Текущий вывод по V29-V33:

- group_shift не надо считать провалом;
- старые метрики его недооценили;
- включать `group_shift` blindly в winner scoring нельзя;
- следующий правильный шаг — paired benchmark с новой remnant/cluster метрикой;
- принимать сдвиг нужно не по zones, а по уменьшению внутренних gaps, secondary waste area и росту cluster compactness при сохранении lead/feasibility.

## V34/V34b: geometry penalties в GA

V34: corner concentration penalty.
Пытался давить на форму отхода внутри GA fitness. Результат не дал стабильного улучшения.

V34b: skyline monotonicity penalty.
Идея: поощрять "лесенку" отхода. Были найдены важные bugs:

- grid scale mismatch: `grid=10` при координатах x1000 создавал огромные массивы и тормоза; исправление: `grid=10_000` как 10 мм;
- направление monotonicity было инвертировано: нужно non-increasing bottom profile для целевой staircase.

После fixes overhead был около 3%, но стабильного качества не появилось.

Вывод: сырые GA fitness penalties по геометрии отхода пока тупик. Без правильно откалиброванной визуальной метрики они либо не влияют, либо ухудшают плотность/сходимость.

## V39: переоценка group_shift и visual metric

V39-reaudit: 10 seeds, profile_pool `[0.2,0.3,0.4,0.5]`, guard 0.8, partition.

Сравнение:

- Run A: `group_shift.enabled=false`;
- Run B: `group_shift.enabled=true`, `min_shift=5mm`, `max_passes=4`;
- метрики: zones с kerf, zones без kerf, compactness, largest_zone_fraction.

Результаты:

| Метрика | Улучшено | Ухудшено | Нейтрально | Avg delta |
|---|---:|---:|---:|---:|
| zones с kerf | 0 | 10 | 0 | +1.10 |
| zones без kerf | 1 | 2 | 7 | +0.20 |
| compactness | 1 | 0 | 9 | +0.0006 |
| largest_zone_frac | 1 | 1 | 8 | +0.0073 |
| коридоры закрыты | - | - | - | 138,595 мм²/seed |

Ключевой диагноз:

- kerf-inflated metric показывает ухудшение там, где визуально gap закрывается;
- без kerf результат почти нейтральный по zones;
- group_shift влияет на локальную форму и внутренние промежутки, но не обязательно уменьшает число связных компонент;
- вывод V29-V33 "нейтрально" был основан на неподходящей метрике.

V39 full benchmark: visual waste metric в оптимизации, 15 seeds.

| Config | lead_util | wr_kerf | wr_visual | vis_zones | <=4_vis | <=5_vis | compact |
|---|---:|---:|---:|---:|---:|---:|---:|
| baseline no gs | 94.40% | 4.8 | 4.6 | 4.6 | 6/15 | 15/15 | 0.1467 |
| gs_on | 94.44% | 5.8 | 5.6 | 4.7 | 0/15 | 6/15 | 0.1468 |

V39 definitive 30-seed:

| Config | lead | wr_visual | vis_zones | <=4_vis | <=5_vis |
|---|---:|---:|---:|---:|---:|
| baseline | 94.44% | 4.6 | 4.6 | 11/30 | 30/30 |
| gs_on | 94.45% | 5.6 | 4.8 | 0/30 | 11/30 |

Вывод:

- visual metric без kerf лучше старой;
- `group_shift` still not good as global on/off;
- но это не доказывает бесполезность group_shift, а доказывает, что его acceptance/scoring надо привязать к другой цели: локальная компактность группы и usable remnant.

## V40: corridor/compactness audit

20 seeds, baseline vs `gs_on`.

| Config | lead | zones | corridor_score | compactness | largest_frac | <=4 | <=5 |
|---|---:|---:|---:|---:|---:|---:|---:|
| baseline | 94.40% | 4.6 | 0.999 | 0.0717 | 0.9728 | 8/20 | 20/20 |
| gs_on | 94.44% | 4.8 | 0.997 | 0.0708 | 0.9681 | 5/20 | 20/20 |

Критическое открытие:

- `corridor_score` насыщается около 1.0 почти для всех guillotine раскроев;
- guillotine naturally создаёт длинные коридорные waste components;
- `largest_frac` около 0.97, то есть почти весь отход уже в крупнейшей зоне;
- разница между 4 и 5 zones часто вызвана одной маленькой вторичной зоной на одном листе.

Вывод: нужно считать не "вся waste-площадь коридорная или нет", а:

- площадь вторичных зон;
- является ли вторичная зона внутренней или edge-connected;
- можно ли устранить её сдвигом группы или переносом одной детали;
- ухудшился ли при этом `lead_util`.

## V41: expanded zone_penalty profile_pool

Гипотеза: baseline pool `[0.2,0.3,0.4,0.5]` не всегда генерирует candidate с 1 waste-zone per sheet. Добавление агрессивных penalties `[0.6,0.8]` может найти более чистую топологию.

V41a: single penalty sweep на 12 seeds с 5 zones.

| zone_penalty | <=4 zones | avg zones | avg lead | remarks |
|---:|---:|---:|---:|---|
| 0.6 | 5/12 | 4.8 | 94.34% | seed1 -> 7 zones |
| 0.8 | 3/12 | 4.9 | 94.46% | neutral/mixed |
| 1.0 | 5/12 | 4.6 | 94.01% | lead страдает |
| 1.5 | fail | - | - | GA плохо находит solution |
| 2.0 | fail | - | - | GA плохо находит solution |
| no_partition | 0/12 | 7.8 | 92.37% | без partition плохо |

V41b: expanded pool `[0.2,0.3,0.4,0.5,0.6,0.8]` vs baseline.

| Config | lead | zones | <=4 | max/sheet |
|---|---:|---:|---:|---:|
| baseline `[0.2-0.5]` | 94.44% | 4.6 | 11/30 | 1.63 |
| expanded `[0.2-0.8]` | 94.38% | 4.4 | 18/30 | 1.40 |

Корректура:

- 7 seeds улучшились 5 -> 4 zones;
- 0 регрессий по zones;
- цена около -0.07pp lead;
- 18/30 <=4 против 11/30 у baseline.

V41c: new aggressive defaults + rescue, 30 seeds.

Сырые артефакты:

- `ai_docs/tmp/v41c_defaults/v41c_defaults_results.json`
- `ai_docs/tmp/v41c_visual_artifacts/seed_11_old.svg`
- `ai_docs/tmp/v41c_visual_artifacts/seed_11_new.svg`
- `ai_docs/tmp/v41c_visual_artifacts/seed_13_old.svg`
- `ai_docs/tmp/v41c_visual_artifacts/seed_13_new.svg`

Сводка V41c:

| Config | n | lead | zones | max/sheet | <=4 | <=5 |
|---|---:|---:|---:|---:|---:|---:|
| old defaults 4 penalties | 30 | 94.32% | 4.33 | 1.33 | 20/30 | 30/30 |
| new defaults 6 + rescue | 30 | 94.25% | 4.23 | 1.23 | 23/30 | 30/30 |

Comparison:

- better zones: 3;
- same zones: 27;
- worse zones: 0;
- avg lead delta: -0.07pp.

Визуальный аудит V41c:

- seed 11 new: улучшение 5 -> 4 zones выглядит в целом адекватно; остаток становится менее фрагментированным, цена по lead умеренная.
- seed 13 new: формально 5 -> 4 zones, но визуально раскрой выглядит рыхлее, а lead падает заметно сильнее. Это доказывает, что `zones` не может быть единственным winner rule.

Текущий best confirmed по числам: V41c aggressive defaults дают 23/30 раскроев с <=4 visual zones на этом прогоне, без zone regressions и с ценой около -0.07pp lead. Но перед merge нужно проверить runtime и визуальный remnant score, потому что "4 zones" иногда покупаются рыхлой геометрией.

## Текущий git/workspace state

На момент сжатия документа:

- `main` был синхронизирован с remote на merge V33: `c0d4656`;
- текущая рабочая ветка: `feat/v34-boundary-monotonicity`;
- в ветке есть коммиты V34/V34b/V39 поверх main;
- есть незакоммиченные правки V41c defaults в `src/optimizer.rs` и `src/tests.rs`;
- есть untracked scripts: `scripts/test_v39_definitive.py`, `scripts/test_v40_corridor_score.py`, `scripts/test_v41_zone_penalty_sweep.py`, `scripts/test_v41b_expanded_pool.py`, `scripts/test_v41c_defaults.py`;
- `ai_docs/CONTEXT_v2.md` и `ai_docs/tmp/` сейчас ignored, поэтому их нужно добавлять в git только осознанно.

Важно: не смешивать cleanup документа с кодовыми гипотезами. Для новой кодовой гипотезы стартовать отдельную ветку от `main` или отдельный worktree.

## Выводы по research_approach.md

Общее направление research_approach подтвердилось частично:

Подтвердилось:

- проблема уже не в количестве листов, а в топологии остатка;
- profile_pool сильнее одиночного penalty;
- zones/visual scoring нужен внутри отбора candidates, а не только после;
- shelf/staircase полезны как эталон формы/repair, но не как основной плотный packing;
- group/anchor-shift как принцип визуально осмыслен.

Уточнено/исправлено:

- "меньше zones" не всегда равно "лучше";
- kerf-inflated zones нельзя использовать как visual metric;
- corridor_score в старом виде непригоден;
- corner pressure без внутренней compactness не решает проблему;
- nested нужно держать под visual guard, иначе оно часто повышает фрагментацию.

Сильнейший текущий путь: не ещё один общий GA penalty, а связка:

1. expanded profile_pool генерирует больше структурно разных candidates;
2. visual/remnant score выбирает candidate с пригодным остатком;
3. group_shift/inter-sheet repair применяются только там, где paired score улучшается.

## Следующие гипотезы

### V42: visual remnant metrics audit

Цель: построить правильную оценку, которая совпадает с визуальным качеством.

Что сделать:

- отдельная ветка от `main`: `feat/v42-visual-remnant-metrics`;
- offline script для JSON/SVG layouts;
- считать `visual_waste_regions` gap=0 и soft gap=1mm;
- считать `secondary_waste_area_ratio`;
- считать `internal_void_area_ratio`;
- считать `largest_boundary_connected_waste_ratio`;
- считать `largest_corner_rect_ratio`;
- считать `cluster_bbox_density`;
- сравнить V31 group_shift before/after, V41c old/new, V22/V41b top layouts;
- сохранить SVG/PNG и metrics JSON в `ai_docs/tmp/v42_visual_remnant_metrics`.

Acceptance:

- метрика должна правильно ранжировать минимум 2-3 визуально очевидные пары;
- seed 13 V41c должен показать trade-off: zones лучше, но compactness/lead хуже;
- group_shift before/after должен получить credit за локальное закрытие gap, даже если zones не изменились.

V42 промежуточный результат (2026-06-16):

- Ветка: `feat/v42-visual-remnant-metrics`, base = `origin/main` (`c0d4656`).
- Скрипт: `scripts/test_v42_visual_remnant_metrics.py`.
- Артефакты: `ai_docs/tmp/v42_visual_remnant_metrics/v42_metrics.json`, `v42_metrics.csv`, `v42_summary.md`.
- Важная correction: API placements уже в координатах usable-area; старые Python metrics иногда вычитали `trim.left/top`, что могло давать расхождение с API/SVG.
- V41c seed 11: 5 -> 4 zones, lead -0.38pp, secondary/internal waste -0.028 ratio; read = metric agrees with visual improvement.
- V41c seed 13: 5 -> 4 zones, но lead -1.21pp; read = zones improve but lead guard fails. Это подтверждает, что zones-only метрика переоценивает такой кандидат.
- V31 group_shift seed 08: zones не изменились (9 -> 9), но combined shape score слегка улучшился. Это подтверждает, что group_shift может улучшать форму без уменьшения zone count.
- V30 seed2 off/on: no clear improvement; group_shift не должен быть глобальным on/off без move-level acceptance.

Следующий шаг: усилить V42 score не только paired-read, а формально: hard guard по lead drop перед zones/remnant score. Для V43 нельзя выбирать 4-zone candidate, если он проваливает density guard, даже если `visual_loss` ниже.

V42b intermediate: добавлена метрика `part_contact_ratio` (near-contact perimeter между деталями при gap <= 7 мм).

- V31 group_shift seed 08: zones 9 -> 9, contact +0.007, loss -0.21. Это уже математически ловит визуально заметное сближение деталей.
- V30 seed2: zones 11 -> 11, contact +0.007, loss -0.17. Раньше это выглядело как 0 uplift; теперь видно локальное улучшение формы без изменения zones.
- V41c seed 11: zones 5 -> 4, lead -0.38pp, contact +0.009; acceptable.
- V41c seed 13: zones 5 -> 4, lead -1.21pp, contact +0.010; still must be rejected/flagged by lead guard despite better topology.

Вывод: для group_shift/anchor-shift нужна не только waste metric, но и contact/cluster metric. Пользовательская гипотеза про сдвиг группы к основной массе подтверждается: эффект проявляется как рост contact ratio, а не обязательно как уменьшение waste zones.

### V43: remnant score в profile_pool selection

Цель: заменить zones-only выбор на combined scoring.

База:

- V41 expanded pool `[0.2,0.3,0.4,0.5,0.6,0.8]`;
- aggressive preset с rescue `[0.8,1.0]` только если zones > 4.

Идея:

- сначала hard filter по sheets и feasibility;
- потом guard по lead drop;
- потом score: zones + secondary area + internal void + boundary/corner remnant;
- отдельно логировать, когда winner отличается от zones-only.

Риск:

- слишком сложный score может переобучиться на один fixture;
- нужен paired visual audit, не только aggregate.

V43 intermediate (2026-06-16):

- Ветка: `feat/v43-profile-pool-hard-lead-guard`, base = `origin/main` (`c0d4656`).
- Минимальный кодовый шаг: убрать bypass `candidate.waste_regions <= 4` из eligibility в `profile_pool_winner_idx`.
- Причина: V42 seed 13 показал false-positive — 4 zones не должны побеждать, если lead падает сильнее `max_lead_drop_pp`.
- Добавлены unit tests:
  - 4-zone candidate ниже lead guard отклоняется;
  - 4-zone candidate внутри lead guard всё ещё побеждает 5-zone candidate.
- Это ещё не full remnant score. Это hard safety guard, который нужен до V43/V44 scoring.
- Empirical smoke на seed 13 с explicit pool `[0.2,0.3,0.4,0.5,0.6,0.8]`: winner стал `zone_penalty=0.4`, `waste_regions=5`, `lead=94.60%`. Низкоплотный 4-zone candidate из V41c больше не проходит.
- Артефакты: `ai_docs/tmp/v43_hard_lead_guard/seed_13_v43_hard_guard.{json,svg,png}`.
- Визуальный вывод: V43 выбирает более плотный 5-zone вариант вместо рыхлого 4-zone варианта. Это подтверждает hard lead guard.

### V44: group_shift acceptance by remnant score

Цель: переоценить V29-V33 не как global on/off, а как local repair.

Что изменить:

- group_shift генерирует moves;
- каждый move оценивается before/after по V42 metrics;
- принимать move, если уменьшается internal gap/secondary area и не растёт bad fragmentation;
- не требовать уменьшения total zones;
- добавить debug artifacts for accepted/rejected top moves.

Ожидание:

- group_shift должен стать полезным как late repair, особенно на cases с явным gap между основной группой и крайними деталями;
- возможный uplift не в avg zones, а в remnant usability / visual score.

V44 intermediate (2026-06-16):

- Ветка: `feat/v44-group-shift-contact-score`, base = `origin/main`.
- Подтверждена проблема V29-V33: move-level выбор по `corridor_closed_area_mm2` может предпочесть большой, но визуально слабый сдвиг вместо меньшего сдвига, который реально притягивает периферийную деталь к основной группе.
- Добавлена telemetry-метрика `group_shift.contact_gain_mm`.
- Внутри выбора group_shift move теперь приоритет: targeted `contact_gain_mm` -> `corridor_closed_area_mm2` -> shift -> tie-breakers.
- Важная деталь: contact считается не как общий прирост контактов по всему листу, а как прирост контакта выбранной группы с anchor-компонентом, если anchor-компонент найден. Это ближе к пользовательской визуальной цели: не просто двигать детали к краю, а сближать крайние детали с плотной основной массой.
- Добавлен unit test `group_shift_prefers_contact_gain_over_large_low_contact_corridor`: большой low-contact corridor больше не должен выигрывать у меньшего сдвига, который даёт контакт с anchor-группой.
- Проверка: `cargo test group_shift -- --test-threads=1` passed (8 tests).
- Вывод: гипотеза group_shift остаётся сильной, но её нужно оценивать через contact/anchor metrics, а не только через zones или raw corridor area. Следующий практический шаг: вынести этот score в paired benchmark/SVG artifacts и затем использовать внутри `profile_pool` scoring.

### V45: one-bad-sheet repair / inter-sheet zone merge

Цель: когда итог 5 zones = один лист имеет 2 зоны, попытаться убрать вторичную зону.

Механика:

- найти лист с `per_sheet_zones > 1`;
- определить детали, создающие вторичную зону;
- попробовать перенести 1 деталь на slack/compatible sheet;
- перепаковать два затронутых листа;
- принять только если sheets не растут, lead drop в guard, secondary area уменьшается.

Это продолжает V39 plan, но с правильными metrics.

### V46: parity tests for visual metrics

Цель: устранить расхождение API/Rust vs Python metrics.

Проблема:

- V39 показал, что API `wr_visual` и Python `vis_zones` могут отличаться из-за разных coordinate spaces / gap handling.

Что сделать:

- один shared definition: trim-adjusted mm coordinates;
- explicit gap mode: `kerf`, `visual0`, `soft1`;
- unit tests на простых layouts;
- snapshot tests на V41c seed 11/13 JSON.

V46 intermediate (2026-06-16):

- Ветка: `feat/v46-visual-metric-parity`, base = `origin/main`.
- Добавлены synthetic unit tests для двух assumptions, из-за которых прошлые Python-аудиты давали спорные выводы:
  - `response_stock_pieces_keep_placements_in_usable_area_coordinates`: API placements уже находятся в usable-area coordinates; trim нельзя вычитать второй раз.
  - `waste_region_count_separates_visual_gap_from_cut_clearance_gap`: `gap=0` и cut-clearance gap могут дать разное количество waste regions на одном и том же визуальном коридоре.
- Проверка: оба targeted tests passed.
- Вывод: текущий Rust `response_waste_regions(resp, gap_mm)` считает не чистую визуальную fragmentation metric, а cut-clearance fragmentation metric, потому что детали inflated by `gap`. Это не “плохо”, но эту метрику нельзя напрямую сравнивать с ручной визуальной оценкой без явного `gap_mode`.
- Практическое правило для следующих benchmark: всегда логировать минимум две метрики рядом: `zones_visual0` и `zones_cut_gap`. Если они расходятся, решение нельзя автоматически считать лучше/хуже без visual audit.

### V47: visual benchmark set

Цель: перестать оценивать только по одному fixture.

Набор:

- текущий `multisheet_varied_4sheets`;
- 2-3 эталонных layouts из `logs/perfect`, если доступны;
- synthetic group_shift case с gap corridor;
- synthetic false-positive case, где zones меньше, но раскрой визуально хуже.

Acceptance:

- новый score должен совпасть с ручной визуальной оценкой на этих cases;
- если не совпадает, фиксировать fail в `CONTEXT_v2.md`, а не двигать код дальше.

V47 intermediate (2026-06-16):

- Ветка: `feat/v47-dual-gap-visual-benchmark`, base = `origin/main`.
- Добавлен reproducible script `scripts/test_v47_dual_gap_visual_benchmark.py`.
- Скрипт читает реальные сохранённые JSON layouts из `ai_docs/tmp` и считает рядом:
  - `zones_visual0` без inflation;
  - `zones_cut_gap` с `cut_gap_mm=7.0`;
  - per-case lead/min util и telemetry snippets.
- Артефакты: `ai_docs/tmp/v47_dual_gap_visual_benchmark/v47_dual_gap_metrics.{json,csv}` и `v47_summary.md`.
- Результат на 8 layouts: 2 gap-sensitive cases.
  - `v43_seed13_hard_guard`: visual0=6, cut_gap=5. Важно: cut-gap может не только увеличивать zones, но и уменьшать их, если inflation съедает малую визуальную область ниже/около cutoff.
  - `v31_seed08_group_shift`: visual0=9, cut_gap=10. Здесь cut clearance делает fragmentation хуже, чем визуальный score.
- Вывод: V46 подтвердился на реальных артефактах. В будущих сравнениях нельзя писать просто `zones=N`; нужно указывать mode. Для UX/визуальной оценки важнее `zones_visual0` + contact/anchor metrics; для технологического clearance риска нужен `zones_cut_gap`.

### V48: profile_pool dual-zone telemetry

Цель: перестать терять различие visual zones и cut-clearance zones в API telemetry.

V48 intermediate (2026-06-16):

- Ветка: `feat/v48-profile-pool-dual-zone-telemetry`, base = `origin/main`.
- Добавлено поле `winner_visual_waste_regions` в `ProfilePoolTelemetry`.
- Внутренний `ProfilePoolCandidate` теперь хранит:
  - `visual_waste_regions = response_waste_regions(response, 0.0)`;
  - `waste_regions = response_waste_regions(response, kerf+spacing)`.
- Selection logic не менялась. Это сознательно observability-only шаг перед будущим score change.
- Проверка: `cargo test profile_pool_tie_breaks_on_group_shift_residual_then_delta -- --test-threads=1` passed; `cargo test optimize_profile_pool_returns_telemetry -- --test-threads=1` passed.
- Вывод: теперь будущие V49/V50 могут сравнивать profile_pool candidates с явным пониманием, где score выигрывает за счёт визуальной компоновки, а где за счёт cut-gap inflated topology.

### V49: visual-zone-first profile_pool selection

Цель: проверить поведенческую гипотезу, что `profile_pool` должен ранжировать topology по visual zones перед cut-clearance zones, но только после hard lead guard.

V49 intermediate (2026-06-16):

- Ветка: `feat/v49-profile-pool-visual-zone-selection`, base = `origin/main`.
- TDD RED:
  - `profile_pool_lead_guard_rejects_low_density_four_zone_candidate` падал: старый код разрешал 4-zone bypass даже при lead drop больше guard.
  - `optimize_profile_pool_returns_telemetry` падал: не было `winner_visual_waste_regions`.
  - `profile_pool_prefers_visual_zones_before_cut_gap_zones` падал: old order выбирал cut-gap zones раньше visual zones.
- GREEN:
  - добавлено `winner_visual_waste_regions`;
  - убран bypass `candidate.waste_regions <= 4` из lead eligibility;
  - `profile_pool_candidate_order`: `used_stock_count -> visual_waste_regions -> waste_regions -> group_shift residual/delta -> lead -> corner`.
- Targeted checks passed:
  - `cargo test profile_pool_prefers_visual_zones_before_cut_gap_zones -- --test-threads=1`;
  - `cargo test profile_pool_lead_guard_rejects_low_density_four_zone_candidate -- --test-threads=1`;
  - `cargo test optimize_profile_pool_returns_telemetry -- --test-threads=1`.
- Риск: V49 пока подтверждён unit-level, не full benchmark. Следующий шаг должен быть paired profile_pool benchmark с SVG/PNG artifacts и сравнением V43/V48/V49 на seed 11/13 и group_shift cases.

### V50: V49 quick profile_pool benchmark

Цель: проверить V49 не только unit tests, а быстрым service-level paired benchmark на фиксированных seeds.

V50 intermediate (2026-06-17):

- Ветка: `feat/v50-v49-profile-pool-benchmark`, base = `origin/main`, сверху cherry-pick V49 commit.
- Добавлен script `scripts/test_v50_v49_profile_pool_benchmark.py`.
- Скрипт стартует текущий service, прогоняет `multisheet_varied_4sheets` с profile_pool `[0.2,0.3,0.4,0.5,0.6,0.8]`, сохраняет JSON/SVG и считает `zones_visual0` vs `zones_cut_gap`.
- Артефакты: `ai_docs/tmp/v50_v49_profile_pool_benchmark/`.
- Verification/run:
  - `python -m py_compile scripts/test_v50_v49_profile_pool_benchmark.py` passed.
  - `python scripts/test_v50_v49_profile_pool_benchmark.py --port 8098 --seeds 11 13 --time-limit-ms 30000 --restarts 5` passed.
- Результат:
  - seed 11: 4 sheets, visual0=6, cut_gap=7, lead=92.83%, min=89.21%, zp=0.4.
  - seed 13: 5 sheets, visual0=7, cut_gap=6, lead=90.82%, min=4.42%, zp=0.8.
- Вывод: V49 выявил новую проблему. Hard lead guard нельзя считать по всем candidates без учёта sheet count: 5-sheet candidate со slack-листом может иметь высокий `lead_util_pct` и отфильтровать 4-sheet candidates. Следующая гипотеза V51: сначала выбрать минимальный `used_stock_count` bucket, затем применять lead guard/visual-zone ordering внутри этого bucket.

### V51: sheet-count bucket lead guard

Цель: не позволить lead guard сравнивать 4-sheet и 5-sheet candidates как один pool.

V51 intermediate (2026-06-17):

- Ветка: `feat/v51-profile-pool-sheet-bucket-guard`, base = `origin/main`, сверху cherry-pick V49 commit.
- TDD RED: `profile_pool_lead_guard_is_scoped_to_min_sheet_count_bucket` падал: high-lead 5-sheet candidate выбирался вместо 4-sheet candidate.
- GREEN: `profile_pool_winner_idx` сначала находит минимальный `used_stock_count` среди не rejected candidates, затем считает `best_lead`, eligibility и fallback ordering только внутри этого bucket.
- Targeted checks passed:
  - `cargo test profile_pool_lead_guard_is_scoped_to_min_sheet_count_bucket -- --test-threads=1`;
  - `cargo test profile_pool_prefers_visual_zones_before_cut_gap_zones -- --test-threads=1`;
  - `cargo test profile_pool_lead_guard_rejects_low_density_four_zone_candidate -- --test-threads=1`.
- Добавлен script `scripts/test_v51_sheet_bucket_guard_benchmark.py`; артефакты: `ai_docs/tmp/v51_sheet_bucket_guard_benchmark/`.
- Smoke run: `python scripts/test_v51_sheet_bucket_guard_benchmark.py --port 8099 --seeds 11 13 --time-limit-ms 30000 --restarts 5` passed.
- Результат smoke:
  - seed 11: 4 sheets, visual0=6, cut_gap=7, lead=92.83%, min=89.21%, zp=0.4.
  - seed 13: 5 sheets, visual0=7, cut_gap=6, lead=90.82%, min=4.42%, zp=0.8.
- Вывод: V51 исправляет реальный selection-safety bug на unit level, но smoke показывает, что seed 13 проблема не ушла. Значит в quick profile_pool setup, вероятно, нет хорошего 4-sheet candidate среди candidates. Следующая гипотеза V52: rescue через seed offsets / расширение candidate generation при `used_stock_count > 4`, а не только изменение ranking.

### V52: seed-offset rescue benchmark

Цель: проверить, может ли существующий `profile_pool.seed_offsets` решить V51 smoke failure через расширение candidate generation.

V52 intermediate (2026-06-17):

- Ветка: `feat/v52-seed-offset-rescue-benchmark`, base = `origin/main`, сверху cherry-pick V49+V51.
- Добавлен script `scripts/test_v52_seed_offset_rescue_benchmark.py`.
- Скрипт добавляет в profile_pool `seed_offsets` и логирует `rescue_triggered`, `seed_offsets_used`, `candidates_completed`.
- Артефакты: `ai_docs/tmp/v52_seed_offset_rescue_benchmark/`.
- Verification/run:
  - `python -m py_compile scripts/test_v52_seed_offset_rescue_benchmark.py` passed.
  - `python scripts/test_v52_seed_offset_rescue_benchmark.py --port 8100 --seeds 11 13 --time-limit-ms 4000 --restarts 5 --seed-offsets 1 2 3 5 7 8 13 21` passed.
- Результат:
  - seed 11: 4 sheets, visual0=6, cut_gap=7, lead=92.83%, min=89.21%, rescue=true, 54 candidates.
  - seed 13: 4 sheets, visual0=9, cut_gap=7, lead=92.52%, min=90.15%, rescue=true, 54 candidates.
- Вывод: seed-offset rescue подтверждён как способ вернуть seed 13 с 5 sheets на 4 sheets. Но visual quality хуже, чем хотелось: `visual0=9`. Следующая гипотеза V53: rescue acceptance/ranking должен учитывать visual/contact guard, а не только вернуть минимальное число листов. Простое расширение seeds без визуального guard может дать плотность, но разорванный остаток.

### V53: contact-aware group_shift signal in profile_pool

Цель: проверить, можно ли сделать математическую оценку ближе к визуальному качеству, добавив в `profile_pool` фактический `group_shift.contact_gain_mm`.

V53 intermediate (2026-06-17):

- Ветка: `feat/v53-profile-pool-visual-rescue-guard`, base = `origin/main`, сверху V49+V51+V44.
- TDD RED: `profile_pool_prefers_group_shift_contact_gain_after_zone_ties` падал, потому что candidates с одинаковыми sheets/zones/residual/delta не различались по contact gain.
- GREEN:
  - добавлено `ProfilePoolCandidate.group_shift_contact_gain_mm`;
  - добавлено telemetry поле `summary.profile_pool.winner_group_shift_contact_gain_mm`;
  - `profile_pool_candidate_order`: `used_stock_count -> visual_waste_regions -> waste_regions -> group_shift residual -> contact_gain -> group_shift delta -> lead -> corner`.
- Targeted checks passed:
  - `cargo test profile_pool -- --test-threads=1` (17 passed);
  - `cargo test group_shift -- --test-threads=1` (9 passed);
  - `cargo test optimize_profile_pool_returns_telemetry -- --test-threads=1`.
- Добавлен script `scripts/test_v53_contact_guard_benchmark.py`.
- Артефакты: `ai_docs/tmp/v53_contact_guard_benchmark/` (`json`, `svg`, `diff.svg`, PIL-rendered `*_pil.png`).
- Run: `python scripts/test_v53_contact_guard_benchmark.py --port 8101 --seeds 11 13 --time-limit-ms 4000 --restarts 5 --seed-offsets 1 2 3 5 7 8 13 21` passed.
- Результат paired off/on:
  - seed 11 off: 4 sheets, visual0=6, cut_gap=7, lead=92.83%, 54 candidates.
  - seed 11 on: 4 sheets, visual0=7, cut_gap=9, contact_gain=816.5mm, moves=3, parts_moved=5, 54 candidates.
  - seed 13 off: 4 sheets, visual0=9, cut_gap=7, lead=92.52%, 54 candidates.
  - seed 13 on: 4 sheets, visual0=9, cut_gap=7, contact_gain=1293.5mm, moves=4, parts_moved=6, 54 candidates.
- Визуальный вывод:
  - seed13 подтверждает пользовательскую гипотезу: group_shift сдвигает крайние группы к основной массе и не ухудшает zones; это визуально выглядит как полезное уплотнение.
  - seed11 показывает риск: contact_gain может быть высоким, но aggregate zones ухудшается. Значит contact_gain нужен как полезный compactness signal, но не должен быть единственным acceptance/scoring критерием.
- Главный вывод V53: подход group_shift/contact недооценён, но ему нужен paired acceptance guard: move/candidate должен получать credit за контакт только если не ухудшает visual/cut-gap topology или usable remnant.
- Следующая гипотеза V54: `group_shift` acceptance по `contact_gain` + hard guard `visual_zones_after <= visual_zones_before` и/или `cut_gap_zones_after <= cut_gap_zones_before + tolerance`, с отдельной фиксацией `before/after` metrics per moved sheet.
- Следующая гипотеза V55: вместо raw zone-count добавить `usable_remnant_score`: цельный крупный остаток у края/угла должен иметь больший вес, чем несколько разрозненных зон, даже если количество зон одинаковое.

## Практические правила дальнейшей работы

- Каждая кодовая гипотеза — отдельная ветка от `main`.
- Документировать результаты в `ai_docs/CONTEXT_v2.md`.
- Артефакты SVG/PNG/JSON — только в `ai_docs/tmp/<version_name>/`.
- Для важных before/after обязательно делать PNG и реально смотреть глазами.
- Не полагаться на одну aggregate metric.
- Не считать `kerf`-inflated zones визуальной метрикой.
- Не считать group_shift проваленным без paired remnant-score benchmark.
- Не тратить много времени на очередной sweep epochs/restarts без новой структурной гипотезы.
- Перед merge: `cargo fmt --check`, `cargo test -- --test-threads=1`, релевантный benchmark, visual artifacts.

## Performance / latency / scale research (V55+, 2026-06-17)

Контекст: интеграция freecut в ERP. Нагрузочное тестирование (cpus=1.5,
mem=512m, лист 2070x2800) выявило, что сервис надёжно отдаёт 200 только на
малых заказах (~10 листов / ~100 деталей, single-pass `disabled` t1200 r5,
~1.1-1.5s). На 20-50 листов / 200-500 деталей сервис возвращал **408 на всех
режимах и бюджетах** (single-pass до 25s; partition до 40s + OOM-рестарты на
512m; profile_pool aggressive 49s). Корневой блокер — `408 вместо best-partial`.

Гипотезы ускорения и оценка (effect/complexity):

| # | Гипотеза | Эффект | Сложность |
|---|---|---|---|
| H1 | best-partial вместо 408 | снимает блокер, bounded latency | низкая |
| H2 | greedy-сид (Skyline/FFDH) | ускорение сходимости, лучше старт | средняя |
| H3 | параллельная per-sheet декомпозиция | масштаб по листам + параллелизм | высокая |
| H4 | больше ядер (1.5->N) | линейный буст параллельных частей | тривиально |
| H5 | не-GA guillotine движок | кратно, структурно | очень высокая |

Вывод по языку: переписывать на Go бессмысленно — сервис уже на Rust и упёрт в
CPU-кэп (150% даже single-pass, многопоточный rayon). Узкое место — алгоритм и
поведение, не рантайм. Go (GC) скорее замедлит.

### V55 (H1): best-partial via synchronous heuristic seed

- Ветка: `feat/v55-best-partial-on-timeout`, base = `main`.
- Диагноз (map кода `src/optimizer.rs`): 408 (`OptimizeError::Timeout`,
  `run_restarts_with_budget` ~:2856/:2860; `optimize_profile_pool` ~:662)
  рождается ТОЛЬКО когда `best == None`. Когда best есть — уже возвращается
  `Ok` + `timeout_reason` (:2709-2747). На больших заказах первый GA-слайс
  таймаутится (:2695) и `handle.abort()` убивает задачу вместе с её
  in-task FFD-эвристиком (:2614-2625) → best остаётся None → 408.
- Фикс: перед timed restart-циклом синхронно строить FFD-эвристик
  (`build_guillotine_heuristic`/`build_nested_heuristic`, работают standalone),
  прогонять через `pick_best_candidate` и засевать `best`. Тогда даже если все
  слайсы таймаутятся, существующий Ok+timeout_reason путь отдаёт валидную
  частичную раскладку. Эвристик дёшев (FFD, ms).
- Замер (dev-контейнер, 3 cpu, single-pass disabled, budget N*200ms):

  | N | детали | было main | стало H1 | листов | unpl | waste% | wall |
  |---|---|---|---|---|---|---|---|
  | 10 | 107 | 200 | 200 | 10 | 0 | 19.7 | 0.8s |
  | 20 | 208 | **408** | **200** | 18 | 0 | 14.3 | 1.6s |
  | 30 | 317 | **408** | **200** | 28 | 0 | 15.6 | 2.4s |
  | 40 | 420 | **408** | **200** | 37 | 0 | 15.4 | 3.2s |
  | 50 | 524 | **408** (даже 25s) | **200** | 45 | 0 | 13.2 | 3.2s |

- Ключевые выводы:
  - **408-блокер снят полностью**: все размеры отдают ПОЛНУЮ раскладку
    (0 unplaced) за секунды, latency bounded бюджетом, `timeout_reason=slice_timeout`.
  - Малые задачи без регресса: где GA сходится (N=10 t4000 -> 10.7%), GA бьёт
    эвристик; seed не мешает. Где раньше был бы 408 — теперь 200+эвристик.
  - Большие задачи = качество FFD-эвристика: **N=50 waste 13.2% не улучшается
    даже при 30s** — GA на 524 деталях НИКОГДА не завершает per-restart слайс,
    всегда падает на heuristic floor. То есть для крупных заказов H1 — это
    «FFD за секунды» (floor ~13%), а не GA-оптимум.
  - Следствие: H1 даёт надёжный usable floor; качество ВЫШЕ floor на больших
    заказах — это уже задача H2 (лучше construction), H3 (per-sheet GA на
    маленьких подзадачах, которые успевают сойтись), H5 (не-GA движок).
- Статус: реализован, нагрузочно подтверждён. cargo test 67 passed/0 failed.
  Коммит+пуш: `feat/v55-best-partial-on-timeout`.

### V-H4: CPU-core scaling (config-only A/B)

- Замер на H1-билде, `docker update --cpus`, один и тот же job.
- N=30 t8000 (slice_timeout -> heuristic floor):
  - cpus=1.5: peakCPU ~100%, waste 15.6%, wall 3.2s.
  - cpus=4.0: peakCPU ~150-200%, **waste 15.6% (то же), wall 3.2s (то же)**.
- cpus 1.0->3.0 на N10/N20 — результаты бит-в-бит идентичны.
- Диагноз кода: `run_restarts_with_budget` гоняет restarts **последовательно**
  (for-loop + `tokio::time::timeout(&mut handle).await` по одной
  `spawn_blocking` задаче за раз). Параллельность только ВНУТРИ одного GA-прогона
  (rayon в cut-optimizer-2d, ~1.5 ядра потолок на запрос).
- Вывод H4: **больше ядер НЕ ускоряет один запрос** (последовательные restarts —
  бутылочное горло). Эффект только на конкурентности (параллельные запросы) и в
  пограничных случаях, где лишняя скорость даёт GA-слайсу досойтись. Чтобы
  утилизировать ядра одним запросом — нужно параллелить restarts (дёшево, H4b)
  или per-sheet декомпозиция (H3). Поднимать прод cpus>1.5 ради latency одного
  запроса смысла нет; ради throughput многих — да.
- Прод-рекомендация: cpus=1.5-2 достаточно; масштабировать ядрами надо
  concurrency (`MAX_CONCURRENT_OPTIMIZE`), не один запрос.

### V56 (H2): multi-heuristic construction seed (раскрой floor)

- Ветка: `feat/v56-better-construction-seed`, base = `main`, cherry-pick V55 (H1).
- Наблюдение: H1 показал, что большие заказы сидят на FFD heuristic floor
  (GA не сходится в слайсе). А `build_guillotine_heuristic` /
  `build_nested_heuristic` в vendor брали лишь `possible_heuristics()[0]`
  (1 из ~14 guillotine / ~26 maxrects вариантов).
- Фикс (vendor `lib.rs`): перебирать ВСЕ варианты эвристики, строить Solution
  на каждый, возвращать Vec. Сервисный `pick_best_candidate` выбирает лучший →
  выше floor. Дёшево (FFD-пасс на вариант, ms), zero downside (никогда не хуже).
- Замер (single-pass, seed 42, тот же бюджет; H2 vs H1):

  | N | H1 waste | H2 waste | H1 листов | H2 листов |
  |---|---|---|---|---|
  | 10 | 19.7 | 19.7 | 10 | 10 |
  | 20 | 14.3 | 14.3 | 18 | 18 |
  | 30 | 15.6 | **12.5** | 28 | **27** |
  | 40 | 15.4 | **13.0** | 37 | **36** |
  | 50 | 13.2 | 13.2 | 45 | 45 |

- Выводы:
  - **Экономия по листу на 30 и 40 листах** (ниже waste), без регресса и без
    latency-штрафа (heuristic-стоимость пренебрежимо мала против GA-бюджета).
  - На 10/20/50 первый heuristic уже был лучшим → нейтрально.
  - Эффект скромный, но «бесплатный» и направлен ровно на бизнес-цель «минимум
    листов». Имеет смысл оставить как floor-улучшение.
- Статус: реализован. cargo test/fmt — см. коммит. Пуш: `feat/v56-better-construction-seed`.

### V57 (H3): parallel full-budget restarts (NEUTRAL, не мержить)

- Ветка: `feat/v57-parallel-restarts`, base = `main`, cherry-pick H1+H2.
- Гипотеза (из H4): restarts последовательны → ядра простаивают. Запустить все
  restarts параллельно (tokio JoinSet), каждому ПОЛНЫЙ бюджет, брать лучший из
  успевших → ожидали, что средние задачи сойдутся до GA-качества там, где
  последовательный слайсинг падал на floor.
- Реализация: цикл заменён на `JoinSet` + deadline-loop с `join_next`,
  `abort_all` на остатке. Удалены `no_improve_streak` / `EARLY_STOP_*`
  (early-stop в параллельной модели не нужен).
- Результат: **НЕЙТРАЛЬНО**. Параллельность реальна (peakCPU 306% при cap 300%,
  все 3 ядра), тесты 67/0, но качество не изменилось vs H2 (N10 t1200 = 19.7%
  floor, N20=14.3%, N30=12.5%, N50=13.2% — бит-в-бит как V56).
- Диагноз провала гипотезы: последовательный путь НЕ использует равные budget/N
  слайсы — у него **прогрессивное расписание** (`build_restart_slice_schedule`),
  которое уже даёт ранним restart-ам достаточно времени сойтись. Параллелизм
  лишь жжёт больше ядер за тот же ответ. На прод 1.5-cpu это была бы contention.
- Большие задачи остаются heuristic-bound в любом случае: один GA-прогон на 500
  деталях слишком медленный для сходимости в любом разумном бюджете, параллельно
  или нет. Реальный рычаг — уменьшить подзадачу (per-sheet декомпозиция) или
  быстрый не-GA движок (H5).
- Статус: задокументированный эксперимент, **NOT recommended for merge**.
  Пуш: `feat/v57-parallel-restarts`.

### V58 (H5): `engine=heuristic` — instant non-GA движок

- Ветка: `feat/v58-heuristic-engine`, base = `main`, cherry-pick H1+H2.
- Премиса (доказана на текущем сервисе): большие задачи сидят на heuristic floor;
  GA-бюджет тратится зря. N50 при `time_limit_ms=100` -> 131ms, 13.2% — тот же
  результат, что при 3000ms. То есть floor доступен мгновенно.
- Реализация: новый параметр `params.engine: "ga" | "heuristic"` (default `ga`).
  При `heuristic` `run_restarts_with_budget` после синхронного multi-variant FFD
  сида (H1/H2) сразу возвращает `RunOutcome` (restarts_used=0), минуя GA-цикл.
  Добавлен `Engine` enum в models.rs, импорт в optimizer, unit test
  `optimize_engine_heuristic_skips_ga_and_returns_layout`.
- Замер engine=heuristic vs ga (тот же seed/бюджет):

  | N | heuristic wall | ga wall | листов | waste% (оба) |
  |---|---|---|---|---|
  | 20 | **29ms** | 2011ms | 18 | 14.3 |
  | 30 | **19ms** | 2017ms | 27 | 12.5 |
  | 40 | **42ms** | 2034ms | 36 | 13.0 |
  | 50 | **30ms** | 2032ms | 45 | 13.2 |

- Вывод: **~50-100x быстрее при идентичном качестве** (те же листы/waste),
  `timeout_reason=None`. Для больших батчей backend шлёт `engine=heuristic` ->
  ответ ~30ms. Для малых quality-заказов остаётся `ga`.
- Статус: реализован + тест. cargo test/fmt — см. коммит. Пуш:
  `feat/v58-heuristic-engine`.

## Итоговый синтез (V55-V58, 2026-06-17)

Вопрос пользователя: что ускорит сервис; поможет ли переписывание на Go.

**Go — нет.** Сервис на Rust, упёрт в CPU (rayon). Узкое место — алгоритм и
поведение, не рантайм. Go (GC) скорее замедлит.

Результаты гипотез:

| # | Гипотеза | Вердикт | Эффект |
|---|---|---|---|
| H1 | best-partial вместо 408 | ✅ MERGE | снят блокер: 20-50 листов отдают полную раскладку за секунды вместо 408 |
| H2 | multi-heuristic seed | ✅ MERGE | -1 лист на 30/40-лист заказах, бесплатно |
| H3 | параллельные restarts | ❌ нейтрально | прогрессивный слайсер уже адекватен; жжёт ядра зря |
| H4 | больше ядер | ❌ для latency | restarts последовательны; ядра -> только throughput |
| H5 | engine=heuristic | ✅ MERGE | большие батчи ~50-100x быстрее при том же качестве |

Рекомендованный merge-порядок в main: **H1 -> H2 -> H5** (стакаются чисто, все
gates зелёные). H3/H4 — закрытые исследования (не мержить).

Merge-факт (2026-06-18): H1+H2+H5 смержены в `main` (merge `c292bdd`,
`13cf8e4..c292bdd`). H3/H4 остались отдельными исследовательскими ветками.

## Перспективные гипотезы V59-V61 (next)

Рамка: главная бизнес-метрика — **число листов**, затем форма/usable-остаток.
Большие заказы сейчас едут на heuristic floor (H5). Цель V59-V61 — пробить floor.

### V59 — per-sheet декомпозиция

- Идея: взять per-sheet группировку деталей из construction-эвристика (она уже
  даёт назначение деталь->лист), затем переоптимизировать КАЖДЫЙ лист отдельным
  маленьким быстрым GA (вход: детали листа + 1 stock), параллельно по ядрам,
  собрать обратно.
- Цель: качество ВНУТРИ листа (плотность, форма остатка) + масштаб/параллелизм.
  Подзадача ~10-15 деталей сходится за мс → впервые GA реально вносит вклад на
  больших заказах (в отличие от монолитного GA на 500 деталях, который не
  сходится никогда).
- Ограничение: назначение фиксировано → **число листов НЕ уменьшит**, только
  форму/плотность внутри листа.
- Бонус: наконец окупает многоядерность (закрывает провал H3/H4 — мелкие
  подзадачи реально параллелятся).
- Сложность: средне-высокая. Риск: средний (reassembly, координаты, feasibility).
- Вердикт: 🟡 перспективна для качества-выше-floor, но не бьёт главную метрику.
  Хороший value/effort и фундамент под V60.

### V60 — лучшее назначение деталей по листам (минимум листов)

- Идея: умнее раскидать детали по листам, чтобы снизить САМО число листов.
- Цель: 🎯 главная метрика (минимум листов = прямая экономия материала).
- Это ядро 2D cutting-stock, NP-hard. GA пытается решать глобально, но не
  сходится на больших входах. Нужен спец-алгоритм: Best-Fit-Decreasing,
  column generation / set-cover, branch-and-bound с хорошими нижними оценками.
- Сложность: 🔴 высокая (настоящая алгоритмическая работа). Риск: высокий.
- Вердикт: 🟢 наивысшая ценность, наивысшая цена. Правильное долгосрочное
  направление.
- **V60-lite (дешёвый первый шаг, РЕКОМЕНДУЕТСЯ начать отсюда):** улучшить
  именно стратегию НАЗНАЧЕНИЯ внутри construction-эвристика — Best-Fit-Decreasing
  вместо First-Fit, skyline-with-rotation, попробовать несколько bin-assignment
  стратегий и взять минимум листов (аналогично тому, как H2 перебрал
  placement-эвристики, но H2 НЕ трогал bin-assignment). Раз большие задачи едут
  на эвристике, сокращение её листов даёт выигрыш сразу и дёшево. Замер на тех
  же 10-50 листах, метрика — used_stock_count.

### V61 — anytime-GA

- Идея: GA, который держит валидный текущий-лучший layout и монотонно улучшает
  heuristic floor; на дедлайне отдаёт лучшее найденное.
- Проблема сейчас: GA на 500 деталях all-or-nothing — никогда не выдаёт лучше
  floor ни за какое время (H5 это и зафиксировал). Anytime-GA дал бы средний
  режим: большой заказ + неск. секунд → лучше floor.
- Цель: окупить GA-бюджет на больших заказах (сейчас тратится зря).
- Сложность: средне-высокая, и лезет в vendor GA internals (`cut-optimizer-2d`)
  — глубже и рискованнее. Риск: высокий (vendor-хирургия).
- Вердикт: 🟡 перспективна, дополняет H5 (малый=ga, большой=heuristic,
  anytime=середина), но vendor-deep.

### Приоритет реализации

1. **V60-lite** — дёшево, бьёт по главной метрике (листы), эффект сразу (большие
   едут на эвристике). Старт отсюда.
2. **V59** — tractable, качество остатка + окупает ядра, фундамент под V60.
3. **V60-full / V61** — крупные структурные работы, когда дёшевые исчерпаны.

Прод-итог для ERP-интеграции:
- Малый заказ (<=10 листов): `engine=ga`, `disabled`, t1200 r5 -> ~1.1s, GA-качество.
- Большой батч (20-50+ листов): `engine=heuristic` -> ~30ms, floor-качество, 0 unplaced.
- Никогда не слать profile_pool/seed_offsets/partition в онлайн (research, 12-49s,
  partition ещё и OOM-ит 512m).

## Alternative engine research (V63+, 2026-06-18)

Пользовательский запрос: не ограничиваться текущим `cut-optimizer-2d`/GA-подходом,
а проверить радикально альтернативные параллельные движки, между которыми можно
будет переключаться через API.

Ограничение текущего блока: исследовать только гипотезы из последнего ресерча:

1. **V63 PackingSolver adapter benchmark** — внешний state-of-the-art C++ solver
   `fontanf/packingsolver` как sidecar/CLI engine. Проверить feasibility
   интеграции, формат входа/выхода, качество против `engine=heuristic/ga`, цену
   запуска и пригодность для `guillotine`/`nested` режимов.
2. **V64 independent constructive portfolio** — собственный независимый движок:
   MaxRects/Skyline/Shelf/Guillotine + разные сортировки/assignment strategies +
   unified visual/remnant score. Цель — быстрый управляемый production engine,
   не завязанный на vendor GA.
3. **V65 CP-SAT exact/verifier** — OR-Tools CP-SAT/NoOverlap2D как exact или
   near-exact engine для малых задач и как verifier/lower-bound инструмент.
4. **V66 SAT/MaxSAT/column-generation feasibility** — не начинать с реализации,
   сначала оценить, есть ли готовый путь/библиотеки и какой размер задач реален.

Приоритет выполнения: V63 -> V64 -> V65 -> V66. Для каждой кодовой гипотезы —
отдельная ветка от свежего `main`; артефакты только в `ai_docs/tmp`.

V63 start:
- base должен быть текущий `origin/main` после merge H1+H2+H5 (`c292bdd`);
- ветка: `feat/v63-packingsolver-adapter-benchmark`;
- первый шаг: проверить наличие/сборку PackingSolver локально в `ai_docs/tmp`,
  не использовать `C:/tmp`;
- acceptance для первого прохода: получить хотя бы один воспроизводимый layout
  или зафиксировать конкретный блокер сборки/CLI/API; сравнить sheets/waste/time
  с текущим `engine=heuristic` на 1-2 fixtures.

V63 intermediate (2026-06-18):

- Worktree: `ai_docs/tmp/worktrees/v63-packingsolver-adapter`.
- External source/build: `ai_docs/tmp/external/packingsolver`.
- Build finding:
  - MinGW configure ломается на CMake/FetchContent stamp/update targets.
  - MSVC BuildTools installed; через `vcvars64.bat` configure/build проходит.
  - Собран CLI:
    `ai_docs/tmp/external/packingsolver/build_v63_vs/src/rectangleguillotine/Release/packingsolver_rectangleguillotine.exe`.
- Script: `scripts/test_v63_packingsolver_benchmark.py`.
- Артефакты: `ai_docs/tmp/v63_packingsolver_benchmark/`
  (`items.csv`, `bins.csv`, `packingsolver_certificate.csv`,
  `packingsolver_layout.svg`, `packingsolver_layout.png`, Freecut baseline
  JSON/SVG, `summary.json`).
- Важные adapter details:
  - Freecut mm converted to integer scale 10, чтобы сохранить 0.5mm spacing.
  - `kerf + spacing = 6.5mm` передан как `--cut-thickness 65`.
  - trims записаны в `bins.csv` как hard trims со всех сторон.
  - `objective=bin-packing` без LP solver падает; для smoke используется
    tree-search-only режим:
    `--use-tree-search true` и все column/sequential/dichotomic algorithms false.
- Benchmark on `tests/fixtures/multisheet_varied_4sheets.json`:

  | Engine | sheets | waste% | solver/API time | request/wall |
  |---|---:|---:|---:|---:|
  | Freecut fixture defaults (`ga`, t1000 r3) | 5 | 26.46 | 1007ms | 3044ms |
  | Freecut `engine=heuristic`, t100 r1 | 5 | 26.46 | 8ms | 25ms |
  | PackingSolver `rectangleguillotine`, tree-search-only, t2s | **4** | **8.07** | 24ms | 36ms |

- Visual check: `packingsolver_layout.png` manually inspected. Layout is valid
  by eye: 4 sheets, all 40 items placed, no obvious overlaps. Shape is not yet
  optimized for single usable remnant, but it is radically better than current
  5-sheet floor on the primary material metric.
- V63 conclusion so far:
  - PackingSolver is not just theoretically interesting; it immediately found
    the 4-sheet theoretical minimum on the main 4-sheet fixture where current
    main default/heuristic returned 5 sheets.
  - This strongly confirms the "parallel alternative engine" direction.
  - Next V63 step: test one larger ERP-like generated fixture (20-50 sheets) and
    then decide whether to build a real `engine=packingsolver` adapter or keep it
    as external sidecar benchmark first.

V63 extended runs (2026-06-18):

- Script extended with `--repeat-factor`, `--packing-mode tree|default`,
  `--lp-solver`. Generated fixtures are not committed; stored in `ai_docs/tmp`.
- Repeat-factor 5 (200 items, area lower bound about 19 sheets, geometric
  repeated pattern target 20):

  | Engine | sheets | waste% | solver/API time | request/wall |
  |---|---:|---:|---:|---:|
  | Freecut fixture defaults | 20 | 8.07 | 1005ms | 3064ms |
  | Freecut `engine=heuristic` | 20 | 8.07 | 61ms | 77ms |
  | PackingSolver tree-search-only, t5s | 20 | 8.07 | 5003ms | 5020ms |

- Repeat-factor 10 (400 items, area lower bound about 37 sheets):

  | Engine | sheets | waste% | solver/API time | request/wall |
  |---|---:|---:|---:|---:|
  | Freecut fixture defaults | 40 | 8.07 | 1012ms | 3083ms |
  | Freecut `engine=heuristic` | 40 | 8.07 | 158ms | 207ms |
  | PackingSolver tree-search-only, t5s | **39** | **5.72** | 5002ms | 5037ms |
  | PackingSolver tree-search-only, t10s | 39 | 5.72 | 10003ms | 10024ms |
  | PackingSolver default+HiGHS, t5s | 39 | 5.72 | 5004ms | 5033ms |

- HiGHS/default notes:
  - Building with `PACKINGSOLVER_USE_HIGHS=ON` succeeds via MSVC.
  - Default mode without `--linear-programming-solver highs` can find a good
    incumbent and then exit with `ERROR, no linear programming solver found`.
  - Passing `--linear-programming-solver highs` fixes the CLI run.
  - On tested fixtures default+HiGHS did not beat tree-search-only, but it is the
    safer mode to preserve for deeper PackingSolver benchmarks.

V63 updated conclusion:
- PackingSolver should be treated as a serious candidate for `engine=packingsolver`.
- It has two distinct value profiles:
  1. Small/medium hard cases: can find the theoretical minimum where current
     Freecut default/heuristic can get stuck at an extra sheet.
  2. Large repeated/ERP-like cases: can sometimes reduce sheet count vs Freecut
     heuristic (`repeat10`: 40 -> 39) but costs seconds, not milliseconds.
- It is not a drop-in replacement for `engine=heuristic` on latency-critical
  large requests. It is a quality/alternative engine, likely exposed through
  `engine=packingsolver` or an `auto_quality` portfolio.
- Next V63/V64 boundary: implement a lightweight internal constructive
  portfolio (MaxRects/Skyline/Guillotine) to see whether we can get the
  PackingSolver-style sheet-count improvement without external C++ sidecar cost.

V64 independent constructive portfolio (2026-06-18):

- Branch/worktree: `feat/v64-constructive-portfolio-benchmark`,
  `ai_docs/tmp/worktrees/v64-constructive-portfolio`.
- Script: `scripts/test_v64_constructive_portfolio.py`.
- This is intentionally independent from `cut-optimizer-2d`: a small
  MaxRects-style constructive portfolio with multiple sort orders and placement
  rules (`bssf`, `baf`, `bl`, `contact`). Effective gap is still
  `kerf_mm + spacing_mm`; placements use usable stock after trim margins.
- Artifact folders:
  - `ai_docs/tmp/v64_constructive_portfolio_main_random_compact/`
  - `ai_docs/tmp/v64_constructive_portfolio_repeat10_random100_compact/`

V64 measured results:

| Case | Mode | candidates | sheets | waste% | lower bound | time note |
|---|---|---:|---:|---:|---:|---|
| main fixture | deterministic only | 24 | 5 | 26.46 | 4 | about 1-2ms per candidate |
| main fixture | +200 random/noisy area restarts | 824 | **4** | **8.07** | 4 | best candidate about 2ms |
| repeat-factor 10 | deterministic only | 24 | 40 | 8.07 | 37 | about 46ms best candidate |
| repeat-factor 10 | +100 random/noisy area restarts | 424 | 40 | 8.07 | 37 | best candidate about 45ms |

V64 visual check:

- Rendered artifact inspected:
  `ai_docs/tmp/v64_constructive_portfolio_main_random_compact/best_layout.svg`
  plus local PNG preview `best_layout_rendered.png`.
- The 4-sheet result is visually valid by eye: all sheets are filled, no obvious
  overlaps, and the sheet-count/waste result matches PackingSolver on the main
  fixture.
- However, this is not yet a final visual-quality engine. Some sheets still have
  fragmented internal empty areas/corridors. The result is strong for material
  count, but weaker than the desired "one compact anchored group with one usable
  remnant" objective.

V64 conclusion:

- The hypothesis is partially confirmed and important:
  a very cheap independent constructive portfolio can escape the current
  Freecut 5-sheet local minimum on the main fixture without an external solver.
- Deterministic MaxRects-style construction alone is not enough; the win came
  from order diversity (`random_area_*`) plus placement-rule portfolio.
- On larger repeated batches this implementation did not reproduce
  PackingSolver's 40 -> 39 sheet improvement, so it should not be considered a
  full PackingSolver replacement yet.
- Next practical direction for V64, if promoted later: integrate this as a fast
  internal `engine=constructive_portfolio` or as a seed generator for the current
  optimizer, then combine it with group-shift / anchor-compact visual scoring.

V65 CP-SAT verifier benchmark (2026-06-18):

- Branch/worktree: `feat/v65-cpsat-verifier-benchmark`,
  `ai_docs/tmp/worktrees/v65-cpsat-verifier`.
- Script: `scripts/test_v65_cpsat_verifier.py`.
- Local dependency for research only:
  `ai_docs/tmp/pydeps/ortools` (`ortools==9.15.6755`), with pip cache/temp also
  under `ai_docs/tmp`. This dependency is intentionally not committed.
- Model:
  - Fixed sheet count feasibility via OR-Tools CP-SAT `NoOverlap2D`.
  - Optional intervals per piece/sheet/orientation.
  - Effective gap is `kerf_mm + spacing_mm`; intervals are inflated by the gap,
    while sheet edge fit is checked against actual part dimensions.
  - Supports `--hint-svg` to validate an existing SVG layout and `--fix-hint`
    for strict fast verification of that layout.

V65 measured results:

| Case | Mode | status | sheets | time | notes |
|---|---|---|---:|---:|---|
| `optimize_valid.json` | no hint, feasibility | OPTIMAL | 1 | ~19ms | confirms model works on tiny cases |
| main fixture | no hint, `anchor`, 30s | UNKNOWN | 4 | ~35s wall | no feasible incumbent found |
| main fixture | no hint, feasibility, 30s | UNKNOWN | 4 | ~30s | no feasible incumbent found |
| main fixture | no hint, feasibility, 10s | UNKNOWN | 5 | ~10s | even easy 5-sheet sanity is too symmetric without hint |
| main fixture | V64 SVG hint, feasibility, 10s | OPTIMAL | **4** | ~0.85s | confirms full 4-sheet layout |
| main fixture | V64 SVG hint + `anchor`, 20s | FEASIBLE | 4 | 20s | did not prove optimal; visual improvement was limited |
| main fixture | V64 SVG hint + `--fix-hint`, 5s | OPTIMAL | **4** | **~18ms** | strict validator, 0 branches |

V65 visual check:

- Rendered/inspected CP-SAT `anchor` output from
  `ai_docs/tmp/v65_cpsat_main_4sheets_hint_v64_anchor_20s/cpsat_layout.svg`.
- CP-SAT moved some parts compared with the V64 hint, but it did not solve the
  main visual-remnant problem. Empty space/corridors remain fragmented.

V65 conclusion:

- CP-SAT is not a good standalone alternative engine for the current 40-piece
  fixture if started cold: the search space is too symmetric and it fails to
  find even known feasible layouts within short online budgets.
- The useful role is different and strong: CP-SAT is an excellent validator for
  layouts produced by V64/PackingSolver/current engine. With a complete SVG hint
  and fixed variables, it validates the 4-sheet layout in milliseconds.
- Practical direction: keep V65 as `quality/verifier` tooling and possibly as a
  small-instance exact mode, but do not prioritize it as the next production
  packing engine. If promoted, feed it strong constructive/PackingSolver hints
  and use it to reject overlaps/gap violations or prove feasibility for a target
  sheet count.

V66 SAT/MaxSAT/column-generation feasibility (2026-06-18):

- Branch/worktree: `feat/v66-sat-feasibility-estimator`,
  `ai_docs/tmp/worktrees/v66-sat-feasibility`.
- Script: `scripts/test_v66_sat_feasibility.py`.
- Scope: no full SAT solver implementation. This pass estimates encoding size
  for grid/candidate-placement SAT/MaxSAT/exact-cover formulations before
  investing in a solver.
- Artifact folders:
  - `ai_docs/tmp/v66_sat_feasibility_main/`
  - `ai_docs/tmp/v66_sat_feasibility_repeat10/`

V66 measured scale:

Main 40-piece fixture, target 4 sheets:

| grid step | candidate vars | same-sheet pair upper bound | note |
|---:|---:|---:|---|
| 100mm | 90,232 | 990,330,388 | coarse proxy only |
| 50mm | 349,800 | 14,881,999,452 | already too large for naive SAT |
| 25mm | 1,372,384 | 229,060,462,032 | too large |
| 10mm | 8,521,248 | 8,830,576,090,976 | too large |
| 1mm | 842,468,160 | 86,313,327,740,542,400 | impractical |
| 0.5mm | 3,367,732,800 | 1,379,257,841,659,171,520 | impractical |

Repeat-factor 10 (400 pieces), area lower bound 37 sheets:

| grid step | candidate vars | same-sheet pair upper bound | note |
|---:|---:|---:|---|
| 100mm | 8,346,460 | 938,863,061,950 | too large even coarse |
| 50mm | 32,356,500 | 14,109,676,611,810 | too large |
| 10mm | 788,215,440 | 8,372,979,935,293,040 | too large |
| 0.5mm | 311,515,284,000 | 1,307,819,119,935,102,965,600 | impractical |

V66 conclusion:

- Naive grid SAT/MaxSAT/exact-cover is not a practical alternative engine for
  this service. Exact-enough discretization must respect 0.5mm-scale geometry
  because `kerf+spacing` can be fractional, and that explodes variables.
- A coarse 100mm grid can be used only as a research proxy; it is not accurate
  enough for real cutting geometry and still becomes large on repeat10.
- The only plausible SAT/column-generation direction is hybrid:
  generate a small candidate set from V64/PackingSolver/current layouts, then use
  exact-cover/MaxSAT to select/repair/verify candidates. This should be treated
  as later-stage tooling, not the next production engine.
- Therefore, among V63-V66 the best next engineering direction remains:
  V63 as high-quality external engine, V64 as fast internal seed/portfolio, V65
  as strict verifier, and no standalone V66 SAT engine for now.

V67 ladder benchmark, practical scale check to 50 sheets (2026-06-18):

- Branch/worktree: `feat/v67-ladder-benchmark`,
  `ai_docs/tmp/worktrees/v67-ladder-benchmark`.
- Script: `scripts/test_v67_ladder_benchmark.py`.
- Generation: deterministic cumulative stream from `multisheet_varied_4sheets.json`;
  requested LB `1..10,15,20,25,30,35,40,45,50`.

V67 main results, sheet count only. For LB 1-7 all engines hit the lower bound;
the first visible split is LB=8.

| LB | pieces | Freecut heuristic | V64-style constructive | PackingSolver 1s |
|---:|---:|---:|---:|---:|
| 8 | 68 | 9 | 8 | 8 |
| 15 | 141 | 16 | 16 | 15 |
| 20 | 203 | 21 | 21 | 20 |
| 25 | 251 | 27 | 26 | 26 |
| 40 | 414 | 43 | 43 | 42 |
| 50 | 527 | 54 | 53 | 52 |

V67 higher-budget PackingSolver:

| LB | pieces | Constructive | PackingSolver 5s | PackingSolver 10s |
|---:|---:|---:|---:|---:|
| 25 | 251 | 26 | **25** | not run |
| 40 | 414 | 43 | 41 | not run |
| 50 | 527 | 53 | 52 | **51** |

V67 conclusions:

- Small jobs up to about 7 sheets are not the main problem: all engines normally
  hit the area lower bound.
- The first practical failure appears at LB=8: Freecut heuristic uses 9 sheets,
  while constructive/PackingSolver still hit 8.
- From LB=15 upward PackingSolver becomes consistently best. The improvement is
  real but not always enough to hit the theoretical area lower bound.
- For the 50-sheet case, current measured best is PackingSolver 10s: **51
  sheets vs lower bound 50**, while Freecut heuristic needs 54 and constructive
  needs 53.
- This confirms the portfolio direction, but 50-sheet production quality needs
  more than short PackingSolver runs: high-quality mode + constructive seeds +
  compact/group-shift scoring + possibly longer/offline budgets.

## V59/V61 productionization A — `cut_quality` profile

- Branch: `feat/freecut-quality-profile`; draft:
  `docs/research/drafts/2026-06-18-cut-quality-profile.md`.
- Низкоуровневые post-process настройки `consolidate`/`lns` свернуты в один
  request-параметр `cut_quality: fast | balanced | max` для
  `engine=heuristic`.
- Профили:
  - `fast` = только floor;
  - `balanced` = consolidate (FFD);
  - `max` = consolidate + lns (`max_iters=4000`).
- Явные объекты `consolidate`/`lns` переопределяют профиль; `engine=ga`
  игнорирует `cut_quality`; отсутствие параметра сохраняет старое поведение.
- Это resolution-layer wrapper поверх существующих V59/V61, без изменения
  качества самого optimizer; never-regress contract сохраняется.
- Prod profile (1.5cpu/512m), single N50 (524 детали): `fast` 285ms/45 листов,
  `balanced` 301ms/45 листов, `max` 12.9s/43 листа (-2, waste 13.2% -> 9.2%).
  `balanced` совпал с `fast` на этом single instance; headline consolidation
  `-11` относится к grid-aggregate, не к этому одному инстансу.

## V59/V61 productionization B — async-safe + bounded post-process

- Branch: `feat/freecut-async-postprocess`; draft:
  `docs/research/drafts/2026-06-18-async-postprocess.md`.
- Синхронный consolidate+lns post-process перенесен с async runtime thread в
  `tokio::task::spawn_blocking`, аналогично GA restarts. Ранее deep (`lns`)
  requests блокировали tokio worker на весь deadline.
- Concurrency decision: request уже держит `optimize_semaphore` permit всю
  свою жизнь, поэтому deep jobs ограничены `MAX_CONCURRENT_OPTIMIZE`; отдельный
  deep cap не нужен.
- Admission queue: over-cap requests теперь ждут permit до
  `OPTIMIZE_QUEUE_WAIT_MS` (новый config, default 60s), вместо мгновенного
  `429`; `0` возвращает immediate-reject. При live cap=1 две concurrent deep
  N30 задачи обе вернули 200, вторая ждала около 6s, без 429.
- Prod profile (1.5cpu/512m, `MAX_CONCURRENT_OPTIMIZE=2`), 2 concurrent deep
  N50 jobs + polling `/health/ready`:
  - inline до `spawn_blocking`: health p95 **3964ms**, max timeout, **2
    timeouts**, deep jobs занимали оба async workers и сериализовались
    (~26.5s wall);
  - `spawn_blocking`: health p95 **7.8ms**, max 124.9ms, **0 timeouts**
    (~12.5s);
  - результаты идентичны (N50 = **43 sheets**, deterministic), изменение только
    execution-context.
- Реальная цена deep-mode в prod profile: N50 `max_iters=4000` около **13.1s**
  wall на 1.5cpu против ~2-3s на 3-cpu dev box; в prod держать
  `MAX_CONCURRENT_OPTIMIZE` малым.

## V72: Честная visual-remnant метрика

- Branch: `feat/remnant-telemetry`; draft:
  `docs/research/drafts/2026-06-18-remnant-telemetry.md`.
- Мотив: V70-разрыв по nested мерялся грубым внешним raster-прокси. Построена
  доверенная in-service метрика, разрыв подтверждён/опровергнут.
- Существующие метрики кандидата меряют *площадь* свободного, не *связность*:
  `bbox_void` показывал паритет nested (7.08M vs 6.99M, +1.3%), `corner_free` даже
  в пользу nested — ни одна не отличает один крупный остаток от множества
  staircase-щелей той же суммарной площади.
- Новая `remnant_metrics` (`summary.remnant`): растеризуем каждый лист по сетке
  20мм, flood-fill пустых ячеек в связные регионы. Поля: `free_fragments`,
  `largest_free_mm2`, `largest_free_frac`, `mean_sheet_largest_free_frac`.
  Считается один раз при сборке ответа, gated на `include_svg`; O(cells), без
  hot-loop стоимости. Unit-тесты (L-форма => 1 фрагмент/frac 1.0; центральная
  полоса => 2/0.5).
- Находка — разрыв РЕАЛЕН по связности. N35 `cut_quality=max`: guillotine
  free_fragments 49 / mean_sheet_largest_free_frac **0.900**; nested 65 /
  **0.795**. Визуальное подтверждение (самый пустой лист в `ai_docs/tmp`):
  guillotine = ровные колонки + один L-остаток; nested упаковал *больше* деталей
  (22 vs 12) с крупным нижним остатком, но с мелкими внутренними staircase-щелями
  между несовпадающими деталями. Глаз и метрика совпали; `bbox_void` — нет.
- Вывод: метрика связности исправляет ошибочный «паритет» по `bbox_void` и даёт
  измеримую цель (`mean_sheet_largest_free_frac`). Failure mode nested —
  внутренние staircase-щели, не разорванный основной остаток. Это переобосновывает
  remnant-aware шаг для nested с реальной целью. Метрика mode-agnostic, полезна
  сама по себе.
