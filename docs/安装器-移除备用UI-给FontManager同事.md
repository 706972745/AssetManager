# 安装器 · 移除「备用安装/卸载 UI」执行手册

> 给维护 FontManager 的同事。照本文从 §5 开始做即可，不需要先读别的文档。
> **一句话原则：只删「NSIS 自己画出来的控件」，不删「决定装到哪、删什么」的逻辑。**

| 项 | 值 |
|---|---|
| 目标工程 | `E:\Users\87557\文档\wordbuddy\软件迭代\FontManager\Fonts工程文件\ZFontManager` |
| 当前版本 | 以 `src-tauri/tauri.conf.json` 的 `version` 为准（本文写作时 2.5.5） |
| 本文命令 cwd | ZFontManager 根目录（另有注明的除外） |
| 允许改 | `src-tauri/windows/installer.nsi`、`src-tauri/tauri.conf.json`；删 `src-tauri/windows/{zfm-strings.nsh,sidebar.bmp,header.bmp,gen-nsis-images.py}` |
| 一行都不许改 | `src-tauri/src/**`、仓库根 `setup.html`、`src/setup/**`、`src-tauri/windows/hooks.nsh`、`tools/**` |
| 行号口径 | 文中行号取自 HEAD `6cff6b3`（2026-11-22 读取）。改完不要靠行号核对，用 §6.1 的 grep |

---

## 0. 一页结论

1. **路线 C 是正式界面，本次一行不动**：`src-tauri/src/setup_ui.rs`、`setup.html`、`src/setup/main.ts`、`src/setup/setup.css`；入口参数 `--installer-ui` / `--uninstall-ui`；开关 `!define ZFM_ROUTE_C`（`installer.nsi:132`，2.1.7 起默认开启）。
2. **要删的是 NSIS 侧自己画的界面**：自绘欢迎页 `ZfmWelcome`、自绘升级页 `PageReinstall` 一族、以及只服务这些页面的文案/图片/生成脚本。
3. **删完之后，路线 C 起不来时的回退面 = 原生 MUI 页**（Welcome / Directory / InstFiles / Finish，卸载 Confirm / InstFiles）。这是本次改动的**预期结果**，不是事故：备用 UI 不再维护。
4. **三样东西看着像 UI，实际是行为，必须留**：
   - `ZfmDirPre` / `ZfmDirLeave`（升级锁目录、`/D=` 优先、写探针）→ B2 / B3 / B4；
   - `un.ConfirmShow` / `un.ConfirmLeave`（卸载「同时删除全部数据与字体库」复选框）→ B17，且 `tools/nsis-fork-audit.js` 的红线点名了它；
   - 完成页的 `MUI_FINISHPAGE_SHOWREADME*` / `MUI_FINISHPAGE_RUN*` 结构（桌面快捷方式 / 装完即运行）→ B15。
5. **一个执行前必须确认的取舍**：删掉 `sidebar.bmp` 后，回退面的欢迎页/完成页会显示 **NSIS 自带的蓝色 `win.bmp`**。不能接受就保留 `sidebar.bmp` + `sidebarImage`（它只是页背景图，不是页面）——见 §7.1。

---

## 1. 背景与结论

FontManager 从 2.1.7 起走**路线 C**：NSIS 包只当引擎（解包、写注册表、建快捷方式、跑 `hooks.nsh` 白名单），安装/卸载界面由主程序 exe 的 `--installer-ui` / `--uninstall-ui` 入口画（`setup_ui.rs` + `setup.html`）。设计依据见 `docs/PRD-2.1.6-安装器路线B.md` §9.7。

但 `installer.nsi` 从 2.1.5 起还保留着一整套「看得见的」NSIS 定制：自绘欢迎页、自绘升级选择页、目录页文案、完成页文案、卸载确认页复选框，以及配套的 `zfm-strings.nsh`、`sidebar.bmp`、`header.bmp`、`gen-nsis-images.py`。这套东西现在的实际处境是：

- 正常安装/卸载**永远看不到**（`installer.nsi:932-955` / `:1249-1275` 里 `.onInit` 先 `ExecWait` 主程序 UI，成功就 `Quit`）；
- 只有 WebView2 缺失、UI 被杀软拦、用户取消等异常才回落；
- 因此它是**第二套要同步维护的界面**：改一句文案、调一次视觉，两处都要改，且没人会记得验。

**结论：删掉备用界面，保留 NSIS 引擎能力。** 回退面退化成原生 MUI 页，难看但能装/能卸——这比「两套 UI 长期漂移」可控。2.1.5 的行为基线 B1–B20（安装目录、升级锁、静默语义、注册表、白名单、标记文件、WebView2）**一条都不动**，逐条对照见 §7.2。

---

## 2. 现状清单（全量盘点）

处置列：**删** = 本次删除；**换** = 用原生 MUI 页替换；**留** = 不能删（原因见 §4）。

