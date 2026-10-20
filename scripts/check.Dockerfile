# Build/test environment with Tauri's Linux libraries, for `make docker-test`
# on machines without the GTK/WebKit development packages.
FROM rust:1.92-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
      libsoup-3.0-dev libjavascriptcoregtk-4.1-dev rsync openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add clippy
