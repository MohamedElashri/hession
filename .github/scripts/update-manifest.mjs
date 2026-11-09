// Publish a feed only after all native packages and their signatures are uploaded.
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { releaseNotes } from './release-notes.mjs';

const targets = {
  'darwin-aarch64': 'darwin-universal-app.app.tar.gz',
  'darwin-x86_64': 'darwin-universal-app.app.tar.gz',
  'linux-x86_64-appimage': 'linux-amd64-appimage.AppImage',
  'linux-aarch64-appimage': 'linux-aarch64-appimage.AppImage',
  'linux-x86_64-deb': 'linux-amd64-deb.deb',
  'linux-aarch64-deb': 'linux-arm64-deb.deb',
  'linux-x86_64-rpm': 'linux-x86_64-rpm.rpm',
  'linux-aarch64-rpm': 'linux-aarch64-rpm.rpm',
};

export function updateManifest({ tag, brand, assets, signatureFor, notes, date = new Date().toISOString() }) {
  if (!/^v\d+\.\d+\.\d+$/.test(tag)) throw new Error('Expected a stable vMAJOR.MINOR.PATCH tag.');
  if (!notes?.trim()) throw new Error('Release notes are missing.');
  const version = tag.slice(1);
  const prefix = brand.name.replace(/\s+/g, '');
  const platforms = {};
  for (const [target, suffix] of Object.entries(targets)) {
    const name = `${prefix}-${tag}-${suffix}`;
    for (const required of [name, `${name}.sig`]) {
      const matching = assets.filter(asset => asset.name === required);
      if (matching.length !== 1 || matching[0].state !== 'uploaded' || !(matching[0].size > 0)) {
        throw new Error(`Missing or incomplete updater asset: ${required}`);
      }
    }
    const signature = signatureFor(`${name}.sig`).trim();
    if (!signature || !/^[A-Za-z0-9+/]+={0,2}$/.test(signature)) throw new Error(`Invalid signature: ${name}`);
    const decoded = Buffer.from(signature, 'base64');
    if (decoded.toString('base64') !== signature) throw new Error(`Invalid signature encoding: ${name}`);
    const lines = decoded.toString('utf8').trim().split('\n');
    if (lines.length !== 4 || !lines[0].startsWith('untrusted comment: ') || !lines[2].startsWith('trusted comment: ') ||
        !lines[2].slice('trusted comment: '.length).split('\t').includes(`version:${version}`)) {
      throw new Error(`Signature is missing the release version ${version}: ${name}`);
    }
    platforms[target] = { url: `https://github.com/${brand.repository}/releases/download/${tag}/${name}`, signature };
  }
  return { version, notes, pub_date: date, platforms };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [tag, assetsFile, signaturesDir, outputFile] = process.argv.slice(2);
  const brand = JSON.parse(readFileSync(new URL('../../brand.json', import.meta.url), 'utf8'));
  const assets = JSON.parse(readFileSync(assetsFile, 'utf8')).flat();
  const manifest = updateManifest({ tag, brand, assets, notes: releaseNotes(tag), signatureFor: name => readFileSync(join(signaturesDir, name), 'utf8') });
  writeFileSync(outputFile, JSON.stringify(manifest, null, 2) + '\n');
  console.log(`Verified signed update feed for ${Object.keys(manifest.platforms).length} targets.`);
}
