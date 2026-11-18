#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""验证「截图 → 中键贴图」的两条硬要求（owner 2026-11-18）：

  1. **尺寸一致**：图片区域的宽高 == 选区宽高（物理像素，1:1，不缩放）；
  2. **位置一致**：图片区域左上角 == 选区左上角（落在截图那块地方，不居中到鼠标）；
  3. **蓝色边框 + 向外渐变**：图片四周（窗口内那一圈透明边距里）有蓝光。

注意：窗口本身比图片大一圈（四周各 `PEEK_GLOW`=20 CSS px，用来画向外的渐变），
所以比较的是**图片区域** = 窗口 rect 内缩 glow，不是窗口 rect。

做法：
  F1 → 拖一块 (700,420,360,320) → 中键 → 找标题「看图」的窗口 → 比 rect → 抓像素看边框。

坐标全是**物理像素**（SetProcessDPIAware + SetCursorPos / GetWindowRect 都是物理）。
DPI 125% 时选区在覆盖层里的 CSS 坐标是 560,336，换算回来必须还是 700,420 ——
差超过 2px 就算没对齐（网页缩放 / 窗口位置换算错一步就会差几十像素）。

用法：python tools/e2e_peek_place.py
退出码 0 = 三条全过。
"""

import ctypes
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import e2e_noclip as E  # noqa: E402  （复用 ensure_app / find / drag / tap / EXE）

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()


class RECT(ctypes.Structure):
    _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long),
                ("right", ctypes.c_long), ("bottom", ctypes.c_long)]


def rect_of(hwnd):
    r = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r.left, r.top, r.right - r.left, r.bottom - r.top


def middle_click(x, y):
    user32.SetCursorPos(x, y)
    time.sleep(0.2)
    user32.mouse_event(0x0020, 0, 0, 0, 0)  # MIDDLEDOWN
    time.sleep(0.06)
    user32.mouse_event(0x0040, 0, 0, 0, 0)  # MIDDLEUP
    time.sleep(0.3)


def grab(x, y, w, h):
    """抓屏幕一块（物理像素）。返回 PIL Image。"""
    from PIL import ImageGrab
    return ImageGrab.grab(bbox=(x, y, x + w, y + h), all_screens=True)


def dpi_scale(hwnd):
    """窗口 DPI 缩放（125% → 1.25）。"""
    try:
        dpi = user32.GetDpiForWindow(hwnd)
        return dpi / 96.0 if dpi else 1.0
    except Exception:
        return 1.0


def edge_is_blue(im):
    """看四条边的中段像素，判断有没有蓝色描边。

    边框是 `inset 0 0 0 1px rgba(56,142,255,.95)`：1 CSS px，125% 下约占 1 物理像素，
    所以只看**最外那一行/列**，并且允许某个方向被任务栏之类挡住（四个方向过两个就算过）。
    """
    w, h = im.size
    px = im.convert("RGB").load()
    hits = 0
    dirs = {
        "上": [(w // 2 + d, 0) for d in (-2, -1, 0, 1, 2)],
        "下": [(w // 2 + d, h - 1) for d in (-2, -1, 0, 1, 2)],
        "左": [(0, h // 2 + d) for d in (-2, -1, 0, 1, 2)],
        "右": [(w - 1, h // 2 + d) for d in (-2, -1, 0, 1, 2)],
    }
    for name, pts in dirs.items():
        ok = False
        for (x, y) in pts:
            r, g, b = px[max(0, min(w - 1, x)), max(0, min(h - 1, y))]
            if b > 150 and b - r > 40 and b - g > 25:
                ok = True
                break
        if ok:
            hits += 1
    return hits, dirs


SEL = (700, 420, 360, 320)  # x, y, w, h（物理像素）
TOL = 2                     # 允许 ±2px（无边框窗在 DWM 下可能多 1px）


def main():
    sw, sh = user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)
    print(f"屏幕 {sw}×{sh}，选区 {SEL}")
    if SEL[0] + SEL[2] > sw or SEL[1] + SEL[3] > sh:
        raise SystemExit("屏幕太小，改 SEL 再跑")

    hwnd = E.ensure_app()
    # 先按一次 Esc 清掉可能残留的覆盖层 / 贴图
    E.tap(E.VK_ESC)
    time.sleep(0.4)
    if not E.snip_open(hwnd):
        raise SystemExit("按 F1 后没等到「截图」覆盖层")

    time.sleep(1.6)
    E.drag(SEL[0], SEL[1], SEL[0] + SEL[2], SEL[1] + SEL[3])
    time.sleep(0.9)  # > 350ms 交付冷却

    middle_click(SEL[0] + SEL[2] // 2, SEL[1] + SEL[3] // 2)

    peek = 0
    for _ in range(30):
        time.sleep(0.25)
        peek = E.find("看图")
        if peek:
            break
    if not peek:
        raise SystemExit("中键之后没等到「看图」窗口")

    time.sleep(0.9)  # 等首帧 + set_size/set_position 都生效
    x, y, w, h = rect_of(peek)
    sc = dpi_scale(peek)
    g = round(20 * sc)          # 四周的发光边（物理；与后端 PEEK_GLOW / CSS --peek-glow 同值）
    ix, iy, iw, ih = x + g, y + g, w - 2 * g, h - 2 * g
    dx, dy, dw, dh = ix - SEL[0], iy - SEL[1], iw - SEL[2], ih - SEL[3]
    print(f"窗口 rect = ({x}, {y}) {w}×{h}   DPI {sc:.2f} → 发光边 {g}px")
    print(f"图片区域 = ({ix}, {iy}) {iw}×{ih}   期望 ({SEL[0]}, {SEL[1]}) {SEL[2]}×{SEL[3]}")
    print(f"偏差: 位置 ({dx:+d}, {dy:+d})  尺寸 ({dw:+d}, {dh:+d})")

    im = grab(x, y, w, h)
    im.save(os.path.join(E.TMP, "peek-place.png"))

    # 外圈光晕：在图片外侧 2~6px 那一圈里找蓝色（越高越靠外越淡）
    px = im.convert("RGB").load()
    hits = 0
    for name, pts in {
        "上": [(w // 2 + d, g - 4) for d in range(-6, 7)],
        "下": [(w // 2 + d, h - g + 3) for d in range(-6, 7)],
        "左": [(g - 4, h // 2 + d) for d in range(-6, 7)],
        "右": [(w - g + 3, h // 2 + d) for d in range(-6, 7)],
    }.items():
        for (a, b) in pts:
            r, gg, bb = px[max(0, min(w - 1, a)), max(0, min(h - 1, b))]
            if bb > 110 and bb - r > 30 and bb - gg > 18:
                hits += 1
                break
    print(f"向外渐变的蓝光：四条边里 {hits} 条命中")

    ok_pos = abs(dx) <= TOL and abs(dy) <= TOL
    ok_size = abs(dw) <= TOL and abs(dh) <= TOL
    ok_frame = hits >= 2
    print()
    print(f"  位置一致  {'PASS' if ok_pos else 'FAIL'}")
    print(f"  尺寸一致  {'PASS' if ok_size else 'FAIL'}")
    print(f"  蓝色边框  {'PASS' if ok_frame else 'FAIL'}")

    # 收尾：关掉这扇贴图（Esc 两下：第一下清 view，第二下关）
    user32.SetForegroundWindow(peek)
    time.sleep(0.3)
    E.tap(E.VK_ESC)
    time.sleep(0.2)
    E.tap(E.VK_ESC)
    time.sleep(0.5)
    return 0 if (ok_pos and ok_size and ok_frame) else 1


if __name__ == "__main__":
    os.makedirs(E.TMP, exist_ok=True)
    sys.exit(main())
