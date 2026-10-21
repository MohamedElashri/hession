#!/bin/sh
# Install or update Hession on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/MohamedElashri/hession/main/install.sh | sh
#
# Installs the latest release, or updates an existing installation to it.
# Every download is verified against the release's SHA256SUMS.
#
# Options (with curl, pass them after `sh -s --`):
#   --version vX.Y.Z   install a specific release instead of the latest
#   --appimage         Linux: install the AppImage for this user only (no sudo)
#   --force            reinstall even if already up to date
#   --uninstall        remove the app (your session library is kept)
#   --help             show this help
#
# Environment:
#   GITHUB_TOKEN       GitHub token, for private repositories or API rate limits
#   NO_COLOR           disable colored output
#
# The app's data lives in ~/.hession and is never touched by this script.

set -eu

# --- generated from brand.json by scripts/brand.mjs; do not edit ---
APP_NAME="Hession"
APP_SLUG="hession"
REPO="MohamedElashri/hession"
# --- end generated ---

ASSET_PREFIX=$(printf '%s' "$APP_NAME" | tr -d ' ')
GITHUB_API="https://api.github.com/repos/$REPO"

# ---------------------------------------------------------------- output

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != "dumb" ]; then
  BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m'); RESET=$(printf '\033[0m')
  RED=$(printf '\033[31m'); GREEN=$(printf '\033[32m'); YELLOW=$(printf '\033[33m'); BLUE=$(printf '\033[34m')
else
  BOLD=''; DIM=''; RESET=''; RED=''; GREEN=''; YELLOW=''; BLUE=''
fi
case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
  *UTF-8* | *utf8* | *UTF8* | *utf-8*) OK_MARK='✓'; ERR_MARK='✗'; ARROW='→' ;;
  *) OK_MARK='OK'; ERR_MARK='x'; ARROW='->' ;;
esac

step() { printf '%s%s==>%s %s%s%s\n' "$BOLD" "$BLUE" "$RESET" "$BOLD" "$*" "$RESET"; }
info() { printf '    %s\n' "$*"; }
ok() { printf '    %s%s%s %s\n' "$GREEN" "$OK_MARK" "$RESET" "$*"; }
warn() { printf '%s%swarning:%s %s\n' "$BOLD" "$YELLOW" "$RESET" "$*" >&2; }
die() {
  printf '%s%s%s error:%s %s\n' "$BOLD" "$RED" "$ERR_MARK" "$RESET" "$*" >&2
  exit 1
}

usage() {
  cat <<EOF
Install or update $APP_NAME on macOS or Linux.

Usage: install.sh [options]
       curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- [options]

Options:
  --version vX.Y.Z   install a specific release instead of the latest
  --appimage         Linux: install the AppImage for this user only (no sudo)
  --force            reinstall even if already up to date
  --uninstall        remove the app (your session library is kept)
  -h, --help         show this help

Environment:
  GITHUB_TOKEN       GitHub token, for private repositories or API rate limits
  NO_COLOR           disable colored output
EOF
}

# ---------------------------------------------------------------- arguments

VERSION=""
METHOD=""
FORCE=0
UNINSTALL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value, e.g. --version v0.1.0"; VERSION=$2; shift ;;
    --version=*) VERSION=${1#*=} ;;
    --appimage) METHOD=appimage ;;
    --force) FORCE=1 ;;
    --uninstall) UNINSTALL=1 ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
  shift
done
case "$VERSION" in "" | v*) ;; *) VERSION="v$VERSION" ;; esac

# ---------------------------------------------------------------- helpers

need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required but was not found"; }
have() { command -v "$1" >/dev/null 2>&1; }

need curl
need uname
need mktemp

TMP=$(mktemp -d 2>/dev/null || mktemp -d -t "$APP_SLUG")
MOUNT=""
cleanup() {
  if [ -n "$MOUNT" ]; then hdiutil detach "$MOUNT" -quiet >/dev/null 2>&1 || true; fi
  rm -rf "${TMP:?}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# HTTPS only, TLS 1.2+, fail on HTTP errors, retry transient failures.
fetch() {
  url=$1; out=$2; shift 2
  set -- --proto '=https' --tlsv1.2 --fail --location --silent --show-error --retry 3 --connect-timeout 15 "$@"
  if [ -n "${GITHUB_TOKEN:-}" ]; then set -- "$@" -H "Authorization: Bearer $GITHUB_TOKEN"; fi
  curl "$@" -o "$out" "$url"
}

sha256() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then shasum -a 256 "$1" | cut -d' ' -f1
  else die "neither sha256sum nor shasum is available to verify downloads"; fi
}

