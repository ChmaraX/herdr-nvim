"""Which panes look focused in a `herdr-app shot`: prints how many (accent,
i.e. blue, border edges crossing the middle row, halved: each
focused pane or panel has a left and a right one) and where the first one
starts as a share of the width. Usage: highlight.py <png>"""
import sys

from PIL import Image

im = Image.open(sys.argv[1]).convert("RGB")
w, h = im.size
y = h // 2
edges, inside = [], False
for x in range(w):
    r, g, b = im.getpixel((x, y))
    accent = b > 150 and b - r > 50 and b - g > 25  # light and dark themes
    if accent and not inside:
        edges.append(x)
    inside = accent
print(len(edges) // 2, round(edges[0] / w, 2) if edges else 0)