| # | 位置（HEAD `6cff6b3`） | 内容 | 处置 |
|---|---|---|---|
| 1 | `installer.nsi:269-359`（`Page custom` 在 285） | 自绘欢迎页 `Function ZfmWelcome`：6 个 `NSD_CreateLabel`、2 个 `CreateFont`、整页白底与版面注释 | 换 |
| 2 | `installer.nsi:373-649`（`Page custom` 在 376） | 自绘升级页：`PageReinstall` / `PageReinstallUpdateSelection` / `PageLeaveReinstall`，含 WiX 探测与 R-18 单选映射 | 删 |
| 3 | `installer.nsi:375` | `Var ReinstallPageCheck`（只被上面三个函数用） | 删 |
| 4 | `installer.nsi:655-656` | `MUI_DIRECTORYPAGE_TEXT_TOP` / `MUI_DIRECTORYPAGE_TEXT_DESTINATION` 文案覆盖 | 删 |
| 5 | `installer.nsi:657-658`、`661-705` | `ZfmDirPre`（`/P` 跳过、`/D=` 优先、升级锁目录）/ `ZfmDirLeave`（写探针） | **留** |
| 6 | `installer.nsi:734-736` | 完成页文案：`MUI_FINISHPAGE_TITLE_3LINES` / `_TITLE` / `_TEXT` | 删 |
| 7 | `installer.nsi:738-745` | 完成页结构：`SHOWREADME`（桌面快捷方式）+ `RUN`（装完运行） | **留** |
| 8 | `installer.nsi:761-800` | 卸载确认页：`MUI_UNCONFIRMPAGE_TEXT_TOP`、`un.ConfirmShow`、`un.ConfirmLeave`、两个 Var、`WS_EX_LAYOUTRTL` | **留** |
| 9 | `installer.nsi:217-218` | `MUI_BGCOLOR` / `MUI_TEXTCOLOR`（仅影响原生页配色） | 留（可选清理） |
| 10 | `installer.nsi:220-237` | `sidebarImage` 注释 + `MUI_WELCOMEFINISHPAGE_BITMAP` | 删 |
| 11 | `installer.nsi:239-254` | `HEADERIMAGE` / `UNINSTALLERHEADERIMAGE` 块（config 没配，渲染为空，恒不生效） | 留（上游结构） |
| 12 | `installer.nsi:814-840` | 从 `sidebarImage` 推源码目录（`!searchreplace ZFM_NSIS_DIR`）+ `!include "zfm-strings.nsh"` | 删 |
| 13 | `zfm-strings.nsh:32-64`（中文）、`67-95`（英文） | 全部 `ZM_*` + `MUI_TEXT_*` 覆盖 + 4 条升级页文案 | 删（5 条迁走） |
| 14 | `src-tauri/windows/sidebar.bmp` | 164×314，完成页/欢迎页左栏背景 | 删 |
| 15 | `src-tauri/windows/header.bmp` | 150×57，config 从未引用（死文件） | 删 |
| 16 | `src-tauri/windows/gen-nsis-images.py` | 生成上面两张 BMP 的脚本（构建时不跑） | 删 |
| 17 | `src-tauri/tauri.conf.json:71` | `"sidebarImage": "./windows/sidebar.bmp"` | 删 |
| 18 | `installer.nsi:132`、`857`、`932`、`1238`、`1249` | `!define ZFM_ROUTE_C` + 4 个 `!ifdef` 块 | **留** |
| 19 | `installer.nsi:842-956`、`1215-1276` | `.onInit` / `un.onInit`（`/P` `/NS` `/DS` `/UPDATE` `/PURGE`、目录默认值、升级锁、降级、两个 `ExecWait`） | **留** |
| 20 | `installer.nsi:958-1483` | `Section WebView2` / `Section Install` / `Section Uninstall` / 快捷方式函数 | **留** |

### 2.1 用 grep 自己核一遍

```bash
# 页面与自绘函数
grep -n "Page custom\|^Function Zfm\|^Function PageReinstall\|^Function PageLeaveReinstall" \
  src-tauri/windows/installer.nsi

# 只被自绘页使用的文案 / 变量
grep -n "ZM_WELCOME\|ZM_FOOT\|ZM_DIR_TOP\|ZM_DIR_DEST\|ZM_FINISH_TITLE\|ZM_FINISH_TEXT\|ReinstallPageCheck" \
  src-tauri/windows/installer.nsi src-tauri/windows/zfm-strings.nsh

# 资源与配置
grep -n "sidebarImage\|headerImage\|uninstallerHeaderImage" src-tauri/tauri.conf.json
grep -rn "sidebar.bmp\|header.bmp\|gen-nsis-images" \
  src-tauri/windows src-tauri/tauri.conf.json

# 路线 C 的五处（1 个 define + 4 个 ifdef）与两个 ExecWait
grep -n "!define ZFM_ROUTE_C" src-tauri/windows/installer.nsi
grep -c "!ifdef ZFM_ROUTE_C" src-tauri/windows/installer.nsi    # 期望 4
grep -n "ExecWait.*--installer-ui\|ExecWait.*--uninstall-ui" src-tauri/windows/installer.nsi
```

