# AssetManager × FontManager 视觉与体验对齐规范

- **参照方**：FontManager `2.5.4`（`git 977483a`），路径 `FontManager/Fonts工程文件/ZFontManager`（**只读**）
- **改造方**：AssetManager `0.3.31`，路径 `AssetManager`
- **产出日期**：2026-11-19
- **本文只谈视觉与交互手感**，不重写架构、不动业务逻辑（素材网格 / 灯箱 / 贴图全部保留）

> ⚠️ **读 FM 值的唯一正确姿势**：FontManager 的 `src/design/tokens.css` 里 `--canvas: #0e0e0e`、
> `--accent-a: #d4d4d8`、`--e1: rgba(255,255,255,.04)` 是 **2.3.0 之前的玻璃体系残留**，
> 运行时被 `src/app.css:7963` 起的「主线视觉对齐 R4/R5」追加段整体覆盖成
> `--canvas:#111110`、`--ui-accent:#5b8cff`、`--e1:#1a1a18`（`app.css:8513`）。
> **量 FM 的数值必须 grep 出全部同名定义、取最后一条**（`docs/FontManager设计统一.md` §1.2 的告警）。
> 本文所有 FM 值均为「最终生效值」。

---

## 0. 一句话结论

**两者已经是同一副骨架，但 AssetManager 在「线」和「动」上没对齐**：AM 把 41 处描边写成
`--hairline: rgba(20,20,18,.06)`（一条**深色**的 6% 黑线），在 `#111110` 的深色画布上等于没画 ——
FM 里靠 `rgba(var(--ink-rgb),.05)` 随主题反转的发丝线，在 AM 深色下**全部消失**；同时 AM 缺整组缓动
令牌（`--ease-*`）、Toast / 滑块 / 弹层底板三处形态与 FM 明显不同。所以并排看是「同一个布局、不同的清晰度和手感」，
不是「两个软件」。

### 0.1 P0 速览（一眼看出不是一家的 7 条）

| # | 改哪里 | 现在 | 改成 |
|---|---|---|---|
| P0-1 | `app/src/styles/tokens.css:60,158` `--hairline` | 深色 `rgba(20,20,18,.06)`（黑线，看不见） | 深色 `rgba(255,255,255,.05)`，浅色 `rgba(20,20,18,.06)`；**或**把 41 处调用点统一换成 `rgba(var(--ink-rgb),.05)` |
| P0-2 | `app/src/components/ui.tsx:478` `Toaster` + `app.css:1220` `.toast` | 无动画、无图标、标题 11px/500、左侧 2px 蓝/红竖条 | 搬 FM `design/primitives/Toast.tsx`：`springSnappy` 进场（x40,y-10,scale .94）、16px 图标、标题 `--fs-13`/600、详情 `--fs-11`、最多 4 条、TTL 4/6.5/7s |
| P0-3 | `app.css:221-264` `.topbar-size` 滑杆 | 轨道 2px、滑块 8px 蓝点 | FM `app.css:629-670`：轨道 4px + 渐变填充、滑块 15px `#f5f5f7` + 光晕、hover `scale(1.15)` |
| P0-4 | `app/src/styles/tokens.css:206-214` `body` 字号 | `--fs-11`（11px） | FM `tokens.css:200`：`--fs-14`（14px，行高 1.5） |
| P0-5 | `app/src/styles/tokens.css`（缺） | 无 `--ease-out/in/std` | 抄 FM `tokens.css:106-108` 三条曲线，并把全站 `transition` 的曲线换成令牌 |
| P0-6 | `app.css:1632` `.set-sheet` | `background: var(--canvas)`（#111110）+ `radius 10` | FM `app.css:8556-8566`：`background: var(--e3)`（#242422）+ `radius 12` + `--shadow-ambient` |
| P0-7 | 启动页 | **不存在** | 照 `splash.html` 做：1120×760、图标 156、进度 336×4、最短显示 1.5s（见 §6） |

---

## 1. 字体与字号阶梯

### 1.1 令牌对照（`design/tokens.css`）

FM：`src/design/tokens.css:69-84` / AM：`app/src/styles/tokens.css:78-93`

| 变量 | FM 值 | AM 值 | 一致 |
|---|---|---|---|
| `--fs-9` | `9px` | `9px` | ✅ |
| `--fs-10` | `10px` | `10px` | ✅ |
| `--fs-11` | `11px` | `11px` | ✅ |
| `--fs-13` | `13px` | `13px` | ✅ |
| `--fs-14` | `14px` | `14px` | ✅ |
| `--fs-16` | `16px` | `16px` | ✅ |
| `--fs-20` | `20px` | `20px` | ✅ |
| `--fs-28` | `28px` | `28px` | ✅ |
| `--fs-40` | `40px` | `40px` | ✅ |
| `--fs-56` | `56px` | `56px` | ✅ |
| `--font-ui` | `"Inter Variable","Inter",system-ui,"Segoe UI","Microsoft YaHei","PingFang SC",…` | 逐字相同 | ✅ |
| `--font-mono` | `"JetBrains Mono Variable","JetBrains Mono",monospace` | 逐字相同 | ✅ |
| 日语 / 繁中字族覆盖 | `tokens.css:186-196` 有 `:root[lang="ja"]`、`:root[lang="zh-TW"]` | **无** | ❌ P2 |

### 1.2 行高 / 字重 / 根字号

| 属性 | FM | AM | 一致 |
|---|---|---|---|
| `body` font-size | `--fs-14`（`tokens.css:200`） | `--fs-11`（`tokens.css:209`） | ❌ **P0-4** |
| `body` line-height | `1.5` | `1.5` | ✅ |
| `body` font-weight | 未声明（400） | 未声明（400） | ✅ |
| `-webkit-font-smoothing` | `body` 上 `antialiased` | `*` 上 `antialiased` | ✅（等效） |
| 防选中 | `body{user-select:none}` + `input,textarea{user-select:text}` | 同 + `[contenteditable]` | ✅ |

### 1.3 字阶分配约定（FM `docs/FontManager设计统一.md` §1.2，必须遵守）

| 档位 | 用途 |
|---|---|
| `--fs-9` | 分区小标题（全大写、`letter-spacing: .08em`、等宽体、`--ui-accent` 色） |
| `--fs-11` | **所有「一行文字」**：行名、行尾计数、面板标题、按钮、输入框、说明文字 |
| `--fs-13` | 面板大标题、导航项 |
| `--fs-16`+ | 标题 / 数字艺术 |

**主次靠字重与颜色区分，不靠字号。** FM 明确列出「历史遗留、不要照抄」的四档：`12px`（卡片名/提示语）、
`14px`（设置项名）、`22/24px`（About 大标题）、`8px`（徽标/键帽）。

### 1.4 例外的裸字号（AM 现状，需逐条处理）

| AM 位置 | 现用 | FM 对应 | 建议 |
|---|---|---|---|
| `app.css:986` `.detail-title` | `15px` / 600 | `.detail-title` = `--fs-20` / 600 / line-height 1.15 | **P1** 改 `--fs-20` |
| `app.css:3758`（另见 §4 说明） | `14px` | 设置项名 = `--fs-14`（FM 允许） | 保留 |
| `app.css:4298,4316` | `12px` | FM 卡片名允许 12px | 保留 |
| `app.css:1498` `.detail-artmeta` 附近 | `10px` | FM 数字一律 `--fs-11` + `tabular-nums` | **P1** 改 `--fs-11` |
| `app.css:4425` 截图工具栏 | `13px` | 独立设计稿，不在对齐范围 | 忽略 |

