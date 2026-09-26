#!/usr/bin/env bash
# Run the cx-smb / cx-webdav integration tests against throwaway servers.
#
#   docker/test-remote-2.sh smb       # Samba on 127.0.0.1:1445
#   docker/test-remote-2.sh webdav    # rclone (Basic) :8088 + Apache (Digest) :8089
#   docker/test-remote-2.sh all
#
# Starts the container(s), waits until they answer, runs the tests and tears
# everything down again (also on failure). Set KEEP=1 to leave them running.
set -euo pipefail

cd "$(dirname "$0")/.."

STARTED=()
cleanup() {
  [ "${KEEP:-}" = 1 ] && return
  for f in "${STARTED[@]+"${STARTED[@]}"}"; do
    docker compose -f "$f" down -v >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

start() {
  STARTED+=("$1")
  docker compose -f "$1" up -d
}

wait_for() { # description, command...
  local what=$1; shift
  for _ in $(seq 1 60); do
    if "$@" >/dev/null 2>&1; then
      echo "ready: $what"
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for $what" >&2
  return 1
}

run_smb() {
  local compose=docker/smb/compose.yml
  start "$compose"
  # Docker Desktop accepts connections on the published port before smbd
  # listens, so ask Samba itself.
  wait_for "samba" docker compose -f "$compose" exec -T samba smbclient -L localhost -U cx%cxpass -m SMB3
  CX_TEST_SMB=1 cargo test -p cx-smb --test live
  # The reconnect test restarts the server, so it runs on its own.
  CX_TEST_SMB=1 CX_SMB_RESTART="$(docker compose -f "$compose" ps -q samba)" \
    cargo test -p cx-smb --test live reconnects_after_server_restart
}

run_webdav() {
  local compose=docker/webdav/compose.yml
  start "$compose"
  wait_for "rclone webdav" sh -c 'curl -fs -o /dev/null -X PROPFIND -H "Depth: 0" -u cx:cxpass http://127.0.0.1:8088/'
  wait_for "apache webdav" sh -c 'curl -fs -o /dev/null -X PROPFIND -H "Depth: 0" --digest -u cx:cxpass http://127.0.0.1:8089/'
  CX_TEST_DAV=1 cargo test -p cx-webdav --test live
}

case "${1:-}" in
  smb) run_smb ;;
  webdav|dav) run_webdav ;;
  all) run_smb; run_webdav ;;
  *) echo "usage: $0 smb|webdav|all" >&2; exit 2 ;;
esac
