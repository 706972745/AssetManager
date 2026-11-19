# FontManager 视觉基准核对（2026-11-19）

> 上一版 `FontManager-视觉与体验对齐规范-2026-11-19.md` 的 §2 颜色表与 §8 的 P0-2 / P0-3 / P1-a / P1-c
> **取值来源有误**：读的是 `app.css` 里 R4 之前的基础段（玻璃时代），而不是 R4/R5 追加段的最终覆盖值。
> 本文档是**更正版**，所有数值都标注了「最后一次定义」的行号。上一版对应条目以本文为准。
> FM 全程只读（`git status` 未产生任何改动）。

---

## 1. 一句话：FM 2.5.4 的视觉基准是什么

**基准不在 `design/tokens.css`，而在 `src/app.css` 末尾连续 5 段「追加覆盖」的最后生效值。**
`tokens.css` 里的 `--canvas:#0e0e0e`、`--accent-a:#d4d4d8`、`--e1:rgba(255,255,255,.04)`、
`--shadow-mass:0 6px 12px` 全是**玻璃时代（2.3.0 之前）的残留**，运行时被后面的段整体替换。

优先级链（`src/main.tsx:6-8` 的导入顺序决定）：

| 序 | 来源 | 文件:行号 | 管什么 | 是否最终生效 |
|---|---|---|---|---|
| 1 | `design/tokens.css` | 全文 | 字号 / 动效时长 / 光标色 / 玻璃系 `--e*`、灰 `--accent-a` | 被 2-8 覆盖 |
| 2 | `app.css` 基础段 | `1-7900` | 玻璃时代组件样式（`backdrop-filter`、半透明叠加） | 部分属性仍生效（如 `.detail` 的 `background`） |
| 3 | **R4/R5 第一批** | `7911-8480` | `--ui-*` 映射、`--canvas:#111110`、滚动条、卡片/表格/网格、Topbar 输入框 2px、详情检查器 | ✅ |
| 4 | **R4/R5 第二批** | `8481-8737` | **强调色灰→蓝**（`--accent-a:#5b8cff` / `--accent-b` / `--accent-gradient` / glow）、`--e1..e4` 实色化、**glass 高光 → transparent**、三级阴影重定、遮罩去模糊、弹窗族 12px、Toast 形态 | ✅ **本轮最关键的段** |
| 5 | **R4/R5 第三批** | `8738-9091` | `.toolbar`、**字号滑杆 2px 轨道 + 8px 实心蓝点**、视图切换 2% 底、在线库、样张 | ✅ |
| 6 | R7 插件分区 | `9092-9391` | 只定义 `--pl-*`（插件/运行时/插件仓库菜单），**不外溢到主界面** | 仅插件子树 |
| 7 | 补丁段 | `9392-9750` 内 | 遮罩深色 **0.6**（覆盖 8547 的 0.46）、`.toaster` gap **14**、减弱动效收口 | ✅ |
| 8 | **R8 尺度收口** | `9521-~9750` | 圆角 5 档（0/4/8/12/语义）、半像素字号并档（10.5→11）、控件高 24/28/32；**明令「轨道类 2px 原样不动」** | ✅ |
| 9 | 后续功能段 | `9750-11693` | 设置卡片化 / 关于页 / 插件中心内嵌遮罩，各管自己子树 | 局部 |
| 10 | `design/primitives/blinkCaret.css` | 全文 | 只管自绘光标 | 仅光标 |

**覆盖规则**：同选择器同特异性 → **后写赢**；`:root:not([data-theme="light"])`（0,2,0）> `:root`（0,1,0）；
`tokens.css` 先导入、`app.css` 后导入 → 同名令牌 **app.css 赢**。

**所以「FM 的最新配色」= R4/R5 第二批 + 第三批 + R8 的最终值**：
纸墨实色底（`--e1 #1a1a18 … --e4 #2a2a28`）、**唯一蓝强调**（`--ui-accent` / `--accent-a` 都是 `#5b8cff`，
只标当前行与焦点）、**没有玻璃高光**（`--glass-top-highlight: transparent`）、
三级阴影极轻（`0 1px 2px/.28`、`0 4px 12px/.3`、`0 12px 32px/.34`）、圆角只有 0/4/8/12。

---

## 2. 已落地 4 条 P0 的逐值核对

