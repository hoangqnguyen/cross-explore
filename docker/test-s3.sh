#!/usr/bin/env bash
# Run the cx-s3 integration tests against a throwaway MinIO.
#
#   docker/test-s3.sh [--keep] [-- extra cargo test args]
#   e.g. docker/test-s3.sh -- --test live -- --nocapture
#
# Brings up docker/s3/compose.yml, waits until MinIO answers and the `init`
# container has created the test buckets, runs `cargo test -p cx-s3` with
# CX_TEST_S3=1, then tears everything down (unless --keep is given).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
compose="$here/s3/compose.yml"

keep=0
cargo_args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --keep) keep=1 ;;
    --) shift; cargo_args=("$@"); break ;;
    *) cargo_args+=("$1") ;;
  esac
  shift
done

down() {
  if [ "$keep" = 0 ]; then
    docker compose -f "$compose" down -v --remove-orphans >/dev/null 2>&1 || true
  fi
}
trap down EXIT

docker compose -f "$compose" up -d --force-recreate

# MinIO's readiness probe needs no credentials.
for i in $(seq 1 120); do
  if curl -fsS -o /dev/null "http://127.0.0.1:9900/minio/health/ready" 2>/dev/null; then break; fi
  [ "$i" = 120 ] && { echo "timed out waiting for MinIO on 127.0.0.1:9900" >&2; exit 1; }
  sleep 1
done

# The public bucket's object is the last thing `init` creates; anonymous
# read works once its policy is set, so this also waits for that.
for i in $(seq 1 120); do
  if curl -fsS -o /dev/null "http://127.0.0.1:9900/cx-public/readme.txt" 2>/dev/null; then break; fi
  [ "$i" = 120 ] && { echo "timed out waiting for the test buckets (see: docker compose -f $compose logs init)" >&2; exit 1; }
  sleep 1
done

cd "$root"
CX_TEST_S3=1 cargo test -p cx-s3 ${cargo_args[@]+"${cargo_args[@]}"}
