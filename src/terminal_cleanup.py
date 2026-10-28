"""One-shot cleanup of Araseo-owned Linux terminals; never a background poller.

New sessions carry a per-terminal ownership token. Legacy sessions must match
Araseo's exact script invocation. Windows confirms dead owners before recovery.
All signals use pidfds after checking Linux process start times.
"""
import json
import os
import pathlib
import re
import select
import signal
import sys
import time

KEY = b"ARASEO_TERMINAL_SESSION="
TOKEN = re.compile(r"^([1-9][0-9]*)\.([1-9][0-9]*)\.([1-9][0-9]*)$")
LEGACY = re.compile(rb"^stty rows 24 cols 80; tty > (/tmp/araseo-pty-([1-9][0-9]*)-([1-9][0-9]*)); ")


def snapshot():
    result = {}
    for entry in pathlib.Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if entry.stat().st_uid != os.getuid():
                continue
            stat = (entry / 'stat').read_text().rsplit(') ', 1)[1].split()
            if stat[0] == 'Z':
                continue
            env = (entry / 'environ').read_bytes().split(b'\0')
            token = next((item[len(KEY):].decode('ascii') for item in env if item.startswith(KEY)), None)
            argv = (entry / 'cmdline').read_bytes().split(b'\0')
            legacy = None
            if len(argv) == 5 and argv[1] == b'-qfec' and argv[3] == b'/dev/null':
                if os.path.basename(os.readlink(entry / 'exe')) == 'script':
                    legacy = LEGACY.match(argv[2])
            result[int(entry.name)] = dict(start=int(stat[19]), parent=int(stat[1]), token=token, legacy=legacy)
        except (OSError, UnicodeError, IndexError, ValueError):
            continue
    return result


def candidates():
    groups = {}
    for pid, item in snapshot().items():
        token = item['token']
        match = TOKEN.fullmatch(token or '')
        if match:
            groups.setdefault(token, dict(token=token, pid=pid, start=item['start'], owner_pid=int(match[1]), owner_created=int(match[2])))
        elif item['legacy']:
            match = item['legacy']
            groups[pid] = dict(token=None, pid=pid, start=item['start'], owner_pid=int(match[2]), owner_created=None)
    return list(groups.values())


def cleanup(targets):
    # Unsupported kernels/Python versions leave the PTY hangup backstop in place.
    if not hasattr(os, 'pidfd_open') or not hasattr(signal, 'pidfd_send_signal'):
        raise RuntimeError('pidfd cleanup requires Python 3.9+ and Linux 5.3+')
    tokens = {item['token'] for item in targets if TOKEN.fullmatch(item.get('token') or '')}
    legacy = {item['pid']: item['start'] for item in targets if item.get('token') is None}
    handles = {}  # (pid, start) -> pidfd; identity remains pinned until cleanup ends.
    owned = set()
    tty_paths = {f"/tmp/araseo-pty-{match[1]}-{match[3]}-{match[2]}" for token in tokens for match in [TOKEN.fullmatch(token)]}

    def discover():
        processes = snapshot()
        selected = {pid for pid, item in processes.items() if item['token'] in tokens or (pid in legacy and item['start'] == legacy[pid] and item['legacy'])}
        # Include children that intentionally replaced their environment while
        # their owned parent is still identifiable; don't target unrelated peers.
        selected.update(pid for pid, item in processes.items() if (pid, item['start']) in owned)
        while True:
            children = {pid for pid, item in processes.items() if item['parent'] in selected}
            if children <= selected:
                break
            selected.update(children)
        for pid in selected:
            if processes[pid]['legacy']:
                tty_paths.add(os.fsdecode(processes[pid]['legacy'][1]))
            if pid == os.getpid():
                continue
            item = processes[pid]
            key = (pid, item['start'])
            if key in handles:
                continue
            fd = None
            try:
                fd = os.pidfd_open(pid)
                current = pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()
                if int(current[19]) != item['start']:
                    os.close(fd)
                    continue
                handles[key] = fd
                owned.add(key)
            except (ProcessLookupError, FileNotFoundError):
                if fd is not None:
                    os.close(fd)
                continue
            except OSError:
                if fd is not None:
                    os.close(fd)
                raise

    def alive(fd):
        poll = select.poll()
        poll.register(fd, select.POLLIN)
        return not poll.poll(0)

    try:
        for sig, grace in [(signal.SIGHUP, 2.0), (signal.SIGTERM, 1.0), (signal.SIGKILL, 0.5)]:
            deadline = time.monotonic() + grace
            sent = set()
            while True:
                discover()
                active = [(key, fd) for key, fd in handles.items() if alive(fd)]
                for key, fd in active:
                    if key not in sent:
                        try:
                            signal.pidfd_send_signal(fd, sig)
                            # A stopped foreground process must be able to handle HUP.
                            if sig != signal.SIGKILL:
                                signal.pidfd_send_signal(fd, signal.SIGCONT)
                        except (ProcessLookupError, PermissionError):
                            pass
                        sent.add(key)
                if not active:
                    return
                if time.monotonic() >= deadline:
                    break
                time.sleep(0.05)
    finally:
        for fd in handles.values():
            os.close(fd)
        for name in tty_paths:
            path = pathlib.Path(name)
            try:
                if path.lstat().st_uid == os.getuid() and not path.is_dir():
                    path.unlink()
            except FileNotFoundError:
                pass


def main():
    mode = sys.argv[1]
    if mode == 'list':
        print(json.dumps(candidates()))
    elif mode == 'close':
        token = sys.argv[2]
        if not TOKEN.fullmatch(token):
            raise ValueError('invalid terminal token')
        cleanup([dict(token=token)])
    elif mode == 'recover':
        cleanup(json.loads(sys.argv[2]))
    else:
        raise ValueError('invalid cleanup mode')


if __name__ == '__main__':
    main()
