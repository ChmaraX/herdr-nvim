"""Press keys in one process only: `keys.py PID SPEC...`, SPEC = macOS virtual
keycode, optionally `:cmd,shift,ctrl,alt`, or a key name (`enter`, `esc`,
`backslash`), or `text:<letters and spaces>` (US layout). The events go to that process
(CGEventPostToPid), not to the focused app, so the person's own window never
receives them and the pointer never moves. Needs pyobjc (Quartz)."""
import sys
import time

import Quartz

FLAGS = {
    "cmd": Quartz.kCGEventFlagMaskCommand,
    "shift": Quartz.kCGEventFlagMaskShift,
    "ctrl": Quartz.kCGEventFlagMaskControl,
    "alt": Quartz.kCGEventFlagMaskAlternate,
}

LETTERS = dict(zip("asdfhgzxcv bqweryt123465=97-80]ou[ip lj'k;\\,/nm.", range(51)))
LETTERS[" "] = 49
LETTERS["`"] = 50
SHIFTED = {"~": "`", "?": "/", "_": "-", ":": ";"}
NAMES = {"enter": 36, "esc": 53, "backslash": 42, "tab": 48, "backspace": 51}


def specs(argv):
    for spec in argv:
        if spec.startswith("text:"):
            for ch in spec[5:]:
                if ch in SHIFTED:
                    yield f"{LETTERS[SHIFTED[ch]]}:shift"
                elif ch.isupper():
                    yield f"{LETTERS[ch.lower()]}:shift"
                else:
                    yield str(LETTERS[ch])
        elif spec.split(":")[0] in NAMES:
            name, _, mods = spec.partition(":")
            yield f"{NAMES[name]}:{mods}"
        else:
            yield spec


pid = int(sys.argv[1])
for spec in specs(sys.argv[2:]):
    code, _, mods = spec.partition(":")
    flags = 0
    for mod in filter(None, mods.split(",")):
        flags |= FLAGS[mod]
    for down in (True, False):
        event = Quartz.CGEventCreateKeyboardEvent(None, int(code), down)
        Quartz.CGEventSetFlags(event, flags)
        Quartz.CGEventPostToPid(pid, event)
        time.sleep(0.03)
    time.sleep(0.12)
