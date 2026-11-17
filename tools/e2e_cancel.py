#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""截图会话自测（只给本机自测用）：F1 → 抓状态 → Esc 取消 → 抓状态。

要验的两件事（owner 2026-11-17 报的）：
1. **取消不能卡死**：Esc 之后原生底图必须消失、桌面要能被别的窗口点；
2. 覆盖层本身要正常显示（有压暗 / 工具栏），不是黑屏。

用法：python tools/e2e_cancel.py
输出：_tmp/cancel-1-snip.png / _tmp/cancel-2-after.png + 终端结论。
"""

import ctypes
import os
import subprocess
import sys
import time

from PIL import ImageGrab

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
TMP = os.path.join(ROOT, "_tmp")
VK_F1 = 0x70
VK_ESC = 0x1B
KEYEVENTF_KEYUP = 0x0002

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()


def find(title: str) -> int:
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(hwnd, _):
        n = user32.GetWindowTextLengthW(hwnd)
        if n:
            buf = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(hwnd, buf, n + 1)
            if buf.value == title and user32.IsWindowVisible(hwnd):
                found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else 0


def tap(vk: int):
    user32.keybd_event(vk, 0, 0, 0)
    time.sleep(0.05)
    user32.keybd_event(vk, 0, KEYEVENTF_KEYUP, 0)


def shot(name: str):
    os.makedirs(TMP, exist_ok=True)
    p = os.path.join(TMP, name)
    ImageGrab.grab(all_screens=True).save(p)
    return p


def splash_windows() -> int:
    """数一下还有没有我们那块原生底图窗口（类名 AMSnipSplash）。"""
    n = [0]

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(hwnd, _):
        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        if cls.value == "AMSnipSplash" and user32.IsWindowVisible(hwnd):
            n[0] += 1
        return True

    user32.EnumWindows(cb, 0)
    return n[0]


def main():
    hwnd = find("AssetManager")
    if not hwnd:
        raise SystemExit("先启动 AssetManager")
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.4)

    print("1) 按 F1 …")
    tap(VK_F1)
    time.sleep(2.5)
    splash = splash_windows()
    snip = find("截图")
    p1 = shot("cancel-1-snip.png")
    print(f"   底图窗口={splash} 覆盖层窗口={'有' if snip else '没有'} → {p1}")

    print("2) 按 Esc 取消 …")
    if snip:
        user32.SetForegroundWindow(snip)
        time.sleep(0.2)
    tap(VK_ESC)
    time.sleep(2.0)
    splash2 = splash_windows()
    snip2 = find("截图")
    p2 = shot("cancel-2-after.png")
    print(f"   底图窗口={splash2} 覆盖层窗口={'还在' if snip2 else '已关'} → {p2}")

    ok = splash2 == 0 and snip2 == 0
    print("结论：" + ("取消干净（不卡死）" if ok else "仍有残留窗口 —— 卡死风险"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
