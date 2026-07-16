# MELDWORLD — one place for the commands you actually run.
#
#   make play         → boot everything + open the game in your browser
#   make play-native  → boot everything + the native desktop window
#   make test         → run the end-to-end test suite
#   make help         → list all targets
#
# All of these boot a throwaway local Postgres under target/pg (reused across
# runs) and the server on $(MELD_ADDR). Requires a local Postgres install
# (initdb/pg_ctl/createdb on PATH); `make play` also needs `trunk` for the wasm
# build (cargo install trunk).

MELD_ADDR ?= 127.0.0.1:18090
WEB_PORT  ?= 9080
SERVE     := client/scripts/serve.sh
PORT      := $(word 2,$(subst :, ,$(MELD_ADDR)))

export MELD_ADDR

.DEFAULT_GOAL := help
.PHONY: help play play-native server smoke test stop

help:
	@echo "MELDWORLD — common tasks:"
	@echo ""
	@echo "  make play         Boot Postgres + server, then serve the browser client."
	@echo "                    Open http://localhost:$(WEB_PORT) and press ENTER to play"
	@echo "                    (or open http://localhost:$(WEB_PORT)/?autoplay to watch it play itself)."
	@echo "  make play-native  Boot Postgres + server + the native desktop window."
	@echo "  make server       Boot Postgres + server only (no client)."
	@echo "  make smoke        Headless client run against the server (exits 0 on victory)."
	@echo "  make test         Run the end-to-end test suite (throwaway Postgres)."
	@echo "  make stop         Stop the local server (Postgres is left running)."
	@echo ""
	@echo "  server address: http://$(MELD_ADDR)   web client: http://localhost:$(WEB_PORT)"

# Browser client: serve.sh boots Postgres + the server (background), then runs
# trunk in the foreground. trunk serves the wasm client on WEB_PORT and proxies
# /v1 + the realtime socket to the server. First run compiles the wasm bundle
# (a minute or two); leave it running and open the URL below.
play:
	@echo "→ Building + starting the server and web client…"
	@echo "→ When trunk says 'server listening', open:  http://localhost:$(WEB_PORT)"
	@echo "→ Click the page, press ENTER to enter the maze (or use /?autoplay)."
	$(SERVE) "$(CURDIR)/client/scripts/trunk-serve.sh" --port $(WEB_PORT) --address 127.0.0.1 --no-autoreload

# Native desktop window (serve.sh's default command is `cargo run -p meld-client`).
play-native:
	$(SERVE)

# Server only — handy for pointing your own client/tests at it. Blocks until Ctrl-C.
server:
	$(SERVE) bash -c 'echo "server ready at $$MELD_SERVER — Ctrl-C to stop"; tail -f /dev/null'

smoke:
	$(SERVE) cargo run -p meld-client --bin smoke

test:
	bash qa/scripts/local_pg.sh cargo test -p meld-qa

stop:
	@lsof -ti tcp:$(PORT) 2>/dev/null | xargs -r kill 2>/dev/null || true
	@echo "server on port $(PORT) stopped (Postgres left running; 'make test'/'make play' reuse it)."