---

## 2. 颜色体系

### 2.1 深色主题

FM 生效值：`src/app.css:6250-6260`（`--qb-*`）+ `:7963-7985`（`--ui-*` 映射）+ `:8513`（`--e*`）
AM：`app/src/styles/tokens.css:16-76`

| 令牌 | FM | AM | 一致 |
|---|---|---|---|
| `--canvas` / `--ui-bg` | `#111110` | `#111110` | ✅ |
| `--ui-panel` | `#171715` | `#171715` | ✅ |
| `--ui-menu` | `#1c1c1a` | `#1c1c1a` | ✅ |
| `--ui-hover` | `#1c1c1a` | `#1c1c1a` | ✅ |
| `--ui-fg` / `--text-primary` | `#f2f2ef` | `#f2f2ef` | ✅ |
| `--ui-fg-2` / `--text-secondary` | `#9a9a94` | `#9a9a94` | ✅ |
| `--ui-fg-3` / `--text-tertiary` | `#666660` | `#666660` | ✅ |
| `--text-disabled` | `rgba(242,242,239,.25)`（`--ink-rgb` 25%） | `rgba(242,242,239,.25)` | ✅ |
| `--ui-line` | `#2a2a27` | `#2a2a27` | ✅ |
| `--ui-line-hover` | `#9a9a94`（`--qb-anchor-hover`） | `#9a9a94` | ✅ |
| `--ui-accent` | `#5b8cff` | `#5b8cff` | ✅ |
| `--e0` | `rgba(255,255,255,0)` | `rgba(255,255,255,0)` | ✅ |
| `--e1` | `#1a1a18` | `#1a1a18` | ✅ |
| `--e2` | `#1f1f1d` | `#1f1f1d` | ✅ |
| `--e3` | `#242422` | `#242422` | ✅ |
| `--e4` | `#2a2a28` | `#2a2a28` | ✅ |
| `--tooltip-bg` | `#1b1b1b` | `#1b1b1b` | ✅ |
| `--glass-border` | `#2a2a27` | `#2a2a27` | ✅ |
| `--glass-top-highlight` | `rgba(255,255,255,.1)` | `rgba(255,255,255,.1)` | ✅ |
| `--glass-top-highlight-strong` | `rgba(255,255,255,.16)` | `rgba(255,255,255,.16)` | ✅ |
| `--accent-a` | **`#d4d4d8`（实色浅灰）** | **`rgba(91,140,255,.16)`（16% 蓝）** | ❌❌ **P1（高危）** |
| `--accent-b` | `#a1a1aa` | **无** | ❌ P1 |
| `--accent-gradient` | `linear-gradient(135deg,#d4d4d8,#a1a1aa)` | **无** | ❌ P1 |
| `--accent-glow` | `rgba(212,212,216,.45)` | `rgba(91,140,255,.35)` | ❌ P1 |
| `--accent-glow-soft` | `rgba(212,212,216,.22)` | `rgba(91,140,255,.18)` | ❌ P1 |
| `--success` / `--warning` / `--error` | 三者全 `#d4d4d8`（刻意无色语义） | `--warning:#d4d4d8`；**无 success/error**，另有 `--danger:#e05a4a` | ❌ P1 |
| `--expiry-red` | `#e5484d` | **无** | ❌ P2（AM 暂无到期标签） |
| `--caret-blue` | `#4c8dff` | `#4c8dff` | ✅ |
| `--font-pending` | `#e08a72` | **无** | P2（AM 无「假字体」概念） |
| `--focus-ring` | `var(--accent-a)`（`tokens.css:40`） | **无** | ❌ P1 |
| `--focus-ring-w` / `--focus-ring-offset` | `2px` / `2px` | **无** | ❌ P2 |
| `--shadow-ground` | `0 1px 2px rgba(0,0,0,.4)` | 同 | ✅ |
| `--shadow-mass` | `0 6px 12px rgba(0,0,0,.3)` | `0 12px 36px rgba(0,0,0,.55)` | ❌ P2 |
| `--shadow-ambient` | `0 24px 48px rgba(0,0,0,.25)` | `0 12px 36px rgba(0,0,0,.45)` | ❌ P2 |
| 遮罩 | `rgba(0,0,0,.46)`（`app.css:8547`，无模糊） | `--scrim: rgba(0,0,0,.6)` | ❌ P1 |
| `--hairline` | **FM 没有这个令牌**（一律 `rgba(var(--ink-rgb),.05)`） | `rgba(20,20,18,.06)` | ❌ **P0-1** |

> **`--accent-a` 是最危险的一条**：同名不同义。FM 的 `--accent-a` 是**实色浅灰**
> （当进度条填充、圆点、focus-ring 用，137 处引用）；AM 的 `--accent-a` 是**16% 蓝**
> （当"淡蓝底/focus 光晕"用，全站 20 余处）。任何从 FM 里整段搬过来的样式，只要用了
> `var(--accent-a)`，在 AM 里会变成一块几乎透明的蓝 —— 反过来也一样。
> **建议**：AM 把 `--accent-a` 改名为 `--accent-soft`（保留现值），并新增真正的
> `--accent-a: #d4d4d8` / `--accent-b: #a1a1aa` / `--accent-gradient`，之后才允许复制 FM 片段。

### 2.2 浅色主题

FM：`app.css:6264-6273` + `:7986-7994` + `:8525-8528`；AM：`tokens.css:124-173`

| 令牌 | FM | AM | 一致 |
|---|---|---|---|
| `--canvas` | `#fafaf8` | `#fafaf8` | ✅ |
| `--ui-panel` | `#ffffff` | `#ffffff` | ✅ |
| `--ui-line` | `#e2e2dd` | `#e2e2dd` | ✅ |
| `--ui-hover` | `#f1f1ed` | `#f1f1ed` | ✅ |
| `--ui-accent` | `#1b4fe0` | `#1b4fe0` | ✅ |
| `--e1..--e4` | `#f2f2ee` / `#eaeae5` / `#ffffff` / `#f7f7f3` | 同 | ✅ |
| `--text-primary/-2/-3` | `#141412` / `#6b6b66` / `#9b9b95` | 同 | ✅ |
| `--accent-a` | `#3f3f46`（实色） | `rgba(27,79,224,.12)` | ❌ 同 2.1 |
| `--accent-glow` | `rgba(63,63,70,.38)` | `rgba(27,79,224,.3)` | ❌ |
| 遮罩 | `rgba(20,20,18,.24)` | `rgba(20,20,18,.24)` | ✅ |

### 2.3 `--hairline` 为什么是 P0（重要）

