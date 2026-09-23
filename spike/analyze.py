# SPIKE ONLY — analyze a screen capture of our window.
# Finds the test page's orange background (#ff5722) bbox = where the child
# webview ACTUALLY reached the screen, and reports it against the expected
# rect (client origin + logical div rect * scale). Also white-fraction of the
# expected region (white-on-load detection, tauri#10011).
# Usage:
#   python spike/analyze.py <png> --client X Y --outer L T --rect X Y W H --scale S
#   python spike/analyze.py <png> --client X Y --outer L T --scale S
#        (second form: rect defaults to the child's creation rect 660 60 560 360)
import argparse, json, sys
from PIL import Image

p = argparse.ArgumentParser()
p.add_argument("png")
p.add_argument("--client", nargs=2, type=float, required=True)   # client origin, screen px
p.add_argument("--outer", nargs=2, type=float, required=True)    # window outer origin, screen px
p.add_argument("--rect", nargs=4, type=float, default=[660, 60, 560, 360])  # logical
p.add_argument("--scale", type=float, required=True)
a = p.parse_args()

img = Image.open(a.png).convert("RGB")
W, H = img.size
px = img.load()

cx, cy = a.client
ox, oy = a.outer
rx, ry, rw, rh = a.rect
s = a.scale
# expected child rect in IMAGE coordinates (image origin = window outer origin)
ex = (cx - ox) + rx * s
ey = (cy - oy) + ry * s
ew, eh = rw * s, rh * s
ex0, ey0, ex1, ey1 = int(round(ex)), int(round(ey)), int(round(ex + ew)), int(round(ey + eh))
ex0c, ey0c = max(ex0, 0), max(ey0, 0)
ex1c, ey1c = min(ex1, W), min(ey1, H)

orange_min_x, orange_min_y, orange_max_x, orange_max_y = W, H, -1, -1
orange_count = 0
white_count = 0
total = 0
for y in range(ey0c, ey1c):
    for x in range(ex0c, ex1c):
        r, g, b = px[x, y]
        total += 1
        if r > 180 and 40 <= g <= 170 and b < 120:  # #ff5722-ish (page bg)
            orange_count += 1
            if x < orange_min_x: orange_min_x = x
            if y < orange_min_y: orange_min_y = y
            if x > orange_max_x: orange_max_x = x
            if y > orange_max_y: orange_max_y = y
        if r >= 245 and g >= 245 and b >= 245:
            white_count += 1

result = {
    "png": a.png,
    "image": [W, H],
    "expected": [ex0, ey0, int(round(ex + ew)), int(round(ey + eh))],
    "white_frac_expected": round(white_count / total, 4) if total else None,
    "orange_frac_expected": round(orange_count / total, 4) if total else None,
}
if orange_count > 100:
    actual = [orange_min_x, orange_min_y, orange_max_x + 1, orange_max_y + 1]
    result["actual_orange_bbox"] = actual
    result["delta_xywh"] = [
        actual[0] - ex0, actual[1] - ey0,
        (actual[2] - actual[0]) - (ex1 - ex0),
        (actual[3] - actual[1]) - (ey1 - ey0),
    ]
    result["delta_logical_xywh"] = [round(d / s, 2) for d in result["delta_xywh"]]
else:
    result["actual_orange_bbox"] = None
    result["note"] = "no orange bbox found in expected region (white/blank/other)"
print(json.dumps(result))
