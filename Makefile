# Common development tasks. Run `make` or `make help` for the list.

SLUG      := $(shell node -p "require('./brand.json').slug" 2>/dev/null || echo app)
CARGO_DIR := src-tauri
CARGO     := cargo --locked
BUNDLE    := $(CARGO_DIR)/target/release/bundle

# Homebrew/Linuxbrew pkg-config cannot see the system GTK/WebKit packages.
ifeq ($(shell uname -s),Linux)
  ifneq ($(wildcard /usr/bin/pkg-config),)
    export PKG_CONFIG := /usr/bin/pkg-config
  endif
endif

.DEFAULT_GOAL := help
.PHONY: help setup deps dev build deb appimage rpm run test check lint icons brand \
        docker-test release-check clean

help: ## Show this help
	@awk 'BEGIN {FS = ":.*## "} /^[a-z][a-z -]*:.*## / {printf "  \033[32m%-18s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

setup: node_modules ## Install npm dependencies

node_modules: package.json package-lock.json
	npm ci
	@touch $@

deps: ## Install system build libraries (Linux; uses sudo)
	@if command -v apt-get >/dev/null; then \
	  sudo apt-get update && sudo apt-get install -y build-essential curl wget file pkg-config \
	    libssl-dev libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
	    libxdo-dev rsync librsvg2-bin; \
	elif command -v dnf >/dev/null; then \
	  sudo dnf install -y webkit2gtk4.1-devel openssl-devel curl wget file libappindicator-gtk3-devel \
	    librsvg2-devel librsvg2-tools libxdo-devel rsync && sudo dnf group install -y c-development; \
	elif command -v pacman >/dev/null; then \
	  sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl appmenu-gtk-module \
	    libappindicator-gtk3 librsvg xdotool rsync; \
	else echo "Unsupported package manager; see docs/development.md"; exit 1; fi

dev: setup ## Run the app with hot reload
	npm run tauri dev

build: setup ## Build release installers for this OS
	npm run tauri build

deb appimage rpm: setup ## Build one Linux bundle (make deb | appimage | rpm)
	npm run tauri build -- --bundles $@
	@ls -1 $(BUNDLE)/$@/

run: ## Run the release binary built by `make build`
	$(CARGO_DIR)/target/release/$(SLUG)

test: setup ## Run Rust tests and the frontend build
	npm run build
	cd $(CARGO_DIR) && $(CARGO) test --lib

check: setup ## Type-check frontend and Rust (no tests)
	npm run check

lint: setup ## Clippy on the Rust code
	npm run build
	cd $(CARGO_DIR) && $(CARGO) clippy --lib

icons: setup ## Regenerate icons and brand assets from branding/
	npm run icons

brand: ## Apply brand.json to Tauri/npm/Cargo manifests
	npm run brand

docker-test: setup ## Run tests in a container (no system libraries needed)
	npm run build
	docker build -q -t $(SLUG)-check -f scripts/check.Dockerfile scripts
	mkdir -p $(HOME)/.cargo/registry $(CARGO_DIR)/target/docker
	docker run --rm -u $$(id -u):$$(id -g) -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/target \
	  -v $(HOME)/.cargo/registry:/cargo/registry -v $(CURDIR)/$(CARGO_DIR)/target/docker:/target \
	  -v $(CURDIR):/w -w /w/$(CARGO_DIR) $(SLUG)-check cargo --locked test --lib

release-check: ## Verify versions agree before tagging (VERSION=x.y.z optional)
	node .github/scripts/check-release.mjs $(if $(VERSION),v$(VERSION))

clean: ## Remove build outputs
	rm -rf dist $(CARGO_DIR)/target
