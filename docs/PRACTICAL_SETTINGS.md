# Freecut Practical Settings Guide

Эта инструкция разделяет параметры Freecut на две группы:

- практические настройки, которые реально нужны в рабочем API;
- исследовательские настройки, которые нужны для перебора гипотез, benchmark-скриптов и визуального анализа, но не должны быть обычным production default.

## Короткий вывод

Для обычного практического использования достаточно управлять:

- `kerf_mm`;
- `spacing_mm`;
- `trim_mm`;
- `objective`;
- `layout_mode`;
- `time_limit_ms`;
- `restarts`;
- `include_svg`;
- `seed`, если нужна воспроизводимость;
- `group_shift`, если нужно пост-уплотнение крайних групп деталей.

Остальное (`ga_override`, ручные `zone_penalties`, `seed_offsets`, `portfolio`, `beam`, `alns`, `partition`) лучше считать исследовательскими или advanced-настройками.

## Kerf vs Spacing

`kerf_mm` и `spacing_mm` оба участвуют в расчете расстояния между деталями, но означают разные вещи.

```text
effective_gap_mm = kerf_mm + spacing_mm
```

### kerf_mm

`kerf_mm` - это физическая ширина реза, то есть сколько материала съедает инструмент.

Примеры:

- пильный диск шириной 3.2 мм;
- фреза диаметром 6.0 мм;
- лазерный рез с эффективной шириной 0.2 мм.

Если две детали стоят рядом и между ними проходит рез, инструмент должен иметь место для своей толщины. Эта толщина не остается ни одной детали, она превращается в потерянный материал.

Практическое правило:

```json
"kerf_mm": 3.2
```

ставить равным реальной ширине пропила/фрезы/луча.

### spacing_mm

`spacing_mm` - это дополнительный технологический зазор сверх ширины инструмента.

Он нужен не потому, что инструмент такой широкий, а потому что производство требует дополнительного безопасного расстояния.

Причины для spacing:

- нужен запас, чтобы детали не касались друг друга;
- материал может иметь сколы, вибрацию, люфт, прижимы;
- нужна перемычка между деталями;
- оператор хочет оставить технологический резерв;
- нужно снизить риск повреждения кромки соседней детали.

Практическое правило:

```json
"spacing_mm": 0.0
```

если дополнительный зазор не нужен.

```json
"spacing_mm": 1.0
```

если нужен небольшой технологический запас.

```json
"spacing_mm": 3.0
```

если материал/станок требуют заметный запас между деталями.

### Примеры

Пильный диск 3.2 мм, дополнительный запас не нужен:

```json
{
  "kerf_mm": 3.2,
  "spacing_mm": 0.0
}
```

Фреза 6 мм и нужен 1 мм технологический запас:

```json
{
  "kerf_mm": 6.0,
  "spacing_mm": 1.0
}
```

Лазер 0.2 мм и детали можно класть почти вплотную:

```json
{
  "kerf_mm": 0.2,
  "spacing_mm": 0.0
}
```

Итоговый зазор между деталями будет:

```text
kerf_mm + spacing_mm
```

Например:

```json
{
  "kerf_mm": 3.2,
  "spacing_mm": 1.0
}
```

даст эффективный зазор `4.2 мм`.

### Что ставить на практике

Для CNC/фрезеровки:

```json
{
  "kerf_mm": 6.0,
  "spacing_mm": 0.5
}
```

или:

```json
{
  "kerf_mm": 6.0,
  "spacing_mm": 1.0
}
```

Для мебельного раскроя пилой:

```json
{
  "kerf_mm": 3.2,
  "spacing_mm": 0.0
}
```

или:

```json
{
  "kerf_mm": 3.2,
  "spacing_mm": 0.5
}
```

Для лазера:

```json
{
  "kerf_mm": 0.1,
  "spacing_mm": 0.0
}
```

или:

```json
{
  "kerf_mm": 0.2,
  "spacing_mm": 0.0
}
```

Если есть сомнение, лучше не завышать оба параметра одновременно. Завышение `kerf_mm + spacing_mm` ухудшает плотность раскроя.

## Рекомендуемый Production Default

Это базовый профиль для рабочего API, где важны скорость, стабильность и предсказуемое поведение.