---

## 3. 必须删除清单

> 顺序按 §5 执行：**先补原生页/内联文案（让文件一直可编译），再删页面宏，最后删函数**。
> 反过来做（先删函数、后删 `Page custom`）会得到 `Function not found` 编译错误。

### 3.1 自绘欢迎页（替换为原生欢迎页）

- 位置：`installer.nsi:269-359`，`Page custom ZfmWelcome` 在 285 行。
- 动作：整块删除（含 `Function ZfmWelcome` 的 6 个 Label、2 个 `CreateFont`、白底与版面注释），替换为 §5.2 的三行原生页。
- 删完判定：`grep -n "ZfmWelcome\|ZM_WELCOME" src-tauri/windows/installer.nsi` 无输出。

### 3.2 自绘升级页

- 位置：`installer.nsi:373-649`。`Var ReinstallPageCheck`（375）、`Page custom PageReinstall PageLeaveReinstall`（376）、`Function PageReinstall`（377-523）、`PageReinstallUpdateSelection`（524-531）、`PageLeaveReinstall`（532-649）。
- 动作：整段删除。`R-18` 的单选映射只在这个页面里有意义，页没了，注释一起删。
- 删完判定：`grep -n "PageReinstall\|ReinstallPageCheck" src-tauri/windows/installer.nsi` 无输出；`grep -n "WixMode" src-tauri/windows/installer.nsi` 只应剩 `CreateOrUpdate*Shortcut` 里的 `$WixMode = 0`（快捷方式迁移判断，保留）。
- 行为后果（只影响回退面）：升级不再提供「先卸载再装」，一律直接覆盖（这本来就是 PRD 的推荐默认）；更早的 WiX 安装的自动卸载也一起消失。正常路线 C 不受影响。

### 3.3 目录页文案覆盖

- 位置：`installer.nsi:655-656` 两行 `MUI_DIRECTORYPAGE_TEXT_TOP` / `MUI_DIRECTORYPAGE_TEXT_DESTINATION`。
- 动作：只删这两行。**`MUI_PAGE_CUSTOMFUNCTION_PRE/LEAVE` 与 `ZfmDirPre`/`ZfmDirLeave` 全部保留**（见 §4.1）。
- 删完判定：页面回到 MUI 原生标题/正文，但仍会跳过被动模式、锁升级目录、写探针。

### 3.4 完成页文案覆盖

- 位置：`installer.nsi:734-736`（`TITLE_3LINES` / `TITLE` / `TEXT`）。
- 动作：只删这三行。`MUI_FINISHPAGE_NOAUTOCLOSE`、`SHOWREADME`、`RUN` 三组结构保留。
- 后果：回退面完成页显示 MUI 原生「安装完成」文案，不再显示「数据保存在安装目录的 data 文件夹」这句披露。介意就保留 735-736 两行（它们只是文案，不是自绘控件），并在 §7 记一笔。

### 3.5 文案文件与目录推导

- 位置：`installer.nsi:814-840` 整段（含 `ZFM_NSIS_DIR` 的 `!searchreplace` 和 `!include "zfm-strings.nsh"`）。
- 动作：整段删除，替换为 §5.1 的 10 行内联 `LangString`（5 条 × 中英）。
- 为什么不能只删 `!include`：`!searchreplace ZFM_NSIS_DIR "${SIDEBARIMAGE}" ...` 依赖 `sidebarImage`；`sidebarImage` 一删，`!searchreplace` 找不到 `\sidebar.bmp` → 命中 `!error`（`installer.nsi:837`）→ **编译期直接失败**。这条依赖必须整段拆掉。
- 删完判定：`grep -n "zfm-strings\|ZFM_NSIS_DIR" src-tauri/windows/installer.nsi` 无输出。

### 3.6 图片、生成脚本与配置

- 删文件：`src-tauri/windows/sidebar.bmp`、`src-tauri/windows/header.bmp`、`src-tauri/windows/gen-nsis-images.py`。
- 删配置：`src-tauri/tauri.conf.json` 的 `"sidebarImage": "./windows/sidebar.bmp",` 一行。
- 删代码：`installer.nsi:220-237` 整段注释 + `!if "${SIDEBARIMAGE}" != ""` 块。
- 删完判定：`grep -rn "sidebar.bmp" src-tauri/windows src-tauri/tauri.conf.json` 无输出。
- 后果：`{{sidebar_image}}` 渲染为空 → `MUI_WELCOMEFINISHPAGE_BITMAP` 不定义 → 回退面欢迎/完成页使用 NSIS 默认蓝色 `win.bmp`。见 §7.1。

### 3.7 顺带清理（零行为，可选）

