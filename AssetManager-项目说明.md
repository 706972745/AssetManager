# AssetManager

对标 Eagle 的素材管理器，主场是**百万级库的流畅度**。
索引式库（不动你的文件）+ 真实文件夹映射（整理即落盘）+ 拖出去的是原始文件。

> Windows 优先。macOS/Linux 的抽象层已经留好（`ThumbProvider` / 平台薄层），但不是现在的目标。

## 快速开始

```bash
# 环境（借用 FontManager 的便携工具链，路径见上两级 `工作区总览与规则.md` §2）
cd E:/Users/87557/文档/wordbuddy/软件迭代
source "FontManager/Fonts工程文件/env.sh"

# 1) 只验底层
cd AssetManager/core
cargo test
cargo run --bin am-cli -- scan "D:\你的素材目录"

# 2) 只调界面（浏览器 + 假数据，不碰磁盘）
cd ../app
npm install
npm run dev          # http://localhost:1430

# 3) 桌面端开发
cd ../src-tauri
cargo run            # 需要 app 的 vite 已启动（devUrl 1430）

# 4) 打包安装程序
cd ../src-tauri
tauri build          # 产物在 target/release/bundle/nsis/
```

## 目录

| 目录 | 作用 |
|---|---|
| `core/` | Rust 核心：索引 / 扫描 / 缩略图（WebP）/ 内嵌预览提取 / 文件操作 / 撤销（不依赖 Tauri，可独立测试） |
| `app/` | 前端：React + Vite，浏览器可直跑（`mock` 数据源） |
| `src-tauri/` | Tauri 壳：命令 + `thumb://` 协议 + 窗口 |
| `tools/` | 图标生成、截图自测 |
| `docs/` | 规格与方案（工作副本，改规格改这里） |

## 铁律（改代码前先看）

1. **缩略图字节不走 IPC**，只走 `thumb://`
2. **扫描不解码像素**，只读文件头拿宽高（RAW / PSD 读 IFD0、26 字节头，也不解像素）
3. **缩略图只有一种格式：WebP**（`cache/thumbs/**/*.webp`，q80，档位 256 / 384 / 512 / 1024）。
   它只用来在软件里看图 —— 复制、拖出、打开一律是磁盘原文件；**不要往缓存里塞第二种编码**
4. **RAW / PSD 走内嵌预览**（`core/preview.rs` 抠文件里那张 JPEG），不做 demosaic：解一张 45 MP 的 RAW 要几百 MB
5. **瀑布流不裁切、不对齐**：列宽固定、按最短列堆叠，图按原始宽高比完整显示
6. **列表永远带 limit**，深翻用 keyset
7. **前端只持当前页**（200/页），不把全库塞进内存
8. **真实文件操作三件保险**：撤销、阈值确认、删除进回收站 —— 不许拆
9. 「流畅」与「体验」冲突时**问产品 owner**，不要自己拍
10. **自动标签绝不落库**：`tags`/`asset_tags` 只存用户手打的；自动标签（`fmt:`/`kind:`/`place:`/`orient:`/`size:`）每次从原始文件事实推导。
    手工值优先于同维度的自动值，用户否决过的永不再提（纪律同 FontManager `derivedTags`）

## 与 FontManager 的边界（重要，勿越界）

AssetManager 是**独立项目**，和 FontManager 只做邻居，不做亲戚。

| 维度 | 结论 |
|---|---|
| 代码 | 不 import / 不复制 FontManager 的源码。`tokens.css`、`app.css`、`ui.tsx` 是**照数值重写**的，注释里写清来源，改数值只改自己这份 |
| 配置 | 独立 `identifier: com.assetmanager.desktop`、独立数据目录 `%LOCALAPPDATA%\AssetManager`、独立 dev 端口 `1430`。**不许复用 FontManager 的 state.json / api.json / 插件目录** |
| 数据 | 用户素材库和 FontManager 的字体库没有任何关系，不共用目录、不共用缓存 |
| 仓库 | **https://github.com/706972745/AssetManager**（public，默认分支 `main`）。独立仓，与 FontManager 无 CI 关系 |
| 许可 | **GPL-3.0**（`LICENSE` 为 GNU 官方原文；Cargo/package.json 标 `GPL-3.0-only`） |
| 发版 | 独立版本号（当前 **0.1.1 正式版**）、独立安装包 `AssetManager_<ver>_x64-setup.exe`，挂在本仓 Release（tag `vX.Y.Z`）。**不并进 FontManager 的 Release、不挂 FontManager 的 tag 流水线** |
| 仓库 | **将来是 GitHub 上的另一个仓库**（独立 remote / 独立 tag / 独立 CI）。**令牌共用**（同一账户），但令牌只做认证，绝不写进任何会提交的文件，也绝不往 FontManager 的 remote 推 |
| 唯一共用 | 便携工具链 `../FontManager/Fonts工程文件/env.sh`（Rust/MSVC/SDK，宿主目录在 FontManager 下，见上两级 `工作区总览与规则.md` §2），以及「界面视觉语言」这一层设计审美 |

