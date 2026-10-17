"""Record native terminal keys and mouse reports without starting an agent."""
import json
import os
import pathlib
import select
import sys
import termios
import tty

path = pathlib.Path(sys.argv[1])
alternate = sys.argv[2] == "alternate"
original = termios.tcgetattr(0)
received = bytearray()


def save(done=False):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps({"ready": True, "done": done, "hex": received.hex()}))
    temporary.replace(path)


try:
    tty.setraw(0)
    if alternate:
        os.write(1, b"\x1b[?1049h")
    os.write(1, b"\x1b[?1000h\x1b[?1006h")
    save()
    while b"\r" not in received:
        if not select.select([0], [], [], 20)[0]:
            break
        chunk = os.read(0, 1024)
        if not chunk:
            break
        received.extend(chunk)
        save()
    save(done=b"\r" in received)
finally:
    os.write(1, b"\x1b[?1000l\x1b[?1006l")
    if alternate:
        os.write(1, b"\x1b[?1049l")
    termios.tcsetattr(0, termios.TCSADRAIN, original)