- `installer.nsi:65-67`：`!include "StrFunc.nsh"` + `${StrCase}` + `${StrLoc}` 只被 `PageReinstall` 的 WiX 探测用；删页后可一起去掉。
- `installer.nsi:206-218` 的 `MUI_BGCOLOR` / `MUI_TEXTCOLOR`：只影响原生页配色，删了就是 MUI 默认黑白。
- 文件头注释（`installer.nsi:1-34`）里提到 `zfm-strings.nsh`、`sidebar.bmp` 的句子要跟着改，否则下一个人按注释去找已删除的文件。

---

## 4. 必须保留清单（删错就坏）

| # | 必须保留 | 位置 | 删错的后果 |
|---|---|---|---|
| K1 | `!define ZFM_ROUTE_C`（1 行，行首，不要注释） | `installer.nsi:132` | 路线 C 全部不参与编译，界面退回 2.1.6 老路径；`node tools/route-c.mjs status` 会 exit 2 |
| K2 | 4 个 `!ifdef ZFM_ROUTE_C` 块 | `:857`（`/DS`）、`:932`（安装 UI）、`:1238`（`/PURGE`）、`:1249`（卸载 UI） | 主程序 UI 不再被拉起 / `/DS` `/PURGE` 失效 |
| K3 | `ExecWait ... --installer-ui` | `:941` | 安装界面消失，直接掉进原生页 |
| K4 | `ExecWait ... --uninstall-ui` | `:1257` | 卸载界面消失；且 `$TEMP\FontManager-setup-ui` 的复制/清理逻辑要一起留 |
| K5 | `.onInit` 的 `/P` `/NS` `/DS` `/UPDATE`、`SetContext`、`$INSTDIR` 默认值、`RestorePreviousInstallLocation`、降级 `Abort` | `:842-956` | B1/B3/B5-B8 直接破 |
| K6 | `un.onInit` 的 `/P` `/UPDATE` `/PURGE` 与临时目录复制 | `:1215-1276` | 静默卸载删数据入口消失；卸载时删不掉正在运行的 exe |
| K7 | `Section WebView2` | `:958-1089` | B20 破；两处 `SetErrorLevel 1` 是 `/S` 的唯一失败语义 |
| K8 | `Section Install`（`UNINSTKEY` 注册表 13 值、`EstimatedSize`、快捷方式、`WriteUninstaller`、`NSIS_HOOK_PRE/POSTINSTALL`） | `:1091-1194` | B11/B15/B16/B19 破 |
| K9 | `Section Uninstall`（白名单由 hooks 执行、`NSIS_HOOK_PRE/POSTUNINSTALL`） | `:1278-1397` | B18 破；误删用户字体 |
| K10 | `hooks.nsh` 全文（233 行，白名单 + `data\.zfm-data-root` 追加） | `src-tauri/windows/hooks.nsh` | 最高风险代码，一个字都不动 |
| K11 | 全部 handlebars（`{{...}}`、`{{#each}}`、`{{#if}}`） | 全文 | 加资源/文件关联/深链时**静默丢失**，构建不报错 |
| K12 | `/S` `/P` `/UPDATE` 语义 | `.onInit` / `un.onInit` / Sections | 一键更新链断裂（`update.rs` 只认 `ExitCode -eq 0`） |
| K13 | `ZfmDirPre` / `ZfmDirLeave` + 两个 `MUI_PAGE_CUSTOMFUNCTION_*` | `:657-658`、`:661-705` | B3 升级锁目录（P0：装出两套）、B4 写探针（半装空壳） |
| K14 | `un.ConfirmShow` / `un.ConfirmLeave` / 两个 Var / `WS_EX_LAYOUTRTL` | `:761-800` | B17 破（GUI 卸载无法删数据）；`nsis-fork-audit.js` 红线 exit 1 |
| K15 | `MUI_FINISHPAGE_SHOWREADME*` / `MUI_FINISHPAGE_RUN*` | `:738-745` | 桌面快捷方式选项消失（B15）；复选框标签变回「Show Readme」 |
| K16 | `MUI_PAGE_STARTMENU` 整段（含 `Skip` 分支） | `:707-722` | 将来填 `startMenuFolder` 时静默失去「开始菜单文件夹名」选项 |
| K17 | 页面顺序：Welcome → Directory → StartMenu → InstFiles → Finish；卸载 Confirm → InstFiles | 全文 | 顺序错了 NSIS 不报错，流程会怪 |

### 4.1 为什么 `ZfmDirPre` / `ZfmDirLeave` 不能当 UI 删

它们**一个控件都不画**，只是 MUI 目录页的 PRE / LEAVE 回调：

