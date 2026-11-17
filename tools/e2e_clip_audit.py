#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""截图四种收尾方式的**剪贴板审计**（owner 报「截图完成后老是自动复制」）。

目的：把「保存 / 中键贴图 / Enter / 双击选区」逐条量一遍，看**哪一种**会写剪贴板。
每一条都先往剪贴板写一个标记文本，做完再看剪贴板是否被换掉。

用法：python tools/e2e_clip_audit.py
输出：终端一行一条结论；退出码 0 = 只有预期的那几种写了剪贴板。
"""

import ctypes
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import e2e_noclip as E  # 复用剪贴板与窗口工具

user32 = E.user32
VK_RETURN = 0x0D
VK_ESC = 0x1B

SEL = (700, 420, 1020, 700)  # 固定的选区（屏幕坐标）


def reset_clip(mark: str) -> None:
    E.clip_set_text(mark)
    time.sleep(0.25)


def snapshot_clip() -> tuple:
    """(文本, 是否位图)"""
    return (E.clip_text(), E.clip_has_image())


def open_snip(hwnd) -> bool:
    hwnd = E.find("AssetManager")
    if hwnd:
        user32.SetForegroundWindow(hwnd)
        time.sleep(0.35)
    E.tap(E.VK_F1)
    for _ in range(20):
        time.sleep(0.25)
        if E.find("截图"):
            return True
    return False


def close_snip_if_open():
    if E.find("截图"):
        E.tap(VK_ESC)
        time.sleep(1.0)


def run_case(hwnd, name: str, do_action) -> dict:
    close_snip_if_open()
    reset_clip(f"MARK-{name}")
    if not open_snip(hwnd):
        return {"case": name, "error": "没等到覆盖层"}
    time.sleep(0.9)
    x0, y0, x1, y1 = SEL
    E.drag(x0, y0, x1, y1)
    time.sleep(0.5)
    do_action(x0, y0, x1, y1)
    time.sleep(3.0)
    text, img = snapshot_clip()
    changed = text != f"MARK-{name}" or img
    close_snip_if_open()
    return {"case": name, "text": text, "image": img, "copied": changed}


def act_save_ctrl_s(x0, y0, x1, y1):
    E.chord(0x11, ord("S"))


def act_enter(x0, y0, x1, y1):
    E.tap(VK_RETURN)


def act_middle(x0, y0, x1, y1):
    """中键 = 贴图（Snipaste 同款），实测这条以前也塞过剪贴板"""
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    user32.SetCursorPos(cx, cy)
    time.sleep(0.2)
    user32.mouse_event(0x0020, 0, 0, 0, 0)  # MIDDLEDOWN
    time.sleep(0.06)
    user32.mouse_event(0x0040, 0, 0, 0, 0)  # MIDDLEUP


def act_double_click(x0, y0, x1, y1):
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    user32.SetCursorPos(cx, cy)
    time.sleep(0.2)
    for _ in range(2):
        user32.mouse_event(0x0002, 0, 0, 0, 0)
        time.sleep(0.03)
        user32.mouse_event(0x0004, 0, 0, 0, 0)
        time.sleep(0.06)


def main():
    # 先把所有实例关掉，保证测的就是 0.1.18 这一个
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)
    time.sleep(1.5)
    hwnd = E.ensure_app()
    print("已启动安装版（先杀掉全部旧实例）")

    cases = [
        ("保存CtrlS", act_save_ctrl_s),
        ("中键贴图", act_middle),
        ("Enter", act_enter),
        ("双击选区", act_double_click),
    ]
    rows = []
    for name, fn in cases:
        r = run_case(hwnd, name, fn)
        rows.append(r)
        if "error" in r:
            print(f"{name:10s}  ! {r['error']}")
        else:
            tag = "写了剪贴板" if r["copied"] else "没动剪贴板"
            print(f"{name:10s}  {tag}   文本={r['text']!r} 位图={r['image']}")

    bad = [r["case"] for r in rows if r.get("copied") and r["case"] in ("保存CtrlS", "中键贴图")]
    print("结论：" + ("保存/贴图都不再动剪贴板" if not bad else f"仍在复制：{bad}"))
    return 0 if not bad else 1


if __name__ == "__main__":
    sys.exit(main())
