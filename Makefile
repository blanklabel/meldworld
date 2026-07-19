# MELDWORLD — one place for the commands you actually run.
#
#   make play         → build the web client, boot everything, play in a browser
#   make play-native  → boot everything + the native desktop window
#   make test         → run the end-to-end test suite
#   make help         → list all targets
#
# All of these boot a throwaway local Postgres under target/pg (reused across
# runs) and the server on $(MELD_ADDR). Requires a local Postgres install
# (initdb/pg_ctl/createdb on PATH); `make play` also needs `trunk`
# (cargo install trunk) + the wasm target (rustup target add wasm32-unknown-unknown).

MELD_ADDR ?= 127.0.0.1:18090
SERVE     := client/scripts/serve.sh
PORT      := $(word 2,$(subst :, ,$(MELD_ADDR)))
DIST      := $(CURDIR)/client/crates/meld-client/dist
URL       := http://$(MELD_ADDR)

# The self-contained ("play-solo"/"dist") build lives in the client workspace and
# writes to its target dir — honour a shared CARGO_TARGET_DIR if one is set, else
# the per-workspace client/target. `meld-client` is the default-run binary name.
CLIENT_TARGET ?= $(if $(CARGO_TARGET_DIR),$(CARGO_TARGET_DIR),$(CURDIR)/client/target)
SOLO_BIN      := $(CLIENT_TARGET)/release/meld-client
DIST_OUT      := $(CURDIR)/dist
DIST_NAME     := meldworld-$(shell uname -s | tr '[:upper:]' '[:lower:]')-$(shell uname -m)

export MELD_ADDR

.DEFAULT_GOAL := help
.PHONY: help play play-native play-solo dist look server smoke test stop

help:
	@echo "MELDWORLD — common tasks:"
	@echo ""
	@echo "  make play         Build the web client + boot Postgres + server, then"
	@echo "                     open $(URL) in your browser and press ENTER"
	@echo "                     (or open $(URL)/?autoplay to watch it play itself)."
	@echo "  make play-native  Boot Postgres + server + the native desktop window."
	@echo "  make play-solo    Run the SELF-CONTAINED build: one native window, server"
	@echo "                     baked in (in-memory DB, no Postgres, no config). Great"
	@echo "                     for a quick local try; state is ephemeral (resets on exit)."
	@echo "  make dist         Build the shippable single-file QA binary (embeds the"
	@echo "                     server + all assets). Hand the one file to a remote tester;"
	@echo "                     they just run it — no Rust, no Postgres, nothing beside it."
	@echo "  make look         HD-2D render look-dev scene (standalone; tune it live, native)."
	@echo "  make server       Boot Postgres + server only (no client)."
	@echo "  make smoke        Headless client run against the server (exits 0 on victory)."
	@echo "  make test         Run the end-to-end test suite (throwaway Postgres)."
	@echo "  make stop         Stop the local server (Postgres is left running)."
	@echo ""
	@echo "  everything lives at one URL: $(URL)"
	@echo "  build your party of 4 on the Join screen (keys 1-4), or preset it:"
	@echo "    browser: $(URL)/?party=squire,psyker,resonant,squire   (or ?class=psyker for the lead)"
	@echo "    native:  MELD_PARTY=squire,psyker,resonant,squire make play-native"

# Browser client, single URL. Build the wasm bundle to dist/, then boot the
# server with MELD_CLIENT_DIST set so it serves that client at / AND handles the
# realtime WebSocket on the SAME origin — no separate web server, no proxy, no
# second port. Open $(URL) once you see "server healthy". First build compiles
# the wasm bundle (a minute or two); leave it running (Ctrl-C to stop).
play:
	@echo "→ Building the web client (first run compiles wasm — a minute or two)…"
	client/scripts/trunk-build.sh
	@echo "→ Starting Postgres + server…  then OPEN:  $(URL)"
	MELD_CLIENT_DIST="$(DIST)" $(SERVE) bash -c 'echo; echo "▶ OPEN  $$MELD_SERVER  in your browser  (Ctrl-C to stop)"; tail -f /dev/null'

# Native desktop window (serve.sh's default command is `cargo run -p meld-client`).
play-native:
	$(SERVE)

# The self-contained build: a single native binary that boots the whole server
# in-process (in-memory DB + embedded balance — no Postgres, no separate server)
# and embeds every asset, then opens the game window. Nothing to set up; state is
# ephemeral. Built from the client workspace with the `embedded-server` feature.
# `play-solo` runs it straight; `dist` packages the release binary to hand off.
play-solo:
	@echo "→ Building + running the self-contained game (first build is slow — server + Bevy + 84MB of assets)…"
	cd client && cargo run -p meld-client --features embedded-server --release

dist:
	@echo "→ Building the shippable single-file QA binary (release; first build is slow)…"
	cd client && cargo build -p meld-client --features embedded-server --release
	@mkdir -p "$(DIST_OUT)"
	@cp "$(SOLO_BIN)" "$(DIST_OUT)/$(DIST_NAME)"
	@echo ""
	@echo "✔ Self-contained QA binary:  $(DIST_OUT)/$(DIST_NAME)"
	@echo "  size: $$(du -h "$(DIST_OUT)/$(DIST_NAME)" | cut -f1)   (one file — no Postgres, no assets folder, no config)"
	@echo "  Hand it to a tester; they just run it. Party/flags still work via env, e.g.:"
	@echo "    MELD_PARTY=squire,psyker,resonant,squire $(DIST_OUT)/$(DIST_NAME)"

# HD-2D render look-dev scene — a standalone diorama (no Postgres/server) for
# tuning the camera / bloom / tilt-shift DoF / fog live with the keyboard. The
# on-screen readout prints the current values so we can bake in a look. Native
# only (the post stack needs a real GPU).
look:
	cd client && cargo run --bin hd2d

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
