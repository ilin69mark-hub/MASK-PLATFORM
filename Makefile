# MASK PLATFORM — Mask Engine
#
# The walking skeleton is reached through `make demo`. Everything else exists so
# that a physical proof can happen without an API, an editor or a database.

ROOT := $(CURDIR)
ENGINE := $(ROOT)/engine
SPIKE := $(ROOT)/spike/geometry-spike
DOCS := $(ROOT)/docs
# Absolute, because the recipes cd into the engine directory first.
MATERIAL := $(ENGINE)/profiles/data/matte-vinyl-090.v4.json
# The physical proof protocol needs printable files for the cases the engine is
# right to refuse. See docs/protocols/PHYSICAL_PROOF_PROTOCOL.md §4.
EXPERIMENTAL_MATERIAL := $(ENGINE)/profiles/data/matte-vinyl-090-experiment.v1.json
PLOTTER := $(ENGINE)/profiles/data/knife-25mm.v2.json
FIXTURE ?= $(ENGINE)/fixtures/proof_panel.json
OUT ?= $(ENGINE)/out

CARGO := cargo
WIDEN ?= 0.80

.PHONY: help
help: ## List targets
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

.PHONY: build
build: ## Compile every crate
	cd $(ENGINE) && $(CARGO) build --workspace

.PHONY: test
test: ## Run the whole test suite
	cd $(ENGINE) && $(CARGO) test --workspace

.PHONY: lint
lint: ## Clippy and formatting
	cd $(ENGINE) && $(CARGO) clippy --workspace --all-targets -- -D warnings
	cd $(ENGINE) && $(CARGO) fmt --all --check

.PHONY: fmt
fmt: ## Format
	cd $(ENGINE) && $(CARGO) fmt --all

.PHONY: wasm
wasm: ## Verify the kernel builds for the browser
	cd $(ENGINE) && $(CARGO) build -p mask-wasm --target wasm32-unknown-unknown

.PHONY: web-install
web-install: ## Install web dependencies exactly as locked
	cd $(ROOT)/web && npm ci

.PHONY: web-typecheck
web-typecheck: web-install ## Typecheck the web client
	cd $(ROOT)/web && npx tsc --noEmit

.PHONY: web-build
# In `check` because `web-build` depends on `wasm-dist`, and without it a green
# check cannot produce web/public/pkg/mask_wasm.js — the artifact the studio fetches.
# A check that never builds the thing it protects is a check about the source rather
# than about the product, and the failure it would have caught is a 404 in the browser.
web-build: web-install wasm-dist ## Build the web client
	cd $(ROOT)/web && npm run build

.PHONY: web-contract
web-contract: wasm-runtime ## Check the TS facade against the real kernel
	cd $(ROOT)/web && node scripts/kernel-contract-check.mjs

.PHONY: web-test
web-test: wasm-runtime ## Run the web tests against the real wasm artifact
	cd $(ROOT)/web && node --import ./tests/register-tsx.mjs --experimental-strip-types --test "tests/*.test.ts"

.PHONY: wasm-dist
wasm-dist: ## Build the browser bundle into web/public/pkg
	cd $(ENGINE)/wasm && wasm-pack build --target web --release --out-dir $(ROOT)/web/public/pkg

.PHONY: wasm-runtime
wasm-runtime: ## Run the built kernel in a real JS runtime and compare with Rust
	cd $(ENGINE)/wasm && wasm-pack build --target nodejs --release --out-dir pkg-node
	cd $(ENGINE)/wasm && node scripts/runtime_check.mjs

.PHONY: schema
schema: ## Validate JSON Schemas and their examples
	python3 $(DOCS)/schema/validate_examples.py

.PHONY: schema-check
schema-check: demo schema ## Validate the manifest the engine actually produces
	python3 $(DOCS)/schema/validate_generated.py

.PHONY: demo
demo: ## Generate masks for a fixture (override with FIXTURE=…)
	@mkdir -p $(OUT)
	cd $(ENGINE) && $(CARGO) run -q -p mask-cli --bin maskctl -- \
		--fixture $(FIXTURE) \
		--material $(MATERIAL) \
		--plotter $(PLOTTER) \
		--widen-to $(WIDEN) \
		--nest \
		--out $(OUT)

.PHONY: fixtures
fixtures: ## Report every fixture against the verdict it declares
	cd $(ENGINE) && $(CARGO) build -q -p mask-cli
	@python3 $(ROOT)/tools/fixture_report.py $(ENGINE)

.PHONY: physical
physical: ## Print-ready files for every physical proof case, including the failing ones
	@mkdir -p $(OUT)/physical
	@cd $(ENGINE) && $(CARGO) build -q -p mask-cli
	@for f in t1_at_threshold t2_below_threshold t3_island_bridged \
	          t4_island_unbridged t5_six_layers t6_min_gap_violated t7_min_gap_ok; do \
		cd $(ENGINE) && ./target/debug/maskctl \
			--fixture fixtures/$$f.json \
			--material $(MATERIAL) \
			--plotter $(PLOTTER) \
			--export-anyway \
			--out $(OUT)/physical > /tmp/maskctl-physical-$$f.log 2>&1 || true; \
		v=$$(grep -oE 'Verdict: [A-Za-z_]+' /tmp/maskctl-physical-$$f.log | head -1 | sed 's/Verdict: //'); \
		n=$$(grep -c '\.svg (' /tmp/maskctl-physical-$$f.log || true); \
		o=$$(grep -c 'overridden:' /tmp/maskctl-physical-$$f.log || true); \
		printf "  %-24s %-20s %s SVG" "$$f" "$${v:-?}" "$$n"; \
		if [ "$$o" -gt 0 ]; then printf "  (%s rule(s) overridden — NOT a production run)" "$$o"; fi; \
		printf "\n"; \
	done
	@echo
	@echo "Print at 100%. Do not scale: scaling changes the element width and voids the run."
	@echo "Record what happens. See docs/protocols/PHYSICAL_PROOF_PROTOCOL.md"

.PHONY: perf-frame
perf-frame: ## Cost of one editor drag frame, per release, and the bisection share
	cd $(ENGINE) && $(CARGO) run --release --quiet -p mask-validate --example editor_frame_cost

.PHONY: spike
spike: ## Re-run the geometry acceptance probe behind ADR-0003
	cd $(SPIKE) && $(CARGO) run --quiet

.PHONY: wasm-check
wasm-check: ## Re-run the ADR-0003 wasm verification
	cd $(SPIKE) && ./wasm_check.sh

.PHONY: check
check: lint test schema schema-check wasm wasm-runtime web-contract web-test web-typecheck web-build ## Everything CI should run