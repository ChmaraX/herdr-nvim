"""What the e2e run checks after each step, as one JSON line: the side panel
(shown, which stack child, width) from the layout file and its saved clicks,
and per tab its terminals, which one is the herdr-nvim pane, and the focused
pane. Reads the private instance named by HERDR_APP_HOME / HC_SESSION."""
import json
import os
import subprocess
import sys

home = os.environ["HERDR_APP_HOME"]
app = sys.argv[1]


def find(node, ident):
    if node.get("id") == ident:
        return node
    for child in node.get("children", []):
        found = find(child, ident)
        if found:
            return found
    return None


def run(*argv):
    return json.loads(subprocess.run(argv, capture_output=True, text=True).stdout or "null")


status = run(app, "status")
name = status["layout"]
layout = json.load(open(f"{home}/layouts/{name}.json"))
try:
    clicks = json.load(open(f"{home}/layouts/.state/{name}.json")).get("nodes", {})
except FileNotFoundError:
    clicks = {}
panel, body = find(layout, "panel") or {}, find(layout, "panel-body") or {}
hidden = clicks.get("panel", {}).get("hidden", panel.get("hidden", False))
active = clicks.get("panel-body", {}).get("active", body.get("active"))
width = (clicks.get("panel", {}).get("size") or panel.get("size") or {}).get("px")
tabs = {}
for space in run(app, "ls")["spaces"]:
    for tab in space["tabs"]:
        nvim = None
        for t in tab["terminals"]:
            labels = run("hc", "get", t["terminal"], "--json").get("labels", {})
            if "herdr-nvim.sidebar" in labels:
                nvim = t["terminal"]
        tabs[tab["id"]] = {"name": tab["name"], "active": tab["active"], "nvim": nvim,
                           "terminals": [t["terminal"] for t in tab["terminals"]]}
print(json.dumps({"panel": "hidden" if hidden else active, "width": width, "tab": status["tab"],
                  "focused_pane": status["focused_pane"], "errors": status["errors"], "tabs": tabs}))