- `/P` 跳过本页（等价于上游 `SkipIfPassive`）；
- 显式 `/D=` 优先，不覆盖（企业静默部署的唯一出口）；
- 注册表已有安装目录 → `StrCpy $INSTDIR` 并 `Abort` 本页（**B3，P0**：不锁就会在升级时装出第二套，字体库和账本留在旧目录，用户看到「升级后数据全没了」）；
- LEAVE 里建目录 + 写 `.zfm-probe` + 失败 `RMDir` 空目录 + 停在当前页（**B4**：不拦就得到「目录建了、文件没写、注册表没写全」的半装）。

删掉它们，回退面的升级锁和写探针一起消失。**保留函数、删掉文案覆盖**，目录页就是「原生长相 + 原行为」。

### 4.2 为什么 `un.ConfirmShow` 不能删

它是卸载确认**原生页**的 SHOW 回调，只做一件事：用 `CreateWindowEx` 加一个「同时删除全部数据与字体库」复选框（默认不勾）。三条理由：

1. **B17 的唯一 GUI 实现**：路线 C 卸载界面自己传 `/PURGE=1`，但这个复选框是**回退面**（WebView2 不可用、UI 起不来）唯一能删数据的入口。删了 = 回退面永远只能保留数据。
2. **仓库自己的审计红线**：`tools/nsis-fork-audit.js:90` 的 `REDLINE` 里明确有 `/un\.ConfirmShow/`，删掉它审计直接 `exit 1`。
3. 它只有三十来行 Win32 调用，不是「第二套界面」，维护成本接近零。

如果产品明确要求「回退面也不许删数据」，那是另一个需求（等于删功能，不是删 UI），要单独评审，不要夹在本次改动里。

---

## 5. 具体改法（严格按顺序）

> 全程在 ZFontManager 根目录。每步做完都能编译，出问题只回退这一步。

### 5.0 打点

```bash
git switch -c chore/nsis-drop-backup-ui
git rev-parse HEAD          # 记下这个 SHA，回滚要用（§7.3）
node tools/route-c.mjs status   # 期望：on
```

### 5.1 先内联还需要的 5 条文案（此时还没删任何东西）

把 `installer.nsi:814-840` 整段替换为：

```nsis
; ────── 仅存的行为性文案（原 zfm-strings.nsh，内联）──────
; 位置不能动：LangString 只能写进 MUI_LANGUAGE 已加载的语言。
LangString deleteAppData     ${LANG_SIMPCHINESE} "同时删除全部数据与字体库"
LangString ZM_UN_TEXT        ${LANG_SIMPCHINESE} "将从这台电脑移除 FontManager。"
LangString ZM_DIR_NOWRITE    ${LANG_SIMPCHINESE} "这个位置当前用户没有写入权限。$\r$\n请换一个位置（例如“文档”下的文件夹），或者以管理员身份重新运行安装程序。"
LangString ZM_FINISH_RUN     ${LANG_SIMPCHINESE} "立即运行 FontManager"
LangString ZM_FINISH_DESKTOP ${LANG_SIMPCHINESE} "在桌面创建快捷方式"

LangString deleteAppData     ${LANG_ENGLISH} "Also delete all data and the font library"
LangString ZM_UN_TEXT        ${LANG_ENGLISH} "This will remove FontManager from this computer."
LangString ZM_DIR_NOWRITE    ${LANG_ENGLISH} "You don't have write access to this folder.$\r$\nPick another location (for example one under Documents), or run the installer as administrator."
LangString ZM_FINISH_RUN     ${LANG_ENGLISH} "Launch FontManager now"
LangString ZM_FINISH_DESKTOP ${LANG_ENGLISH} "Create a desktop shortcut"
```

保存要求：**UTF-8**。`installer.nsi` 源文件现在没有 BOM，不要在编辑器里另存成 GBK/ANSI；tauri 渲染时会补 BOM（`installer.nsi:828-831` 有实测记录）。

### 5.2 替换欢迎页

把 `installer.nsi:269-359` 整段换成：

```nsis
; 1. 欢迎页（原生 MUI；路线 C 下 .onInit 已 Quit，不显示）
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_WELCOME
```

如果决定连欢迎页也不要（回退面直接从目录页开始），整段删掉即可，同样能编译。

### 5.3 删升级页

删 `installer.nsi:373-649`（`Var ReinstallPageCheck` 到 `Function PageLeaveReinstall` 的 `FunctionEnd`）。
注意：`Page custom` 行先删、函数后删，避免 `Function not found`。

### 5.4 删目录页/完成页文案

- 删 `:655-656` 两行 `MUI_DIRECTORYPAGE_TEXT_*`；
- 删 `:734-736` 三行 `MUI_FINISHPAGE_TITLE_3LINES` / `_TITLE` / `_TEXT`；
- 两处的 `MUI_PAGE_CUSTOMFUNCTION_*`、`SHOWREADME`、`RUN`、`NOAUTOCLOSE` 全部保留。

### 5.5 删 sidebar 块与配置

