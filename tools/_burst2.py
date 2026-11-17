#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""点一下再连拍（**屏幕取像**，不是 PrintWindow）。

为什么另起一个：WebView2 的 PrintWindow(PW_RENDERFULLCONTENT) 在动画期间会
返回**上一帧已经提交的合成结果**，连拍出来 8 张一模一样（实测），根本看不出
有没有在动。屏幕 BitBlt 拿的是当下真正显示出来的像素，动画能看见。

用法：python tools/_burst2.py <x> <y> <帧数> <间隔ms> <输出前缀> [x2 y2]
（x,y 是**窗口相对**坐标；给了第二组坐标就再点一下，用来测"点了又点回去"）
"""
import ctypes
import sys
import time
from ctypes import wintypes

from PIL import Image

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
user32.SetProcessDPIAware()

MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004


def find_window(part: str):
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _l):
        if not user32.IsWindowVisible(hwnd):
            return True
        n = user32.GetWindowTextLengthW(hwnd)
        if n == 0:
            return True
        b = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, b, n + 1)
        if b.value == part:
            found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else None


def grab(rect) -> Image.Image:
    x, y, w, h = rect
    hdc = user32.GetDC(0)
    mem = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, w, h)
    gdi32.SelectObject(mem, bmp)
    gdi32.BitBlt(mem, 0, 0, w, h, hdc, x, y, 0x00CC0020)  # SRCCOPY
    class BIH(ctypes.Structure):
        _fields_ = [
            ("biSize", wintypes.DWORD),
            ("biWidth", wintypes.LONG),
            ("biHeight", wintypes.LONG),
            ("biPlanes", wintypes.WORD),
            ("biBitCount", wintypes.WORD),
            ("biCompression", wintypes.DWORD),
            ("biSizeImage", wintypes.DWORD),
            ("biXPelsPerMeter", wintypes.LONG),
            ("biYPelsPerMeter", wintypes.LONG),
            ("biClrUsed", wintypes.DWORD),
            ("biClrImportant", wintypes.DWORD),
        ]
    class BI(ctypes.Structure):
        _fields_ = [("bmiHeader", BIH), ("bmiColors", wintypes.DWORD * 3)]

    bi = BI()
    bi.bmiHeader.biSize = ctypes.sizeof(BIH)
    bi.bmiHeader.biWidth = w
    bi.bmiHeader.biHeight = -h
    bi.bmiHeader.biPlanes = 1
    bi.bmiHeader.biBitCount = 32
    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mem, bmp, 0, h, buf, ctypes.byref(bi), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mem)
    user32.ReleaseDC(0, hdc)
    return Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB")


def click(x: int, y: int) -> None:
    user32.SetCursorPos(x, y)
    time.sleep(0.03)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    time.sleep(0.015)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)


def main() -> None:
    x, y = int(sys.argv[1]), int(sys.argv[2])
    n = int(sys.argv[3])
    gap = int(sys.argv[4]) / 1000.0
    prefix = sys.argv[5]

    hwnd = find_window("AssetManager")
    if not hwnd:
        raise SystemExit("NO_WINDOW")
    r = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    rect = (r.left, r.top, r.right - r.left, r.bottom - r.top)

    click(r.left + x, r.top + y)
    t0 = time.perf_counter()
    for i in range(n):
        grab(rect).save(f"{prefix}{i}.png")
        print(i, f"{(time.perf_counter() - t0) * 1000:.0f}ms")
        time.sleep(gap)


if __name__ == "__main__":
    main()