- FM：`app.css:80` `.titlebar{border-bottom:1px solid rgba(var(--ink-rgb),.05)}` —— 深色下是 5% **白**线，浅色下是 5% **黑**线。
- AM：`app.css:28` `.titlebar{border-bottom:1px solid var(--hairline)}`，而 `--hairline` 在**两套主题下都是 `rgba(20,20,18,.06)`**。
- 结果：AM 深色下 `rgba(20,20,18,.06)` 叠在 `#111110` 上，色差 ≈ 1.1% —— 肉眼不可见。用 `grep -c "var(--hairline)"` = **41 处**：标题栏下沿、侧栏右缘、顶栏控件描边、详情徽标框、卡片描边全部因此消失。
- 证据：`docs/shots-0.6.0-真机深色.png` 里侧栏与主区之间没有分界线；FM `main.png` 里同位置有一条清晰细线。
- 修法（二选一，推荐 A）：
  - **A**：`--hairline` 分主题定义 —— 深色 `rgba(255,255,255,.05)`、浅色 `rgba(20,20,18,.06)`。改动 2 行，41 处调用点自动生效。
  - **B**：把 41 处换成 `rgba(var(--ink-rgb),.05)`，与 FM 逐字一致。

---

## 3. 板块配色

### 3.1 五块面板逐项对照

| 板块 | 类名（FM / AM） | 属性 | FM 值 | AM 值 | 一致 |
|---|---|---|---|---|---|
| 标题栏 | `.titlebar` / `.titlebar` | 高 | `40px` | `var(--titlebar-h)=40px` | ✅ |
| | | 内距 | `0 5px 0 14px` | `0 5px 0 14px` | ✅ |
| | | 下沿 | `rgba(ink,.05)` | `var(--hairline)` | ❌ P0-1 |
| | | 底 | `rgba(ink,.015)` | `rgba(ink,.015)` | ✅ |
| 侧栏 | `.sidebar` / `.tree` | 宽 | `248px` | `--tree-w=248px` | ✅ |
| | | 内距 | `10px 10px 12px` | `10px 10px 12px` | ✅ |
| | | 间距 | `gap:4px` | `gap:4px` | ✅ |
| | | 右沿 | `rgba(ink,.05)` | `var(--hairline)` | ❌ P0-1 |
| | | 底 | `var(--ui-bg)` | `var(--ui-bg)` | ✅ |
| 顶栏 | `.topbar` / `.topbar` | 内距 / 控件高 | `10px 16px` / `--control-h:36px` | 同 | ✅ |
| | | 下沿 / 底 | `rgba(ink,.05)` / `rgba(ink,.01)` | 同 | ✅ |
| | `.topbar-field` | 圆角 / 描边 | `2px` / `--ui-line` | `--r-4=4px` / `var(--hairline)` | ❌ P1 |
| | | 底 / 聚焦 | `--ui-panel`；聚焦 `--ui-accent` + `box-shadow:none` | `--ui-panel`；聚焦 `--accent` + `0 0 0 3px var(--accent-a)` | ❌ P1 |
| 内容区 | `.grid-scroll` / `.viewport` | 内距 | `18px 20px` | `14px 16px 22px` | ❌ P1 |
| | | 底 | 继承 `--ui-bg` | `var(--ui-bg)` | ✅ |
| 详情面板 | `.detail` / `.detail` | 宽 / 左沿 | `348px` / `rgba(ink,.06)` | `348px` / `rgba(ink,.06)` | ✅ |
| | | 底 | `rgba(ink,.025)` + `backdrop-filter: blur(32px) saturate(140%) brightness(1.05)` | `rgba(ink,.02)`，**无模糊** | ❌ P1 |
| | `.detail-inner` | 内距 / 间距 | `20px 18px` / `gap:18px` | `18px 16px 22px` / `gap:16px` | ❌ P1 |
| | 滚动条 | | 隐藏（`scrollbar-width:none`，2026-11-19 用户要求） | 显示 | ❌ P1 |
| 底部状态栏 | `.warm-footer` / **无** | | `padding:6px 16px 8px`、`border-top:rgba(ink,.05)`、`--fs-10`/500、`--text-tertiary` | AM 无底栏 | ❌ P1 |

### 3.2 弹层 / 浮层

| 项 | FM | AM | 一致 |
|---|---|---|---|
| 设置面板尺寸 | `min(900px,96vw)` × `min(760px,94vh)`，`grid: 196px | 1fr`（`app.css:11115`） | 同（`app.css:1616`） | ✅ |
| 面板圆角 | `12px`（`app.css:8556`） | `10px` | ❌ P1 |
| 面板底 | `var(--e3)`（`#242422`；`app.css:8566` 有整段注释解释「用 e3 才是浮起来」） | `var(--canvas)`（`#111110`） | ❌ **P0-6** |
| 面板阴影 | `var(--shadow-ambient)` | `var(--shadow-mass)` | ❌ P2 |
| 遮罩 | `rgba(0,0,0,.46)`，**不加模糊** | `rgba(0,0,0,.6)`，无模糊 | ❌ P1 |
| 遮罩圆角 | `var(--qb-radius,8px)`（防四角露直角） | `var(--qb-radius)` | ✅ |
| 菜单底 / 圆角 | `--ui-menu` / `8px` | `--ui-menu` / ? | ✅（圆角沿用） |
| Toast 位置 | `fixed; top:54px; right:16px; gap:8px; z-index:70` | `top:54px; right:16px; gap:12px; z-index:90` | ❌ P1 |
| Toast 形态 | `glass-e3` 玻璃 + 16px 状态图标 + 动作按钮 | `--ui-menu` 实底 + 左侧 2px 竖条 + 无图标 | ❌ **P0-2** |

### 3.3 行契约（列表行，FM 硬约定，AM 已对齐）

| 属性 | FM `.nav-row` | AM `.tree-row` | 一致 |
|---|---|---|---|
| 高 | `32px` | `32px` | ✅ |
| 左右内距 | `11px` | `11px` | ✅ |
| 字号 / 字重 | `11px` / `500` | `--fs-11` / `500` | ✅ |
| 圆角 | `0` | `0` | ✅ |
| 图标 | `15px` / `strokeWidth 1.5` | `size={15} stroke={1.5}` | ✅ |
| 悬停 / 选中底 | `--ui-hover` | `--ui-hover` | ✅ |
| 选中竖线 | `2px` `--ui-accent`，贴左、通高、`z-index:1` | `2px` `--accent` + `border-radius:var(--r-anchor)=1px` | ⚠️ 圆角应删（FM 是直角）P2 |

---

## 4. 可直接复用（一字不改）的清单

判定标准：**不依赖字体业务数据**（font/face/store）即可整段搬。

