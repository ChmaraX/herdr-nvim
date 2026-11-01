"""Write a pi session log the way pi does for an agent in CWD: under
<PI_CODING_AGENT_DIR>/sessions/--<cwd with / as ->--/. The agent read
package.json, then edited src/notes.md, so the picker must list notes.md
first, then package.json. Usage: fake-pi-session.py <pi dir> <cwd>"""
import datetime
import json
import os
import sys

root, cwd = sys.argv[1], sys.argv[2]
folder = os.path.join(root, "sessions", "--" + cwd.lstrip("/").replace("/", "-").replace(":", "-") + "--")
os.makedirs(folder, exist_ok=True)
now = datetime.datetime.now(datetime.UTC)


def ts(seconds):
    return (now + datetime.timedelta(seconds=seconds)).strftime("%Y-%m-%dT%H:%M:%S.000Z")


def call(i, name, args, seconds):
    return {"type": "message", "id": f"m{i}", "parentId": None, "timestamp": ts(seconds),
            "message": {"role": "assistant", "content": [{"type": "toolCall", "id": f"c{i}", "name": name, "arguments": args}]}}


lines = [{"type": "session", "version": 3, "id": "e2e", "timestamp": ts(0), "cwd": cwd},
         call(1, "read", {"path": f"{cwd}/package.json"}, 1),
         call(2, "edit", {"path": "src/notes.md", "oldText": "three", "newText": "three"}, 2)]
with open(os.path.join(folder, now.strftime("%Y-%m-%dT%H-%M-%S-000Z") + "_e2e.jsonl"), "w") as f:
    f.write("".join(json.dumps(line) + "\n" for line in lines))
