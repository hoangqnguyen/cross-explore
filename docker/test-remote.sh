#!/usr/bin/env bash
# Run a remote provider's integration tests against a throwaway server.
#
#   docker/test-remote.sh <protocol> [--keep] [-- extra cargo test args]
#   e.g. docker/test-remote.sh ftp -- -- --nocapture
#
# Brings up docker/<protocol>/compose.yml, waits until the server answers,
# runs the crate's tests with the env var that enables them, then tears the
# container down (unless --keep is given).
#
# Each protocol is one entry in the `case` below: the crate to test, the env
# var that enables its integration tests, the port to wait for and any setup.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"

proto="${1:-}"
[ -n "$proto" ] || { echo "usage: $0 <sftp|ftp|...> [--keep] [-- cargo args]" >&2; exit 2; }
shift
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

wait_port() { # host port what
  local i
  for i in $(seq 1 120); do
    if nc -z "$1" "$2" 2>/dev/null; then return 0; fi
    sleep 1
  done
  echo "timed out waiting for $3 on $1:$2" >&2
  return 1
}

wait_banner() { # host port prefix what — the port may accept before the daemon is ready
  local i
  for i in $(seq 1 120); do
    if (exec 3<>"/dev/tcp/$1/$2" && head -c 64 <&3 | grep -q "$3") 2>/dev/null; then return 0; fi
    sleep 1
  done
  echo "timed out waiting for $4 banner on $1:$2" >&2
  return 1
}

case "$proto" in
  sftp)
    crate=cx-sftp
    env_var=CX_TEST_SFTP
    compose="$here/sftp/compose.yml"
    mkdir -p "$here/sftp/keys"
    if [ ! -f "$here/sftp/keys/id_ed25519" ]; then
      ssh-keygen -q -t ed25519 -N "" -C cx-test -f "$here/sftp/keys/id_ed25519"
    fi
    chmod 600 "$here/sftp/keys/id_ed25519"
    ready() { wait_banner 127.0.0.1 2222 SSH- sftp && wait_banner 127.0.0.1 2223 SSH- sftp-modern; }
    ;;
  ftp)
    crate=cx-ftp
    env_var=CX_TEST_FTP
    compose="$here/ftp/compose.yml"
    ready() { wait_banner 127.0.0.1 2121 220 pure-ftpd && wait_banner 127.0.0.1 2122 220 vsftpd; }
    ;;
  *)
    echo "unknown protocol: $proto" >&2
    exit 2
    ;;
esac

down() {
  if [ "$keep" = 0 ]; then
    docker compose -f "$compose" down -v --remove-orphans >/dev/null 2>&1 || true
  fi
}
trap down EXIT

docker compose -f "$compose" up -d --force-recreate
ready
# Give daemons a moment after the banner (key generation, user setup).
sleep 2

cd "$root"
env "$env_var=1" cargo test -p "$crate" ${cargo_args[@]+"${cargo_args[@]}"}
