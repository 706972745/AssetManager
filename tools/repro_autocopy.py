"""复现「框选完立刻复制并结束截图」。

对每种「框选松手之后用户可能做的事」单独起一次会话，先往剪贴板写标记文本，
做完看剪贴板有没有被换、覆盖层还在不在。日志里的 `snip copy` 能确认走的是哪条分支。
"""
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import e2e_noclip as E  # noqa: E402

LOG = os.path.join(os.environ["LOCALAPPDATA"], "com.assetmanager.desktop", "runtime.log")


def log_size():
    try:
        return os.path.getsize(LOG)
    except OSError:
        return 0


def log_tail(since, n=12):
    try:
        with open(LOG, "r", encoding="utf-8", errors="replace") as f:
            f.seek(since)
            return [ln.rstrip() for ln in f.readlines()][-n:]
    except OSError:
        return []


def open_snip():
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)
    time.sleep(1.2)
    hwnd = E.ensure_app()
    E.user32.SetForegroundWindow(hwnd)
    time.sleep(0.4)
    E.tap(E.VK_F1)
    for _ in range(24):
        time.sleep(0.25)
        if E.find("截图"):
            break
    time.sleep(1.2)
    return E.find("截图") is not None


def drag_select():
    E.drag(700, 420, 1020, 700)
    time.sleep(0.8)


def report(name, since):
    still = E.find("截图")
    print(
        f"{name}: 剪贴板文本={E.clip_text()!r} 位图={E.clip_has_image()} "
        f"覆盖层={'在' if still else '已关'}"
    )
    for ln in log_tail(since):
        if "snip" in ln.lower():
            print("   log:", ln)
    if still:
        E.tap(E.VK_ESC)
        time.sleep(1.0)


def case(name, act):
    base = log_size()
    if not open_snip():
        print(f"{name}: 截图没打开，跳过")
        return
    E.clip_set_text("MARK-" + name)
    drag_select()
    act()
    time.sleep(4.0)
    report(name, base)


def nothing():
    pass


def one_click():
    E.user32.SetCursorPos(860, 560)
    time.sleep(0.2)
    E.user32.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.04)
    E.user32.mouse_event(0x0004, 0, 0, 0, 0)


def double_click():
    E.user32.SetCursorPos(860, 560)
    time.sleep(0.2)
    for _ in range(2):
        E.user32.mouse_event(0x0002, 0, 0, 0, 0)
        time.sleep(0.03)
        E.user32.mouse_event(0x0004, 0, 0, 0, 0)
        time.sleep(0.06)


if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    cases = {"none": ("只框选/松手", nothing), "click": ("框选+单击", one_click), "dbl": ("框选+双击", double_click)}
    for key, (label, fn) in cases.items():
        if which in ("all", key):
            case(label, fn)
