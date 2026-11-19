"""Linux idle/active observer measurement; only generated histories are indexed.

Run release builds alone, with identical arguments for baseline and candidate.
CPU is measured with process-wide CPU clocks (including retired threads) and
per-thread scheduler runtime at nanosecond resolution. Context switches expose idle wakeups even below the
process accounting clock's resolution. Use --gui on an isolated X11 desktop
and private session bus to include the minimized GUI and WebKit processes.
"""
import argparse
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import sqlite3
import statistics
import subprocess
import tempfile
import time


# POSIX process clocks include retired threads without /proc/stat tick rounding.
# https://man7.org/linux/man-pages/man3/clock_getcpuclockid.3.html
_process_clock = ctypes.CDLL(None).clock_getcpuclockid
_process_clock.argtypes = [ctypes.c_int, ctypes.POINTER(ctypes.c_int)]
_process_clock.restype = ctypes.c_int


def process_clock(pid):
    clock = ctypes.c_int()
    error = _process_clock(pid, ctypes.byref(clock))
    if error:
        raise OSError(error, os.strerror(error))
    return time.clock_gettime_ns(clock.value)


def counters(pid):
    root = Path('/proc') / str(pid)
    threads = {}
    for thread in (root / 'task').iterdir():
        try:
            status = dict(line.split(':', 1) for line in (thread / 'status').read_text().splitlines() if ':' in line)
            threads[thread.name] = {
                'name': status['Name'].strip(),
                'runtimeNs': int((thread / 'schedstat').read_text().split()[0]),
                'switches': int(status['voluntary_ctxt_switches']) + int(status['nonvoluntary_ctxt_switches']),
            }
        except FileNotFoundError:
            pass
    status = dict(line.split(':', 1) for line in (root / 'status').read_text().splitlines() if ':' in line)
    io = dict(line.split(':', 1) for line in (root / 'io').read_text().splitlines())
    clock_cpu_ms = process_clock(pid) / 1e6
    rollup = dict(line.split(':', 1) for line in (root / 'smaps_rollup').read_text().splitlines() if ':' in line)
    return {'threads': threads, 'clockCpuMs': clock_cpu_ms, 'readBytes': int(io['rchar']), 'rssKiB': int(status['VmRSS'].split()[0]),
            'pssKiB': int(rollup['Pss'].split()[0]), 'memoryBreakdownKiB': {key: int(rollup[key].split()[0]) for key in ['Pss_Anon', 'Pss_File', 'Pss_Shmem', 'Private_Clean', 'Private_Dirty'] if key in rollup}, 'peakRssKiB': int(status['VmHWM'].split()[0])}


def delta(before, after, seconds):
    runtime = switches = 0
    observer = {}
    thread_rows = {}
    for tid, previous in before['threads'].items():
        current = after['threads'].get(tid)
        if current is None:
            continue
        elapsed = current['runtimeNs'] - previous['runtimeNs']
        wakes = current['switches'] - previous['switches']
        runtime += elapsed
        switches += wakes
        thread_rows[tid] = {'name': previous['name'], 'cpuMs': elapsed / 1e6, 'contextSwitches': wakes}
        if previous['name'] == 'history-observe':  # Linux comm is limited to 15 bytes.
            observer = {'cpuMs': elapsed / 1e6, 'oneCoreCpuPercent': elapsed / 1e9 / seconds * 100, 'contextSwitches': wakes}
    return {'wallSeconds': seconds, 'cpuMs': runtime / 1e6, 'oneCoreCpuPercent': runtime / 1e9 / seconds * 100,
            'contextSwitches': switches, 'clockCpuMs': after['clockCpuMs'] - before['clockCpuMs'], 'readBytes': after['readBytes'] - before['readBytes'], 'rssKiB': after['rssKiB'],
            'peakRssKiB': after['peakRssKiB'], 'pssKiB': after['pssKiB'], 'memoryBreakdownKiB': after['memoryBreakdownKiB'], 'observer': observer,
            'threads': thread_rows, 'newThreads': len(after['threads'].keys() - before['threads'].keys()),
            'retiredThreads': len(before['threads'].keys() - after['threads'].keys())}


def tree_counters(pid):
    result, pending = {}, [pid]
    while pending:
        current = pending.pop()
        if current in result:
            continue
        try:
            result[current] = counters(current)
            for task in (Path('/proc') / str(current) / 'task').iterdir():
                pending.extend(int(value) for value in (task / 'children').read_text().split())
        except FileNotFoundError:
            pass
    return result


