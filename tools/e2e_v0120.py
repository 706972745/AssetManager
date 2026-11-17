"""0.1.20 验收：框选后按 Enter 不该结束截图，且必须能接着画标注。

流程：F1 → 框选 → Enter（应无反应）→ R 矩形工具 → 拖一条 → 抓图 → Ctrl+S 保存。
日志里 `snip finish action=save from=ctrl+s` 是最后一环的证据。
"""
import glob
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import e2e_noclip as E  # noqa: E402
from PIL import ImageGrab  # noqa: E402

SHOTS = os.path.join(os.environ["LOCALAPPDATA"], "AssetManager", "screenshots")
LOG = os.path.join(os.environ["LOCALAPPDATA"], "com.assetmanager.desktop", "runtime.log")
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "_tmp")


def main() -> int:
    os.makedirs(OUT, exist_ok=True)
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)
    time.sleep(1.5)
    try:
        log0 = os.path.getsize(LOG)
    except OSError:
        log0 = 0

    hwnd = E.ensure_app()
    time.sleep(2.0)
    ImageGrab.grab(all_screens=True).save(os.path.join(OUT, "v0120-main.png"))

    E.clip_set_text("MARK0120")
    E.user32.SetForegroundWindow(hwnd)
    time.sleep(0.4)
    E.tap(E.VK_F1)
    for _ in range(24):
        time.sleep(0.25)
        if E.find("截图"):
            break
    time.sleep(1.2)
    if not E.find("截图"):
        print("FAIL 覆盖层没打开")
        return 1

    E.drag(700, 420, 1020, 700)
    time.sleep(0.8)
    print("框选完成：覆盖层", "在" if E.find("截图") else "已关")

    E.tap(0x0D)  # Enter
    time.sleep(2.2)
    still = E.find("截图") is not None
    print(f"按 Enter 后：覆盖层={'在' if still else '已关'}  剪贴板文本={E.clip_text()!r} 位图={E.clip_has_image()}")

    # 矩形工具 + 拖一条
    E.tap(ord("R"))
    time.sleep(0.5)
    E.drag(760, 480, 950, 620)
    time.sleep(0.8)
    ImageGrab.grab(all_screens=True).save(os.path.join(OUT, "v0120-rect.png"))

    before = set(glob.glob(os.path.join(SHOTS, "*.png")))
    E.chord(0x11, ord("S"))
    time.sleep(3.5)
    after = set(glob.glob(os.path.join(SHOTS, "*.png")))
    new = sorted(os.path.basename(p) for p in after - before)
    print(f"Ctrl+S 后：新文件={new or '无'}  覆盖层={'在' if E.find('截图') else '已关'}")

    try:
        with open(LOG, "r", encoding="utf-8", errors="replace") as f:
            f.seek(log0)
            tail = [ln.rstrip() for ln in f.readlines()]
        print("--- 日志 ---")
        for ln in tail[-8:]:
            print(" ", ln)
    except OSError:
        pass

    if E.find("截图"):
        E.tap(E.VK_ESC)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