| # | 源文件（FM） | 内容 | 依赖 | 可否直接搬 |
|---|---|---|---|---|
| 1 | `src/design/springs.ts`（31 行） | 4 条 spring + `easeTop` + `scrimFade` + stagger | 仅 `motion/react` | ✅ **已经搬齐**（AM `design/springs.ts` 数值逐条相同，仅多一个 `respectMotion`） |
| 2 | `src/design/primitives/blinkCaret.css`（111 行） | 自绘块状光标样式 | 无 | ✅ 可搬；AM 已把等价规则内联在 `styles/app.css:1505-1560`，**内容一致**，不必再搬 |
| 3 | `src/design/primitives/BlinkCaretInput.tsx`（283 行） | 块状光标输入框 | `lucide-react`、`blinkCaret.ts` | ⚠️ AM 已在 `ui.tsx` 里自有一套等价实现（含 IME `is-composing` 还原），**建议不替换**，只做行为核对 |
| 4 | `src/design/primitives/Toast.tsx`（227 行） | Toast 队列 + 合并 + 暂停 + 动作按钮 | `zustand`、`springs`、`lib/sound`、`lib/toastQueue` | ⚠️ **搬 UI 与动画，换数据源**：删掉 `playError/playKind`、`enqueueToast`，改成 AM 自己的 toast 状态 |
| 5 | `src/design/primitives/PillToggle.tsx`（84 行） | 开关（34×19 / 46×26，滑块 scaleX 1.25 按下反馈） | `lib/sound` 的 `playToggle` | ⚠️ 删掉 3 行 `playToggle` 即可整段搬；AM 现有 `.switch`（`app.css:1901`）是**静态**开关，无按下反馈、无 `role="switch"` 组件封装 |
| 6 | `src/design/primitives/BottomBar.tsx`（52 行） | 贴底进度条进出动画（0.18s，y:8→0） | `motion/react` | ✅ 可整段搬（AM 完全没有；`app.css:797` 的 `.progress-bar` 是瞬间出现/消失） |
| 7 | `src/design/primitives/useMarquee.ts`（66 行） | 悬停文字跑马灯（CSS 变量 `--marquee-dur`） | 无 | ✅ 可整段搬（AM 无；文件名过长时现在只能 `ellipsis`） |
| 8 | `src/design/primitives/BootErrorBoundary.tsx`（91 行） | 启动异常兜底页 | 无 | ✅ 可整段搬（AM 无 ErrorBoundary，白屏即失联） |
| 9 | `src/lib/dockHover.ts` / `dockOpen.ts` | 双面板悬停互斥 / 同时开一半 | zustand | ✅ AM 已有同名文件（`app/src/lib/dockHover.ts`、`dockOpen.ts`），**核对数值即可** |
| 10 | `app.css:7968-7999` | 全局滚动条（10px 槽 / 4.5px 透明边 = 1px 视觉 / 6px 抓取 / `min-height:36px`） | 无 | ✅ **已对齐**（AM `tokens.css:263-286` 逐值相同）；注意两条硬约定：不许写 `scrollbar-width`、`.scroll-hot` 必须挂滚动容器自己 |
| 11 | `app.css:9553-9606` | 圆角归并表（控件 4px / 菜单 8px / 行 0） | 无 | ✅ 可作 AM 的圆角自检表 |
| 12 | `app.css:8513-8528` + `8556-8566` | 纸墨实色阶 + 弹窗族（12px 圆角 + `--e3` 底 + `--shadow-ambient`） | 无 | ✅ 可搬（见 P0-6） |
| 13 | `tokens.css:106-114` | 三条缓动 + 焦点环几何令牌 | 无 | ✅ 可搬（见 P0-5） |
| 14 | `splash.html` + `src/splash.ts` | 启动页 | 无 React | ⚠️ 搬结构，换图标与文案（§6） |
| 15 | `setup.html` + `src/setup/setup.css` + `src/setup/main.ts` + `src-tauri/src/setup_ui.rs` | 安装 / 卸载界面（7 屏） | Tauri 命令 | ❌ 不能直搬：命令名 `setup_*` 与 AM 的 Rust 侧不同名，且安装逻辑（字体库、运行时下载）完全不适用；**只借鉴版式与文案**（§6） |

**不能搬的**（依赖字体业务）：`FontGrid/FontList/FamilyCard/SpecimenView/DetailPanel/OnlineView/CompareOverlay/ApplyProgress/PackProgress/RuntimeManager/PluginCenter*`、`lib/faceContainers.ts`、`lib/affinity.ts`、`state/store`。

---

## 5. 交互动效

### 5.1 时长与缓动令牌

| 令牌 | FM | AM | 一致 |
|---|---|---|---|
| `--t-micro` | `100ms` | `100ms` | ✅ |
| `--t-quick` | `180ms` | `180ms` | ✅ |
| `--t-state` | `260ms` | `260ms` | ✅ |
| `--t-layout` | `400ms` | `400ms` | ✅ |
| `--t-scene` | `600ms` | `600ms` | ✅ |
| `--ease-out` | `cubic-bezier(.16,1,.3,1)` | **无** | ❌ P0-5 |
| `--ease-in` | `cubic-bezier(.4,0,1,1)` | **无** | ❌ P0-5 |
| `--ease-std` | `cubic-bezier(.4,0,.2,1)` | **无** | ❌ P0-5 |
| 减弱动效（令牌层） | 五档全部 → `1ms`（`tokens.css:373-388`，双轨：`data-motion` + `@media`） | 同（`tokens.css:337-355`） | ✅ |

### 5.2 spring 常量（`design/springs.ts`，两边逐条相同 ✅）

| 常量 | stiffness | damping | mass | 用在哪 |
|---|---|---|---|---|
| `spring` | 300 | 30 | 1 | 默认（面板、卡片） |
| `springSnappy` | 500 | 35 | — | Toast 进场、快速反馈 |
| `springSoft` | 180 | 24 | — | 柔和展开 |
| `springBouncy` | 560 | 24 | — | 强调弹入 |
| `easeTop` | `duration .2` + `[.32,.72,0,1]` | | | 即时开合（不走弹簧） |
| `scrimFade` | `duration .18` | | | 全站遮罩统一 |
| `STAGGER_CAP` / `STAGGER_STEP` | `14` / `0.04` | | | 列表错峰，第 n 项延迟 `min(n,14)*0.04` |

### 5.3 场景对照表（触发场景 → 数值）

| 场景 | FM | AM | 差异 |
|---|---|---|---|
| 应用进场 | `#root{animation:app-arrive 240ms ease-out}`（`app.css:20`） | `#root{animation:app-arrive var(--t-state) ease-out}` = 260ms（`tokens.css:194`） | P2：改 240ms |
| 视图切换滑块 | `.view-pill` `transform var(--t-state) ease`；位移 `calc(var(--i)*(100% + 2px))`（用自身百分比，抗窗口缩放） | `transform var(--t-state) ease`；位移 `calc(var(--i)*36px)`（**写死 36px**） | ❌ P1：AM 在窗口缩放/字号变化时会错位 |
| 侧栏选中竖线 | 无动画（瞬间） | 无动画 | ✅ |
| 列表行 hover | `background/color var(--t-quick) ease` | `background/color var(--t-quick)`（无曲线 → 默认 `ease`） | ⚠️ 等效，但建议补令牌 |
| 按钮按下 | PillToggle 滑块 `scaleX(1.25)`，`transform 110ms` | 无按下反馈 | ❌ P1 |
| 停靠面板升降 | `transition: height 110ms ease`（`app.css` `.tag-dock`），**唯一允许写死毫秒的例外** | `--t-dock:110ms`（`tokens.css:359`）+ `transition 110ms ease` 2 处 | ✅ |
| 弹层进场（设置） | 遮罩 `scrimFade .18` / 面板 motion spring | 遮罩 `set-scrim-in var(--t-quick)` / 面板 `set-sheet-in var(--t-state) cubic-bezier(.32,.72,0,1)` + `translateY(32px)` | ⚠️ P1：曲线 `.32,.72,0,1` 不在 FM 三条曲线里；FM 弹层用 `spring`，不用位移 32px |
| 灯箱 | FM 无灯箱 | `lb-scrim` / `lb-panel` 仅 opacity | ✅（AM 自有业务） |
| 骨架屏 | `.skeleton::after` `shimmer 1.5s infinite`，`linear-gradient(60deg,transparent 30%,rgba(ink,.06) 50%,transparent 70%)`，`translateX(-100%)→100%` | **逐字相同** | ✅ |
| 缩略图淡入 | `opacity var(--t-quick) linear`（FM 卡片图） | `opacity 90ms / 120ms / var(--t-quick)` 三种混用 | ❌ P1：统一到 `--t-quick` |
| 拖拽跟手 | FM 拖拽走原生 OLE，无自定义 | AM 自有（`momentumScroll.ts`） | 不在对齐范围 |
| 滚动惯性 | 原生 | `lib/momentumScroll.ts` 自实现 | 不在对齐范围 |
| Toast 进场 | `springSnappy`：`initial{opacity:0,x:40,y:-10,scale:.94}` → `{1,0,0,1}`，`exit` 反向 | **无动画**（直接挂载） | ❌ **P0-2** |
| 底栏进度条进出 | `BottomBar`：`.18s` `y:8→0` + fade，ease `[.16,1,.3,1]` | 条件渲染，瞬现瞬消 | ❌ P1 |
| 设置项定位高亮 | `.set-row-flash` → `@keyframes setRowFlash`（18% 处 `color-mix(--text-primary 9%)`，1.1s 后移除） | 无 | P2（AM 设置搜索功能待核对） |
| Toast 闪烁提示 | `@keyframes toast-flash` / `toast-flash-error`（box-shadow 从彩色描边淡到底影） | 无 | P2 |
| 窗口关闭 | `@keyframes ui-fade-out` | 无 | P2 |

