#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""截图交付自测：**保存不能动剪贴板**（owner 2026-11-18 报的 bug）。

判据：
1. 剪贴板里先放一段文本「AMCLIP123」；
2. F1 → 框一块 → `Ctrl+S` 保存；
3. 保存完后剪贴板**仍然是那段文本**（0.1.17 及以前会被换成截图 PNG）；
4. 再来一次，这次按 `Ctrl+C`（显式复制）→ 剪贴板里应该有图片（说明"复制"这条路没被误伤）。

用法：python tools/e2e_noclip.py
输出：_tmp/noclip-*.png + 终端结论；退出码 0 = 全过。
"""

import ctypes
import os
import subprocess
import sys
import time

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32
user32.SetProcessDPIAware()

# 64 位：句柄类返回值必须显式声明成 c_void_p，默认 c_int 会把指针截断
kernel32.GlobalAlloc.restype = ctypes.c_void_p
kernel32.GlobalAlloc.argtypes = [ctypes.c_uint, ctypes.c_size_t]
kernel32.GlobalLock.restype = ctypes.c_void_p
kernel32.GlobalLock.argtypes = [ctypes.c_void_p]
kernel32.GlobalUnlock.argtypes = [ctypes.c_void_p]
user32.GetClipboardData.restype = ctypes.c_void_p
user32.GetClipboardData.argtypes = [ctypes.c_uint]
user32.SetClipboardData.restype = ctypes.c_void_p
user32.SetClipboardData.argtypes = [ctypes.c_uint, ctypes.c_void_p]


CF_UNICODETEXT = 13
CF_DIB = 8
GMEM_MOVEABLE = 0x0002
VK_F1, VK_ESC = 0x70, 0x1B
KEYUP = 0x0002
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
TMP = os.path.join(ROOT, "_tmp")
EXE = os.path.join(os.environ.get("LOCALAPPDATA", ""), "AssetManager", "assetmanager.exe")

MARK = "AMCLIP123"


# ---------------------------------------------------------------- 剪贴板

def clip_set_text(s: str) -> None:
    user32.OpenClipboard(None)
    try:
        user32.EmptyClipboard()
        data = (s + "\0").encode("utf-16-le")
        h = kernel32.GlobalAlloc(GMEM_MOVEABLE, len(data))
        p = kernel32.GlobalLock(h)
        ctypes.memmove(p, data, len(data))
        kernel32.GlobalUnlock(h)
        user32.SetClipboardData(CF_UNICODETEXT, h)
    finally:
        user32.CloseClipboard()


def clip_text() -> str:
    if not user32.IsClipboardFormatAvailable(CF_UNICODETEXT):
        return ""
    user32.OpenClipboard(None)
    try:
        h = user32.GetClipboardData(CF_UNICODETEXT)
        p = kernel32.GlobalLock(h)
        try:
            return ctypes.wstring_at(p)
        finally:
            kernel32.GlobalUnlock(h)
    finally:
        user32.CloseClipboard()


def clip_has_image() -> bool:
    """CF_DIB 在 = 剪贴板里有位图（PNG 走 Shell 时也会同时带 DIB）。"""
    return bool(user32.IsClipboardFormatAvailable(CF_DIB))


# ---------------------------------------------------------------- 窗口

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
    user32.keybd_event(vk, 0, KEYUP, 0)


def chord(mod: int, vk: int):
    user32.keybd_event(mod, 0, 0, 0)
    time.sleep(0.05)
    tap(vk)
    user32.keybd_event(mod, 0, KEYUP, 0)


def drag(x0, y0, x1, y1):
    user32.SetCursorPos(x0, y0)
    time.sleep(0.15)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.08)
    steps = 12
    for i in range(1, steps + 1):
        user32.SetCursorPos(int(x0 + (x1 - x0) * i / steps), int(y0 + (y1 - y0) * i / steps))
        time.sleep(0.02)
    time.sleep(0.12)
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(0.25)


# ---------------------------------------------------------------- 主流程

def ensure_app() -> int:
    hwnd = find("AssetManager")
    if hwnd:
        return hwnd
    if not os.path.isfile(EXE):
        raise SystemExit(f"没找到安装版：{EXE}")
    subprocess.Popen([EXE], cwd=os.path.dirname(EXE))
    for _ in range(40):
        time.sleep(0.5)
        hwnd = find("AssetManager")
        if hwnd:
            time.sleep(2.0)  # 等首屏渲染完（后台还在扫库）
            return hwnd
    raise SystemExit("启动后没等到主窗口")


def snip_open(hwnd) -> bool:
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.4)
    tap(VK_F1)
    for _ in range(20):
        time.sleep(0.25)
        if find("截图"):
            return True
    return False


def main():
    os.makedirs(TMP, exist_ok=True)
    hwnd = ensure_app()
    print("主窗口已就绪")

    # ---- 场景 1：保存不应该动剪贴板
    clip_set_text(MARK)
    time.sleep(0.2)
    if clip_text() != MARK:
        raise SystemExit("前置失败：剪贴板里写不进标记文本")

    if not snip_open(hwnd):
        raise SystemExit("按 F1 后没等到截图覆盖层")
    time.sleep(0.8)
    drag(700, 420, 1000, 680)
    time.sleep(0.4)
    print("框选完成，按 Ctrl+S 保存 …")
    chord(0x11, ord("S"))
    time.sleep(3.0)

    left_text = clip_text()
    left_img = clip_has_image()
    print(f"   保存后：剪贴板文本={left_text!r} 有位图={left_img}")
    ok_save = left_text == MARK and not left_img
    print("   " + ("✅ 保存没有动剪贴板" if ok_save else "❌ 保存把剪贴板换掉了"))

    # ---- 场景 2：显式 Ctrl+C 仍然要复制成图
    clip_set_text("AMCLIP-PHASE2")
    time.sleep(0.2)
    if not snip_open(hwnd):
        raise SystemExit("第二次按 F1 没等到覆盖层")
    time.sleep(0.8)
    drag(700, 420, 1000, 680)
    time.sleep(0.4)
    print("框选完成，按 Ctrl+C 复制 …")
    chord(0x11, ord("C"))
    time.sleep(3.0)
    ok_copy = clip_has_image()
    print(f"   复制后：剪贴板格式 DIB={ok_copy}")
    print("   " + ("✅ Ctrl+C 仍然复制成图" if ok_copy else "❌ Ctrl+C 没复制图片"))

    print("结论：" + ("两项都过" if ok_save and ok_copy else "有失败项"))
    return 0 if (ok_save and ok_copy) else 1


if __name__ == "__main__":
    sys.exit(main())
