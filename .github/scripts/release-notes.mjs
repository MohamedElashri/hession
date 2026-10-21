// Prints the CHANGELOG.md section for a version (`v0.1.1` or `0.1.1`), used as
// the GitHub release notes. Exits non-zero if the section is missing or empty.
import { readFileSync } from 'node:fs';

export function releaseNotes(version, changelog = readFileSync(new URL('../../CHANGELOG.md', import.meta.url), 'utf8')) {
  const plain = String(version).replace(/^v/, '');
  const lines = changelog.split('\n');
  const start = lines.findIndex((line) => line.startsWith(`## [${plain}]`));
  if (start < 0) return null;
  const end = lines.findIndex((line, i) => i > start && (line.startsWith('## [') || /^\[[^\]]+\]: /.test(line)));
  const body = lines.slice(start + 1, end < 0 ? undefined : end).join('\n').trim();
  return body || null;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const notes = releaseNotes(process.argv[2] ?? '');
  if (!notes) {
    console.error(`CHANGELOG.md has no entry for ${process.argv[2]}.`);
    process.exit(1);
  }
  console.log(notes);
}
