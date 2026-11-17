#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""截图标注**坐标是否偏移**自测（owner 报「用工具箱有时会偏移」）。

做法：F1 → 拖一个选区 → 按 R 选矩形工具 → 在选区里拖一个已知矩形 →
截图后按像素找那圈红色描边，和"我们拖的位置"比。误差超过 4px 就算偏移。

用法：python tools/e2e_offset.py
输出：_tmp/offset.png + 终端里的偏差数据。
"""

import ctypes
import os
import sys
import time

from PIL import ImageGrab

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TMP = os.path.join(ROOT, "_tmp")
DPR = 1.25
VK_F1, VK_R, VK_ESC = 0x70, 0x52, 0x1B
KEYUP = 0x0002
DOWN, UP = 0x0002, 0x0004

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()


def find(title):
    got = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(h, _):
        n = user32.GetWindowTextLengthW(h)
        if n:
            b = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(h, b, n + 1)
            if b.value == title and user32.IsWindowVisible(h):
                got.append(h)
        return True

    user32.EnumWindows(cb, 0)
    return got[0] if got else 0


def tap(vk):
    user32.keybd_event(vk, 0, 0, 0)
    time.sleep(0.06)
    user32.keybd_event(vk, 0, KEYUP, 0)
    time.sleep(0.25)


def drag(x1, y1, x2, y2, steps=18):
    """物理像素坐标拖拽"""
    user32.SetCursorPos(int(x1), int(y1))
    time.sleep(0.12)
    user32.mouse_event(DOWN, 0, 0, 0, 0)
    for i in range(1, steps + 1):
        x = x1 + (x2 - x1) * i / steps
        y = y1 + (y2 - y1) * i / steps
        user32.SetCursorPos(int(x), int(y))
        time.sleep(0.012)
    time.sleep(0.1)
    user32.mouse_event(UP, 0, 0, 0, 0)
    time.sleep(0.3)


def main():
    hwnd = find("AssetManager")
    if not hwnd:
        raise SystemExit("先启动 AssetManager")
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.4)

    tap(VK_F1)
    time.sleep(2.6)

    # 选区：物理 (700,320) → (1500,900)
    drag(700, 320, 1500, 900)
    time.sleep(0.5)

    # 矩形工具（R），然后在选区里拖一个已知矩形：物理 (900,450) → (1300,750)
    tap(VK_R)
    drag(900, 450, 1300, 750)
    time.sleep(0.6)

    os.makedirs(TMP, exist_ok=True)
    from PIL import Image

    # 画之前：光标停在一个固定点（放大镜跟着光标，得让它两次一致）
    park = (1900, 1250)
    user32.SetCursorPos(*park)
    time.sleep(0.5)
    a_path = os.path.join(TMP, "offset-before.png")
    ImageGrab.grab(all_screens=True).save(a_path)

    # 画矩形：物理 (900,450) → (1300,750)
    drag(900, 450, 1300, 750)
    user32.SetCursorPos(*park)
    time.sleep(0.6)
    b_path = os.path.join(TMP, "offset-after.png")
    ImageGrab.grab(all_screens=True).save(b_path)

    before = Image.open(a_path).convert("RGB")
    after = Image.open(b_path).convert("RGB")
    pb, pa = before.load(), after.load()

    # 只在选区内部找"变了的像素" —— 那就是画上去的描边
    # （截图里本来就有的红色素材不能当证据，所以用前后差分）
    xs, ys = [], []
    for y in range(340, 890):
        for x in range(710, 1490):
            r1, g1, b1 = pb[x, y]
            r2, g2, b2 = pa[x, y]
            if abs(r1 - r2) + abs(g1 - g2) + abs(b1 - b2) > 60:
                xs.append(x)
                ys.append(y)
    tap(VK_ESC)
    time.sleep(1.2)

    if not xs:
        print("前后没差异：矩形没画上？")
        return 1
    fx0, fx1, fy0, fy1 = min(xs), max(xs), min(ys), max(ys)
    print("拖拽（物理）:  x 900–1300  y 450–750")
    print(f"实测描边:      x {fx0}–{fx1}  y {fy0}–{fy1}   ({len(xs)} px)")
    half = 2  # 线宽 3px，描边中心线在两角内侧 1.5px
    dx = (fx0 - 900, fx1 - 1300)
    dy = (fy0 - 450, fy1 - 750)
    print(f"偏差:          dx={dx}  dy={dy}  (容差 ±4，线宽 ±{half})")
    ok = all(abs(v) <= 4 for v in (*dx, *dy))
    print("结论：" + ("坐标一致（无偏移）" if ok else "有偏移，需要查"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
