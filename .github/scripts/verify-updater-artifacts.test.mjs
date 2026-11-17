import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import assert from 'node:assert/strict';
import test from 'node:test';
import { checkSignature, checkArtifacts } from './verify-updater-artifacts.mjs';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const { privateKey, publicKey } = generateKeyPairSync('ed25519');
const id = Buffer.from('0102030405060708', 'hex');
const encode = value => Buffer.from(value).toString('base64');
const publicText = encode(`untrusted comment: synthetic key\n${Buffer.concat([Buffer.from('Ed'), id, publicKey.export({ type: 'spki', format: 'der' }).subarray(-32)]).toString('base64')}\n`);
const digest = createHash('blake2b512').update('synthetic updater artifact').digest();
function signature(comment = 'timestamp:0\tfile:fixture\tversion:0.5.0') {
  const raw = sign(null, digest, privateKey);
  return encode(`untrusted comment: synthetic signature\n${Buffer.concat([Buffer.from('ED'), id, raw]).toString('base64')}\ntrusted comment: ${comment}\n${sign(null, Buffer.concat([raw, Buffer.from(comment)]), privateKey).toString('base64')}\n`);
}
test('authenticates package digest and trusted version independently', () => {
  checkSignature(digest, signature(), publicText, '0.5.0');
  assert.throws(() => checkSignature(createHash('blake2b512').update('tampered').digest(), signature(), publicText, '0.5.0'), /verification/);
  assert.throws(() => checkSignature(digest, signature(), publicText, '0.5.1'), /version/);
  assert.throws(() => checkSignature(digest, signature('timestamp:0'), publicText, '0.5.0'), /version/);
  assert.throws(() => checkSignature(digest, signature('version:0.5.0\tversion:0.5.0'), publicText, '0.5.0'), /version/);
  const tampered = encode(Buffer.from(signature(), 'base64').toString().replace('version:0.5.0', 'version:0.5.1'));
  assert.throws(() => checkSignature(digest, tampered, publicText, '0.5.1'), /verification/);
  assert.throws(() => checkSignature(digest, '***', publicText, '0.5.0'), /base64/);
});

test('requires a signature for each updater package', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'hession-updater-test-'));
  try {
    const key = join(directory, 'key.pub');
    await writeFile(key, publicText);
    await writeFile(join(directory, 'Hession_0.5.0_amd64.deb'), 'unsigned package');
    await assert.rejects(checkArtifacts(directory, key, '0.5.0'), /ENOENT|Missing updater signature/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
