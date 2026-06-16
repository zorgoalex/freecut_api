# CONTEXT_v2 — компактный рабочий контекст Freecut

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
