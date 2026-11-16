#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""真机 GUI 驱动小工具（只给本机自测用，不进产品）。

用法：
  python tools/gui.py shot <窗口标题> <输出png>
  python tools/gui.py key <窗口标题> ctrl+comma
  python tools/gui.py click <窗口标题> <cssX> <cssY> [--dpr 1.25]

坐标约定：**传 CSS 像素**（跟界面里写的数值一致），脚本按窗口原点 + DPR 换算成屏幕坐标。
DPR 必须显式给或从环境读 —— 本机是 1.25（1600×1025 截图对应 1280×820 CSS），
按 1 算是「点了但没反应」的经典原因（FontManager WORKLOG #10/#13 记过）。
"""

import ctypes
import os
import sys
import time
from ctypes import wintypes

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

SW_RESTORE = 9
VK = {
    "ctrl": 0x11,
    "shift": 0x10,
    "alt": 0x12,
    "escape": 0x1B,
    "enter": 0x0D,
    # 字母与常用符号：快捷键自动化会用到（Ctrl+K / Ctrl+A / Ctrl+Z…）
    **{c: ord(c.upper()) for c in "abcdefghijklmnopqrstuvwxyz"},
    "comma": 0xBC,
    "period": 0xBE,
}


def find_hwnd(title: str) -> int:
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _):
        n = user32.GetWindowTextLengthW(hwnd)
        if n == 0:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        if buf.value == title and user32.IsWindowVisible(hwnd):
            cls = ctypes.create_unicode_buffer(256)
            user32.GetClassNameW(hwnd, cls, 256)
            if cls.value not in ("ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS"):
                found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    if not found:
        raise SystemExit(f"找不到窗口：{title}")
    return found[0]


def rect(hwnd):
    r = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r


def focus(hwnd):
    user32.ShowWindow(hwnd, SW_RESTORE)
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.35)


def click(hwnd, css_x, css_y, dpr=1.25):
    focus(hwnd)
    r = rect(hwnd)
    x = int(r.left + css_x * dpr)
    y = int(r.top + css_y * dpr)
    user32.SetCursorPos(x, y)
    time.sleep(0.12)
    user32.mouse_event(0x0002, 0, 0, 0, 0)  # LEFTDOWN
    time.sleep(0.05)
    user32.mouse_event(0x0004, 0, 0, 0, 0)  # LEFTUP
    time.sleep(0.45)
    print(f"click css=({css_x},{css_y}) -> screen=({x},{y})")


def key(hwnd, spec):
    focus(hwnd)
    parts = spec.lower().split("+")
    mods = [VK[p] for p in parts[:-1] if p in VK]
    main = VK.get(parts[-1])
    if main is None:
        raise SystemExit(f"不认识的键：{parts[-1]}")
    for m in mods:
        user32.keybd_event(m, 0, 0, 0)
    time.sleep(0.05)
    user32.keybd_event(main, 0, 0, 0)
    time.sleep(0.05)
    user32.keybd_event(main, 0, 2, 0)
    for m in reversed(mods):
        user32.keybd_event(m, 0, 2, 0)
    time.sleep(0.5)
    print(f"key {spec}")


def wheel(hwnd, css_x, css_y, notches, dpr=1.25):
    """在 (css_x, css_y) 处滚动鼠标滚轮。notches 为负 = 向下。"""
    focus(hwnd)
    r = rect(hwnd)
    x = int(r.left + css_x * dpr)
    y = int(r.top + css_y * dpr)
    user32.SetCursorPos(x, y)
    time.sleep(0.1)
    for _ in range(abs(int(notches))):
        delta = -120 if notches < 0 else 120
        user32.mouse_event(0x0800, 0, 0, delta, 0)  # MOUSEEVENTF_WHEEL
        time.sleep(0.02)
    time.sleep(0.2)
    print(f"wheel {notches} @css=({css_x},{css_y})")


def screen_shot(hwnd, out):
    """屏幕抓图（ImageGrab + 窗口矩形）。

    ⚠️ **别用 PrintWindow 截这个软件**：WebView2 的合成窗口在 PrintWindow 里会被要求
    重绘一次，实测多次之后渲染进程直接退出（进程消失、无 WER 事件、像"点了一下就关了"）。
    FontManager 的 WORKLOG #12 只记了「四角纯黑」，没记这个更要命的副作用。
    这条路径也拿得到桌面合成结果，圆角、透明边都是真实像素。
    """
    from PIL import ImageGrab

    focus(hwnd)
    time.sleep(0.5)
    r = rect(hwnd)
    img = ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom), all_screens=True)
    img.save(out)
    print(f"screen {img.size} -> {out}")


def main():
    args = sys.argv[1:]
    if not args:
        raise SystemExit(__doc__)
    cmd = args[0]
    dpr = 1.25
    if "--dpr" in args:
        i = args.index("--dpr")
        dpr = float(args[i + 1])
        args = args[:i] + args[i + 2 :]

    if cmd == "shot":
        title, out = args[1], args[2]
        hwnd = find_hwnd(title)
        os.system(f'python "E:/Users/87557/文档/wordbuddy/软件迭代/AssetManager/tools/shot.py" "{title}" "{out}"')
        print("shot ->", out)
    elif cmd == "screen":
        screen_shot(find_hwnd(args[1]), args[2])
    elif cmd == "key":
        key(find_hwnd(args[1]), args[2])
    elif cmd == "click":
        click(find_hwnd(args[1]), float(args[2]), float(args[3]), dpr)
    elif cmd == "wheel":
        wheel(find_hwnd(args[1]), float(args[2]), float(args[3]), int(args[4]), dpr)
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
