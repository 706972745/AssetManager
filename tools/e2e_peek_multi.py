#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""回归：多张贴图时双击关掉一张，**其余贴图必须原样不动**（0.6.1 修的那个 bug）。

判据：每扇贴图窗里 `.peek-img` 的 src 带 asset id；关掉一张之后，剩下那几张的
asset id / 图尺寸都不能变；再贴一张新的，只有被复用的那扇窗换图。
修之前的表现：关一张 → 其余全变空白；再贴一张 → 所有贴图都变成新图。

用法：python tools/e2e_peek_multi.py

判据很硬：每扇贴图窗里 `.peek-img` 的 src 带着 asset id（`thumb.localhost/<id>`），
所以「显示不正确」= 关掉一张之后，剩下那几张的 id 变了 / 尺寸变了 / 窗口挪了。

用法：python _tmp/repro_peek_close.py
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

ROLE = """(() => {
  if (window.__AM_BOARD__) return 'board';
  if (window.__AM_PEEK__) return 'peek';
  if (document.querySelector('.app-frame')) return 'main';
  return 'other';
})()"""


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
        self.ws = websocket.create_connection(t["webSocketDebuggerUrl"], timeout=20)
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
                    raise RuntimeError(str(r["exceptionDetails"])[:300])
                return r.get("result", {}).get("value")


def peeks():
    """所有 peek 窗：label(经 boot.id) + 当前显示的 asset id + 图尺寸 + 窗口矩形"""
    out = []
    for t in targets():
        if t.get("type") != "page":
            continue
        try:
            c = Cdp(t)
            if c.eval(ROLE) != "peek":
                continue
            info = c.eval("""(() => {
              const img = document.querySelector('.peek-img');
              const root = document.querySelector('.peek-root');
              const lab = (window.__AM_PEEK__ && window.__AM_PEEK__.id) || '?';
              const src = img ? img.src : '';
              const m = src.match(/thumb\\.localhost\\/(\\d+)/);
              return JSON.stringify({
                label: lab,
                asset: m ? Number(m[1]) : 0,
                natural: img ? [img.naturalWidth, img.naturalHeight] : [0,0],
                box: img ? [Math.round(img.getBoundingClientRect().width),
                            Math.round(img.getBoundingClientRect().height)] : [0,0],
                opacity: root ? getComputedStyle(root).opacity : '',
                vis: document.visibilityState,
              });
            })()""")
            d = json.loads(info)
            d["_cdp"] = c
            out.append(d)
        except Exception:
            continue
    return out


def hwnd_of(label):
    for h, t in wins():
        if t == label:
            return h
    return None


def wins():
    out = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        n = user32.GetWindowTextLengthW(h)
        if n:
            b = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(h, b, n + 1)
            out.append((h, b.value))
        return True

    user32.EnumWindows(cb, 0)
    return out


def rect(h):
    r = wintypes.RECT()
    user32.GetWindowRect(h, ctypes.byref(r))
    return (r.left, r.top, r.right - r.left, r.bottom - r.top)


def dump(tag, rows):
    print(f"\n--- {tag} ---")
    for d in sorted(rows, key=lambda x: x["label"]):
        h = hwnd_of(d["label"])
        rr = rect(h) if h else None
        print(f"  {d['label']:<8} asset={d['asset']:<16} natural={d['natural']} box={d['box']} "
              f"win={rr} vis={d['vis']}")


def main():
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.2)
    # 清掉上次留下的贴图存档，从零开始
    pj = r"D:\AssetManager\data\peeks.json"
    if os.path.exists(pj):
        open(pj, "w", encoding="utf-8").write("[]")

    p = subprocess.Popen([EXE], env=dict(os.environ, AM_CDP="1"), cwd=os.path.dirname(EXE))
    try:
        main_cdp = None
        end = time.time() + 45
        while time.time() < end and not main_cdp:
            for t in targets():
                if t.get("type") != "page":
                    continue
                try:
                    c = Cdp(t)
                    if c.eval(ROLE) == "main":
                        main_cdp = c
                        break
                except Exception:
                    pass
            time.sleep(0.5)
        if not main_cdp:
            raise SystemExit("主窗没起来")
        time.sleep(2.5)

        roots = main_cdp.eval("window.__TAURI_INTERNALS__.invoke('am_roots').then(r=>JSON.stringify(r))")
        root = json.loads(roots)[0]["id"]
        rows = json.loads(main_cdp.eval(
            f"window.__TAURI_INTERNALS__.invoke('am_list',"
            f"{{q:{{dirId:{root},recursive:true,limit:6}}}}).then(r=>JSON.stringify(r))"))
        ids = [r["id"] for r in rows]
        print("库里前几个 id:", ids)
        if len(ids) < 3:
            raise SystemExit("素材不够 3 张，换个库再试")

        for i, aid in enumerate(ids[:3]):
            main_cdp.eval(f"window.__TAURI_INTERNALS__.invoke('am_peek_open',"
                          f"{{ids:[{aid}],index:0}})")
            time.sleep(1.6)
        dump("打开 3 张之后", peeks())

        cur = peeks()
        mid = sorted(cur, key=lambda x: x["label"])[1]
        print(f"\n>>> 双击关掉中间那张：{mid['label']} (asset={mid['asset']})")
        mid["_cdp"].eval("""(() => {
            const el = document.querySelector('.peek-root');
            el.dispatchEvent(new MouseEvent('dblclick', {bubbles:true}));
        })()""")
        time.sleep(2.0)
        dump("关掉一张之后", [d for d in peeks()])

        cur = [d for d in peeks()]
        if len(cur) >= 2:
            victim = sorted(cur, key=lambda x: x["label"])[1]
            print(f"\n>>> 再关一张（这次热备已被占用）：{victim['label']} (asset={victim['asset']})")
            victim["_cdp"].eval("""(() => {
                const el = document.querySelector('.peek-root');
                el.dispatchEvent(new MouseEvent('dblclick', {bubbles:true}));
            })()""")
            time.sleep(2.0)
            dump("关掉第二张之后", peeks())

        print("\n>>> 再贴一张新的（asset=%s）" % ids[3])
        main_cdp.eval(f"window.__TAURI_INTERNALS__.invoke('am_peek_open',{{ids:[{ids[3]}],index:0}})")
        time.sleep(2.0)
        dump("新贴一张之后", peeks())
        print(f"\n期望：剩下几张的 asset 不变；新贴的那张 asset={ids[3]}")
    finally:
        subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
