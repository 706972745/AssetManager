#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""真机自测：**拖出窗口再拖回来松手，不能把原文件写进剪贴板**。

owner 2026-11-21 连着两次报「拖动图片松开还是可以复制这个文件，我不需要他复制」。
0.4.13 把 click 拦截器装在 `am://drag-end` / drag-drop 事件上，自测先 emit 再 click 能过，
真机照旧 —— OLE 一落地渲染进程先补发 click，IPC 事件晚几毫秒。0.4.14 改成常驻闸门
（按下瓦片就压下去，松手后 300ms 放行）。

判据（脚本自己跑完给结论，退出码 0 = 全过）：
  P1 bug 场景：多选 2 项 → 批量条出现 → 从瓦片拖出窗口边 → 拖回「复制原文件」按钮上松手
     → 剪贴板**不能**有 CF_HDROP，也不能弹「已复制原文件」
  P2 对照组：700ms 后正常点同一个按钮 → 剪贴板**必须**出现 CF_HDROP（没被误伤）

用法：
  python tools/e2e_drag_copy.py            # 自己起 app（AM_CDP=1，端口 9222 取元素坐标）
  python tools/e2e_drag_copy.py --keep     # 跑完不关 app

依赖：系统 python 的 websocket-client（只用来读元素坐标，不是业务依赖）
"""

import ctypes
import json
import os
import subprocess
import sys
import time
import urllib.request
from ctypes import wintypes

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32
user32.SetProcessDPIAware()

HERE = os.path.dirname(os.path.abspath(__file__))
EXE = os.environ.get("AM_EXE") or os.path.join(
    os.environ.get("LOCALAPPDATA", ""), "AssetManager", "assetmanager.exe")
CF_HDROP = 15
CF_UNICODETEXT = 13
MARK = "AMDRAGCHK"

user32.GetClipboardData.restype = ctypes.c_void_p
user32.GetClipboardData.argtypes = [ctypes.c_uint]
user32.SetClipboardData.restype = ctypes.c_void_p
user32.SetClipboardData.argtypes = [ctypes.c_uint, ctypes.c_void_p]
kernel32.GlobalAlloc.restype = ctypes.c_void_p
kernel32.GlobalAlloc.argtypes = [ctypes.c_uint, ctypes.c_size_t]
kernel32.GlobalLock.restype = ctypes.c_void_p
kernel32.GlobalLock.argtypes = [ctypes.c_void_p]
kernel32.GlobalUnlock.argtypes = [ctypes.c_void_p]

# ---------------------------------------------------------------- 剪贴板


def clip_set_text(s: str) -> None:
    """写一段文本 —— 顺手把剪贴板里所有格式（含 CF_HDROP）清掉"""
    user32.OpenClipboard(None)
    try:
        user32.EmptyClipboard()
        data = (s + "\0").encode("utf-16-le")
        h = kernel32.GlobalAlloc(0x0002, len(data))
        p = kernel32.GlobalLock(h)
        ctypes.memmove(p, data, len(data))
        kernel32.GlobalUnlock(h)
        user32.SetClipboardData(CF_UNICODETEXT, h)
    finally:
        user32.CloseClipboard()


def clip_has_files() -> bool:
    return bool(user32.IsClipboardFormatAvailable(CF_HDROP))


# ---------------------------------------------------------------- 窗口

def find_window(title: str) -> int:
    exact, loose = [], []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        n = user32.GetWindowTextLengthW(hwnd)
        if n == 0:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        if buf.value == title:
            exact.append(hwnd)
        elif title in buf.value:
            loose.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return (exact or loose or [0])[0]


def client_origin(hwnd: int):
    """客户区左上角的**屏幕物理**坐标 + 客户区尺寸"""
    pt = wintypes.POINT(0, 0)
    user32.ClientToScreen(hwnd, ctypes.byref(pt))
    rc = wintypes.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(rc))
    return (pt.x, pt.y, rc.right - rc.left, rc.bottom - rc.top)


# ---------------------------------------------------------------- 输入

INPUT_MOUSE = 0
MOUSEEVENTF_MOVE = 0x0001
MOUSEEVENTF_ABSOLUTE = 0x8000
MOUSEEVENTF_VIRTUALDESK = 0x4000
MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
MOUSEEVENTF_RIGHTDOWN = 0x0008
MOUSEEVENTF_RIGHTUP = 0x0010


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", ctypes.c_long), ("dy", ctypes.c_long),
                ("mouseData", ctypes.c_ulong), ("dwFlags", ctypes.c_ulong),
                ("time", ctypes.c_ulong), ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong))]


class INPUT(ctypes.Structure):
    class _U(ctypes.Union):
        _fields_ = [("mi", MOUSEINPUT)]

    _anonymous_ = ("u",)
    _fields_ = [("type", ctypes.c_ulong), ("u", _U)]


def _send(flags: int, x: int = 0, y: int = 0) -> None:
    vx = user32.GetSystemMetrics(76)
    vy = user32.GetSystemMetrics(77)
    vw = user32.GetSystemMetrics(78)
    vh = user32.GetSystemMetrics(79)
    ax = int(round((x - vx) * 65535 / max(vw - 1, 1)))
    ay = int(round((y - vy) * 65535 / max(vh - 1, 1)))
    inp = INPUT(type=INPUT_MOUSE)
    inp.mi = MOUSEINPUT(ax, ay, 0, flags, 0, None)
    user32.SendInput(1, ctypes.byref(inp), ctypes.sizeof(INPUT))


def move_to(x: int, y: int) -> None:
    _send(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, x, y)


def mouse_down(x: int, y: int) -> None:
    move_to(x, y)
    time.sleep(0.02)
    _send(MOUSEEVENTF_LEFTDOWN | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, x, y)


def mouse_up(x: int, y: int) -> None:
    _send(MOUSEEVENTF_LEFTUP | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, x, y)


def click(x: int, y: int) -> None:
    mouse_down(x, y)
    time.sleep(0.05)
    mouse_up(x, y)


def ctrl_click(x: int, y: int) -> None:
    user32.keybd_event(0x11, 0, 0, 0)
    time.sleep(0.05)
    click(x, y)
    time.sleep(0.05)
    user32.keybd_event(0x11, 0, 2, 0)


def glide(x0: int, y0: int, x1: int, y1: int, steps: int = 14, hold: bool = True) -> None:
    for i in range(1, steps + 1):
        move_to(int(x0 + (x1 - x0) * i / steps), int(y0 + (y1 - y0) * i / steps))
        time.sleep(0.03 if hold else 0.01)


# ---------------------------------------------------------------- CDP

def cdp_page(timeout: float = 30.0):
    end = time.time() + timeout
    while time.time() < end:
        try:
            req = urllib.request.Request("http://127.0.0.1:9222/json",
                                         headers={"Host": "127.0.0.1:9222"})
            with urllib.request.urlopen(req, timeout=2) as r:
                pages = [t for t in json.load(r) if t.get("type") == "page"]
            if pages:
                return pages[0]
        except Exception:
            pass
        time.sleep(0.4)
    raise RuntimeError("CDP 端口没起来（AM_CDP=1 才开；release 版需要 devtools 特性）")

class Js:
    def __init__(self, ws_url: str):
        import websocket  # websocket-client

        self.ws = websocket.create_connection(ws_url, timeout=10)
        self.i = 0

    def __call__(self, expr: str):
        self.i += 1
        self.ws.send(json.dumps({"id": self.i, "method": "Runtime.evaluate",
                                 "params": {"expression": expr, "returnByValue": True,
                                            "awaitPromise": True}}))
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == self.i:
                r = msg.get("result", {})
                if "exceptionDetails" in r:
                    raise RuntimeError(r["exceptionDetails"].get("text"))
                return r.get("result", {}).get("value")

    def close(self):
        try:
            self.ws.close()
        except Exception:
            pass


# ---------------------------------------------------------------- 场景

def cdp_alive(timeout: float = 1.5) -> bool:
    try:
        req = urllib.request.Request("http://127.0.0.1:9222/json",
                                     headers={"Host": "127.0.0.1:9222"})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status == 200
    except Exception:
        return False


def ensure_app(keep: bool) -> int:
    """起一个**干净**的实例。

    两个坑（都踩过）：
    ① 单实例锁：旧进程没退干净时，新进程会转交一下立刻退出，窗口留在老进程上；
    ② CDP 端口是旧进程的：连上去看到的是**上一轮**的 DOM（toast 都是旧的），
       会把"上一轮复制过"误判成这一轮复制的。所以必须等端口真的消失再起。
    """
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)
    for _ in range(30):                     # 等窗口消失 + 9222 端口消失
        time.sleep(0.5)
        if not find_window("AssetManager") and not cdp_alive(0.4):
            break
    else:
        raise RuntimeError("旧实例没退干净（窗口或 9222 还在）")
    time.sleep(2.0)                         # WebView2 的 user-data-dir 锁要几秒才松
    env = dict(os.environ, AM_CDP="1")
    subprocess.Popen([EXE], env=env, close_fds=True)
    hwnd = 0
    for _ in range(40):
        time.sleep(0.5)
        hwnd = find_window("AssetManager")
        if hwnd:
            break
    if not hwnd:
        raise RuntimeError("AssetManager 窗口没起来")
    for _ in range(30):                     # 等 CDP 端口起来（新进程的）
        time.sleep(0.5)
        if cdp_alive(0.6):
            break
    time.sleep(2.0)
    bring_front(hwnd)
    time.sleep(0.6)
    return hwnd


LAYOUT = r"""(() => {
  const tiles = [...document.querySelectorAll('.tile[data-asset-id]')]
    .map(el => { const r = el.getBoundingClientRect();
      return {id: el.dataset.assetId, x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height}; })
    .filter(t => t.w > 40 && t.h > 40 && t.y > 80 && t.y < innerHeight - 120);
  const bar = document.querySelector('.batch-bar');
  let btn = null;
  if (bar) {
    const b = [...bar.querySelectorAll('button')].find(b => b.textContent.trim() === '复制原文件');
    if (b) { const r = b.getBoundingClientRect();
      btn = {x: r.left + r.width / 2, y: r.top + r.height / 2}; }
  }
  const toast = [...document.querySelectorAll('.toast, [class*="toast"]')]
    .map(e => e.innerText.replace(/\s+/g, ' ').trim()).filter(Boolean);
  return {tiles, bar: !!bar, btn, toast, w: innerWidth, h: innerHeight, dpr: devicePixelRatio};
})()
"""


def bring_front(hwnd: int) -> None:
    """抢前台：SetForegroundWindow 在别的进程占着前台时会被系统拒，
    先按一下 Alt（拿到输入权）再抢，Windows 就放行了。"""
    if user32.IsIconic(hwnd):
        user32.ShowWindow(hwnd, 9)
    user32.keybd_event(0x12, 0, 0, 0)
    user32.keybd_event(0x12, 0, 2, 0)
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.4)


def E_ensure(keep: bool, attach: bool) -> int:
    if attach:
        hwnd = find_window("AssetManager")
        if not hwnd:
            raise RuntimeError("--attach 但没找到 AssetManager 窗口")
        bring_front(hwnd)
        return hwnd
    return ensure_app(keep)


def main() -> int:
    keep = "--keep" in sys.argv
    attach = "--attach" in sys.argv      # 已经用 AM_CDP=1 起好了 app，不再自己拉起
    case = "bar"
    if "--case" in sys.argv:
        case = sys.argv[sys.argv.index("--case") + 1]
    hwnd = E_ensure(keep, attach)
    page = cdp_page()
    js = Js(page["webSocketDebuggerUrl"])
    boot_toasts = js("document.querySelectorAll('.toast').length")
    print(f"页面就绪，初始 toast 数 = {boot_toasts}（必须是 0，否则连到的是上一轮的窗口）")
    ox, oy, cw, ch = client_origin(hwnd)
    print(f"窗口客户区 {cw}x{ch} @ {ox},{oy}")

    lay = js(LAYOUT)
    if len(lay["tiles"]) < 2:
        print("!! 网格里没有可用的瓦片（先让索引跑完 / 换一个有图的目录）")
        return 2
    dpr = lay["dpr"] or 1.0
    to_screen = lambda p: (int(ox + p["x"] * dpr), int(oy + p["y"] * dpr))

    t0 = lay["tiles"][0]
    click(*to_screen(t0))
    time.sleep(0.6)
    lay = js(LAYOUT)      # 详情面板一开网格会重排，坐标要重取
    print(f"选中 {js('document.querySelectorAll(\".tile.sel\").length')} 个，瓦片 {len(lay['tiles'])} 张")

    if case == "short":
        # owner 2026-11-21：**拖一点点就松手**也会复制。窗口内短拖，落点还在瓦片上
        target_name = "窗口内短拖（不落任何按钮）"
        btn = None
    elif case == "bar":
        t1 = next((t for t in lay["tiles"] if t["id"] != t0["id"]), None)
        if not t1:
            print("!! 找不到第二张可点的瓦片")
            return 2
        ctrl_click(*to_screen(t1))
        time.sleep(1.0)
        lay = js(LAYOUT)
        print(f"Ctrl 点第 2 张后：选中 {js('document.querySelectorAll(\".tile.sel\").length')} 个，批量条={lay['bar']}")
        if not lay["bar"] or not lay["btn"]:
            print(f"!! 多选后没出现批量条（bar={lay['bar']} btn={lay['btn']}）")
            return 2
        target_name = "批量条「复制原文件」"
    else:
        # 单选：详情面板滚到底，拿「复制文件」（Chrome.tsx 操作区那个）
        js("(() => {const el=document.querySelector('.detail-inner'); if(el) el.scrollTop=el.scrollHeight; return 1})()")
        time.sleep(0.6)
        btn_css = js(r"""(() => {
          const b=[...document.querySelectorAll('.detail-inner button')]
            .find(x=>x.textContent.trim()==='复制文件');
          if(!b) return null; const r=b.getBoundingClientRect();
          return {x:r.left+r.width/2, y:r.top+r.height/2};
        })()""")
        if not btn_css:
            print("!! 详情面板里找不到「复制文件」按钮")
            return 2
        lay["btn"] = btn_css
        target_name = "详情面板「复制文件」"

    # ---- P1
    clip_set_text(MARK)
    time.sleep(0.3)
    if clip_has_files():
        print("!! 起点就不干净：剪贴板还有文件")
        return 2

    tile = to_screen(lay["tiles"][0])
    before = set(js(LAYOUT)["toast"])

    if case == "short":
        # 短拖：按下后只挪 30px 就松手，落点还在原来的瓦片上
        end = (tile[0] + 30, tile[1] + 22)
        print(f"短拖：{tile} → {end}（出窗 0 次）")
        mouse_down(*tile)
        time.sleep(0.2)
        glide(*tile, *end, steps=5)
        time.sleep(0.25)
        mouse_up(*end)
        btn = end
    else:
        btn = to_screen(lay["btn"])
        print(f"落点：{target_name} @ {btn}")
        wx, wy, ww, wh = ox, oy, cw, ch
        sw, sh = user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)
        out = None
        for c in ((wx - 90, tile[1]), (wx + ww // 2, wy - 90),
                  (wx + ww + 90, tile[1]), (wx + ww // 2, wy + wh + 90)):
            if 0 <= c[0] < sw and 0 <= c[1] < sh:
                out = (int(c[0]), int(c[1]))
                break
        if not out:
            print("!! 窗口外找不到落点（窗口贴边？）")
            return 2
        print(f"拖拽：瓦片 {tile} → 出窗 {out} → 回按钮 {btn}")
        mouse_down(*tile)
        time.sleep(0.25)
        glide(*tile, *out, steps=16)
        time.sleep(0.9)          # 在窗外停一下：确认已经交接给系统拖放
        mid = set(js(LAYOUT)["toast"]) - before
        if mid:
            print(f"   出窗期间新 toast = {sorted(mid)}")
        glide(*out, *btn, steps=18)
        time.sleep(0.35)
        mouse_up(*btn)

    # 三段采样：松手后立刻抓，再等一会抓，最后落定再抓
    time.sleep(0.15)
    s1 = clip_has_files()
    time.sleep(0.45)
    s2 = clip_has_files()
    time.sleep(1.0)
    s3 = clip_has_files()
    print(f"   剪贴板采样 0.15s={s1} 0.6s={s2} 1.6s={s3}")

    lay2 = js(LAYOUT)
    files = clip_has_files()
    toast_txt = " ".join(lay2["toast"])
    print(f"[P1] 剪贴板有文件={files}  toast={toast_txt!r}")
    p1 = (not files) and ("已复制" not in toast_txt)

    # ---- P2：正常点同一个按钮，必须能复制（没被误伤）
    time.sleep(0.8)
    click(*btn)
    time.sleep(1.4)
    files2 = clip_has_files()
    toast2 = " ".join(js(LAYOUT)["toast"])
    print(f"[P2] 剪贴板有文件={files2}  toast={toast2!r}")
    p2 = files2

    js.close()
    if not keep:
        subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)

    print()
    print(f"P1 拖拽松手不复制：{'PASS' if p1 else 'FAIL'}")
    print(f"P2 正常点击仍能复制：{'PASS' if p2 else 'FAIL'}")
    return 0 if (p1 and p2) else 1


if __name__ == "__main__":
    sys.exit(main())