| # | AM 现值 | FM 最终值（来源） | 判定 |
|---|---|---|---|
| 1 | `--hairline` 深 `rgba(255,255,255,.05)` / 浅 `rgba(20,20,18,.06)`（`tokens.css:69,184`） | **FM 没有 `--hairline` 这个令牌**，逐处用 `rgba(var(--ink-rgb),N)`：标题栏下沿 `.05`（`app.css:80`）、侧栏右缘 `.05`（`157`）、详情左沿 `.06`（`1447`）、卡片底 `.07`；浅色 `--ink-rgb = 22,22,32`（`tokens.css:120`） | **基本一致**。深色数值与 `.05` 逐字相同（深色 ink 就是 255,255,255）；浅色 AM 用 `(20,20,18)` vs FM `(22,22,32)`，肉眼无差；但 FM 分 `.05/.06/.07` 三档，AM 只有一档 → 详情左沿应略重一点 |
| 2 | `--ease-out: cubic-bezier(.16,1,.3,1)`、`--ease-in: (.4,0,1,1)`、`--ease-std: (.4,0,.2,1)`（`tokens.css:123-125`） | `tokens.css:106-108` 逐字相同 | **一致** ✅ |
| 3 | 滑杆：4px 轨道 + 渐变填充 + 15px `#f5f5f7` 滑块 + 光晕 + hover `scale(1.15)`（`app.css:236-266`） | **FM 最终 = 2px 轨道 `--ui-line` + 8px 实心 `--ui-accent` 圆点 + `box-shadow:none` + hover 不缩放 + `margin-top:-3px`**（`app.css:8755-8796`；R8 C 级第 ④ 条 `9560` 明令「轨道类 2px 原样不动」） | **❌ 不一致，且是回退**。我上一版给的 629-670 是**旧玻璃段**（15px 白旋钮），已被 8755 覆盖。AM 现在正是旧的 15px 白旋钮 → **要改回 8px 蓝点** |
| 4 | `.set-sheet`：`background: var(--e3)`、`border-radius: 12px`、`box-shadow: var(--shadow-ambient)`（`app.css:1660-1685`） | `--e3` 深 `#242422`（`app.css:8514`）/ 浅 `#ffffff`（`8526`）；`.settings-panel` `border-radius:12px`、`background:var(--e3)`、`box-shadow:var(--shadow-ambient)`（`8557-8567`） | **结构一致 ✅，令牌值需改**：AM `--shadow-ambient = 0 12px 36px rgba(0,0,0,.45)`，FM = `0 12px 32px rgba(0,0,0,.34)`（`8522`） |

---

## 3. P0 差异清单（一眼看出不是最新设计，按观感影响排序）

