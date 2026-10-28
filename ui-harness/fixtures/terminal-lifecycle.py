"""Idle fake agent with a detached child that ignores HUP and TERM."""
import json
import os
import pathlib
import signal
import subprocess
import sys
import time

root = pathlib.Path(sys.argv[1])
name = sys.argv[2]
child = subprocess.Popen([
    sys.executable, '-c',
    'import signal,time; signal.signal(signal.SIGHUP,signal.SIG_IGN); signal.signal(signal.SIGTERM,signal.SIG_IGN); print("ready",flush=True); time.sleep(300)'
], start_new_session=True, stdout=subprocess.PIPE)
assert child.stdout.readline().strip() == b'ready'

def stop(sig, frame):
    (root / (name + '.hup')).write_text('saved on hangup')
    raise SystemExit

signal.signal(signal.SIGHUP, stop)
(root / (name + '.json')).write_text(json.dumps({
    'pid': os.getpid(), 'child': child.pid,
    'start': int(pathlib.Path(f'/proc/{os.getpid()}/stat').read_text().rsplit(') ', 1)[1].split()[19]),
    'child_start': int(pathlib.Path(f'/proc/{child.pid}/stat').read_text().rsplit(') ', 1)[1].split()[19]),
    'token': os.environ.get('ARASEO_TERMINAL_SESSION'), 'tty': os.ttyname(0)
}))
while True:
    time.sleep(1)
