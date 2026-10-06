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

web-install: ## Install web dependencies
	cd $(ROOT)/web && npm install

web-typecheck: web-install ## Typecheck the web client
	cd $(ROOT)/web && npx tsc --noEmit

web-build: web-install wasm-dist ## Build the web client
	cd $(ROOT)/web && npm run build

web-contract: wasm-runtime ## Check the TS facade against the real kernel
	cd $(ROOT)/web && node scripts/kernel-contract-check.mjs

web-test: ## Run the facade tests against the real wasm artifact
	cd $(ROOT)/web && node --experimental-strip-types --test tests/kernel.test.ts

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

.PHONY: spike
spike: ## Re-run the geometry acceptance probe behind ADR-0003
	cd $(SPIKE) && $(CARGO) run --quiet

.PHONY: wasm-check
wasm-check: ## Re-run the ADR-0003 wasm verification
	cd $(SPIKE) && ./wasm_check.sh

.PHONY: check
check: lint test schema schema-check wasm wasm-runtime web-contract web-test web-typecheck ## Everything CI should run