#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
生成 AssetManager 应用图标（拼贴纸片风，母题「叠片」）。

用法：python tools/make-icons.py
产出：src-tauri/icons/{icon.ico, 32x32.png, 64x64.png, 128x128.png, 128x128@2x.png, icon.png}
      + 另外四套同族图标（T19）：pack.ico / library.ico / asset3d.ico / broken.ico

五套图标的用途（同一视觉语言，只换中间那块母题）：
  icon      主程序
  pack      素材包（一个库导出成一个包）
  library   素材库（侧栏"素材库"那一栏）
  asset3d   3D 素材
  broken    坏库 / 缺失文件（索引损坏时的提示位）

硬约束（沿用 FontManager 图标规范）：
- .ico 必须七帧 16/24/32/48/64/128/256，少一帧以后单测要挂
- 16/24 单独看可辨识，纸纹和细线在 16px 会糊
- 母版 1024 绘制后 Lanczos 下采样，不做 512 以上放大
"""

import os
from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "src-tauri", "icons")

S = 1024
INK = (20, 20, 18, 255)
PAPER = (255, 255, 255, 255)
G1 = (216, 216, 210, 255)
G2 = (176, 176, 168, 255)


def rounded(draw, box, r, fill, outline=None, width=0):
    draw.rounded_rectangle(box, radius=r, fill=fill, outline=outline, width=width)


def master(motif: str = "main", bold: bool = False) -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # 白底圆角方块（桌面上像一张纸）
    m = int(S * 0.06)
    rounded(d, (m, m, S - m, S - m), int(S * 0.22), PAPER)

    # 三张错位叠片
    cw, ch = int(S * 0.44), int(S * 0.50)
    x0, y0 = int(S * 0.20), int(S * 0.30)
    step = int(S * 0.055)
    rounded(d, (x0, y0 + step * 2, x0 + cw, y0 + ch + step * 2), int(S * 0.03), G1)
    rounded(d, (x0 + step, y0 + step, x0 + cw + step, y0 + ch + step), int(S * 0.03), G2)

    # 最上面那张：白卡 + 墨线 + 一张“照片”
    card = (x0 + step * 2, y0, x0 + cw + step * 2, y0 + ch)
    rounded(d, card, int(S * 0.03), PAPER, INK, max(3, int(S * 0.012)))

    # 照片区（内嵌）
    pad = int(S * 0.045)
    px0, py0 = card[0] + pad, card[1] + pad
    px1, py1 = card[2] - pad, card[3] - pad
    rounded(d, (px0, py0, px1, py1), int(S * 0.012), (238, 238, 232, 255))

    # 山与太阳
    sun_r = int(S * 0.030)
    sx, sy = px0 + int((px1 - px0) * 0.72), py0 + int((py1 - py0) * 0.26)
    d.ellipse((sx - sun_r, sy - sun_r, sx + sun_r, sy + sun_r), fill=INK)
    d.polygon(
        [
            (px0 + int((px1 - px0) * 0.10), py1 - int((py1 - py0) * 0.10)),
            (px0 + int((px1 - px0) * 0.40), py0 + int((py1 - py0) * 0.46)),
            (px0 + int((px1 - px0) * 0.66), py1 - int((py1 - py0) * 0.10)),
        ],
        fill=INK,
    )
    d.polygon(
        [
            (px0 + int((px1 - px0) * 0.50), py1 - int((py1 - py0) * 0.10)),
            (px0 + int((px1 - px0) * 0.72), py0 + int((py1 - py0) * 0.58)),
            (px0 + int((px1 - px0) * 0.95), py1 - int((py1 - py0) * 0.10)),
        ],
        fill=INK,
    )

    # 四套变体：白纸 + 叠片不变（一眼认得出是同一家），只换照片区那块母题
    w = px1 - px0
    h = py1 - py0
    if motif != "main":
        # 先把刚才画的"照片"擦成空白，再画各自母题
        rounded(d, (px0, py0, px1, py1), int(S * 0.012), (238, 238, 232, 255))
        # 16/24px 是「另一张母版」：线宽 ×3、格子缝隙拉大。
        # 按比例缩下去的话 4px 线在 16px 里只剩 0.06px，等于什么都没有（实测墨点 0.4%）。
        k = 3 if bold else 1
        lw = max(4, int(S * 0.014)) * k

        if motif == "pack":
            # 一叠被束带捆住的片：两条横带 + 中间一道竖扣
            rounded(
                d,
                (px0 + int(w * 0.10), py0 + int(h * 0.14), px1 - int(w * 0.10), py0 + int(h * 0.34)),
                int(S * 0.008), G2,
            )
            rounded(
                d,
                (px0 + int(w * 0.10), py0 + int(h * 0.40), px1 - int(w * 0.10), py0 + int(h * 0.60)),
                int(S * 0.008), G2,
            )
            rounded(
                d,
                (px0 + int(w * 0.10), py0 + int(h * 0.66), px1 - int(w * 0.10), py0 + int(h * 0.86)),
                int(S * 0.008), G2,
            )
            d.rectangle(
                (px0 + int(w * 0.44), py0 + int(h * 0.06), px0 + int(w * 0.56), py1 - int(h * 0.06)),
                fill=INK,
            )
        elif motif == "library":
            # 2×2 方格：库的"多目录"意象
            gap = int(w * (0.10 if bold else 0.06))
            cwid = (w - gap) // 2
            chgt = (h - gap) // 2
            for r in range(2):
                for c in range(2):
                    x = px0 + c * (cwid + gap)
                    y = py0 + r * (chgt + gap)
                    rounded(d, (x, y, x + cwid, y + chgt), int(S * 0.010), INK)
        elif motif == "asset3d":
            # 立方体线框（等轴测）
            cx, cy = (px0 + px1) // 2, (py0 + py1) // 2
            rw, rh = int(w * 0.34), int(h * 0.30)
            top = [(cx, cy - rh), (cx + rw, cy - rh // 2), (cx, cy), (cx - rw, cy - rh // 2)]
            bot = [(cx - rw, cy + rh // 2), (cx, cy + rh), (cx + rw, cy + rh // 2)]
            d.polygon(top, fill=None, outline=INK)
            d.line([top[3], bot[0], bot[1], bot[2], top[1]], fill=INK, width=lw, joint="curve")
            d.line([top[0], (cx, cy)], fill=INK, width=lw)
            d.line([(cx, cy), (cx, cy + rh)], fill=INK, width=lw)
            d.line([(cx - rw, cy - rh // 2), bot[0]], fill=INK, width=lw)
            d.line([(cx + rw, cy - rh // 2), bot[2]], fill=INK, width=lw)
            d.line([top[0], top[1], top[2], top[3], top[0]], fill=INK, width=lw, joint="curve")
        elif motif == "broken":
            # 一道锯齿裂缝把片子劈开
            d.line(
                [
                    (px0 + int(w * 0.52), py0),
                    (px0 + int(w * 0.40), py0 + int(h * 0.32)),
                    (px0 + int(w * 0.58), py0 + int(h * 0.50)),
                    (px0 + int(w * 0.44), py0 + int(h * 0.74)),
                    (px0 + int(w * 0.52), py1),
                ],
                fill=INK,
                width=max(6, int(S * 0.018)) * k,
                joint="curve",
            )
    return img


SIZES = [16, 24, 32, 48, 64, 128, 256]


def write_ico(big: Image.Image, name: str, bold: Image.Image | None = None) -> int:
    """七帧一起写。少一帧 Windows 在某些缩放比例下会拿 256 硬缩，边缘发毛。

    `bold` = 加粗母版，只给 16 / 24 两帧用 —— 细线在 16px 会整条消失，
    必须换一张"粗到不讲理"的图缩下去才看得见。
    """
    frames = []
    for s in SIZES:
        src = bold if (bold is not None and s <= 24) else big
        b = 1024 if src.width != 1024 else src.width
        frames.append(src.resize((s, s), Image.LANCZOS) if b >= s else src.resize((s, s), Image.LANCZOS))
    path = os.path.join(OUT, name)
    frames[-1].save(
        path,
        format="ICO",
        sizes=[(s, s) for s in SIZES],
        append_images=frames[:-1],
    )
    return os.path.getsize(path)


def main():
    os.makedirs(OUT, exist_ok=True)
    big = master("main")
    big = big.resize((1024, 1024), Image.LANCZOS)
    write_ico(big, "icon.ico")

    for name, s in [
        ("32x32.png", 32),
        ("64x64.png", 64),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("icon.png", 512),
    ]:
        big.resize((s, s), Image.LANCZOS).save(os.path.join(OUT, name))

    ico = os.path.join(OUT, "icon.ico")
    print("已生成 %s（%d 字节）" % (ico, os.path.getsize(ico)))
    for name in ["32x32.png", "64x64.png", "128x128.png", "128x128@2x.png", "icon.png"]:
        print("  ", name)

    # 另外四套（T19）：同族，只换母题
    for motif, fname in [
        ("pack", "pack.ico"),
        ("library", "library.ico"),
        ("asset3d", "asset3d.ico"),
        ("broken", "broken.ico"),
    ]:
        v = master(motif).resize((1024, 1024), Image.LANCZOS)
        vb = master(motif, bold=True).resize((1024, 1024), Image.LANCZOS)
        n = write_ico(v, fname, vb)
        # 16px 单独校验：墨水覆盖率太低（<3%）说明细节糊没了，得回头加粗
        p16 = vb.resize((16, 16), Image.LANCZOS)
        # "有内容" = 不透明且不是白底（灰片也算，别只数纯黑线）
        ink = sum(
            1
            for px in p16.getdata()
            if px[3] > 40 and not (px[0] > 225 and px[1] > 225 and px[2] > 220)
        )
        cover = ink / 256.0
        flag = "OK" if cover >= 0.10 else "太淡！"
        print("  %-12s %6d 字节 · 16px 墨点 %.1f%% %s" % (fname, n, cover * 100, flag))


if __name__ == "__main__":
    main()
