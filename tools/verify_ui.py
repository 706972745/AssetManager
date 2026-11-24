#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""0.6.x 三项 UI 改动的实机验证（自测用，不进产品）。

验的是四件事：
  1. 灵感板平时是**箭头**光标，只有中键按住拖动才是抓手；
  2. 灵感板 / 贴图都有 **3px 紫色阴影**（读 computed style + 抓像素复核）；
  3. 灵感板 **不进任务栏**（WS_EX_TOOLWINDOW），点最小化后**隐藏**（IsWindowVisible=0），
     再从标签菜单开同一块板能把它叫回来；
  4. 贴图窗阴影同理，且窗口 = 图 + 2×glow（图仍然贴在选择区上）。

做法：用 `AM_CDP=1` 起程序（release 包也认这个环境变量），连 9222 端口的 CDP，
在**主窗**里直接 invoke `am_board_open` / `am_peek_open` —— 不靠点坐标，稳定且能读 DOM。

用法：
    python tools/verify_ui.py            # 全跑
    python tools/verify_ui.py --keep     # 跑完不停程序（截完图自己看）
退出码 0 = 全过。
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

import websocket  # websocket-client

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

EXE = os.path.abspath(
    next((a for a in sys.argv[1:] if a.lower().endswith(".exe")),
         "src-tauri/target/release/assetmanager.exe"))
PORT = 9222
GWL_EXSTYLE = -20
WS_EX_TOOLWINDOW = 0x00000080
WS_EX_APPWINDOW = 0x00040000

ok = []
bad = []


def check(name, cond, detail=""):
    (ok if cond else bad).append(name)
    print(("  [OK] " if cond else "  [FAIL] ") + name + (f"  —— {detail}" if detail else ""))
    return cond


# ---------------------------------------------------------------- CDP

def targets():
    for _ in range(120):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json", timeout=1) as r:
                return json.load(r)
        except Exception:
            time.sleep(0.5)
    raise SystemExit("✗ CDP 端口没起来（程序没启动或 AM_CDP 没生效）")


ROLE_JS = """(() => {
  if (window.__AM_BOARD__) return 'board';
  if (window.__AM_PEEK__) return 'peek';
  if (window.__AM_SNIP__) return 'snip';
  if (document.querySelector('.app-frame')) return 'main';
  if (document.body && /setup/.test(document.body.className)) return 'setup';
  return 'other';
})()"""


def find_role(role, timeout=40.0):
    """按**窗口角色**找 CDP target。

    不能按标题找：主窗、托盘菜单、闪屏的 title 都叫 AssetManager；
    也不能按 url 找：自定义协议下所有页面都是 `http://tauri.localhost/`。
    直接问页面自己（前端用 `__AM_BOARD__` / `__AM_PEEK__` 区分，主窗有 `.app-frame`）。
    """
    end = time.time() + timeout
    while time.time() < end:
        for t in targets():
            if t.get("type") != "page":
                continue
            try:
                c = Cdp(t, quiet=True)
                got = c.eval(ROLE_JS)
            except Exception:
                continue
            if got == role:
                c.quiet = False
                return c
            c.close()
        time.sleep(0.4)
    return None


class Cdp:
    def __init__(self, target, quiet=False):
        self.ws = websocket.create_connection(target["webSocketDebuggerUrl"], timeout=15)
        self.n = 0
        self.quiet = quiet

    def eval(self, expr):
        self.n += 1
        self.ws.send(json.dumps({
            "id": self.n,
            "method": "Runtime.evaluate",
            "params": {"expression": expr, "awaitPromise": True, "returnByValue": True},
        }))
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == self.n:
                res = msg.get("result", {})
                if "exceptionDetails" in res:
                    raise RuntimeError(json.dumps(res["exceptionDetails"], ensure_ascii=False)[:400])
                return res.get("result", {}).get("value")

    def close(self):
        try:
            self.ws.close()
        except Exception:
            pass


def invoke(cdp, cmd, args=None):
    a = json.dumps(args or {})
    return cdp.eval(f"window.__TAURI_INTERNALS__.invoke({json.dumps(cmd)}, {a})")


# ---------------------------------------------------------------- win32

def hwnd_by_title_contains(part):
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        n = user32.GetWindowTextLengthW(h)
        if n:
            buf = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(h, buf, n + 1)
            if part in buf.value:
                found.append((h, buf.value))
        return True

    user32.EnumWindows(cb, 0)
    return found