### 5.4 AM 里的「漏网时长」（绕过减弱动效开关）

`grep` 结果：`var(--t-base, 180ms)` ×3（`--t-base` **在 tokens.css 里根本没定义**，只是靠 fallback 生效）、
`90ms` ×2、`100ms` ×1、`110ms` ×2、`120ms` ×2。全部应换成 `--t-quick` / `--t-micro`，
否则「减少动效」开关关不掉它们（FM 在 `tokens.css:365-428` 专门为这类硬编码列了一张收口名单）。

---

## 6. 启动页 / 安装向导 / 卸载

### 6.1 启动页（splash）

| 项 | FontManager | AssetManager | 一致 |
|---|---|---|---|
| 是否存在 | 有：`splash.html`（355 行，纯静态 + 3KB `src/splash.ts`）+ `src-tauri/src/lib.rs` 建窗（`:3439-3465`） | **无**（`grep splash` 0 命中） | ❌ **P0-7** |
| 窗口 | `1120×760`、`resizable(false)`、`decorations(false)`、`transparent`、`shadow:false`、`always_on_top(true)` | — | — |
| 圆角 | `24px`（注释解释：整卡 ×2 放大，只有圆角不跟着放大） | — | — |
| 底 | 深色 `radial-gradient(120% 100% at 50% -18%, #191c21, #0e0f12 52%, #08080a)`；浅色 `#ffffff → #fdfdff → #f3f5f9` | — | — |
| 光晕 | `640×640` 圆 + `blur(12px)`，深色 `rgba(100,136,255,.26)`、浅色 `rgba(27,54,196,.12)` | — | — |
| 图标 | `.tile` 156×156、`radius 38`、`padding 18`、底 `#fbfaf5`、双层阴影 `0 28px 60px -16px rgba(0,0,0,.62)` + `0 0 0 2px rgba(255,255,255,.14)` | — | — |
| 应用名 | `30px`/600，`margin-top:44px` | — | — |
| 进度条 | `336×4`、`radius 99`、`margin-top:48px`；填充 `linear-gradient(90deg,#3f5be0,#6f8cff)`；数值补间 `width 900ms cubic-bezier(.2,.8,.2,1)`；无分母时 30% 亮块 `slide 2.4s linear infinite` | — | — |
| 状态行 | `21px`、`rgba(255,255,255,.42)`、`tabular-nums`、`min-height:1.6em` | — | — |
| 页脚 | `left/right 44px; bottom 30px`、Consolas `18px`、`letter-spacing .08em`、左版本号右 `IIEOZO` | — | — |
| 退出 | `.out{opacity:0}`，`transition: opacity 120ms linear`；Rust 侧再等 220ms 关窗 | — | — |
| 最短显示 | **1.5s**（`lib.rs:115-120`，用户明确要求「每次打开都先看到启动页，最少 1.5 秒」），从「窗口真正 show 出来」起算 | — | — |
| 主题 | 四段叠：`:root` 深色 → `@media prefers-color-scheme:light` → `[data-theme=dark]` → `[data-theme=light]`；**不写内联 script**（CSP `default-src 'self'` 会拦，白屏） | — | — |

**AM 落地建议（P0-7 最小版）**：先只做「静态壳 + 循环进度条 + 版本号」，不做真实进度
（AM 的扫描进度已有 `progress-bar`，接进去是第二步）。窗口 1120×760 会被 DPI 缩放，
1x 屏幕上偏大 —— 需要 owner 确认（见 §10 Q3）。

### 6.2 安装向导（NSIS）

| 项 | FM（`src-tauri/tauri.conf.json`） | AM | 一致 |
|---|---|---|---|
| 自定义模板 | `"template": "./windows/installer.nsi"`（fork 自 Tauri CLI 2.12.2 内嵌模板，984 行，`tools/nsis-fork-audit.js` 可 diff） | **无**（用 Tauri 默认模板） | ❌ |
| 安装器 / 卸载器图标 | `installerIcon: icons/icon.ico`、`uninstallerIcon: icons/icon.ico` | 未配置（默认取 `bundle.icon`） | ⚠️ P2 |
| 侧栏图 | `sidebarImage: ./windows/sidebar.bmp`（替掉 NSIS 自带蓝色 win.bmp） | 无 | P2 |
| 语言 | `["SimpChinese","English"]`，`displayLanguageSelector:false` | 同 | ✅ |
| 文案集中表 | `windows/zfm-strings.nsh`（`MUI_TEXT_*` 覆盖） | 无 | P2 |
| 安装模式 | 未写（默认 perMachine，可提权） | `"installMode": "currentUser"` | ⚠️ 决策项（§10 Q4） |
| 安装界面（自定义 UI） | 有：`--installer-ui` 起一个 `1000×620`、`decorations:false`、`transparent`、`shadow:false` 的 Tauri 窗口加载 `setup.html`（7 屏：欢迎 / 位置 / 安装中 / 完成 / 卸载确认 / 卸载中 / 卸载完成 + 运行中拦截 + 取消二次确认），独立 WebView2 数据目录放 `%TEMP%\FontManager-setup-webview`，失败自动回落原生 NSIS | **无** | ❌ P1（工作量大） |
| 钩子 | `windows/hooks.nsh`：`PREINSTALL`（杀残留 msedgewebview2）、`POSTINSTALL`（建 `\data` + 写标记文件）、`POSTUNINSTALL`（白名单精确清理） | 无 | ❌ P2（AM 无同类需求） |

### 6.3 卸载

