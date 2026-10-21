#!/usr/bin/env python3
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent

JSON_FILES = [
    Path("package.json"),
    Path("src-tauri/tauri.conf.json"),
]
CARGO_TOML = Path("src-tauri/Cargo.toml")
PACKAGE_LOCK = Path("package-lock.json")
CARGO_LOCK = Path("Cargo.lock")


def read(p):
    return (ROOT / p).read_text(encoding="utf-8")


def write(p, text):
    (ROOT / p).write_text(text, encoding="utf-8")


def existing_versions():
    versions = {}
    for p in JSON_FILES:
        versions[str(p)] = json.loads(read(p))["version"]
    toml = read(CARGO_TOML)
    m = re.search(r'^version\s*=\s*"([^"]+)"', toml, re.M)
    versions[str(CARGO_TOML)] = m.group(1) if m else None
    lock = json.loads(read(PACKAGE_LOCK))
    versions[str(PACKAGE_LOCK)] = lock.get("version")
    clog = read(CARGO_LOCK)
    m = re.search(r'name = "app"\nversion = "([^"]+)"', clog)
    versions[str(CARGO_LOCK)] = m.group(1) if m else None
    return versions


def main():
    versions = existing_versions()
    print("Existing versions:")
    for f, v in versions.items():
        print(f"  {f}: {v}")
    distinct = {v for v in versions.values() if v}
    if len(distinct) > 1:
        print(f"WARNING: versions differ across files: {sorted(distinct)}")

    new = input("Enter new version (e.g. 0.3.0): ").strip()
    if not re.fullmatch(r"\d+\.\d+\.\d+", new):
        print("Invalid version; expected numeric major.minor.patch.")
        sys.exit(1)
    print(f"New version will be: {new}")
    confirm = input("Proceed? (y/N): ").strip().lower()
    if confirm != "y":
        print("Aborted.")
        sys.exit(0)

    for p in JSON_FILES:
        data = json.loads(read(p))
        data["version"] = new
        write(p, json.dumps(data, indent=2) + "\n")
        print(f"updated {p}")

    toml = read(CARGO_TOML)
    toml = re.sub(r'^version\s*=\s*"[^"]+"', f'version = "{new}"', toml, count=1, flags=re.M)
    write(CARGO_TOML, toml)
    print(f"updated {CARGO_TOML}")

    lock = json.loads(read(PACKAGE_LOCK))
    lock["version"] = new
    if "packages" in lock and "" in lock["packages"]:
        lock["packages"][""]["version"] = new
    write(PACKAGE_LOCK, json.dumps(lock, indent=2) + "\n")
    print(f"updated {PACKAGE_LOCK}")

    clog = read(CARGO_LOCK)
    clog = re.sub(
        r'(name = "app"\nversion = ")[^"]+(")',
        rf"\g<1>{new}\g<2>",
        clog,
        count=1,
    )
    write(CARGO_LOCK, clog)
    print(f"updated {CARGO_LOCK}")

    print("\nRunning npm install --package-lock-only ...")
    subprocess.run(["npm", "install", "--package-lock-only"], cwd=ROOT, check=False)
    print("Running cargo check ...")
    subprocess.run(
        ["cargo", "check", "--manifest-path", "src-tauri/Cargo.toml"],
        cwd=ROOT,
        check=False,
    )
    print("Running release check ...")
    subprocess.run(
        ["node", ".github/scripts/check-release.mjs", f"v{new}"],
        cwd=ROOT,
        check=False,
    )


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nAborted.")
        sys.exit(130)