```json
{
  "params": {
    "kerf_mm": 2.0,
    "spacing_mm": 1.0,
    "trim_mm": {
      "left": 0.0,
      "right": 0.0,
      "top": 0.0,
      "bottom": 0.0
    },
    "objective": "min_waste",
    "layout_mode": "guillotine",
    "time_limit_ms": 2000,
    "restarts": 10,
    "sla_profile": "balanced",
    "ga_profile": "balanced",
    "include_svg": true,
    "retry_strategy": "smart"
  }
}
```

Почему так:

- `layout_mode: "guillotine"` безопаснее как default для производства, где важна реалистичная последовательность реза.
- `objective: "min_waste"` обычно соответствует задаче минимизации остатка.
- `time_limit_ms: 2000` и `restarts: 10` дают нормальный баланс качества и времени.
- `retry_strategy: "smart"` полезен в рабочем API: сервис может сам сделать recovery-попытку при неудачном первом результате.
- `include_svg: true` удобно для аудита результата, но в высоконагруженной интеграции можно ставить `false`.

## Production Quality Profile

Если раскрой сложнее и допустима более высокая цена по времени:

```json
{
  "params": {
    "objective": "min_waste",
    "layout_mode": "guillotine",
    "time_limit_ms": 4000,
    "restarts": 8,
    "sla_profile": "balanced",
    "ga_profile": "quality",
    "include_svg": true,
    "retry_strategy": "smart"
  }
}
```

Использовать для:

- дорогого материала;
- заказов с большим количеством деталей;
- случаев, где важнее качество раскроя, чем быстрый ответ;
- ручного подтверждения раскроя оператором.

Не стоит делать это единственным default для всех запросов, потому что время ответа будет выше.

## Настройки Group Shift Для Практического Использования

`group_shift` - это postprocess, который сдвигает периферийные группы деталей к основной плотной группе. Это именно тот механизм, который был проверен в V29-V33 и дальше переоценен через contact/anchor метрики.

Рекомендуемый практический профиль:

```json
{
  "params": {
    "group_shift": {
      "enabled": true,
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  }
}
```

Эти значения совпадают с рабочим режимом V29-V33:

- `min_shift_mm: 5.0` отсекает мелкие бессмысленные движения;
- `max_passes: 4` обычно достаточно, чтобы сдвинуть не одну деталь, а несколько групп;
- `debug_artifacts` не включается, чтобы не раздувать response.

Минимально можно передать:

```json
{
  "params": {
    "group_shift": {}
  }
}
```

Если объект `group_shift` присутствует, текущие defaults такие:

- `enabled = true`;
- `min_shift_mm = 5.0`;
- `max_passes = 4`;
- `debug_artifacts = false`.

## Group Shift Для Визуального Аудита

Если нужно понять, что именно сдвинулось, включать debug SVG:

```json
{
  "params": {
    "include_svg": true,
    "retry_strategy": "disabled",
    "group_shift": {
      "enabled": true,
      "debug_artifacts": true,
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  }
}
```

В ответе появятся:

- `artifacts.svg` - финальный раскрой после group shift;
- `artifacts.group_shift_before_svg` - раскрой до group shift;
- `artifacts.group_shift_diff_svg` - визуальный diff сдвигов.

Для честного before/after сравнения лучше ставить:

```json
"retry_strategy": "disabled"
```

И задавать фиксированный:

```json
"seed": 12345
```

Так вы сравниваете один и тот же исходный раскрой до/после postprocess, а не разные случайные попытки.

## Более Сильный Group Shift

Если нужно агрессивнее сдвигать крайние группы:

```json
{
  "params": {
    "group_shift": {
      "enabled": true,
      "min_shift_mm": 3.0,
      "max_passes": 6
    }
  }
}
```

Использовать осторожно:

- `min_shift_mm: 3.0` разрешает больше мелких движений;
- `max_passes: 6` даёт больше попыток;
- качество может стать лучше визуально, но иногда ухудшается форма остатка по zone-метрикам.

Для production default лучше оставить:

```json
{
  "min_shift_mm": 5.0,
  "max_passes": 4
}
```

## Практический Профиль С Group Shift

Хороший рабочий payload для обычного использования:

```json
{
  "units": "mm",
  "params": {
    "kerf_mm": 2.0,
    "spacing_mm": 1.0,
    "trim_mm": {
      "left": 0.0,
      "right": 0.0,
      "top": 0.0,
      "bottom": 0.0
    },
    "objective": "min_waste",
    "layout_mode": "guillotine",
    "time_limit_ms": 3000,
    "restarts": 3,
    "sla_profile": "balanced",
    "ga_profile": "balanced",
    "include_svg": true,
    "retry_strategy": "smart",
    "group_shift": {
      "enabled": true,
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  },
  "stock": [],
  "items": []
}
```