| # | 位置（AM） | AM 现值 | FM 最新值（来源） | 观感差异 |
|---|---|---|---|---|
| **P0-1** | `app.css:236-266` `.topbar-size` | 轨道 4px + 蓝→浅蓝渐变填充 + 15px 近白滑块 + 1px 暗环 + 光晕 + hover 放大 1.15 | `app.css:8755-8796`：轨道 **2px** `var(--ui-line)`、滑块 **8px** 实心 `var(--ui-accent)`、`box-shadow:none`、hover `transform:none`、`margin-top:-3px`、无渐变填充 | 顶栏最显眼的控件：FM 是「一条细线 + 一颗干净蓝点」，AM 是「粗轨道 + 白旋钮 + 光晕」——**这是 owner 一眼看出不是最新设计的第一处，且是照我上一版错误规格改出来的，必须回退** |
| ~~P0-2~~ → **P1** | `tokens.css:76` `--accent-a` | `rgba(91,140,255,.16)`；**全站 17 处全部是"柔化"用途**（9 处 background 淡蓝底、5 处 `box-shadow: 0 0 0 3px` 光晕、3 处渐变起点），**没有一处当实色用** | `app.css:8493`：`--accent-a: #5b8cff`（实色，137 处当圆点/进度填充/文字色） | **见 §5 更正：这不是视觉缺陷，是命名冲突**。AM 的行选中竖线/进度条用的是实色 `--accent`，本来就"点亮"了。直接换值会让 11 处淡蓝底变实色蓝（`#f2f2ef` 压 `#5b8cff` 仅 2.82:1，11px 正文不达标）→ 必须拆两步做 |
| **P0-3** | `tokens.css`（缺） | 无 `--accent-b` / `--accent-gradient` | `app.css:8494-8495`：`--accent-b:#2f6bff`、`--accent-gradient:linear-gradient(135deg,#5b8cff,#2f6bff)` | 开关打开态、主按钮在 FM 是蓝→深蓝渐变；AM 只能平涂，缺一层"实体感" |
| **P0-4** | `tokens.css:86-87` `--glass-top-highlight(-strong)` | `rgba(255,255,255,.1)` / `.16`，仍被 4 处 `inset 0 1px 0` 使用（`app.css:342,925,1314,2609`） | `app.css:8518-8519`：两个都是 **`transparent`**（"纸墨体系里没有玻璃高光，留着会在卡片上缘画一道假亮边"） | AM 的视图切换滑块、浮层、卡片顶端各有一条 1px 假亮边，FM 已经全部去掉 |
| **P0-5** | `tokens.css:88-90` 三级阴影 | ground `.4` / mass `0 12px 36px .55` / ambient `0 12px 36px .45` | `app.css:8520-8522`：ground `0 1px 2px rgba(0,0,0,.28)`、mass `0 4px 12px rgba(0,0,0,.3)`、ambient `0 12px 32px rgba(0,0,0,.34)` | AM 的 Toast / 菜单 / 设置面板落影比 FM 重一倍（36px/55% vs 12px/30%），纸墨体系下看着"糊"、像旧玻璃卡 |
| **P0-6** | `tokens.css:198-200` 浅色阴影 | ground `(30,30,55,.12)` / mass `(20,20,18,.16)` / ambient `(20,20,18,.1)` | `app.css:8531-8533`：`.06` / `.08` / `0 12px 32px .1` | 浅色主题下 AM 的投影比 FM 黑一倍，白纸底上显得脏 |
| **P0-7** ✅已完成 | `app.css:153` `.topbar-field` 描边 | ~~`1px solid var(--hairline)`~~ → 现为 `1px solid var(--ui-line)` | `app.css:8299-8301`：`border: 1px solid var(--ui-line)`（#2a2a27）、`background: var(--ui-panel)`；R8 只把圆角改回 4px（`9580`），**描边仍是 `--ui-line`** | 已落地并核验（2026-11-19）：深色下输入框重新有了可见边界 |
| **P0-8** ✅已完成 | `app.css:169-175` `.topbar-field:focus` | ~~`border-color: var(--accent)` + `box-shadow: 0 0 0 3px var(--accent-a)`~~ → 现为 `border-color: var(--ui-accent)` + `box-shadow: none` | `app.css:8304-8307`：`border-color: var(--ui-accent)` + **`box-shadow: none`**（R4 "聚焦只转蓝，不加光晕"） | 已落地并核验；特异性 (0,2,0) 压过全局 `:focus-visible` 兜底，输入框聚焦只剩"描边变蓝" |
| **P0-9** | `tokens.css:77-78` glow | `--accent-glow: rgba(91,140,255,.35)`、`-soft: .18` | `app.css:8496-8497`：`.42` / `.2` | 焦点环（`box-shadow: 0 0 0 1px glow, 0 0 0 5px glow-soft`）比 FM 弱一档，Tab 过去不够醒目 |
| ~~P0-10~~ 误判 | `app.css:982-995` `.detail-inner` | `gap:16px`、`padding:18px 16px 22px`（**死代码**） | 真正生效的是 `.detail-body`，现值已是 FM 值 `padding:20px 18px 24px; gap:20px`（`app.css:3080-3083`） | **无需修改**：`.detail-inner` 后面被 `padding:0; gap:0` 覆盖，内距由 `.detail-body` 承担，而它已是 FM 值（2026-11-19 逐值确认） |

**次一级（P1，不进上表）**：Toast 缺 16px 状态图标列与 `min-height:52px`（FM `Toast.tsx:193-197` + `app.css:8663-8668`）、`.toaster` gap 应 14（`9458`）且 z-index 70（`2334`）、错误提示竖条 FM 恒为 `--ui-accent`（AM 改成了红）、`body` 字号仍 11px（FM `tokens.css:200` 是 14px）、浅色 `--hairline` 的 ink 应为 `(22,22,32)`。

---

## 4. 结论：AM 现在这套是不是 FM 最新设计

**部分（约 70%）。**

