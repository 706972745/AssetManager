#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""一次性自测（不進产品）：主界面截图 → F1 截图覆盖层 → 框选 → 回车保存 → 校验落盘。

⚠ 已废弃（0.1.20）：Enter 不再交付，本脚本的 Enter 判据会把正常行为判成失败。发版门禁不要跑它，用 tools/e2e_v0120.py 代替。

为什么留着：文件里的窗口探测、抢拍、选区拖拽、截图目录差集这些步骤本身还是对的，
只把「Enter 交付」那一步换掉就能复活（未来若要重做这条链路可参考）。
下面 `main()` 里的有效部分都保留着，只是入口不再执行 —— 直接跑会被劝退（退出码 2），
避免有人误跑拿到「失败」结论。

用法（历史）：python tools/_verify.py
前提：src-tauri/target/release/assetmanager.exe 刚构建过，且没有另一个实例在跑。
"""

import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import e2e_snip as E  # noqa: E402

EXE = r"E:/Users/87557/文档/wordbuddy/软件迭代/AssetManager/src-tauri/target/release/assetmanager.exe"
SHOTS = r"E:/Users/87557/文档/wordbuddy/软件迭代/AssetManager/src-tauri/target/release/screenshots"


def main():
    user32 = E.user32
    # 只 kill 自己这个进程名，绝不碰 fontmanager.exe
    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)

    before = set(os.listdir(SHOTS)) if os.path.isdir(SHOTS) else set()
    proc = subprocess.Popen([EXE])
    print("started", proc.pid)
    time.sleep(9)

    w = E.find("AssetManager")
    if not w:
        print("❌ 主窗没出现")
        return 1
    print("① 主窗", w[1:5])
    user32.SetForegroundWindow(w[5])
    time.sleep(0.5)
    E.shot("AssetManager", "_tmp/verify-1-main.png")

    # F1：覆盖层
    E.key(0x70)
    E.key(0x70, True)
    t0 = time.time()
    time.sleep(0.25)
    E.shot("截图", "_tmp/verify-2-snip-fast.png")  # 抢拍"黑屏瞬间"
    print("② 0.25s 抢拍完成", round(time.time() - t0, 2))
    time.sleep(1.4)
    E.shot("截图", "_tmp/verify-3-snip.png")

    s = E.find("截图")
    if not s:
        print("❌ 截图覆盖层没出现")
        return 1
    print("③ 覆盖层", s[1:5])

    # 框一个选区（窗口相对坐标）
    E.drag(260, 200, 980, 720, steps=12, origin=(s[1], s[2]))
    time.sleep(1.2)
    E.shot("截图", "_tmp/verify-4-select.png")

    # ⚠ 过期判据（0.1.17 之前的口径）：当时 Enter = 保存（finish("save")）。
    # 0.1.20 起 Enter 什么都不做（框完直接画标注），要交付必须 Ctrl+S / Ctrl+C。
    # 所以这一步在现版本必然"落盘无新文件"，据此得出的失败结论无效。
    # 若将来复活脚本：把这两行换成 Ctrl+S（0x53，带 Ctrl 修饰）即可。
    E.key(0x0D)
    E.key(0x0D, True)
    time.sleep(2.0)
    E.shot("AssetManager", "_tmp/verify-5-after-save.png")

    after = set(os.listdir(SHOTS)) if os.path.isdir(SHOTS) else set()
    new = sorted(after - before)
    print("④ 新文件：", new if new else "（无）")

    subprocess.run(["taskkill", "/IM", "assetmanager.exe", "/F"], capture_output=True)
    return 0 if new else 2


if __name__ == "__main__":
    print(
        "⚠ 已废弃（0.1.20）：Enter 不再交付，本脚本的 Enter 判据会把正常行为判成失败。"
        "发版门禁不要跑它，用 tools/e2e_v0120.py 代替。",
        file=sys.stderr,
    )
    raise SystemExit(2)