run_as_root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"
  elif have sudo; then sudo "$@"
  else die "administrator rights are needed; install sudo or rerun with --appimage"; fi
}

# ---------------------------------------------------------------- platform

case "$(uname -s)" in
  Darwin) OS=macos ;;
  Linux) OS=linux ;;
  *) die "unsupported system '$(uname -s)'; $APP_NAME runs on macOS and Linux" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x64 ;;
  arm64 | aarch64) ARCH=arm64 ;;
  *) die "unsupported architecture '$(uname -m)'" ;;
esac

if [ "$OS" = macos ]; then
  METHOD=dmg
elif [ -z "$METHOD" ]; then
  if have dpkg && have apt-get; then METHOD=deb
  elif have rpm && { have dnf || have zypper || have yum; }; then METHOD=rpm
  else METHOD=appimage; fi
fi

# Release asset suffix for this system (see .github/scripts/check-release.mjs).
case "$METHOD-$ARCH" in
  dmg-*) SUFFIX=darwin-universal-dmg.dmg ;;
  deb-x64) SUFFIX=linux-amd64-deb.deb ;;
  deb-arm64) SUFFIX=linux-arm64-deb.deb ;;
  rpm-x64) SUFFIX=linux-x86_64-rpm.rpm ;;
  rpm-arm64) SUFFIX=linux-aarch64-rpm.rpm ;;
  appimage-x64) SUFFIX=linux-amd64-appimage.AppImage ;;
  appimage-arm64) SUFFIX=linux-aarch64-appimage.AppImage ;;
esac

DATA_HOME=${XDG_DATA_HOME:-$HOME/.local/share}
APPIMAGE_DIR="$DATA_HOME/$APP_SLUG"
APPIMAGE_PATH="$APPIMAGE_DIR/$ASSET_PREFIX.AppImage"
BIN_DIR="$HOME/.local/bin"
DESKTOP_FILE="$DATA_HOME/applications/$APP_SLUG.desktop"

mac_app_path() {
  for dir in /Applications "$HOME/Applications"; do
    if [ -d "$dir/$APP_NAME.app" ]; then printf '%s\n' "$dir/$APP_NAME.app"; return; fi
  done
}

# Installed version (without a leading v), or nothing.
installed_version() {
  case "$METHOD" in
    dmg)
      app=$(mac_app_path)
      if [ -n "$app" ]; then /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist" 2>/dev/null || true; fi ;;
    # Only fully installed packages count (not "removed, config remains").
    deb) dpkg-query -W -f='${db:Status-Status} ${Version}\n' "$APP_SLUG" 2>/dev/null | sed -n 's/^installed //p' || true ;;
    rpm) rpm -q --qf '%{VERSION}' "$APP_SLUG" 2>/dev/null | grep -v 'not installed' || true ;;
    appimage) if [ -f "$APPIMAGE_DIR/VERSION" ]; then cat "$APPIMAGE_DIR/VERSION"; fi ;;
  esac
}

# ---------------------------------------------------------------- uninstall

if [ "$UNINSTALL" -eq 1 ]; then
  step "Uninstalling $APP_NAME"
  case "$METHOD" in
    dmg)
      app=$(mac_app_path)
      [ -n "$app" ] || die "$APP_NAME is not installed in /Applications or ~/Applications"
      rm -rf "${app:?}" 2>/dev/null || run_as_root rm -rf "${app:?}"
      ok "Removed $app" ;;
    deb) [ -n "$(installed_version)" ] || die "the $APP_SLUG package is not installed"
      run_as_root apt-get remove -y "$APP_SLUG"; ok "Removed the $APP_SLUG package" ;;
    rpm) [ -n "$(installed_version)" ] || die "the $APP_SLUG package is not installed"
      if have dnf; then run_as_root dnf remove -y "$APP_SLUG"; elif have zypper; then run_as_root zypper --non-interactive remove "$APP_SLUG"; else run_as_root yum remove -y "$APP_SLUG"; fi
      ok "Removed the $APP_SLUG package" ;;
    appimage)
      [ -e "$APPIMAGE_PATH" ] || die "$APP_NAME is not installed as an AppImage for this user"
      rm -rf "${APPIMAGE_DIR:?}" "${BIN_DIR:?}/${APP_SLUG:?}" "${DESKTOP_FILE:?}"
      for size in 32x32 64x64 128x128 512x512; do rm -f "$DATA_HOME/icons/hicolor/$size/apps/$APP_SLUG.png"; done
      ok "Removed the AppImage, launcher, and icons" ;;
  esac
  info "${DIM}Your session library in ~/.$APP_SLUG was kept; delete it yourself if you no longer need it.${RESET}"
  exit 0
