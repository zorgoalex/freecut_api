#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT_DIR=$(cd "$SCRIPT_DIR/.." && pwd)

BASE_URL=${BASE_URL:-http://127.0.0.1:8088}
CURL_IMAGE=${CURL_IMAGE:-"curlimages/curl:8.6.0"}

printf "Health live: "
code=$(docker run --rm --network host "$CURL_IMAGE" -sS -o /dev/null -w "%{http_code}" "$BASE_URL/health/live")
echo "$code"

printf "Health ready: "
code=$(docker run --rm --network host "$CURL_IMAGE" -sS -o /dev/null -w "%{http_code}" "$BASE_URL/health/ready")
echo "$code"

printf "Version: "
version_json=$(docker run --rm --network host "$CURL_IMAGE" -sS "$BASE_URL/version")
echo "$version_json"

printf "OpenAPI: "
openapi_code=$(docker run --rm --network host "$CURL_IMAGE" -sS -o /dev/null -w "%{http_code}" "$BASE_URL/openapi.json")
echo "$openapi_code"

printf "Docs: "
docs_code=$(docker run --rm --network host "$CURL_IMAGE" -sS -L -o /dev/null -w "%{http_code}" "$BASE_URL/docs")
echo "$docs_code"

printf "Optimize (valid): "
resp=$(cat "$ROOT_DIR/tests/fixtures/optimize_valid.json" | \
  docker run --rm -i --network host "$CURL_IMAGE" -sS -H "Content-Type: application/json" -d @- "$BASE_URL/v1/optimize")
echo "received"

printf "%s" "$resp" | python3 "$SCRIPT_DIR/freecut_check.py"

printf "Optimize (invalid trim): "
code=$(cat "$ROOT_DIR/tests/fixtures/optimize_invalid_trim.json" | \
  docker run --rm -i --network host "$CURL_IMAGE" -sS -o /dev/null -w "%{http_code}" -H "Content-Type: application/json" -d @- "$BASE_URL/v1/optimize")
echo "$code"

printf "Invalid JSON: "
code=$(docker run --rm --network host "$CURL_IMAGE" -sS -o /dev/null -w "%{http_code}" -H "Content-Type: application/json" -d "{" "$BASE_URL/v1/optimize")
echo "$code"
