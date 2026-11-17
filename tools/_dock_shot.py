"""看停靠面板的实际样子（自测用，不进产品）。

用法：
    python tools/_dock_shot.py             # 只截当前窗口
    python tools/_dock_shot.py coll        # 点一下「分组」头部（固定展开）后再截
    python tools/_dock_shot.py tag         # 点一下「标签」头部后再截
坐标按 CSS 像素 × DPR 换算成物理像素，基准点取窗口客户区左上角。
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
DPR = 1.25
EXE = r"src-tauri/target/release/assetmanager.exe"


def rect_of(hwnd):
    r = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r


kernel32 = ctypes.windll.kernel32
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000


def exe_of(hwnd):
    """取窗口所属进程的可执行文件名（区分「AssetManager 文件夹」和本软件）。"""
    pid = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    h = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid.value)
    if not h:
        return ""
    try:
        buf = ctypes.create_unicode_buffer(1024)
        size = wintypes.DWORD(1024)
        if kernel32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(size)):
            return buf.value
        return ""
    finally:
        kernel32.CloseHandle(h)


def find_app():
    """按进程名找主窗（**不能只按标题**：资源管理器里有个叫 AssetManager 的文件夹）。"""
    hits = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(h, _):
        if user32.IsWindowVisible(h):
            n = user32.GetWindowTextLengthW(h)
            if n:
                b = ctypes.create_unicode_buffer(n + 1)
                user32.GetWindowTextW(h, b, n + 1)
                if "AssetManager" in b.value and exe_of(h).lower().endswith("assetmanager.exe"):
                    hits.append(h)
        return True

    user32.EnumWindows(cb, 0)
    return hits[0] if hits else 0


def click(x, y):
    user32.SetCursorPos(int(x), int(y))
    time.sleep(0.3)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.06)
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(0.6)


def main():
    what = sys.argv[1] if len(sys.argv) > 1 else ""
    hwnd = find_app()
    if not hwnd:
        subprocess.Popen([EXE], cwd=".")
        for _ in range(40):
            time.sleep(0.5)
            hwnd = find_app()
            if hwnd:
                break
        time.sleep(2.5)
    if not hwnd:
        raise SystemExit("没等到主窗口")
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.5)
    r = rect_of(hwnd)
    # 侧栏里两个面板头部的位置：从截图上量出来的（CSS 像素）
    if what == "coll":
        click(r.left + 60 * DPR, r.top + 606 * DPR)
    elif what == "tag":
        click(r.left + 60 * DPR, r.top + 646 * DPR)
    elif what == "both":
        # 先开分组（在上），它的头部位置不变；再点标签
        click(r.left + 60 * DPR, r.top + 606 * DPR)
        time.sleep(0.6)
        click(r.left + 60 * DPR, r.top + 646 * DPR)
    time.sleep(1.0)
    user32.SetCursorPos(r.left + 800, r.top + 500)  # 指针挪开，别污染悬停态
    time.sleep(0.5)
    img = shot.capture(hwnd)
    out = f"_tmp/dock{('-' + what) if what else ''}.png"
    img.save(out)
    print("已保存", out, img.size)


if __name__ == "__main__":
    main()