def exstyle(hwnd):
    return user32.GetWindowLongW(hwnd, GWL_EXSTYLE)


def visible(hwnd):
    return bool(user32.IsWindowVisible(hwnd))


def rect(hwnd):
    r = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r.left, r.top, r.right, r.bottom


def shot(path, hwnd):
    """抓窗口区域（ImageGrab 抓屏幕；PrintWindow 对 WebView2 会串画面，见 WORKLOG #84）。"""
    from PIL import ImageGrab
    x0, y0, x1, y1 = rect(hwnd)
    img = ImageGrab.grab(bbox=(x0, y0, x1, y1), all_screens=True)
    img.save(path)
    return img


def purple_ish(px):
    """紫：蓝分量 ≥ 红分量 ≥ 绿分量，且蓝比绿高出一点。

    判**色相**而不是亮度：3px 光晕落在白底上是很浅的淡紫（236,232,244），
    落在深色桌面上是很深的暗紫（25,21,34），同一层阴影两种亮度都要认。
    中性灰（230,230,230）蓝绿相等 → 不算紫。
    """
    r, g, b = px[0], px[1], px[2]
    return b >= r >= g and (b - g) >= 6


# ---------------------------------------------------------------- 主流程

def main():
    keep = "--keep" in sys.argv

    # 关掉已经开着的实例（单实例守卫会让新进程直接退出）
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.2)

    env = dict(os.environ, AM_CDP="1", AM_IPC_AUDIT="0")
    print(f"· 启动 {EXE}")
    proc = subprocess.Popen([EXE], env=env, cwd=os.path.dirname(EXE))
    try:
        # 先起 splash 再建主窗，所以要等主窗出现，不能只看第一次
        main_cdp = find_role("main", 45)
        if not main_cdp:
            with urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json", timeout=3) as r:
                print("  当前 targets: " + json.dumps(
                    [(t.get("type"), t.get("title"), t.get("url")) for t in json.load(r)],
                    ensure_ascii=False))
            raise SystemExit("✗ 找不到主窗的 CDP target")
        time.sleep(2.5)  # 等首屏把库读出来

        # ---------- 开一块板 ----------
        tags = invoke(main_cdp, "am_auto_tags") or []
        if not tags:
            tags = invoke(main_cdp, "am_tags") or []
        if not tags:
            raise SystemExit("✗ 库里一个标签都没有，造不出来板（先给素材打个标签）")
        tag = tags[0]["name"] if isinstance(tags[0], dict) else str(tags[0])
        print(f"· 用标签「{tag}」开板")
        invoke(main_cdp, "am_board_open", {"tag": tag, "title": f"灵感板 · {tag}"})
        b = find_role("board", 20)
        if not b:
            raise SystemExit("✗ 板窗没起来")
        time.sleep(1.2)

        print("\n【1】鼠标指示")
        cur = b.eval("getComputedStyle(document.querySelector('.board-root')).cursor")
        check("板窗默认光标是箭头", cur in ("default", "auto"), f"computed cursor={cur}")
        # 中键按下后应变成抓手（Board 用的是 onMouseDown，不是 pointerdown）
        b.eval("""(() => {
            const el = document.querySelector('.board-root');
            el.dispatchEvent(new MouseEvent('mousedown',
              {button:1, buttons:4, bubbles:true, clientX:200, clientY:150}));
        })()""")
        time.sleep(0.35)
        cur2 = b.eval("getComputedStyle(document.querySelector('.board-root')).cursor")
        check("中键按住时是抓手", cur2 in ("grabbing", "grab"), f"computed cursor={cur2}")
        b.eval("""(() => {
            const el = document.querySelector('.board-root');
            el.dispatchEvent(new MouseEvent('mouseup',
              {button:1, buttons:0, bubbles:true, clientX:200, clientY:150}));
            window.dispatchEvent(new MouseEvent('mouseup', {button:1, bubbles:true}));
        })()""")
        time.sleep(0.3)
        cur3 = b.eval("getComputedStyle(document.querySelector('.board-root')).cursor")
        check("松手后回到箭头", cur3 in ("default", "auto"), f"computed cursor={cur3}")

        print("\n【2】灵感板 3px 紫色阴影")
        shadow = b.eval("getComputedStyle(document.querySelector('.board-root')).boxShadow")
        check("板窗有 3px 阴影", "3px" in (shadow or ""), shadow)
        check("阴影是紫色", "167, 139, 250" in (shadow or "") or "108, 63, 209" in (shadow or ""),
              shadow)

        print("\n【3】任务栏 / 最小化")
        wins = hwnd_by_title_contains("灵感板")
        if not wins:
            check("找到板窗 HWND", False, "EnumWindows 没找到标题含「灵感板」的窗口")
        else:
            h, title = wins[0]
            ex = exstyle(h)
            check("板窗不在任务栏（WS_EX_TOOLWINDOW）", bool(ex & WS_EX_TOOLWINDOW),
                  f"exstyle=0x{ex:08X} appwindow={bool(ex & WS_EX_APPWINDOW)}")
            check("板窗窗口可见", visible(h), f"hwnd=0x{h:X}")
            # 顶栏最小化按钮：按 title 找（中英都认），找不到就退回"关闭前一个"
            clicked = b.eval("""(() => {
                const bs = [...document.querySelectorAll('.board-top button')];
                if (!bs.length) return 'no-buttons';
                const byTitle = bs.find((x) => /最小化|minimi/i.test(x.getAttribute('title') || ''));
                const btn = byTitle || bs[bs.length - 2] || bs[0];
                btn.click();
                return btn.getAttribute('title') || '(no title)';
            })()""")
            time.sleep(1.0)
            check("点最小化后窗口隐藏", not visible(h), f"点了「{clicked}」后 visible={visible(h)}")
            # 从标签菜单再开一次同一个标签 → 应该把窗叫回来
            invoke(main_cdp, "am_board_open", {"tag": tag, "title": f"灵感板 · {tag}"})
            time.sleep(1.2)
            check("重新开板能复原", visible(h), f"visible={visible(h)}")
            shot("_tmp/v-board.png", h)
            print("  · 板窗截图 → _tmp/v-board.png")

            # 像素复核：窗口最外 1~2px 应该是紫的
            img = shot("_tmp/v-board-px.png", h)
            w, hh = img.size
            samples = [img.getpixel((1, hh // 2)), img.getpixel((w - 2, hh // 2)),
                       img.getpixel((w // 2, 1)), img.getpixel((w // 2, hh - 2))]
            hit = [p for p in samples if purple_ish(p)]
            check("窗口边缘像素是紫的", len(hit) >= 2, f"四边采样={samples}")

        # ---------- 贴图 ----------
        print("\n【4】贴图窗阴影")
        roots = invoke(main_cdp, "am_roots") or []
        q = {"limit": 1}
        if roots:
            q["dirId"] = roots[0]["id"]
            q["recursive"] = True
        rows = invoke(main_cdp, "am_list", {"q": q}) or []
        if not rows:
            check("库里有素材", False, "am_list 空")
        else:
            aid = rows[0]["id"]
            invoke(main_cdp, "am_peek_open", {"ids": [aid], "index": 0})
            pk = find_role("peek", 20)
            if not pk:
                check("贴图窗起来了", False, "20s 内没有 peek 窗")
            else:
                time.sleep(1.2)
                shadow = pk.eval("getComputedStyle(document.querySelector('.peek-root')).boxShadow")
                glow = pk.eval("getComputedStyle(document.querySelector('.peek-root')).getPropertyValue('--peek-glow')")
                check("贴图有 3px 阴影", "3px" in (shadow or ""), f"{shadow} / glow={glow}")
                check("glow 已改成 3px", (glow or "").strip().startswith("3"),
                      f"--peek-glow={(glow or '').strip()}")
                # 只看**可见**的那个「看图」：库里可能还躺着一扇隐藏的热备窗
                pw = [(h, t) for h, t in hwnd_by_title_contains("看图") if visible(h)]
                if pw:
                    ph = pw[0][0]
                    img = shot("_tmp/v-peek.png", ph)
                    print("  · 贴图截图 → _tmp/v-peek.png")
                    # 贴图窗 = 图 + 2×3px：最外 1~2px 必须是紫的，往里才是图
                    w, hh = img.size
                    ring = [img.getpixel((1, hh // 2)), img.getpixel((w - 2, hh // 2))]
                    check("贴图外圈像素是紫的", any(purple_ish(p) for p in ring), f"采样={ring}")
                pk.close()

        b.close()
        main_cdp.close()
    finally:
        if keep:
            print("\n· --keep：程序留着，自己看。")
        else:
            subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    print(f"\n{'='*46}\n通过 {len(ok)} 项，失败 {len(bad)} 项")
    if bad:
        for x in bad:
            print("  ✗ " + x)
        return 1
    print("全过 ✓")
    return 0


if __name__ == "__main__":
    sys.exit(main())