def tree_delta(before, after, seconds, primary):
    rows = {pid: delta(previous, after[pid], seconds) for pid, previous in before.items() if pid in after}
    result = {key: sum(row[key] for row in rows.values()) for key in
              ['cpuMs', 'clockCpuMs', 'contextSwitches', 'readBytes', 'rssKiB', 'pssKiB', 'peakRssKiB']}
    result.update(wallSeconds=seconds, oneCoreCpuPercent=result['cpuMs'] / 10 / seconds,
                  clockOneCoreCpuPercent=result['clockCpuMs'] / 10 / seconds,
                  newProcesses=len(after.keys() - before.keys()), retiredProcesses=len(before.keys() - after.keys()),
                  processes=rows, observer=rows[primary]['observer'])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--gui', action='store_true', help='Include the minimized GUI and WebKit children; requires a private X11 display and session bus')
    parser.add_argument('--output', required=True)
    parser.add_argument('--baseline', help='Compare a report from identical corpus/sampling parameters')
    parser.add_argument('--require-improvement', type=float, default=0,
                        help='Minimum factor for observer idle CPU, observer wakeups, and append read volume; RAM is reported separately')
    parser.add_argument('--empty-gui-report', help='Tiny-library GUI control from the same binary for measuring added library memory')
    parser.add_argument('--max-native-overhead-mib', type=float, default=0)
    parser.add_argument('--max-gui-overhead-mib', type=float, default=0)
    parser.add_argument('--max-idle-rss-mib', type=float, default=0, help='Fail if retained idle RSS exceeds this budget')
    parser.add_argument('--max-idle-pss-mib', type=float, default=0, help='Fail if retained idle PSS exceeds this budget')
    parser.add_argument('--hermes-package-dirs', type=int, default=0, help='GUI-only synthetic Hermes environment tree to reproduce watch-budget exhaustion')
    parser.add_argument('--files', type=int, default=160)
    parser.add_argument('--entries', type=int, default=100)
    parser.add_argument('--text-bytes', type=int, default=16384)
    parser.add_argument('--idle-seconds', type=float, default=35)
    parser.add_argument('--samples', type=int, default=3)
    args = parser.parse_args()
    assert 1 <= args.files <= 10000 and 1 <= args.entries <= 10000 and 1 <= args.text_bytes <= 1024 * 1024
    assert 30 <= args.idle_seconds <= 600 and 1 <= args.samples <= 100
    assert 0 <= args.hermes_package_dirs <= 20000 and (args.gui or not args.hermes_package_dirs)
    assert args.require_improvement >= 0 and (not args.require_improvement or args.baseline)
    assert not (args.max_native_overhead_mib or args.max_gui_overhead_mib) or (args.gui and args.empty_gui_report)
    binary = str(Path(args.binary).resolve(strict=True))
    if args.gui:
        assert os.environ.get('HESSION_NATIVE_SMOKE_ISOLATED') == '1', 'GUI benchmarks require an explicitly isolated X11 display/session bus'
        assert os.environ.get('DISPLAY') and os.environ.get('DBUS_SESSION_BUS_ADDRESS')
    cpu = Path('/proc/cpuinfo').read_text()
    host = {'kernel': os.uname().release, 'machine': os.uname().machine, 'logicalCpus': os.cpu_count(),
            'cpu': next((line.split(':', 1)[1].strip() for line in cpu.splitlines() if line.startswith('model name')), 'unknown')}
    with open(binary, 'rb') as stream:
        binary_hash = hashlib.file_digest(stream, 'sha256').hexdigest()
    report = {'host': host, 'binarySha256': binary_hash, 'binary': binary, 'version': subprocess.check_output([binary, '--version'], text=True).strip(),
              'parameters': vars(args), 'note': 'Synthetic local-only histories. GUI mode includes WebKit children; headless mode excludes GUI/WebKit. Compare identical arguments and host conditions.'}
    with tempfile.TemporaryDirectory(prefix='hession-idle-') as temporary:
        root = Path(temporary)
        source_dir = root / 'codex/sessions'
        source_dir.mkdir(parents=True)
        env = dict(os.environ, CODEX_HOME=str(source_dir.parent), HESSION_DATA_DIR=str(root / 'library'))
        if args.gui:
            env.pop('WAYLAND_DISPLAY', None)
            env['GDK_BACKEND'] = 'x11'
            # Resolve fallback home paths inside the disposable working directory.
            env.pop('HOME', None)
            for key in ['XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR',
                        'CLAUDE_CONFIG_DIR', 'PI_CODING_AGENT_DIR', 'COMMANDCODE_HOME', 'HERMES_HOME',
                        'ANTIGRAVITY_HOME', 'CLINE_DIR', 'CLINE_DATA_DIR', 'CLINE_STORAGE_DIR',
                        'CLINE_SESSION_DATA_DIR', 'CLINE_DB_DATA_DIR']:
                env[key] = str(root / key.lower())
                Path(env[key]).mkdir(mode=0o700)
            env.update(WEBKIT_DISABLE_COMPOSITING_MODE='1', LIBGL_ALWAYS_SOFTWARE='1', NO_AT_BRIDGE='1', GIO_USE_VFS='local', GTK_USE_PORTAL='0')
        else:
            for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS']:
                env.pop(key, None)
        if args.hermes_package_dirs:
            packages = Path(env['HERMES_HOME']) / '.venv/lib/python3.12/site-packages'
            packages.mkdir(parents=True)
            for number in range(args.hermes_package_dirs):
                (packages / f'package-{number}').mkdir()
        total_bytes = 0
        for number in range(args.files):
            with (source_dir / f'{number}.jsonl').open('w') as stream:
                stream.write(json.dumps({'type': 'session_meta', 'payload': {'id': f'idle-{number}', 'cwd': '/synthetic/project'}}) + '\n')
                for entry in range(args.entries):
                    stream.write(json.dumps({'type': 'event_msg', 'payload': {'type': 'user_message', 'message': f'{entry}: ' + 'x' * args.text_bytes}}) + '\n')
            total_bytes += (source_dir / f'{number}.jsonl').stat().st_size
        report['sourceBytes'] = total_bytes
        database = root / 'library/sessions.sqlite'
        expected = args.files * (args.entries + 1)
        with (root / 'stdout').open('w') as stdout, (root / 'stderr').open('w') as stderr:
            start = time.perf_counter()
            desktop = wm = None
            if args.gui:
                spec = importlib.util.spec_from_file_location('hession_native_smoke', Path(__file__).resolve().parents[1] / '.github/scripts/native-instance-smoke.py')
                native = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(native)
                desktop = native.Desktop()
                wm_env = dict(env)
                if os.environ.get('HESSION_TEST_WM_LD_LIBRARY_PATH'):
                    wm_env['LD_LIBRARY_PATH'] = os.environ['HESSION_TEST_WM_LD_LIBRARY_PATH']
                if os.environ.get('HESSION_TEST_WM_XDG_DATA_DIRS'):
                    wm_env['XDG_DATA_DIRS'] = os.environ['HESSION_TEST_WM_XDG_DATA_DIRS']
                config = root / 'openbox.xml'
                config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><focus><focusNew>yes</focusNew><followMouse>no</followMouse></focus></openbox_config>')
                wm = subprocess.Popen([os.environ.get('HESSION_TEST_WM', 'openbox'), '--sm-disable', '--config-file', str(config)], cwd=root, env=wm_env, stdout=stderr, stderr=stderr)
                native.until(lambda: desktop.property(desktop.root, '_NET_SUPPORTING_WM_CHECK'), 'Private window manager did not start')
                assert wm.poll() is None, 'Private window manager exited'
            command = [binary] if args.gui else [binary, 'index', '--watch', '--agent', 'codex', '--json']
            process = subprocess.Popen(command, cwd=root, env=env, stdout=stdout, stderr=stderr)
            try:
                def count():
                    assert process.poll() is None, (root / 'stderr').read_text()
                    if not database.is_file():
                        return -1
                    try:
                        with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as conn:
                            return conn.execute('SELECT count(*) FROM entry').fetchone()[0]
                    except sqlite3.OperationalError:
                        return -1

                def wait_for_count(target):
                    deadline = time.monotonic() + 300
                    while True:
                        current = count()
                        if current == target:
                            return
                        if args.gui and target == expected and current <= 0:
                            assert time.monotonic() < deadline - 270, 'GUI imported no histories in 30s; build with make build (embedded production assets), not bare cargo build'
                        assert time.monotonic() < deadline, f'Observer did not reach {target} records'
                        time.sleep(0.02)
                wait_for_count(expected)
                report['startupMs'] = (time.perf_counter() - start) * 1000
                # Wait until imports and the final frontend refresh have settled.
                time.sleep(5)
                if desktop:
                    assert wm.poll() is None, 'Private window manager exited before Hession appeared'
                    window = native.until(lambda: desktop.windows(process.pid), 'Hession window did not appear')[0]
                    desktop.iconify(window)
                    time.sleep(2)
                initial = tree_counters(process.pid)
                start = time.perf_counter()
                time.sleep(args.idle_seconds)
                report['idle'] = tree_delta(initial, tree_counters(process.pid), time.perf_counter() - start, process.pid)
                assert count() == expected
                hot_file = source_dir / '0.jsonl'
                cold_file = source_dir / '1.jsonl' if args.files > 1 else hot_file
                cold_hash = hashlib.sha256(cold_file.read_bytes()).hexdigest()
                report['appends'] = []
                for sample in range(args.samples):
                    initial = tree_counters(process.pid)
                    start = time.perf_counter()
                    with hot_file.open('a') as stream:
                        stream.write(json.dumps({'type': 'event_msg', 'payload': {'type': 'user_message', 'message': f'Controlled append {sample}'}}) + '\n')
                        stream.flush()
                        os.fsync(stream.fileno())
                    wait_for_count(expected + sample + 1)
                    latency_ms = (time.perf_counter() - start) * 1000
                    # Include post-commit work on neighboring files in the measurement.
                    time.sleep(2)
                    measurement = tree_delta(initial, tree_counters(process.pid), time.perf_counter() - start, process.pid)
                    measurement['writeToCommitMs'] = latency_ms
                    report['appends'].append(measurement)
                if args.files > 1:
                    assert hashlib.sha256(cold_file.read_bytes()).hexdigest() == cold_hash
                with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as conn:
                    assert conn.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
                report['integrity'] = 'ok'
            finally:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                    raise AssertionError('Observer did not shut down promptly')
                finally:
                    if desktop:
                        desktop.x.XCloseDisplay(desktop.display)
                    if wm:
                        wm.terminate()
                        wm.wait(timeout=5)
                    if args.gui:
                        stderr.flush()
                        diagnostic = Path(args.output).with_suffix('.stderr.log')
                        diagnostic.parent.mkdir(parents=True, exist_ok=True)
                        diagnostic.write_text((root / 'stderr').read_text())
            assert process.returncode in (0, 143, -signal.SIGTERM), (process.returncode, (root / 'stderr').read_text())
    if args.baseline:
        baseline = json.loads(Path(args.baseline).read_text())
        for key in ['files', 'entries', 'text_bytes', 'idle_seconds', 'samples', 'gui', 'hermes_package_dirs']:
            assert baseline['parameters'].get(key, False) == report['parameters'].get(key, False), f'Baseline parameter mismatch: {key}'
        def factor(previous, current):
            return previous / current if current else None
        report['improvement'] = {
            'observerIdleCpu': factor(baseline['idle']['observer']['cpuMs'], report['idle']['observer']['cpuMs']),
            'observerIdleWakeups': factor(baseline['idle']['observer']['contextSwitches'], report['idle']['observer']['contextSwitches']),
            'appendReadBytes': factor(statistics.median(row['readBytes'] for row in baseline['appends']), statistics.median(row['readBytes'] for row in report['appends'])),
            'appendCpu': factor(statistics.median(row['cpuMs'] for row in baseline['appends']), statistics.median(row['cpuMs'] for row in report['appends'])),
            'idleRss': factor(baseline['idle']['rssKiB'], report['idle']['rssKiB']),
        }
    if args.empty_gui_report:
        control = json.loads(Path(args.empty_gui_report).read_text())
        assert args.gui and control['parameters']['gui'], 'Memory controls require GUI measurements'
        assert control['binarySha256'] == report['binarySha256'], 'Memory control must use the same production binary'
        assert control['parameters']['files'] == 1 and control['parameters']['entries'] == 1 and control['parameters']['text_bytes'] == 1
        def native_row(measurement):
            return next(row for row in measurement['processes'].values() if row['observer'])
        report['memoryOverhead'] = {
            'nativeRssKiB': native_row(report['idle'])['rssKiB'] - native_row(control['idle'])['rssKiB'],
            'totalPssKiB': report['idle']['pssKiB'] - control['idle']['pssKiB'],
            'controlNativeRssKiB': native_row(control['idle'])['rssKiB'],
            'controlTotalPssKiB': control['idle']['pssKiB'],
        }
    Path(args.output).parent.mkdir(parents=True, exist_ok=True)
    Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    if args.max_native_overhead_mib:
        assert report['memoryOverhead']['nativeRssKiB'] <= args.max_native_overhead_mib * 1024, 'Native library memory exceeds the configured overhead budget'
    if args.max_gui_overhead_mib:
        assert report['memoryOverhead']['totalPssKiB'] <= args.max_gui_overhead_mib * 1024, 'GUI library memory exceeds the configured overhead budget'
    if args.max_idle_rss_mib:
        assert report['idle']['rssKiB'] <= args.max_idle_rss_mib * 1024, 'Idle RSS exceeds the configured memory budget'
    if args.max_idle_pss_mib:
        assert report['idle']['pssKiB'] <= args.max_idle_pss_mib * 1024, 'Idle PSS exceeds the configured memory budget'
    if args.require_improvement:
        for metric in ['observerIdleCpu', 'observerIdleWakeups', 'appendReadBytes']:
            value = report['improvement'][metric]
            assert value is None or value >= args.require_improvement, f'{metric}: {value:.1f}x improvement is below {args.require_improvement}x'
        assert not report['idle']['newProcesses'] and not report['idle']['retiredProcesses'], 'GUI process churn requires a new settled measurement'
        assert max(report['idle']['oneCoreCpuPercent'], report['idle']['clockOneCoreCpuPercent']) <= 0.07, 'Background CPU exceeds the reported 3.5% / 50 target'


if __name__ == '__main__':
    main()