| 项 | FM | AM | 一致 |
|---|---|---|---|
| 数据清理 | 依赖 NSIS 内置「删除应用数据」复选框（`$DeleteAppDataCheckboxState`），钩子里补清：`<安装目录>\data`、`%LOCALAPPDATA%\FontManager\data`、`%APPDATA%\FontManager`、运行时目录、临时文件、**HKCU 字体表里指向本软件字体库的值**；原则是**白名单精确删除 + 先认特征文件**（`state.json` / `font-cache.json` / `.zfm-data-marker`），绝不 `RMDir /r "$INSTDIR"` | **无任何卸载钩子**：用户勾「删除数据」时 Tauri 只清 `%APPDATA%\<identifier>`；AM 的真实数据在用户自选的素材索引位置 + `%APPDATA%\com.assetmanager.desktop`，**索引库不会被清** | ❌ **P1** |
| 覆盖升级保护 | 所有清理都在 `${If} $UpdateMode <> 1` 里，升级不删数据 | 无（默认行为即不删） | ✅ |
| 卸载界面 | 自定义 UI（同 §6.2），明确写「字体库、分组、快照与设置默认保留 —— 重新安装会直接接上」 | 原生 NSIS | ❌ P1 |

**AM 最小闭环（P1，建议下一轮做）**：不搬自定义 UI，只加 `installerHooks` 一个 `.nsh`，
在 `NSIS_HOOK_POSTUNINSTALL` 里按「特征文件」清 AM 的索引目录与缩略图缓存 —— 抄 FM 的
「白名单 + 先认特征文件 + 升级跳过」三原则，别抄它的字体注册表逻辑。

---

## 7. 设计理念（从代码与文档提炼，最多 5 条）

| # | 理念 | 出处 | 所以界面上应该怎么做 |
|---|---|---|---|
| 1 | **单一强调色标「当前」**：全站只有一个蓝 `#5b8cff`，且只用于「当前行 / 当前视图 / 焦点环」；不设红绿语义色 | `docs/FontManager设计统一.md` §1.1「一旦有人加一个成功绿，整套就散了」；`tokens.css:42-44` 三个语义色全是 `#d4d4d8` | 不要新增强调色；危险动作用**文字 + 位置 + 二次确认**表达，不用红。AM 现在有 `--danger:#e05a4a` 且用于关闭按钮 hover、Toast 竖条、删除按钮 → 收敛为灰度 + 文案 |
| 2 | **纸 / 墨平色，不用玻璃**：R4 把「半透明玻璃」整体换成实色阶 `--e1..#e4`，面板之间靠 1px 发丝线分层，不靠阴影 | `app.css:7900-7910`、`8513-8528`，`--e1:#1a1a18` 等四档实色 | 不要给卡片加阴影/毛玻璃；分层用**实色阶 + 1px 线**。AM 的 `--hairline` 不可见 = 这条理念失效，必须先修 P0-1 |
| 3 | **一行文字就是 11px / 500 / 32px 高 / 左右 11px / 直角 / 15px 图标**：主次只靠字重与颜色，不靠字号 | 设计统一 §2「行契约」 | 任何新行（素材行、标签行、分组行）先套这六个数；不要用 12/13px 表达层级 |
| 4 | **动效只有时长令牌 + 三条曲线**，且必须能被「减少动效」压成 1ms；弹簧参数集中一份 | `tokens.css:92-108` 的长注释；`springs.ts` | 不写裸毫秒；新过渡一律 `var(--t-quick) var(--ease-std)` 这种组合；`prefers-reduced-motion` 与软件内开关都要能停 |
| 5 | **数字等宽、贴右；空状态说「怎么办」而不是「暂无数据」** | 设计统一 §10；`.tabular{font-variant-numeric:tabular-nums}` | 所有计数/尺寸/百分比挂 `.tabular` 并直接写在数字元素上（不要只靠调用点记得加类）；空态写下一步动作 |

---

## 8. 对齐清单

### P0 — 视觉一眼看出不是一家（先做这 7 条）

| # | 文件 / 位置 | 从（AM 现状） | 改成（FM 值） | 工作量 |
|---|---|---|---|---|
| P0-1 | `app/src/styles/tokens.css:60` 与 `:158` | `--hairline: rgba(20,20,18,.06)`（两套主题同值） | 深色 `rgba(255,255,255,.05)`；浅色保持 `rgba(20,20,18,.06)` | 2 行 |
| P0-2 | `app/src/components/ui.tsx:478-496` + `app/src/styles/app.css:1210-1263` | 静态 `div.toast`，无动画/图标/动作 | 搬 `Toast.tsx` 结构 + `springSnappy` 进场 + `glass-e3` + 16px 图标 + 标题 `--fs-13`/600 + `gap:8px` + `z-index:70` + TTL 4000/6500/7000 + 上限 4 条 | 中 |
| P0-3 | `app.css:221-264` `.topbar-size` | 轨道 2px `--ui-line`；滑块 8px 蓝点；无 hover 缩放 | FM `app.css:629-670`：轨道 4px + `linear-gradient(90deg,--accent-a,--accent-b)` 填充、滑块 15px `#f5f5f7` + `0 0 0 1px rgba(0,0,0,.25)` + `0 0 10px --accent-glow-soft`、hover `scale(1.15)`、active `scale(1.05)` | 小（但要先做 P1-a 才有 `--accent-a/-b`） |
| P0-4 | `tokens.css:209` `body` | `font-size: var(--fs-11)` | `font-size: var(--fs-14)` | 1 行（改完需扫一遍未显式设字号的地方） |
| P0-5 | `tokens.css`（新增） | 无缓动令牌，全站 `transition` 省略曲线 | 新增 `--ease-out/in/std` 三条（`tokens.css:106-108`），并替换全站 transition 的曲线 | 中 |
| P0-6 | `app.css:1632-1636` `.set-sheet` | `background: var(--canvas)`、`border-radius:10px`、`box-shadow: var(--shadow-mass)` | `background: var(--e3)`、`border-radius:12px`、`box-shadow: var(--shadow-ambient)` | 3 行 |
| P0-7 | 新建 `splash.html` + `src-tauri` 建窗 | 无启动页 | 照 §6.1（最小版：静态壳 + 循环条 + 版本号 + 最短 1.5s） | 大 |

### P1 — 细节不一致（一轮内做完）

