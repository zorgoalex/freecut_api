# Last State (Compressed Summary)

## 1) Текущее состояние
- Сервис стабилен, основной endpoint: `POST /v1/optimize`.
- Экспериментальные/расширенные endpoint’ы:
  - `POST /v1/optimize/beam`
  - `POST /v1/optimize/alns`
- Базовый эталон для сравнения алгоритмов:
  - `tests/fixtures/multisheet_oversized.json`
  - лист MDF `2800x2070`.

## 2) Формула качества и бизнес-gate
- Текущий ranking candidates:
  - `sort_key = (internal_void, occupied_perimeter, void_compactness, corridor_components, waste_percent)`.
- Hard constraints (обязательные):
  - `placeable_placed_ratio = 1.0`
  - `internal_void = 0`.
- Бизнес-цель уровня 1: максимум размещённых деталей / минимум листов.
- `waste_percent` оставлен как tie-breaker; не использовать как главную метрику геометрического качества.
- Формула и диапазоны параметров остаются исследовательскими и могут меняться.

## 3) Ключевые выводы по алгоритмам
### Standard
- После последних фиксов снова корректный baseline.
- Для текущего baseline даёт стабильные `200` и хорошую latency.

### Greedy
- Проведён deep review, выявлены ограничения реализации.
- На текущих сценариях не дал устойчивого превосходства над standard.
- Оставлен как дополнительный/исследовательский режим, не основной путь.

### Portfolio/Anytime
- На `time_limit <= 2000ms` обычно хуже standard по quality-формуле.
- Около `2500ms` — паритет.
- С `~3000ms+` начинает выигрывать по quality на baseline.
- Компромисс: latency выше standard (примерно `~1.8–2.0s` против `~1.2–1.3s`).

### Beam
- Как отдельный endpoint реализован и покрыт тестами.
- На малых лимитах по времени/рестартам сильно чувствителен к timeout.
- Минимальный проверенный порог преимущества по quality над standard: `~4900ms`.
- Цена: latency около `~4s` (`~x3` к standard).
- Вывод: не default для бизнеса; только опциональный quality-mode.

### ALNS/LNS
- Реализован как отдельный endpoint с телеметрией операторов.
- По baseline:
  - минимальный подтверждённый порог преимущества над standard: `~1250ms`;
  - увеличение `time_limit` выше `~2000ms` не дало гарантированного прироста качества.
- Реальная польза: quality-mode при аккуратной настройке, но не универсальный default.

## 4) Критичный инцидент и фикс (restarts/time_limit)
### Инцидент (до фикса)
- На heavy baseline для standard при `time_limit=2000` и `restarts>=4` наблюдался массовый `408`.
- Это оказалось не хардкодом, а эффектом budget slicing + fail-fast при первом `slice_timeout`.

### Что внедрено
- Timeout rescue в standard-пути (если нет incumbent и первый slice timeout).
- Budget-aware restart policy:
  - dynamic cap по effective restarts,
  - baseline-first budgeting,
  - progressive slicing.
- SLA profile в API (`params.sla_profile`):
  - `fast`, `balanced` (default), `quality`.
- Расширенная телеметрия `summary.restart_policy`:
  - профиль, cap/effective restarts, baseline budget, planned slices,
  - timeout counters, best-found index, rescue flags.
- Perf-gate regression test добавлен (heavy fixture, ignored/manual).

### Результат после фикса
- Повторный restart-sweep на тех же диапазонах:
  - standard: теперь стабилен на `restarts=1..100` (`ok_rate=1.0`, `hard_ok_rate=1.0` на baseline).
  - фактически standard обычно использует около `~2` рестартов при больших requested (за счёт policy cap).
- Для portfolio/beam/alns рабочие зоны по рестартам в целом сохранились:
  - portfolio: обычно рабочая зона до `~8`, дальше высокий риск timeout;
  - beam: обычно рабочая зона до `~4`, дальше высокий риск timeout;
  - alns: стабильный `200` при `r>=2`, но фактические restarts часто около `1`.

## 5) Текущая позиция для бизнеса
- Default online путь: `standard` с `sla_profile=balanced`.
- `portfolio`, `beam`, `alns` — как отдельные quality-mode сценарии под явный запрос/профиль.
- Для online SLA не повышать `requested_restarts` без контроля effective slicing.
- Визуальная проверка остаётся обязательным этапом отбора лучших раскладок (после автоматического фильтра top-N).

## 6) Риски/заметки
- Риск коллизии маппинга stock при одинаковом usable-размере листа (`(usable_w, usable_h)`), если отличаются `stock_id/qty`.
  - Это критично именно для бизнес-контекста сервиса (учёт остатков/ограничений склада).
- Формула качества не покрывает все визуально значимые кейсы; требуется дальнейшая итерация критериев.

## 7) Основные артефакты измерений
- Beam sweep: `ai_docs/tmp/beam_time_sweep_quality_*.json`
- Portfolio sweep: `ai_docs/tmp/portfolio_time_sweep_quality_*.json`
- ALNS sweep: `ai_docs/tmp/alns_time_sweep_quality_*.json`
- Restart sweep (до/после фикса standard):
  - `ai_docs/tmp/restart_sweep_quality_s20.json`
  - `ai_docs/tmp/restart_sweep_quality_s20_postfix.json`

