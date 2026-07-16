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
[ -d "$PGDATA/base" ] || initdb -D "$PGDATA" -U "$PGUSER" --auth=trust >/dev/null
pg_ctl -D "$PGDATA" status >/dev/null 2>&1 || \
  pg_ctl -D "$PGDATA" -o "-p $PGPORT -k $SOCKDIR" -l "$PGDATA/server.log" -w start >/dev/null
createdb -p "$PGPORT" -h 127.0.0.1 -U "$PGUSER" "$DBNAME" 2>/dev/null || true
export MELD_DATABASE_URL="postgres://$PGUSER@127.0.0.1:$PGPORT/$DBNAME"

echo "› building + starting server on $ADDR"
( cd "$ROOT" && MELD_ADDR="$ADDR" cargo run -q -p meld-server ) >/tmp/meld-server.log 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null || true' EXIT
until curl -sf "http://$ADDR/v1/healthz" >/dev/null 2>&1; do sleep 0.3; done
echo "› server healthy at http://$ADDR"

export MELD_SERVER="http://$ADDR"
if [ "$#" -eq 0 ]; then set -- cargo run -p meld-client; fi   # the Bevy window
echo "› MELD_SERVER=$MELD_SERVER  $*"
( cd "$ROOT/client" && "$@" )
