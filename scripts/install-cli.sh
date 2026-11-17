#!/bin/sh
# Opt-in CLI link for an existing app-bundle executable. Never replaces a command.
set -eu
usage() {
  printf '%s\n' 'Usage: install-cli.sh ABSOLUTE_BUNDLE_EXECUTABLE [ABSOLUTE_DESTINATION]' \
    'Creates a non-overwriting symbolic link. Default destination: ~/.local/bin/<executable-name>.'
}
case "${1:-}" in -h|--help) usage; exit 0 ;; esac
[ "$#" -ge 1 ] && [ "$#" -le 2 ] || { usage >&2; exit 2; }
cli_source=$1
case "$cli_source" in /*) ;; *) printf '%s\n' 'Use an absolute executable path.' >&2; exit 2 ;; esac
[ -f "$cli_source" ] && [ -x "$cli_source" ] || { printf '%s\n' 'Bundle executable is missing or not executable.' >&2; exit 2; }
if [ "$#" -eq 2 ]; then
  cli_destination=$2
else
  [ -n "${HOME:-}" ] || { printf '%s\n' 'Pass an explicit destination when HOME is unavailable.' >&2; exit 2; }
  cli_destination="$HOME/.local/bin/$(basename "$cli_source")"
fi
case "$cli_destination" in /*) ;; *) printf '%s\n' 'Use an absolute destination path.' >&2; exit 2 ;; esac
if [ -L "$cli_destination" ] && [ "$(readlink "$cli_destination")" = "$cli_source" ]; then
  printf '%s\n' 'The CLI link is already installed.'
  exit 0
fi
if [ -e "$cli_destination" ] || [ -L "$cli_destination" ]; then
  printf '%s\n' 'Refusing to replace an existing command or link. Choose another destination.' >&2
  exit 1
fi
mkdir -p "$(dirname "$cli_destination")"
# No -f: a file or link created concurrently is preserved as well.
ln -s "$cli_source" "$cli_destination"
printf '%s\n' 'CLI link installed. Ensure its directory is on PATH.'
