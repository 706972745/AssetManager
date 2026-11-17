"""门禁：截图覆盖层不许「自动交付」。

背景（2026-11-18）：本机有程序向**前台窗口注入 Ctrl+C**（低级键盘钩子实测：`0xa2`(左 Ctrl)
与 `C` 同一毫秒到达、都带 INJECTED 标记）。0.1.22 及更早把它当成用户意图，
表现成「框完 / 画完一条线，它自己复制并关窗」。

判据（判据在 Rust 侧：`core/src/keywatch.rs` 只记**真人**按键，
`am_snip_finish` 要求最近 1.2s 内真人按的正是那颗 C / S）：

  A) 注入 Ctrl+C → 覆盖层必须还在、剪贴板不许变位图；
  B) 注入 Ctrl+S → 同上（保存也不许被注水）；
  C) 中键贴图 → 必须照常交付（非快捷键路径不能被误伤）。

⚠️ **真人 Ctrl+C 这一条没法用脚本验**：脚本发的按键本身就是注入，
一定会被闸门挡下 —— 那是判据在起作用，不是缺陷。
真人按键请 owner 手按一次，日志里会留 `snip finish … last_real=Some((67, …))`。

用法（必须用 bash 启动，见 WORKLOG §36.6）：
    python tools/e2e_noauto.py
"""

import ctypes
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import e2e_noclip as E  # noqa: E402

VK_CONTROL, VK_C, VK_S, VK_ESC, VK_MBUTTON = 0x11, 0x43, 0x53, 0x1B, 0x04
LOG = os.path.join(os.environ["LOCALAPPDATA"], "com.assetmanager.desktop", "runtime.log")


def log_size():
    try:
        return os.path.getsize(LOG)
    except OSError:
        return 0


def log_since(since):
    try:
        with open(LOG, "r", encoding="utf-8", errors="replace") as f:
            f.seek(since)
            return [ln.rstrip() for ln in f.readlines()]
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
    time.sleep(1.4)
    return E.find("截图") is not None


def select():
    E.drag(700, 420, 1060, 740)
    time.sleep(0.9)


def inject_copy():
    """模拟那台机器上的注入：Control↓ 与 C↓ 同一批次，中间不 sleep。"""
    E.user32.keybd_event(VK_CONTROL, 0, 0, 0)
    E.user32.keybd_event(VK_C, 0, 0, 0)
    E.user32.keybd_event(VK_C, 0, 2, 0)
    E.user32.keybd_event(VK_CONTROL, 0, 2, 0)


def inject_save():
    E.user32.keybd_event(VK_CONTROL, 0, 0, 0)
    E.user32.keybd_event(VK_S, 0, 0, 0)
    E.user32.keybd_event(VK_S, 0, 2, 0)
    E.user32.keybd_event(VK_CONTROL, 0, 2, 0)


def middle_click():
    E.user32.mouse_event(0x0020, 0, 0, 0, 0)  # MOUSEEVENTF_MIDDLEDOWN
    time.sleep(0.05)
    E.user32.mouse_event(0x0040, 0, 0, 0, 0)  # MOUSEEVENTF_MIDDLEUP


def case(name, act, expect):
    base = log_size()
    if not open_snip():
        print(f"[{name}] 截图没打开 → FAIL")
        return False
    E.clip_set_text("MARK-" + name)
    select()
    act()
    time.sleep(4.0)
    alive = bool(E.find("截图"))
    bitmap = E.clip_has_image()
    lines = [ln for ln in log_since(base) if "snip " in ln and "hotkey" not in ln]
    if expect == "block":
        ok = alive and not bitmap
    else:  # deliver
        ok = (not alive)
    print(f"[{name}] 覆盖层={'在' if alive else '已关'} 剪贴板位图={bitmap} 日志={lines or '无'} → {'PASS' if ok else 'FAIL'}")
    if alive:
        E.tap(VK_ESC)
        time.sleep(0.8)
    E.user32.keybd_event(VK_CONTROL, 0, 2, 0)  # 兜底：别把 Ctrl 留在按下状态
    return ok


def main():
    r = []
    r.append(case("A_注入 Ctrl+C（必须挡住）", inject_copy, "block"))
    r.append(case("B_注入 Ctrl+S（必须挡住）", inject_save, "block"))
    r.append(case("C_中键贴图（必须照常交付）", middle_click, "deliver"))
    ok = all(r)
    print("\n注：真人 Ctrl+C 请 owner 手按一次验证（脚本的按键是注入，一定被挡）。")
    print("EXIT=", 0 if ok else 1)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
