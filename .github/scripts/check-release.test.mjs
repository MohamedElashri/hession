import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';

function fixture(run) {
  const root = mkdtempSync(join(tmpdir(), 'hession-release-'));
  const write = (path, content) => {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), content);
  };
  for (const script of ['check-release.mjs', 'release-notes.mjs']) {
    write(`.github/scripts/${script}`, readFileSync(new URL(script, import.meta.url)));
  }
  cpSync(new URL('../../bump_version.py', import.meta.url), join(root, 'bump_version.py'));
  write('package.json', '{"version":"0.3.0"}');
  write('package-lock.json', JSON.stringify({ version: '0.3.0', packages: { '': { version: '0.3.0' }, 'node_modules/example': { version: '1.2.3' } } }));
  write('src-tauri/tauri.conf.json', '{"version":"0.3.0"}');
  write('brand.json', '{"name":"Hession"}');
  write('CHANGELOG.md', '## [0.3.0]\n\nA complete release.\n\n## [0.4.0]\n\nAnother release.\n');
  for (const [path, name] of [
    ['src-tauri/Cargo.toml', 'app'], ['crates/hession-sync/Cargo.toml', 'hession-sync'],
    ['crates/hession-relay/Cargo.toml', 'hession-relay'],
  ]) write(path, `[package]\nname = "${name}"\nversion = "0.3.0"\n\n[dependencies]\nexample = "1.2.3"\n`);
  write('Cargo.lock', ['app', 'hession-sync', 'hession-relay'].map(name => `[[package]]\nname = "${name}"\nversion = "0.3.0"\n`).join('\n'));
  write('deploy/relay/compose.yaml', 'services:\n  relay:\n    image: hession-relay:0.3.0\n');
  const check = version => spawnSync(process.execPath, [join(root, '.github/scripts/check-release.mjs'), `v${version}`], { encoding: 'utf8' });
  try { run({ root, write, check, read: path => readFileSync(join(root, path), 'utf8') }); }
  finally { rmSync(root, { recursive: true, force: true }); }
}

test('release checks accept matching desktop, relay, image, and lockfile versions', () => fixture(({ check }) => {
  const result = check('0.3.0');
  assert.equal(result.status, 0, result.stderr);
}));

for (const [path, marker] of [
  ['crates/hession-relay/Cargo.toml', 'crates/hession-relay/Cargo.toml'],
  ['crates/hession-sync/Cargo.toml', 'crates/hession-sync/Cargo.toml'],
  ['deploy/relay/compose.yaml', 'deploy/relay/compose.yaml image'],
]) test(`release checks reject stale ${path}`, () => fixture(({ read, write, check }) => {
  write(path, read(path).replace('0.3.0', '0.2.1'));
  const result = check('0.3.0');
  assert.notEqual(result.status, 0);
  assert(result.stderr.includes(`${marker} version must match`), result.stderr);
}));

for (const name of ['app', 'hession-sync', 'hession-relay']) {
  test(`release checks reject a stale ${name} lockfile entry`, () => fixture(({ read, write, check }) => {
    write('Cargo.lock', read('Cargo.lock').replace(`name = "${name}"\nversion = "0.3.0"`, `name = "${name}"\nversion = "0.2.1"`));
    const result = check('0.3.0');
    assert.notEqual(result.status, 0);
    assert(result.stderr.includes(`Cargo.lock (${name}) version must match`), result.stderr);
  }));
}

test('version bump updates every release component without changing dependencies', () => fixture(({ root, read, check }) => {
  const result = spawnSync('python3', [join(root, 'bump_version.py'), '0.4.0', '--yes', '--skip-checks'], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(check('0.4.0').status, 0);
  assert.equal(JSON.parse(read('package-lock.json')).packages['node_modules/example'].version, '1.2.3');
  assert(read('crates/hession-relay/Cargo.toml').includes('example = "1.2.3"'));
}));