Здесь `stock` и `items` должны быть заполнены реальными листами и деталями.

## Когда Включать Group Shift

Включать:

- когда на визуальном раскрое остаются узкие коридоры между основной группой и крайними деталями;
- когда нужна более плотная группа деталей и более цельный остаток;
- когда оператор визуально проверяет SVG;
- когда важна компактность группы, а не только формальный процент отхода.

Не включать вслепую:

- если раскрой уже плотный и без заметных внутренних коридоров;
- если каждая миллисекунда ответа важна;
- если downstream-система не готова принимать postprocess-сдвиги;
- если вам нужна строгая неизменность baseline-результата для benchmark.

## Profile Pool В Практике

`profile_pool` запускает несколько вариантов `zone_penalty` и выбирает лучший. Это полезно, но дороже по времени.

Практический умеренный вариант для сложных раскроев:

```json
{
  "params": {
    "profile_pool": {
      "enabled": true,
      "preset": "balanced_quality",
      "max_lead_drop_pp": 0.8
    }
  }
}
```

Использовать:

- для сложных заказов;
- когда обычный результат визуально дробит остаток;
- когда время ответа 4-10 секунд допустимо;
- для дорогих материалов.

Не делать обычным default для всех запросов:

- profile pool умножает число внутренних запусков;
- `seed_offsets` резко увеличивают время;
- без визуального контроля можно получить математически приемлемый, но визуально спорный результат.

## Profile Pool + Group Shift

Для максимально качественного, но более дорогого режима:

```json
{
  "params": {
    "time_limit_ms": 4000,
    "restarts": 5,
    "objective": "min_waste",
    "layout_mode": "guillotine",
    "retry_strategy": "disabled",
    "include_svg": true,
    "profile_pool": {
      "enabled": true,
      "zone_penalties": [0.2, 0.3, 0.4, 0.5, 0.6, 0.8],
      "fill_penalty": 0.1,
      "max_lead_drop_pp": 0.8,
      "seed_offsets": [1, 2, 3, 5, 7, 8, 13, 21],
      "rescue_when_zones_gt": 5
    },
    "group_shift": {
      "enabled": true,
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  }
}
```

Это не production default. Это quality/research режим.

По результатам V52/V53:

- `seed_offsets` помогают найти 4-листовой раскрой там, где обычный pool мог дать 5 листов;
- `group_shift` может визуально улучшать компактность;
- но `group_shift` без дополнительного acceptance guard иногда ухудшает zone count.

Поэтому этот режим полезен для сложных раскроев, но требует просмотра SVG и telemetry.

## Что Оставить Только Для Исследований

Следующие параметры не стоит отдавать обычному пользователю как стандартные настройки.

### ga_override

```json
{
  "ga_override": {
    "epochs": 180,
    "breed_factor": 0.55,
    "survival_factor": 0.7,
    "top_k_candidates": 12,
    "zone_penalty": 0.4,
    "fill_penalty": 0.1
  }
}
```

Использовать только для:

- benchmark;
- подбора параметров;
- регрессионных исследований;
- сравнения гипотез.

В рабочем API лучше использовать `ga_profile`.

### Ручные zone_penalties

```json
{
  "profile_pool": {
    "zone_penalties": [0.2, 0.3, 0.4, 0.5, 0.6, 0.8]
  }
}
```

Это research-level настройка. Для production лучше:

```json
{
  "profile_pool": {
    "preset": "balanced_quality"
  }
}
```

### seed_offsets

```json
{
  "profile_pool": {
    "seed_offsets": [1, 2, 3, 5, 7, 8, 13, 21]
  }
}
```

Полезно для исследований и дорогого quality mode, но не для обычного default. Причина: каждый offset умножает число candidate runs.

### debug_artifacts

```json
{
  "group_shift": {
    "debug_artifacts": true
  }
}
```

Нужно для визуального аудита и разработки. В production обычно ставить `false`, иначе response становится больше.

### portfolio, beam, alns

Эти режимы полезны для экспериментов и альтернативных orchestration-стратегий.

Обычному API-клиенту лучше начинать с:

```json
"POST /v1/optimize"
```

А не с:

```text
POST /v1/optimize/beam
POST /v1/optimize/alns
```

### partition

