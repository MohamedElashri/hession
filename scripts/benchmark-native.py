"""Synthetic indexing/export/idle measurements against an explicitly chosen binary."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import sqlite3
import statistics
import subprocess
import tempfile
import time
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--sizes', nargs='+', type=int, default=[1000, 10000, 100000])
    parser.add_argument('--samples', type=int, default=3)
    parser.add_argument('--idle', action='store_true', help='Measure 30 seconds of watcher CPU on Linux')
    args = parser.parse_args()
    assert all(1 < size <= 100000 for size in args.sizes) and 1 <= args.samples <= 10
    binary = str(Path(args.binary).resolve(strict=True))
    report = {'binary': binary, 'version': subprocess.check_output([binary, '--version'], text=True).strip(), 'cases': []}
    with tempfile.TemporaryDirectory(prefix='hession-native-perf-') as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS']:
            env.pop(key, None)
        env['HOME'] = str(root)
        for key in ['HESSION_DATA_DIR', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'PI_CODING_AGENT_DIR', 'COMMANDCODE_HOME', 'HERMES_HOME', 'ANTIGRAVITY_HOME', 'CLINE_DIR', 'CLINE_DATA_DIR', 'CLINE_STORAGE_DIR']:
            env[key] = str(root / key.lower())
            Path(env[key]).mkdir(mode=0o700)

        def measure(arguments):
            before = resource.getrusage(resource.RUSAGE_CHILDREN)
            start = time.perf_counter()
            result = subprocess.run([binary, *arguments], cwd=root, env=env, capture_output=True, text=True, timeout=600, check=True)
            wall = (time.perf_counter() - start) * 1000
            after = resource.getrusage(resource.RUSAGE_CHILDREN)
            return json.loads(result.stdout), {'wallMs': wall, 'cpuMs': (after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime) * 1000}

        def event(number):
            return json.dumps({'type': 'response_item', 'payload': {'type': 'message', 'role': 'user', 'id': str(number), 'content': [{'type': 'input_text', 'text': f'Synthetic progress event {number}: ' + 'x' * 128}]}}) + '\n'

        for size in args.sizes:
            case = root / str(size)
            source_dir = case / 'codex/sessions'
            source_dir.mkdir(parents=True)
            (case / 'workspace').mkdir()
            env.update(CODEX_HOME=str(case / 'codex'), HESSION_DATA_DIR=str(case / 'library'))
            source = source_dir / 'history.jsonl'
            with source.open('w') as stream:
                stream.write(json.dumps({'type': 'session_meta', 'payload': {'id': f'performance-{size}', 'cwd': str(case / 'workspace')}}) + '\n')
                for number in range(1, size):
                    stream.write(event(number))
            original = source.read_bytes()
            summary, initial = measure(['index', '--once', '--agent', 'codex', '--json'])
            assert not summary['errors']
            database = case / 'library/sessions.sqlite'
            with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as conn:
                assert conn.execute('SELECT count(*) FROM entry').fetchone()[0] == size
                first_id = conn.execute('SELECT id FROM entry ORDER BY ordinal LIMIT 1').fetchone()[0]
                workspace = conn.execute('SELECT id FROM workspace').fetchone()[0]
            append_samples = []
            for sample in range(args.samples):
                with source.open('a') as stream:
                    for number in range(10):
                        stream.write(event(size + sample * 10 + number))
                    stream.flush()
                    os.fsync(stream.fileno())
                summary, timing = measure(['index', '--once', '--agent', 'codex', '--json'])
                assert not summary['errors']
                append_samples.append(timing)
                with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as conn:
                    assert conn.execute('SELECT count(*) FROM entry').fetchone()[0] == size + (sample + 1) * 10
                    assert conn.execute('SELECT id FROM entry ORDER BY ordinal LIMIT 1').fetchone()[0] == first_id
            baseline_source = hashlib.sha256(source.read_bytes()).hexdigest()
            export_samples = []
            for sample in range(args.samples):
                archive = case / f'archive-{sample}.zip'
                _, timing = measure(['export', 'project', f'workspace:{workspace}', '--format', 'zip', '--output', str(archive), '--json'])
                export_samples.append(timing)
                with zipfile.ZipFile(archive) as exported:
                    assert exported.testzip() is None
                    assert 'manifest.json' in exported.namelist()
                archive.unlink()
            assert source.read_bytes().startswith(original)
            assert hashlib.sha256(source.read_bytes()).hexdigest() == baseline_source
            with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as conn:
                assert conn.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
            row = {'initialEntries': size, 'nativeBytes': source.stat().st_size, 'databaseMainFileBytes': database.stat().st_size, 'databaseWalBytes': Path(str(database) + '-wal').stat().st_size if Path(str(database) + '-wal').exists() else 0, 'initialImport': initial, 'fixedTenAppendSamples': append_samples, 'appendMedianMs': statistics.median(x['wallMs'] for x in append_samples), 'exportSamples': export_samples, 'exportMedianMs': statistics.median(x['wallMs'] for x in export_samples), 'sourcePreserved': True, 'integrity': 'ok'}
            report['cases'].append(row)
            print(json.dumps(row), flush=True)
            Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
        if args.idle:
            assert Path('/proc/self/stat').exists(), 'Idle measurement needs Linux /proc'
            case = root / 'idle'
            (case / 'codex/sessions').mkdir(parents=True)
            env.update(CODEX_HOME=str(case / 'codex'), HESSION_DATA_DIR=str(case / 'library'))
            for index in range(100):
                with (case / f'codex/sessions/{index}.jsonl').open('w') as stream:
                    stream.write(json.dumps({'type': 'session_meta', 'payload': {'id': f'idle-{index}', 'cwd': str(case / 'workspace')}}) + '\n')
                    for number in range(10): stream.write(event(number))
            measure(['index', '--once', '--agent', 'codex', '--json'])
            with tempfile.TemporaryFile() as output:
                child = subprocess.Popen([binary, 'index', '--watch', '--agent', 'codex', '--json'], cwd=root, env=env, stdout=output, stderr=output)
                try:
                    time.sleep(5)
                    assert child.poll() is None
                    def ticks():
                        fields = Path(f'/proc/{child.pid}/stat').read_text().rsplit(')', 1)[1].split()
                        return int(fields[11]) + int(fields[12])
                    before = ticks()
                    start = time.perf_counter()
                    time.sleep(30)
                    wall = time.perf_counter() - start
                    assert child.poll() is None
                    cpu = (ticks() - before) / os.sysconf('SC_CLK_TCK')
                    report['idle'] = {'sources': 100, 'entries': 1100, 'wallSeconds': wall, 'processCpuSeconds': cpu, 'oneCoreCpuPercent': cpu / wall * 100}
                finally:
                    if child.poll() is None: child.terminate()
                    try: child.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        child.kill(); child.wait()
            Path(args.output).write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
