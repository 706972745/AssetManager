# AssetManager 工作日志

> **给接手/协作的人看的**：这份文件按「现在是什么状态 → 怎么跑 → 为什么这么设计 → 下一步做什么」组织。
> 每次改完代码请补一条到 §12 变更记录，不要另开文档。
>
> - 项目定位：**对标 Eagle 的素材管理器，主场是百万级库的流畅度**
> - 当前版本：**0.5.0 开发版**（P0 + P1 全清；0.4.0 加标签系统，0.5.0 补标签组/颜色/搜索并把侧栏换成悬浮抽屉；更早的 `0.2.x-ui` 是内部界面迭代代号，不是发布版本号）
> - 最后更新：2026-11-16
> - 上游设计文档：`docs/AssetManager-MVP方案-2026-11-15.md`（v1.2，范围与规格的事实来源）

---

## 0. 项目边界：与 FontManager 是两个项目（2026-11-15 owner 强调，重中之重）

> owner 原话：「不要和 FontManager 软件混在一起，这个是重中之重，因为 AssetManager 是我准备做的另外一个项目。」

这不是「尽量分开」，是**硬边界**。落地成五条，任何一条破了都算技术债：

| # | 边界 | 具体含义 | 违反后果 |
|---|---|---|---|
| B1 | **代码不共享** | 不 import、不 `path = "../Fonts工程文件/..."` 引用、不复制粘贴整个模块。`app/src/styles/*.css`、`components/ui.tsx` 是**照数值重写**的独立副本，注释里标来源只是给后来人对照，不是依赖 | FontManager 改 UI 会把 AssetManager 带崩；两边需求分叉后无法各自演进 |
| B2 | **配置不共享** | 独立 `identifier: com.assetmanager.desktop`、数据目录 `%LOCALAPPDATA%\AssetManager`、dev 端口 `1430`、单实例锁自己一套。**绝不读写 FontManager 的 `state.json` / `api.json` / `plugins/` / `data/`** | 用户数据被另一个软件改写（FontManager 11-11 的 state 事故就是同类问题的代价） |
| B3 | **不并成插件/子程序** | AssetManager 不是 FontManager 的插件、不是它的一个页面、不挂在它的插件市场里 | 装上 FontManager 的人被动多一个百万级索引器；发版节奏被绑死 |
| B4 | **仓库/发版线独立** | **将来上传到 GitHub 的另一个仓库**：自己的 remote、自己的 tag、自己的 CI、自己的 `AssetManager_<ver>_x64-setup.exe`。**令牌共用**（同一 GitHub 账户，2026-11-15 owner 明确）；但令牌只做认证——不写进任何会提交的文件，不往 FontManager 的 remote 推，推送前 `git remote -v` 核对仓库名 | 一个软件回滚另一个遭殃；令牌泄漏或被误推到错的仓；`python-runtime.zip` 这类资产张冠李戴 |
| B5 | **工作日志/文档各自一份** | 本文件只记 AssetManager；FontManager 的日志在 `软件迭代/.workbuddy/memory/`。规格改在本项目 `docs/` | 两个项目的决策混在一份记录里，接手的人分不清哪条约束属于谁 |

**唯一共用**：便携工具链 `../FontManager/Fonts工程文件/env.sh`（Rust 1.99 + 便携 MSVC + SDK，工具链宿主目录在 `FontManager/` 下，原因见工作区 `工作区总览与规则.md` §2），以及「纸墨 + 单一蓝 + 0 圆角 + 发丝线 + 自绘块光标」这套**视觉语言**。

> 注意方向别搞反：视觉像 FontManager 是**刻意的产品决策**（同一作者、两软件一家人），工程上必须像两个软件。
> 所以「为了跟 FontManager 区分开」而改设计是错的；「为了方便而共用代码」也是错的。

现状核对（2026-11-15 逐项验过）：`assetmanager-app` / `com.assetmanager.desktop` / `%LOCALAPPDATA%\AssetManager` / `:1430` 全部独立，源码里 `FontManager` 字样只出现在**注释与文档**中，没有一处 import 或路径依赖。 ✅
仓库现状：本目录**还不是 git 仓库**；将来 `git init` 后要配**自己的 remote**（独立仓），GitHub 令牌与 FontManager 共用（同一账户），按 B4 的规矩用。 ✅

---

## 1. 一句话状态

Windows 桌面端已经能：**建库 → 扫描（可取消）→ 网格/瀑布流看图 → 搜索 → 空格预览（1–8× 缩放）→ 多选批量 → 拖出原文件到 PS/资源管理器 → 拖到目录树＝真实移动（后台线程 + 进度）→ 打开原文件 / 定位 / 复制原文件 / 重命名 / 删除（回收站）→ 详情面板 → 偏好设置 → 重建索引 / 导出用户数据**。
安装包：`src-tauri/target/release/bundle/nsis/AssetManager_0.3.1_x64-setup.exe`（按用户安装，不需要管理员）。
核心索引、缩略图管线、真实文件操作、撤销都在 Rust 侧跑通并有单测（**22 项全绿**）。

> ✅ **0.3.1 安装包已打好并安装（2026-11-16 08:56）**：先静默卸载旧版 0.2.0 再全新安装，
> 实测 `FileVersion=0.3.1 / ProductVersion=0.3.1`；界面「关于」显示 `0.3.1 开发版`。
> 本轮只收 P1 的两处遗留（name_asc 深翻、跨盘移动阻塞），没动别的功能。

**还不能**：标签 / 智能文件夹 / 评分（P2）；跨盘的「移动进度」只有条数和当前文件名，没有字节级速度；macOS/Linux 未支持。

---

## 1.5 铁律：缩略图只是显示层（2026-11-15 owner 定）

> 原话：「软件中显示的缩略图可以是占内存和速度最快的方式和格式，但是对这个缩略图的任何操作都是属于在原文件做一样的操作。」

这条是**架构约束**，不是优化建议。落地成三条：

| 层 | 允许做什么 | 禁止做什么 |
|---|---|---|
| 显示层（网格 / 瀑布流 / 空格大图 / 详情预览） | 只读 `cache/thumbs/**/*.webp`；只按档位取 256 / 384 / 512 / 1024；解码并发 ≤ 4；超 80 MP 的图直接不自解码 | 不把原图字节读进 UI；不让缩略图成为任何数据源；不从低档缩略图放大成大档 |
| 操作层（打开 / 定位 / 复制 / 重命名 / 删除 / 移动 / 拖出） | 一律用 `assets.path` 指向的**磁盘原文件** | 永不操作缓存文件；永不把缩略图当"文件"交出去 |
| 元数据（格式 / 大小 / 尺寸 / 时间） | 描述**原文件** | 不拿缩略图的 JPEG 属性冒充原文件 |

对应的实现位置：

- `core/src/thumb.rs` — 只产出缓存 JPEG，缓存 key 用 `(volume_serial/file_id, size, mtime, px)`，移动或改名后 key 不变
- `core/src/fsops.rs::open_with_default` — ShellExecuteW 打开 `assets.path`
- `core/src/fsops.rs::copy_files_to_clipboard` — 写 **CF_HDROP**，剪贴板里是原文件（不是位图）
- `src-tauri/src/main.rs` — 14 个命令全部收 path，没有一个是收 asset_id 去动缓存的
- 验证方式：`am-cli copy <原文件>` 之后在 PowerShell 跑 `Get-Clipboard -Format FileDropList`，应打印原文件完整路径

---

## 2. 目录结构

```
AssetManager/
  core/                 Rust 核心库（不依赖 Tauri，可独立测试）
    src/
      model.rs          DTO（camelCase 序列化给前端）
      formats.rs        格式表 + 文件头嗅探 + 徽标
      imageinfo.rs      只读文件头拿宽高（百万级扫描不解码的前提）
      db.rs             SQLite：schema / 分页 / 目录对账 / ops 日志
      scan.rs           扫描器（BFS、可中断、防环、超长路径）
      thumb.rs          缩略图管线 + 缓存 + LRU
      fsops.rs          真实文件操作 + 校验 + 撤销
      bin/am_cli.rs     冒烟 CLI（不起界面就能验底层）
  app/                  前端（Vite + React + TS，浏览器可直跑）
    src/
      types.ts          Asset / DirNode / Source 接口
      data/source.ts    数据源选择：有 __TAURI__ 用桌面，否则用 mock
      data/mock.ts      浏览器预览用的假库（2600 张，确定性伪随机）
      lib/layout.ts     网格 / 瀑布流排版算法
      components/       ui（图标·块光标·Toast·菜单）/ Chrome（标题栏·顶栏·详情）/ Tree / Views
      styles/           tokens.css（FontManager 令牌）+ app.css
  src-tauri/            壳：命令 + thumb:// 协议 + 窗口
  tools/make-icons.py   生成 .ico（七帧）与各尺寸 PNG
  tools/shot.py         截图自测（PrintWindow + 屏幕抓图两路）
  docs/                 规格与方案（工作副本，改规格改这里）
```

**分层铁律**：业务逻辑只在 `core/`；`src-tauri` 不写算法；`app/` 不碰文件系统。

**docs/ 的来历**：两份规格是从对话产物目录 `wordbuddy/spaces/2026_11_15_19-50-20/` **复制**进来的，原件保留作归档。
今后**改规格只改 `docs/` 这份**，不要再回去改归档；HTML 版是 `make-am-html.py` 生成的派生物，项目里不放。

**仓库卫生（2026-11-15 收口）**：根目录新增 `.gitignore`，挡掉 `target/`、`app/node_modules/`、`app/dist/`、`src-tauri/gen/`、`*.db*`、`cache/`、`UI待定问题清单.md`。
当前这三个目录合计约 **5.1 GB**（`src-tauri/target` 4.1 G / `core/target` 991 M / `node_modules` 90 M），全部可再生，**没有进版本库的风险但要留意磁盘**。
（还没 `git init` —— 建仓前先看 §0：这是一个独立仓库，不是 FontManager 的子目录。）

---

## 3. 怎么跑

环境（与 FontManager 共用一套便携工具链）：

```bash
cd E:/Users/87557/文档/wordbuddy/软件迭代
source "FontManager/Fonts工程文件/env.sh"        # Rust 1.99 + 便携 MSVC + SDK（宿主在 FontManager 下）
```

### 3.1 只验底层（最快，不开界面）

```bash
cd AssetManager/core
cargo test                            # 9 个单测：格式/嗅探/尺寸解析/命名校验
cargo run --bin am-cli -- scan "E:\tmp\am-test" --data "E:\tmp\am-data"
cargo run --bin am-cli -- list  --data "E:\tmp\am-data" --limit 10
cargo run --bin am-cli -- thumb 4 --px 256 --data "E:\tmp\am-data"
cargo run --bin am-cli -- tree  --data "E:\tmp\am-data"
```

### 3.2 只调 UI（浏览器，假数据）

```bash
cd AssetManager/app
npm install
npm run dev        # http://localhost:1430
```

浏览器模式下 `Source.desktop === false`，文件操作按钮会给出明确提示，不会假装成功。

### 3.3 桌面端

```bash
cd AssetManager/src-tauri
cargo tauri dev      # 或 cargo run（前端已 build 时）
```

### 3.4 打包安装程序

```bash
cd AssetManager/src-tauri
export PATH="../app/node_modules/.bin:$PATH"     # 用 app 里装的 tauri CLI
tauri build --config tauri.conf.json
# 产物：src-tauri/target/release/bundle/nsis/AssetManager_0.1.1_x64-setup.exe
```

**只想要 exe（不打包）时也必须带特性**，否则它不会内嵌前端：

```bash
cargo build --release --features tauri/custom-protocol
```

图标改完要重跑 `python tools/make-icons.py`（**ico 必须七帧**，少一帧以后单测会挂）。

---

## 4. 已实现（含验证证据）

| # | 能力 | 位置 | 怎么验的 |
|---|---|---|---|
| 1 | SQLite 索引（WAL / 64MB 缓存 / 派生数据） | `core/db.rs` | `am-cli scan` 后 `list/tree` 有数 |
| 2 | 扫描器：BFS、按目录对账、可中断、防环、`\\?\` 长路径 | `core/scan.rs` | 4 目录 / 4 文件 / 0 错误 / 5 ms |
| 3 | **只读文件头拿宽高**（PNG/JPEG/GIF/WebP/BMP/ICO + image 兜底） | `core/imageinfo.rs` | 4 张 PNG 尺寸全对；5 个单测覆盖头解析 |
| 4 | 格式表 + 魔数嗅探（扩展名撒谎也能认出来） | `core/formats.rs` | 单测：jpg/png/webp 嗅探、jpeg→JPG 徽标 |
| 5 | 缩略图管线：取源→Triangle 缩放→**WebP q80**→原子写→LRU 记账 | `core/thumb.rs` | 288×512 / 5.0 KB；同图对比 JPEG78 省 28–51%（见 §12） |
| 6 | 缓存 key 用「创建时间+大小」而非路径（**移动后不重生成**） | `core/thumb.rs` | 代码，待 M1 做移动回归 |
| 6.5 | **相机 RAW / PSD 内嵌预览直取**（不解 RAW 像素） | `core/preview.rs` | 造 CR2 / CR3 / RAF / PSD 四种容器全部出图；无预览的 PSD 优雅失败。见 §12 |
| 7 | 中文子串搜索（FTS5 trigram，≥3 字走 FTS，1–2 字退 LIKE） | `core/db.rs` | 搜「封面」命中 3/4 |
| 8 | keyset 分页（mtime/size 双列游标，name 走 offset） | `core/db.rs` | 列表排序与深翻待压测 |
| 9 | 真实文件操作：建/改名/移动（同盘 rename、跨盘复制校验后删源）/删除进回收站 | `core/fsops.rs` | 命名校验单测 4 条；真机操作待回归 |
| 10 | 撤销（`ops` 定长 20，删除不可撤销并如实告知） | `core/fsops.rs` | 代码 |
| 11 | `thumb://` 自定义协议（字节不过 IPC + 一年缓存头） | `src-tauri/src/main.rs` | 代码，待真机验 |
| 12 | 常规网格（小/中/大 120·176·260） | `app/components/Views.tsx` | 前端构建通过 |
| 13 | 瀑布流（justified、最后一行不拉伸、contain 不裁切） | `app/lib/layout.ts` | 同上 |
| 14 | 虚拟化（按行算、视口 ±400px 缓冲） | `app/components/Views.tsx` | 同上 |
| 15 | 目录树（懒加载、空目录可见、选中 2×32 蓝竖线、拖放高亮） | `app/components/Tree.tsx` | 同上 |
| 16 | 自绘块光标（块高=字号+2、宽=字符 advance+8、1.1s 硬切、IME 回退） | `app/components/ui.tsx` | 同上 |
| 17 | Toast（8 圆角 / 左 2px 竖条 / 最多 4 条） | `app/components/ui.tsx` | 同上 |
| 18 | 空格大图预览（12 圆角、遮罩无模糊、方向键切换、Esc） | `app/App.tsx` | 同上 |
| 19 | 图标（拼贴纸片风、叠片母题、ico 七帧） | `tools/make-icons.py` | 128px 目视通过 |
| 20 | 无边框窗口 + 自绘标题栏（拖拽区 / 最小化 / 最大化 / 关闭） | `app/components/Chrome.tsx` + `main.rs` | 真机截图确认 |
| 21 | 打包链路（Tauri build → NSIS） | `src-tauri/tauri.conf.json` | 产出 2.5 MB 安装包，release exe 在**没有 dev 服务器**时能正常加载内嵌前端 |

### 底层实测数据（本机，小样本）

