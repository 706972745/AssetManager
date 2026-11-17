#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""截图覆盖层 E2E（0.10.1 自测）：F1 → 拖选区 → 截图 → 中键贴图 → 再截。

只给本机自测用，不进产品。坐标一律 **CSS 像素**（本机 DPR 1.25）。
"""

import ctypes
import subprocess
import sys
import time
from ctypes import wintypes

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()
DPR = 1.25


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [
        ("wVk", wintypes.WORD),
        ("wScan", wintypes.WORD),
        ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.POINTER(wintypes.ULONG)),
    ]


class INPUT(ctypes.Structure):
    class _U(ctypes.Union):
        _fields_ = [("ki", KEYBDINPUT), ("padding", ctypes.c_byte * 24)]

    _anonymous_ = ("u",)
    _fields_ = [("type", wintypes.DWORD), ("u", _U)]


def key(vk, up=False):
    """按键。

    **必须用 `keybd_event`，不能用 `SendInput`**（2026-11-18 实测）。
    这个脚本原来走 SendInput，在参考机上 F1 **一次都送不到全局热键**
    （日志里连 `hotkey fired F1` 都没有），于是永远报「截图覆盖层没出现」；
    换回 `keybd_event` 立刻出覆盖层。同机另外三个门禁（e2e_cancel / e2e_v0120 /
    e2e_noclip）本来就是 keybd_event，所以它们一直是好的 —— 别再把这条改回去。
    """
    user32.keybd_event(vk, 0, 2 if up else 0, 0)


def find(title, visible=True):
    out = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        n = user32.GetWindowTextLengthW(h)
        if n and (not visible or user32.IsWindowVisible(h)):
            b = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(h, b, n + 1)
            r = wintypes.RECT()
            user32.GetWindowRect(h, ctypes.byref(r))
            out.append((b.value, r.left, r.top, r.right - r.left, r.bottom - r.top, h))
        return True

    user32.EnumWindows(cb, 0)
    hit = [x for x in out if x[0] == title]
    if not hit:
        # 覆盖层标题在不同版本里可能带后缀（「截图 1920×1080」之类），退一步按前缀认
        hit = [x for x in out if x[0].startswith(title)]
    return hit[0] if hit else None


def drag(x1, y1, x2, y2, steps=10, origin=(0, 0)):
    ox, oy = origin
    user32.SetCursorPos(int(ox + x1 * DPR), int(oy + y1 * DPR))
    time.sleep(0.15)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.08)
    for i in range(1, steps + 1):
        user32.SetCursorPos(
            int(ox + (x1 + (x2 - x1) * i / steps) * DPR),
            int(oy + (y1 + (y2 - y1) * i / steps) * DPR),
        )
        time.sleep(0.03)
    time.sleep(0.08)
    user32.mouse_event(0x0004, 0, 0, 0, 0)


def click(x, y, origin=(0, 0)):
    ox, oy = origin
    user32.SetCursorPos(int(ox + x * DPR), int(oy + y * DPR))
    time.sleep(0.15)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.06)
    user32.mouse_event(0x0004, 0, 0, 0, 0)


def shot(title, out):
    subprocess.run(
        [sys.executable, "tools/shot.py", title, out],
        check=False,
        cwd="E:/Users/87557/文档/wordbuddy/软件迭代/AssetManager",
    )


def main():
    # 1) F1 开截图
    main_win = find("AssetManager")
    if main_win:
        user32.ShowWindow(main_win[5], 9)
        user32.SetForegroundWindow(main_win[5])
        time.sleep(0.4)
    key(0x70)
    key(0x70, True)
    # 2.5s（原 1.8s）：覆盖层要先拍一张全屏静态图再显示，慢机器/大桌面会超过 1.8s
    time.sleep(2.5)

    w = find("截图")
    if not w:
        print("❌ 截图覆盖层没出现")
        return 1
    print("① 覆盖层", w[1:5])

    # 2) 拖一个选区
    drag(300, 220, 1000, 760)
    time.sleep(1.5)
    shot("截图", "tools/_snip-1-select.png")
    print("② 选区已框，图存 tools/_snip-1-select.png")

    # 3) 画一个矩形（默认工具）
    drag(400, 300, 700, 520, steps=8)
    time.sleep(0.5)
    # 4) 切到箭头再画一笔
    click(300 + 26 * 3 + 4 + 13, 220 + 760 + 10 + 17)  # 工具栏第 4 个（画笔）附近
    time.sleep(0.3)
    shot("截图", "tools/_snip-2-draw.png")
    print("③ 标注后图存 tools/_snip-2-draw.png")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