`partition` может быть полезен для экспериментов с dense-first раскладкой, но не должен быть default, пока нет стабильной политики выбора случаев, где он улучшает результат.

## Что Показывать Пользователю В UI

Для пользовательского интерфейса лучше не показывать все внутренние параметры.

Показывать:

- `layout_mode`: guillotine / nested;
- `objective`: minimum waste / minimum sheets;
- quality level: fast / balanced / quality;
- checkbox `Уплотнять группы деталей`;
- checkbox `Показывать SVG`;
- optional seed for reproducibility.

Не показывать обычному пользователю:

- `ga_override`;
- `zone_penalties`;
- `fill_penalty`;
- `seed_offsets`;
- `rescue_when_zones_gt`;
- `beam_width`;
- `alns.temperature_*`;
- `reaction_factor`;
- `partition.sheet_budget_ms`.

## Mapping UI Quality Level To API

### Fast

```json
{
  "time_limit_ms": 1000,
  "restarts": 4,
  "sla_profile": "fast",
  "ga_profile": "fast",
  "retry_strategy": "smart"
}
```

### Balanced

```json
{
  "time_limit_ms": 2000,
  "restarts": 10,
  "sla_profile": "balanced",
  "ga_profile": "balanced",
  "retry_strategy": "smart"
}
```

### Quality

```json
{
  "time_limit_ms": 4000,
  "restarts": 8,
  "sla_profile": "balanced",
  "ga_profile": "quality",
  "retry_strategy": "smart",
  "profile_pool": {
    "enabled": true,
    "preset": "balanced_quality",
    "max_lead_drop_pp": 0.8
  }
}
```

### Quality + Group Compaction

```json
{
  "time_limit_ms": 4000,
  "restarts": 8,
  "sla_profile": "balanced",
  "ga_profile": "quality",
  "retry_strategy": "smart",
  "profile_pool": {
    "enabled": true,
    "preset": "balanced_quality",
    "max_lead_drop_pp": 0.8
  },
  "group_shift": {
    "enabled": true,
    "min_shift_mm": 5.0,
    "max_passes": 4
  }
}
```

## Рекомендуемые Defaults Для Интеграции

Если нужно выбрать один практический default:

```json
{
  "objective": "min_waste",
  "layout_mode": "guillotine",
  "time_limit_ms": 2000,
  "restarts": 10,
  "sla_profile": "balanced",
  "ga_profile": "balanced",
  "include_svg": true,
  "retry_strategy": "smart"
}
```

Если нужно выбрать один practical quality default с group shift:

```json
{
  "objective": "min_waste",
  "layout_mode": "guillotine",
  "time_limit_ms": 3000,
  "restarts": 3,
  "sla_profile": "balanced",
  "ga_profile": "balanced",
  "include_svg": true,
  "retry_strategy": "smart",
  "group_shift": {
    "enabled": true,
    "min_shift_mm": 5.0,
    "max_passes": 4
  }
}
```

Если нужно выбрать research preset для поиска лучших раскроев:

```json
{
  "objective": "min_waste",
  "layout_mode": "guillotine",
  "time_limit_ms": 4000,
  "restarts": 5,
  "include_svg": true,
  "retry_strategy": "disabled",
  "profile_pool": {
    "enabled": true,
    "zone_penalties": [0.2, 0.3, 0.4, 0.5, 0.6, 0.8],
    "fill_penalty": 0.1,
    "max_lead_drop_pp": 0.8,
    "seed_offsets": [1, 2, 3, 5, 7, 8, 13, 21],
    "rescue_when_zones_gt": 5
  },
  "group_shift": {
    "enabled": true,
    "debug_artifacts": true,
    "min_shift_mm": 5.0,
    "max_passes": 4
  }
}
```

## Operational Rules

1. For production, start with simple settings and inspect SVG.
2. Enable `group_shift` when visual gaps between edge groups and main group matter.
3. Enable `debug_artifacts` only for analysis.
4. Use fixed `seed` when comparing layouts.
5. Use `retry_strategy: "disabled"` for exact before/after benchmark.
6. Use `retry_strategy: "smart"` for real user-facing API.
7. Do not expose raw GA and profile-pool internals to ordinary users.
8. Keep `profile_pool.seed_offsets` for expensive quality mode or research.
9. Treat `waste_percent` as necessary but not sufficient; always inspect SVG for important jobs.
10. For group-shift quality, look at `summary.group_shift.contact_gain_mm`, `moves_applied`, and before/diff SVG.
