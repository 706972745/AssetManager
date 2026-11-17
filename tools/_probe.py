#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""临时探针：给窗口发鼠标移动 / 滚轮 / 点击，配合 tools/shot.py 做视觉核对。

用法：
  python tools/_probe.py rect
  python tools/_probe.py move <x> <y>          # 相对窗口左上角
  python tools/_probe.py wheel <x> <y> <n>     # n>0 向上，n<0 向下
  python tools/_probe.py click <x> <y>
坐标都是**窗口相对**（窗口无边框，等价于 client）。
"""

import ctypes
import sys
import time
from ctypes import wintypes

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

MOUSEEVENTF_WHEEL = 0x0800
MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
MOUSEEVENTF_RIGHTDOWN = 0x0008
MOUSEEVENTF_RIGHTUP = 0x0010


def hwnd_of(title: str) -> int:
    """和 shot.py 同一套：优先标题**完全相等**的可见窗口（FindWindowW 会先撞到
    任务栏 / IME 那种同名小窗），没有才退回包含匹配。"""
    exact, loose = [], []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _l):
        if not user32.IsWindowVisible(hwnd):
            return True
        n = user32.GetWindowTextLengthW(hwnd)
        if n == 0:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        if buf.value == title:
            exact.append(hwnd)
        elif title.lower() in buf.value.lower():
            loose.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    if exact:
        return exact[0]
    if loose:
        return loose[0]
    raise SystemExit(f"window not found: {title}")


def rect_of(h: int):
    r = wintypes.RECT()
    user32.GetWindowRect(h, ctypes.byref(r))
    return r


def main() -> None:
    cmd = sys.argv[1] if len(sys.argv) > 1 else "rect"
    h = hwnd_of("AssetManager")
    r = rect_of(h)
    if cmd == "rect":
        print(r.left, r.top, r.right - r.left, r.bottom - r.top)
        return
    if cmd == "move":
        x, y = int(sys.argv[2]), int(sys.argv[3])
        user32.SetCursorPos(r.left + x, r.top + y)
        return
    if cmd == "wheel":
        x, y, n = int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
        user32.SetCursorPos(r.left + x, r.top + y)
        for _ in range(abs(n)):
            user32.mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -120 if n > 0 else 120, 0)
            time.sleep(0.03)
        return
    if cmd == "rclick":
        x, y = int(sys.argv[2]), int(sys.argv[3])
        user32.SetCursorPos(r.left + x, r.top + y)
        time.sleep(0.05)
        user32.mouse_event(MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, 0)
        time.sleep(0.03)
        user32.mouse_event(MOUSEEVENTF_RIGHTUP, 0, 0, 0, 0)
        print("RCLICK", x, y)
        return

    if cmd == "click":
        x, y = int(sys.argv[2]), int(sys.argv[3])
        user32.SetCursorPos(r.left + x, r.top + y)
        time.sleep(0.1)
        user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
        time.sleep(0.05)
        user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
        return
    if cmd == "dblclick":
        x, y = int(sys.argv[2]), int(sys.argv[3])
        user32.SetCursorPos(r.left + x, r.top + y)
        time.sleep(0.1)
        for _ in range(2):
            user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
            time.sleep(0.04)
            user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
            time.sleep(0.04)
        return
    raise SystemExit(f"unknown cmd {cmd}")


if __name__ == "__main__":
    main()
