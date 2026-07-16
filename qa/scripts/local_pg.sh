#!/usr/bin/env bash
# Boot a throwaway local Postgres and run a command against it with
# MELD_DATABASE_URL set. Defaults to running the QA test suite.
#
#   qa/scripts/local_pg.sh                       # runs: cargo test -p meld-qa
#   qa/scripts/local_pg.sh cargo run -p meld-server
#
# Postgres data lives under target/pg (gitignored) and is reused across runs.
# Requires a local Postgres install (initdb / pg_ctl / createdb on PATH).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PGDATA="${MELD_PGDATA:-$ROOT/target/pg}"
PGPORT="${MELD_PGPORT:-5433}"
PGUSER="${MELD_PGUSER:-$(whoami)}"
# Postgres caps unix-socket dir paths at ~103 bytes, so keep it short.
SOCKDIR="${MELD_PGSOCK:-/tmp/meldworld-pg}"
DBNAME="meldworld"

# macOS: avoid "postmaster became multithreaded during startup".
export LC_ALL="${LC_ALL:-C}" LANG="${LANG:-C}"

mkdir -p "$SOCKDIR"

# Reuse a Postgres already listening on PGPORT (another checkout/worktree or a
# still-running session) rather than failing to start a second one on the port.
if pg_isready -q -h 127.0.0.1 -p "$PGPORT" 2>/dev/null; then
  echo "› reusing Postgres already listening on 127.0.0.1:$PGPORT"
else
  if [ ! -d "$PGDATA/base" ]; then
    echo "› initdb $PGDATA"
    initdb -D "$PGDATA" -U "$PGUSER" --auth=trust >/dev/null
  fi
  echo "› starting postgres on 127.0.0.1:$PGPORT"
  pg_ctl -D "$PGDATA" -o "-p $PGPORT -k $SOCKDIR" -l "$PGDATA/server.log" -w start
fi

createdb -p "$PGPORT" -h 127.0.0.1 -U "$PGUSER" "$DBNAME" 2>/dev/null || true

export MELD_DATABASE_URL="postgres://$PGUSER@127.0.0.1:$PGPORT/$DBNAME"
echo "› MELD_DATABASE_URL=$MELD_DATABASE_URL"

if [ "$#" -eq 0 ]; then
  set -- cargo test -p meld-qa
fi
echo "› $*"
exec "$@"
