"""How many panes look focused in a `herdr-app shot`: the number of accent
(saturated blue) border edges crossing the middle row, halved. Each focused
pane or panel has a left and a right accent edge. Usage: highlight.py <png>"""
import sys

from PIL import Image

im = Image.open(sys.argv[1]).convert("RGB")
w, h = im.size
y = h // 2
edges, inside = 0, False
for x in range(w):
    r, g, b = im.getpixel((x, y))
    accent = b > 150 and b - r > 80 and b - g > 40
    if accent and not inside:
        edges += 1
    inside = accent
print(edges // 2)
