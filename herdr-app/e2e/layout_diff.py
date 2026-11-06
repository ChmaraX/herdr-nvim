"""What changed between two layout files, node by node:

    layout_diff.py BEFORE.json AFTER.json [--expect JSON]

prints {"changed": {id: [fields]}, "added": [ids], "removed": [ids]} (a node's
fields without its children; the root's `cloned_from` is left out, so a clone
compares with its origin) and, with --expect, exits 1 unless it is exactly
that.

    layout_diff.py --same BEFORE.json AFTER.json

exits 0 only if both files hold the same JSON with the keys in the same
order (what the person sees in the file), whatever the whitespace."""
import json
import sys


def nodes(node, out):
    out[node.get("id")] = {k: v for k, v in node.items() if k != "children"}
    for child in node.get("children", []):
        nodes(child, out)
    return out


def load(path):
    with open(path) as f:
        return json.load(f)


args = sys.argv[1:]
if args[0] == "--same":
    a, b = (json.dumps(load(p)) for p in args[1:3])
    if a != b:
        print(f"{args[2]} differs from {args[1]}")
        sys.exit(1)
    print(f"{args[2]}: same content and key order as before")
    sys.exit(0)

before, after = nodes(load(args[0]), {}), nodes(load(args[1]), {})
for side in (before, after):
    side.get("root", {}).pop("cloned_from", None)
diff = {
    "changed": {k: sorted(f for f in set(before[k]) | set(after[k]) if before[k].get(f) != after[k].get(f))
                for k in sorted(set(before) & set(after)) if before[k] != after[k]},
    "added": sorted(set(after) - set(before)),
    "removed": sorted(set(before) - set(after)),
}
print(json.dumps(diff))
if "--expect" in args and diff != json.loads(args[args.index("--expect") + 1]):
    sys.exit(1)
