# Freecut Docker Deployment Guide

Эта инструкция описывает, как развернуть Freecut как Docker-сервис.

## Что Собирает Dockerfile

Текущий `Dockerfile` использует multi-stage build:

1. `rust:1.93-bookworm` собирает release binary.
2. `debian:bookworm-slim` запускает готовый `/app/freecut`.

Runtime image содержит:

- бинарник `/app/freecut`;
- `curl` для Docker healthcheck;
- `ca-certificates`;
- env defaults сервиса.

По умолчанию контейнер слушает порт:

```text
8088
```

## Быстрый Запуск

Из корня репозитория:

```bash
docker build -t freecut-mvp .
docker run --rm -p 8088:8088 freecut-mvp
```

Проверка:

```bash
curl http://127.0.0.1:8088/health/live
curl http://127.0.0.1:8088/health/ready
curl http://127.0.0.1:8088/version
```

Swagger UI:

```text
http://127.0.0.1:8088/docs
```

OpenAPI:

```text
http://127.0.0.1:8088/openapi.json
```

## Production Run Command

Практический вариант запуска:

```bash
docker run -d \
  --name freecut \
  --restart unless-stopped \
  -p 8088:8088 \
  -e RUST_LOG=info \
  -e MAX_BODY_BYTES=5242880 \
  -e MAX_INSTANCES=5000 \
  -e DEFAULT_TIME_LIMIT_MS=2000 \
  -e DEFAULT_RESTARTS=10 \
  -e MAX_CONCURRENT_OPTIMIZE=4 \
  freecut-mvp
```

Проверить статус:

```bash
docker ps
docker logs -f freecut
docker inspect --format='{{json .State.Health}}' freecut
```

Остановить:

```bash
docker stop freecut
docker rm freecut
```

## Environment Variables

| Variable | Default | Description |
|---|---:|---|
| `PORT` | `8088` | HTTP port inside the container. |
| `RUST_LOG` | `info` | Rust tracing/log level. |
| `MAX_BODY_BYTES` | `5242880` | Max JSON request body size. |
| `MAX_INSTANCES` | `5000` | Max total item instances in one request. |
| `DEFAULT_TIME_LIMIT_MS` | `2000` | Default optimizer time budget if request omits `params.time_limit_ms`. |
| `DEFAULT_RESTARTS` | `10` | Default restarts if request omits `params.restarts`. |
| `MAX_CONCURRENT_OPTIMIZE` | CPU count, min `1` | Max concurrent optimize requests. |

Если контейнерный порт меняется через `PORT`, надо менять и Docker port mapping:

```bash
docker run -d \
  --name freecut \
  -e PORT=8090 \
  -p 8090:8090 \
  freecut-mvp
```

Обычно проще оставить `PORT=8088` внутри контейнера и менять только внешний порт:

```bash
docker run -d \
  --name freecut \
  -p 9000:8088 \
  freecut-mvp
```

Тогда сервис будет доступен с хоста:

```text
http://127.0.0.1:9000
```

## docker compose

Создать `docker-compose.yml`:

```yaml
services:
  freecut:
    build:
      context: .
      dockerfile: Dockerfile
    image: freecut-mvp:local
    container_name: freecut
    restart: unless-stopped
    ports:
      - "8088:8088"
    environment:
      PORT: "8088"
      RUST_LOG: "info"
      MAX_BODY_BYTES: "5242880"
      MAX_INSTANCES: "5000"
      DEFAULT_TIME_LIMIT_MS: "2000"
      DEFAULT_RESTARTS: "10"
      MAX_CONCURRENT_OPTIMIZE: "4"
    healthcheck:
      test: ["CMD", "curl", "-fsS", "http://localhost:8088/health/live"]
      interval: 30s
      timeout: 3s
      start_period: 5s
      retries: 3
```

Запуск:

```bash
docker compose up -d --build
```

Логи:

```bash
docker compose logs -f freecut
```

Остановка:

```bash
docker compose down
```

## Smoke Test

В репозитории есть smoke test:

```bash
./scripts/docker_smoke.sh
```

Он проверяет:

- `/health/live`;
- `/health/ready`;
- `/version`;
- `/openapi.json`;
- `/docs`;
- валидный `/v1/optimize`;
- invalid trim case;
- invalid JSON case.

Перед запуском smoke test контейнер должен быть уже запущен:

```bash
docker build -t freecut-mvp .
docker run --rm -p 8088:8088 freecut-mvp
```

В другом терминале:

```bash
./scripts/docker_smoke.sh
```

Если сервис доступен на другом адресе:

```bash
BASE_URL=http://127.0.0.1:9000 ./scripts/docker_smoke.sh
```

На Linux скрипт использует `docker run --network host` для curl-контейнера.