- 删 `installer.nsi:220-237`；
- 删 `tauri.conf.json` 的 `"sidebarImage": "./windows/sidebar.bmp",`；
- 删三个文件：`sidebar.bmp`、`header.bmp`、`gen-nsis-images.py`。

### 5.6 删文案文件

`rm src-tauri/windows/zfm-strings.nsh`（此时它的 5 条已内联，`!include` 已随 5.1 删除）。

### 5.7 改文件头注释

`installer.nsi:1-34` 里提到 `zfm-strings.nsh`、`sidebar.bmp` 的句子改成「文案已内联 / 不再使用侧栏图」，避免误导。

### 5.8 静态自检（不打包）

```bash
git diff --stat
grep -n "Page custom" src-tauri/windows/installer.nsi     # 期望无输出
grep -n "zfm-strings\|ZFM_NSIS_DIR\|sidebar.bmp" src-tauri/windows/installer.nsi   # 期望无输出
grep -c "!ifdef ZFM_ROUTE_C" src-tauri/windows/installer.nsi   # 期望 4
node tools/nsis-fork-audit.js _tmp/pristine.nsi src-tauri/windows/installer.nsi
```

审计脚本的判读：

- `[溯源] 未命中=0` —— 基线仍可信；
- `[审计]` 会打印**几百行**被删/新增，这是正常的（本次就是大改 UI）；
- **只认 `[红线]`**：必须是 `0 处`。如果 removed 列表里出现 `Section Install` / `Function .onInit` / `un.ConfirmShow` / `{{#each}}`，说明删过头了。

`_tmp/pristine.nsi` 不在时（`_tmp` 会被清）：

```bash
mkdir -p _tmp
git show 0b4f117:src-tauri/windows/installer.nsi > _tmp/pristine.nsi
```

---

## 6. 验证

### 6.1 静态断言（全部在仓库根跑）

```bash
# 1) 备用页函数名 0 命中
grep -n "ZfmWelcome\|PageReinstall\|PageLeaveReinstall\|ReinstallPageCheck" \
  src-tauri/windows/installer.nsi          # 期望：无输出

# 2) 文案文件与目录推导已消失
grep -rn "zfm-strings\|ZFM_NSIS_DIR" src-tauri/windows src-tauri/tauri.conf.json
                                           # 期望：无输出

# 3) 图片引用已消失（文件本身也应不存在）
grep -rn "sidebar.bmp\|header.bmp" src-tauri/windows src-tauri/tauri.conf.json
ls src-tauri/windows/sidebar.bmp src-tauri/windows/header.bmp 2>&1   # 期望：No such file

# 4) 路线 C 标记仍在，且只有一行 define
grep -n "^!define ZFM_ROUTE_C" src-tauri/windows/installer.nsi        # 1 行
grep -c "!ifdef ZFM_ROUTE_C" src-tauri/windows/installer.nsi          # 4
grep -n "ExecWait.*--installer-ui"  src-tauri/windows/installer.nsi   # 1
grep -n "ExecWait.*--uninstall-ui"  src-tauri/windows/installer.nsi   # 1

# 5) 受保护区零改动（对 git diff 断言，比行号可靠）
git diff -U0 -- src-tauri/windows/installer.nsi | grep -E "^[+-].*(Section (Install|Uninstall|WebView2)|Function \.onInit|un\.ConfirmShow|un\.ConfirmLeave|\{\{|NSIS_HOOK_)" 
                                           # 期望：无输出

# 6) 完成页结构、目录页钩子还在
grep -n "MUI_FINISHPAGE_SHOWREADME_FUNCTION\|MUI_FINISHPAGE_RUN_FUNCTION" src-tauri/windows/installer.nsi   # 各 1
grep -n "ZfmDirPre\|ZfmDirLeave" src-tauri/windows/installer.nsi   # 期望 4 处（2 个 define + 2 个 Function）

# 7) route C 开关仍是发版状态
node tools/route-c.mjs status             # 期望：on

# 8) 白名单与路线 C 源码零改动（最强的两条“没碰错东西”断言）
git diff --stat -- src-tauri/windows/hooks.nsh        # 期望：无输出
git diff --stat -- src-tauri/src src/setup setup.html # 期望：无输出
```

### 6.2 审计与静默护栏

```bash
node tools/nsis-fork-audit.js _tmp/pristine.nsi src-tauri/windows/installer.nsi
# 期望：exit 0；[红线] 命中受保护区段 0 处

node tools/check-silent-paths.mjs
# 期望：结果：FAIL 0 · PASS（不要加 --strict；ZfmDirLeave 的 MessageBox 会留一条 GUI-only WARN）
```

### 6.3 构建

```bash
source "E:/Users/87557/文档/wordbuddy/软件迭代/FontManager/Fonts工程文件/env.sh"

# 正式包（会先跑 pnpm build 更新 dist/）
pnpm tauri build --bundles nsis
```

只改 NSIS 想省一次前端构建时，可以用：

