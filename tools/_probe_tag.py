"""探测：标签面板的右键菜单到底能不能弹（自测用，不进产品）。

用法：
    python tools/_probe_tag.py open     # 点「标签」头展开，截一张
    python tools/_probe_tag.py right    # 再对第一行标签右键，截一张

坐标基准：主窗客户区左上角（窗口是 decorations=false，GetWindowRect == 客户区）。
像素一律**物理**（DPR 1.25 下 CSS × 1.25）。
"""

import ctypes
import subprocess
import sys
import time
from ctypes import wintypes

sys.path.insert(0, "tools")
import shot  # noqa: E402

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()
TMP = "_tmp"
EXE = r"src-tauri/target/release/assetmanager.exe"


kernel32 = ctypes.windll.kernel32
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000


def exe_of(hwnd):
    """窗口属于哪个 exe —— 桌面上可能开着**同名的文件夹窗口**（标题也叫 AssetManager），
    只按标题找会截到资源管理器。"""
    pid = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    h = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid.value)
    if not h:
        return ""
    try:
        buf = ctypes.create_unicode_buffer(1024)
        n = wintypes.DWORD(1024)
        if kernel32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(n)):
            return buf.value.lower()
        return ""
    finally:
        kernel32.CloseHandle(h)


def find_app():
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(hwnd, _):
        n = user32.GetWindowTextLengthW(hwnd)
        if n:
            b = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(hwnd, b, n + 1)
            if (
                b.value.startswith("AssetManager")
                and user32.IsWindowVisible(hwnd)
                and exe_of(hwnd).endswith("assetmanager.exe")
            ):
                r = wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(r))
                if r.right - r.left > 600:  # 主窗，不是截图覆盖层
                    found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else 0


def ensure():
    h = find_app()
    if h:
        return h
    subprocess.Popen([EXE])
    for _ in range(40):
        time.sleep(0.5)
        h = find_app()
        if h:
            time.sleep(2.0)
            return h
    raise SystemExit("没等到主窗")


def rect(hwnd):
    r = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r.left, r.top, r.right - r.left, r.bottom - r.top


def click(x, y, button="left"):
    user32.SetCursorPos(int(x), int(y))
    time.sleep(0.15)
    if button == "right":
        user32.mouse_event(0x0008, 0, 0, 0, 0)  # RIGHTDOWN
        time.sleep(0.06)
        user32.mouse_event(0x0010, 0, 0, 0, 0)  # RIGHTUP
    else:
        user32.mouse_event(0x0002, 0, 0, 0, 0)
        time.sleep(0.05)
        user32.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(0.5)


def snap(name):
    """按窗口句柄抓（`shot.capture` 走 PrintWindow），不按标题找 ——
    桌面上可能开着同名的 AssetManager 文件夹窗口。"""
    hwnd = find_app()
    img = shot.capture(hwnd)
    img.save(f"{TMP}/{name}.png")
    print("SAVED", name, f"{img.size[0]}x{img.size[1]}")


def main():
    what = sys.argv[1] if len(sys.argv) > 1 else "open"
    hwnd = ensure()
    # 先点一下标题栏空白处：SetForegroundWindow 对非前台进程常常被拒，
    # 直接点内容会落到压在它上面的窗口上
    r0 = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r0))
    click(r0.left + 700, r0.top + 14)
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.6)
    print("前台窗口 =", user32.GetForegroundWindow(), "目标 =", hwnd, "一致" if user32.GetForegroundWindow() == hwnd else "不一致")
    left, top, w, h = rect(hwnd)
    print(f"窗口 {left},{top} {w}×{h}")

    # 「标签」头部：侧栏左侧、靠近底部（实测 dock.png 里在 y≈815 物理）
    tag_head = (left + 130, top + 815)
    if what == "open":
        click(*tag_head)
        time.sleep(0.6)
        snap("probe-open")
    else:
        # 面板已展开时，第一行标签在头部上方 ~40px 起
        click(*tag_head)  # 确保是开的状态（若已开会收起，所以下面再判断）
        time.sleep(0.6)
        snap("probe-before-right")
        for dy in (58, 83, 108):
            click(tag_head[0], tag_head[1] - dy, "right")
            time.sleep(0.6)
            snap(f"probe-right-{dy}")
            user32.keybd_event(0x1B, 0, 0, 0)
            user32.keybd_event(0x1B, 0, 2, 0)
            time.sleep(0.4)


if __name__ == "__main__":
    main()
