import { readFileSync } from 'node:fs';
import { releaseNotes } from './release-notes.mjs';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const json = (path) => JSON.parse(read(path));
const config = json('src-tauri/tauri.conf.json');
const version = config.version;
const tag = process.argv[2] || process.env.RELEASE_TAG;
const assetsPath = process.argv[3];

function requireCondition(condition, message) {
  if (!condition) throw new Error(message);
}

requireCondition(
  typeof version === 'string' && /^\d+\.\d+\.\d+$/.test(version),
  'Release version must be a numeric major.minor.patch version.',
);

const cargoPackage = read('src-tauri/Cargo.toml')
  .split(/^\[/m)
  .find((section) => section.startsWith('package]'));
const cargoVersion = cargoPackage?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const npmLock = json('package-lock.json');
for (const [name, actual] of Object.entries({
  'package.json': json('package.json').version,
  'package-lock.json': npmLock.version,
  'package-lock.json root package': npmLock.packages?.['']?.version,
  'src-tauri/Cargo.toml': cargoVersion,
})) {
  requireCondition(actual === version, `${name} version must match Tauri ${version}.`);
}
if (tag !== undefined) {
  requireCondition(tag === `v${version}`, `Release tag must be v${version}.`);
}
requireCondition(releaseNotes(version), `CHANGELOG.md needs a "## [${version}]" entry for this release.`);

// Per-bundle architecture names follow tauri-action.
const suffixes = [
  'darwin-universal-dmg.dmg',
  'linux-amd64-appimage.AppImage',
  'linux-amd64-deb.deb',
  'linux-x86_64-rpm.rpm',
  'linux-aarch64-appimage.AppImage',
  'linux-arm64-deb.deb',
  'linux-aarch64-rpm.rpm',
];
// Must match releaseAssetNamePattern in release.yml (brand name without spaces).
const assetPrefix = json('brand.json').name.replace(/\s+/g, '');
const expectedNames = suffixes.map((suffix) => `${assetPrefix}-v${version}-${suffix}`);

if (assetsPath) {
  // gh api --paginate --slurp produces an array of pages.
  const assets = JSON.parse(readFileSync(assetsPath, 'utf8')).flat();
  for (const name of expectedNames) {
    const matches = assets.filter((asset) => asset.name === name);
    requireCondition(
      matches.length === 1 && matches[0].size > 0 && matches[0].state === 'uploaded',
      `Release is missing a complete installer: ${name}`,
    );
  }
  console.log('All seven release installers are uploaded and nonempty.');
}

console.log(`Release metadata matches v${version}.`);
