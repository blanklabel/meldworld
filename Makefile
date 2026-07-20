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

# owner/repo from the origin remote (handles git@ and https forms), for the URLs
# `make release` prints. Empty if there's no GitHub origin.
GH_SLUG := $(shell git remote get-url origin 2>/dev/null | sed -E 's|^.*github\.com[:/]||; s|\.git$$||')

export MELD_ADDR

.DEFAULT_GOAL := help
.PHONY: help play play-native play-solo dist release look server smoke test stop

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
	@echo "  make release VERSION=v0.1.0"
	@echo "                     Tag latest main + push it → CI builds the win/mac/linux"
	@echo "                     binaries and attaches them to a GitHub Release."
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

# Cut a cross-platform QA release. Tags the LATEST origin/main (not your local
# branch — releases ship canonical main, and this sidesteps tagging an orphaned
# commit) with VERSION, then pushes the tag. The `dist` GitHub Actions workflow
# picks up the `v*` tag, builds the native win/mac/linux binaries, and attaches
# them to a GitHub Release. Builds happen in CI, so this is quick + local-toolchain
# free. Needs only git + a GitHub origin.
#
#   make release VERSION=v0.1.0
release:
	@if [ -z "$(VERSION)" ]; then echo "✗ VERSION is required, e.g.  make release VERSION=v0.1.0"; exit 1; fi
	@case "$(VERSION)" in v*) ;; *) echo "✗ VERSION must start with 'v' to match the dist workflow (got '$(VERSION)')"; exit 1;; esac
	@echo "→ Fetching latest origin/main…"
	@git fetch -q origin main
	@if git rev-parse -q --verify "refs/tags/$(VERSION)" >/dev/null || git ls-remote --tags --exit-code origin "$(VERSION)" >/dev/null 2>&1; then \
		echo "✗ tag $(VERSION) already exists (local or remote) — pick a new version"; exit 1; \
	fi
	@echo "→ Tagging origin/main ($$(git rev-parse --short origin/main)) as $(VERSION) and pushing…"
	@git tag -a "$(VERSION)" origin/main -m "MELDWORLD $(VERSION) — self-contained QA binaries (win/mac/linux)"
	@git push origin "$(VERSION)" || { git tag -d "$(VERSION)" >/dev/null; echo "✗ push failed; removed the local tag. Nothing published."; exit 1; }
	@echo ""
	@echo "✔ Pushed $(VERSION). CI is building the binaries now (a few minutes)."
	@echo "  Actions:  https://github.com/$(GH_SLUG)/actions/workflows/dist.yml"
	@echo "  Release:  https://github.com/$(GH_SLUG)/releases/tag/$(VERSION)   (assets appear when the build finishes)"

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