fi

# ---------------------------------------------------------------- release

printf '\n%s%s%s installer\n\n' "$BOLD" "$APP_NAME" "$RESET"
step "Finding the ${VERSION:-latest} release"
if [ -n "$VERSION" ]; then RELEASE_URL="$GITHUB_API/releases/tags/$VERSION"; else RELEASE_URL="$GITHUB_API/releases/latest"; fi
fetch "$RELEASE_URL" "$TMP/release.json" -H 'Accept: application/vnd.github+json' \
  || die "could not read ${VERSION:-the latest} release of $REPO (set GITHUB_TOKEN if the repository is private)"

# One JSON member per line, whether GitHub sent pretty or compact JSON.
# shellcheck disable=SC2020 # three characters, each mapped to a newline
tr ',{}' '\n\n\n' < "$TMP/release.json" > "$TMP/release.lines"
TAG=$(sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' "$TMP/release.lines" | head -n 1)
[ -n "$TAG" ] || die "the release information from GitHub could not be read"
LATEST=${TAG#v}

# One "name<TAB>api-url<TAB>download-url" line per asset.
awk '
  /^ *"url": *"https:\/\/api\.github\.com\/repos\/.*\/releases\/assets\/[0-9]+"/ { split($0, a, "\""); api = a[4]; name = "" }
  /^ *"name": *"/ && api != "" && name == "" { split($0, a, "\""); name = a[4] }
  /^ *"browser_download_url": *"/ && api != "" { split($0, a, "\""); printf "%s\t%s\t%s\n", name, api, a[4]; api = "" }
' "$TMP/release.lines" > "$TMP/assets.tsv"

asset_field() { awk -F '\t' -v n="$1" -v f="$2" '$1 == n { print $f; exit }' "$TMP/assets.tsv"; }
download_asset() {
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    fetch "$(asset_field "$1" 2)" "$2" -H 'Accept: application/octet-stream'
  else
    fetch "$(asset_field "$1" 3)" "$2"
  fi
}

ASSET="$ASSET_PREFIX-$TAG-$SUFFIX"
[ -n "$(asset_field "$ASSET" 2)" ] || die "release $TAG has no installer for this system ($ASSET)"
ok "$APP_NAME $TAG ${DIM}($OS $ARCH, $METHOD)${RESET}"

CURRENT=$(installed_version)
if [ -n "$CURRENT" ]; then
  if [ "$CURRENT" = "$LATEST" ] && [ "$FORCE" -eq 0 ]; then
    ok "Already up to date (version $CURRENT). Use --force to reinstall."
    exit 0
  fi
  if [ "$CURRENT" = "$LATEST" ]; then info "Reinstalling ${BOLD}$LATEST${RESET}"
  else info "Updating from ${BOLD}$CURRENT${RESET} $ARROW ${BOLD}$LATEST${RESET}"; fi
fi

# ---------------------------------------------------------------- download

step "Downloading $ASSET"
download_asset "$ASSET" "$TMP/$ASSET" || die "the download failed"
ok "Downloaded ${DIM}($(du -h "$TMP/$ASSET" | cut -f1 | tr -d ' '))${RESET}"

step "Verifying the checksum"
[ -n "$(asset_field SHA256SUMS 2)" ] || die "release $TAG has no SHA256SUMS file, so the download cannot be verified"
download_asset SHA256SUMS "$TMP/SHA256SUMS" || die "could not download SHA256SUMS"
EXPECTED=$(awk -v n="$ASSET" '$2 == n || $2 == "*" n { print $1; exit }' "$TMP/SHA256SUMS")
[ -n "$EXPECTED" ] || die "SHA256SUMS has no entry for $ASSET"
ACTUAL=$(sha256 "$TMP/$ASSET")
[ "$ACTUAL" = "$EXPECTED" ] || die "checksum mismatch for $ASSET (expected $EXPECTED, got $ACTUAL); nothing was installed"
ok "SHA-256 matches"

# ---------------------------------------------------------------- install

