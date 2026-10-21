// Applies brand.json to the static manifests that cannot read it at build time
// (Tauri config, npm package name, Cargo metadata, install.sh). Runtime code reads
// brand.json directly: Vite imports it, and src-tauri/build.rs exports it to Rust.
// Idempotent; runs before every `npm run tauri …`.
import { readFileSync, writeFileSync } from 'node:fs';

const root = new URL('../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const write = (path, text) => {
  if (read(path) !== text) {
    writeFileSync(new URL(path, root), text);
    console.log(`brand: updated ${path}`);
  }
};
const brand = JSON.parse(read('brand.json'));

const tauriPath = 'src-tauri/tauri.conf.json';
const tauri = JSON.parse(read(tauriPath));
tauri.productName = brand.name;
tauri.mainBinaryName = brand.slug;
tauri.identifier = brand.identifier;
for (const window of tauri.app?.windows ?? []) window.title = brand.name;
tauri.bundle.shortDescription = brand.tagline;
tauri.bundle.longDescription = `${brand.name} is a local-first desktop app for browsing, searching, and analyzing your coding agent sessions on this machine and on your SSH hosts.`;
write(tauriPath, JSON.stringify(tauri, null, 2) + '\n');

const pkg = JSON.parse(read('package.json'));
pkg.name = brand.slug;
write('package.json', JSON.stringify(pkg, null, 2) + '\n');

const lock = JSON.parse(read('package-lock.json'));
lock.name = brand.slug;
if (lock.packages?.['']) lock.packages[''].name = brand.slug;
write('package-lock.json', JSON.stringify(lock, null, 2) + '\n');

// The binary is named after the slug in dev too (not just in bundles, via
// mainBinaryName): GTK uses it as the Wayland app_id / X11 WM_CLASS, which the
// desktop entry's StartupWMClass must match for the dock to show our icon.
const cargoPath = 'src-tauri/Cargo.toml';
const bin = `[[bin]]\nname = ${JSON.stringify(brand.slug)}\npath = "src/main.rs"\n`;
let cargo = read(cargoPath)
  .replace(/^description = ".*"$/m, `description = ${JSON.stringify(brand.tagline)}`)
  .replace(/^authors = \[.*\]$/m, `authors = [${JSON.stringify(brand.author)}]`);
cargo = /^\[\[bin\]\]\nname = ".*"\npath = "src\/main\.rs"\n/m.test(cargo)
  ? cargo.replace(/^\[\[bin\]\]\nname = ".*"\npath = "src\/main\.rs"\n/m, bin)
  : cargo.replace(/^\[build-dependencies\]$/m, `${bin}\n[build-dependencies]`);
write(cargoPath, cargo);

// install.sh runs standalone (piped from curl), so it carries its own copy.
const installer = read('install.sh');
const block = /# --- generated from brand\.json by scripts\/brand\.mjs; do not edit ---\n[\s\S]*?# --- end generated ---/;
write('install.sh', installer.replace(block, [
  '# --- generated from brand.json by scripts/brand.mjs; do not edit ---',
  `APP_NAME=${JSON.stringify(brand.name)}`,
  `APP_SLUG=${JSON.stringify(brand.slug)}`,
  `REPO=${JSON.stringify(brand.repository)}`,
  '# --- end generated ---',
].join('\n')));
