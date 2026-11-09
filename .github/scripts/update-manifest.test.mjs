import assert from 'node:assert/strict';
import test from 'node:test';
import { updateManifest } from './update-manifest.mjs';

function release() {
  const files = ['darwin-universal-app.app.tar.gz', 'linux-amd64-appimage.AppImage', 'linux-aarch64-appimage.AppImage', 'linux-amd64-deb.deb', 'linux-arm64-deb.deb', 'linux-x86_64-rpm.rpm', 'linux-aarch64-rpm.rpm'];
  return {
    tag: 'v2.3.4', brand: { name: 'Hession', repository: 'owner/hession' }, notes: 'Release notes',
    assets: files.flatMap(file => ['', '.sig'].map(ext => ({ name: `Hession-v2.3.4-${file}${ext}`, state: 'uploaded', size: 100 }))),
    signatureFor: () => Buffer.from('untrusted comment: signature\nYWJj\ntrusted comment: timestamp:1\tfile:app\tversion:2.3.4\nZGVm\n').toString('base64'),
  };
}

test('feed covers both macOS architectures and every Linux package format', () => {
  const feed = updateManifest(release());
  assert.equal(Object.keys(feed.platforms).length, 8);
  assert.deepEqual(feed.platforms['darwin-aarch64'], feed.platforms['darwin-x86_64']);
  assert.match(feed.platforms['linux-x86_64-deb'].url, /linux-amd64-deb\.deb$/);
  assert.match(feed.platforms['linux-aarch64-rpm'].url, /linux-aarch64-rpm\.rpm$/);
  assert.equal(feed.version, '2.3.4');
});

test('incomplete or ambiguous release assets stop publication', () => {
  for (const change of [
    data => data.assets.pop(),
    data => { data.assets[0].size = 0; },
    data => { data.assets[0].state = 'new'; },
    data => data.assets.push(data.assets[0]),
  ]) {
    const data = release(); change(data);
    assert.throws(() => updateManifest(data), /incomplete updater asset/);
  }
});

test('feed rejects invalid signatures, wrong signed versions and prerelease tags', () => {
  const data = release();
  assert.throws(() => updateManifest({ ...data, signatureFor: () => 'bad signature' }), /Invalid signature/);
  assert.throws(() => updateManifest({ ...data, tag: 'v2.3.5' , assets: data.assets.map(asset => ({ ...asset, name: asset.name.replace('v2.3.4', 'v2.3.5') })) }), /missing the release version/);
  assert.throws(() => updateManifest({ ...data, tag: 'v2.3.4-beta' }), /stable/);
  assert.throws(() => updateManifest({ ...data, notes: '' }), /notes/);
});
