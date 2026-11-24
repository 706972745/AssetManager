#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""软件内自动更新的**真实链路**验证（自测用，不进产品）。

验的是「装着的旧版能不能把自己升上去」，不是 mock：
  1. 起一个指定 exe（带 AM_CDP=1），连 CDP；
  2. `am_update_check` → 必须能拿到 latest，且 `blocked` 为空（旧版的 bug 就死在这一步）；
  3. `am_update_download` → 落盘 + SHA-256 校验；
  4. `am_update_install` → 程序自己退出、PowerShell 等它退出后跑 `/S` 安装、再拉新版；
  5. 轮询目标 exe 的 FileVersion，等于远端版本才算成功（这才叫"升上去了"）。

用法：
    python tools/verify_update.py "D:/AssetManager/assetmanager.exe"            # 跑完整链路
    python tools/verify_update.py "D:/AssetManager/assetmanager.exe" --check    # 只看到哪一步
"""

import ctypes
import json
import os
import subprocess
import sys
import time
import urllib.request

PORT = 9222
GWL_EXSTYLE = -20


def targets():
    for _ in range(120):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json", timeout=1) as r:
                return json.load(r)
        except Exception:
            time.sleep(0.5)
    raise SystemExit("✗ CDP 端口没起来")


ROLE_JS = """(() => {
  if (window.__AM_BOARD__) return 'board';
  if (window.__AM_PEEK__) return 'peek';
  if (document.querySelector('.app-frame')) return 'main';
  return 'other';
})()"""


def find_main(timeout=45.0):
    """按角色找主窗：自定义协议下 url 全是 `http://tauri.localhost/`，
    标题又和托盘/闪屏重名，只能问页面自己。"""
    end = time.time() + timeout
    while time.time() < end:
        for t in targets():
            if t.get("type") != "page":
                continue
            try:
                c = Cdp(t)
                if c.eval(ROLE_JS) == "main":
                    return c
            except Exception:
                continue
        time.sleep(0.5)
    return None


class Cdp:
    def __init__(self, target):
        import websocket
        self.ws = websocket.create_connection(target["webSocketDebuggerUrl"], timeout=30)
        self.n = 0

    def eval(self, expr, timeout=60):
        self.n += 1
        self.ws.send(json.dumps({
            "id": self.n, "method": "Runtime.evaluate",
            "params": {"expression": expr, "awaitPromise": True, "returnByValue": True},
        }))
        self.ws.settimeout(timeout)
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == self.n:
                res = msg.get("result", {})
                if "exceptionDetails" in res:
                    raise RuntimeError(json.dumps(res["exceptionDetails"], ensure_ascii=False)[:600])
                return res.get("result", {}).get("value")


def invoke(cdp, cmd, args=None, timeout=120):
    a = json.dumps(args or {})
    return cdp.eval(f"window.__TAURI_INTERNALS__.invoke({json.dumps(cmd)}, {a})", timeout)


def file_version(exe):
    """读 exe 的 FileVersion（纯 ctypes，不依赖 PowerShell）。"""
    ver = ctypes.windll.version
    size = ver.GetFileVersionInfoSizeW(ctypes.c_wchar_p(exe), None)
    if not size:
        return ""
    buf = ctypes.create_string_buffer(size)
    if not ver.GetFileVersionInfoW(ctypes.c_wchar_p(exe), 0, size, buf):
        return ""
    p = ctypes.c_void_p()
    ln = ctypes.c_uint()
    if not ver.VerQueryValueW(buf, ctypes.c_wchar_p("\\"), ctypes.byref(p), ctypes.byref(ln)):
        return ""
    data = ctypes.cast(p, ctypes.POINTER(ctypes.c_uint32 * 4)).contents
    ms, ls = data[2], data[3]
    return f"{ms >> 16}.{ms & 0xFFFF}.{ls >> 16}.{ls & 0xFFFF}"


def main():
    exe = os.path.abspath(sys.argv[1])
    only_check = "--check" in sys.argv
    print(f"· 目标 {exe}")
    print(f"· 当前 FileVersion = {file_version(exe) or '(读不到)'}")

    subprocess.run(["taskkill", "/IM", os.path.basename(exe), "/F"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.2)
    env = dict(os.environ, AM_CDP="1")
    proc = subprocess.Popen([exe], env=env, cwd=os.path.dirname(exe))
    try:
        cdp = find_main()
        if not cdp:
            raise SystemExit("✗ 主窗 CDP target 没起来")
        time.sleep(2.5)

        print("\n【1】检查更新")
        info = invoke(cdp, "am_update_check", {"force": True})
        brief = {k: v for k, v in info.items() if k != "notes"}
        print("   " + json.dumps(brief, ensure_ascii=False)[:400])
        latest = info.get("latest") or ""
        blocked = info.get("blocked") or ""
        if blocked:
            print(f"   ✗ 被拒：blocked={blocked} —— 这就是「升不上去」的原因")
            return 1
        if not latest:
            print("   ✗ 没拿到远端版本号")
            return 1
        print(f"   ✓ latest={latest}，blocked 为空")
        if only_check:
            return 0

        print("\n【2】下载")
        print(f"   installKind={info.get('installKind')} canAutoInstall={info.get('canAutoInstall')}")
        path = invoke(cdp, "am_update_download", {
            "url": info["setupUrl"], "name": info["setupName"],
            "sha256": info["sha256"], "assetId": info.get("setupAssetId") or 0,
        }, timeout=900)
        print(f"   ✓ 落盘 {path}")

        print("\n【3】安装（本进程会自己退出）")
        try:
            invoke(cdp, "am_update_install", {
                "path": path, "sha256": info["sha256"], "version": latest,
            }, timeout=20)
        except Exception as e:
            print(f"   （预期内：进程退出导致连接断开 {type(e).__name__}）")

        print("\n【4】等它自己装完")
        want = latest.lstrip("v")
        end = time.time() + 420
        got = ""
        while time.time() < end:
            time.sleep(3)
            got = file_version(exe)
            if got.startswith(want):
                break
        if got.startswith(want):
            print(f"   ✓ 装完：{exe} 现在是 {got}（期望 {want}）")
            log = os.path.join(os.environ.get("TEMP", "."), "AssetManager-update", "apply-update.log")
            if os.path.exists(log):
                print("   · apply-update.log 末尾：")
                for line in open(log, encoding="utf-8", errors="replace").read().splitlines()[-8:]:
                    print("     " + line)
            return 0
        print(f"   ✗ 没升上去：{exe} 仍是 {got or '(读不到)'}，期望 {want}")
        return 1
    finally:
        try:
            if proc.poll() is None:
                proc.terminate()
        except Exception:
            pass


if __name__ == "__main__":
    sys.exit(main())