| # | 文件 / 位置 | 从 | 改成 |
|---|---|---|---|
| P1-a | `tokens.css:61-64` | `--accent-a: rgba(91,140,255,.16)`；无 `--accent-b/-gradient` | 新增 `--accent-a:#d4d4d8`、`--accent-b:#a1a1aa`、`--accent-gradient:linear-gradient(135deg,#d4d4d8,#a1a1aa)`、`--accent-glow:rgba(212,212,216,.45)`、`--accent-glow-soft:rgba(212,212,216,.22)`；原 16% 蓝改名为 `--accent-soft`（20 余处调用点跟着改名） |
| P1-b | `tokens.css` | 无 `--focus-ring` | 加 `--focus-ring: var(--accent-a)`、`--focus-ring-w:2px`、`--focus-ring-offset:2px`；`app.css:253` 的 `outline:2px solid var(--accent-a)`（16% 蓝，几乎不可见）改为 `var(--focus-ring)` |
| P1-c | `app.css:149-171` `.topbar-field` | `--r-4` / `var(--hairline)` / 聚焦 `box-shadow:0 0 0 3px var(--accent-a)` | `border-radius:2px` / `border:1px solid var(--ui-line)` / 聚焦 `border-color:var(--ui-accent); box-shadow:none` |
| P1-d | `app.css:610-613` `.viewport` | `padding:14px 16px 22px` | `padding:18px 20px` |
| P1-e | `app.css:917-918, 957-960` `.detail` | `rgba(ink,.02)`，无模糊；内距 `18px 16px 22px`、`gap:16px`；滚动条可见 | `rgba(ink,.025)` + `backdrop-filter:blur(32px) saturate(140%) brightness(1.05)`；内距 `20px 18px`、`gap:18px`；滚动条隐藏（`scrollbar-width:none` + `::-webkit-scrollbar{width:0}`，照 `app.css` FM `.detail-inner`） |
| P1-f | `app.css:986` `.detail-title` | `font-size:15px` | `font-size: var(--fs-20)`，`line-height:1.15` |
| P1-g | `app.css:309` `.view-pill` | `transform: translateX(calc(var(--i)*36px))` | `translateX(calc(var(--i) * (100% + 2px)))` + `width: calc((100% - 10px)/3)`（照 FM `app.css:1026-1040`） |
| P1-h | 新增 | 无底栏 | 搬 `BottomBar.tsx`；底栏样式照 `.warm-footer`（`padding:6px 16px 8px`、`border-top:rgba(ink,.05)`、`--fs-10`/500、`--text-tertiary`） |
| P1-i | 新增 | 无 `useMarquee` | 搬 `useMarquee.ts`，用于详情标题/文件名一行过长时 |
| P1-j | 新增 | 无 ErrorBoundary | 搬 `BootErrorBoundary.tsx` 到 `main.tsx` |
| P1-k | `tokens.css` + `app.css` 6 处 | `var(--t-base,180ms)` / `90ms` / `100ms` / `120ms` / `110ms` 硬编码 | 全部换 `--t-quick` / `--t-micro`（`110ms` 的两处保留，是停靠面板例外） |
| P1-l | `app.css:114` `.tc-close:hover` | `rgba(224,90,74,.18)` + `var(--danger)` | FM 是 `color-mix(in srgb, var(--error) 18%, transparent)`，而 `--error` 是灰度 → AM 改 `rgba(var(--ink-rgb),.08)` + `--text-primary`（或保留红色并接受「不一家」，见 §10 Q1） |
| P1-m | 新增 `src-tauri/windows/hooks.nsh` | 无卸载清理 | `NSIS_HOOK_POSTUNINSTALL` 白名单清理 AM 索引/缩略图缓存，`${If} $UpdateMode <> 1` 包裹 |
| P1-n | `app.css:1632` 附近 `.set-scrim` | `var(--scrim)` = `rgba(0,0,0,.6)` | `rgba(0,0,0,.46)`（浅色 `rgba(20,20,18,.24)` 已对） |

### P2 — 锦上添花

| # | 位置 | 内容 |
|---|---|---|
| P2-1 | `tokens.css` | `--shadow-mass` / `--shadow-ambient` 对齐 FM 值（`0 6px 12px/…30%`、`0 24px 48px/…25%`） |
| P2-2 | `tokens.css:193-195` | `#root` 进场 260ms → `240ms` |
| P2-3 | `tokens.css` | 补 `:root[lang="ja"]` / `:root[lang="zh-TW"]` 字族覆盖 |
| P2-4 | `app.css` `.tree-row.sel::before` | 删 `border-radius: var(--r-anchor)`（FM 竖线是直角） |
| P2-5 | `app.css:5-18` `.app-frame` | 补 `::before`（双 radial 光晕 `rgba(212,212,216,.1)/.08`）与 `::after`（SVG 噪点 `opacity .35`） |
| P2-6 | 新增 | `@keyframes toast-flash` / `setRowFlash` / `ui-fade-out` |
| P2-7 | `.detail-artmeta` | `--fs-10` → `--fs-11` + `tabular-nums` |
| P2-8 | 图标（本轮不动） | 见 §9.3 记录 |

---

## 9. 可直接复制的代码 / 样式片段（原样贴，标注来源）

### 9.1 缓动与焦点环令牌 —— `FontManager/src/design/tokens.css:106-114`

```css
  --ease-out: cubic-bezier(0.16, 1, 0.3, 1);
  --ease-in: cubic-bezier(0.4, 0, 1, 1);
  --ease-std: cubic-bezier(0.4, 0, 0.2, 1);

  --focus-ring-w: 2px;
  --focus-ring-offset: 2px;
```

```css
  --focus-ring: var(--accent-a);   /* tokens.css:40 */
```

### 9.2 弹窗族（12px 圆角 + `--e3` 底）—— `FontManager/src/app.css:8556-8567`

```css
.settings-panel,
.shortcuts-panel,
.palette-panel,
.compare-panel {
  border-radius: 12px;
  border-color: var(--ui-line);
  box-shadow: var(--shadow-ambient);
  background: var(--e3);
}
```

### 9.3 全局滚动条 —— `FontManager/src/app.css:7989-8006`（AM 已对齐，留作文档基线）

```css
*::-webkit-scrollbar { width: 10px; height: 10px; }
*::-webkit-scrollbar-track { background: transparent; }
*::-webkit-scrollbar-corner { background: transparent; }
*::-webkit-scrollbar-thumb {
  background: var(--ui-fg-3);
  background-clip: padding-box;
  border: 4.5px solid transparent;
  border-radius: 999px;
  min-height: 36px;
}
.scroll-hot::-webkit-scrollbar-thumb,
*::-webkit-scrollbar-thumb:hover,
*::-webkit-scrollbar-thumb:active {
  border-width: 2px;
  background-color: color-mix(in srgb, var(--ui-fg) 40%, transparent);
  background-clip: padding-box;
}
```

### 9.4 字号滑杆 —— `FontManager/src/app.css:629-670`

```css
.topbar-size input[type="range"] {
  appearance: none;
  -webkit-appearance: none;
  width: clamp(72px, 10vw, 128px);
  height: 4px;
  border-radius: 2px;
  background:
    linear-gradient(90deg, var(--accent-a), var(--accent-b)) 0 0 / var(--fill) 100%
      no-repeat,
    var(--e2);
  outline: none;
  cursor: pointer;
}
.topbar-size input[type="range"]::-webkit-slider-thumb {
  appearance: none;
  -webkit-appearance: none;
  width: 15px;
  height: 15px;
  border-radius: 50%;
  background: #f5f5f7;
  border: none;
  box-shadow:
    0 0 0 1px rgba(0, 0, 0, 0.25),
    0 0 10px var(--accent-glow-soft),
    var(--shadow-ground);
  transition: transform var(--t-micro) ease, box-shadow var(--t-quick) ease;
}
.topbar-size input[type="range"]::-webkit-slider-thumb:hover {
  transform: scale(1.15);
  box-shadow:
    0 0 0 1px rgba(0, 0, 0, 0.25),
    0 0 16px var(--accent-glow),
    var(--shadow-ground);
}
.topbar-size input[type="range"]::-webkit-slider-thumb:active {
  transform: scale(1.05);
}
```

### 9.5 贴底条进出动画 —— `FontManager/src/design/primitives/BottomBar.tsx`（全文）

