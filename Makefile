# `make fmt` and `make check-fmt` call mdtablefix directly. `--git` selects the
# Markdown files Git tracks and `--include-untracked` adds the untracked files
# Git does not ignore, so a new document is formatted before it is staged.
# Both modes need mdtablefix 0.6.0 or later; CI pins the version at its
# install-mdtablefix step.
MDTABLEFIX ?= mdtablefix
MDTABLEFIX_SELECT = --git --include-untracked
MDTABLEFIX_RULES = --wrap --renumber --breaks --ellipsis --fences

.PHONY: help all clean test build release lint fmt check-fmt typecheck \
	markdownlint nixie spelling spelling-helper-test workflow-contracts

APP ?= fingermouse
CARGO ?= cargo
BUILD_JOBS ?=
CLIPPY_FLAGS ?= --all-targets --all-features -- -D warnings
MDLINT ?= markdownlint-cli2
NIXIE ?= nixie
WHITAKER ?= whitaker
UV ?= uv
UV_ENV = UV_CACHE_DIR=.uv-cache UV_TOOL_DIR=.uv-tools

# The CV-005 CodeScene contracts live in shared-actions and run from a full
# commit, so a fix is a pin bump. `.github/cv005.toml` holds this repository's
# only parameters.
CV005_CONTRACTS_REF ?= a38feb9be25755c30eca5bda96bd3786a5b89c6b
CV005_CONTRACTS = $(UV_ENV) $(UV) tool run --python 3.13 \
	--from 'git+https://github.com/leynos/shared-actions@$(CV005_CONTRACTS_REF)\#subdirectory=packages/cv005-contracts' \
	cv005-contracts

RUFF_VERSION ?= 0.15.12
TYPOS_VERSION ?= 1.48.0
TYPOS = $(UV) tool run typos@$(TYPOS_VERSION)
WORKFLOW_CONTRACTS = tests/workflow_contracts

# The development build standard (concordat rule `rust-build-defaults`):
# the parallel rustc frontend and, on Linux, the mold linker. An assigned
# RUSTFLAGS replaces every `rustflags` table in .cargo/config.toml, so each
# recipe that sets it composes these onto any inherited value (CI's
# setup-rust exports one), except coverage, which stays on LLVM and the
# platform linker.
BUILD_HOST_OS := $(shell uname -s)
STANDARD_RUSTFLAGS := -Zthreads=8$(if $(filter Linux,$(BUILD_HOST_OS)), -Clink-arg=-fuse-ld=mold)

build: target/debug/$(APP) ## Build debug binary
release: target/release/$(APP) ## Build release binary

all: release spelling workflow-contracts ## Build the release binary and enforce spelling

clean: ## Remove build artifacts
	$(CARGO) clean

test: ## Run tests with warnings treated as errors
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }-D warnings $(STANDARD_RUSTFLAGS)" $(CARGO) test --all-targets --all-features $(BUILD_JOBS)

target/%/$(APP): ## Build binary in debug or release mode
	$(if $(findstring release,$(@)),RUSTFLAGS="$${RUSTFLAGS-}",RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)") $(CARGO) build $(BUILD_JOBS) $(if $(findstring release,$(@)),--release) --bin $(APP)

lint: ## Run Clippy and the Whitaker Dylint suite with warnings denied
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) clippy --all-targets --all-features -- -D warnings
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }-D warnings $(STANDARD_RUSTFLAGS)" $(WHITAKER) --all -- --all-targets --all-features

typecheck: ## Run cargo check for fast no-link verification
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) check --all-targets $(BUILD_JOBS)

fmt: ## Format Rust and Markdown sources
	$(CARGO) fmt --all
	$(MDTABLEFIX) --in-place $(MDTABLEFIX_SELECT) $(MDTABLEFIX_RULES)
	$(MDLINT) --fix "**/*.md"

check-fmt: ## Verify formatting
	$(CARGO) fmt --all -- --check
	$(MDTABLEFIX) --check $(MDTABLEFIX_SELECT) $(MDTABLEFIX_RULES)

markdownlint: spelling ## Lint Markdown files and enforce repository spelling
	$(MDLINT) '**/*.md'

spelling: spelling-helper-test ## Enforce en-GB-oxendict spelling in Markdown prose
	@$(UV_ENV) $(UV) run scripts/generate_typos_config.py
	@git ls-files -z '*.md' | \
		xargs -0 -r env $(UV_ENV) $(TYPOS) --config typos.toml --force-exclude

spelling-helper-test: ## Validate the shared spelling-policy integration
	@$(UV_ENV) $(UV) tool run ruff@$(RUFF_VERSION) format --isolated \
		--target-version py313 --check scripts/generate_typos_config.py \
		scripts/typos_rollout.py scripts/typos_rollout_cache.py \
		scripts/tests/test_typos_rollout.py
	@$(UV_ENV) $(UV) tool run ruff@$(RUFF_VERSION) check --isolated \
		--target-version py313 scripts/generate_typos_config.py \
		scripts/typos_rollout.py scripts/typos_rollout_cache.py \
		scripts/tests/test_typos_rollout.py
	@PYTHONPATH=scripts $(UV_ENV) $(UV) run --no-project --python 3.13 \
		--with pytest==9.0.2 --with pytest-cov==7.0.0 \
		python -m pytest scripts/tests/test_typos_rollout.py \
		-c /dev/null --rootdir=. -p no:cacheprovider \
		--cov=generate_typos_config --cov=typos_rollout \
		--cov=typos_rollout_cache --cov-fail-under=90

workflow-contracts: ## Check the CV-005 CodeScene workflow contract
	$(CV005_CONTRACTS) check --repository .
	$(UV_ENV) $(UV) tool run ruff@$(RUFF_VERSION) format --isolated \
		--target-version py313 --check $(WORKFLOW_CONTRACTS)
	$(UV_ENV) $(UV) tool run ruff@$(RUFF_VERSION) check --isolated \
		--target-version py313 $(WORKFLOW_CONTRACTS)
	PYTHONDONTWRITEBYTECODE=1 $(UV_ENV) $(UV) run --no-project --python 3.13 \
		--with pytest==9.0.2 --with pyyaml==6.0.3 \
		python -m pytest $(WORKFLOW_CONTRACTS) \
		-c /dev/null --rootdir=. -p no:cacheprovider

nixie: ## Validate Mermaid diagrams
	$(NIXIE) --no-sandbox

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?##' $(MAKEFILE_LIST) | \
	awk 'BEGIN {FS=":"; printf "Available targets:\n"} {printf "  %-20s %s\n", $$1, $$2}'
