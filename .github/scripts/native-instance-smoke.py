"""Exercise the installed app with synthetic data on an explicitly isolated desktop.

Linux: run inside ONE private dbus-run-session and Xvfb, with Openbox on PATH.
macOS: use a disposable CI login session with no existing Hession instance.
"""
import ctypes
import ctypes.util
import json
import os
from pathlib import Path
import signal
import shlex
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time


def until(check, description, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError(description)


class Desktop:
    """Xlib checks stay on the caller's disposable Xvfb display."""
    def __init__(self):
        self.x = ctypes.CDLL(ctypes.util.find_library('X11'))
        definitions = {
            'XOpenDisplay': ([ctypes.c_char_p], ctypes.c_void_p),
            'XDefaultRootWindow': ([ctypes.c_void_p], ctypes.c_ulong),
            'XInternAtom': ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int], ctypes.c_ulong),
            'XQueryTree': ([ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)), ctypes.POINTER(ctypes.c_uint)], ctypes.c_int),
            'XGetWindowProperty': ([ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_long, ctypes.c_long, ctypes.c_int, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.POINTER(ctypes.c_ubyte))], ctypes.c_int),
            'XFree': ([ctypes.c_void_p], ctypes.c_int),
            'XIconifyWindow': ([ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int], ctypes.c_int),
            'XFlush': ([ctypes.c_void_p], ctypes.c_int),
            'XCreateSimpleWindow': ([ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_ulong], ctypes.c_ulong),
            'XMapWindow': ([ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
            'XDestroyWindow': ([ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
            'XGetInputFocus': ([ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int)], ctypes.c_int),
            'XSetInputFocus': ([ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong], ctypes.c_int),
            'XCloseDisplay': ([ctypes.c_void_p], ctypes.c_int),
        }
        for name, (args, result) in definitions.items():
            method = getattr(self.x, name)
            method.argtypes, method.restype = args, result
        # A window can disappear between enumeration and inspection on exit.
        error_callback = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
        self.ignore_destroyed_window = error_callback(lambda _display, _event: 0)
        self.x.XSetErrorHandler.argtypes = [error_callback]
        self.x.XSetErrorHandler(self.ignore_destroyed_window)
        self.display = self.x.XOpenDisplay(None)
        assert self.display, 'Xvfb display unavailable'
        self.root = self.x.XDefaultRootWindow(self.display)

    def property(self, window, name):
        atom = self.x.XInternAtom(self.display, name.encode(), 0)
        kind, fmt, size, remaining = ctypes.c_ulong(), ctypes.c_int(), ctypes.c_ulong(), ctypes.c_ulong()
        data = ctypes.POINTER(ctypes.c_ubyte)()
        self.x.XGetWindowProperty(self.display, window, atom, 0, 32, 0, 0, ctypes.byref(kind), ctypes.byref(fmt), ctypes.byref(size), ctypes.byref(remaining), ctypes.byref(data))
        try:
            return list(ctypes.cast(data, ctypes.POINTER(ctypes.c_ulong))[:size.value]) if fmt.value == 32 and data else []
        finally:
            if data:
                self.x.XFree(data)

    def tree(self, window):
        root, parent, count = ctypes.c_ulong(), ctypes.c_ulong(), ctypes.c_uint()
        children = ctypes.POINTER(ctypes.c_ulong)()
        self.x.XQueryTree(self.display, window, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count))
        try:
            return list(children[:count.value])
        finally:
            if children:
                self.x.XFree(children)

    def windows(self, pid):
        pending, found = [self.root], []
        while pending:
            window = pending.pop()
            if self.property(window, '_NET_WM_PID') == [pid] and self.property(window, 'WM_STATE'):
                found.append(window)
            pending.extend(self.tree(window))
        return found

    def focused(self, window):
        focus, revert = ctypes.c_ulong(), ctypes.c_int()
        self.x.XGetInputFocus(self.display, ctypes.byref(focus), ctypes.byref(revert))
        pending = [window]
        while pending:
            child = pending.pop()
            if child == focus.value:
                return True
            pending.extend(self.tree(child))
        return False

    def wait_for_window_manager(self):
        # _NET_SUPPORTING_WM_CHECK is advertised during WM initialization.
        # A managed probe proves that its event loop can actually map clients.
        probe = self.x.XCreateSimpleWindow(self.display, self.root, 0, 0, 32, 32, 0, 0, 0)
        assert probe, 'Cannot create window-manager readiness probe'
        try:
            self.x.XMapWindow(self.display, probe)
            self.x.XFlush(self.display)
            until(lambda: self.property(probe, 'WM_STATE')[:1] == [1], 'Test window manager did not manage its readiness probe', timeout=60)
        finally:
            self.x.XDestroyWindow(self.display, probe)
            self.x.XFlush(self.display)

    def iconify(self, window):
        assert self.x.XIconifyWindow(self.display, window, 0)
        self.x.XFlush(self.display)
        until(lambda: self.property(window, 'WM_STATE')[:1] == [3], 'Window did not minimize')

    def unfocus(self, window):
        self.x.XSetInputFocus(self.display, self.root, 1, 0)
        self.x.XFlush(self.display)
        until(lambda: not self.focused(window), 'Window did not lose focus')

    def close_window(self, window):
        # XClientMessageEvent occupies an XEvent union of 24 native longs.
        class Data(ctypes.Union):
            _fields_ = [('l', ctypes.c_long * 5), ('b', ctypes.c_char * 20)]
        class Message(ctypes.Structure):
            _fields_ = [('type', ctypes.c_int), ('serial', ctypes.c_ulong), ('send_event', ctypes.c_int), ('display', ctypes.c_void_p), ('window', ctypes.c_ulong), ('message_type', ctypes.c_ulong), ('format', ctypes.c_int), ('data', Data)]
        event = (ctypes.c_long * 24)()
        message = ctypes.cast(event, ctypes.POINTER(Message)).contents
        message.type, message.display, message.window = 33, self.display, window
        message.message_type = self.x.XInternAtom(self.display, b'WM_PROTOCOLS', 0)
        message.format = 32
        message.data.l[0] = self.x.XInternAtom(self.display, b'WM_DELETE_WINDOW', 0)
        self.x.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
        assert self.x.XSendEvent(self.display, window, 0, 0, event)
        self.x.XFlush(self.display)


def main():
    assert os.environ.get('HESSION_NATIVE_SMOKE_ISOLATED') == '1', 'Requires an isolated desktop, never your personal session'
    binary = str(Path(sys.argv[1]).resolve(strict=True))
    output = Path(sys.argv[2]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='hession-instance-'))
    env = dict(os.environ)
    env.pop('HOME', None)
    env.pop('WAYLAND_DISPLAY', None)
    for key in ['HESSION_DATA_DIR', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'PI_CODING_AGENT_DIR', 'COMMANDCODE_HOME', 'HERMES_HOME', 'ANTIGRAVITY_HOME', 'CLINE_DIR', 'CLINE_DATA_DIR', 'CLINE_STORAGE_DIR', 'CLINE_SESSION_DATA_DIR', 'CLINE_DB_DATA_DIR']:
        env[key] = str(root / key.lower())
        Path(env[key]).mkdir(mode=0o700)
    env.update(WEBKIT_DISABLE_COMPOSITING_MODE='1', LIBGL_ALWAYS_SOFTWARE='1', NO_AT_BRIDGE='1', GIO_USE_VFS='local', GTK_USE_PORTAL='0')
    source = Path(env['CODEX_HOME']) / 'sessions'
    source.mkdir()
    source /= 'native.jsonl'
    source.write_text('\n'.join(json.dumps(row) for row in [
        {'type': 'session_meta', 'payload': {'id': 'native-instance', 'cwd': str(root / 'workspace')}},
        {'type': 'event_msg', 'payload': {'type': 'user_message', 'message': 'Synthetic native instance check.\n```javascript\nconst ready = true;\n```'}},
    ]) + '\n')
    original = source.read_bytes()
    processes = []
    desktop = None
    log = (output / 'instance-gui.log').open('w')

    def command(args, headless=False):
        command_env = dict(env)
        if headless:
            for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS']:
                command_env.pop(key, None)
        return subprocess.run([binary, *args], cwd=root, env=command_env, capture_output=True, text=True, timeout=20, check=True)

    def start():
        child = subprocess.Popen([binary], cwd=root, env=env, stdout=log, stderr=log, start_new_session=True)
        processes.append(child)
        if desktop:
            window = until(lambda: desktop.windows(child.pid), 'Primary window unavailable (see instance-gui.log)')[0]
            until(lambda: desktop.property(window, 'WM_STATE')[:1] == [1], 'Primary not mapped')
        else:
            time.sleep(8)
        assert child.poll() is None, 'Primary exited early'
        return child

    def second(primary):
        result = command([])
        assert result.returncode == 0
        assert primary.poll() is None, 'Primary process changed'
        if desktop:
            until(lambda: desktop.property(window, 'WM_STATE')[:1] == [1] and desktop.focused(window), 'Existing window was not restored and focused')
            assert desktop.windows(primary.pid) == [window], 'More than one main window'

    try:
        command(['index', '--once', '--agent', 'codex', '--json'], headless=True)
        if sys.platform.startswith('linux'):
            assert env.get('DBUS_SESSION_BUS_ADDRESS'), 'Requires a private session bus'
            config = root / 'openbox.xml'
            config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><focus><focusNew>yes</focusNew><followMouse>no</followMouse></focus></openbox_config>')
            wm_env = dict(env)
            # Local extracted test tools can supply their own library/theme roots.
            for key in ['LD_LIBRARY_PATH', 'XDG_DATA_DIRS']:
                if os.environ.get('HESSION_TEST_WM_' + key):
                    wm_env[key] = os.environ['HESSION_TEST_WM_' + key]
            wm_ready = root / 'window-manager-ready'
            startup = shlex.join([shutil.which('touch') or '/usr/bin/touch', str(wm_ready)])
            wm = subprocess.Popen([os.environ.get('HESSION_TEST_WM', 'openbox'), '--sm-disable', '--config-file', str(config), '--startup', startup], env=wm_env, cwd=root, stdout=log, stderr=log, start_new_session=True)
            processes.append(wm)
            desktop = Desktop()
            # Openbox publishes its owner property before installing event
            # handling; its startup hook runs after that initialization.
            until(wm_ready.is_file, 'Test window manager did not finish startup', timeout=60)
            until(lambda: desktop.property(desktop.root, '_NET_SUPPORTING_WM_CHECK'), 'Test window manager did not initialize')
            assert wm.poll() is None, 'Test window manager exited'
            desktop.wait_for_window_manager()
        primary = start()
        if desktop:
            window = desktop.windows(primary.pid)[0]
            desktop.unfocus(window)
        second(primary)
        alternate = root / 'another launcher'
        alternate.symlink_to(binary)
        other_library = root / 'unused-secondary-library'
        alternate_env = dict(env, HESSION_DATA_DIR=str(other_library))
        subprocess.run([str(alternate)], cwd=Path(env['CODEX_HOME']), env=alternate_env, capture_output=True, text=True, timeout=20, check=True)
        assert primary.poll() is None
        assert not (other_library / 'sessions.sqlite').exists(), 'Secondary initialized another library'
        if desktop:
            assert desktop.windows(primary.pid) == [window]
            # Repeat immediate minimize/relaunch to exercise activation while
            # GTK's cached window state can still lag the window manager.
            for _ in range(12):
                desktop.iconify(window)
                second(primary)
        for args in [['--version'], ['--help'], ['status', '--json'], ['sessions', 'list', '--json'], ['index', '--once', '--agent', 'codex', '--json']]:
            assert command(args, headless=True).stdout.strip()
            assert primary.poll() is None
        if desktop:
            desktop.close_window(window)
            assert primary.wait(timeout=15) == 0
        else:
            primary.terminate()
            primary.wait(timeout=15)
        primary = start()
        if desktop:
            window = desktop.windows(primary.pid)[0]
        second(primary)
        # Crash recovery must not leave stale instance ownership.
        primary.kill()
        primary.wait(timeout=15)
        primary = start()
        if desktop:
            window = desktop.windows(primary.pid)[0]
        second(primary)
        assert source.read_bytes() == original, 'Native source was changed'
        with sqlite3.connect((Path(env['HESSION_DATA_DIR']) / 'sessions.sqlite').as_uri() + '?mode=ro', uri=True) as database:
            assert database.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
            assert database.execute('SELECT count(*) FROM session').fetchone()[0] == 1
        if desktop and shutil.which('import'):
            time.sleep(3)  # Let the final restarted webview paint its synthetic session.
            subprocess.run(['import', '-display', env['DISPLAY'], '-window', 'root', str(output / 'instance-restored.png')], env=env, cwd=root, check=True, timeout=15)
        report = {'binary': binary, 'platform': sys.platform, 'secondLaunchExits': True, 'samePrimary': True, 'alternateLauncherAndLibrary': True, 'headlessWhileOpen': True, 'closeAndRelaunch': True, 'crashRecovery': True, 'sourceUnchanged': True, 'libraryIntegrity': True, 'minimizeAndFocus': bool(desktop)}
        (output / 'instance-results.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)
    except Exception:
        print('Owned process states:', [(process.pid, process.poll()) for process in processes], flush=True)
        if desktop:
            pending = [desktop.root]
            while pending:
                item = pending.pop()
                pid = desktop.property(item, '_NET_WM_PID')
                state = desktop.property(item, 'WM_STATE')
                if pid or state:
                    print('Owned display window:', item, 'pid:', pid, 'state:', state, flush=True)
                pending.extend(desktop.tree(item))
        if desktop and shutil.which('import'):
            subprocess.run(['import', '-display', env['DISPLAY'], '-window', 'root', str(output / 'instance-failure.png')], env=env, cwd=root, timeout=15, check=False)
        raise
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        if desktop:
            desktop.x.XCloseDisplay(desktop.display)
        log.close()


if __name__ == '__main__':
    main()