> 视觉可以像（这是刻意的：两个软件一个作者、一套纸墨语言），**工程上必须是两个软件**。
> 反过来也一样：不要因为「不像 FontManager」就改设计，那层参照是产品决策，见 WORKLOG §4.5。

## 更多

- **视觉与交互基准（与 FontManager 同一套）**：`../FontManager/Fonts工程文件/ZFontManager/docs/FontManager设计统一.md`
  —— 行契约（32px / 11px / 500 / 图标 15 / 直角）、单一强调色、滚动条只一套、动效五档令牌、
  停靠面板高度与头部位置、菜单 208/30、设置面板 900×760 固定尺寸。**改界面前先读它**（本项目的
  数值就是照它对齐的，注释里标了出处）。
- 多库是**每库一个独立 DB 文件**（`<数据目录>/libs/lib-<id>.db`，注册库 `index.db` 只放库清单与标签体系）：
  单个库坏了不拖垮别的库，整库备份只要拷一个文件。老库首次启动自动拆分（先备份到 `trash-libs/`）。
- 详情面板照 Eagle 排：预览（**固定 1:1 方框，可滚动看其余部分**）→ 尺寸/体积/类型 → 原文件 →
  标签 → 我的（评分 / **主色板** / 注释 / 来源）→ 操作 → 显示层 → 引用。
- **无边框看图可以多开**（`B` / 中键）：想贴几张贴几张，各自独立；关软件/重启电脑后，
  上次没手动关的全部自己回来（位置/大小/不透明度都记着）；`Shift+滚轮` 图内缩放、
  `Ctrl+滚轮` 调不透明度、双击撕掉、右键有复制原图/复制显示尺寸/另存。
- **快照**：把一批图记成一份快照（**只记图片的内容编号，不搬文件**）。侧栏「快照」一段（照 FontManager：**紧跟素材库目录树**、在标签之上，独立段落可折叠，随目录树一起滚）：点 = 打开灵感板，
  段头 `+` 就地输入新建（照 FontManager：`Enter` 确定 / `Esc` 取消）、`导入` 按钮读 `.json`；右键 = 重新对账 / 导出清单 `.json` / 导出原图 `.zip` / 改名 / 删除；标签右键 →「把标签下的图片加入快照…」。
  因为身份是**文件内容哈希**，图片改名、换目录、甚至换到另一个素材库，导回来都还认得出是同一张。
- **灵感板（无限画布）**：**任意标签右键** → 「打开灵感板」，把该标签下所有软件能出图的格式铺到一块
  无边界画布上（照 PureRef）：拖 = 移、右下角手柄 / `Alt+滚轮` = 改大小、滚轮 = 锚定指针缩放、`F` = 全部适应。
  板上复制/打开/定位/另存**一律是源文件**，缩略图只用来在板上看。板内 `Ctrl+V` 把剪贴板图片存进
  `程序目录\screenshots`、自动入库并打上这块板的标签。
- **截图与剪贴板**：`F1` 选区存到 `程序目录\screenshots`（自动进素材库）· `Alt+F1` 截完直接进剪贴板 ·
  `F3` 把剪贴板图片用无边框看图打开（留最近 10 张）。三个键都是系统级、可在设置里改。
- **无边框看图**的手感：窗口就是那张图（无标题栏/无边框，长边 342 DIP 起），滚轮连窗口一起缩放、
  整窗可拖、默认置顶 —— 专门配合屏幕截图（照果核看图的「无边框模式」+ Snipaste 的贴图）。
- 工具面板（`Ctrl+K` 搜「重复 / 引用 / 重命名」）：重复检测（xxh3 精确 + aHash 相似）、
  丢失的引用、批量重命名（规则 + 预览 + 可撤销）。
- 窗口尺寸 **1280×820 / min 900×600**，出厂**深色**主题（浅色可在设置 → 外观切换；`?theme=light`
  可临时预览，不落盘）。多库上限 **3 个**，库根右键可「从素材库移除（不删文件）」。
- **进度总览（已实现能力 + 决策摘要 + 与规格对照）**：[`AssetManager-工作日志.md`](AssetManager-工作日志.md)
- **待办清单（P0→P3，含验收标准）**：[`AssetManager-待办清单.md`](AssetManager-待办清单.md)
- 开发流水（状态 / 决策 / TODO / 踩坑，append-only）：[`WORKLOG.md`](WORKLOG.md)
- 范围与规格：`docs/AssetManager-MVP方案-2026-11-15.md`（v1.2，**事实来源**）
- 上游讨论全集：`docs/AssetManager-需求与架构整理-2026-11-15.md`（v0.2）

> **上传 GitHub 时**：仓库根需要一个 `README.md`（GitHub 只渲染这个名字）。
> 那时在仓库根放两三行指路（「这是 AssetManager，说明见 `AssetManager-项目说明.md`」+ 相对链接）即可，
> **正文只维护本文件这一份**，别复制成两份各自演进。见 `../工作区总览与规则.md` §7。
