#!/usr/bin/env python3
"""Keep desktop, relay, container, and lockfile release versions together."""
import argparse
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent
JSON_FILES = [Path("package.json"), Path("src-tauri/tauri.conf.json")]
CARGO_PACKAGES = [
    (Path("src-tauri/Cargo.toml"), "app"),
    (Path("crates/hession-sync/Cargo.toml"), "hession-sync"),
    (Path("crates/hession-relay/Cargo.toml"), "hession-relay"),
]
PACKAGE_LOCK = Path("package-lock.json")
CARGO_LOCK = Path("Cargo.lock")
COMPOSE = Path("deploy/relay/compose.yaml")


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def existing_versions():
    versions = {str(path): json.loads(read(path))["version"] for path in JSON_FILES}
    lock = json.loads(read(PACKAGE_LOCK))
    versions[str(PACKAGE_LOCK)] = lock.get("version")
    versions["package-lock.json root package"] = lock.get("packages", {}).get("", {}).get("version")
    for path, name in CARGO_PACKAGES:
        versions[str(path)] = re.search(r'^version\s*=\s*"([^"]+)"', read(path), re.M).group(1)
        versions[f"Cargo.lock ({name})"] = re.search(
            rf'name = "{re.escape(name)}"\nversion = "([^"]+)"', read(CARGO_LOCK)
        ).group(1)
    versions[str(COMPOSE)] = re.search(r'^\s+image: hession-relay:([^\s]+)', read(COMPOSE), re.M).group(1)
    return versions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", nargs="?", help="numeric major.minor.patch")
    parser.add_argument("--yes", action="store_true", help="apply without an interactive prompt")
    parser.add_argument("--skip-checks", action="store_true", help="run release/build checks separately")
    args = parser.parse_args()
    print("Existing versions:")
    for name, value in existing_versions().items():
        print(f"  {name}: {value}")
    version = args.version or input("Enter new version (e.g. 0.3.0): ").strip()
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        parser.error("expected numeric major.minor.patch")
    if not args.yes and input(f"Set all release versions to {version}? (y/N): ").strip().lower() != "y":
        print("Aborted.")
        return
    # Prepare every edit before writing, so a missing package entry cannot leave
    # only part of a release bumped. Dependency versions are left unchanged.
    edits = {}
    for path in JSON_FILES:
        data = json.loads(read(path))
        data["version"] = version
        edits[path] = json.dumps(data, indent=2) + "\n"
    lock = json.loads(read(PACKAGE_LOCK))
    lock["version"] = version
    lock["packages"][""]["version"] = version
    edits[PACKAGE_LOCK] = json.dumps(lock, indent=2) + "\n"
    cargo_lock = read(CARGO_LOCK)
    for path, name in CARGO_PACKAGES:
        edits[path], count = re.subn(r'^version\s*=\s*"[^"]+"', f'version = "{version}"', read(path), count=1, flags=re.M)
        if count != 1:
            raise ValueError(f"Missing package version: {path}")
        cargo_lock, count = re.subn(
            rf'(name = "{re.escape(name)}"\nversion = ")[^"]+(")',
            rf"\g<1>{version}\g<2>", cargo_lock,
        )
        if count != 1:
            raise ValueError(f"Expected one lockfile entry for {name}, found {count}")
    edits[CARGO_LOCK] = cargo_lock
    edits[COMPOSE], count = re.subn(r'^(\s+image: hession-relay:)[^\s]+', rf"\g<1>{version}", read(COMPOSE), count=1, flags=re.M)
    if count != 1:
        raise ValueError("Missing relay Compose image tag")
    for path, text in edits.items():
        (ROOT / path).write_text(text, encoding="utf-8")
        print(f"updated {path}")
    if not args.skip_checks:
        subprocess.run(["node", ".github/scripts/check-release.mjs", f"v{version}"], cwd=ROOT, check=True)
        subprocess.run(["cargo", "check", "--locked", "--workspace"], cwd=ROOT, check=True)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        raise SystemExit(130)