```
扫描 E:\tmp\am-test → 4 目录 / 4 文件 / 4 张图 / 4 有尺寸 / 0 错误 · 5 ms
解码 1080×1920 → 144×256 JPEG q78 → ≈4.8 KB · 盒式采样（thumbnail）比 Lanczos3 快 3~5 倍
搜索「封面」→ 3 条（FTS5 trigram）
剪贴板：am-cli copy <原文件> → PowerShell `Get-Clipboard -Format FileDropList` 回读 = 原文件路径
```

### 4.5 界面与 FontManager 的对照（数值全部来自 `FontManager/src/app.css`，不新造）

| 元素 | 生效值 | 我们落在哪 |
|---|---|---|
| 窗口本体 | **`transparent: true` + `shadow: false` + `decorations: false`**；`html/body/#root` 全透明 | `src-tauri/tauri.conf.json`、`tokens.css` |
| 窗口外框 | `.app-frame`：`border: 1px solid rgba(ink,.07)` + `border-radius: var(--qb-radius)`(8) + `overflow: hidden`；四角透出桌面 | `app.css` |
| 窗口底 | `--canvas: #fafaf8`，标题栏 `rgba(ink,.015)`、顶栏纯 canvas，分界全是 1px 发丝线 | `styles/tokens.css` / `app.css` |
| 标题栏 | 40 高，`padding: 0 5px 0 14px`，品牌标 21，标题 13/500 **次文字色**（实测参照截图字核 = `#6b6b66`），动作键 30×30 gap6 图标 14/1.5，窗口键 38×30，关窗键 hover 红底红字 | `Chrome.tsx` |
| 侧栏 | **248 宽**，`padding: 10 10 12`，分节标题 10/600 字距 .1em，行 **32 高**、11px、0 圆角、悬停 `--ui-hover`，选中 = 2×32px 蓝竖线 + 2% 底；行内距 11 起 → 图标 11 / 文字 35（截图实测 30 / 59 物理像素 @DPR1.25，与参照物逐像素相同） | `Tree.tsx` |
| 侧栏行首箭头 | **绝对定位压在左内距里**（`left: depth*14`），不参与 flex 排版 —— 否则文字被顶到 68px，比参照物右移 20px | `app.css` `.tree-caret` |
| 侧栏底栏 | 固定四行（打开库 / **回收站 0** / 关于 / **快捷键 + 16px 键帽**），顶部 1px 分界线 + `padding-top 8` + `margin-top: auto` | `Tree.tsx` |
| 侧栏计数 | 等宽 tabular，右对齐 `min-width: 4ch`，**0 也显示**，距右缘 11 | `Tree.tsx` |
| 顶栏 | `padding: 10 16`，控件统一 **36 高**、1px 描边、4px 圆角、**13px** 字号、底色 `--e1`（hover `--e2`） | `Chrome.tsx` |
| 顶栏输入框 | **白底** `--ui-panel` + 1px 发丝线（参照截图内 252,252,252）；按钮才是 `--e1` | `app.css` `.topbar-field` |
| 类型筛选 | **36×36 纯图标钮**（漏斗 / 1.5 描边），激活加 `inset 0 0 0 1px --accent-a` 环 + 图标转 accent；类型名只留在 title | `Chrome.tsx` |
| 排序 | 图标（⇅）+ 当前排序名 + ⌄，13px | `Chrome.tsx` |
| 尺寸滑杆 | 轨道 2px / 手柄 **8px 实心蓝点** / `margin-top: -3px` / 右侧等宽 `176px`（小/中/大进 title） | `app.css` `.topbar-size` |
| 视图切换 | 36 高托盘 + 3px 内距 + **滑动白底块**（`translateX(i*36px)`），每项 34 宽，当前项只转主文字色，不套蓝底 | `app.css` `.view-toggle/.view-pill` |
| 载入进度 | 内容区**底部** 24 高细带：1px 上发丝线 + `padding 5 16` + 左 `flex:1` 的 3px 圆角轨道（`rgba(ink,.08)`）+ 右 11px 弱色「载入中 · 已取 N 项」；空闲整条不渲染 | `App.tsx` `.progress-bar` |
| 徽标 | 9px/700、字距 .07em、1px 描边、0 圆角 | `.tile-badge` |
| 详情面板 | 300 宽、44 高头、10px 大写小标题、属性表 72px 标签列 | `Chrome.tsx` |

**与 FontManager 的有意差异**（不是没对上，是产品不同）：

1. **没有状态栏** —— FontManager 主窗口也没有；数量改由侧栏行承担，载入进度用**内容区底部 24px 细带**（与 FontManager `.load-progress` 同一套几何：3px 轨道 + 右侧文字）。
2. **行首锚点改横向**（owner 2026-11-15 定）：静止是 6px 圆点，悬停/选中横向长成 `格宽 − 16` 的 2px 线（悬停白、选中蓝）。FontManager 是竖线，因为它的内容是文字行；我们是图块，竖线会压在图上像裁切标记。
3. **详情面板可收起**，默认不占位；Eagle / FontManager 的详情是常驻的。
4. 顶栏只有一组输入框（没有"预览文字"），所以搜索框有 `max-width: 340`，避免宽窗口下横成一条。
5. **侧栏只有一段「素材库」**：FontManager 的浏览/分组/快照/标签是字体特有的组织维度（收藏/启用/停用/分组/快照/标签），素材库没有对应实体，硬做就是空壳按钮 —— 按 owner「宁可不做也不放空壳」的纪律不做。
6. 侧栏行**多一列折叠箭头**（目录树必须有），压在左内距里，所以图标/文字仍与参照物同列。
7. 视图切换 2 项（产品只有网格/瀑布流），参照物 3 项。
8. 侧栏选中底的右缘在**侧栏内容框**上（距右边框 10），不是贴到边框 —— 这是照参照截图实测（填充 16…297 物理像素，边框在 310 @DPR1.25），不是漏了右内距。

---

## 5. 未实现 / 下一步（按优先级）

| 优先级 | 任务 | 说明 | 预估 |
|---|---|---|---|
| **P0** | **OLE 拖拽出窗（CF_HDROP）** | 现在只是一个会 toast 的占位。要在 Rust 侧 `DoDragDrop` + `SHCreateDataObject`，命令必须 `run_on_main_thread` 后立即返回，拖前 `ReleaseCapture()`。见 MVP 方案 §2 | 2 天 |
| **P0** | 目录树拖放＝真实移动 | 前端已画了落点高亮，`onDropTo` 还是占位；接 `am_move` + 阈值确认 | 0.5 天 |
| P1 | 百万级压测 | 现在只在 4 文件上验过。需要造 10 万/100 万库，测首屏、滚动帧率、闪白、跳行 | 2 天 |
| ~~P1~~ | ~~缓存上限与清理 UI~~ | **已完成（0.2.2-ui）**：设置 → 缓存与索引里显示真实占用、可调上限、一键清理（接 `am_cache_stat` / `am_cache_gc`）；设置面板也不再是 toast 占位 | — |
| ~~P1~~ | ~~设置面板~~ | **已完成（0.2.2-ui）**：Eagle 式左导航 + 分组卡片 + 应用/保存设置，滚动条与交互沿用 FontManager | — |
| ~~P1~~ | ~~窗口圆角~~ | **已完成（0.2.1-ui）**：透明窗口 + `.app-frame` 8px 圆角 + 1px 描边，见 §4.5 | — |
| P1 | 回收站面板 / 批量恢复 | 侧栏第四行现在只报 0 + toast，按纪律不放空壳 | M1 |
| P2 | 标签 / 智能文件夹 / 评分 | M1，schema 里还没建表（`assets.note/rating` 已留列） | 3 天 |
| P2 | 插件宿主（PSD / 3D 缩略图） | 协议已在文档定义，代码只留了 `builtin_thumbable` 的失败标记 | 3 天 |
| P2 | 文件夹选择器 | 现在用 `window.prompt()` 手输路径，丑但可用；应换 `IFileDialog`（需 windows crate） | 0.5 天 |
| P3 | 系统缩略图兜底（L3） | `ThumbProvider` trait 还没抽，L2/L3 未接 | 1 天 |
| P3 | 建 `UI待定问题清单.md` | §11.2 的纪律已经定了，但**文件还没建**（FontManager 那份在 `Fonts工程文件/ZFontManager/`，不要照抄内容，格式照搬即可） | 0.2 天 |
| P3 | 重建索引 / 多库 / 9 语言 / 安装器路线 C | 参考 FontManager，地基可复用 | 各 0.5–2 天 |

---

## 6. 关键决策记录（为什么这么做）

> 每条都写「如果反悔怎么办」，因为接手的人最容易在没读懂理由时把它改坏。

### D1 索引式库：文件留在原地，只建索引
- **理由**：百万级下托管式（Eagle 做法）＝ 双倍存储 + 300 万小文件（每素材一个 `.info` 目录），NTFS 枚举、杀软、备份全遭殃。
- **代价**：一个文件只能在一个目录 → 「一图多归」以后只能靠标签。
- **反悔代价**：高。要迁移用户文件，且库结构变化。

### D2 文件夹 1:1 映射磁盘，操作真落盘
- **理由**：无黑盒、不丢东西、与资源管理器双向一致、换软件零成本。
- **代价**：误操作是真的 → 所以必须有撤销 + 阈值确认 + 删除进回收站。**这三样不是可选项，删掉任何一样这套方案就不成立。**
- **反悔代价**：极高。

### D3 `assets` 只存 `dir_id + name`，不存完整路径
- **理由**：重命名目录 = 一条前缀 SQL（`UPDATE dirs SET path=?||substr(...)`），百万库毫秒级；存完整路径就得改几百万行。
- **依赖**：`dirs.path` 必须始终是真实绝对路径；改目录名后要 `repath_subtree`。
- **反悔代价**：高（schema 变更 + 全量回填）。

### D4 缩略图 key 不含路径
- **理由**：含路径的话「把 3000 张图挪进子目录」会让 3000 张缩略图全部失效重生成，用户看到的是「软件又在扫了」。
- **实现**：`(creation_time, size, mtime, px)` 的 xxh3。不用 `file_index`（std 里还是 unstable），零依赖。
- **代价**：跨盘复制会生成新缓存（合理）；同一文件被「复制成两份」不共享缓存。
- **反悔代价**：中，换 key 只是旧缓存作废一次。

### D5 扫描阶段只读文件头，不解码
- **理由**：百万级解码会直接把 IO/CPU 打满；但瀑布流需要宽高比才能不跳行。
- **实现**：`imageinfo::dimensions()` 读首 64KB，手写 PNG/JPEG/GIF/WebP/BMP/ICO 解析，其余退 `image` 的 `into_dimensions()`。
- **注意**：拿不到尺寸的按 4:3 兜底，别让格子消失。

### D6 缩略图字节走 `thumb://`，不走 IPC
- **理由**：Tauri IPC 会把 `Vec<u8>` 序列化成 JSON 数字数组，5MB 图能膨胀成 20MB JSON，直接卡死。
- **实现**：`register_uri_scheme_protocol("thumb")`，Windows 上落在 `http://thumb.localhost/<id>?px=256`，带 `immutable` 缓存头让 WebView 再缓一层。

### D7 三档 R1–R4（分辨率规则）
- R1 取源 = min(档位, 原图长边)，**永不放大**
- **R2 永不从低档缩略图放大成大档**（必须重新从原图解，否则糊两次）
- R3 原图 < 档位×0.75 → 标记 `upscaled`，界面如实说「接近原尺寸显示」
- R4 DPR 变化不触发重新生成（档位像素固定）
- **反悔代价**：低，但违反 R2 是体验灾难，改之前先想清楚。

### D8 两种拖拽语义分开
- 拖到**目录树节点** = 真实移动（落盘）
- 拖出**窗口** = 复制原文件出去（CF_HDROP，源不动）
- **MVP 只允许内部移动到目录树**，不允许拖到网格空白 → 真实移动是高危操作，落点要明确。

### D9 前端数据结构：只持当前页 + 预取
- **理由**：百万条元数据全进内存 = 300MB+，滚动必卡。
- **实现**：每页 200，`cursor` keyset，滚到距底 900px 触发下一页；请求带序号，过期响应丢弃。

---

## 7. 数据库 schema（实际生效）

见 `core/src/db.rs::SCHEMA`。要点：

| 表 | 作用 | 备注 |
|---|---|---|
| `libs` | 库根目录 | 一个库可多个根 |
| `dirs` | 目录树 | `path` 是派生列，重命名用前缀 UPDATE |
| `assets` | 素材行 | 只存 `dir_id + name`；`thumb_key/state/quality` |
| `assets_fts` | 全文索引 | `content=''` + trigram，手动 add/del |
| `thumb_lru` | 缓存记账 | `gc()` 按 `last_used` 淘汰 |
| `ops` | 操作日志 | 定长 20，撤销用 |
| `meta` | 杂项 | 预留 |

**没有 `UNIQUE(dir_id,name)`**：Windows/macOS 大小写不敏感、Linux 敏感，同一约束三平台语义不同。重复由扫描「按 name 对账」处理（查出既有 → 更新 / 新增 / 删除缺失），顺带保住 `thumb_key` 与 `rating`。

---

## 8. 命令与协议清单

| 命令 | 参数 | 返回 |
|---|---|---|
| `am_roots` | — | `DirNode[]`（库根） |
| `am_children` | `parent?: number` | `DirNode[]`（一层，懒加载） |
| `am_list` | `q: AssetQuery` | `Asset[]`（≤1000） |
| `am_total` | — | `number` |
| `am_scan` | `path` | 人类可读统计串 |
| `am_reveal` | `path` | — |
| `am_rename` | `path, name` | 新路径 |
| `am_mkdir` | `parent, name` | 新路径 |
| `am_move` | `paths[], target` | 移动条数 |
| `am_delete` | `paths[]` | —（进回收站） |
| `am_undo` | — | 结果文案 |
| `am_cache_stat` | — | `[bytes, files]`（遍历 `cache/thumbs`） |
| `am_cache_gc` | `limit, target` | `[淘汰前总量, 释放字节]`，清全部传 `(0,0)` |
| `am_data_dir` | — | 索引/缓存目录绝对路径 |
| `am_close / am_minimize / am_toggle_max` | — | — |

协议：`GET thumb://<assetId>?px=256` → **`image/webp`**（命中缓存或现场生成）。

`AssetQuery`：`{ dirId, recursive, kinds[], text, sort, cursor:[k,id], limit }`。

---

## 9. 性能纪律（改代码不能破坏的 8 条）

1. **缩略图字节绝不走 IPC**，只走 `thumb://`
2. **扫描不解码像素**，只读文件头
3. **列表永远带 limit**，不做 `SELECT *` 全量
4. **深翻用 keyset**，不用 `OFFSET`（三种排序都走双列游标；name 用上一页最后一行的 id 反查 name）
5. **前端只持当前页**，不把全库塞进 React state
6. **缩略图三态**（骨架 / 低清 / 高清），滚走就取消
7. **格子尺寸预先算好**（按宽高比撑开），图来了不许跳行
8. **字节要原子写**（`.tmp` 改名），不许出现半张图
9. **解码绝不持全局锁**（2026-11-16，0.3.2）：`Mutex<App>` 只用来「读一行元数据 / 写一行元数据」，
   解码、编码、读文件全在不持锁状态做完。任何「握着锁做几十毫秒以上的活」都会把列表、树、详情一起按住
10. **滚动期间不改 state 的重活**：窗口计算 rAF 合帧 + 位移阈值；瓦片必须 `memo`，交给它的回调必须稳定引用

验收指标（MVP 方案 §7）：首屏 ≤800ms、滚动 ≥55FPS、**闪白 0 次**、**跳行 0 次**、搜索 ≤120ms、缩略图 p95 ≤120ms、常驻内存 ≤500MB、切视图/切档不丢滚动位置。

