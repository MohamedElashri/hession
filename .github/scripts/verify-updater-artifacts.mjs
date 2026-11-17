// Independent, streaming verification of Tauri's prehashed Minisign packages.
import { createHash, createPublicKey, verify } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { readFile, readdir, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

function decode(value, length) {
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(value)) throw new Error('Invalid base64');
  const bytes = Buffer.from(value, 'base64');
  if (bytes.toString('base64') !== value || (length != null && bytes.length !== length)) throw new Error('Invalid encoded length');
  return bytes;
}
export function checkSignature(digest, encodedSignature, encodedPublic, version) {
  const publicLines = decode(encodedPublic.trim()).toString('utf8').trim().split(/\r?\n/);
  const signatureLines = decode(encodedSignature.trim()).toString('utf8').trim().split(/\r?\n/);
  if (publicLines.length !== 2 || !publicLines[0].startsWith('untrusted comment: ') || signatureLines.length !== 4 ||
      !signatureLines[0].startsWith('untrusted comment: ') || !signatureLines[2].startsWith('trusted comment: ')) throw new Error('Invalid Minisign structure');
  const publicBytes = decode(publicLines[1], 42), signature = decode(signatureLines[1], 74), global = decode(signatureLines[3], 64);
  if (publicBytes.subarray(0, 2).toString() !== 'Ed' || signature.subarray(0, 2).toString() !== 'ED') throw new Error('Expected prehashed Ed25519 Minisign');
  if (!publicBytes.subarray(2, 10).equals(signature.subarray(2, 10))) throw new Error('Signing key does not match');
  const key = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), publicBytes.subarray(10)]), type: 'spki', format: 'der' });
  const rawSignature = signature.subarray(10), comment = signatureLines[2].slice('trusted comment: '.length);
  if (!verify(null, digest, key, rawSignature) || !verify(null, Buffer.concat([rawSignature, Buffer.from(comment)]), key, global)) throw new Error('Signature verification failed');
  const versions = comment.split('\t').filter(field => field.startsWith('version:'));
  if (versions.length !== 1 || versions[0] !== `version:${version}`) throw new Error('Signed release version does not match');
}
async function signatures(root) {
  const result = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) result.push(...await signatures(path));
    else if (entry.isFile()) {
      if (entry.name.endsWith('.sig')) result.push(path);
      else if (/\.(?:deb|rpm|AppImage|app\.tar\.gz)$/.test(entry.name)) {
        if (!(await stat(`${path}.sig`)).isFile()) throw new Error('Missing updater signature');
      }
    }
  }
  return result;
}
export async function checkArtifacts(root, publicFile, version) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Expected numeric release version');
  if ((await stat(publicFile)).size > 4096) throw new Error('Public key exceeds limit');
  const publicKey = await readFile(publicFile, 'utf8'), files = await signatures(root);
  if (!files.length) throw new Error('No signed updater artifacts found');
  for (const signatureFile of files) {
    if ((await stat(signatureFile)).size > 16384) throw new Error('Signature exceeds limit');
    const artifact = signatureFile.slice(0, -4);
    if (!(await stat(artifact)).isFile() || !(await stat(artifact)).size) throw new Error('Missing signed package');
    const hash = createHash('blake2b512');
    for await (const chunk of createReadStream(artifact)) hash.update(chunk);
    checkSignature(hash.digest(), await readFile(signatureFile, 'utf8'), publicKey, version);
  }
  return files.length;
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const count = await checkArtifacts(...process.argv.slice(2));
  console.log(`Verified ${count} updater package signatures and their signed release version.`);
}
