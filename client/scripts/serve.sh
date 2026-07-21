#!/usr/bin/env bash
# Boot a throwaway Postgres + the MELDWORLD server, then run a command with
# MELD_SERVER pointing at it. Defaults to launching the Bevy client window.
#
#   client/scripts/serve.sh                    # → boots server + opens the client
#   client/scripts/serve.sh cargo run -p meld-client --bin smoke   # headless check
#
# Requires a local Postgres (initdb/pg_ctl/createdb on PATH).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PGDATA="${MELD_PGDATA:-$ROOT/target/pg}"; PGPORT="${MELD_PGPORT:-5433}"
PGUSER="${MELD_PGUSER:-$(whoami)}"; SOCKDIR="${MELD_PGSOCK:-/tmp/meldworld-pg}"; DBNAME=meldworld
ADDR="${MELD_ADDR:-127.0.0.1:18090}"   # matches the browser client's Trunk.toml proxy
export LC_ALL=C LANG=C
mkdir -p "$SOCKDIR"
# Reuse a Postgres already listening on PGPORT (e.g. from another checkout /
# worktree, or a still-running previous session) instead of trying to start a
# second one — starting a second postmaster on the same port just fails with
# "could not start server". Only initdb/start our own when the port is free.
if pg_isready -q -h 127.0.0.1 -p "$PGPORT" 2>/dev/null; then
  echo "› reusing Postgres already listening on 127.0.0.1:$PGPORT"
else
  [ -d "$PGDATA/base" ] || initdb -D "$PGDATA" -U "$PGUSER" --auth=trust >/dev/null
  pg_ctl -D "$PGDATA" -o "-p $PGPORT -k $SOCKDIR" -l "$PGDATA/server.log" -w start >/dev/null
fi
createdb -p "$PGPORT" -h 127.0.0.1 -U "$PGUSER" "$DBNAME" 2>/dev/null || true
export MELD_DATABASE_URL="postgres://$PGUSER@127.0.0.1:$PGPORT/$DBNAME"

# Free OUR server port first. A stale server from a previous session (or a
# crashed run) that still holds $ADDR would make the freshly-built server fail to
# bind — and the health check below would then pass against the OLD binary, so
# the client silently plays against old code (the "minimap/perks don't work"
# trap). Scoped to THIS run's port only; never a blanket `pkill`, so it's safe
# beside other worktrees/agents on their own ports (AGENTS.md: stop by port).
SRVPORT="${ADDR##*:}"
STALE="$(lsof -nP -iTCP:"$SRVPORT" -sTCP:LISTEN -t 2>/dev/null || true)"
if [ -n "$STALE" ]; then
  echo "› freeing port $SRVPORT held by a stale server (pid $STALE)"
  # shellcheck disable=SC2086
  kill $STALE 2>/dev/null || true
  for _ in $(seq 1 10); do
    lsof -nP -iTCP:"$SRVPORT" -sTCP:LISTEN -t >/dev/null 2>&1 || break
    sleep 0.3
  done
  if lsof -nP -iTCP:"$SRVPORT" -sTCP:LISTEN -t >/dev/null 2>&1; then
    echo "› still held — forcing (SIGKILL)"
    lsof -nP -iTCP:"$SRVPORT" -sTCP:LISTEN -t 2>/dev/null | xargs kill -9 2>/dev/null || true
    sleep 0.3
  fi
fi

echo "› building + starting server on $ADDR ($(cd "$ROOT" && git rev-parse --short HEAD 2>/dev/null || echo '?'))"
( cd "$ROOT" && MELD_ADDR="$ADDR" cargo run -q -p meld-server ) >/tmp/meld-server.log 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null || true' EXIT
# Wait for OUR server to become healthy — but fail loudly if it dies first (e.g.
# the port was still blocked), instead of silently accepting a different server.
until curl -sf "http://$ADDR/v1/healthz" >/dev/null 2>&1; do
  if ! kill -0 "$SRV" 2>/dev/null; then
    echo "✗ server exited before it was healthy — last lines of /tmp/meld-server.log:" >&2
    tail -20 /tmp/meld-server.log >&2
    exit 1
  fi
  sleep 0.3
done
echo "› server healthy at http://$ADDR"

export MELD_SERVER="http://$ADDR"
if [ "$#" -eq 0 ]; then set -- cargo run -p meld-client; fi   # the Bevy window
echo "› MELD_SERVER=$MELD_SERVER  $*"
( cd "$ROOT/client" && "$@" )
