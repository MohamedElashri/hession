"""Linux peak-memory and append-reuse probe, using synthetic local histories only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import sqlite3
import subprocess
import tempfile
import time


def digest_rows(database):
    digest = hashlib.sha256()
    with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as conn:
        for query in [
            "SELECT ordinal,kind,role,search_text,payload_json FROM entry WHERE session_id=(SELECT id FROM session WHERE source_uri<>'fixture://memory-sentinel' LIMIT 1) ORDER BY ordinal",
            "SELECT e.ordinal,p.ordinal,p.kind,p.text,p.data_json FROM content_part p JOIN entry e ON e.id=p.entry_id WHERE e.session_id=(SELECT id FROM session WHERE source_uri<>'fixture://memory-sentinel' LIMIT 1) ORDER BY e.ordinal,p.ordinal",
        ]:
            for row in conn.execute(query):
                digest.update(json.dumps(row, ensure_ascii=False, separators=(',', ':')).encode())
                digest.update(b'\n')
        assert conn.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--baseline')
    parser.add_argument('--family', choices=['pi', 'commandcode', 'codex', 'claude'], default='pi')
    parser.add_argument('--entries', type=int, default=1000)
    parser.add_argument('--text-bytes', type=int, default=49152)
    parser.add_argument('--max-peak-rss-mib', type=float, default=0)
    parser.add_argument('--require-stable-append', action='store_true')
    args = parser.parse_args()
    assert Path('/proc').is_dir() and Path('/usr/bin/time').is_file(), 'This probe requires Linux /proc and GNU time'
    assert 1 <= args.entries <= 10000 and 1 <= args.text_bytes <= 1024 * 1024
    assert args.entries * args.text_bytes < 100 * 1024 * 1024
    binary = str(Path(args.binary).resolve(strict=True))
    with open(binary, 'rb') as stream:
        binary_hash = hashlib.file_digest(stream, 'sha256').hexdigest()
    report = {'binary': binary, 'binarySha256': binary_hash, 'parameters': vars(args)}
    with tempfile.TemporaryDirectory(prefix='hession-import-memory-') as temporary:
        root = Path(temporary)
        store = root / 'native'
        sub = 'projects' if args.family in ('claude', 'commandcode') else 'sessions'
        histories = store / sub
        histories.mkdir(parents=True)
        source = histories / 'history.jsonl'
        def entry(number, text):
            message = {'role': 'user', 'content': text}
            if args.family == 'codex':
                return {'type': 'event_msg', 'payload': {'type': 'user_message', 'message': text}}
            return {'type': 'user' if args.family == 'claude' else 'message', 'id': str(number), 'uuid': str(number), 'sessionId': 'memory-fixture', 'message': message}
        with source.open('w') as stream:
            for number in range(args.entries):
                stream.write(json.dumps(entry(number, f'{number}: ' + 'x' * args.text_bytes)) + '\n')
        original_hash = hashlib.sha256(source.read_bytes()).hexdigest()
        report['sourceBytes'] = source.stat().st_size
        env = dict(os.environ, HOME=str(root), HESSION_DATA_DIR=str(root / 'library'))
        for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS']:
            env.pop(key, None)
        source_key = {'pi': 'PI_CODING_AGENT_DIR', 'commandcode': 'COMMANDCODE_HOME', 'claude': 'CLAUDE_CONFIG_DIR', 'codex': 'CODEX_HOME'}[args.family]
        env[source_key] = str(store)
        database = root / 'library/sessions.sqlite'
        def run(name):
            peak = root / f'{name}.rss'
            before = resource.getrusage(resource.RUSAGE_CHILDREN)
            start = time.perf_counter()
            result = subprocess.run(['/usr/bin/time', '-f', '%M', '-o', str(peak), binary, 'index', '--once', '--agent', args.family, '--json'], cwd=root, env=env, text=True, capture_output=True, timeout=300)
            elapsed = time.perf_counter() - start
            after = resource.getrusage(resource.RUSAGE_CHILDREN)
            assert result.returncode == 0, result.stderr
            summary = json.loads(result.stdout)
            assert not summary['errors'] and not summary['cancelled'], summary
            return {'wallMs': elapsed * 1000, 'cpuMs': ((after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime)) * 1000,
                    'peakRssKiB': int(peak.read_text().strip()), 'summary': summary}
        report['initial'] = run('initial')
        assert report['initial']['summary']['records'] == args.entries
        report['initialContentDigest'] = digest_rows(database)
        with sqlite3.connect(database) as conn:
            session, agent = conn.execute('SELECT id,agent_id FROM session').fetchone()
            old_ids = conn.execute('SELECT id FROM entry WHERE session_id=? ORDER BY ordinal', (session,)).fetchall()
            conn.execute("INSERT INTO session(agent_id,external_id,source_uri,title) VALUES(?,'memory-sentinel','fixture://memory-sentinel','Sentinel')", (agent,))
            sentinel = conn.execute("SELECT id FROM session WHERE source_uri='fixture://memory-sentinel'").fetchone()[0]
            conn.execute("INSERT INTO entry(session_id,ordinal,kind,payload_json) VALUES(?,0,'event','{}')", (sentinel,))
            conn.execute('INSERT INTO session_annotation(session_id,starred) VALUES(?,1)', (session,))
        report['quiet'] = run('quiet')
        assert report['quiet']['summary']['records'] == 0
        assert digest_rows(database) == report['initialContentDigest']
        assert hashlib.sha256(source.read_bytes()).hexdigest() == original_hash
        with source.open('a') as stream:
            stream.write(json.dumps(entry(args.entries, 'One controlled continuation')) + '\n')
        expected_hash = hashlib.sha256(source.read_bytes()).hexdigest()
        report['append'] = run('append')
        report['appendedContentDigest'] = digest_rows(database)
        with sqlite3.connect(database) as conn:
            ids = conn.execute('SELECT id FROM entry WHERE session_id=? ORDER BY ordinal', (session,)).fetchall()
            assert len(ids) == args.entries + 1
            report['oldEntryIdsPreserved'] = ids[:args.entries] == old_ids
            assert conn.execute('SELECT starred FROM session_annotation WHERE session_id=?', (session,)).fetchone()[0] == 1
        assert hashlib.sha256(source.read_bytes()).hexdigest() == expected_hash
        report['integrity'] = 'ok'
    if args.baseline:
        baseline = json.loads(Path(args.baseline).read_text())
        for key in ['family', 'entries', 'text_bytes']:
            assert baseline['parameters'][key] == report['parameters'][key]
        assert baseline['initialContentDigest'] == report['initialContentDigest']
        assert baseline['appendedContentDigest'] == report['appendedContentDigest']
        report['improvement'] = {phase: {
            'peakRss': baseline[phase]['peakRssKiB'] / report[phase]['peakRssKiB'],
            'cpu': baseline[phase]['cpuMs'] / report[phase]['cpuMs'],
            'wall': baseline[phase]['wallMs'] / report[phase]['wallMs'],
        } for phase in ['initial', 'quiet', 'append']}
    Path(args.output).parent.mkdir(parents=True, exist_ok=True)
    Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    if args.require_stable_append:
        assert report['oldEntryIdsPreserved'], 'Append replaced existing entry IDs'
        assert report['append']['summary']['records'] == 1, 'Append renormalized historical entries'
    if args.max_peak_rss_mib:
        assert all(report[phase]['peakRssKiB'] <= args.max_peak_rss_mib * 1024 for phase in ['initial', 'quiet', 'append'])


if __name__ == '__main__':
    main()