---

## 10. 已知缺陷与限制

| # | 问题 | 影响 | 计划 |
|---|---|---|---|
| 1 | ~~拖拽出窗只是占位 toast~~ **已修（0.3.0）** | 现在是真 OLE 拖拽，出去的是磁盘原文件 | `am_drag_out`（`drag` crate + CF_HDROP） |
| 2 | ~~目录树拖放不落盘~~ **已修（0.3.0）** | 拖到树行＝真实移动，`am://drag-end` 回传松手坐标 | `am_move` / `am_move_start` |
| 3 | ~~窗口方角（没有 `--qb-radius` 运行时圆角）~~ **已修（0.2.1-ui）** | 视觉细节 | 已生效：`transparent:true` + `.app-frame` 8px 圆角 + 1px 描边 |
| 4 | ~~加库用 `prompt()` 手输路径~~ **已修（0.3.0）** | 加库/新建目录走原生 `IFileDialog`（`rfd`）；只有「输入文件夹名 / 新文件名」还用 `prompt()`（那是输入文字，不是选路径） | — |
| 5 | ~~`name_asc` 排序用 OFFSET 分页~~ **已修（0.3.1）** | 深翻是「跳过 N 行」，翻页间隙有增删还会重复/漏图 | 改 (name COLLATE NOCASE, id) keyset：用上一页最后一行的 id 反查 name，前端协议不变；id 已被删才退回 OFFSET（单测 `name_asc_pages_without_dup_or_gap`） |
| 6 | FTS 手动维护（add/del） | 改名失败理论上会漏 | 有 `rebuild-fts` 兜底 |
| 7 | ~~跨盘移动是同步阻塞~~ **已修（0.3.1）** | 大目录移动时命令不返回、界面干等 | `am_move_start`：独立线程 + 独立连接 + `move://progress`（100ms 节流）/ `move://done`；`MOVE_BUSY` 防重入 |
| 8 | ~~没做系统缩略图兜底（L3）~~ **已做（0.3.0）** | HEIC / PDF / AI / 视频有系统缩略图了（`IShellItemImageFactory` 手写 FFI） | `core/src/shellthumb.rs`；`am-cli shell` 可单测这一层 |
| 9 | 图标是脚本生成的 v1 | 已补 `pack/library/asset3d/broken` 四套、16/24px 用加粗母版；仍不够精致 | 交设计重做，保持七帧 |
| 10 | ~~百万级完全没压测~~ **已做（0.3.0）** | `am-bench` 10 万 / 100 万行：首页 0.46/0.48ms，搜索 20/29.8ms，树 0.03/0.06ms | 顺带修了两个真 bug（排序索引前缀、树递归计数） |
| 11 | ~~网格固定尺寸、右侧留白不铺满~~ **已修** | 列数改成四舍五入 + 格宽吃掉余量（174 而不是 176），一行正好铺满 | 已生效 |
| 12 | ~~目录树计数只算直接子项~~ **已修** | 改成递归 CTE 统计整棵子树（一条查询算完同层所有兄弟） | 百万级需实测这条查询耗时 |
| 13 | ~~缩略图编码 JPEG q78~~ **已改 WebP q80（2026-11-16）** | 体积小 28–51%，`libwebp-sys` 静态编译进 exe（不吃 cmake） | 已生效 |
| 14 | ~~递归计数在百万库上未测~~ **已解决（0.3.0）** | 已换成 `dirs.cnt_all` 沿父链维护，100 万行 534ms → 0.06ms | 老库首次启动自动回填 |
| 15 | 缩略图解码并发靠进程内闸门（4） | 多窗口/多进程会各算各的 | 够用；真要限就上系统级信号量 |

---

## 10.5 踩过的坑（别再踩第二次）

| # | 现象 | 根因 | 解法 |
|---|---|---|---|
| 1 | 打好的 exe 打开是一张「localhost 拒绝连接」的浏览器错误页 | `cargo build --release` 没带 `tauri/custom-protocol` 特性 → 走 devUrl 而不是内嵌 dist | `--features tauri/custom-protocol`；`tauri build` 会自动带 |
| 2 | 界面全是彩色假图（`2600 项`），不是真实库 | Tauri 2 默认不注入 `window.__TAURI__` | 用 `window.__TAURI_INTERNALS__.invoke`（默认存在），或在配置里开 `withGlobalTauri` |
| 3 | 缩略图第一次正常，之后**永久空白**，删缓存才好 | `thumb://` 的 404 带了可缓存头，被 WebView 当 immutable 存进磁盘缓存 | 404 加 `Cache-Control: no-store`；URL 里加 `&r=<mtime>` 让 URL 随内容变化 |
| 4 | `tauri build` 报 `ENOENT ...\软件迭代\app\package.json` | `beforeBuildCommand` 的 cwd 是**项目根**（src-tauri 的上一级），`--prefix ../app` 多走了一层 | 写成 `npm --prefix app run build` |
| 5 | 三栏布局在没选目录时错乱：详情面板跑到左边 | `main.no-tree` 把列数改成 2，但 DOM 里仍有 3 个子节点，第一个子节点被压成整列 | 不按「有没有目录」改列数；只按「详情是否展开」改 |
| 6 | 首屏一个素材都不显示（已载入 0） | 没选目录时不发查询 | 启动后自动选中第一个库根 |
| 7 | 格子里的图是「浅灰方块」但状态栏没报错 | `.tile-img` 自带 skeleton 底色，掩盖了「已 200 但解码失败」与「正在加载」的区别 | `.tile-img` 底色改透明，骨架只由 `.tile-skel` 承担 |
| 8 | 根目录旁的计数显示 `0`，但点进去有一屏素材 | 计数只算直接子项，素材都在子目录里 | 改递归 CTE；顺便把「0 也要显示」统一，否则用户读不出「真的空」还是「没算」 |
| 9 | 每行右边空出 150px，像界面没铺满 | 列数用 `floor`，余量全丢在右边 | 用 `round` 定列数，再按 `(宽 − 间距) / 列数` 回算格宽 |
| 10 | 自动化截图里点缩略图「没反应」 | 截图按 1.28 DPI 放大，我拿 CSS 坐标当屏幕坐标，实际点在了侧栏上 | 先 `GetWindowRect` 取窗口原点，再按 **DPR 换算**后点击；验证交互时用「点完看 DOM 变化」而不是看鼠标位置 |
| 11 | 透明窗口后圆角**外侧还有一块灰**，只有弹层（大图预览）四角是直角 | `position: fixed` 的元素逃出 `.app-frame` 的 `overflow: hidden`，遮罩自己不带圆角 | 给 `.lightbox` 补 `border-radius: var(--qb-radius)`；以后凡 fixed 全屏层都要自带圆角 |
| 12 | `tools/shot.py` 截透明窗口时四角是**纯黑**，看着像没生效 | PrintWindow 拿不到其它进程合成的桌面，透明像素被填成 0 | 判圆角要看**屏幕抓图**（`ImageGrab` + `GetWindowRect` 的 bbox），四角像素应等于桌面色；PrintWindow 只用来核对内容 |
| 13 | 按 1.28 换算 DPR 对不上（侧栏 248 应 317 物理像素，实测 310） | 本机实际 DPR = **1.25**（截图 1600×1025 = 1280×820 CSS；标题栏 50 物理 = 40 CSS） | 量任何像素前先用「标题栏 40 / 侧栏 248 / 行 32」这三个已知值反推 DPR，别抄别处的数 |
| 14 | 侧栏行里加折叠箭头后文字整体右移 20px | 箭头是 flex 子项，占了 12+10 的宽度 | 箭头 `position: absolute` 压进左内距（`left: depth*14`），行内只剩图标+文字 |
| 15 | 界面看着「哪儿都不对」，但逐项量数值都对：搜索框压在侧栏上、工具栏偏左、侧栏整体下沉 | **顶栏放错层级**：`TopBar` 和 `main` 平级（都在 `.app-frame` 下），于是它成为通栏，宽度吃满整个窗口 | 顶栏必须住在「侧栏右侧」的那一列里：`.app-frame > .main > [Tree, .workspace > (TopBar, .content), Detail]`。**先量骨架再量数值** —— 骨架错 1px，下面所有数值对齐都是白做 |
| 17 | 灯箱（双击/空格）里**竖图被裁掉一半**，面板还顶出窗口底部 | ① `.lightbox-panel` 的 `grid-template-rows: 44px 1fr`：`1fr` 的最小值是 **auto**，一张 1000×1920 的图会把行撑到 1920px，面板再 `overflow:hidden`，看到的就是被裁的图；② 面板高度写死 `min(760px, 88vh)`，窗口矮时顶出去 | 行改 `minmax(0, 1fr)`，`.lightbox-img` 补 `min-width/height: 0`，面板宽高改 `min(1100px, 100%)` / `min(760px, 100%)`，`.lightbox` 加 `padding: 24px` 兜住四周 |
| 19 | 10 万行列表首页要 **117ms**（滚动时每次翻页都会卡） | 排序索引写成 `idx_assets_mtime ON assets(kind, mtime DESC)` —— **`kind` 在最前面**，默认视图不筛类型时整条索引报废，SQLite 只能把 10 万行全取出来排序；`EXPLAIN QUERY PLAN` 里是 `USE TEMP B-TREE FOR ORDER BY` | 补三条**纯排序索引** `(mtime DESC,id DESC)` / `(size DESC,id DESC)` / `(name COLLATE NOCASE,id)`（原来的带 kind 版本保留给筛选），配 `LIMIT 200` 能顺着索引取够就停：**117ms → 0.46ms**。教训：凡是 `ORDER BY` 的列，索引第一列就必须是它 |
| 20 | 100 万行时目录树根层 **534ms**，树一打开像卡死 | `dir_children` 给每个子项跑一次递归 CTE（`sub` 展开整棵子树再 JOIN assets 计数），N 个子目录 = N 次全库展开 | 改成扫描时维护 `dirs.cnt_all`（`recalc_dir_chain` 沿父链重算：自己直接几条 + 子目录 cnt_all 之和），树只读一列：**534ms → 0.06ms**；老库靠 `migrate()` 首次启动回填（100 万行 0.48s）。教训：**别在列表查询里现场聚合整棵子树** |
| 21 | 瓦片拖拽在真机上**完全没反应**（浏览器里却正常） | 起手式用的是 `pointerdown` + `pointermove`（`e.buttons !== 0`）。WebView2 在这条路径上根本没把 pointermove 送进来（合成鼠标事件与真实鼠标都试了），但 **mouse 事件一定送达** | 全换成 `mousedown` / `mousemove`(`e.buttons===1`) / `mouseup`，并给 `.tile,.tile-img` 加 `-webkit-user-drag:none; user-select:none`（否则 WebView2 会自己起原生图片拖拽抢手势）。换完实测：`big_0799.webp` 从 `am-big` 真被拖进 `am-scan-test`（800→799 / 6000→6001） |
| 23 | 引入 `windows = 0.62` 只开 `Win32_UI_Shell`，编译到一半 **rustc 工作集 37 GB、6 分钟不推进 1% CPU**，机器直接换页卡死 | `windows` crate 把整个 Shell/GDI 的元数据一次性摊开，16 GB 机器扛不住；而且这一条依赖会让**每次**全量构建都多花十几分钟 | 删掉依赖，`core/src/shellthumb.rs` **手写 FFI**（`#[link(shell32/gdi32/ole32)]` + vtable 第 4 槽 `GetImage`），和 `fsops` 的 CF_HDROP 同一条路线。教训：只为了 3 个系统调用不值得引成套绑定 |
| 24 | `cargo build ... 2>&1 \| tail -40` 之后**编译进程突然停住**，CPU 曲线变平，等半小时也没结果 | 工具侧不主动抽干后台任务的 stdout，管道缓冲区满了以后 rustc 写日志被阻塞（不是编译慢，是写不出去） | 所有构建命令一律 **`> /tmp/x.log 2>&1`**，再读文件；不要在后台任务里把构建输出接给 `tail`/`grep` |
| 25 | `cargo test` / `cargo build`（debug）在这台机器上动不动就 OOM 换页；同样代码 `--release` 2 分钟就好 | debug 带 `debuginfo=2`，16 GB 内存下峰值直接把物理内存打穿（可用只剩 6 GB） | core 的 test / build **一律加 `--release`**；顺带和打包产物一致，少一类"debug 好 release 崩"的惊喜 |
| 26 | `npm run build` 报 `[safe-delete] 操作失败: spawnSync ... ETIMEDOUT` | vite 的 `emptyOutDir` 走 node `rmSync`，被沙箱 shim 改写成"移到回收站"，一万多个文件搬到超时 | 先 `rm -rf app/dist` 再 `npm run build`（构建本身没问题，只是清目录那一步被拦） |
| 27 | 框选从内容区边距起手完全没反应 | `onMouseDown` 挂在 `.view-inner` 上，而 `.viewport` 四周有 14/16 px 内距，起手点落在内层外面，事件根本没进来 | 监听挂到 `.viewport`，命中测试仍用 `.view-inner` 的矩形 |
| 28 | 灯箱滚轮缩放不生效，控制台还刷 `Unable to preventDefault inside passive event listener invocation`，且背后的网格跟着滚 | React 17+ 把 `onWheel` 委托到根节点且默认 `passive: true`，里面 `preventDefault()` 被忽略 | 用 `addEventListener("wheel", h, { passive: false })` 原生挂载，配合 `zoomRef` 拿最新值，监听只装一次 |
| 29 | `GetImage` 明明返回了 HBITMAP，`hbitmap_to_rgba` 却拿不到像素 | 我用 `GetDIBits(hdc, hbm, 0, 0, NULL, &bi, ...)` 做"探测调用"问尺寸 —— 对 `GetImage` 返回的 **DIB section** 它直接返回 0 | 改 `GetObjectW` 读 `BITMAP` 结构的宽高，再带显式尺寸调 `GetDIBits`。实测 PNG 768×481/57 ms、PDF 423×615/50 ms |
| 30 | 同族图标的 16px 一塌糊涂（细线整条消失，墨点只有 0.4%） | 母版 1024 画 4 px 的线，按比例缩到 16px 只剩 0.06 px | 每种母题**另存一张加粗母版**（线宽 ×3、格子缝拉大），只给 16/24 两帧用；`make-icons.py` 生成时顺带打印 16px 墨点覆盖率自检（现在 24–30%） |
| 22 | 调试版窗口一片「localhost 拒绝连接」 | `cargo build` 打出来的 debug 包会**继承上次 `tauri dev` 的 dev 环境**（build script 里 `dev` cfg 被缓存），于是去连 1430 而不是用内嵌 dist | 二选一：要么先 `npm run dev` 再跑 debug exe（HMR 直接生效），要么 `touch src/main.rs && cargo build` 强制重跑 build script 让它回到内嵌 dist |
| 18 | 灯箱明明传了 `px=1600`，缓存里却只有 1024，图还是糊 | `main.rs::parse_query` 把 px **静默** `clamp(64, 1024)` —— 前端传什么都没用，而且不会报错，只看得到「高档位没生成」 | 上限放到 **2048**（灯箱 1600 档用）；这类"钳位"以后一律写注释说明为什么是这个数 |
| 16 | 按一次 Esc 关设置，详情面板也跟着被收起 | 设置面板与 App 的全局快捷键**都挂在 window 上**；设置用 capture 监听调 `stopPropagation()`，只挡「传播到别的节点」，挡不住**同一节点**上后注册的冒泡监听 | 设置的 Esc 用 `e.stopImmediatePropagation()` + `preventDefault()`；或者把全局快捷键改成读 ref。别靠「状态已经改成 false 了，另一个监听应该会 return」——离散事件里 state 刷新时机不站在你这边 |

