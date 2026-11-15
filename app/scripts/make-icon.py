"""Generates assets/icon.png (256px, used as window icon) and assets/icon.ico."""
from PIL import Image, ImageDraw, ImageChops
import os

S = 1024
out = os.path.join(os.path.dirname(__file__), "..", "assets")

def lerp(a, b, t): return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))

# Background: diagonal gradient inside a rounded square.
top, bot = (0x7a, 0x8a, 0xf5), (0x44, 0x4f, 0xc8)
bg = Image.new("RGB", (S, S))
px = bg.load()
for y in range(S):
    for x in range(S):
        px[x, y] = lerp(top, bot, (x + y) / (2 * S - 2))
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, S - 1, S - 1), radius=230, fill=255)
icon = bg.convert("RGBA"); icon.putalpha(mask)

# Soft shadow + white speech bubble with tail.
bubble = Image.new("L", (S, S), 0)
d = ImageDraw.Draw(bubble)
d.rounded_rectangle((170, 220, 854, 700), radius=170, fill=255)
d.polygon([(330, 660), (330, 840), (520, 690)], fill=255)
from PIL import ImageFilter
shadow = bubble.filter(ImageFilter.GaussianBlur(28)).point(lambda v: int(v * 0.30))
sh = Image.new("RGBA", (S, S), (20, 25, 90, 0)); sh.putalpha(shadow)
sh = ImageChops.offset(sh, 0, 22)
icon = Image.alpha_composite(icon, sh)
white = Image.new("RGBA", (S, S), (255, 255, 255, 255)); white.putalpha(bubble)
icon = Image.alpha_composite(icon, white)

# Three dots; the middle one is the online-green accent.
d = ImageDraw.Draw(icon)
for cx, col in ((345, (0x5b, 0x6e, 0xe1)), (512, (0x3b, 0xa5, 0x5d)), (679, (0x5b, 0x6e, 0xe1))):
    d.ellipse((cx - 56, 460 - 56, cx + 56, 460 + 56), fill=col)

icon.resize((256, 256), Image.LANCZOS).save(os.path.join(out, "icon.png"))
icon.save(os.path.join(out, "icon.ico"), sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
icon.save(os.path.join(out, "icon-1024.png"))
