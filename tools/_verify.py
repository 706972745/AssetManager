#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""一次性自测（不進产品）：主界面截图 → F1 截图覆盖层 → 框选 → 回车保存 → 校验落盘。

用法：python tools/_verify.py
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

    # 回车 = 保存（finish("save")）
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
    raise SystemExit(main())
