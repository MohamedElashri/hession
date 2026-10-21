# Common development tasks. Run `make` or `make help` for the list.

SLUG      := $(shell node -p "require('./brand.json').slug" 2>/dev/null || echo app)
APP_NAME  := $(shell node -p "require('./brand.json').name" 2>/dev/null || echo App)
CARGO_DIR := src-tauri
CARGO     := cargo --locked
TARGET    := target
BUNDLE    := $(TARGET)/release/bundle

# Homebrew/Linuxbrew pkg-config cannot see the system GTK/WebKit packages, and
# tools such as linuxdeploy's GTK plugin call pkgconf/pkg-config from PATH
# rather than $PKG_CONFIG. On Linux, put the system ones first via a shim.
ifeq ($(shell uname -s),Linux)
  ifneq ($(wildcard /usr/bin/pkg-config),)
    PKG_SHIM := $(CURDIR)/$(TARGET)/pkg-config-shim
    SYSTEM_PKGCONF := $(or $(wildcard /usr/bin/pkgconf),/usr/bin/pkg-config)
    $(shell mkdir -p $(PKG_SHIM) && ln -sf /usr/bin/pkg-config $(PKG_SHIM)/pkg-config && ln -sf $(SYSTEM_PKGCONF) $(PKG_SHIM)/pkgconf)
    export PKG_CONFIG := /usr/bin/pkg-config
    export PATH := $(PKG_SHIM):$(PATH)
  endif
endif

.DEFAULT_GOAL := help
.PHONY: help setup deps dev preview preview-fixtures build deb appimage rpm run test check lint lint-js unused fmt fmt-check icons brand \
        docker-test release-check clean desktop-entry desktop-entry-remove

help: ## Show this help
	@awk 'BEGIN {FS = ":.*## "} /^[a-z][a-z -]*:.*## / {printf "  \033[32m%-22s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

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

preview: setup ## UI in a browser with sample data, no Rust build (http://127.0.0.1:1420/?mock)
	@echo "Open http://127.0.0.1:1420/?mock"
	npm run dev

preview-fixtures: ## Regenerate the preview's sample data from the real backend
	cd $(CARGO_DIR) && $(CARGO) test --lib dump_preview_fixtures -- --ignored

build: setup ## Build release installers for this OS (into target/release/bundle/)
	npm run tauri build
	@echo "Installers:"; find $(BUNDLE) -maxdepth 2 -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' -o -name '*.dmg' \) | sed 's/^/  /'

deb appimage rpm: setup ## Build one Linux bundle (make deb | appimage | rpm)
	npm run tauri build -- --bundles $@
	@ls -1 $(BUNDLE)/$@/

run: ## Run the release binary built by `make build`
	$(TARGET)/release/$(SLUG)

test: setup ## Run Rust tests and the frontend build
	npm run build
	cd $(CARGO_DIR) && $(CARGO) test --lib

check: setup ## Build the frontend and cargo check the backend (no tests)
	npm run check

lint: lint-js ## ESLint + Clippy; any warning fails
	npm run build
	cd $(CARGO_DIR) && $(CARGO) clippy --all-targets -- -D warnings

lint-js: setup ## ESLint on the frontend and scripts
	npm run lint

unused: setup ## Report unused files, exports, and npm dependencies (knip)
	npm run unused

fmt: ## Format the Rust code
	cd $(CARGO_DIR) && cargo fmt

fmt-check: ## Fail if the Rust code is not formatted
	cd $(CARGO_DIR) && cargo fmt --check

icons: setup ## Regenerate icons and brand assets from branding/
	npm run icons

brand: ## Apply brand.json to Tauri/npm/Cargo manifests
	npm run brand

docker-test: setup ## Run tests in a container (no system libraries needed)
	npm run build
	docker build -q -t $(SLUG)-check -f scripts/check.Dockerfile scripts
	mkdir -p $(HOME)/.cargo/registry $(TARGET)/docker
	docker run --rm -u $$(id -u):$$(id -g) -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/target \
	  -v $(HOME)/.cargo/registry:/cargo/registry -v $(CURDIR)/$(TARGET)/docker:/target \
	  -v $(CURDIR):/w -w /w/$(CARGO_DIR) $(SLUG)-check cargo --locked test --lib

# GNOME/KDE pick the dock icon from the .desktop entry whose StartupWMClass (or
# file name) matches the window's app_id / WM_CLASS, which GTK sets to the binary
# name ($(SLUG)). `make dev` installs no entry, so add a per-user one.
XDG_DATA := $(or $(XDG_DATA_HOME),$(HOME)/.local/share)
ICON_SIZES := 32x32:32x32 64x64:64x64 128x128:128x128 256x256:128x128@2x 512x512:icon

desktop-entry: ## Install a per-user .desktop entry + icons (dock icon for `make dev`, Linux)
	@for s in $(ICON_SIZES); do \
	  install -Dm644 $(CARGO_DIR)/icons/$${s#*:}.png $(XDG_DATA)/icons/hicolor/$${s%%:*}/apps/$(SLUG).png; done
	@mkdir -p $(XDG_DATA)/applications
	@printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=$(APP_NAME)' \
	  'Exec=$(CURDIR)/$(TARGET)/release/$(SLUG)' 'Icon=$(SLUG)' \
	  'StartupWMClass=$(SLUG)' 'Categories=Office;' 'Terminal=false' \
	  > $(XDG_DATA)/applications/$(SLUG).desktop
	-@gtk-update-icon-cache -qtf $(XDG_DATA)/icons/hicolor 2>/dev/null
	-@update-desktop-database -q $(XDG_DATA)/applications 2>/dev/null
	@echo "Installed $(XDG_DATA)/applications/$(SLUG).desktop"

desktop-entry-remove: ## Remove the per-user .desktop entry and icons
	rm -f $(XDG_DATA)/applications/$(SLUG).desktop
	@for s in $(ICON_SIZES); do rm -f $(XDG_DATA)/icons/hicolor/$${s%%:*}/apps/$(SLUG).png; done
	-@gtk-update-icon-cache -qtf $(XDG_DATA)/icons/hicolor 2>/dev/null
	-@update-desktop-database -q $(XDG_DATA)/applications 2>/dev/null

release-check: ## Verify versions agree before tagging (VERSION=x.y.z optional)
	node .github/scripts/check-release.mjs $(if $(VERSION),v$(VERSION))

clean: ## Remove build outputs
	rm -rf dist $(TARGET)