## 8) Глубинная причина повторяемости «рваных» раскладок (новые выводы)
- Проблема не сводится к seed/restarts: на текущем наборе деталей видимые паттерны пустот и периметра остаются близкими между алгоритмами даже при случайном `seed`.
- По исходнику `cut-optimizer-2d` подтверждено, что все наши режимы (`standard/portfolio/beam/alns`) опираются на один и тот же базовый GA-движок и одну внутреннюю целевую оптимизацию (`price + fitness`), а не на бизнес-метрики пустот/компактности.
- Библиотека возвращает один «best» кандидат популяции; альтернативные сильные кандидаты не отдаются наружу, поэтому пост-отбор по нашей формуле ограничен входным множеством.
- Параметры GA в библиотеке фактически фиксированы в текущей версии (`epochs(100)`, `breed_factor(0.5)`, `survival_factor(0.6)`), что ограничивает управляемость качества через API.

### Что уже сделано в сервисе
- Добавлена диверсификация траекторий на рестартах (варианты порядка `cut_pieces/stock_pieces`).
- Добавлен tie-break по плотности bbox (`bbox void`) для снижения повторяемости рыхлых/рваных паттернов при равных базовых целях.
- Seedless-сравнение (`--omit-seed`) показало рост разнообразия top-N, но без радикального изменения «макро-геометрии» раскладок.

### Практический вывод
- Для реального качественного сдвига требуется не только orchestration снаружи, а изменение уровня библиотеки:
  - возврат top-K кандидатов из GA,
  - интеграция бизнес-скора в финальный выбор,
  - вынос GA-параметров в управляемый профиль/API,
  - diversity-penalty между кандидатами в финальном ранжировании.

## 9) Обновление по рефакторингу top-K/business scorer (выполнено)
- В `vendor/cut-optimizer-2d` добавлен возврат `top-K` кандидатов популяции (`optimize_*_top_k`) с сохранением обратной совместимости `optimize_*` (`k=1`).
- В сервисе финальный выбор кандидата теперь идёт по бизнес-ранжированию среди `top-K` и прокинут в API как телеметрия:
  - `summary.candidate_selection` с причинами отбраковки:
    - `candidates_rejected_primary_objective`
    - `candidates_rejected_tie_bbox_void`
    - `candidates_rejected_tie_bbox_area`
    - `candidates_rejected_tie_perimeter`
    - `candidates_rejected_equal`
  - и со snapshot-метриками победителя (`winner_*`).
- GA-параметры вынесены в API:
  - `params.ga_profile`: `fast|balanced|quality`
  - `params.ga_override`: `epochs`, `breed_factor`, `survival_factor`, `top_k_candidates`.

## 10) Baseline-артефакты после рефакторинга
- Oversized baseline (`tests/fixtures/multisheet_oversized.json`, `s60`, `restarts=2`):
  - все алгоритмы: `200=60/60`, `hard_ok=60/60`, `selected_top_n=10`.
  - артефакты: `ai_docs/tmp/top10_algorithms_baseline_oversized_s60/report.json`.
- Varied baseline с прежними лимитами (`2k/3k/4.9k/1.25k`) дал сплошной timeout:
  - все алгоритмы: `408=60/60`.
  - артефакты: `ai_docs/tmp/top10_algorithms_baseline_varied4_s60/report.json`.
- Varied baseline с повышенными лимитами (`6k/9k/12k/6k`) стал рабочим:
  - standard: `200=47/60`, `408=13/60`, `hard_ok=41/60`.
  - portfolio: `200=60/60`, `hard_ok=54/60`.
  - beam: `200=53/60`, `408=7/60`, `hard_ok=49/60`.
  - alns: `200=60/60`, `hard_ok=60/60`.
  - артефакты: `ai_docs/tmp/top10_algorithms_baseline_varied4_s60_hi/report.json`.
- Вывод: fixed time-budget из oversized baseline не переносится на сложный varied-набор; для честного сравнения нужно указывать рабочие бюджеты per fixture/класс задачи.

## 11) Актуальный отбор top-10 по каждому алгоритму (последний прогон)
- Fixture: `tests/fixtures/multisheet_oversized.json`.
- Параметры: `seeds=60`, `restarts=2`, лимиты:
  - standard `2000ms`
  - portfolio `3000ms`
  - beam `4900ms`
  - alns `1250ms`
- Сводка: у всех алгоритмов `200=60/60`, `hard_ok=60/60`, `selected_top_n=10`.
- Отчёт: `ai_docs/tmp/top10_algorithms_current_s60/report.json`.
- Папки с раскладками:
  - standard: `ai_docs/tmp/top10_algorithms_current_s60/standard`
  - portfolio: `ai_docs/tmp/top10_algorithms_current_s60/portfolio`
  - beam: `ai_docs/tmp/top10_algorithms_current_s60/beam`
  - alns: `ai_docs/tmp/top10_algorithms_current_s60/alns`
- В каждой папке:
  - `manifest.json`
  - `overview.png`
  - `rank_01...rank_10_*.svg`
- Практический вывод: на этом конкретном наборе top-10 между алгоритмами снова очень близки по форме; различия в основном на уровне seed/локальных перестановок, а не в макро-структуре раскладки.
