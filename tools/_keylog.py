"""诊断：截图覆盖层「自动复制」时，Ctrl+C 到底是哪来的。

装 WH_KEYBOARD_LL 记录每一次真实 keydown（含 INJECTED 标记与当时前台窗口标题）。
两个阶段：
  A) 空转 30 秒（不碰截图）—— 看机器上是否有别的东西在乱发 Ctrl+C；
  B) 两轮「F1 → 框选 → 干等 8 秒」—— 看自动复制那一刻前后有没有真实按键。

用法（bash 启动）：
    python tools/_keylog.py
"""

import ctypes
import os
import subprocess
import sys
import threading
import time
from ctypes import wintypes

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import e2e_noclip as E  # noqa: E402

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

WH_KEYBOARD_LL = 13
WM_KEYDOWN, WM_SYSKEYDOWN = 0x100, 0x104


class KBDLLHOOKSTRUCT(ctypes.Structure):
    _fields_ = [
        ("vkCode", wintypes.DWORD),
        ("scanCode", wintypes.DWORD),
        ("flags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.c_void_p),
    ]


HOOKPROC = ctypes.WINFUNCTYPE(ctypes.c_ssize_t, ctypes.c_int, wintypes.WPARAM, wintypes.LPARAM)

user32.SetWindowsHookExW.argtypes = [ctypes.c_int, HOOKPROC, ctypes.c_void_p, wintypes.DWORD]
user32.SetWindowsHookExW.restype = ctypes.c_void_p
user32.CallNextHookEx.argtypes = [ctypes.c_void_p, ctypes.c_int, wintypes.WPARAM, wintypes.LPARAM]
user32.CallNextHookEx.restype = ctypes.c_ssize_t
user32.GetMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), ctypes.c_void_p, wintypes.UINT, wintypes.UINT]

EVENTS = []
LOCK = threading.Lock()
PROC_REF = None


def fg_title():
    h = user32.GetForegroundWindow()
    n = user32.GetWindowTextLengthW(h)
    if not n:
        return "<无标题>"
    b = ctypes.create_unicode_buffer(n + 1)
    user32.GetWindowTextW(h, b, n + 1)
    return b.value


def _proc(nCode, wParam, lParam):
    if nCode == 0 and wParam in (WM_KEYDOWN, WM_SYSKEYDOWN):
        info = ctypes.cast(lParam, ctypes.POINTER(KBDLLHOOKSTRUCT)).contents
        with LOCK:
            EVENTS.append((time.time(), info.vkCode, info.flags, fg_title()))
    return user32.CallNextHookEx(None, nCode, wParam, lParam)


def hook_thread():
    global PROC_REF
    PROC_REF = HOOKPROC(_proc)
    h = user32.SetWindowsHookExW(WH_KEYBOARD_LL, PROC_REF, None, 0)
    if not h:
        print("[钩子] 装不上 err=", ctypes.get_last_error())
        return
    print("[钩子] 已装")
    msg = wintypes.MSG()
    while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
        user32.TranslateMessage(ctypes.byref(msg))
        user32.DispatchMessageW(ctypes.byref(msg))


NAMES = {0x11: "Ctrl", 0x43: "C", 0x53: "S", 0x50: "P", 0x41: "A", 0x70: "F1", 0x1B: "Esc", 0x0D: "Enter"}


def dump(t0, tag):
    with LOCK:
        rows = [r for r in EVENTS if r[0] >= t0]
    print(f"--- {tag}：{len(rows)} 次 keydown ---")
    for ts, vk, flags, title in rows:
        mark = " [INJECTED]" if flags & 0x10 else ""
        print(f"  {ts - t0:6.2f}s  {NAMES.get(vk, hex(vk)):6} fg={title!r}{mark}")


def session(idx):
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)
    time.sleep(1.2)
    hwnd = E.ensure_app()
    E.user32.SetForegroundWindow(hwnd)
    time.sleep(0.4)
    t0 = time.time()
    E.tap(E.VK_F1)
    for _ in range(24):
        time.sleep(0.25)
        if E.find("截图"):
            break
    time.sleep(1.2)
    E.drag(700, 420, 1060, 740)
    print(f"[第 {idx} 轮] 框选完，干等 8 秒")
    time.sleep(8.0)
    still = E.find("截图")
    print(f"[第 {idx} 轮] 覆盖层={'在' if still else '已关'} 位图={E.clip_has_image()}")
    dump(t0, f"第 {idx} 轮按键")
    if still:
        E.tap(E.VK_ESC)
        time.sleep(1.0)


def main():
    threading.Thread(target=hook_thread, daemon=True).start()
    time.sleep(1.0)
    print("=== A) 空转 30 秒：期间请不要碰键盘 ===")
    t = time.time()
    time.sleep(30)
    dump(t, "空转")
    print("=== B) 两轮截图（脚本自己发 F1/鼠标，请也别碰键盘） ===")
    for i in (1, 2):
        session(i)


if __name__ == "__main__":
    main()