```bash
pnpm tauri build --config _tauri.nobuild.json --bundles nsis
```

产物名 `FontManager_<ver>_x64-setup.exe`（`<ver>` 取 `tauri.conf.json`）。产物目录以构建日志的 `Bundling` 行为准（本仓库历史产物在 `rust-target/release/bundle/nsis/`，未设 target 时在 `src-tauri/target/release/bundle/nsis/`）。
**构建失败时先看是不是 `Function not found`**——那就是 §5 的删除顺序反了。

### 6.4 GUI 安装（路线 C 四屏）

双击 `FontManager_<ver>_x64-setup.exe`：

- 应出现主程序自绘的 4 屏：欢迎 → 位置 → 安装中 → 完成（1000×620、无系统标题栏）；
- **不应**出现 NSIS 灰白向导页；
- 第 1 屏点取消：进程退出码应为 **10**（路线 C 约定：10 = 用户取消，不是失败）；
- 第 2 屏改路径 → 安装 → 装完检查 `HKCU\Software\IIEOzo\FontManager` 默认值 = 所选目录。

### 6.5 `/S` 静默安装

```powershell
$exe = ".\FontManager_2.5.5_x64-setup.exe"   # 换成实际产物名
$p = Start-Process -FilePath $exe `
  -ArgumentList "/S","/NS","/D=$env:TEMP\zfm-probe" -Wait -PassThru
$p.ExitCode                                  # 期望 0
Test-Path "$env:TEMP\zfm-probe\fontmanager.exe"   # 期望 True
```

注意：`/D=` 必须放**最后**且不要加引号（NSIS 习惯）。装完检查 `uninstall.exe` 存在、注册表 `InstallLocation` 带引号。

### 6.6 卸载（路线 C 三屏）

```powershell
& "$env:LOCALAPPDATA\FontManager\uninstall.exe"
```

- 应出现确认 → 卸载中 → 完成三屏；
- 确认屏复选框默认**不勾**；
- 完成屏关窗后退出码 0；点取消退出码 10；
- 再跑一次 `/S`：

```powershell
$u = Start-Process "$env:LOCALAPPDATA\FontManager\uninstall.exe" `
  -ArgumentList "/S" -Wait -PassThru
$u.ExitCode        # 期望 0
```

勾选「同时删除全部数据与字体库」后再卸一次，确认 `<install>\data`、`%APPDATA%\FontManager` 消失，而安装目录根下用户手放的文件仍在（B17/B18 的口径不变）。

### 6.7 回退面必须真跑一次（本次改的就是它）

```bash
node tools/route-c.mjs off
pnpm tauri build --config _tauri.nobuild.json --bundles nsis
# 跑安装器：应是原生 MUI 页 Welcome → Directory → Installing → Finish
# 检查三件事：
#   ① 完成页复选框文案 =「在桌面创建快捷方式」（不是 Show Readme）
#   ② 卸载确认页复选框 =「同时删除全部数据与字体库」，中文不乱码
#   ③ 升级安装（已有旧目录）时目录页被跳过，$INSTDIR 仍是旧目录（B3）
node tools/route-c.mjs on
```

这一趟同时验证：**没有备用自绘页也能装**、内联中文编码正确、`ZfmDirPre`/`un.ConfirmShow` 仍在工作。测完务必 `on` 回去，否则发版包没有路线 C 界面。

### 6.8 收尾断言

```bash
node tools/route-c.mjs status    # on
git status --short               # 只应有 §1 允许改的文件
git diff --stat
```

---

## 7. 风险与回滚

### 7.1 已知变化（全部只影响回退面）

| # | 变化 | 影响 | 处置 |
|---|---|---|---|
| R1 | 删除 `sidebar.bmp` 后，回退面欢迎/完成页显示 NSIS 默认蓝色 `win.bmp` | 纯视觉，正常路线 C 看不到 | 不能接受 → 保留 `sidebar.bmp` + `sidebarImage`，只删自绘页；这是唯一例外，需在提交说明里写清 |
| R2 | 回退面升级不再提供「先卸载再装」/WiX 自动卸载 | 升级一律直接覆盖（推荐默认），不产生第二套；老 WiX 安装可能残留一条卸载项 | 接受；在 release note 记一句 |
| R3 | 回退面完成页不再显示「数据保存在 data 文件夹」 | 信息减少 | 想保留就留下 `MUI_FINISHPAGE_TITLE/TEXT` 两行（只是文案） |
| R4 | 内联中文若存成 GBK，回退面卸载确认页会乱码 | 编译不报错，装机才发现 | 保存为 UTF-8；§6.7 ② 就是编码探针 |
| R5 | 删到 `Section` / `un.ConfirmShow` / handlebars | 数据或更新链断裂 | `nsis-fork-audit.js` 红线 + §6.1 第 5 条 diff 断言 |

### 7.2 2.1.5 行为基线 B1–B20 对照