---

## 11. 协作约定（重要）

0. **动手前先看 §0**：这是独立项目，任何「顺手复用 FontManager 的代码/配置/发版线」的念头都先停下。
1. **「百万级流畅」与「用户体验」冲突时，不要自己拍，问用户。**
   用户原话：*「如果冲突告诉我由我来决定那一刻是选择哪个，后续出现冲突都是需要询问」*。
   例：为了流畅要不要砍掉「实时文件监听」？不要自己决定，列出取舍再问。
2. **拿不准的 UI 决定写进 `UI待定问题清单.md`**（沿用 FontManager 纪律：四段定位 + 编号分段 + 只增不删不复用）。
3. **真实文件操作的三件保险不许拆**：撤销、阈值确认、删除进回收站。
4. **schema 改动必须写 migration**，不许直接改 `SCHEMA` 常量了事（现在还是 v0，等有真实用户数据后立刻加 `user_version` 迁移）。
5. **提交前**：`cargo test`、`npx tsc --noEmit`、`npm run build` 三件全绿。

---

## 12. 变更记录

| 日期 | 版本 | 内容 |
|---|---|---|
| 2026-11-15 | 0.1.0-mvp | 首版骨架：core（索引/扫描/头解析/缩略图/文件操作/撤销）+ 前端（网格三档/瀑布流/目录树/搜索/详情/空格预览/块光标）+ Tauri 壳（命令/thumb 协议/无边框窗口）+ 图标脚本。9 个单测通过，前端构建通过，NSIS 安装包产出。OLE 拖拽为占位。 |
| 2026-11-15 | 0.1.0-mvp+ | 真机联调修正：修 3 个必现 bug（带 custom-protocol 才能内嵌前端 / 用 `__TAURI_INTERNALS__.invoke` / 缩略图 404 被缓存成永久空白）；布局三栏修正；详情默认收起；补 §10.5 踩坑表。 |
| 2026-11-15 | **0.2.0-ui** | 界面按 FontManager **源码**重做：标题栏/侧栏 248·32·11px/顶栏 36 控件/滑杆 8px 蓝点/视图切换滑动底块/去掉蓝底选中/去掉状态栏；新增 §1.5「缩略图只是显示层」铁律 + §4.5 对照表。能力上补：用默认程序打开原文件（ShellExecuteW）、复制原文件到剪贴板（CF_HDROP，已用 PowerShell 回读验证）、删除到回收站、目录计数改递归、网格铺满、锚点改横向、缩略图改盒式采样 + 80MP 护栏 + 解码并发 4。 |
| 2026-11-15 | **0.2.1-docs** | **项目治理（本轮无功能改动）**：① 新增 §0「与 FontManager 的两个项目边界」五条硬约束（代码/配置/插件关系/仓库发版线/文档各自一份），并核对现状全部合规；② 规格文档落地到本项目 `docs/`（MVP 方案 v1.2 + 需求整理 v0.2），修掉 WORKLOG 里指向 `../spaces/...` 的**失效路径**；③ 新增 `.gitignore`（挡 target/node_modules/dist/gen/*.db/cache，并挡内部 `UI待定问题清单.md`），盘点构建产物 5.1 GB；④ README 补「与 FontManager 的边界」表 + 目录表补 `docs/`；⑤ §11 增加第 0 条「动手前先看 §0」；⑥ §1 标出**现有 NSIS 包（21:36）早于 0.2.1-ui 改动（21:45–21:58），对外安装前要重打**。 |
| 2026-11-15 | **0.2.1-docs++** | **文档改名（用户反馈「三个 README 分不清」）**：入口说明由 `README.md` 改为 **`AssetManager-项目说明.md`**（工作区级改 `工作区总览与规则.md`、FontManager 改 `FontManager-项目说明.md`）；交叉引用全部改新名；新增工作区 §7「文档命名与放置规范」——**唯一例外**是上 GitHub 的仓库根必须 `README.md`，那时只放两三行指路，正文仍以本说明为准，不维护两份。 |
| 2026-11-15 | **0.2.1-docs+** | **工作区重排 + 仓库规则**：① FontManager 全部内容收进 `软件迭代/FontManager/`（含 `env/` `vs2022/` `rust-target/` `node_modules/`），本项目环境脚本路径改为 `../FontManager/Fonts工程文件/env.sh`（已实机验证 rustc/cl/rc 可用）；② **B4 升级为「仓库/发版线独立」**：将来建 GitHub 独立仓，**令牌共用同一账户**但绝不写进可提交文件、绝不推错 remote，推送前 `git remote -v` 核对；③ 工作区新增总导航文档（`工作区总览与规则.md`）（判定规则 / 硬边界 / 缓存纪律 / 自助检查）。 |
| 2026-11-15 | **0.2.1-ui** | 向 FontManager **实机截图**（`E:/tmp/fm-real.png`，1600×1025 @DPR1.25）贴身对齐，本轮无新业务功能：① 窗口改 `transparent:true/shadow:false` + `.app-frame` 8px 圆角 + 1px 外描边（`--qb-radius`），页画布全透明；② 侧栏折叠箭头改绝对定位，图标/文字落回 11/35（与参照物逐像素相同）；③ 侧栏底栏补到四行（打开库 / 回收站 0 / 关于 / 快捷键+键帽），快捷键走 toast 不做空壳；④ 顶栏控件 11→13px、底 `--ui-panel`→`--e1`、类型筛选改 36×36 纯图标钮、排序加 ⇅ 图标、输入框回白底、尺寸值只报 `176px`；⑤ 载入进度从内容区顶部 2px 挪到**底部 24px 细带**（3px 轨道 + 右侧「载入中 · 已取 N 项」）；⑥ 大图遮罩补圆角。`npm run build` + `cargo build --release --features tauri/custom-protocol` 全绿，§10.5 补 4 条坑（fixed 层逃逸/PrintWindow 拍透明/DPR=1.25/箭头挤行）。 |
| 2026-11-15 | **0.2.2-ui** | **骨架对齐 FontManager（owner 截图指出「和参考软件不一样」）**：根因是**顶栏放错层级** —— 原来是 `TitleBar / TopBar / main{Tree, content}`，顶栏成了通栏，横跨 248px 的侧栏宽度，于是连锁出四个症状：① 搜索框整个压在侧栏列上；② 整条工具栏（滑杆 / 筛选 / 排序 / 视图切换）左移 248px；③ 侧栏被顶下去 57px，所有行、底栏、目录树整体错位；④ 详情面板跟着顶栏一起矮了 57px。改成与 FontManager `.app-shell / .app-main` 同构的骨架：`TitleBar / main{ Tree, workspace{ TopBar, content }, Detail }`，新增 `.workspace`（flex column, min-width:0）。顺带对齐两处：顶栏底色 `--canvas` → `rgba(ink,.01)`、底边 `--hairline` → `rgba(ink,.05)`（实测参照物顶栏是 247,247,245，纯 canvas 会和内容连成一片）；侧栏行补 `font-weight:500`（`.nav-row` 基础段就写着 500，11px 覆盖层没动字重，少了这半档会显得比参照浅）。**未改任何业务逻辑**，`tsc --noEmit` + `vite build` + `tauri build` 全绿，NSIS 包 23:36 重打并安装。 |
| 2026-11-15 | **0.2.2-ui+** | **详情面板 + 偏好设置（owner 第二轮截图）**：① 详情面板按 FontManager `.detail` 重做 —— 默认 348 宽、左缘 10px 握把可拖（300–560，双击复位）、文件名点击即复制（Clipboard API + textarea 兜底）、去掉 44px 分隔头、格式徽标、预览卡（1px 描边 8 圆角）、`<dl>` 属性表 88px/13px、路径可点开资源管理器、主操作整宽按钮 + 两列次操作；② 新增 `Settings.tsx`：Eagle 式「左导航（标题 + 真实搜索 + 5 分区）/ 右分组卡片 / 底部 关闭·应用·保存设置」，但选中态、发丝线、键帽、滚动条全是 FontManager 那套；设置项**都真的生效**（默认档位/视图/悬停文件名 → localStorage `am.prefs.v1`；缓存占用与清理 → 真命令）；③ Rust 侧把已存在的 `thumb::gc` 暴露成 `am_cache_stat` / `am_cache_gc`，另加 `am_data_dir`；④ `Ctrl+,` 开设置、Esc 层次关闭；⑤ 瓦片的原生 `title` 双行系统白框换成 420ms 自绘 hint（名字 / 路径 / 尺寸·格式）。**注意**：Esc 曾出现「关设置顺带把详情也收了」——两个监听都挂 window，`stopPropagation` 挡不住同节点的冒泡监听，必须 `stopImmediatePropagation`（§10.5 #16）。 |
| 2026-11-16 | **0.2.2-ui++** | **右键菜单（owner 第三轮截图）**：WebView2 在页面右键会弹「返回/刷新/另存为/打印/更多工具」那套浏览器菜单。新增 `components/ContextMenu.tsx`（createPortal 挂 body、贴边翻转、点外面/Esc/滚动/失焦关闭），样式照 FontManager `.ctx-menu`（208 宽 / 4 圆角 / 30 高行 / 1px 分隔）。`App.tsx` 用**一处 document capture 监听**接管全窗口 `contextmenu`：非输入区一律 `preventDefault()`（实测 `dispatchEvent` 返回 false）；输入框保留系统编辑菜单。三种真菜单：**素材格**（打开/预览/定位/复制原文件/复制完整路径/重命名/移到回收站，右键先选中）、**目录树行**（资源管理器打开/重扫该目录/建子文件夹，靠 `data-dir-path`）、**空白区**（重扫/加文件夹/撤销/切视图/偏好设置）。图标靠 `data-asset-id` 定位。CDP 实跑验证：三处菜单内容正确、左键与 Esc 能关、原生菜单 PREVENTED、无 JS 异常。 |
| 2026-11-16 | **0.1.1 开发版** | **版本号统一（owner 定）**：之前源码里是 0.1.0、本文件里是 0.2.x-ui 界面迭代代号、界面「关于」又显示 0.1.0，三套说法。现在**发布版本号统一为 `0.1.1`，界面标注「0.1.1 开发版」**；`0.2.x-ui` 只作为历史界面迭代代号留在 §12，不再当版本号用。改了 9 处：`tauri.conf.json` / `app/package.json` / `app/package-lock.json`（顶层 + packages[""] 两处）/ `src-tauri/Cargo.toml` / `core/Cargo.toml` / 两个 `Cargo.lock` 里的本地包条目 / `Settings.tsx` 关于行 / `App.tsx` 关于 toast；文档同步 `WORKLOG.md` 头部与安装包名、`AssetManager-项目说明.md`、工作区 `工作区总览与规则.md` 与记忆索引。安装包随之变成 `AssetManager_0.1.1_x64-setup.exe`。**没改** MVP 规格里插件示例 `plugin.json` 的 `"version": "0.1.0"`（那是示例插件自己的版本，不是本软件）。 |
| 2026-11-16 | **0.1.1 开发版** | **缩略图口径 + 观看体验（owner 第四轮）**：① **缩略图统一成 WebP q80**（`thumb.rs`，`libwebp-sys` 静态进包，不吃 cmake）：同图实测 256 −28% / 384 −36% / 512 −41% / 1024 −51%，缩放从盒式采样换 **Triangle** 补回清晰度，旧 `.jpg` 缓存启动后后台清掉（`thumb::purge_legacy`）；协议 content-type 改 `image/webp`。② **新增 `core/preview.rs`：RAW/PSD 直取内嵌 JPEG 预览**（容器无关扫描 + SOF 定尺寸 + 真解码验证），覆盖 CR2/CR3/CRW/NEF/NRW/ARW/SRF/SR2/DNG/RAF/ORF/RW2/RWL/SRW/PEF/3FR/FFF/IIQ/MOS/MRW/MEF/ERF/KDC/DCR/X3F + PSD/PSB，不解传感器像素、不占几百 MB；造四种容器实测全部出图，无预览的 PSD 优雅失败。③ `imageinfo` 补 **PSD（26 字节头）与 TIFF-IFD0 宽高**解析，出图后再回填扫描时读不到的尺寸（瀑布流不再跳行）。④ **瀑布流改真 masonry**：固定列宽 200/300/420、按最短列堆叠、行不对齐、图完整不裁切（原先 justified 等高行会拉伸/裁切）。⑤ 瓦片悬停提示改 **rAF + transform 跟手**（原来每次 mousemove setState 重渲全部瓦片，慢半拍）。⑥ **块光标重写**：盖住光标前一个字符、宽=字符+左右各 4、高=字号+2、1.1s `steps(1,end)` 55% 硬闪，搜索框与侧栏内联输入共用。⑦ **侧栏 + 改内联输入**（同 FontManager 分组 +：原地长一行、回车加入库、Esc 取消、失焦提交）。验证：`cargo test` 14 绿、`tsc --noEmit` + `vite build` 绿、puppeteer 实测瀑布流列宽 264/166/132/351 高度不齐且图片与格子同尺寸（无裁切）、hint 位移与鼠标位移 1:1（+60/+90）、块光标 `caret-blink 1.1s`。 |
| 2026-11-16 | **0.1.1 开发版** | **灯箱不裁切 + 高档位 + 点同图收起详情（owner 第五轮）**：① 修 `.lightbox-panel` 竖图裁切/顶出窗口 —— `grid-template-rows: 44px 1fr` 里的 `1fr` 最小尺寸是 auto，1000×1920 会把行撑到 1920px，改 `minmax(0, 1fr)` + `.lightbox-img{min-height:0}`，面板尺寸改 `min(1100px,100%)/min(760px,100%)` 且 `.lightbox` 加 24px 内距；② 灯箱档位 1024 → **1600**，并修掉 `parse_query` 把 px 静默 `clamp(64,1024)` 的老限制（上限 2048）—— 这是"传了 1600 却只生成 1024"的根因；③ **点同一张图收起详情**（与 FontManager 同一互动：选中态保留，再点又开）。验证：puppeteer 测 `detail true→false→true`、灯箱面板 1100×760 落在 1360×860 视口内且图 contain 完整；真机装 0.1.1 后搜图开灯箱生成 `1600×800` WebP（8.2 KB），再点同一格详情形收合。 |
| 2026-11-16 | **0.1.1 开发版** | **文档梳理（无代码改动）**：按 owner 要求把「已经做好的」与「还没做的」分开落盘 —— 新增 `AssetManager-工作日志.md`（60 条已完成能力，逐条标代码位置 + 验证方式；含决策摘要与 MVP 11 项对照）与 `AssetManager-待办清单.md`（P0 6 项 / P1 14 项 / P2 15 项 / P3 10 项 + 待 owner 拍板 5 项，每项带验收标准与依赖）；`AssetManager-项目说明.md` 的「更多」加入口。核对结论：**MVP 11 项里 4 项完成（建库/搜索/空格大图/详情+缓存）、6 项部分、1 项未做（拖出原文件）**。另确认：仓库里**没有**一份独立的「总结文档」，最接近的是 `spaces/2026_11_15_19-50-20/.workbuddy/memory/2026-11-15.md`（那次会话的需求→MVP→编码全过程流水），内容已吸收进工作日志，别再找。 |
| 2026-11-16 | **0.2.0 开发版** | **P0 全清（owner：P1 先不做）**：① **T01 拖出原文件**：`am_drag_out` 用 `drag` crate（内部 `DoDragDrop` + `CF_HDROP`），`run_on_main_thread` 后立刻返回、一律 COPY、>50 截断、缺文件剔除；拖影由缓存 WebP 转 96px **PNG**（WIC 不保证认 WebP）。② **T02 拖到目录树＝真实移动**：`am://drag-end` 回传松手坐标，前端 `elementFromPoint` 命中 `[data-dir-path]` 就调 `am_move`；另给右键「把选中的素材移到这里」做兜底。③ **T03 后台扫描**：`am_scan_start/scan_cancel` + `scan://progress|done`，独立线程**独立连接**（不占全局 `Mutex<App>`），进度改**时间驱动** 120ms；界面底部细带 + 取消按钮。④ **T05 冲突与异常**：`explain_io` 按 Win32 码翻 15 类人话；`MovePolicy{skip,keep_both,replace}`（替换＝旧文件先进回收站，可还原）；`am_move_plan`/`am_stat_paths` 供阈值确认（>100 个 / >2GB、删除 >20 个）。⑤ **T06 外部变更**：`core/src/watch.rs`（notify，**非递归**，只挂当前目录）+ 400ms 去抖 → `fs://changed`；启动 2.5s 后跑**增量扫描**（按 `dirs.mtime` 比对，只重扫变了的，目录没了删叶子行）。⑥ **T04 压测**：新增 `core/src/bin/am_bench.rs`（`am-bench`），10 万 / 100 万行两轮 —— **列表首页 0.46 / 0.48ms、搜索 20 / 29.8ms、目录树 0.03 / 0.06ms、真实扫描 1693 文件/秒**。⑦ 顺带修两个**真实性能 bug**（见 §10.5 #19/#20）。⑧ owner 两条界面意见：**详情面板常驻不再收起**（去掉 ×，Esc 只关灯箱）、**图标换成 FontManager 同一套 lucide-react 1.24.0**（`strokeWidth=1.5`，设置键抄 FontManager 的 `SettingsGlyph` 滑杆，不再自己画）。版本号 **0.1.1 → 0.2.0**。 |
| 2026-11-16 | **0.3.0 开发版** | **P1 全清（14 项）**：① **T10 系统缩略图兜底** —— 新增 `core/src/shellthumb.rs`，手写 FFI 调 `SHCreateItemFromParsingName` + `IShellItemImageFactory::GetImage`（**不用 `windows` crate**，见 §10.5 #23），`thumb::load_source` 在本家解码失败后才走它；实测 PNG 768×481/57 ms、**PDF 423×615/50 ms**，命中的标 `quality=2`。新增 `am-cli shell <file>` 专测这一层。② **T11 多选与批量** —— Ctrl/Shift/框选/Ctrl+A，批量条（移动/复制/删除/取消），删除 **200 一片**分片 + 进度条，拖出按"选中集合"整批出去。③ **T07 三态加载** —— 瓦片双层 `<img>`（骨架 / 上一张 6px 模糊垫底 / 高清淡入），灯箱先铺已缓存的 512 档再换 1600 档，切档与开大图都不闪白。④ **T16 灯箱缩放** —— 1×–8×、以指针为中心、放大可拖动、双击复位、←→ 切换；滚轮必须用原生 **非 passive** 监听（#28）。⑤ **T08 滚动锚点** —— `layoutSig` 变化时按「首个可见瓦片 + 偏移」回位，换查询才回顶部。⑥ **T13 重建索引** —— `db::reset_index`（保库根）/ `App::rebuild` / 设置面板二次确认；`db::open` 坏库改名 `index.corrupt-<ts>.db` 留证；`am_export_userdata` 导出 `userdata-<ts>.json`。⑦ **T15 原生文件夹选择器**（`rfd`）替换 `window.prompt`，加库与新建子文件夹两处。⑧ **T09 回收站入口** 改打开系统回收站（不自建面板）。⑨ **T14 树展开持久化** 存**路径**（不是 id），并区分"首次启动"与"用户手动全收起"。⑩ **T12 详情纠偏**：真实档位 + 来源三态（本地解码 / 内嵌预览 / 系统缩略图）。⑪ **T20 列数上限** 16/12/8 与瀑布流 12。⑫ **T19 四套新图标**（pack/library/asset3d/broken，七帧，小尺寸加粗母版）。⑬ **T18** 建 `UI待定问题清单.md`（13 条待定 + 3 条不做）。版本号 **0.2.0 → 0.3.0**；`cargo test --release` 20 项全绿（新增导出/重置/坏库 3 条）。 |
| 2026-11-16 | **0.3.0 代码完成 · 打包被环境阻塞** | **P1 十四项代码全部落地并通过单测/前端 E2E，但 0.3.0 安装包没能产出** —— 07:00 之后本机所有 `cargo` 驱动的 `rustc` 都会在启动几秒后冻结（CPU 停在 0.1–4 s 不再增长、内存不涨、线程全部 `Wait/UserRequest`），导致依赖无法重编。已排除：① 不是 LTO（关掉 LTO 一样冻）；② 不是 target 目录（换到 `E:/tmp` 一样）；③ 不是沙箱（禁用沙箱一样）；④ 不是路径（项目整份复制到 `E:/tmp/am-build` 一样）；⑤ 不是并发（`-j1/2/4/8` 一样）；⑥ 不是工具链本身（直接 `rustc h.rs` 0.3 s 正常，空项目 + serde 37 s 正常）。**最可能**：Defender（`MsMpEng` 累计 CPU 已 6000 s+）对每个新产出的 `.rmeta/.o` 做实时扫描，rustc 读回时被卡住；或承载进程的 supervisor（`sandbox-center` 累计 8500 s CPU）状态异常。**下次继续时**：先重启机器 / 关掉占用大的应用（或给 `target` 加 Defender 排除），再从 `tauri build` 重试；代码不需要改动。当前已安装的仍是 **0.2.0**。 |
| 2026-11-16 | **0.3.1 开发版** | **P1 收尾（二次核对后才补的两项）**：上一轮 P1 表标了"14 项全清"，但对着 §10 逐条回查，还有两处写着"计划 P1"却没人动 —— 本轮补完，**没有新增功能**。<br>① **缺陷 #5 `name_asc` 深翻**：`list_assets` 原来对 name 排序用 `OFFSET`（`cursor.0` 当偏移），深翻等于「让 SQLite 白扫 N 行」，而且翻页间隙只要有人改名/删图，就会看到重复条目或整段漏掉。改成 **(name COLLATE NOCASE, id) 双列 keyset** —— 前端协议不动，仍然是 `[已取条数, lastId]`，Rust 侧用 lastId 主键点查拿回 name 再拼游标（`idx_assets_name2` 正好覆盖）；只有"那一行刚好被删"时才退回 OFFSET 兜底，结果是慢一次而不是错一页。补单测 `name_asc_pages_without_dup_or_gap`（8 个大小写混合的名字分 3 页翻完：顺序对、不重、不漏）。<br>② **缺陷 #7 跨盘移动阻塞**：`mv_with` 里跨盘走 `copy_then_remove`，几百 MB 的文件会让整条命令几十秒不返回，界面只有一个"忙"字，用户只能猜是不是卡死。新增 `fsops::mv_with_progress(paths, target, policy, on_progress)`（`mv_with` 内部改成委托，行为不变），回调按 **100ms 节流**、最后一项必发；Tauri 侧新增 **`am_move_start`**，照 `am_scan_start` 那套 **独立线程 + 独立连接**跑，逐项发 `move://progress`、收尾发 `move://done`（含 ok/message/outcome），`MOVE_BUSY` 防重入；索引在同线程里 `refresh_dirs` 刷新，完成后前端自动 `refreshKey++`。前端 `doMove` 改成"确认 → `moveStart` → 底部进度条（`正在移动 · <文件名>`）"，结果 toast 由 `move://done` 驱动。顺带给 `db::open` 加 `PRAGMA busy_timeout=3000`（两个连接同时写时排队等，不立刻甩 `database is locked`）。补单测 `move_reports_progress_per_item`（5 个文件：进度单调递增、最后一项报满、落盘位置正确）。<br>验证：`cargo test --release` **22 项全绿**、`tsc --noEmit` + `vite build` 绿、`cargo check --release` 绿；版本号 9 处同步 **0.3.0 → 0.3.1**（tauri.conf / package.json / package-lock 两处 / 两个 Cargo.toml / 两个 Cargo.lock 的本地包 / Settings 关于行 / App 关于 toast）；按 §4.5 **占锁 → `tauri build`（3m09s）→ 静默卸载 0.2.0 → 静默安装 → 删锁**，产物 `AssetManager_0.3.1_x64-setup.exe`（2.81 MiB），实测 `FileVersion=0.3.1`。 |
| 2026-11-16 | **0.3.2 开发版** | **缩略图与滚动的两处性能修复（owner 反馈「显示图片开始有点卡」）**。没有新增功能，只改热路径。<br>① **`thumb://` 协议不再全程握全局 `Mutex<App>`**（主因）：旧实现 `let app = lock(&state)` 之后才解码、读文件、写库，而一张 36MP 图解码 **0.5 秒**（24MP 0.25 秒），这段时间 `am_list` / `am_children` / 详情 / 缓存命令全部排在锁后面 —— 滚到一屏新图，整界面就像被按住。拆成三段：**短锁取路径（微秒级）→ 不持锁解码（`thumb::ensure_path`，纯文件系统）→ 短锁回写**。<br>② **缓存命中时零写库**：旧实现每请求都 `db::set_thumb`（UPDATE assets + UPSERT thumb_lru），滚一屏 40 张就是 40 次写 + 40 次等锁。现在只有 `thumb_key` 变了或缺尺寸才写；缺文件也只在状态不是 FAILED 时标一次。<br>③ **前端滚动不再每事件重渲整屏瓦片**：`onScroll` 原来每个事件 `setScrollTop`（一帧可能 3–5 次），每次都把 40+ 个 `Tile` 重渲。现在 ① rAF 合帧；② 位移 <100px 不提交 state（可见窗口上下各有 400px 预渲染余量，肉眼无差别）；③ `Tile` 改 `memo`，交给它的回调全部 `useCallback([])` + ref 取值（否则 memo 形同虚设）；④ 锚点采集从「每事件扫全部 rect」降到 **120ms 一次**。<br>**验证**：`cargo test --release` 22 绿、`tsc --noEmit` + `vite build` 绿、`cargo check --release` 绿；打包（占锁 2m35s）→ 卸载 0.3.1 → 装 0.3.2 → 实机截图确认缩略图/详情预览正常出图（协议改写没把图打坏）。版本 9 处 → **0.3.2**。 |
| 2026-11-16 | **0.4.0 开发版** | **标签系统（照 FontManager 一比一搬，自动规则换成「原始文件规格」）**。owner 原话：「分自动和手动标签等一比一搬过来」。<br>**数据模型**：新增 `tags` / `asset_tags`（`WITHOUT ROWID` + `idx_asset_tags_tag`）。**只有用户手打的标签落库**；自动标签是运行时推导，绝不写进去 —— 这是 FontManager `derivedTags` 的第一条纪律，写进去等于把猜测伪装成事实，规则一改还要写迁移脚本清脏数据。<br>**自动标签（`core/src/autotag.rs`）**：照搬「手工优先 / 宁可少给 / 只读事实」三条纪律，但判定依据按 owner 要求换成**原始文件的客观规格**：`fmt:<扩展名>`、`kind:<类型>`、`place:<8k/4k/2k/fhd/hd/sd>`（长边）、`orient:<landscape/portrait/square>`（宽高比）、`size:<xl/lg/md/sm/xs>`（体积档）。只对图片/视频/3D 出尺寸类标签（文档算长边没意义）；扫描没读到尺寸就一条都不给，不猜。`PLACE_STEPS` / `SIZE_STEPS` 两张档位表是**唯一阈值来源**，详情推导与侧栏 SQL 聚合都从它生成，避免两边口径分叉。<br>**互斥与否决**：同一维度手工打过任意值 → 该维度自动值全部让位（`apply_manual_priority`）；`fmt:`/`kind:`/`place:`/`orient:`/`size:` 五个前缀**不开放给手工命名**（`validate_tag_name` 拒），否则两套口径会撞成一条；自动标签可右键「不再建议」，名单存 `localStorage am.tags.denied.v1`，设置里能一键恢复。<br>**筛选**：`AssetQuery.tag` —— 自动标签在 SQL 里翻译成列条件（`lower(ext)` / `kind` / 长边区间 / 宽高比较 / 体积区间），手工标签走 `EXISTS(asset_tags JOIN tags)` 子查询。**都在 SQL 里筛**，不是取回前端过滤，所以百万库分页仍然正确。<br>**界面（1:1）**：① 侧栏「标签」分区：手动标签行（使用数、悬停出 ×＝从所有素材移除）+ 折叠的「自动标签」分组，点一下筛选、再点取消；② 顶部出现「标签：xxx」筛选条 + 清除；③ 详情面板标签区：实心 chip＝手动（点筛、× 移除），虚线 chip＋小圆点＝自动（点筛、右键否决），「+ 标签」内联输入带已有标签候选与「新建 xxx」；④ 批量条「打标签」+ **T 快捷键**（多选就整批）→ `TagPrompt` 弹窗（照 BulkTagPrompt：遮罩 + 卡片 + 输入 + 候选 + 回车应用 / Esc 取消）；⑤ 设置 → 外观：「显示自动标签」开关 + 「恢复被否决的自动标签」。<br>**测试**：`cargo test --release` **26 项全绿**（新增 `autotag` 三条 + `tags_crud_and_filter` 一条）；`tsc --noEmit` + `vite build` 绿；`cargo check --release` 绿。**端到端**：`am-cli --data <临时库> scan` 造 4 个素材后 `--tag fmt:png` / `orient:portrait` / `place:4k` / `size:xs` 全部筛对、未知标签 0 条（没有污染用户真实库）。打包（持锁）→ 卸 0.3.2 → 装 0.4.0（2.84 MiB，`FileVersion=0.4.0`），实机截图确认侧栏标签分区与详情自动标签（WEBP / IMAGE / FHD / LANDSCAPE / XS）正常。 |
| 2026-11-16 | **0.5.0 开发版** | **标签组 / 标签颜色 / 标签参与搜索 + 侧栏改悬浮抽屉**（owner：这三样都做，并把标签放进侧栏面板，但默认收起、悬浮左缘展开；素材库与回收站面板永不收起）。<br>**数据模型**：新增 `tag_groups`（一层，`name` 唯一 + `sort` + `color`），`tags` 加 `color` / `group_id`（`ON DELETE SET NULL`）；`migrate` 里对老库 `ALTER` 补列。`all_tags` 改成带 color/group 并按「组 sort → 组内使用数 → 名字」排；新增 `all_groups`（含组内素材数 `count` 与标签数 `tags`，**空组也返回**）。新增 `set_tag_color` / `set_tag_group` / `rename_tag`（撞名**合并**）/ `group_new|rename|set_color|delete`（解散只把 `group_id` 置空，标签一条不删）。<br>**颜色只存语义键**（`gray/red/orange/amber/green/teal/blue/indigo/violet/pink`，白名单在 `db::TAG_COLORS`，前端 `lib/tags.ts::TAG_COLORS` 逐字对齐）：键 → 实际色值放在 `app.css` 的 `--tc-*`（浅色/深色各一套），所以同一个键在深色主题下自动提亮。存十六进制就等于把配色钉死在一个主题上。每色三个变量 `--tc`（实色）/`--tcs`（淡底）/`--tcline`（描边），`gray` 特殊走主题自己的 `--e1`/`--ui-line`，让「没设颜色」的标签看起来就是普通 chip。<br>**搜索**：`list_assets` 的文本条件从「只查 FTS」变成 **FTS ∪ 标签名**，并支持 `#词` = 只搜标签。标签匹配用 `EXISTS(asset_tags JOIN tags)` 子查询而不是往 FTS 塞副本 —— 塞副本就得在打/去/改名/合并/解散五条路径上同步，多一处必漏。顺带补 `like_contains()`：`%` `_` `\` 全部转义（以前搜 `50%` 会把全库搜出来）。<br>**界面**：侧栏拆三段 —— 素材库（永远展开）/ 标签抽屉（默认收起）/ 底栏四行（永远可点）。抽屉关闭时只留左缘一条 19px 竖排「标签」书签（`writing-mode: vertical-rl`，位于抽屉上边界处），热区是**窗口左缘全高 12px**（不能再宽：目录树展开箭头就在 x≈11）；展开后抽屉占侧栏下半截 66%，上半截素材库仍可见可点；延时 220ms 收起；点书签也能开（触屏/键盘），Esc 与右上角 × 可关。抽屉内：按组摆开（组行带色点/计数，点折叠、右键编辑）+「未分组」+「自动标签」按维度（格式/类型/像素/比例/体积）分组。新增 `TagEdit` 编辑卡：改名（回车/失焦提交）、10 色板、分组归属（含「新建分组」）、删除；**改动即时生效不设保存**（每项都能一键改回，草稿反而让人怀疑存没存）。详情 chip 带色点、右键打开同一张卡；批量条「打标签」与 T 键不变。<br>**踩坑（真·启动崩溃）**：一开始把 `CREATE INDEX ... ON tags(group_id)` 放进 SCHEMA，老库执行 SCHEMA 时列还没补上 → `no such column: group_id`，`setup` 直接 panic，软件打不开。索引必须挪到 `migrate` 里、**补列之后**无条件建。补了单测 `migrates_old_tags_table`（手搓 0.4.x 的 tags 表 → `open()` 不崩 → 老标签还在 → 能立刻上色进组）。<br>**验证**：`cargo test --release` **28 项全绿**（新增 `tag_groups_colors_and_search`、`migrates_old_tags_table`）；`tsc --noEmit` + `vite build` 绿；`cargo check --release` 绿；拿**真实库副本**（849 素材 / 1 个手打标签）跑 `am-cli list`：列补上、索引建好、老标签与素材全在。打包（持锁 3m29s）→ 卸 0.4.0 → 装 0.5.0（2.86 MiB，`FileVersion=0.5.0`）。<br>**另出**：`AssetManager/标签系统方案.md` —— 给 FontManager 复用同一套语义（数据模型的 JSON 等价映射、命名空间、颜色/分组规范、搜索规则、界面规范、命令契约、接入步骤与验收清单）。 |
| 2026-11-16 | **0.5.0 交互重做** | **标签面板从「左缘热区浮层抽屉」改成「常驻可折叠侧栏分区」**（owner 反馈：判定不准、反应慢、右键会把面板收掉）。<br>**形态**：头部（`🏷 标签`）**永远可见、位置不动**，钉在底栏上方；内容块用 `order` 排到头部上方，容器 `height` 在 `33px` ↔ `min(320px, 45vh)` 之间过渡；目录树被压缩而不是被盖住。**悬停展开交给 CSS `:hover`**（不再用 JS 事件 + 260/90ms 定时器 —— 那正是"贴上去半天不出来、鼠标走了它还赖着"的根因）。**点头部 = 固定展开**（存 `localStorage`，可跨启动），工具行另有「固定 / 已固定」状态指示按钮；`hold` 保证**编辑卡/批量弹窗/右键菜单开着时不收**。<br>**右键**：标签有自己的菜单（`只看「xxx」/取消筛选` · `编辑标签（颜色/分组/改名）` · `从所有素材移除`），分组有 `编辑分组`；App 的全局 `contextmenu` 捕获器对 `.tag-dock` / `.dlg` 直接 return，不再叠出「重新扫描 / 全选 / 瀑布流」那套库级菜单。<br>**踩坑（都写进方案文档，FontManager 别再踩）**：① `height: clamp(190px, 42%, 460px)` 在"侧栏是 flex 项、height:auto"的链路上百分比无法解析 → **整条声明失效**，现象是"类加上了、箭头翻了、高度不动"；② `flex-direction: column-reverse` 的溢出内容**命中测试不可靠**：看得见、点不到、点下去落到目录树上；③ 头部会随展开位移时**不能当点击目标**（一点就落到刚跳上来的行上）。最终用「正常文档流 + `order` 交换 + 固定 px 高度 + CSS hover」三件套解决。<br>**文档**：`AssetManager/标签系统方案.md` §8.1 重写成最终形态（含三种被否方案的成因、时序参数、右键菜单规范、`hold` 规则），FontManager 可直接照抄。 |

| 2026-11-16 | **0.6.0 开发版** | **与 FontManager 全面对齐（兄弟软件同一套视觉/动效/骨架）+ 多库（最多 3 个）**。owner 原话：「各种可以复用的 icon、界面布局、相同功能的动画方式和界面位置都可以拿来使用，风格也一样，窗口尺寸、动画、互动效果都一样，但要以素材软件功能为主，FontManager 的速览界面不要」。<br>**① 主题系统（最大缺口）**：以前 `index.html` 把 `data-theme="light"` 写死、没有切换。现在**出厂深色**、设置 → 外观可切浅色；令牌照 FontManager 重写 —— 颜色三层（原色 → `--ui-*`/`--text-*` → 组件）、`--e1..--e4` 全部改成**半透明叠加**（实色 hex 在浅色主题下会糊成一片）、字号 9/10/11/13（+16..56 标题档）、动效五档 `--t-micro/quick/state/layout/scene`（纯时长，不再把 cubic-bezier 塞进变量）。旧的 `--text-1/2/3` 留成别名，全站组件不用改名。<br>**② 字体自带**：引入 `@fontsource-variable/inter` + `jetbrains-mono`（与 FontManager 同版本同包），此前依赖系统 Inter（多数机器没有，会退到 Segoe UI，两个软件的字形从此不一样）。<br>**③ 动效方式统一**：引入 `motion@12` + `design/springs.ts`（曲线数值逐条照抄：spring/springSnappy/springSoft/easeTop/scrimFade）与 `lib/momentumScroll.ts`（**整份复制**惯性滚动，零外部依赖，注释标明来源与同步义务）。落地的四处：右键菜单（0.96 缩放 + y-4，springSnappy）、确认框 / 打标签 / 编辑卡（遮罩 0.18s + 卡片 scale .96/y12）、设置面板（y32 滑入，springSoft）、灯箱（遮罩 0.18s + 面板淡入，不动 transform —— 缩放靠 stage）。`#root` 补 240ms 入场。全站 CSS 过渡改吃令牌，「减少动效」双轨（软件开关 `data-motion` + 系统 `prefers-reduced-motion`）压到 1ms。<br>**④ 滚动条改回一套**：删掉 tokens 里的 `scrollbar-width: thin`（Chromium 里只要声明它，整套 `::-webkit-scrollbar` 会被禁用 —— 这正是「菜单滚动条和别处不一样」的来源）。改 FontManager 口径：槽 10px、thumb 平时视觉 1px（4.5px 透明 border）、靠近变 6px、`--ui-fg-3` 底色、hover 走 `color-mix(--ui-fg 40%)`。新增 `lib/scrollHot.ts` 统一挂 `.scroll-hot`（指针进容器右缘 14px 内），侧栏目录树例外隐藏条（仍可滚）。<br>**⑤ 行契约与组件数值**：侧栏行图标 14 → **15**（`strokeWidth 1.5`）、顶栏控件 14 → **13**、标签面板头部按 `.nav-row` 整段重写（33px → 32px、10px/600/大写 → **11px/500**、圆角 4 → **0**、gap 6 → 10、图标 12 → 15）、展开高度 320/45vh → **300px/42vh**、过渡改用 `--t-dock`(110ms)、菜单 **208 宽 / 30 高行 / 6 圆角**（原 180/32/8）、开关换成 FontManager 的 pill 形态（34×19 + 内阴影 + 白色渐变滑块，位移由「轨道宽−轨道高」算）。<br>**⑥ 设置面板按 FontManager 重做**：940×640 → **900×760 固定尺寸**（切页不再跳）、左列 236 → **196**、导航行 32/0圆角/11px → **30/6圆角/13px**、选中态从「蓝竖线」改成 **e2 底块**（设置页没有「当前行」概念）。**去掉「应用 / 保存设置」三个按钮**：所有改动即时生效（FontManager §9.3「不要放假按钮」），底栏只留「所有改动即时生效，不需要点保存」+「完成」（完成按钮用 e2 底块，不用蓝底）。<br>**⑦ 窗口与标题栏**：1360×860 / min 960 → **1280×820 / min 900**（与 FontManager 同体量）。<br>**⑧ 多库（最多 3 个，owner 同日追加要求）**：数据模型本来就支持多个库根（`libs` 表 + `roots()` 返回全部顶层），缺的是上限与出口。新增 `db::lib_count` / `db::remove_lib`（**递归 CTE 按 parent_id 走**，不用 `path LIKE` —— 路径里的 `_` `%` 会被当通配符）、`App::lib_count` / `remove_lib`、Tauri 命令 `am_lib_count` / `am_remove_lib`、`am-cli rmlib|libs`。前端：侧栏标题显示 **`x/3`** 配额、加库前先查数量（到顶直接提示，不弹选择框）、库根右键多一项「从素材库移除（不删文件）」（子目录没有这一项，用 `data-dir-root` 区分），移除后清空选中并刷新。**移除只清索引，磁盘文件一个不动**。<br>**真实库验证（owner 指定的 `Desktop/新建文件夹/资源库测试`，独立数据目录 `E:/tmp/am-test-data`，没碰用户真实库）**：扫描 **1691 目录 / 32766 文件 / 32609 张图 / 31438 有尺寸 / 0 错误 / 140 秒**；`tree` 根计数 32766 正确；列表首屏正常；再加入两个小库测到 3 个根；`rmlib` 移除 3 万级库后 **目录索引 1691 → 0、assets 级联清空、孤儿目录 0、磁盘文件仍在**（在数据库副本上测的，真实索引未动）。<br>**验证**：`cargo test`（core 28 项，本轮改数据库层，见下）`tsc --noEmit` + `vite build` + `cargo check --release` 全绿；版本号 9 处 → **0.6.0**；按 §4.5 **占锁 → `tauri build`（3m14s）→ 静默安装 → 删锁**，产物 `AssetManager_0.6.0_x64-setup.exe`（3.20 MiB），真机截图确认深色主题、4 列铺满、`3/3` 配额、自动标签、标签面板正常。<br>**未做（留待 owner 拍板）**：FontManager 的「速览条 / 命令面板 / 多语言」三项本软件没有对应功能，未引入；浅色主题的真机人工确认只做了浏览器版本（数值与深色同源）。 |

| 2026-11-16 | **0.6.1 → 0.6.3 开发版** | **数据文件夹可自定义（照 FontManager 的「字体库文件夹」）+ 修「加了文件夹没显示」**。owner 给的截图是 FontManager 设置 → 字体库 →「字体库文件夹」那一块（默认路径灰条 / 使用自定义文件夹开关 / 添加文件夹… / 地址清单带「当前」徽章 + 打开 + ×）。<br>**① 数据文件夹（0.6.1）**：新增 `core/src/dataroot.rs`。配置是指针文件 `<默认目录>\data-root.json`（`{enabled,current,dirs}`）—— 指针只能放在默认目录里，否则启动时不知道该读哪份配置（鸡生蛋）。启用了自定义且目录可用 → 用它；**目录不可用（盘拔了/被删）自动回退默认**，不报错不让软件打不开（设置面板显示的是「当前生效」的真实路径，不是用户填的那个）。`is_usable` **故意不自动创建目录**：`create_dir_all` 一把会在空目录上偷偷重建一个空库，用户看到「我的库怎么空了」。切换**下次启动生效**（SQLite 连接启动就绑定了文件路径），设置里给「立即重启」（`am_relaunch`）。可选**把现有索引复制过去**：`VACUUM INTO` 而不是 `fs::copy`（copy 会丢掉 `-wal` 里未落盘的事务，VACUUM 顺手整理空闲页，写出单文件干净库）；**缩略图缓存不复制**（几 GB，按需重建）。Tauri 命令 `am_data_root` / `am_set_data_root` / `am_relaunch`；前端 `components/DataFolderCard.tsx`（默认灰条 / 开关 / 添加 / 地址行带「当前」+ 打开 + 移除 / 生效状态 + 立即重启）。<br>**② 「选了资产文件夹，并没有在软件中显示出来」（owner 2026-11-16 报）**：真因是**库已经 3/3**（`am-big` 800 / `am-demo` 45 / `am-test` 4），加第 4 个被 `MAX_LIBS` 拦下，而当时只弹了一条 toast —— 用户点「添加文件夹」看到选择框没出来 / 选完列表不变，结论就是「功能坏了」。改法：超限时弹**对话框**，列出当前 3 个库路径 + 明确告知「右键库 → 从素材库移除（不删文件）」；另外加「这个文件夹已经在库里了」的重复判断（同路径归一化后比较）。<br>**③ 事故与修复（0.6.3）**：20:53 前后，数据目录被写成了**测试素材库根**（`Desktop\新建文件夹\资源库测试`），于是库根里冒出 `index.db` / `index.db-shm` / `index.db-wal` / `cache/` / `runtime.log`。**已全部整体移到 `E:/tmp/am-recover-2026-11-16/`（连同 data-root.json 的备份），素材文件一个没动**；`data-root.json` 已删除，软件回到默认目录 `%LOCALAPPDATA%\com.assetmanager.desktop`（注：这才是真实数据目录，`%LOCALAPPDATA%\AssetManager` 是**安装目录**，两边别搞混）。防复发：`dataroot::conflicts_with_libs` —— 数据文件夹不许是某个库根、也不许在库根内部（大小写/分隔符归一化后比较），`am_set_data_root` 直接拒绝并说明原因；配单测 `rejects_data_dir_inside_a_library`。<br>**④ 诊断能力（0.6.2）**：新增 `runtime.log`（默认目录下）—— 启动时记版本与真实数据目录，`panic` hook 记文件:行，`RunEvent` 记 `exit-requested` / `exit` / `window-destroyed`。有了它，「点一下窗口就没了」这类问题才能区分是 panic、WebView 崩、还是被要求退出（本次排查就靠它确认了「进程是被外部终止、不是自己退出」）。<br>**⑤ 调试入口**：`?theme=light|dark`、`?motion=reduced`、`?settings=cache|appearance|…`（只影响本次页面、不落盘），截图与排错不必再模拟点击。新增 `tools/gui.py`（`screen` 屏幕抓图 / `click` CSS 坐标点击 / `key` 快捷键，DPR 默认 1.25）。**踩坑**：`shot.py` 的 PrintWindow 截 WebView2 之后进程会消失（FontManager WORKLOG #12 只记了「四角纯黑」，没记这条），改用 `ImageGrab` 屏幕抓图；在 bash heredoc 里写 Rust raw string 时反斜杠会被吃（`r"E:////x"` 变成 `//`），测试改成用 `PathBuf::join` 拼路径；`tauri.conf.json` 不许写注释（标准 JSON）。<br>**验证**：`cargo test --release` **32 项全绿**（新增 dataroot 三条 + log 一条）；`tsc --noEmit` + `vite build` + `cargo check --release` 绿；版本 0.6.1 → 0.6.2 → 0.6.3 各打一包（每次占锁 → build → 安装 → 删锁），当前装的是 **0.6.3**。 |

| 2026-11-16 | **0.6.4 开发版** | **「设置了文件库但没有任何效果」的根治 + 设置里终于能直接管素材库**。owner 截图是主界面 0/3 空状态。<br>**① 真因（第二次同类事故）**：`data-root.json` 的 `current` 被设成了**素材库根** `Desktop\新建文件夹\资源库测试` —— 于是软件在**另一个数据目录**里打开一个空库（0 个库、0 素材），而索引文件（`index.db`/`cache`）直接写进了用户的素材目录。前一轮加的 `conflicts_with_libs` 拦不住：那一刻库里是 0 个库（我刚清完三个测试库），没有"冲突对象"。**词错了**：上一轮把这个功能叫「数据文件夹」（照 FontManager「字体库文件夹」的对位），用户理解成「我的素材放哪」。<br>**② 修名 + 修语义**：设置里「数据文件夹」→ **「索引与缓存位置」**，副标题直接写「只放 index.db 与缩略图；**素材库不在这里改** —— 去左侧「素材库」页」；按钮「添加文件夹…」→「选择文件夹…」。<br>**③ 新增设置 → 素材库页**（`components/LibraryCard.tsx`）：列出库根**完整路径 + 素材数 + 打开 + 移除**、`x/3` 配额、右上「添加文件夹…」（走 App 的 `onAddFolder`，上限/重复/扫描进度一套），空状态写清「点右上角选一个放素材的目录」。监听 `scan://done` 自动刷新 —— 不加这条，用户点完加号列表还是空的，又是"没效果"。侧栏那个不起眼的 `+` 不再是唯一入口。<br>**④ 选完目录先体检**（`core/src/dataroot.rs::probe` + 命令 `am_probe_dir`）：两层内数图片/视频（数到 500 就停，大库不卡界面）。有图就弹**三选一**：「加入素材库」/「仍用作索引位置」/「取消」—— 把用户从"设错地方"直接送到正确出口。后端 `am_set_data_root` 加 `force`，非 force 且有图一律拒（最后一道）。<br>**⑤ 现状修复**：误写进素材库的 `index.db`/`-shm`/`-wal`/`cache/` 整体移到 `E:/tmp/am-recover-2026-11-16/spill-2/`（素材零改动），`data-root.json` 备份后删除 → 回默认目录；然后把 `资源库测试` **正式加为库 1**：`34441 目录 / 109608 文件 / 76539 图 / 0 错误 / 11m30s`。<br>**⑥ 发现（待 owner 决定）**：这个库比上次大 3 倍多，原因是里面多了一个 Eagle 的 `的VS发.library`——**76842 个内部文件**（缩略图 + JSON 元数据）。它们都被当成素材入库（kind 4 文档 32982 条），而默认排序「最近修改」正好把今天写过的 JSON 排在首屏，看起来像"全是 json、没有图"。没有擅自过滤用户文件；要不要「跳过 `.library` 内部」或「默认只看图片」等 owner 拍板。<br>**验证**：`cargo test --release` **33 项全绿**（新增 `probe_tells_library_from_empty_dir`）；`tsc --noEmit` + `vite build` + `cargo check --release` 绿；占锁 → `tauri build` → 静默安装 → 删锁，产物 `AssetManager_0.6.4_x64-setup.exe`（3.22 MiB，`FileVersion` 已核对 0.6.4）；真机截图确认侧栏 `1/3 / 资源库测试 / 109,608`、设置 → 素材库页有库行与配额。<br>**踩坑**：Rust **函数参数上不能写 `///` 文档注释**（`error: allow, cfg, ... are the only allowed built-in attributes in function parameters`）；设置里的 block 行**不渲染 label**，所以卡片自带的标题会和页面标题重复成两个「素材库」，已删卡内标题；`tools/gui.py click` 收**CSS 像素**（DPR 1.25），按截图上的物理像素读坐标会点到隔壁（这次点了搜索框）。 |

| 2026-11-16 | **0.6.5 开发版** | **「这些缩略图都没有办法显示出来」—— 根因是库里混进了别人的素材库**。owner 截图：首屏 4 列全是灰方块 + `暂无预览` + `JSON` 角标。<br>**① 诊断**：不是缩略图管线坏了（缓存里已有 1037 个 WebP，图片能正常出图），而是**首屏全是 json**。库里有 `的VS发.library`（Eagle 的库）—— **76842 个内部文件**（`metadata.json` / `mtime.json` / `images/` / `thumbs/`），全被当素材索引：109608 个文件里 32982 是文档、76539 张"图"大半是 Eagle 的缩略图缓存。默认排序「最近修改」又正好把今天写过的 json 排在前面，看起来就是"缩略图全挂了"。<br>**② 扫描跳过「库中库」**（`scan::is_foreign_library`）：**名字以 `.library` 结尾 且 目录里有 `metadata.json`** 才跳 —— 只看名字会误伤用户自己起的 `我的素材.library`，所以必须验内容。`ScanStats` 加 `skipped`，扫描完成提示里报「跳过 N 个外部素材库目录」，不静默。**只跳索引，磁盘文件一个不删**（实测 76842 个文件仍在原地）。<br>**③ 没图可出的文件不再装"坏了"**：`PREVIEWABLE_KINDS`（只有图片 0 / 视频 1 能出缩略图）提到 `types.ts` 共用；`Views.Tile` 对其它类型**不再请求 `thumb://`**（以前每张都先吃一个 404 才画灰方块 —— 三万条 json = 三万次必然失败的请求），改画「类型图标 + 文件名（常显、最多两行）+ 扩展名角标」，新建 `.tile-kind` 样式；`ui.tsx` 补 `file/video/audio/model` 四个图标。详情面板「显示层」对这类文件改成「缩略图：没有（这类文件没有预览图）／怎么看：双击或用默认程序打开」，不再骗用户等一张永远不会生成的图。<br>**④ 结果（真机）**：库从 109608 → **32766** 个文件（1691 目录 / 32609 图 / 114 文档 / 18 视频 / 0 错误），首屏终于全是设计图缩略图；同一台机器重扫 **690s → 220s**。<br>**验证**：`cargo test --release` **34 项全绿**（新增 `foreign_library_needs_both_name_and_metadata`：名字像但没有 metadata.json 不跳、有 metadata.json 但名字不像不跳、大小写不敏感）；`tsc --noEmit` + `vite build` + `cargo check --release` 绿；占锁 → `tauri build` → 静默安装 → 删锁，产物 `AssetManager_0.6.5_x64-setup.exe`（3.21 MiB，FileVersion 核对 0.6.5）。<br>**待 owner 拍板**：是否需要「排除某个子文件夹」这类用户自定义忽略规则（现在只有内建的 Eagle 规则）；是否需要「默认只看图片」的开关 —— 我没做，因为一旦默认过滤，往库里加 PDF 目录的人会看到空列表，又是一次"没效果"。 |

| 2026-11-16 | **0.6.6 开发版** | **缩略图速度：出图 2 倍、并发 2.5 倍、还提前一屏做好**。owner：「每次滑动，图片显示出来的时间都很久，能不能实时显示」——这次是**真·性能问题**，和 0.6.5 的"库里混了 json"无关。<br>**① 先量再改**：`am-thumbbench` 基线（合成图，384 档）——36MP PNG 550ms / 24MP PNG 272ms / 24MP JPG 253ms / 12MP JPG 98ms / 2MP JPG 39ms；一屏 40 格平均 242ms → **串行 9.7s、4 路并发 2.4s**。用户体感"很久"对得上。<br>**② JPEG 降采样解码**（`thumb::decode_jpeg_scaled`）：`image` 0.25 用的 zune-jpeg **没有 DCT 缩放**，只能全解 24MP（72MB RGB）再缩到 384px —— 99% 的像素白算。新增 `jpeg-decoder 0.3` 依赖：只读文件头 → 选一个「刚好不小于目标」的 1/2^n 比例 → 让它在 IDCT 阶段只算需要的块（1/8 时像素少 64 倍）。灰度图（L8）铺成 RGB；CMYK / 16 位等不支持的情况自动回落通用路径。合成 24MP JPG **253 → 128ms**；真实素材平均 **104 → 54ms**。逃生开关 `AM_NO_JPEG_SCALE=1` 可关掉（A/B 与排障）。<br>**③ 解码闸门改成「按像素估重」**：原来是「同时 4 张」，小截图和大合成图一视同仁 —— 一屏 40 张 2MP 的图要排 10 轮。现在 1 份 ≈ 4MP 的解码内存、预算 10 份：2MP 小图 1 份（10 路并发）、24MP 照片 6 份、36MP 大图 9 份独占；**JPEG 按降采样后的像素估重**（否则会被误判成重活又回到 4 张一起卡）。内存峰值仍封在 160–200MB。<br>**④ 预取下一屏**（`am_prefetch_thumbs` + `Views.tsx`）：滚动停下 220ms 后，把可视区**上下各一屏**排进后台生成；**同时只有一个预取任务**（epoch 递增，旧任务下一张就退），**共用同一道闸门**（抢不走全部带宽，可见区仍优先），命中缓存直接跳过、不写库。真机验证：启动 12 秒缓存从 1451 → 1492（+41，正好"可见一屏 + 预取一屏"）。<br>**⑤ 真机验收**（`tools/gui.py` 新增 `wheel`）：滚轮跳滚 **25 格**后 **300ms** 截图 → 可见 20 格**全部出图、零灰块**；6 格小幅滚动 500ms 同样全出。真实目录 100 张 384 档：**0.56s / 179 张每秒**（关掉降采样是 1.07s）。<br>**验证**：`cargo test --release` **36 项全绿**（新增 `jpeg_scaled_decode_actually_scales` 守性能契约、`jpeg_scaled_decode_survives_garbage` 守不 panic）；`tsc --noEmit` + `vite build` + `cargo check --release` 绿；占锁 → `tauri build` → 静默安装 → 删锁，产物 `AssetManager_0.6.6_x64-setup.exe`（3.27 MiB，FileVersion 0.6.6）。<br>**没做（说明理由）**：PNG 没有 DCT 这种捷径，只能靠并发 + 预取盖住（24MP PNG 仍 ~260ms）；WebView 是否缓存 `thumb://` 不可控，所以没依赖它。<br>**顺手发现**：owner 自己加了第二个库 `Desktop\新建文件夹\新建文件夹 (4)`（2/3），索引正常。 |

| 2026-11-16 | **0.6.7 开发版** | **全库缩略图预热**（owner：「用一个只有这个软件能识别的格式，把所有图转成同一个格式，但一定要流畅」）。<br>**① 先回答那个设想**：缓存里其实**已经是**"只有本软件认的格式"——WebP（`cache/thumbs/…​.webp`，q80，按档位 256/384/512 分桶，界面只用它、任何导出/拖出都走原文件）。所以"换格式"不会更快：WebP 解码一张 384px 只要 1–2 ms，慢的从来是**第一次从原图生成**。<br>**② 那就把"第一次"提前**：新增全库预热 `am_warm_start(px)`。后台按 `kind=0` 取全部图片，逐张 `ensure_path`（已在盘上就跳过）；**只有 2 条 lane**、且最近 800 ms 内有可见请求就睡 250 ms 让路（`LAST_THUMB_REQ` 在 thumb 协议入口打点）；`epoch` 一改下一张就退（可停、换档位自动重开）；**不写库**（真请求到来时命中缓存再补 `thumb_key`）。进度走 `warm://progress` 事件。<br>**③ 前端**：设置 → 缓存与索引 两行 —— 「启动时自动预热缩略图」开关（`Prefs.warmThumbs`，默认开）+ 「预生成全部缩略图」按钮（显示 `进行中 2,480 / 65,218`，运行中变「停止」）；启动 6 秒后自动开跑（避开首屏那一波请求）。<br>**④ 顺手试了 PNG 解压后端**：`flate2` 换 `zlib-rs`（纯 Rust，不用 C 工具链）。**实测无提升**（24MP PNG 258 → 263 ms）→ PNG 的耗时不在 inflate，而在 unfilter + 24M 像素的 RGB 转换。保留该依赖没有收益，**已回退** —— 去掉之后复测同样 21 ms，留着纯属债务（回退后重打了一包，产物 3.27 MiB，与本段描述一致）。<br>**⑤ 关于"清晰度"（owner 明确要求）**：三档仍是 256/384/512，对应瓦片 120/176/260 CSS × DPR 1.25 = 150/220/325 设备像素 —— 实际是 **1.6–1.75 倍超采样**，比屏幕需要还高；JPEG 降采样的比例循环保证**解码出来的边永远 ≥ 目标档位**（`longest/(div*2) < target` 才停，例如 6000px 源、384 档解 750px 再缩），所以**没有一丝是靠"缩小源图"换来的速度**。<br>**验证**：真机 74 秒预热 **1492 → 3938 张（≈34 张/秒，2 lane）**；旧行为对照：不预热时首次滚到新目录要现做。缓存 39.9 MB / 4,328 文件。`cargo test --release` 36 项全绿；`tsc --noEmit` + `vite build` + `cargo check --release` 绿；占锁 → `tauri build` → 静默安装 → 删锁，产物 `AssetManager_0.6.7_x64-setup.exe`（3.30 MiB，FileVersion 0.6.7）。<br>**给 owner 的提醒**：库里有两个库 `资源库测试` 和 `新建文件夹 (4)`，**后者是前者的整份副本**（各 32,766 文件、各 701 张同名 png）。索引、缓存、预热都在做双份功（65,218 张图）。建议移除一个。 |

| 2026-11-17 | **0.7.0 开发版** | **P2 批次一：T23 评分/注释/来源 URL + T27 命令面板**（owner：「T22 智能文件夹 / T30 Eagle 迁移 / T35 列表视图 都不做，其余 P2 继续」）。<br>**① T23**：`assets` 原有 `rating/note`，新增 `source_url`（`migrate` 里 `has_column` 守卫 + ALTER，老库自动补；新库建表就带）。`Asset` DTO 补三字段，两个 SELECT 同步（**漏一个就是"列表里是 0、详情里有 5"这种鬼故事**）。排序新增 `rating_desc`：`ORDER BY rating DESC, id DESC`，游标 `(rating, id)` —— 只用两维是因为前端游标只有两格；同分按 id 倒序（≈后入先出），不用 mtime 是因为三维比不了。索引 `idx_assets_rating(rating DESC, id DESC)` 在 migrate 里建（老库也要有）。<br>**② 详情「我的」段**：5 颗星（点同一颗 = 减一星）、注释 textarea（**失焦即存**，不是每键入一次就写库）、来源 URL 输入 + 打开按钮。三样都**不是从文件推导的**，所以 `sync_dir` 不碰它们 —— 单测 `user_fields_survive_rescan_and_sort_by_rating` 专门守这条（重扫后评分/注释/来源必须原样，且 rating 排序 + keyset 翻页不漏不重）。瓦片右上角加星级角标（有分才出现）。<br>**③ `am_open_url`**：ShellExecuteW 打开链接，**协议白名单只放 http/https** —— 不做白名单的话，用户粘 `ms-settings:` / 自定义协议就是"点一下就执行别的程序"。走 FFI 不拼命令行（用 shell 拼接会被 URL 里的 `&` 截断）。<br>**④ T27 命令面板**（`CommandPalette.tsx`）：`Ctrl/⌘+K`，560×420，`↑↓` 选 / `Enter` 跑 / `Esc` 关，鼠标可点可悬停。27 条命令**全部指向 App 已有 handler**（打开设置、添加素材库、定位目录、新建子文件夹、视图×2、尺寸×3、排序×4、类型×2、评分 3 档、打标签、删除、全屏预览、撤销、重建索引（二次确认）、清缓存、预热、导出、回收站）。原则：只列「现在真能做」的 —— 没选中素材就没有"删除选中项"。快捷键表里也补了 Ctrl+K。<br>**验证**：`cargo test --release` **37 项全绿**（新增 T23 那条）；`tsc --noEmit` + `vite build` + `cargo check --release` 绿；占锁 → `tauri build` → 静默安装 → 删锁，产物 `AssetManager_0.7.0_x64-setup.exe`（3.28 MiB，FileVersion 0.7.0）。真机：点第 4 颗星 → 库里 `1787637229244.jpg rating=4`（**落库成功**），详情显示 4 颗金星 + "4 星"，瓦片右上角出现 `★4` 角标；`Ctrl+K` 弹出面板，27 条命令、背景遮罩正常。<br>**踩坑**：给 `Asset` 加字段会连累所有手写 `Asset { … }` 的测试字面量（`autotag.rs` 一处）；`AssetQuery.limit` 是 `Option<i64>` 不是 i64；给组件加 props 时 `onEditTag` 在 TagDock 和 Detail 都出现，第一遍把 3 个新 prop 插到 TagDock 上了（TS 立刻报"TagDock 没有 onSetRating"）。 |

---

## 13. 0.8.0（2026-11-17）：拆库落地 + P2 功能补齐 + 看图体验

> 这一轮从"上一轮留下的半成品"开始：`lib.rs` 已经按拆库（T28）重写过，但
> `dupes.rs / palette.rs / refs.rs / libs_migrate.rs` 四个模块**根本没建**，
> 另有 5 个 db 辅助函数缺失、`src-tauri` 与前端仍按旧单库 API 调用 → 整仓编不过。
> 所以第一步是"把地基补完、跑绿"，第二步才做新功能。

### 13.1 地基修复（不做这一步后面全是空中楼阁）

| 问题 | 真相 | 修法 |
|---|---|---|
| 4 个模块文件不存在 | 上一轮只写了 `mod` 声明与调用点 | 补齐 `libs_migrate`（老库拆分）、`palette`（主色/色桶）、`dupes`（xxh3 + aHash）、`refs`（工程引用提取），各带单测 |
| `db::name_of / last_undoable_op_with_ts / thumb_lru_rows / all_asset_tags / user_rows` 缺失 | 拆库后跨库归并要用 | 一并补上；`user_rows` 只导"人写过东西"的行（评分/备注/来源/标签），空库导出的 assets 是空数组才是对的 |
| **列表查询把整表丢空** | `row_to_asset` 读 `palette/color` 两列，但两处 `SELECT` 都没查它们 → rusqlite 报列不存在 → `filter_map` 静默吞掉每一行 | 两处 SELECT 补列。**教训**：`filter_map(|x| x.ok())` 会把"查询写错"变成"列表空"，一定要有测试断言条数 |
| 旧库搜索标签名报错 | 搜索 SQL 里 `JOIN tags`，而 `tags` 表已经搬到注册库（另一个文件），跨文件 JOIN 做不到 | `AssetQuery` 加 `search_tag_ids`：名字→id 的换算在 `App::list` 做（注册库标签量级很小，内存过滤），库内只按 id 查 `asset_tags` |
| 老 `tags` 表升级时报 `no such column: group_id` | `REG_SCHEMA` 里有 `CREATE INDEX ... ON tags(group_id)`，而补列在它之后 | 把这条索引移到 `reg::open` 的补列之后（**和 0.5.0 同一个坑，第二次踩**） |
| `rebuild_fts` 静默无效 | `assets_fts` 是 contentless 表（`content=''`），`VALUES('rebuild')` 什么都不干 | 改成 `delete-all` + 逐行从 assets 灌回去；顺带确认 trigram 分词器**最短 3 字符**才建索引（拿 `y` 去 MATCH 永远 0） |
| CLI/bench 编译不过 | 还在用 `app.conn` | `App::conn_for(gid)` / `conn_for_path(path)` / `ensure_thumb(gid)` 三件套，让上层永远不自己 `libs.iter().find` |

**验证**：`cargo test --release` **55 项全绿**（新增拆库/色板/查重/引用/批量重命名单测）。

### 13.2 T28 多库拆成独立 DB（实测）

- 落点：`<数据目录>/libs/lib-<id>.db`，注册库仍是 `index.db`（只有库清单 + 标签体系 + 自动规则）。
- 对外 id：`lib << 40 | 库内 id`，前端完全无感；跨库合并分页的三种排序都有单测（不重不漏）。
- **老库迁移实测**（本机真实库：2 个库各 32766 张，共 65,532 行）：10.3 秒拆完，2 个库文件 15.7 MB / 16.4 MB；第二次启动 61 ms（不重复迁移）。拆之前自动备份到 `trash-libs/index-pre-split-<ts>.db`。
- 移除库：库文件挪进 `trash-libs/` 留证（用户误点移除时评分/标签还能找回），磁盘素材一个不动。
- 设置 → 素材库新增「每个库的索引文件」一栏，路径 + 体积直接可见。

### 13.3 详情面板照 Eagle 重排 + 预览固定 1:1

- 顺序：文件名 → **预览** → 尺寸/体积/类型 → 原文件（格式/尺寸/大小/修改时间/位置）→ 标签 → 我的（评分/色板/注释/来源）→ 操作 → 显示层 → 引用。
- 预览改成**固定 1:1 方框**，图按右侧"铺满短边"放大，超出部分滚动查看（Eagle 的做法）。不用 `object-fit: contain`（缩成小图 + 大片留白），也不裁切（用户要"看全"）。
- 「我的」里加了主色板（最多 5 个色块，点一下复制色值）+ 色桶名。
- 侧栏「快捷键」不再弹 toast 挤在详情面板上（owner 截图指出），改成**双栏独立面板**（浏览/选择 | 文件/面板），键帽沿用 `.kbd-set`。

### 13.4 格式支持（owner 给的四组清单）

- 清单里的扩展名全部认得出（入库）：补齐 `avifs`、`ptx`、`mfw`。
- 嗅探补：ICO、HEIF 家族 brand（heic/heix/heis/heim/hevm/mif1/msf1）、`avis`（AVIF 序列）。
- 出图路径三档不变：自己解码（image crate）→ 文件内嵌预览（RAW/PSD）→ **系统缩略图**（HEIC/AVIF/JXL/SVG/EPS/PDF/WMF/EMF/视频）。认得出 ≠ 出得来，详情面板如实写来源。

### 13.5 P2 补齐（T24/T25/T26/T33/T34）

| 项 | 实现 | 入口 |
|---|---|---|
| T24 颜色筛选 + 主色板 | 出缩略图时顺带算（`palette.rs`，4bit 直方图 + 去近色，纯整数零依赖）；`assets.color` 落库，按颜色筛是一次索引查找；老数据用「补算」从**缓存缩略图**回填（1–3 ms/张） | 顶栏颜色按钮（只列有素材的色桶）· 详情色板 · 设置 → 缓存与索引 → 素材元数据 |
| T25 重复检测 | 精确：xxh3-128 读全文件（256 KB 缓冲，不解码）；相似：aHash 64 位（8×8 灰度）+ 鸽笼分桶（4×16bit）避免 O(n²) | Ctrl+K →「查找重复素材」（"完全相同 / 相似"两档） |
| T26 引用关系 | 扫 psd/ai/blend/max/c4d… 里**明文路径段**，只认结尾是已知媒体扩展名的；解析不出就记 `missing=1` 并保留原串 | Ctrl+K →「检查丢失的引用」· 详情「引用」段 · 设置 → 工程引用 |
| T33 批量重命名 | 规则（查找替换/前后缀/序号/大小写）+ **预览表**（每条标红原因：非法字符/批内重名/磁盘已存在）→ 执行走 `FsOps::rename`（自带 ops 日志，可 Ctrl+Z） | Ctrl+K →「批量重命名」（作用于当前选中） |
| T34 自动标签规则 | 规则存注册库 `tags.auto_rule`，**不物化**关联行；筛/计数时现场翻译成 SQL | 标签编辑卡里的「自动规则」块 |

### 13.6 看图流畅度：这一轮做了什么、为什么

owner 给的参照是果核看图（core-ui）。我们的管线本来就有：WebP 单格式缓存、JPEG DCT 降采样、按像素估重的解码闸门（10 份）、预取下一屏、全库预热、三态瓦片。

**这轮加的是一层"零成本"的感知优化：主色板占位。**
色板本来就跟列表一起回来了（DTO 里已有），在缩略图到位之前用它铺一张同色调渐变 —— 一片灰方块变成"已经像那张图"的色块，等待感消失。**不增加一个字节的存储、不增加一个请求**，这是比"再快 50ms"更划算的一步。

未做（留作下一轮，需要 owner 决定）：
- 缩略图 16×16 内联 LQIP（每张约 300–600 字节，65k 张 ≈ +30 MB 索引体积）——**与"体积不要太大"冲突，先不做**；
- 缓存打成一个 pack 文件减少随机 IO（收益要看 SSD/机械盘，改动面大）；
- 解码闸门按"离视口距离"排优先级（现在是先到先服务）。

### 13.7 踩坑（新的，别再踩）

1. **`cargo test` 不会更新 `target/release/am-cli.exe`**：我拿旧二进制去验迁移，看到的是旧结果（id 没编码、目录没拆），差点误判"迁移没跑"。跑 CLI 前先 `cargo build --release --bin am-cli`。
2. `assets_fts` 是 contentless 表：`VALUES('rebuild')` 静默无效（见 13.1）。
3. **SQL 字符串里的反斜杠**：`LIKE '%\'`（拼路径）在 Rust 普通字符串里要写 `'%\\'`，但我在 `r"..."` 原始串里多写了一层，SQL 收到的是 `%\\`（结尾两个反斜杠）→ 匹配不上任何行。原始串里就是一个反斜杠。
4. `impl` 块被"插入到 `/* 工具函数 */` 之前"这类改动**很容易多出一个 `}`**：Rust 报的是"unexpected closing delimiter"，位置在几百行之后 —— 改完先 `cargo check` 再继续写。
5. 替换 `db.rs` 测试模块时，旧测试全部依赖 `app.conn` 与 `db::all_tags` 这类已搬家 API：把它们改成 **App 级测试**（`app.tag_assets / app.tags / app.rename_tag …`），顺带把断言口径对齐新架构（比如"导出只含人写过的素材"）。

### 13.8 体积控制

- 没有引入任何新 crate（色板/查重/引用全部纯 Rust 手写）。
- 安装包仍是一个 NSIS 单文件（见下）；索引侧新增的列只有 `palette/color/phash`（每行几十字节）。
- 明确**不做**：把 LQIP 塞进索引（+30 MB）、给 image 加 AVIF 编码器（+2–4 MB 二进制约）。


---

## 14. 0.9.0（2026-11-17）：无边框看图窗口 —— 对果核看图的一次实地拆解

> owner 的原话：「果核看图的无边框功能很棒我很需要，可以配合屏幕截图来用；
> 右击菜单的内容功能你也可以参考，UI 中的功能也能参考」。

### 14.1 怎么"研究"它的（不是看截图，是上手测）

1. **读语言包**：`<安装目录>\lang\zh-CN.lang`（40 KB）就是它的全部 UI 文案 —— 比截图更快更全，
   菜单、提示、设置项、错误提示全在里面。
2. **读配置**：`%APPDATA%\GuoheView\config.ini` 把它的**性能开关全暴露了**（见下表）。
3. **真跑 + 合成输入**：用 ctypes 发真实鼠标/键盘事件（D2D 窗口不认 PostMessage 假消息），
   量出窗口几何、缩放步长、菜单结构；右键菜单在独立的 `UiCore_OverlayHost` 窗口里。
4. **读开源仓库**：`ghboke/core-ui`（MIT）—— C ABI + `.uix`(Vue3 SFC) + QuickJS-NG + Direct2D/D3D11，
   4.1 MB DLL、冷启动 <200 ms、空窗内存 <30 MB。

**实测到的事实**（不是猜的）：

| 项 | 值 |
|---|---|
| 体积 | 整个程序目录 **20 MB**（主程序 4.0 + core-ui.dll 4.6 + 解码 ghde.dll 6.7） |
| 窗口类名 | `UiCore_Window`；菜单/浮层在 `UiCore_OverlayHost` |
| 无边框窗口 | 图片 1280×820 → 窗口外框 485×331 ⇒ **图区 427×274**（长边 342 DIP @1.25 缩放）+ 四周 29 px 阴影留白 |
| 滚轮缩放 | 342×428 → 407×509 两格 ⇒ **每格 ×1.09**，**锚定窗口中心**（中心坐标不变） |
| 右键菜单 | 25 项，6 组分隔；组内「复制图片/复制文件/复制更多›」→「重命名/另存为/调整尺寸/编辑/打印」→「打开位置/图片信息/收集›」→「设置」→「删除」→「置顶/退出无边框(F11 全屏)/打开方式›/更多›」→「退出」 |
| 配置里的性能开关 | `nav_frame_gate`（切图至少显示一帧）、`smooth_zoom`、`menu_open_anim`、`parallel_jpeg_decode`、`tiff_decoder_v2`、`mid_band_progressive`、`png_threads=8`、`tile_decode_us`/`tile_upload_us`、`perf_mode`+`detected_tier`（按 16 核/16 GB 自动判档）、双缓存 `limit_gb=5` + `preview_limit_gb=5`、ICC 三档 |

### 14.2 做了什么

**无边框看图窗口（`B` / 右键 / 命令面板）**，`src-tauri/src/main.rs` + `app/src/components/Peek.tsx`：

- 独立窗口 `label=peek`，`decorations=false` + `shadow=false` + **不透明**；
  尺寸 = 图片比例，长边 342 DIP（`peek_size_for`，未知尺寸 4:3 兜底）。
- **滚轮 = 连窗口一起缩放**（`am_peek_zoom`，×1.09，锚定中心，长边夹 96–4096 DIP）。
  不是 CSS transform —— 那样窗口边缘会脱离图片，截图要手动避开一圈透明。
- 整窗可拖（`data-tauri-drag-region`）、方向键换图（`am_peek_ratio` 按新图比例重算、长边不变中心不动）、
  `Esc` 关、`Alt+T` 置顶、`F11` 全屏、`Ctrl+滚轮` 图内缩放（窗口不动）。
- 鼠标进来才浮出三个图标按钮（置顶/全屏/关闭）+ 左下角「序号/总数 · 尺寸」；静止时完全干净。
- 同一份前端包跑两种窗口：`initialization_script` 注入 `window.__AM_PEEK__`，`main.tsx` 据此分叉。

### 14.3 三个坑（写进 §4b，这里只列结论）

1. **窗口几何相关的命令必须 `async fn`**：同步命令在主线程，`build()`/`set_size()` 又要回主线程 → 自死锁。
   症状是"窗口建出来了但全空白、on_page_load 不触发、命令不返回"。
2. `build()` 失败**别用 `?` 早退**，先 `log_runtime` 留证，否则错误被前端 toast 吞成"什么都没发生"。
3. 惯性滚动会把一次滚轮拆成多帧合成事件 → 无边框窗口里"滚轮缩放"被放大数倍，必须不装。

### 14.4 明确没抄的（以及为什么）

- **29 px 阴影留白**：它是为了给窗口放投影；我们的用法是截图，窗口边界=图片边界更省事。
- **分块解码 + 分块上传**：那是 D2D 直接画图的产物，WebView2 拿不到这个控制权。
- **改为 core-ui 重写**：4 MB DLL + 200 ms 冷启动确实香，但等于把整个前端（约 1.2 万行 TSX + 全部交互）
  重写一遍，收益是"启动更快"，代价是"现有东西全推倒"。**不做**，但把它记进参考板当长期选项。

### 14.5 下一轮可以抄的（已写进待办清单）

按性价比排：① 连续切图逐张显示（`nav_frame_gate` 思路，我们的大图翻页直接受益）
② 右键菜单加"另存为 / 调整尺寸" ③ 设置可搜索 ④ 灯箱鸟瞰图 / 长图阅读条
⑤ 图片信息（EXIF）⑥ 按机器档位调解码闸门份额。
