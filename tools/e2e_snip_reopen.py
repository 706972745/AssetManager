#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""回归：「F1 → 框选 → 右键返回 → 再按 F1」必须还能截（0.6.1 修的那个 bug）。

修之前：右键删掉最后一块选区时覆盖层不退出，会话一直活着，之后每次 F1 都回
「上一次截图还没结束」——用户判定"F1 坏了"。

为什么要脚本而不是手点：这个 bug 是**状态机**问题（原生会话的 busy 标志），
手点一次看不出来，要连续几十个来回才暴露。

做法：
  · 用 `am_snip_start`（就是 F1 走的同一条路）开截图；
  · 用真实 SendInput 鼠标事件框选、点工具、右键取消；
  · 每次右键之后立刻再开一次，把返回值记下来（Err 就是复现了）。
"""

import ctypes
import json
import os
import subprocess
import sys
import time
import urllib.request
from ctypes import wintypes

sys.path.insert(0, "tools")
import websocket

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()
EXE = r"D:\AssetManager\assetmanager.exe"

MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
MOUSEEVENTF_RIGHTDOWN = 0x0008
MOUSEEVENTF_RIGHTUP = 0x0010


def move(x, y):
    user32.SetCursorPos(int(x), int(y))
    time.sleep(0.05)


def click(btn, x, y):
    move(x, y)
    if btn == "L":
        user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
        time.sleep(0.04)
        user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
    else:
        user32.mouse_event(MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, 0)
        time.sleep(0.04)
        user32.mouse_event(MOUSEEVENTF_RIGHTUP, 0, 0, 0, 0)
    time.sleep(0.12)


def drag(x0, y0, x1, y1):
    move(x0, y0)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    steps = 8
    for i in range(1, steps + 1):
        move(x0 + (x1 - x0) * i / steps, y0 + (y1 - y0) * i / steps)
        time.sleep(0.03)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
    time.sleep(0.25)


def overlay_alive():
    """覆盖层的窗口类名是 A MSnipNative（`snip_native::cls`）。
    只在它存在的时候点鼠标 —— 否则会把点击打到别的软件上。"""
    h = user32.FindWindowW("AMSnipNative", None)
    return bool(h)


def targets():
    for _ in range(120):
        try:
            with urllib.request.urlopen("http://127.0.0.1:9222/json", timeout=1) as r:
                return json.load(r)
        except Exception:
            time.sleep(0.5)
    raise SystemExit("CDP 没起来")


class Cdp:
    def __init__(self, t):
        self.ws = websocket.create_connection(t["webSocketDebuggerUrl"], timeout=30)
        self.n = 0

    def eval(self, e):
        self.n += 1
        self.ws.send(json.dumps({"id": self.n, "method": "Runtime.evaluate",
                                 "params": {"expression": e, "awaitPromise": True,
                                            "returnByValue": True}}))
        while True:
            m = json.loads(self.ws.recv())
            if m.get("id") == self.n:
                r = m.get("result", {})
                if "exceptionDetails" in r:
                    return "ERR:" + json.dumps(r["exceptionDetails"], ensure_ascii=False)[:200]
                return r.get("result", {}).get("value")


def snip(cdp):
    """开一次截图，返回 'ok' 或错误文本。"""
    r = cdp.eval("(async () => { try { await window.__TAURI_INTERNALS__.invoke('am_snip_start');"
                 " return 'ok'; } catch (e) { return String(e); } })()")
    return r


def main():
    rounds = int(sys.argv[1]) if len(sys.argv) > 1 else 10
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.2)
    pj = r"D://AssetManager//data//peeks.json"
    if os.path.exists(pj):
        open(pj, "w", encoding="utf-8").write("[]")
    p = subprocess.Popen([EXE], env=dict(os.environ, AM_CDP="1"), cwd=os.path.dirname(EXE))
    fails = []
    try:
        cdp = None
        end = time.time() + 45
        while time.time() < end and not cdp:
            for t in targets():
                if t.get("type") != "page":
                    continue
                c = Cdp(t)
                if c.eval("!!document.querySelector('.app-frame')"):
                    cdp = c
                    break
            time.sleep(0.5)
        if not cdp:
            raise SystemExit("主窗没起来")
        time.sleep(3)

        for i in range(1, rounds + 1):
            r1 = snip(cdp)
            time.sleep(1.4)                       # 等覆盖层出来
            if not overlay_alive():
                print(f"  #{i:<2} 覆盖层没出来（开={r1}）")
                fails.append((i, "overlay missing after start: " + str(r1)))
                continue
            drag(700, 350, 1040, 640)             # 框一块
            time.sleep(0.3)
            click("L", 1100, 900)                  # 画布外点一下（去掉选区手柄焦点）
            time.sleep(0.2)
            click("R", 150, 1450)                  # 右键：移除当前块
            time.sleep(0.6)
            click("R", 150, 1450)                  # 右键：取消本次
            time.sleep(1.2)
            gone = not overlay_alive()
            print(f"      右键两次后覆盖层消失={gone}")
            r2 = snip(cdp)                         # 关键：立刻再开一次
            print(f"  #{i:<2} 开={r1:<6} 右键返回后再开={r2}")
            if r2 != "ok":
                fails.append((i, r2))
                # 卡住了也继续压几轮，看是不是永久
            time.sleep(1.3)
            # 收掉这次开的（Esc，且只在覆盖层在的时候发）
            if overlay_alive():
                user32.keybd_event(0x1B, 0, 0, 0)
                time.sleep(0.05)
                user32.keybd_event(0x1B, 0, 2, 0)
                time.sleep(1.0)
    finally:
        subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    print(f"\n失败 {len(fails)} 次 / 共 {rounds} 轮")
    for i, e in fails:
        print(f"  #{i}: {e}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