- **已经是最新**：骨架与布局（标题栏 40 / 侧栏 248 / 详情 348 / 顶栏 padding 10-16）、字号阶梯 9/11/13/14/16/20/28/40/56、字体族、`--e1..#e4` 实色阶、`--ui-*` 语义层、圆角 `--r-*`（0/2/4/8/12/pill/anchor，与 R8 的 5 档一致）、滚动条整套、行契约、设置面板 900×760 + 12px + `--e3`、遮罩 0.6、`--ease-*`、springs.ts。
- **还停在 R4 之前的玻璃时代**（这批是 owner "一眼看出不对"的来源）：
  1. 字号滑杆＝旧的 15px 白旋钮（**且是照我上一版错误规格改的，已于 2026-11-19 回退为 2px/8px 蓝点**）；
  2. `--accent-a` 的**命名与语义**和 FM 不同（不是视觉缺陷，见 §5），且缺 `--accent-b` / `--accent-gradient`；
  3. `--glass-top-highlight` 仍是 `.1/.16`（FM 已置 `transparent`），4 处假亮边还在；
  4. 三级阴影全部偏重（尤其 mass `36px/55%` vs `12px/30%`）；
  5. 顶栏输入框描边用 5% 白而非 `--ui-line`，聚焦还多一圈光晕；
  6. 详情面板内距比 FM 紧一档。
- **需要 owner 注意**：上一版规范里「P0-2 Toast」「P0-3 滑杆」「P1-a `--accent-a` 应为灰色」三条是**错的**（读到了被覆盖的基础段）。Toast 结构其实已基本对齐（只缺图标列/间距/入场动画），滑杆要改回 8px 蓝点，`--accent-a` 应为 `#5b8cff` 而不是灰色。

**建议动作顺序**：P0-1（回退滑杆，已完成）→ P0-4/5/6（令牌 5 行，全站生效）→ P0-7/8/9（顶栏 3 处）→ P0-10（详情内距）→ 最后再做 §5 的 `--accent-a` / `--accent-b` / `--accent-gradient` 两步拆解。全部集中在 `tokens.css` 与 `app.css` 的两三个块里，不涉及任何业务组件。

---

## 5. 更正：`--accent-a` 不是视觉缺陷，命名冲突要拆两步改

**我上一版把它列为 P0 是过头了，工程师的反驳成立。** 复核 AM 全部 17 处引用（`styles/app.css`）：

| 用途 | 行号 | 数量 |
|---|---|---|
| `background: var(--accent-a)`（淡蓝底） | 507、1594、2554、3413、6252、6921、7157、7279、7429 | 9 |
| `box-shadow: 0 0 0 3px var(--accent-a)`（柔光环） | 170、214、1711、1989、4828 | 5 |
| 渐变起点（`radial-gradient` / `linear-gradient`） | 907、5339、5780 | 3 |

**没有一处是当实色用的**。由此可见：AM 的行选中竖线（`.tree-row.sel::before{background:var(--accent)}`）、进度条填充（`.progress-fill{background:var(--accent)}`）本来就是实色蓝 —— 我上一版说"锚点没点亮"是错的。所以：

- **第 1 步（改名，无视觉变化）**：新增 `--accent-soft: rgba(91,140,255,.16)`（浅色 `rgba(27,79,224,.12)`），把上表 17 处全部换成 `var(--accent-soft)`。做完之后 `--accent-a` 的消费点数为 **0**。
- **第 2 步（对齐 FM 语义，同样无视觉变化）**：`--accent-a: #5b8cff`（浅色 `#1b4fe0`）、新增 `--accent-b: #2f6bff`（浅色 `#1a42b8`）、`--accent-gradient: linear-gradient(135deg,#5b8cff,#2f6bff)`（浅色 `1b4fe0 → 1a42b8`）。目的是**以后从 FM 复制的片段能 1:1 直接用**（FM 里 `--accent-a` 有 137 处，全是实色用途）。
- **为什么必须拆**：工程师实测 `#f2f2ef` 压 `#5b8cff` 只有 **2.82:1**，11px 正文不达 4.5:1。盲换会让 `.suggest-item.on`（7279）、`.lang-item.on`（7429）、`.tag-chip.pick.on`（3413）等 9 处选中行糊掉。

**顺带一个真·视觉收益（可选，3 行）**：AM `.switch.on` 现在用实色 `var(--accent)`，FM 的 `.pill-on` 用渐变 + 内高光 + 光晕（`app.css:1379-1386`）：
`background: var(--accent-gradient)` + `inset 0 1px 0 rgba(255,255,255,.25)` + `0 0 14px var(--accent-glow-soft)`。这是"开关手感像不像一家"的直接体现，但要等第 2 步有 `--accent-gradient` 之后才能做。

**另一处顺带**：AM `.seg` 圆角 `5px`（`app.css:1908`），R8 把同类分段控件统一到 4（`app.css:9580` 的 `.view-toggle`）。

---

## 6. 本轮落地核验（2026-11-19，逐值对文件确认）

### 6.1 已生效且与 FM 相等

