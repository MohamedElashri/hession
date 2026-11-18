"""Compare two CLIs serially on identical synthetic project catalogs (no user data)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
import zipfile


def measure(command, env, root):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    result = subprocess.run(command, env=env, cwd=root, capture_output=True, text=True, check=True, timeout=600)
    elapsed = (time.perf_counter() - start) * 1000
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return json.loads(result.stdout), {'wallMs': elapsed, 'cpuMs': (after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime) * 1000}


def contents(path):
    with zipfile.ZipFile(path) as archive:
        assert archive.testzip() is None
        manifest = json.loads(archive.read('manifest.json'))
        for item in manifest['checksums']:
            assert hashlib.sha256(archive.read(item['path'])).hexdigest() == item['sha256']
        # Compare every transcript/activity field, normalizing only export timestamps.
        normalized = {}
        entries = 0
        markdown_entries = 0
        for name in archive.namelist():
            raw = archive.read(name)
            if name.endswith('.json'):
                value = json.loads(raw)
                if name == 'manifest.json':
                    # Its timestamp-derived checksums were verified above.
                    value.pop('checksums', None)
                value.pop('exportedAt', None)
                value.pop('preparedAt', None)
                if name.startswith('sessions/'):
                    entries += sum(len(s['entries']) for s in value['sessions'])
                normalized[name] = value
            else:
                # Markdown's export timestamp is the only changing line.
                lines = raw.decode().splitlines()
                if name.startswith('sessions/') and name.endswith('.md'):
                    markdown_entries += sum(line == '#### user' for line in lines)
                normalized[name] = [line for line in lines if not line.startswith('Exported: ')]
        return normalized, entries or markdown_entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seed-binary', required=True, help='Desktop CLI providing index --once')
    parser.add_argument('--before', required=True)
    parser.add_argument('--after', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--sizes', nargs='+', type=int, default=[1000, 10000, 100000])
    parser.add_argument('--samples', type=int, default=3)
    args = parser.parse_args()
    assert all(1 < n <= 100000 for n in args.sizes) and 1 <= args.samples <= 10
    binaries = {key: str(Path(getattr(args, key)).resolve(strict=True)) for key in ['before', 'after']}
    seed = str(Path(args.seed_binary).resolve(strict=True))
    report = {'binaries': binaries, 'binarySha256': {key: hashlib.sha256(Path(path).read_bytes()).hexdigest() for key, path in binaries.items()}, 'cases': [], 'cache': 'warm; shared catalogs; alternating order'}
    with tempfile.TemporaryDirectory(prefix='hession-export-perf-') as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS']:
            env.pop(key, None)
        env['HOME'] = str(root)
        for key in ['HESSION_DATA_DIR', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'PI_CODING_AGENT_DIR', 'COMMANDCODE_HOME', 'HERMES_HOME', 'ANTIGRAVITY_HOME', 'CLINE_DIR', 'CLINE_DATA_DIR', 'CLINE_STORAGE_DIR']:
            env[key] = str(root / key.lower())
            Path(env[key]).mkdir(mode=0o700)
        for size in args.sizes:
            case = root / str(size)
            sources = case / 'codex/sessions'
            sources.mkdir(parents=True)
            (case / 'workspace').mkdir()
            env.update(CODEX_HOME=str(case / 'codex'), HESSION_DATA_DIR=str(case / 'library'))
            source = sources / 'history.jsonl'
            with source.open('w') as stream:
                stream.write(json.dumps({'type': 'session_meta', 'payload': {'id': f'export-perf-{size}', 'cwd': str(case / 'workspace')}}) + '\n')
                for number in range(1, size):
                    stream.write(json.dumps({'type': 'response_item', 'payload': {'type': 'message', 'role': 'user', 'id': str(number), 'content': [{'type': 'input_text', 'text': f'Event {number}: api_key=synthetic-secret ' + 'x' * 128}]}}) + '\n')
            summary, _ = measure([seed, 'index', '--once', '--agent', 'codex', '--json'], env, root)
            assert not summary['errors']
            database = case / 'library/sessions.sqlite'
            with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as conn:
                assert conn.execute('SELECT count(*) FROM entry').fetchone()[0] == size
                workspace = conn.execute('SELECT id FROM workspace').fetchone()[0]
            source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
            timings = {'before': [], 'after': []}
            reference = None
            for sample in range(args.samples):
                for key in (['before', 'after'] if sample % 2 == 0 else ['after', 'before']):
                    print(f'{size} entries: {key}, sample {sample + 1}/{args.samples}', file=sys.stderr, flush=True)
                    output = case / f'{key}-{sample}.zip'
                    _, timing = measure([binaries[key], 'export', 'project', f'workspace:{workspace}', '--database', str(database), '--output', str(output), '--json'], env, root)
                    normalized, entries = contents(output)
                    assert entries == size - 1  # Native session_meta is intentionally omitted.
                    assert 'synthetic-secret' not in json.dumps(normalized)
                    if reference is None:
                        reference = normalized
                    else:
                        assert normalized == reference, 'Archive content changed beyond export timestamps'
                    timings[key].append(timing)
                    output.unlink()
            assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
            with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as conn:
                assert conn.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
            row = {'entries': size, 'samples': timings, 'median': {key: {metric: statistics.median(s[metric] for s in samples) for metric in ['wallMs', 'cpuMs']} for key, samples in timings.items()}, 'archivesEquivalent': True, 'sourcePreserved': True}
            report['cases'].append(row)
            Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(row), flush=True)


if __name__ == '__main__':
    main()