step "Installing"
case "$METHOD" in
  dmg)
    need hdiutil
    MOUNT="$TMP/volume"
    mkdir -p "$MOUNT"
    hdiutil attach "$TMP/$ASSET" -nobrowse -readonly -noautoopen -mountpoint "$MOUNT" -quiet || die "could not open the disk image"
    [ -d "$MOUNT/$APP_NAME.app" ] || die "the disk image does not contain $APP_NAME.app"
    DEST=$(mac_app_path)
    if [ -z "$DEST" ]; then
      if [ -w /Applications ]; then DEST="/Applications/$APP_NAME.app"; else mkdir -p "$HOME/Applications"; DEST="$HOME/Applications/$APP_NAME.app"; fi
    fi
    if pgrep -xq "$APP_NAME" 2>/dev/null; then warn "$APP_NAME is running; restart it to use the new version"; fi
    # Copy next to the destination first, then swap, so a failure never leaves a half-installed app.
    ditto "$MOUNT/$APP_NAME.app" "$DEST.new" || die "could not copy the app to $(dirname "$DEST")"
    rm -rf "${DEST:?}" && mv "$DEST.new" "$DEST"
    xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true
    ok "Installed $DEST"
    LAUNCH="open -a '$APP_NAME'" ;;
  deb)
    info "Installing the package needs administrator rights; you may be asked for your password."
    run_as_root apt-get install -y --allow-downgrades "$TMP/$ASSET" >/dev/null || die "apt-get could not install the package"
    ok "Installed the $APP_SLUG package"
    LAUNCH="$APP_SLUG" ;;
  rpm)
    info "Installing the package needs administrator rights; you may be asked for your password."
    if have dnf; then run_as_root dnf install -y "$TMP/$ASSET" >/dev/null
    elif have zypper; then run_as_root zypper --non-interactive install --allow-unsigned-rpm "$TMP/$ASSET" >/dev/null
    else run_as_root yum install -y "$TMP/$ASSET" >/dev/null; fi || die "the package manager could not install the package"
    ok "Installed the $APP_SLUG package"
    LAUNCH="$APP_SLUG" ;;
  appimage)
    mkdir -p "$APPIMAGE_DIR" "$BIN_DIR" "$DATA_HOME/applications"
    chmod +x "$TMP/$ASSET"
    mv -f "$TMP/$ASSET" "$APPIMAGE_PATH.new" && mv -f "$APPIMAGE_PATH.new" "$APPIMAGE_PATH"
    printf '%s\n' "$LATEST" > "$APPIMAGE_DIR/VERSION"
    # Icons for the launcher and dock, from inside the AppImage.
    (cd "$TMP" && "$APPIMAGE_PATH" --appimage-extract 'usr/share/icons/hicolor/*' >/dev/null 2>&1) || true
    for size in 32x32 64x64 128x128 512x512; do
      icon="$TMP/squashfs-root/usr/share/icons/hicolor/$size/apps/$APP_SLUG.png"
      if [ -f "$icon" ]; then mkdir -p "$DATA_HOME/icons/hicolor/$size/apps" && cp "$icon" "$DATA_HOME/icons/hicolor/$size/apps/"; fi
    done
    # AppImages need FUSE 2; without it, run them by extracting to a temp dir.
    RUN_ENV=""
    if [ "$OS" = linux ] && ! { ldconfig -p 2>/dev/null | grep -q 'libfuse\.so\.2'; }; then
      RUN_ENV="APPIMAGE_EXTRACT_AND_RUN=1 "
      warn "FUSE 2 (libfuse2) was not found; $APP_NAME will start a little slower. Install libfuse2 to fix this."
    fi
    printf '#!/bin/sh\nexec env %s"%s" "$@"\n' "$RUN_ENV" "$APPIMAGE_PATH" > "$BIN_DIR/$APP_SLUG"
    chmod +x "$BIN_DIR/$APP_SLUG"
    cat > "$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=$APP_NAME
Comment=Your coding-agent sessions, local and remote
Exec=$BIN_DIR/$APP_SLUG
Icon=$APP_SLUG
StartupWMClass=$APP_SLUG
Categories=Development;
Terminal=false
EOF
    if have update-desktop-database; then update-desktop-database -q "$DATA_HOME/applications" 2>/dev/null || true; fi
    if have gtk-update-icon-cache; then gtk-update-icon-cache -qtf "$DATA_HOME/icons/hicolor" 2>/dev/null || true; fi
    ok "Installed $APPIMAGE_PATH"
    ok "Added the '$APP_SLUG' command and an app-menu entry"
    LAUNCH="$APP_SLUG"
    case ":$PATH:" in *":$BIN_DIR:"*) ;; *) warn "$BIN_DIR is not on your PATH; add it to start $APP_NAME from a terminal" ;; esac ;;
esac

printf '\n%s%s%s %s %s is ready.%s\n' "$BOLD" "$GREEN" "$OK_MARK" "$APP_NAME" "$LATEST" "$RESET"
info "Start it from your applications menu, or run: ${BOLD}$LAUNCH${RESET}"
info "${DIM}Run this installer again at any time to update.${RESET}"
printf '\n'