| 项 | AM 现值 | FM 值 | 结论 |
|---|---|---|---|
| `--glass-top-highlight` / `-strong` | `transparent`（两套主题，`tokens.css:100-101` / `216-217`） | `app.css:8518-8519` | 一致 ✅ 四处 1px 假亮边消失，4 处 `inset 0 1px 0 transparent` 声明保留无害 |
| `--shadow-ground` | `0 1px 2px rgba(0,0,0,.28)`（`tokens.css:104`） | `8520` | 一致 ✅ |
| `--shadow-mass` | `0 4px 12px rgba(0,0,0,.3)`（`tokens.css:106`） | `8521` | 一致 ✅ |
| `--shadow-ambient` | `0 12px 32px rgba(0,0,0,.34)`（`tokens.css:105`） | `8522` | 一致 ✅ |
| 浅色三级阴影 | `.06` / `.1` / `.08`（`219-221`） | `8531-8533` | 一致 ✅ |
| `--accent-glow` / `-soft` | `.42` / `.2`（`87-88`）；浅色 `.3` / `.14`（`207-208`） | `8496-8497` / `8509-8510` | 一致 ✅ |
| `.topbar-field` | 描边 `--ui-line`、聚焦 `--ui-accent` + `box-shadow:none`（`app.css:153,169-175`） | `8299-8301` / `8304-8307` | 一致 ✅ |
| `.detail-body` 内距 | `padding:20px 18px 24px; gap:20px` | `3080-3083` | 一致 ✅（`.detail-inner` 的四行是死代码，不必改） |

### 6.2 连带效应（必须在验收单里说明，否则会被当成"没人改过却变了"）

**全局焦点环变强 ≈ +11% / +20%，方向是「向 FM 补齐」而不是「改坏了」。**
`tokens.css:290-294` 的兜底规则 `:focus-visible { box-shadow: 0 0 0 1px var(--accent-glow), 0 0 0 5px var(--accent-glow-soft) }`
引用这两个令牌；令牌从 `.35/.18` 提到 `.42/.2` 之后，所有**非输入元素**（按钮、树行、瓦片、菜单项）用键盘 Tab 时的焦点环同步变实。
关键判据：FM `tokens.css:247-251` 的兜底规则**逐字相同、引用同两个令牌**，所以
**AM 改后 = FM 现值**；变强的只是「相对 AM 上一个构建」。输入类被 `input:focus{outline:none; box-shadow:none}` 摘出去，不受影响。

### 6.3 一处更正：`.menu` 不吃 `--shadow-mass`

`.menu`（`app.css:1300`，排序/筛选下拉）的阴影是**硬编码** `inset 0 1px 0 var(--glass-top-highlight-strong), 0 1px 2px rgba(0,0,0,.5), 0 12px 32px rgba(0,0,0,.45)`，
所以本轮阴影令牌变化对它**无影响**（只有那层内高光变透明）。
真正消费 `--shadow-mass` 的是 **7 处**：`.toast`(1261)、`.lightbox-panel`(1414)、`.ctx-menu`(2299)、`.dlg`(2388)、`.batch-bar`(2472)、`.rating-pop`(5408)、`.hover-hint`(6147，后两处与 `--shadow-ambient` 叠加)。
7 处均有 1px `--ui-line` 描边 + 不透明实色底；最紧的一处是 `.ctx-menu` 压在 `--e1` 卡片上（`#1c1c1a` vs `#1a1a18`，底色差约 0.8%），靠描边约 6.3% 的对比撑住 —— 不会糊，**不需要补阴影**。

### 6.4 本轮仍未处理（留待下一批）

| 项 | 现状 | 归属 |
|---|---|---|
| `--accent-a` → `--accent-soft` 迁移（17 处）+ 实色化 | 未动 | 下一批，两步走（§5） |
| `--accent-b` / `--accent-gradient` 消费（`.switch.on`、主按钮） | 令牌已预置、无消费点（`tokens.css:82-86` 有防删注释） | D 组 |
| `--focus-ring-w` / `--focus-ring-offset` | 缺失，滑块用 `var(--focus-ring-offset, 2px)` 兜底 | 下批补令牌 |
| `.seg` 圆角 5 → 4 | 未动 | D 组 |
| Toast 4 条（16px 图标列 / `min-height:52px` / `gap:14`+`z:70` / 错误条恒蓝） | 主体已对齐 | C 组，待派单 |
| `body` 字号 11px vs FM 14px | 未动 | **需决策**：会牵动所有未显式设字号的文字行高，必须 QA 全量扫，「改了更好」不是「必须改」 |


