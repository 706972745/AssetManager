#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""截图四种收尾方式的**交付语义审计**（owner 反复报的「截图完成后老是自动复制」）。

0.1.20 口径（规范 §5.1 G3 / 测试文档 §8.2）：
  - 保存 Ctrl+S / 中键贴图：落盘 / 贴图，**都不许写剪贴板**；
  - Enter / 双击选区：**什么都不做** —— 既不许关覆盖层，也不许写剪贴板。
    这两条是 owner 三次报「框完就复制、没法加标注」的真凶（见 WORKLOG §32）。

仅检查剪贴板是不够的：Enter/双击若错误地"关窗但没复制"，剪贴板检查照样通过，
所以这里同时断言**覆盖层是否还在** —— 这是本脚本 0.1.20 修的那处"假绿"。

用法：python tools/e2e_clip_audit.py
输出：终端一行一条结论；退出码 0 = 四条全部符合 0.1.20 语义。
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
    # 0.1.20：Enter/双击必须"什么都不做"，所以要看覆盖层是否还在（关窗即交付/取消，都是错的）
    snip_open = E.find("截图") != 0
    close_snip_if_open()
    return {"case": name, "text": text, "image": img, "copied": changed, "snip_open": snip_open}


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
    # 先把所有实例关掉，保证测的就是当前安装版这一个
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)
    time.sleep(1.5)
    hwnd = E.ensure_app()
    print("已启动安装版（先杀掉全部旧实例）")

    cases = [
        # 保存改成「点按钮」之前先别测 Ctrl+S：0.1.25 起快捷键交付只认**真人**按键，
        # 脚本发的是注入键，一定被挡（见 tools/e2e_noauto.py）。那一条挪去做手测。
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
            win = "覆盖层在" if r["snip_open"] else "覆盖层已关"
            print(f"{name:10s}  {tag}   {win}   文本={r['text']!r} 位图={r['image']}")

    # 0.1.20 判据：四条全都不许写剪贴板；Enter/双击还必须保持覆盖层打开
    bad = []
    for r in rows:
        if "error" in r:
            bad.append(f"{r['case']}(没跑起来)")
            continue
        if r.get("copied"):
            bad.append(f"{r['case']}写了剪贴板")
        if r["case"] in ("Enter", "双击选区") and not r["snip_open"]:
            bad.append(f"{r['case']}关了覆盖层")
        if r["case"] == "中键贴图" and r["snip_open"]:
            bad.append(f"{r['case']}没关覆盖层")
    if bad:
        print("结论：不符合 0.1.20 语义 → " + " / ".join(bad))
        return 1
    print("结论：符合（中键贴图不写剪贴板并关窗；Enter/双击什么都不做）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
