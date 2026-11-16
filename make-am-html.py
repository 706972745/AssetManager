#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
把本目录下的 AssetManager 文档渲染成纸墨版 HTML。

复用 FontManager 的 docs/make-spec-html.py 里的 Markdown 转换器与样式，
保证这些讨论稿和 FontManager 的规格书看起来是一家人。

用法：
    python make-am-html.py            # 生成全部
    python make-am-html.py mvp        # 只生成 MVP 方案
    python make-am-html.py digest     # 只生成讨论整理
"""

import importlib.util
import io
import os
import sys
import html as html_mod

HERE = os.path.dirname(os.path.abspath(__file__))
FM = os.path.join(
    "E:", os.sep, "Users", "87557", "文档", "wordbuddy", "软件迭代",
    "Fonts工程文件", "ZFontManager", "docs", "make-spec-html.py",
)

DOCS = {
    "digest": {
        "src": "AssetManager-需求与架构整理-2026-11-15.md",
        "title": "AssetManager · 讨论整理与可行性评估",
        "kicker": "DISCUSSION DIGEST / FEASIBILITY REVIEW",
        "h1": "AssetManager<br>讨论整理与可行性评估",
        "sub": "对标 Eagle 的素材管理器 · 主场是百万级库的流畅度 · 整合同事四轮讨论并标注需修正之处",
        "meta": [
            ("版本", "v0.2"),
            ("性质", "讨论全集"),
            ("覆盖", "讨论还原 / 可行性 / Eagle 拆解 / 架构 / 性能预算 / UI 复用 / 路线图"),
            ("UI 基准", "FontManager v2.2.2"),
        ],
    },
    "mvp": {
        "src": "AssetManager-MVP方案-2026-11-15.md",
        "title": "AssetManager · MVP 方案（范围收敛版）",
        "kicker": "MVP SCOPE / EXECUTION PLAN",
        "h1": "AssetManager<br>MVP 方案",
        "sub": "能建库、能秒看、能拖进 PS、能真的整理文件夹 · 拖的是缩略图，出去的是原文件",
        "meta": [
            ("版本", "v1.2"),
            ("性质", "执行方案（待确认）"),
            ("周期", "3 周 MVP"),
            ("UI 基准", "FontManager v2.2.2"),
        ],
    },
}

FOOTER = "Markdown 是唯一事实来源；改结论先改 .md，再跑一次本脚本。"


def load_converter():
    spec = importlib.util.spec_from_file_location("fm_spec", FM)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def build(fm, cfg):
    src = os.path.join(HERE, cfg["src"])
    dst = src[:-3] + ".html"

    md = io.open(src, encoding="utf-8").read()
    body, toc = fm.convert(md)

    toc_html = ['<nav class="doc-toc"><h2>目录</h2><ul>']
    for level, text, sid in toc:
        cls = "" if level == 2 else (" toc-l3" if level == 3 else " toc-l4")
        toc_html.append(
            '<li class="%s"><a href="#%s">%s</a></li>'
            % (cls.strip(), sid, html_mod.escape(fm.plain(text)))
        )
    toc_html.append("</ul></nav>")

    meta_html = "".join("<span>%s <b>%s</b></span>" % (k, v) for k, v in cfg["meta"])

    doc = """<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>%s</title>
<style>%s</style></head><body>
<section class="cover">
  <div class="kicker">%s</div>
  <h1>%s</h1>
  <div class="sub">%s</div>
  <div class="meta">
    %s
  </div>
</section>

%s

%s

<div class="footer">
本页由 make-am-html.py 从 %s 生成，请勿手工修改 HTML。<br>
%s
</div>
</body></html>
""" % (
        cfg["title"], fm.CSS, cfg["kicker"], cfg["h1"], cfg["sub"], meta_html,
        "\n".join(toc_html), body, cfg["src"], FOOTER,
    )

    io.open(dst, "w", encoding="utf-8", newline="\n").write(doc)
    print("已生成 %s（%d 字符，目录 %d 条）" % (os.path.basename(dst), len(doc), len(toc)))


def main():
    fm = load_converter()
    keys = [sys.argv[1]] if len(sys.argv) > 1 else list(DOCS)
    for key in keys:
        if key not in DOCS:
            print("未知文档：%s（可选：%s）" % (key, " / ".join(DOCS)))
            raise SystemExit(2)
        build(fm, DOCS[key])


if __name__ == "__main__":
    main()
