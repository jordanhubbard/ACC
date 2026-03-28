.PHONY: build dev release clean check install-tools

# Paths
DASHBOARD_UI  := dashboard-ui
DASHBOARD_SRV := dashboard-server
DIST          := dist

# ── Tool availability checks ──────────────────────────────────────────────
TRUNK   := $(shell command -v trunk 2>/dev/null)
WASM_TG := $(shell rustup target list --installed 2>/dev/null | grep wasm32-unknown-unknown)

check:
	@echo "=== Build tool check ==="
	@command -v cargo >/dev/null 2>&1 && echo "  ✓ cargo" || echo "  ✗ cargo (install Rust)"
	@command -v trunk >/dev/null 2>&1  && echo "  ✓ trunk" || echo "  ✗ trunk (run: cargo install trunk)"
	@rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown \
		&& echo "  ✓ wasm32 target" \
		|| echo "  ✗ wasm32 target (run: rustup target add wasm32-unknown-unknown)"
	@command -v wasm-opt >/dev/null 2>&1 && echo "  ✓ wasm-opt" || echo "  ~ wasm-opt (optional, for smaller WASM)"

install-tools:
	rustup target add wasm32-unknown-unknown
	cargo install trunk

# ── Build ─────────────────────────────────────────────────────────────────
build: build-ui build-server

build-ui:
	@echo "=== Building WASM frontend ==="
	cd $(DASHBOARD_UI) && trunk build

build-server:
	@echo "=== Building Rust server ==="
	cargo build -p dashboard-server

# ── Release ───────────────────────────────────────────────────────────────
release: release-ui release-server

release-ui:
	@echo "=== Building WASM frontend (release) ==="
	cd $(DASHBOARD_UI) && trunk build --release

release-server:
	@echo "=== Building server (release) ==="
	cargo build -p dashboard-server --release

# ── Dev (hot-reload UI + run server) ─────────────────────────────────────
dev:
	@echo "Starting dev: trunk watch + server"
	@echo "Frontend: http://localhost:8790"
	@echo "  trunk serves dev proxy; server proxies RCC API"
	@trap 'kill %1 %2 2>/dev/null' EXIT; \
		cd $(DASHBOARD_UI) && trunk watch &  \
		RCC_DASHBOARD_PORT=8790 cargo run -p dashboard-server &  \
		wait

# ── Clean ─────────────────────────────────────────────────────────────────
clean:
	cargo clean
	rm -rf $(DIST)

# ── Run (after build) ─────────────────────────────────────────────────────
run: build-server
	DASHBOARD_DIST=$(DIST) \
	RCC_DASHBOARD_PORT=$${RCC_DASHBOARD_PORT:-8790} \
	RCC_URL=$${RCC_URL:-http://localhost:8789} \
	RCC_AGENT_TOKEN=$${RCC_AGENT_TOKEN:-} \
	./target/debug/dashboard-server

run-release: release-server
	DASHBOARD_DIST=$(DIST) \
	RCC_DASHBOARD_PORT=$${RCC_DASHBOARD_PORT:-8790} \
	RCC_URL=$${RCC_URL:-http://localhost:8789} \
	RCC_AGENT_TOKEN=$${RCC_AGENT_TOKEN:-} \
	./target/release/dashboard-server