## Проверка Optimize API

Пример запроса:

```bash
curl -sS -X POST "http://127.0.0.1:8088/v1/optimize" \
  -H "Content-Type: application/json" \
  --data-binary @examples/optimize_request.json
```

Без SVG, чтобы ответ был меньше:

```bash
curl -sS -X POST "http://127.0.0.1:8088/v1/optimize" \
  -H "Content-Type: application/json" \
  -d '{
    "units": "mm",
    "params": {
      "kerf_mm": 2.0,
      "spacing_mm": 1.0,
      "trim_mm": { "left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0 },
      "objective": "min_waste",
      "layout_mode": "guillotine",
      "time_limit_ms": 1000,
      "restarts": 3,
      "include_svg": false
    },
    "stock": [
      { "id": "sheet-1000", "width_mm": 1000.0, "height_mm": 1000.0, "qty": 2 }
    ],
    "items": [
      { "id": "A", "width_mm": 200.0, "height_mm": 300.0, "qty": 2, "rotation": "allow_90", "pattern_direction": "none" },
      { "id": "B", "width_mm": 400.0, "height_mm": 400.0, "qty": 1, "rotation": "allow_90", "pattern_direction": "none" }
    ]
  }'
```

## Resource Settings

Для production желательно ограничить CPU/RAM контейнера на уровне Docker/Compose/оркестратора.

Пример `docker run`:

```bash
docker run -d \
  --name freecut \
  --restart unless-stopped \
  -p 8088:8088 \
  --cpus="4" \
  --memory="2g" \
  -e MAX_CONCURRENT_OPTIMIZE=4 \
  freecut-mvp
```

Практическое правило:

- `MAX_CONCURRENT_OPTIMIZE` не ставить сильно выше доступных CPU cores;
- если запросы тяжелые, начать с `MAX_CONCURRENT_OPTIMIZE=2..4`;
- если много мелких запросов, можно увеличить после нагрузочного теста.

## Updating The Service

Обновление при локальной сборке:

```bash
git pull
docker build -t freecut-mvp .
docker stop freecut
docker rm freecut
docker run -d \
  --name freecut \
  --restart unless-stopped \
  -p 8088:8088 \
  freecut-mvp
```

Если используется compose:

```bash
git pull
docker compose up -d --build
```

## Logs

Посмотреть логи:

```bash
docker logs -f freecut
```

Увеличить детализацию:

```bash
docker run -d \
  --name freecut \
  -p 8088:8088 \
  -e RUST_LOG=debug \
  freecut-mvp
```

Для обычного production лучше:

```text
RUST_LOG=info
```

## Healthcheck

В Dockerfile уже задан:

```dockerfile
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -fsS "http://localhost:${PORT}/health/live" || exit 1
```

Проверка:

```bash
docker inspect --format='{{.State.Health.Status}}' freecut
```

Ожидаемый статус:

```text
healthy
```

## Troubleshooting

### Port already in use

Симптом:

```text
Bind for 0.0.0.0:8088 failed: port is already allocated
```

Решение:

```bash
docker run -d --name freecut -p 9000:8088 freecut-mvp
```

И обращаться к:

```text
http://127.0.0.1:9000
```

### Container is unhealthy

Проверить логи:

```bash
docker logs freecut
```

Проверить endpoint из контейнера:

```bash
docker exec freecut curl -fsS http://localhost:8088/health/live
```

### Request body too large

Увеличить:

```bash
-e MAX_BODY_BYTES=10485760
```

### Too many concurrent requests

Сервис вернет `429 OVERLOADED`, если заняты все optimize slots.

Увеличить:

```bash
-e MAX_CONCURRENT_OPTIMIZE=8
```

Но только если хватает CPU/RAM.

### Optimization too slow

Для API-клиента уменьшить:

```json
{
  "time_limit_ms": 1000,
  "restarts": 4,
  "include_svg": false
}
```

Для качества увеличить:

```json
{
  "time_limit_ms": 4000,
  "restarts": 8
}
```

## Recommended Production Defaults

Для контейнера:

```bash
-e RUST_LOG=info
-e MAX_BODY_BYTES=5242880
-e MAX_INSTANCES=5000
-e DEFAULT_TIME_LIMIT_MS=2000
-e DEFAULT_RESTARTS=10
-e MAX_CONCURRENT_OPTIMIZE=4
```

Для API-запросов:

```json
{
  "objective": "min_waste",
  "layout_mode": "guillotine",
  "time_limit_ms": 2000,
  "restarts": 10,
  "include_svg": true,
  "retry_strategy": "smart"
}
```

Если нужен group-shift postprocess:

```json
{
  "group_shift": {
    "enabled": true,
    "min_shift_mm": 5.0,
    "max_passes": 4
  }
}
```

