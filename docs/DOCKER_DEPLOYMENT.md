# Freecut Docker Deployment Guide

This guide explains how to deploy Freecut as a Docker service.

## What The Dockerfile Builds

The current `Dockerfile` uses a multi-stage build:

1. `rust:1.93-bookworm` builds the release binary.
2. `debian:bookworm-slim` runs the compiled `/app/freecut` binary.

The runtime image contains:

- `/app/freecut`;
- `curl` for Docker health checks;
- `ca-certificates`;
- default service environment variables.

By default, the container listens on:

```text
8088
```

## Quick Start

From the repository root:

```bash
docker build -t freecut-mvp .
docker run --rm -p 8088:8088 freecut-mvp
```

Check service health:

```bash
curl http://127.0.0.1:8088/health/live
curl http://127.0.0.1:8088/health/ready
curl http://127.0.0.1:8088/version
```

Swagger UI:

```text
http://127.0.0.1:8088/docs
```

OpenAPI JSON:

```text
http://127.0.0.1:8088/openapi.json
```

## Production Run Command

Practical production-style `docker run` command:

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

Check status:

```bash
docker ps
docker logs -f freecut
docker inspect --format='{{json .State.Health}}' freecut
```

Stop and remove:

```bash
docker stop freecut
docker rm freecut
```

## Environment Variables

| Variable | Default | Description |
|---|---:|---|
| `PORT` | `8088` | HTTP port inside the container. |
| `RUST_LOG` | `info` | Rust tracing/log level. |
| `MAX_BODY_BYTES` | `5242880` | Maximum JSON request body size. |
| `MAX_INSTANCES` | `5000` | Maximum total item instances in one request. |
| `DEFAULT_TIME_LIMIT_MS` | `2000` | Default optimizer time budget when request omits `params.time_limit_ms`. |
| `DEFAULT_RESTARTS` | `10` | Default restart count when request omits `params.restarts`. |
| `MAX_CONCURRENT_OPTIMIZE` | CPU count, minimum `1` | Maximum concurrent optimize requests. |

If you change the internal `PORT`, also change the Docker port mapping:

```bash
docker run -d \
  --name freecut \
  -e PORT=8090 \
  -p 8090:8090 \
  freecut-mvp
```

Usually it is simpler to keep `PORT=8088` inside the container and change only the host port:

```bash
docker run -d \
  --name freecut \
  -p 9000:8088 \
  freecut-mvp
```

The service will then be available on the host at:

```text
http://127.0.0.1:9000
```

## docker compose

Example `docker-compose.yml`:

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

Start:

```bash
docker compose up -d --build
```

Logs:

```bash
docker compose logs -f freecut
```

Stop:

```bash
docker compose down
```

## Smoke Test

The repository includes a smoke test script:

```bash
./scripts/docker_smoke.sh
```

It checks:

- `/health/live`;
- `/health/ready`;
- `/version`;
- `/openapi.json`;
- `/docs`;
- a valid `/v1/optimize` request;
- invalid trim handling;
- invalid JSON handling.

The Freecut container must already be running:

```bash
docker build -t freecut-mvp .
docker run --rm -p 8088:8088 freecut-mvp
```

In another terminal:

```bash
./scripts/docker_smoke.sh
```

If the service is available at another URL:

```bash
BASE_URL=http://127.0.0.1:9000 ./scripts/docker_smoke.sh
```

On Linux, the script uses `docker run --network host` for the curl container.

## Test The Optimize API

Example request:

```bash
curl -sS -X POST "http://127.0.0.1:8088/v1/optimize" \
  -H "Content-Type: application/json" \
  --data-binary @examples/optimize_request.json
```

Smaller response without SVG:

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

For production, limit CPU/RAM at the Docker, Compose, or orchestration layer.

Example `docker run`:

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

Practical rules:

- do not set `MAX_CONCURRENT_OPTIMIZE` much higher than available CPU cores;
- for heavy requests, start with `MAX_CONCURRENT_OPTIMIZE=2..4`;
- for many small requests, increase it only after load testing.

## Updating The Service

Update after local source changes:

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

With Compose:

```bash
git pull
docker compose up -d --build
```

## Logs

View logs:

```bash
docker logs -f freecut
```

Increase logging detail:

```bash
docker run -d \
  --name freecut \
  -p 8088:8088 \
  -e RUST_LOG=debug \
  freecut-mvp
```

For normal production use:

```text
RUST_LOG=info
```

## Healthcheck

The Dockerfile already defines:

```dockerfile
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -fsS "http://localhost:${PORT}/health/live" || exit 1
```

Check status:

```bash
docker inspect --format='{{.State.Health.Status}}' freecut
```

Expected status:

```text
healthy
```

## Troubleshooting

### Port already in use

Symptom:

```text
Bind for 0.0.0.0:8088 failed: port is already allocated
```

Fix:

```bash
docker run -d --name freecut -p 9000:8088 freecut-mvp
```

Then use:

```text
http://127.0.0.1:9000
```

### Container is unhealthy

Check logs:

```bash
docker logs freecut
```

Check endpoint from inside the container:

```bash
docker exec freecut curl -fsS http://localhost:8088/health/live
```

### Request body too large

Increase:

```bash
-e MAX_BODY_BYTES=10485760
```

### Too many concurrent requests

The service returns `429 OVERLOADED` when all optimize slots are busy.

Increase:

```bash
-e MAX_CONCURRENT_OPTIMIZE=8
```

Only do this if the host has enough CPU/RAM.

### Optimization is too slow

For faster API responses, reduce request-side budget:

```json
{
  "time_limit_ms": 1000,
  "restarts": 4,
  "include_svg": false
}
```

For higher quality, increase it:

```json
{
  "time_limit_ms": 4000,
  "restarts": 8
}
```

## Recommended Production Defaults

For the container:

```bash
-e RUST_LOG=info
-e MAX_BODY_BYTES=5242880
-e MAX_INSTANCES=5000
-e DEFAULT_TIME_LIMIT_MS=2000
-e DEFAULT_RESTARTS=10
-e MAX_CONCURRENT_OPTIMIZE=4
```

For API requests:

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

If group-shift postprocess is needed:

```json
{
  "group_shift": {
    "enabled": true,
    "min_shift_mm": 5.0,
    "max_passes": 4
  }
}
```
