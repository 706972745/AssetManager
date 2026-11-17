<div align="center">

<img src="src-tauri/icons/128x128.png" alt="AssetManager icon" width="110" />

# AssetManager

**素材库只记索引，不搬你的文件 —— 拖出去、复制、打开的永远是磁盘上那一份。**

百万级素材库里滚得动、看得清、找得到：多格式缩略图、标签与分组、快照、无限灵感板、截图标注。

<br>

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-gray?style=flat-square)](LICENSE)
[![Version](https://img.shields.io/badge/Version-0.1.1%20dev-gray?style=flat-square)](../../releases)
[![Platform](https://img.shields.io/badge/Platform-Windows-gray?style=flat-square)](#平台与构建)
[![Built with Tauri](https://img.shields.io/badge/Built%20with-Tauri%20v2-gray?style=flat-square)](https://tauri.app)
[![Status](https://img.shields.io/badge/Status-开发版%20(0.x)-gray?style=flat-square)](#已知限制)

<br>

**[⬇ 下载最新安装包](../../releases/latest)** ·
**[📘 项目说明](AssetManager-项目说明.md)** ·
**[📓 工作日志](AssetManager-工作日志.md)** ·
**[🗂 待办清单](AssetManager-待办清单.md)** ·
**[🏷 标签系统方案](标签系统方案.md)**

<br>

<img src="docs/shots-0.1.1-主界面.png" alt="AssetManager — 侧栏（素材库 / 快照 / 标签）、素材网格、详情面板" width="900" />

<sub>左侧：素材库目录树 · 快照 · 标签；中间：缩略图网格；右侧：详情、评分、注释（照 Eagle 的排布）。</sub>

</div>

<br>

---

## 这是什么

一个 **Windows 桌面素材管理器**（对标 Eagle），核心是一条约定：

> **索引，不搬家。** 你的原图待在它原来的文件夹里。软件只记住"它在哪、多大、什么格式、属于哪些标签"，
> 拖到 Photoshop、复制、在资源管理器打开、另存 —— 全部作用于**磁盘上的原文件**。
> 缩略图只是用来看的，永远不会被当成结果交出去。

技术上：**Tauri v2 + React + TypeScript**（界面） + **Rust**（扫描、缩略图、SQLite 索引、系统集成）。
索引落在本地 SQLite，一个素材库对应一个索引；多库各自独立、互不干扰。

## 主要能力

| 能力 | 说明 |
|---|---|
| **多素材库** | 最多 3 个库根，自有数据目录与索引；库根可随时加 / 移除，文件不受影响 |
| **多格式缩略图** | JPG/PNG/WebP/AVIF/HEIC、PSD/PSB、AI/EPS、SVG/WMF/EMF、各厂 RAW、PDF —— 统一走一条缩略图管线（内嵌预览优先，取不到再用系统缩略图），失败不报错、留占位 |
| **滚动性能** | 视口外不挂 `<img>`（不挂就不解码）；百万元素靠 keyset 分页，滚多快都不抖 |
| **标签 / 分组** | 手工标签 + 自动标签（格式、类型、尺寸、色彩等从文件事实推导，只读）；标签分组、颜色、批量打标 |
| **快照** | 把一批图"记下来"（不搬文件）。身份是**文件内容指纹**（xxh3-128），所以**改名、换目录、换素材库之后导回来照样对得上**；可导出清单 `.json`、可把原图打包 `.zip` |
| **灵感板** | 任意标签 / 任意快照右键 → 一块**无限、无边界**的画布（照 PureRef）：拖动、缩放、改大小；板上复制 / 打开 / 定位一律作用原文件；板内 `Ctrl+V` 把剪贴板图存进 `screenshots` 并自动打标 |
| **截图标注** | `F1` 选区 → 矩形/椭圆/箭头/画笔/文字/马赛克 + 撤销重做 → 复制或保存；`Alt+F1` 截完直接进剪贴板；`F3` 贴图板 |
| **工具面板** | 重复文件、丢失引用、批量重命名 |
| **命令面板** | `Ctrl+K` 找命令、跳转、新建快照、导入快照 |

## 快捷键

| 键 | 作用 |
|---|---|
| `F1` | 选区截图（松手进标注态） |
| `Alt+F1` | 截图并直接进剪贴板 |
| `F3` | 贴图板（剪贴板贴图 / 多开 / 重启还原） |
| `Ctrl+K` | 命令面板 |
| `Space` | 预览当前素材 |
| `Ctrl+C` / `Ctrl+V` | 复制原文件 / 在灵感板里粘贴入库 |
| 灵感板：拖空白 / 滚轮 / `F` / `0` / `Esc` | 平移 / 缩放 / 适应窗口 / 100% / 关闭 |

## 平台与构建

Windows 10/11 x64。需要 Rust 工具链 + Node 20+ + WebView2（Win11 自带）。

```bash
# 依赖
cd AssetManager/app && npm install

# 开发（dev 端口 1430）
cd .. && ./app/node_modules/.bin/tauri dev

# 打包（产出 src-tauri/target/release/bundle/nsis/AssetManager_<ver>_x64-setup.exe）
./app/node_modules/.bin/tauri build
```

## 已知限制

- 仍是 **0.x 开发版**：数据结构还可能动，不建议当唯一存档；
- 灵感板上的位置 / 大小**不落库**（临时视图，关掉即失）；
- 快照看图的入口是灵感板，主列表暂不支持"按快照筛选"；
- 只支持 Windows。

## 与 FontManager 的关系

同一台机器上的另一个项目是 [FontManager](https://github.com/706972745/FontManager)（字体管理器）。
两者**互不依赖**：各自的仓库、版本号、安装包、数据目录、端口（AssetManager `1430`）。
只共用便携工具链与视觉语汇 —— 侧栏、动效、标签面板、快照段的形态刻意保持一致，用起来是同一套手感。

## 许可

**GNU General Public License v3.0**（GPL-3.0）· 全文见 [`LICENSE`](LICENSE)。

Copyright (C) 2026 AssetManager authors

简单说：可以自由使用、修改、分发，甚至商用；但**分发时必须一并提供源码**，
且衍生作品必须同样以 GPL-3.0 发布。想把它并进闭源产品是不行的。
（如果你只想在自己机器上用，这些义务与你无关。）

## 文档索引

- 怎么跑、铁律、目录职责：[`AssetManager-项目说明.md`](AssetManager-项目说明.md)
- 每一版做了什么、为什么这么做：[`WORKLOG.md`](WORKLOG.md)
- 优先级与未完成项：[`AssetManager-待办清单.md`](AssetManager-待办清单.md)
- 标签系统的设计取舍（FontManager 也复用了这份）：[`标签系统方案.md`](标签系统方案.md)
