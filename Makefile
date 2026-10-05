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
	cd $(ENGINE) && $(CARGO) build -p mask-kernel --target wasm32-unknown-unknown

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
fixtures: ## Report the verdict of every fixture, with and without nesting
	@for f in $(ENGINE)/fixtures/*.json; do \
		n=$$(basename $$f .json); \
		for mode in "" "--nest"; do \
			out=$$(cd $(ENGINE) && $(CARGO) run -q -p mask-cli --bin maskctl -- \
				--fixture $$f --material $(MATERIAL) --plotter $(PLOTTER) \
				--widen-to $(WIDEN) $$mode --out /tmp/maskctl-reg 2>&1); \
			v=$$(echo "$$out" | grep -oE 'Verdict: [A-Za-z_]+' | head -1); \
			c=$$(echo "$$out" | grep -c 'wrote '); \
			printf "  %-24s %-7s %-30s %s file(s)\n" "$$n" "$${mode:-plain}" "$${v:-no verdict}" "$$c"; \
		done; \
	done

.PHONY: spike
spike: ## Re-run the geometry acceptance probe behind ADR-0003
	cd $(SPIKE) && $(CARGO) run --quiet

.PHONY: wasm-check
wasm-check: ## Re-run the ADR-0003 wasm verification
	cd $(SPIKE) && ./wasm_check.sh

.PHONY: check
check: lint test schema schema-check wasm ## Everything CI should run