依据 `docs/PRD-2.1.6-安装器路线B.md` §2。结论：**20 条全部不触碰**。

| 编号 | 依赖代码 | 本次是否触碰 |
|---|---|---|
| B1 按用户安装、默认 `%LOCALAPPDATA%\FontManager` | `.onInit:876-895` | 否 |
| B2 `/D=` 与目录页浏览 | `ZfmDirPre:668-671` | 否（钩子保留） |
| B3 升级锁目录 | `ZfmDirPre:679-683` | 否（钩子保留） |
| B4 目录页写探针 | `ZfmDirLeave:693-704` | 否（钩子保留） |
| B5 `/S` 全静默、退出码 0 | 内建 `Silent` + Section | 否 |
| B6 `/P` 被动 | `Var PassiveMode` + `SkipIfPassive` + `ZfmDirPre:664` | 否（欢迎页换成原生后仍带 `SkipIfPassive`） |
| B7 `/UPDATE` | `.onInit:865-868`、`un.onInit:1229-1232`、`hooks.nsh` | 否 |
| B8 `/NS` | `.onInit:848-851`、两个快捷方式函数 | 否 |
| B9 `/R` + `/ARGS` | `.onInstSuccess:1203-1212` | 否 |
| B10 运行中检测 | `Section Install:1098`、`:1284` | 否 |
| B11 注册表 13 值 + `Installer Language` | `Section Install:1153-1172` | 否 |
| B12 `App Paths` | 不存在 | 否 |
| B13 文件关联 | 主程序写入；安装器不碰 | 否 |
| B14 开始菜单快捷方式 | `:1175-1177`、`:1416-1456` | 否 |
| B15 桌面快捷方式 | `:738-740`、`:1179-1184`、`:1458-1483` | 结构保留，只删文案 |
| B16 卸载程序本身 | `:1132`、`:1159` | 否 |
| B17 卸载问「是否保留数据」 | `un.ConfirmShow/Leave:766-798` | 否（保留） |
| B18 白名单精确删除 | `hooks.nsh` | 否（一字未动） |
| B19 `data\` + `.zfm-data-root` 追加 | `hooks.nsh` POSTINSTALL | 否 |
| B20 WebView2 检测/安装 | `Section WebView2:958-1089` | 否 |

> 唯一消失的是 `PageLeaveReinstall` 里的 E-1 修复（`unableToUninstall` 的 `${If} ${Silent}` 保护）——整段功能随页面删除，代码路径不存在了，不是回归。`/S` 侧的失败语义仍由 `Section WebView2` 的两处 `SetErrorLevel 1` 和 `.onInstSuccess` 的 `SetErrorLevel 0` 负责。

### 7.3 回滚

不跑 `git reset --hard`、不删工作区。

```bash
# 只回滚本次动过的路径（会一并恢复被删的 bmp / nsh）
git checkout <§5.0 记下的 SHA> -- src-tauri/windows src-tauri/tauri.conf.json

# 或者按提交回退（推荐：每步一个 commit）
git log --oneline
git revert <commit>
```

「保留 git 分支 + 每步一个 commit」比任何备份都可靠：真出问题，`git checkout <SHA> -- src-tauri/windows` 一条命令回到改动前，`src-tauri/src`、`src/setup` 完全不受影响。

### 7.4 提交建议

```text
chore(installer): 移除 NSIS 备用安装/卸载 UI，界面只保留路线 C

- 删自绘欢迎页 ZfmWelcome / 升级页 PageReinstall 一族
- 删目录页、完成页文案覆盖；保留 ZfmDirPre/ZfmDirLeave（B2-B4）
- 保留 un.ConfirmShow 删数据复选框（B17）与完成页快捷方式结构（B15）
- zfm-strings.nsh 仅 5 条行为文案内联进 installer.nsi
- 删 sidebar.bmp / header.bmp / gen-nsis-images.py 与 sidebarImage 配置
- 回退面 = 原生 MUI 页；ZFM_ROUTE_C 与 hooks.nsh 未动
```

---

## 附录 A · 一页速查

| 我要确认 | 命令 / 位置 |
|---|---|
| 路线 C 开着吗 | `node tools/route-c.mjs status` |
| 还有没有自绘页 | `grep -n "Page custom" src-tauri/windows/installer.nsi` |
| 文案文件死透了吗 | `grep -rn "zfm-strings" src-tauri/windows` |
| 图片还有人引用吗 | `grep -rn "sidebar.bmp\|header.bmp" src-tauri` |
| 红线破了吗 | `node tools/nsis-fork-audit.js _tmp/pristine.nsi src-tauri/windows/installer.nsi` |
| `/S` 会挂吗 | `node tools/check-silent-paths.mjs` |
| 装完退几 | `$p.ExitCode`（0 成功 / 10 用户取消 / 其它失败） |
| 界面是谁画的 | 1000×620 无标题栏 = 路线 C；灰白向导 = 回退面 |
