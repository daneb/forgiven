.PHONY: help check fmt fmt-fix lint audit deny test build companion install install-tools

# ── Colours ────────────────────────────────────────────────────────────────────
BOLD  := \033[1m
RESET := \033[0m
GREEN := \033[0;32m
CYAN  := \033[0;36m

help: ## Show this help
	@echo ""
	@echo "$(BOLD)forgiven — dev task runner$(RESET)"
	@echo ""
	@echo "$(CYAN)Quality checks$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  $(BOLD)%-18s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "Run $(BOLD)make check$(RESET) to execute all checks in CI order."

# ── Aggregate ──────────────────────────────────────────────────────────────────

check: fmt lint audit deny test ## Run ALL checks (CI order): fmt → lint → audit → deny → test

# ── Individual targets ─────────────────────────────────────────────────────────

fmt: ## Check formatting (fails if any file needs reformatting)
	cargo fmt --all -- --check

fmt-fix: ## Auto-format all source files
	cargo fmt --all

lint: ## Run Clippy; treat every warning as an error
	cargo clippy --all-targets --all-features -- -D warnings

audit: ## Scan dependencies for known CVEs (requires cargo-audit)
	cargo audit

deny: ## Check licences, advisories and banned crates (requires cargo-deny)
	cargo deny check

test: ## Run the full test suite
	cargo test

build: ## Build an optimised release binary
	cargo build --release

companion: ## Build the Tauri companion window (requires Node/npm)
	cd companion && npm install && npm run tauri build

install: build companion ## Build both binaries and install to ~/.local/bin
	@mkdir -p ~/.local/bin
	install -m755 target/release/forgiven                                              ~/.local/bin/forgiven
	install -m755 companion/src-tauri/target/release/forgiven-companion                ~/.local/bin/forgiven-companion
	@echo "$(GREEN)Installed forgiven and forgiven-companion to ~/.local/bin$(RESET)"
	@echo "Ensure ~/.local/bin is on your PATH."

# ── Tool installation ──────────────────────────────────────────────────────────

install-tools: ## Install all required dev/security tools
	cargo install cargo-audit --locked
	cargo install cargo-deny  --locked
	@echo "$(GREEN)Tools installed.$(RESET)"
	@echo "Also ensure the following are on PATH:"
	@echo "  rg (ripgrep)   — brew install ripgrep"
	@echo "  lazygit        — brew install lazygit"