```tsx
import { AnimatePresence, motion } from "motion/react";
import type { ReactNode } from "react";

export function BottomBar({ show, className, hint, children }: {
  show: boolean;
  className?: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <AnimatePresence>
      {show && (
        <motion.div
          className={className}
          role="status"
          aria-live="polite"
          data-hint={hint}
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: 8 }}
          transition={{ duration: 0.18, ease: [0.16, 1, 0.3, 1] }}
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
```

### 9.6 骨架屏 —— `FontManager/src/design/tokens.css:327-350`（与 AM 逐字相同，留作回归基线）

```css
.skeleton {
  position: relative;
  overflow: hidden;
  background: var(--e1);
  border-radius: 5px;
}
.skeleton::after {
  content: "";
  position: absolute;
  inset: 0;
  background: linear-gradient(
    60deg,
    transparent 30%,
    rgba(var(--ink-rgb), 0.06) 50%,
    transparent 70%
  );
  transform: translateX(-100%);
  animation: shimmer 1.5s infinite;
}
@keyframes shimmer { to { transform: translateX(100%); } }
```

### 9.7 视图切换滑块几何 —— `FontManager/src/app.css:1026-1040`

```css
.view-pill {
  position: absolute;
  top: 3px;
  bottom: 3px;
  left: 3px;
  width: calc((100% - 10px) / 3);   /* 3 = 按钮数，改按钮数要同步 */
  z-index: 0;
  pointer-events: none;
  border-radius: 2px;
  background: var(--e3);
  box-shadow:
    inset 0 1px 0 var(--glass-top-highlight),
    0 0 10px var(--accent-glow-soft),
    var(--shadow-ground);
  transform: translateX(calc(var(--i, 0) * (100% + 2px)));
  transition: transform var(--t-state) ease;
}
```

### 9.8 图标来源记录（本轮不改，留后续）

| 项 | FontManager | AssetManager |
|---|---|---|
| 界面图标库 | `lucide-react`，`strokeWidth` 默认 1.5 | `lucide-react` 1.24.0（`ui.tsx` 注释：同版本、同 24 格、同 1.5） |
| 界面图标尺寸 | 侧栏/行 `15`、菜单项 `11`、顶栏 `13`、标题栏动作 `?`（`Titlebar.tsx` 用 16 格自绘 `SettingsGlyph`） | 侧栏 `15`、菜单 `11`、顶栏 `13`、标题栏动作 `14`（`Chrome.tsx:74-86`）、折叠箭头 `10` |
| 设置图标 | 自绘「滑杆」glyph（16 格） | **逐字节复制** FM 的 `SettingsGlyph`（`ui.tsx`） |
| 应用图标 | 「拼贴纸片风」**5 套**（主图标 / 快照 / 导出包 / 仅数据 / 库文件夹），每套 `.ico` **7 帧**（16/24/32/48/64/128/256），见 `docs/图标规范.md` | `src-tauri/icons/`：`icon.ico`、`32x32`、`64x64`、`128x128`、`128x128@2x`、`icon.png`；另有 `asset3d.ico`/`broken.ico`/`library.ico`/`pack.ico`；`app/public/` 只有 `icon.png`。**`.ico` 帧数未核实** |

---

## 10. 待 owner 决策（5 条）

| # | 问题 | 选项 | 影响 |
|---|---|---|---|
| Q1 | **红色语义色要不要留？** FM 刻意零语义色（`--success/--warning/--error` 全 `#d4d4d8`），危险动作只靠文案 + 位置 + 二次确认；AM 现在有 `--danger:#e05a4a`，用在关闭按钮 hover、Toast 错误竖条、「删除」按钮 | A 跟 FM 全去红，`--danger` 映射到 `--text-primary`；B 保留红（更符合素材管理「删除即毁文件」的心理） | 选 A 才真正「一家」；选 B 要接受一个长期例外，并写进 AM 的设计规范 |
| Q2 | **`--accent-a` 改名要不要做？** FM 该令牌是实色浅灰（137 处），AM 是 16% 蓝 | A 改名 `--accent-soft` 并补齐 FM 三个 accent 令牌（一次性代价：20 余处调用点）；B 不改名，以后拒绝复制任何用到 `--accent-a` 的 FM 片段 | 选 A 是「可长期复用」的前提 |
| Q3 | **启动页窗口尺寸**：FM 是 1120×760（2.1.7 等比放大一倍）。1x DPI 屏幕上这扇窗口偏大，且 AM 的启动比 FM 快（无字体种子装载） | A 原尺寸照搬；B 缩到 800×560 等比；C 只做「主窗口内遮罩式启动页」，不另开窗口 | 直接决定 P0-7 的实现方式 |
| Q4 | **安装模式**：FM 默认 perMachine（可写 `Program Files`，数据落在安装目录或回退 `%LOCALAPPDATA%`）；AM 是 `currentUser` | A 保持 `currentUser`（无需提权，更新更简单）；B 改 perMachine（更像「正经软件」） | 影响安装器 UI 与数据目录策略；不建议本轮改 |
| Q5 | **安装器 / 卸载器自定义 UI 要不要做？** FM 有 7 屏自定义界面（1000×620，`setup.html` 1401 行 + `setup_ui.rs`） | A 本轮只做卸载钩子，UI 保持原生；B 完整搬 7 屏；C 只做卸载确认页 | 工作量差异最大的一项（B 约 2–3 人日） |

---

## 11. 覆盖度自检

| 块 | 数据完整度 | 说明 |
|---|---|---|
| 1 字体与字号 | **完整** | 两边 tokens.css 逐变量比对；字阶分配取自 FM 设计统一 §1.2；AM 的裸字号已逐条 grep |
| 2 颜色体系 | **完整** | 深/浅两套逐令牌比对；已标注 FM 的「tokens.css 残留 vs app.css 覆盖」陷阱 |
| 3 板块配色 | **完整（1 处待核）** | 五块面板 + 弹层 + 行契约均有类名与数值；**设置面板左列 `.set-nav` 的内距（FM `16px 10px 14px`）未与 AM 逐值核对**，因为不影响一眼观感 |
| 4 可直接复用 | **较完整** | 15 条已判定；`ContextMenu.tsx`（748 行）只核到「AM 有 submenu」，未逐项 diff 键盘/二级菜单行为 |
| 5 交互动效 | **较完整** | 时长/曲线/spring/16 个场景均有值；**FM 的 motion 组件（`motion.div` 的 initial/animate 参数）散落在 40 余个 tsx 里，本轮只抽了 Toast / BottomBar / 弹层三处**，其余以 CSS 过渡为主 |
| 6 启动页 / 安装 / 卸载 | **完整（启动页动画细节略）** | splash.html 的尺寸、色值、时序、最短显示均已抄全；`setup.html` 只抽了屏结构与窗口参数，**7 屏各自的像素级样式（`src/setup/setup.css` 733 行）未逐条展开** —— 若 Q5 选 B，需要单独一轮细读 |
| 7 设计理念 | **完整** | 5 条均可落回具体文件行号 |

**未取到 / 无法取到的**：
- AM 的 `.ico` 帧数（需二进制解析，本轮按「图标后改」跳过）。
- FM 的 `Titlebar.tsx` 动作按钮图标尺寸（未读到具体 `size=`，只知设置项是 16 格 glyph）。
- FM 截图 `main.png` 为浅色主题，深色观感以 `dark` 令牌值为准（未找到深色主界面截图）。
