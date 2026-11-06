"""A stand-in coding agent for the e2e run: reports its status to herdr-core
(OSC 7501, app=pi), prints file paths like an agent's summary does, and keeps
an input box: pasted text (bracketed paste) lands in it, Enter submits it.
Every state change is printed, so `hc read` shows what arrived and how."""
import os, sys, termios, tty

out = sys.stdout
def w(text):
    out.write(text); out.flush()

def report(state):
    w(f"\033]7501;state={state}:app=pi\033\\")

fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
tty.setraw(fd)
w("\033[?2004h")  # bracketed paste, as every coding agent asks for
report("idle")
cwd = os.getcwd()
w(f"dummy agent in {cwd}\r\n")
w("I edited src/greet.js:5 and read src/notes.md:3\r\n")
w(f"See also {cwd}/package.json\r\n")

box, pasting, pending = "", False, b""
try:
    while True:
        b = os.read(fd, 4096)
        if not b:
            break
        pending += b
        while pending:
            if pending.startswith(b"\033[200~"):
                pasting, pending = True, pending[6:]; continue
            if pending.startswith(b"\033[201~"):
                pasting, pending = False, pending[6:]
                w("INPUT: " + box.replace("\n", "\\n") + "\r\n"); continue
            if pending[:1] == b"\033" and len(pending) < 6:
                break  # wait for the rest of an escape sequence
            ch, pending = pending[:1], pending[1:]
            if ch == b"\x03":
                raise KeyboardInterrupt
            if ch == b"\r" and not pasting:
                w("SUBMITTED: " + box.replace("\n", "\\n") + "\r\n"); box = ""
            else:
                box += ch.decode("utf-8", "replace").replace("\r", "\n")
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
