//! 原生截图覆盖层（0.3.0「A 方案」）
//!
//! **完全不经过 WebView2**。为什么要重写：新建一个 WebView2 窗口从"可见"到"交出第一帧"
//! 本身要 ~600ms（0.2.3/0.2.4 实测，alpha 隐身 / disable-gpu / 关遮挡计算都改不动它），
//! 而 Snipaste 这类工具是按下快捷键就能框选、工具栏随即就在。要做到那个手感，只能自己画。
//!
//! 这一层负责：冻结画面 → 拖框 / 移动 / 拉伸 → 压暗 → 工具栏 → 各种标注 → 撤销重做。
//! 交付（复制 / 保存 / 贴图）不在这里做，只把"裁好的 PNG + 动作"还给调用方，
//! 由 `main.rs` 复用原来那套已跑通的交付链路（剪贴板 / 截图库 / 贴图窗口）。
//!
//! 约定：
//! - 坐标一律**物理像素**（窗口就是按虚拟桌面尺寸建的，客户区坐标即物理像素）；
//! - 所有绘制都直接画进那块 DIB（`mem` DC 选着它），最后一次 `UpdateLayeredWindow` 上屏；
//! - 每帧全量重绘（整屏 memcpy + 重放标注），2560×1440 实测每帧 4~8ms，
//!   拖框时够 60fps，而且撤销/重做只是改 `ops` 再重绘，不需要任何增量状态。
//! - **颜色一律从 `Ctx::pal` 取**（深/浅两套 `Palette`，由 `main.rs` 的 `UI_THEME` 传进来）。
//!   绘制函数里写死色值 = 另一个主题下必然错一处，且离屏预览只能测到一个主题。

use std::ffi::c_void;
use std::sync::Mutex;

use crate::screen;

type Hwnd = isize;
type Hdc = isize;
type Hgdiobj = isize;
type Hfont = isize;

const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const ULW_ALPHA: u32 = 0x0000_0002;
const AC_SRC_OVER: u8 = 0x00;
const AC_SRC_ALPHA: u8 = 0x01;
const SW_SHOW: i32 = 5;
const TRANSPARENT: i32 = 1;const DT_CENTER: u32 = 0x0000_0001;
const DT_VCENTER: u32 = 0x0000_0004;
const DT_SINGLELINE: u32 = 0x0000_0020;
const DT_CALCRECT: u32 = 0x0000_0400; // 只量尺寸不画（气泡/提示面板靠它算宽）
const FW_NORMAL: i32 = 400;
const FW_BOLD: i32 = 700;

const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000F;
const WM_KEYDOWN: u32 = 0x0100;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_TIMER: u32 = 0x0113;

#[repr(C)]
#[derive(Default)]
struct RectPos {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_xppm: i32,
    bi_yppm: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[repr(C)]
#[derive(Default)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 3],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct BlendFunction {
    blend_op: u8,
    blend_flags: u8,
    source_constant_alpha: u8,
    alpha_format: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PointI {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Size {
    cx: i32,
    cy: i32,
}

type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassW {
    style: u32,
    wnd_proc: Option<WndProc>,
    cls_extra: i32,
    wnd_extra: i32,
    hinstance: isize,
    hicon: isize,
    hcursor: isize,
    hbr_background: isize,
    menu_name: *const u16,
    class_name: *const u16,
}

extern "system" {
    fn RegisterClassW(c: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: isize,
        menu: isize,
        inst: isize,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
    fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
    fn DestroyWindow(h: Hwnd) -> i32;
    fn GetMessageW(m: *mut Msg, h: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(m: *const Msg) -> i32;
    fn DispatchMessageW(m: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn GetDC(h: Hwnd) -> Hdc;
    fn ReleaseDC(h: Hwnd, dc: Hdc) -> i32;
    fn GetModuleHandleW(name: *const u16) -> isize;
    fn UpdateLayeredWindow(
        h: Hwnd,
        dst: Hdc,
        dst_pos: *const Point,
        size: *const Size,
        src: Hdc,
        src_pos: *const Point,
        key: u32,
        blend: *const BlendFunction,
        flags: u32,
    ) -> i32;
    fn SetCapture(h: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
    fn SetForegroundWindow(h: Hwnd) -> i32;
    fn SetFocus(h: Hwnd) -> Hwnd;
    fn GetKeyState(vk: i32) -> i16;
    fn LoadCursorW(inst: isize, name: *const u16) -> isize;
    fn GetAsyncKeyState(vk: i32) -> i16;
    fn SendMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
    fn SetTimer(h: Hwnd, id: usize, ms: u32, f: usize) -> usize;
    fn KillTimer(h: Hwnd, id: usize) -> i32;
    fn ScreenToClient(h: Hwnd, p: *mut PointI) -> i32;
    /* ---- 窗口识别（0.5.x 取色器 + 窗口截图） ----
       覆盖层是整屏 topmost 的，`WindowFromPoint` 只会命中它自己；
       所以拿到本进程的句柄之后还要沿 z 序往下走（见 `pick_window_hwnd`）。 */
    fn WindowFromPoint(p: PointI) -> Hwnd;
    fn GetAncestor(h: Hwnd, flags: u32) -> Hwnd;
    fn GetTopWindow(h: Hwnd) -> Hwnd;
    fn GetWindow(h: Hwnd, cmd: u32) -> Hwnd;
    fn IsWindowVisible(h: Hwnd) -> i32;
    fn IsIconic(h: Hwnd) -> i32;
    fn GetWindowThreadProcessId(h: Hwnd, pid: *mut u32) -> u32;
    fn GetClassNameW(h: Hwnd, buf: *mut u16, n: i32) -> i32;
    fn GetCursorPos(p: *mut PointI) -> i32;
}

/* DWM 真实边框（含阴影外的可见边界）。`GetWindowRect` 会把不可见的 8px
   resize 边框也算进去，截出来四周多一圈桌面 —— 取色/截窗都该用 DWM 这一版。 */
#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetWindowAttribute(h: Hwnd, attr: u32, out: *mut c_void, size: u32) -> i32;
}

extern "system" {
    fn CreateCompatibleDC(dc: Hdc) -> Hdc;
    fn DeleteDC(dc: Hdc) -> i32;
    fn CreateDIBSection(
        dc: Hdc,
        bi: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: isize,
        offset: u32,
    ) -> isize;
    fn SelectObject(dc: Hdc, o: Hgdiobj) -> Hgdiobj;
    fn DeleteObject(o: Hgdiobj) -> i32;
    fn SetBkMode(dc: Hdc, mode: i32) -> i32;
    fn SetTextColor(dc: Hdc, c: u32) -> u32;
    fn CreateFontW(
        h: i32,
        w: i32,
        esc: i32,
        ori: i32,
        weight: i32,
        italic: u32,
        ul: u32,
        so: u32,
        cs: u32,
        op: u32,
        cp: u32,
        q: u32,
        pf: u32,
        face: *const u16,
    ) -> Hfont;
    fn DrawTextW(dc: Hdc, s: *const u16, n: i32, r: *mut RectPos, fmt: u32) -> i32;
    fn GetWindowTextW(h: Hwnd, buf: *mut u16, n: i32) -> i32;
    fn GetWindowTextLengthW(h: Hwnd) -> i32;
}

#[repr(C)]
struct Msg {
    hwnd: isize,
    message: u32,
    w: usize,
    l: isize,
    time: u32,
    pt: Point,
}

/* ---------------- 模型 ---------------- */

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Pen,
    Marker,
    Mosaic,
    /// 高斯模糊（Snipaste 的那个）
    Blur,
    Text,
    /// 序号标记（1、2、3…点一下就放一个）
    Number,
    Eraser,
    /* ---- 0.4.19 起补齐参考稿 `screenshot-toolbar-v2` 的五个工具 ---- */
    /// 引线气泡：对话泡泡 + 引线尖，可在里面写字
    Callout,
    /// 色块遮挡：整块实心盖住，最直接的"藏信息"
    Block,
    /// 贴纸：点击落一个图案（先用内置几种表情/符号）
    Sticker,
    /// 勾选图章
    Check,
    /// 叉号图章
    Cross,
    /// 插入图片：选一张本地图贴到选区内
    Image,
}

/* ---------------- 工具栏模型（v2：分组 + 悬停托盘） ----------------

   2026-11-21 owner 拿 `screenshot-toolbar-v2/` 的方案要求改成这一版：
   同构工具收进组，悬停 110ms 展开托盘、移开 190ms 收起；属性（颜色/粗细/填充）
   常驻成胶囊。0.3.8 那版是**平铺 19 项 + 第二行选项条**（当时为了"像 Snipaste"），
   这一版把平铺换成分组，第二行整体删掉。

   0.3.1–0.3.5 那三次"面板展不开"的根因是：面板的打开状态与命中区域各写一份，
   且没有超时兜底。这次的做法：
   ① 打开/收起只由 `hover_due`（截止时间 + 目标下标）一个状态机驱动，
      `WM_TIMER` 里统一落地，鼠标移动只负责"改截止时间"；
   ② 托盘矩形 `self.tray` 由 `tray_layout` 算一次，绘制与命中都读它；
   ③ 命中顺序固定：调色板 → 托盘 → 主条 → 选区把手 → 选区内。 */

#[derive(Clone, Copy, PartialEq, Debug)]
enum BarKind {
    Tool(Tool),
    /// 折叠组，下标指向 `GROUPS`
    Group(usize),
    Chip(Chip),
    Sep,
    Act(&'static str),
}

/// 属性胶囊：颜色 / 粗细 / 填充
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chip {
    Color,
    Width,
    Fill,
}

const G_SHAPE: [Tool; 4] = [Tool::Rect, Tool::Ellipse, Tool::Line, Tool::Arrow];
const G_BRUSH: [Tool; 2] = [Tool::Pen, Tool::Marker];
const G_TEXT: [Tool; 3] = [Tool::Text, Tool::Number, Tool::Callout];
const G_MASK: [Tool; 3] = [Tool::Mosaic, Tool::Blur, Tool::Block];
const G_STICKER: [Tool; 4] = [Tool::Sticker, Tool::Check, Tool::Cross, Tool::Image];
const GROUPS: [&[Tool]; 5] = [&G_SHAPE, &G_BRUSH, &G_TEXT, &G_MASK, &G_STICKER];
/// 每组收起时默认显示的工具（用户换过之后记在 `Ctx::face` 里）
const GROUP_DEFAULT: [Tool; 5] = [
    Tool::Rect,
    Tool::Pen,
    Tool::Text,
    Tool::Mosaic,
    Tool::Sticker,
];
const GROUP_NAMES: [&str; 5] = ["形状", "画笔", "文字标注", "遮挡 / 打码", "素材"];

const BAR: [BarKind; 17] = [
    BarKind::Tool(Tool::Select),
    BarKind::Sep,
    BarKind::Group(0),
    BarKind::Group(1),
    BarKind::Group(2),
    BarKind::Group(3),
    BarKind::Group(4),
    BarKind::Sep,
    BarKind::Chip(Chip::Color),
    BarKind::Chip(Chip::Width),
    BarKind::Chip(Chip::Fill),
    BarKind::Sep,
    BarKind::Act("undo"),
    BarKind::Act("redo"),
    BarKind::Sep,
    /* 分裂按钮的左半边（照原型 §6.5）：点击 = 直接复制 */
    BarKind::Act("copy"),
    /* 右半边 22px：展开"另存为 / 贴图 / OCR / 分享" */
    BarKind::Act("copymore"),
];

/// `[复制|▾]` 的右段展开项（原型 §6.5）：(文案, 键位, 交付动作)
const MENU_ITEMS: [(&str, &str, &str, &str); 3] = [
    ("复制到剪贴板", "Ctrl+C", "copy", "copy"),
    ("另存为 PNG", "Ctrl+S", "saveas", "save"),
    ("贴到桌面", "Ctrl+P", "peek", "pin"),
];

/// 菜单条目高度 / 条目间距（原型 `.split__tray .menu-item{height:32px}` + `gap:1px`）
const MENU_ITEM_H: i32 = 32;
const MENU_GAP: i32 = 1;

/// 「取消」固定放最末尾：它和三个交付动作语义相反，而覆盖层是全屏的 ——
/// 挨着放时手一偏就是"白框一次、什么都没留下"。
const BAR_TAIL: BarKind = BarKind::Act("cancel");

/* ---------------- 调色板（21 色 7×3，取自设计稿 §6.4） ----------------

   分行语义：中性（白→黑）/ 暖色警示 / 冷色与强调；蓝这一列用软件强调色。 */
const SWATCHES: [u32; 21] = [
    0xFFFFFF, 0xF5F7FA, 0xC9CFDA, 0x8A93A5, 0x4B5464, 0x232936, 0x000000, 0xFF4D4F,
    0xFF7A45, 0xFFB020, 0xFFE14D, 0xA0E64B, 0x34D399, 0x22D3EE, 0xA78BFA, 0x6C3FD1,
    0x7C6BFF, 0xC084FC, 0xF472B6, 0xFB7185, 0x14B8A6,
];
/// 线宽范围（滚轮 / `[` `]` 连续调，单位物理像素）
const W_MIN: i32 = 1;
const W_MAX: i32 = 24;

/// 调色板块的几何（绘制与命中共用）
const SW_COLS: i32 = 7;
const SW_CELL: i32 = 22;
const SW_GAP: i32 = 5;
const SW_PAD: i32 = 10;

/// 悬停展开 / 收起的延迟（毫秒，设计稿 §5.1）
const OPEN_MS: u64 = 110;
const CLOSE_MS: u64 = 190;


/* ---------------- 工具栏配色（v2.1 设计稿：深浅双主题） ----------------

   为什么必须收成结构化的 `Palette`，而不是几个散落的 const：

   ① 覆盖层浮在**任意**内容之上 —— 深色面板落在深色代码/深色桌面上会糊掉，
      白面板落在白底文档上同样会糊掉。所以边界策略按主题**反转**：
      深色靠亮边（`#3a3a37`）勾轮廓，浅色靠深外沿（`#b9b9b0`）+ 柔影把白面板拽出来；
   ② 工具栏与左下角快捷键面板必须是同一套色 —— 两处各写一份常量，迟早有一处忘改。

   色值全部取自主界面 `tokens.css` 的纸墨体系，不引入新色（设计稿 §04 / §06）。
   像素直绘没有 alpha 合成层，所以"选中底"直接取 accent 实色，不做半透明叠色。 */
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Palette {
    /// 面板底
    bg: u32,
    /// 面板内描边：深色里当"亮边"，浅色里比外沿浅一档
    edge: u32,
    /// 最外 1px 硬轮廓：深色纯黑、浅色中灰
    edge_out: u32,
    /// 图标墨色（比 `--ui-fg` 柔和一档）
    icon: u32,
    sep: u32,
    hover: u32,
    /// 强调色实色（分裂按钮底、调色板选中环）
    active: u32,
    /// 强调色（选中工具的图标色 / 16% 底色 / 34% 内环）
    accent: u32,
    danger: u32,
    /// 键帽底 / 描边
    key_bg: u32,
    key_edge: u32,
    /// 第二级文字（说明 / 气泡里的键名）
    text2: u32,
    /// 第三级文字（次级说明）
    text3: u32,
}

impl Palette {
    const fn dark() -> Self {
        Self {
            bg: 0x1C1C1A,
            edge: 0x3A3A37,
            edge_out: 0x000000,
            icon: 0xE6E6E2,
            /* 比 bg 亮三档：第一版用 #333330，离屏预览里几乎看不见 */
            sep: 0x3F3F3B,
            hover: 0x2A2A27,
            active: 0x8B5CF6,
            accent: 0xA78BFA,
            danger: 0xE05A4A,
            key_bg: 0x2E2E2B,
            key_edge: 0x4A4A46,
            text2: 0xA0A09A,
            text3: 0x6F6F69,
        }
    }

    const fn light() -> Self {
        Self {
            bg: 0xFFFFFF,
            edge: 0xE2E2DD,
            edge_out: 0xB9B9B0,
            icon: 0x3A3A37,
            sep: 0xE2E2DD,
            hover: 0xEAEAE5,
            active: 0x8B5CF6,
            accent: 0x6C3FD1,
            danger: 0xC0392B,
            key_bg: 0xF1F1ED,
            key_edge: 0xD4D4CC,
            text2: 0x6B6B64,
            text3: 0x96968F,
        }
    }

    /// 0=未知 / 1=浅 / 2=深（`main.rs::UI_THEME` 的口径）。未知按出厂深色走。
    const fn from_code(code: u8) -> Self {
        if code == 1 {
            Self::light()
        } else {
            Self::dark()
        }
    }
}

/* ---------------- 工具栏尺寸（对齐 Snipaste 2.11.3 实测 + 本软件自己的圆角语言） ----------------

   owner 2026-11-18：按钮更大、图标更细 → 按钮 36×34、条高 40、图标框 22、线宽 1.8。
   **圆角不跟 Snipaste，跟本软件**：主窗口与停靠块用 `--qb-radius: 8px`，控件用 4px，
   所以外框取 8、按钮取 4 —— 截图浮层和主界面是同一套圆角语言，才不像外挂控件。

   **`BTN_W` 是绘制与命中共用的步长**（`bar_item_x` / `panel_layout` / `panel_hit`
   都靠它算），改的时候必须一起改：只改绘制不改命中，用户看到的就是"点上去没反应"，
   而这类偏差在离屏预览里完全看不出来。 */
const BTN_W: i32 = 36;
const SEP_W: i32 = 13;
const BAR_H: i32 = 56;
const BAR_RADIUS: i32 = 14;
const BTN_RADIUS: i32 = 9;
/// 分裂按钮圆角（原型 `.split{border-radius:10px}`）
const SPLIT_RADIUS: i32 = 10;

/// 相邻两项之间的间距 —— 照原型三条规则叠出来的：
/// `.toolbar{gap:2px}`（全局）/ `.stack+.stack{margin-left:6px}`（组间额外 6）/ `.prop{gap:4px}`（属性组内 4）
fn bar_gap(prev: BarKind, cur: BarKind) -> i32 {
    if matches!(prev, BarKind::Chip(_)) && matches!(cur, BarKind::Chip(_)) {
        return 4;
    }
    if matches!(prev, BarKind::Group(_)) && matches!(cur, BarKind::Group(_)) {
        return 8;
    }
    2
}

fn bar_w(k: BarKind) -> i32 {
    match k {
        BarKind::Sep => SEP_W,
        /* 颜色 / 粗细是窄胶囊（32），填充用整格 */
        BarKind::Chip(Chip::Color) | BarKind::Chip(Chip::Width) => 32,
        /* 复制分裂按钮：左段 66（图标 + "复制"两字），右段 22（原型 §6.5） */
        BarKind::Act("copy") => 67,
        BarKind::Act("copymore") => 22,
        _ => BTN_W,
    }
}

/// 单个工具的名字与快捷键（悬停气泡用）
fn tool_label(t: Tool) -> (&'static str, &'static str) {
    match t {
        Tool::Select => ("选择", "框选 / 移动选区"),
        Tool::Rect => ("矩形", "R"),
        Tool::Ellipse => ("椭圆", "E"),
        Tool::Line => ("直线", "L"),
        Tool::Arrow => ("箭头", "A"),
        Tool::Pen => ("铅笔", "P"),
        Tool::Marker => ("记号笔", "H"),
        Tool::Mosaic => ("马赛克", "M"),
        Tool::Blur => ("高斯模糊", "B"),
        Tool::Number => ("序号", "N"),
        Tool::Text => ("文字", "T"),
        Tool::Eraser => ("橡皮擦", "X"),
        Tool::Callout => ("引线气泡", "K"),
        Tool::Block => ("色块遮挡", "G"),
        Tool::Sticker => ("贴纸", "S"),
        Tool::Check => ("勾选图章", "X"),
        Tool::Cross => ("叉号图章", "Z"),
        Tool::Image => ("插入图片", "I"),
    }
}

fn bar_label(k: BarKind) -> (&'static str, &'static str) {
    match k {
        BarKind::Tool(t) => tool_label(t),
        BarKind::Group(g) => (
            GROUP_NAMES.get(g).copied().unwrap_or(""),
            "点一下 = 当前工具 · 悬停展开更多",
        ),
        BarKind::Chip(Chip::Color) => ("标注颜色", "点开调色板"),
        BarKind::Chip(Chip::Width) => ("线条粗细", "滚轮 / [ ] 调节"),
        BarKind::Chip(Chip::Fill) => ("形状填充", "当前色 20%"),
        BarKind::Act(a) => match a {
            "undo" => ("撤销", "Ctrl+Z"),
            "redo" => ("重做", "Ctrl+Y"),
            "cancel" => ("取消", "Esc"),
            "pin" => ("贴图", "中键"),
            "save" => ("保存到截图库", "Ctrl+S"),
            "copymore" => ("更多输出", "另存为 / 贴图 / OCR / 分享"),
            _ => ("复制到剪贴板", "Ctrl+C"),
        },
        BarKind::Sep => ("", ""),
    }
}

#[derive(Clone)]
enum Op {
    /// 矩形 / 椭圆最后那个 bool = 形状填充（当前色 20%，不遮底图文字）
    Rect([i32; 4], u32, i32, bool),
    Ellipse([i32; 4], u32, i32, bool),
    /// 直线存**起止两点**，和箭头同一个理由：包围盒装不下方向。
    /// 2026-11-19 owner 报"直线有 bug"，根因就是这里被 `norm4` 归一化成了
    /// "左上 → 右下" —— 从右上往左下拖，画出来的线是反的（对称的，所以看截图
    /// 很难发现，只有带着方向用才露馅）。箭头 0.3.x 踩过同一个坑，这里补上。
    Line((i32, i32), (i32, i32), u32, i32),
    /// 箭头存**起止两点**，不是矩形。
    ///
    /// 为什么单独一种形状：矩形/椭圆/直线只关心包围盒，两点顺序无所谓；
    /// 箭头有方向，一旦被 `norm4` 归一化就只剩"左上 → 右下"。
    /// 2026-11-18 owner 报"箭头没办法指向任意地方"，根因就是这里被当成矩形存。
    Arrow((i32, i32), (i32, i32), u32, i32),
    Pen(Vec<(i32, i32)>, u32, i32),
    Marker(Vec<(i32, i32)>, u32, i32),
    Mosaic([i32; 4]),
    /// 画笔式马赛克：路径点 + 覆盖半径（块大小固定 10）
    MosaicPath(Vec<(i32, i32)>, i32),
    /// 画笔式高斯模糊：路径点 + 覆盖半径。和 `MosaicPath` 同一套交互，
    /// 区别只在于沿路径做的是模糊而不是取块均值。
    BlurPath(Vec<(i32, i32)>, i32),
    Blur([i32; 4]),
    /// 序号：中心点、序号值、颜色
    Number(i32, i32, i32, u32),
    Text(String, i32, i32, u32, i32),
    /* ---- 0.4.19：参考稿补齐的工具 ---- */
    /// 色块遮挡：整块实心，彻底盖掉（不透明，和"半透明填充"区分开）
    Block([i32; 4], u32),
    /// 图章：中心点 + 颜色 + 种类（勾 / 叉 / 贴纸笑脸）
    Stamp(i32, i32, u32, Stamp),
    /// 引线气泡：矩形 + 颜色 + 文字（空串 = 只有泡泡）
    Callout([i32; 4], u32, String),
}

/// 图章的种类
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stamp {
    Check,
    Cross,
    Smiley,
}

impl Op {
    /// 脏矩形（擦橡皮与重绘判据用）。箭头这里必须**归一化**：
    /// 端点可以是任意方向，但包围盒只能是正矩形。
    fn bbox(&self) -> [i32; 4] {
        match self {
            Op::Rect(a, _, _, _) | Op::Ellipse(a, _, _, _) | Op::Mosaic(a) => *a,
            Op::Blur(a) => *a,
            /* 直线 / 箭头：端点方向任意，包围盒只能先归一化 */
            Op::Line(p0, p1, _, w) => {
                let r = norm4(*p0, *p1);
                let pad = w + 2;
                [r[0] - pad, r[1] - pad, r[2] + pad * 2, r[3] + pad * 2]
            }
            Op::Arrow(p0, p1, _, w) => {
                let r = norm4(*p0, *p1);
                let pad = w + 2;
                [r[0] - pad, r[1] - pad, r[2] + pad * 2, r[3] + pad * 2]
            }
            Op::Number(x, y, _, _) => [*x - 14, *y - 14, 28, 28],
            Op::Block(a, _) => *a,
            Op::Stamp(x, y, _, _) => [*x - 18, *y - 18, 36, 36],
            Op::Callout(a, _, _) => *a,
            Op::MosaicPath(p, r) | Op::BlurPath(p, r) => {
                let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
                for (x, y) in p {
                    x0 = x0.min(*x - r);
                    y0 = y0.min(*y - r);
                    x1 = x1.max(*x + r);
                    y1 = y1.max(*y + r);
                }
                [x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)]
            }
            Op::Pen(p, _, w) | Op::Marker(p, _, w) => {
                let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
                for (x, y) in p {
                    x0 = x0.min(*x);
                    y0 = y0.min(*y);
                    x1 = x1.max(*x);
                    y1 = y1.max(*y);
                }
                [x0 - w, y0 - w, (x1 - x0) + w * 2, (y1 - y0) + w * 2]
            }
            Op::Text(_, x, y, _, size) => [*x, *y - size, size * 40, size + size / 2],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    NewSel,
    MoveSel,
    Resize(usize), // 0..3 左上/右上/左下/右下
    Draw,
}

struct Ctx {
    w: i32,
    h: i32,
    /// 虚拟桌面左上角在屏幕坐标系里的位置（HUD 显示绝对坐标用）
    ox: i32,
    oy: i32,
    /// 主窗当前主题对应的那一套色（`run` 时定下来，整场截图不变）
    pal: Palette,
    mem: Hdc,
    bits: *mut u8,
    base: Vec<u8>,
    dim: Vec<u8>,
    /// **不许压暗的矩形**（物理像素，相对虚拟桌面左上角）—— 屏幕上的贴图窗口。
    ///
    /// 为什么：贴图是用户特意摆在屏幕上的东西，owner 2026-11-19 要求"截图时贴图
    /// 不透明度 100%"。冻结图里本来就有它们，但整屏压暗会连它们一起压暗，
    /// 看上去就是"贴图慢慢消失了"。这里在压暗之后再从原始冻结图盖一遍。
    keep: Vec<[i32; 4]>,
    sel: Option<[i32; 4]>,
    sel0: [i32; 4],
    /// 多框选（0.4.24）：已确认的选区块集合。在已有块**外面**再拖 = 追加一块，
    /// 右键 = 移除当前块；交付取所有块的**并集**（块内不透明、块间透明）。
    /// 单块时行为与老版本完全一致。
    areas: Vec<[i32; 4]>,
    ops: Vec<Op>,
    redo: Vec<Op>,
    tool: Tool,
    color: u32,
    width: i32,
    drag: Drag,
    anchor: (i32, i32),
    cur: (i32, i32),
    pts: Vec<(i32, i32)>,
    bar: [i32; 4],
    /// 正在拖的那一笔（实时预览，不进 ops）
    preview: Option<Op>,
    /// 遮罩类工具（马赛克 / 高斯模糊）的形态：false = 拖矩形，true = 当画笔随手涂。
    /// 两种工具**共用一个开关** —— 它们是同一类操作，拆成两个字段只会出现
    /// "马赛克是画笔、模糊还停在矩形"这种自相矛盾的状态。
    brush: bool,
    /// 下一个序号（放下一个数字标记时 +1）
    number_next: i32,
    /// 主条上悬停的按钮下标（分隔条不算）
    hover: Option<usize>,
    /// 当前展开的托盘属于哪个组（BAR 下标）。同屏只允许一个。
    open: Option<usize>,
    /// 托盘矩形。`tray_layout` 算一次存下来，绘制与命中共用。
    tray: [i32; 4],
    /// 托盘里悬停的条目下标
    tray_hover: Option<usize>,
    /// 悬停状态机：到达 `Instant` 时把 `open` 设成（true, 下标）或 None。
    /// 鼠标移动只改这个截止时间，真正的开合在 `WM_TIMER` 里落地。
    hover_due: Option<(std::time::Instant, bool, usize)>,
    /// 每组收起时显示的工具 = 该组"上次使用的那个"
    face: [Tool; 5],
    /// 分裂按钮右段是否悬停（左段底是两段共用的，高亮要一起亮）
    more_hover: bool,
    /// `[复制|▾]` 的下拉是否展开
    menu_open: bool,
    /// 下拉矩形（绘制与命中共用）
    menu: [i32; 4],
    /// 下拉里悬停的条目（0..MENU_ITEMS）。**没有它，菜单里鼠标移过去毫无反馈** ——
    /// owner 2026-11-21 的截图报的就是这个：三行长得一模一样，看不出要交付哪一条。
    menu_hover: Option<usize>,
    /// 调色板展开状态与它的矩形
    palette: bool,
    sw: [i32; 4],
    /// 形状填充（当前色 20%）
    fill: bool,
    hwnd: Hwnd,
    /// 右下角提示面板的矩形（`draw_keys` 算出来记下，测试要断言这块的 alpha）
    keys: [i32; 4],
    editor: Hwnd,
    editing: Option<(i32, i32)>,
    /* ---- 取色器 / 窗口识别（0.5.x） ---- */
    /// 光标在客户区里的位置（物理像素）。放大镜和窗口识别都读它。
    mouse: Option<(i32, i32)>,
    /// 色值格式：false = `R, G, B`，true = `#RRGGBB`（Shift 切换）
    hex: bool,
    /// 光标下待识别的顶层窗口矩形（虚拟桌面坐标，已 clamp）
    win_hit: Option<[i32; 4]>,
    /// 按下瞬间抓到的窗口矩形 —— 抬起时位移小于 `CLICK_SLOP` 就用它当选区
    press_win: Option<[i32; 4]>,
    /// 本次按下后是否真的拖动过（区分「单击截窗」与「自由框选」）
    moved: bool,
    // 交付结果
    done: Option<(String, Vec<u8>, [f64; 4])>,
}

unsafe impl Send for Ctx {}

static CTX: Mutex<Option<Box<Ctx>>> = Mutex::new(None);
static CUR: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn cls() -> &'static [u16] {
    // "AMSnipNative\0"
    &[
        65, 77, 83, 110, 105, 112, 78, 97, 116, 105, 118, 101, 0,
    ]
}

/* ---------------- 小工具 ---------------- */

/* ---------- 几何（图标抗锯齿用） ---------- */

/// 点到线段的最短距离
fn dist_seg(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let (vx, vy) = (x1 - x0, y1 - y0);
    let (wx, wy) = (px - x0, py - y0);
    let l2 = vx * vx + vy * vy;
    let t = if l2 <= 1e-6 {
        0.0
    } else {
        ((wx * vx + wy * vy) / l2).clamp(0.0, 1.0)
    };
    let dx = px - (x0 + t * vx);
    let dy = py - (y0 + t * vy);
    (dx * dx + dy * dy).sqrt()
}

/// 射线法判断点是否在多边形内（4×4 超采样用，不做边界特判）
fn point_in_poly(px: f32, py: f32, pts: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > py) != (yj > py) {
            let x_int = (xj - xi) * (py - yi) / (yj - yi) + xi;
            if px < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/* ---------------- SVG 图标引擎 ----------------

   owner 2026-11-21 给了 `screenshot-toolbar-v2/`（24 个 20×20 图标、stroke 1.6、
   圆头圆角），要求「icon 用里面的」。上一版是 `icon_px` 里逐个 `match` 手绘近似，
   密度和语义都追不上原稿；这一版直接解析原型里的 SVG path，用已有的覆盖率原语
   描边/填充 —— 等于原样复刻，改图标只改下面 `ICONS` 里的字符串。

   只实现图标用到的命令：M L H V C S Q T A Z（大写绝对 / 小写相对）。
   曲线按 12 段采样、弧线按 7.5° 采样：20px 下看不出折线感，
   但省掉贝塞尔 flatness 与 SVG 的完整命令集。 */

/// 读一个 SVG 数字（跳过空白/逗号，支持符号、小数、指数）
fn svg_num(cs: &[char], i: &mut usize) -> f32 {
    let ws = |c: char| c == ' ' || c == ',' || c == '\n' || c == '\r' || c == '\t';
    while *i < cs.len() && ws(cs[*i]) {
        *i += 1;
    }
    let start = *i;
    if *i < cs.len() && (cs[*i] == '-' || cs[*i] == '+') {
        *i += 1;
    }
    while *i < cs.len() && cs[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i < cs.len() && cs[*i] == '.' {
        *i += 1;
        while *i < cs.len() && cs[*i].is_ascii_digit() {
            *i += 1;
        }
    }
    if *i < cs.len() && (cs[*i] == 'e' || cs[*i] == 'E') {
        *i += 1;
        if *i < cs.len() && (cs[*i] == '-' || cs[*i] == '+') {
            *i += 1;
        }
        while *i < cs.len() && cs[*i].is_ascii_digit() {
            *i += 1;
        }
    }
    cs[start..*i]
        .iter()
        .collect::<String>()
        .parse::<f32>()
        .unwrap_or(0.0)
}

/// 三次贝塞尔采样（`t` 从 0 到 1，含终点，不含起点）
fn svg_cubic(out: &mut Vec<(f32, f32)>, p0: (f32, f32), c1: (f32, f32), c2: (f32, f32), p1: (f32, f32)) {
    let n = 12;
    for k in 1..=n {
        let t = k as f32 / n as f32;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((
            a * p0.0 + b * c1.0 + c * c2.0 + d * p1.0,
            a * p0.1 + b * c1.1 + c * c2.1 + d * p1.1,
        ));
    }
}

/// 二次贝塞尔采样
fn svg_quad(out: &mut Vec<(f32, f32)>, p0: (f32, f32), c: (f32, f32), p1: (f32, f32)) {
    let n = 10;
    for k in 1..=n {
        let t = k as f32 / n as f32;
        let u = 1.0 - t;
        out.push((
            u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
            u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
        ));
    }
}

/// 椭圆弧端点参数化 → 圆心参数化，再按角度采样（SVG 规范 F.6.5，`phi` 恒为 0）
fn svg_arc(
    out: &mut Vec<(f32, f32)>,
    x1: f32,
    y1: f32,
    rx: f32,
    ry: f32,
    large: bool,
    sweep: bool,
    x2: f32,
    y2: f32,
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-4 || ry < 1e-4 {
        out.push((x2, y2));
        return;
    }
    let dx2 = (x1 - x2) * 0.5;
    let dy2 = (y1 - y2) * 0.5;
    let lam = dx2 * dx2 / (rx * rx) + dy2 * dy2 / (ry * ry);
    if lam > 1.0 {
        let s = lam.sqrt();
        rx *= s;
        ry *= s;
    }
    let sign = if large != sweep { 1.0 } else { -1.0 };
    let num = rx * rx * ry * ry - rx * rx * dy2 * dy2 - ry * ry * dx2 * dx2;
    let den = rx * rx * dy2 * dy2 + ry * ry * dx2 * dx2;
    let co = if den < 1e-9 {
        0.0
    } else {
        sign * (num.max(0.0) / den).sqrt()
    };
    let cxp = co * rx * dy2 / ry;
    let cyp = -co * ry * dx2 / rx;
    let cx = cxp + (x1 + x2) * 0.5;
    let cy = cyp + (y1 + y2) * 0.5;
    let ux = (dx2 - cxp) / rx;
    let uy = (dy2 - cyp) / ry;
    let vx = (-dx2 - cxp) / rx;
    let vy = (-dy2 - cyp) / ry;
    let th1 = uy.atan2(ux);
    let mut dth = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    if !sweep && dth > 0.0 {
        dth -= std::f32::consts::TAU;
    } else if sweep && dth < 0.0 {
        dth += std::f32::consts::TAU;
    }
    let n = ((dth.abs() / (std::f32::consts::PI / 24.0)).ceil() as i32).clamp(2, 96);
    for k in 1..=n {
        let t = th1 + dth * (k as f32 / n as f32);
        out.push((cx + rx * t.cos(), cy + ry * t.sin()));
    }
}

/// 把一条 SVG path 拆成若干折线子路径（viewBox 原始坐标，未缩放）
fn svg_polys(d: &str) -> Vec<Vec<(f32, f32)>> {
    let cs: Vec<char> = d.chars().collect();
    let mut i = 0usize;
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    let mut polys: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut prev_c: Option<(f32, f32)> = None;
    let mut prev_q: Option<(f32, f32)> = None;
    let mut last = 'M';
    while i < cs.len() {
        while i < cs.len() && (cs[i] == ' ' || cs[i] == ',' || cs[i] == '\n' || cs[i] == '\r' || cs[i] == '\t') {
            i += 1;
        }
        if i >= cs.len() {
            break;
        }
        let raw = cs[i];
        let c = if raw.is_ascii_alphabetic() {
            i += 1;
            last = raw;
            raw
        } else {
            last
        };
        let rel = c.is_ascii_lowercase();
        let up = c.to_ascii_uppercase();
        let (px, py) = (x, y);
        match up {
            'M' => {
                let dx = svg_num(&cs, &mut i);
                let dy = svg_num(&cs, &mut i);
                let (nx, ny) = if rel { (px + dx, py + dy) } else { (dx, dy) };
                if cur.len() > 1 {
                    polys.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                cur.push((nx, ny));
                x = nx;
                y = ny;
                sx = nx;
                sy = ny;
                prev_c = None;
                prev_q = None;
                /* M 后面跟的裸坐标是隐式 L */
                last = if rel { 'l' } else { 'L' };
            }
            'L' => {
                let dx = svg_num(&cs, &mut i);
                let dy = svg_num(&cs, &mut i);
                let (nx, ny) = if rel { (px + dx, py + dy) } else { (dx, dy) };
                cur.push((nx, ny));
                x = nx;
                y = ny;
                prev_c = None;
                prev_q = None;
            }
            'H' => {
                let dx = svg_num(&cs, &mut i);
                let nx = if rel { px + dx } else { dx };
                cur.push((nx, y));
                x = nx;
                prev_c = None;
                prev_q = None;
            }
            'V' => {
                let dy = svg_num(&cs, &mut i);
                let ny = if rel { py + dy } else { dy };
                cur.push((x, ny));
                y = ny;
                prev_c = None;
                prev_q = None;
            }
            'C' => {
                let (mut c1x, mut c1y) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                let (mut c2x, mut c2y) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                let (mut ex, mut ey) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                if rel {
                    c1x += px;
                    c1y += py;
                    c2x += px;
                    c2y += py;
                    ex += px;
                    ey += py;
                }
                svg_cubic(&mut cur, (px, py), (c1x, c1y), (c2x, c2y), (ex, ey));
                prev_c = Some((c2x, c2y));
                prev_q = None;
                x = ex;
                y = ey;
            }
            'S' => {
                let (c2x, c2y) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                let (ex, ey) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                let (mut c2x, mut c2y, mut ex, mut ey) = (c2x, c2y, ex, ey);
                if rel {
                    c2x += px;
                    c2y += py;
                    ex += px;
                    ey += py;
                }
                let c1 = match prev_c {
                    Some((qx, qy)) => (2.0 * px - qx, 2.0 * py - qy),
                    None => (px, py),
                };
                svg_cubic(&mut cur, (px, py), c1, (c2x, c2y), (ex, ey));
                prev_c = Some((c2x, c2y));
                prev_q = None;
                x = ex;
                y = ey;
            }
            'Q' => {
                let (cx, cy) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                let (ex, ey) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                let (mut cx, mut cy, mut ex, mut ey) = (cx, cy, ex, ey);
                if rel {
                    cx += px;
                    cy += py;
                    ex += px;
                    ey += py;
                }
                svg_quad(&mut cur, (px, py), (cx, cy), (ex, ey));
                prev_q = Some((cx, cy));
                prev_c = None;
                x = ex;
                y = ey;
            }
            'T' => {
                let (ex, ey) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                let (mut ex, mut ey) = (ex, ey);
                if rel {
                    ex += px;
                    ey += py;
                }
                let c = match prev_q {
                    Some((qx, qy)) => (2.0 * px - qx, 2.0 * py - qy),
                    None => (px, py),
                };
                svg_quad(&mut cur, (px, py), c, (ex, ey));
                prev_q = Some(c);
                prev_c = None;
                x = ex;
                y = ey;
            }
            'A' => {
                let rx = svg_num(&cs, &mut i);
                let ry = svg_num(&cs, &mut i);
                let _rot = svg_num(&cs, &mut i);
                let laf = svg_num(&cs, &mut i) != 0.0;
                let sf = svg_num(&cs, &mut i) != 0.0;
                let (ex, ey) = (svg_num(&cs, &mut i), svg_num(&cs, &mut i));
                let (ex, ey) = if rel { (px + ex, py + ey) } else { (ex, ey) };
                svg_arc(&mut cur, px, py, rx, ry, laf, sf, ex, ey);
                prev_c = None;
                prev_q = None;
                x = ex;
                y = ey;
            }
            'Z' => {
                if !cur.is_empty() {
                    cur.push((sx, sy));
                    polys.push(std::mem::take(&mut cur));
                }
                x = sx;
                y = sy;
                prev_c = None;
                prev_q = None;
            }
            _ => {
                /* 未知命令：吞一个数字避免死循环 */
                let _ = svg_num(&cs, &mut i);
            }
        }
        /* 没有新命令字母时 `last` 保持，下一轮按同一命令继续读参数 */
        if last == 'Z' || last == 'z' {
            last = 'M';
        }
    }
    if cur.len() > 1 {
        polys.push(cur);
    }
    polys
}

fn rgb(c: u32) -> u32 {
    // 0xRRGGBB → COLORREF(0x00BBGGRR)
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

/* ---------------- 图标表（照抄 `screenshot-toolbar-v2/index.html` 的 ICONS） ----------------

   每个部件是 `(模式, 透明度, path)`：
     0 = 描边（stroke 1.6 / viewBox 单位，圆头圆角）
     1 = 填充
     2 = 虚线描边（dash 1.6、gap 3.2，只给 `blur` 用）

   原稿里的 `<rect>` / `<ellipse>` / `<circle>` 已展开成等价 path ——
   引擎只认 path，少一套分支。24×24 viewBox，实际渲染 size（默认 20）。 */

/// r=8 的圆（check / cross / sticker / badge 共用）
const IC_CIRCLE8: &str = "M4 12A8 8 0 1 0 20 12A8 8 0 1 0 4 12Z";

const ICONS: &[(&str, &[(u8, f32, &str)])] = &[
    ("select", &[(0, 1.0, "M5 3v16.6l4.4-4.2 2.6 5 3-1.6-2.7-4.9 6-.5z")]),
    ("rect", &[(0, 1.0, "M6.4 5H17.6A2.2 2.2 0 0 1 19.8 7.2V16.8A2.2 2.2 0 0 1 17.6 19H6.4A2.2 2.2 0 0 1 4.2 16.8V7.2A2.2 2.2 0 0 1 6.4 5Z")]),
    ("ellipse", &[(0, 1.0, "M4 12A8 6.6 0 1 0 20 12A8 6.6 0 1 0 4 12Z")]),
    ("line", &[(0, 1.0, "M5 19L19 5")]),
    ("arrow", &[(0, 1.0, "M4.5 19.5L19 5"), (0, 1.0, "M12.6 5H19V11.4")]),
    ("pen", &[
        (0, 1.0, "M4 20l4-.9L19.3 7.8a2.4 2.4 0 0 0-3.4-3.4L4.6 15.7z"),
        (0, 1.0, "M14.4 6.6l3 3"),
    ]),
    ("marker", &[
        (0, 1.0, "M7.6 16.4L15.1 8.9a2.35 2.35 0 0 1 3.3 3.3l-7.5 7.5-4.5 1.1z"),
        (0, 1.0, "M13.2 10.8l3.3 3.3"),
    ]),
    ("text", &[
        (0, 1.0, "M5 6.4V5h14v1.4"),
        (0, 1.0, "M12 5V19"),
        (0, 1.0, "M8.6 19h6.8"),
    ]),
    ("badge", &[
        (0, 1.0, IC_CIRCLE8),
        (0, 1.0, "M11.1 9.5L12.8 8.2V16"),
        (0, 1.0, "M9.8 16h5.6"),
    ]),
    ("callout", &[(0, 1.0, "M5 5.5h14v10h-9.2L6.8 19v-3.5H5z")]),
    ("mosaic", &[(1, 0.85, "M4 4h5v5H4zM13 4h7v5h-7zM4 11h5v5H4zM13 11h7v5h-7zM4 18h5v2H4zM13 18h7v2h-7z")]),
    ("blur", &[
        (2, 1.0, "M4.4 12A7.6 7.6 0 1 0 19.6 12A7.6 7.6 0 1 0 4.4 12Z"),
        (0, 0.55, "M8.6 12A3.4 3.4 0 1 0 15.4 12A3.4 3.4 0 1 0 8.6 12Z"),
    ]),
    ("block", &[(1, 0.85, "M6.5 6H17.5A2 2 0 0 1 19.5 8V16A2 2 0 0 1 17.5 18H6.5A2 2 0 0 1 4.5 16V8A2 2 0 0 1 6.5 6Z")]),
    ("sticker", &[
        (0, 1.0, IC_CIRCLE8),
        (0, 1.0, "M9.2 13.6c.6 1 1.7 1.6 2.8 1.6s2.2-.6 2.8-1.6"),
        (1, 1.0, "M8.45 9.9A0.95 0.95 0 1 0 10.35 9.9A0.95 0.95 0 1 0 8.45 9.9Z"),
        (1, 1.0, "M13.65 9.9A0.95 0.95 0 1 0 15.55 9.9A0.95 0.95 0 1 0 13.65 9.9Z"),
    ]),
    ("check", &[(0, 1.0, IC_CIRCLE8), (0, 1.0, "M8.4 12.2l2.5 2.5 4.7-5")]),
    ("cross", &[
        (0, 1.0, IC_CIRCLE8),
        (0, 1.0, "M9.2 9.2l5.6 5.6"),
        (0, 1.0, "M14.8 9.2l-5.6 5.6"),
    ]),
    ("image", &[
        (0, 1.0, "M6 5H18A2 2 0 0 1 20 7V17A2 2 0 0 1 18 19H6A2 2 0 0 1 4 17V7A2 2 0 0 1 6 5Z"),
        (0, 1.0, "M7.7 10A1.3 1.3 0 1 0 10.3 10A1.3 1.3 0 1 0 7.7 10Z"),
        (0, 1.0, "M5 17.2l4.5-4.5 3 3 2.8-2.8 3.7 3.7"),
    ]),
    ("fill", &[
        (0, 1.0, "M6.4 4.4H17.6A2 2 0 0 1 19.6 6.4V17.6A2 2 0 0 1 17.6 19.6H6.4A2 2 0 0 1 4.4 17.6V6.4A2 2 0 0 1 6.4 4.4Z"),
        (1, 0.5, "M4.4 12h15.2v5.6a2 2 0 0 1-2 2H6.4a2 2 0 0 1-2-2z"),
    ]),
    ("undo", &[
        (0, 1.0, "M4.5 9.5h9a4.3 4.3 0 0 1 0 8.6H8.4"),
        (0, 1.0, "M8 5.6L4.2 9.5L8 13.4"),
    ]),
    ("redo", &[
        (0, 1.0, "M19.5 9.5h-9a4.3 4.3 0 0 0 0 8.6h5.1"),
        (0, 1.0, "M16 5.6l3.8 3.9L16 13.4"),
    ]),
    ("copy", &[
        (0, 1.0, "M10.2 8.2H17.2A2 2 0 0 1 19.2 10.2V17.2A2 2 0 0 1 17.2 19.2H10.2A2 2 0 0 1 8.2 17.2V10.2A2 2 0 0 1 10.2 8.2Z"),
        (0, 1.0, "M15.6 5H6.6A1.6 1.6 0 0 0 5 6.6V15.6"),
    ]),
    ("save", &[
        (0, 1.0, "M5 5h9.4L19 9.6V19H5z"),
        (0, 1.0, "M8.4 5v4.6h6.8V5"),
        (0, 1.0, "M8.4 13H15.6V19H8.4Z"),
    ]),
    ("pin", &[
        (0, 1.0, "M9.4 4h5.2l-1 4.8 3.4 3H6L9.4 8.8z"),
        (0, 1.0, "M12 11.8V20"),
    ]),
    ("ocr", &[
        (0, 1.0, "M4.6 8.4V5.6h2.8M16.6 5.6h2.8v2.8M19.4 15.6v2.8h-2.8M7.4 18.4H4.6v-2.8"),
        (0, 1.0, "M8.6 10.6h6.8M8.6 13.6h4.4"),
    ]),
    ("share", &[
        (0, 1.0, "M12 15.4V4.2"),
        (0, 1.0, "M8.4 7.8L12 4.2l3.6 3.6"),
        (0, 1.0, "M5.2 13.4v6.4h13.6v-6.4"),
    ]),
    ("close", &[(0, 1.0, "M6.4 6.4l11.2 11.2M17.6 6.4L6.4 17.6")]),
    ("chevron", &[(0, 1.0, "M6 9.5l6 6 6-6")]),
    ("chevronUp", &[(0, 1.0, "M6 14.5l6-6 6 6")]),
];

fn icon_parts(name: &str) -> &'static [(u8, f32, &'static str)] {
    for (n, p) in ICONS {
        if *n == name {
            return p;
        }
    }
    &[]
}




fn norm4(a: (i32, i32), b: (i32, i32)) -> [i32; 4] {
    [
        a.0.min(b.0),
        a.1.min(b.1),
        (a.0 - b.0).abs(),
        (a.1 - b.1).abs(),
    ]
}

fn inside(r: [i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]
}

/* ---------------- 取色器（放大镜） / 窗口识别 ----------------

   两块功能都只改原生层：`Snip.tsx` 是 A 方案失败时的回落，真机走不到，
   而且那边另一路在改，保持原样。

   坐标口径沿用本文件：**虚拟桌面物理像素**（客户区原点 = 虚拟桌面左上角）。
   放大镜取样直接读 `Ctx::base`（冻结帧的原始 BGRA），不读 `bits` ——
   后者已经压暗、画过标注，取到的颜色是"用户看到的假颜色"。

   8 倍、184×136 是照 `Snip.tsx` 的 `LOUPE` 口径（184/8=23、136/8=17，
   都是奇数，光标那一像素正好落在面板正中心，描边框才对得齐）。 */

const LOUPE_ZOOM: i32 = 8;
/// 放大镜显示区（取样 23×17 个源像素）
const LOUPE_W: i32 = 184;
const LOUPE_H: i32 = 136;
/// 面板内距（放大镜不能顶到圆角上）
const LOUPE_PAD: i32 = 6;
/// 放大镜下方：坐标 + 色块/色值
const LOUPE_INFO_H: i32 = 56;
/// 最底部两行提示（按 C / 按 Shift）
const LOUPE_HINT_H: i32 = 36;
/// 面板离屏幕边的硬留白
const LOUPE_MARGIN: i32 = 4;
/// 面板与光标的间距（放右下方；贴边时翻到左上方）
const LOUPE_OFFSET: i32 = 18;
/// 「单击」与「拖动」的分界（物理像素）。手抖几个像素仍然算单击。
const CLICK_SLOP: i32 = 5;

fn loupe_panel_w() -> i32 {
    LOUPE_W + LOUPE_PAD * 2
}

fn loupe_panel_h() -> i32 {
    LOUPE_PAD + LOUPE_H + LOUPE_INFO_H + LOUPE_HINT_H
}

/// `R, G, B`（owner 截图里的默认格式，如 `250, 250, 248`）
fn fmt_rgb(r: u8, g: u8, b: u8) -> String {
    format!("{r}, {g}, {b}")
}

/// `#RRGGBB`（Shift 切换后的格式，大写更符合设计稿）
fn fmt_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// 把一个矩形夹进 `w × h` 的桌面；完全落在外面返回 None。
/// 窗口矩形来自 DWM，最大化窗口的边框会伸到屏幕外（常见 -8px），必须夹。
fn clamp_rect(r: [i32; 4], w: i32, h: i32) -> Option<[i32; 4]> {
    let x0 = r[0].max(0);
    let y0 = r[1].max(0);
    let x1 = (r[0] + r[2]).min(w);
    let y1 = (r[1] + r[3]).min(h);
    if x1 - x0 <= 0 || y1 - y0 <= 0 {
        None
    } else {
        Some([x0, y0, x1 - x0, y1 - y0])
    }
}

/// 单轴夹紧；屏幕比面板还小时（罕见）退化成居中偏 0，绝不 panic。
fn clamp_panel(v: i32, size: i32, total: i32) -> i32 {
    let hi = total - size - LOUPE_MARGIN;
    if hi < LOUPE_MARGIN {
        ((total - size) / 2).max(0)
    } else {
        v.clamp(LOUPE_MARGIN, hi)
    }
}

/// 放大镜面板矩形：默认在光标右下，右边/下边放不下就翻到左/上，最后夹进屏幕。
/// 纯函数，单测直接覆盖四角与翻转。
fn loupe_at(cx: i32, cy: i32, sw: i32, sh: i32) -> [i32; 4] {
    let (pw, ph) = (loupe_panel_w(), loupe_panel_h());
    let mut x = cx + LOUPE_OFFSET;
    let mut y = cy + LOUPE_OFFSET;
    if x + pw > sw - LOUPE_MARGIN {
        x = cx - LOUPE_OFFSET - pw;
    }
    if y + ph > sh - LOUPE_MARGIN {
        y = cy - LOUPE_OFFSET - ph;
    }
    [
        clamp_panel(x, pw, sw),
        clamp_panel(y, ph, sh),
        pw,
        ph,
    ]
}


fn lo(v: isize) -> i32 {
    (v & 0xFFFF) as i16 as i32
}
fn hi(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as i16 as i32
}

impl Ctx {
    fn put(&mut self, x: i32, y: i32, r: u8, g: u8, b: u8, a: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let o = ((y as usize) * (self.w as usize) + x as usize) * 4;
        unsafe {
            let p = self.bits.add(o);
            let k = a.clamp(0.0, 1.0);
            *p = ((*p as f32) * (1.0 - k) + (b as f32) * k) as u8;
            *p.add(1) = ((*p.add(1) as f32) * (1.0 - k) + (g as f32) * k) as u8;
            *p.add(2) = ((*p.add(2) as f32) * (1.0 - k) + (r as f32) * k) as u8;
        }
    }

    /// 冻结图 → DIB；有选区则选区外压暗
    fn fill_base(&mut self) {
        let row = self.w as usize * 4;
        /* 亮区 = 已确认块 ∪ 当前选区（0.4.24 多框选）：并集内保持 100% 亮度，
           其余压暗。单块时与老行为逐像素等价。 */
        let mut lit: Vec<[i32; 4]> = self.areas.clone();
        if let Some(s) = self.sel {
            if !lit.contains(&s) {
                lit.push(s);
            }
        }
        lit.retain(|r| r[2] > 2 && r[3] > 2);
        if lit.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.dim.as_ptr(),
                    self.bits,
                    (self.w as usize) * (self.h as usize) * 4,
                );
            }
        } else {
            for y in 0..self.h {
                let off = y as usize * row;
                /* 先整行压暗，再把落在亮区里的段从原始冻结图盖回来 */
                unsafe {
                    std::ptr::copy_nonoverlapping(self.dim.as_ptr().add(off), self.bits.add(off), row);
                }
                for r in &lit {
                    if y < r[1] || y >= r[1] + r[3] {
                        continue;
                    }
                    let l = (r[0].max(0) as usize) * 4;
                    let rr = ((r[0] + r[2]).min(self.w) as usize) * 4;
                    if rr > l {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                self.base.as_ptr().add(off + l),
                                self.bits.add(off + l),
                                rr - l,
                            );
                        }
                    }
                }
            }
        }
        /* 压暗之后再把这些矩形从**原始冻结图**盖回来：贴图窗口保持 100% 亮度。
           放在压暗之后、标注之前 —— 标注（箭头/文字）依然可以画在贴图上。 */
        if !self.keep.is_empty() {
            let row = self.w as usize * 4;
            for r in &self.keep {
                let x0 = r[0].max(0);
                let y0 = r[1].max(0);
                let x1 = (r[0] + r[2]).min(self.w);
                let y1 = (r[1] + r[3]).min(self.h);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let n = (x1 - x0) as usize * 4;
                for y in y0..y1 {
                    let off = y as usize * row + x0 as usize * 4;
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            self.base.as_ptr().add(off),
                            self.bits.add(off),
                            n,
                        );
                    }
                }
            }
        }
    }

    fn draw_ops(&mut self) {
        let ops = std::mem::take(&mut self.ops);
        for op in &ops {
            self.draw_op(op);
        }
        self.ops = ops;
    }

    fn draw_op(&mut self, op: &Op) {
        /* 标注也一律像素直绘。原因和工具栏一样：GDI 的 pen/brush 在这块 layered DIB 上
           会残留状态（同一批调用只有后一半生效），表现为"有的标注画不出来"。
           只有文字仍走 GDI —— 中文字形没法自己点像素。 */
        match op {
            Op::Rect(r, c, w, fill) => {
                if *fill {
                    self.pfill_a(r[0], r[1], r[0] + r[2], r[1] + r[3], *c, 0.20);
                }
                self.pframe(r[0], r[1], r[0] + r[2], r[1] + r[3], *w, *c)
            }
            Op::Ellipse(r, c, w, fill) => {
                let (cx, cy) = (r[0] + r[2] / 2, r[1] + r[3] / 2);
                if *fill {
                    self.fill_ellipse(cx, cy, r[2] / 2, r[3] / 2, *c, 0.20);
                }
                self.pellipse(cx, cy, r[2] / 2, r[3] / 2, *w, *c)
            }
            Op::Line(p0, p1, c, w) => self.pline(p0.0, p0.1, p1.0, p1.1, *w, *c),
            Op::Arrow(p0, p1, c, w) => {
                /* 起点、终点直接用，方向由它们算 —— 别在这里套 norm4（见 `Op::Arrow` 的注释）。 */
                let (x0, y0) = *p0;
                let (x1, y1) = *p1;
                self.pline(x0, y0, x1, y1, *w, *c);
                let (dx, dy) = ((x1 - x0) as f32, (y1 - y0) as f32);
                let len = (dx * dx + dy * dy).sqrt().max(1.0);
                let (ux, uy) = (dx / len, dy / len);
                let hl = (14.0 + *w as f32 * 2.5).min(len);
                let hw = hl * 0.55;
                let (bx, by) = (x1 as f32 - ux * hl, y1 as f32 - uy * hl);
                let pts = [
                    (x1, y1),
                    ((bx - uy * hw) as i32, (by + ux * hw) as i32),
                    ((bx + uy * hw) as i32, (by - ux * hw) as i32),
                ];
                self.ppoly(&pts, *c);
            }
            Op::Pen(p, c, w) => {
                for i in 1..p.len() {
                    self.pline(p[i - 1].0, p[i - 1].1, p[i].0, p[i].1, *w, *c);
                }
            }
            Op::Marker(p, c, w) => {
                /* 记号笔 = 半透明粗线（55%）。以前想"先画到临时数组再混合"，
                   现在像素直绘直接按 alpha 混，一行事。 */
                for i in 1..p.len() {
                    self.pline_a(p[i - 1].0, p[i - 1].1, p[i].0, p[i].1, *w, *c, 0.55);
                }
            }
            Op::MosaicPath(pts, r) => {
                /* 画笔式马赛克：把路径经过的每个 10px 格子马赛克化。
                   HashSet 去重 —— 慢拖时点很密，同一格会被反复命中。 */
                let bs = 10i32;
                let mut seen = std::collections::HashSet::new();
                for (px, py) in pts {
                    let gx0 = (px - r).div_euclid(bs) * bs;
                    let gy0 = (py - r).div_euclid(bs) * bs;
                    let gx1 = (px + r).div_euclid(bs) * bs;
                    let gy1 = (py + r).div_euclid(bs) * bs;
                    let mut gy = gy0;
                    while gy <= gy1 {
                        let mut gx = gx0;
                        while gx <= gx1 {
                            if seen.insert((gx, gy)) {
                                self.mosaic(gx, gy, bs, bs);
                            }
                            gx += bs;
                        }
                        gy += bs;
                    }
                }
            }
            Op::BlurPath(pts, r) => {
                /* 画笔式模糊：沿路径逐点模糊一块 2r 见方的区域。
                   两点讲究：
                   ① **不做格子去重**（马赛克要按块对齐才去重），而是按"离上一个落笔点
                      够近就跳过"降采样 —— 模糊区域是圆的，按格去重会在拐角漏出一条缝；
                   ② 模糊读的是原始冻结图（`base`），幂等，所以重叠区域重复处理
                      不会越涂越糊，只是白费时间。 */
                let step = (*r / 2).max(3);
                let mut last: Option<(i32, i32)> = None;
                for (px, py) in pts {
                    if let Some((lx, ly)) = last {
                        if (px - lx).abs() + (py - ly).abs() < step {
                            continue;
                        }
                    }
                    last = Some((*px, *py));
                    self.blur_region(px - *r, py - *r, *r * 2, *r * 2);
                }
            }
            /* 色块遮挡：整块实心（不透明）。和 `Rect` 的 20% 填充区分开 ——
               那个是"标重点"，这个是"盖掉敏感信息"。 */
            Op::Block(r, c) => self.pfill(r[0], r[1], r[0] + r[2], r[1] + r[3], *c),
            Op::Stamp(x, y, c, k) => {
                let (cx, cy) = (*x as f32, *y as f32);
                self.pdisc_aa(cx, cy, 15.0, *c);
                let ink = 0xFFFFFFu32;
                match k {
                    Stamp::Check => {
                        self.pline_aa(cx - 6.2, cy + 0.6, cx - 1.8, cy + 5.0, 3.0, ink);
                        self.pline_aa(cx - 1.8, cy + 5.0, cx + 6.6, cy - 4.8, 3.0, ink);
                    }
                    Stamp::Cross => {
                        self.pline_aa(cx - 5.2, cy - 5.2, cx + 5.2, cy + 5.2, 3.0, ink);
                        self.pline_aa(cx + 5.2, cy - 5.2, cx - 5.2, cy + 5.2, 3.0, ink);
                    }
                    Stamp::Smiley => {
                        /* 贴纸先给一个笑脸：两个实心眼 + 一段下弯的嘴（折线拟合） */
                        self.pdisc_aa(cx - 4.6, cy - 3.4, 1.7, ink);
                        self.pdisc_aa(cx + 4.6, cy - 3.4, 1.7, ink);
                        let mut prev = (cx - 6.0, cy + 2.4);
                        for i in 1..=10 {
                            let t = i as f32 / 10.0;
                            let px = cx - 6.0 + 12.0 * t;
                            let py = cy + 2.4 + 4.6 * (t * std::f32::consts::PI).sin();
                            self.pline_aa(prev.0, prev.1, px, py, 2.4, ink);
                            prev = (px, py);
                        }
                    }
                }
            }
            Op::Callout(r, c, txt) => {
                /* 引线气泡：白底（92%）+ 当前色描边 + 左下引线尖 + 文字。
                   白底是必须的 —— 气泡里的字要压得住任意底图。 */
                let (x, y, w, h) = (r[0], r[1], r[2].max(24), r[3].max(24));
                self.pfill_round_a(x, y, x + w, y + h, 8, 0xFFFFFF, 0.93);
                self.pframe_aa(x as f32, y as f32, (x + w) as f32, (y + h) as f32, 2.0, *c);
                let tx = (x + 18) as f32;
                let ty = (y + h) as f32;
                self.ppoly_aa_a(
                    &[(tx, ty - 1.0), (tx + 18.0, ty - 1.0), (tx + 2.0, ty + 14.0)],
                    *c,
                    0.93,
                );
                self.ppoly_aa_a(
                    &[(tx + 1.5, ty), (tx + 15.5, ty), (tx + 2.8, ty + 10.5)],
                    0xFFFFFF,
                    0.93,
                );
                if !txt.is_empty() {
                    let u: Vec<u16> = txt.encode_utf16().collect();
                    let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
                    unsafe {
                        let f = CreateFontW(-16, 0, 0, 0, 400, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
                        let oldf = SelectObject(self.mem, f);
                        SetBkMode(self.mem, TRANSPARENT);
                        SetTextColor(self.mem, rgb(0x1B1F27));
                        let mut rc = RectPos {
                            left: x + 10,
                            top: y + 8,
                            right: x + w - 10,
                            bottom: y + h - 8,
                        };
                        /* DT_LEFT(0) | DT_TOP(0) | DT_WORDBREAK(0x10)：这几个常量没导进来，直接给值 */
                        DrawTextW(self.mem, u.as_ptr(), u.len() as i32, &mut rc, 0x10);
                        SelectObject(self.mem, oldf);
                        DeleteObject(f);
                    }
                }
            }
            Op::Mosaic(r) => self.mosaic(r[0], r[1], r[2], r[3]),
            Op::Blur(r) => self.blur_region(r[0], r[1], r[2], r[3]),
            Op::Number(x, y, n, c) => {
                /* 序号：实心圆 + 白字。圆用像素画（GDI 椭圆在这块 DIB 上不可靠），
                   数字只能走 GDI 文字。 */
                self.pdisc(*x, *y, 14, *c);
                self.pframe(*x - 14, *y - 14, *x + 14, *y + 14, 1, 0xFFFFFF);
                let txt: Vec<u16> = n.to_string().encode_utf16().collect();
                let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
                unsafe {
                    let f = CreateFontW(-20, 0, 0, 0, FW_BOLD, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
                    let old = SelectObject(self.mem, f);
                    SetBkMode(self.mem, TRANSPARENT);
                    SetTextColor(self.mem, 0xFFFFFF);
                    let mut rc = RectPos {
                        left: *x - 14,
                        top: *y - 13,
                        right: *x + 14,
                        bottom: *y + 13,
                    };
                    DrawTextW(
                        self.mem,
                        txt.as_ptr(),
                        txt.len() as i32,
                        &mut rc,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                    SelectObject(self.mem, old);
                    DeleteObject(f);
                }
            }
            Op::Text(s, x, y, c, size) => unsafe {
                let tw = (s.chars().count() as i32 * (*size) + 60).min(self.w);
                self.force_alpha(*x, *y - *size - 4, *x + tw, *y + *size + 8);
                let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
                let f = CreateFontW(
                    -*size, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr(),
                );
                let old = SelectObject(self.mem, f);
                SetBkMode(self.mem, TRANSPARENT);
                SetTextColor(self.mem, rgb(*c));
                let wide: Vec<u16> = s.encode_utf16().collect();
                let mut rc = RectPos {
                    left: *x,
                    top: *y,
                    right: self.w,
                    bottom: self.h,
                };
                DrawTextW(self.mem, wide.as_ptr(), wide.len() as i32, &mut rc, 0);
                self.force_alpha(*x, *y - *size - 4, *x + tw, *y + *size + 8);
                SelectObject(self.mem, old);
                DeleteObject(f);
            },
        }
    }

    /// 把一块区域的 alpha 强制拉满。
    /// 为什么要它：GDI 画抗锯齿文字（ClearType）时会把像素的 alpha 写成 0，
    /// 而我们的窗口是 AC_SRC_ALPHA 合成 —— 结果是文字看起来"半透明/发白"，
    /// 叠在浅色截图上几乎看不见（owner 2026-11-18 报的"文字看不清"）。
    fn force_alpha(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let (x0, y0) = (x0.max(0), y0.max(0));
        let (x1, y1) = (x1.min(self.w), y1.min(self.h));
        let mut y = y0;
        while y < y1 {
            let mut x = x0;
            while x < x1 {
                let o = (y as usize * self.w as usize + x as usize) * 4;
                unsafe {
                    *self.bits.add(o + 3) = 255;
                }
                x += 1;
            }
            y += 1;
        }
    }

    /// 实心圆（像素）
    fn pdisc(&mut self, cx: i32, cy: i32, r: i32, c: u32) {
        for dy in -r..=r {
            let dx = (((r * r - dy * dy) as f32).sqrt()) as i32;
            self.pfill(cx - dx, cy + dy, cx + dx + 1, cy + dy + 1, c);
        }
    }

    /// 高斯模糊的近似：横向 + 纵向各一趟箱式模糊。
    /// 两趟已经看不出"方块感"，三趟以上肉眼无差别，但每帧重绘都要重算，
    /// 所以停在两趟（拖拽预览时也不掉帧）。数据源同样是**原始冻结图**。
    fn blur_region(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let (x0, y0) = (x.max(0), y.max(0));
        let x1 = (x + w).min(self.w);
        let y1 = (y + h).min(self.h);
        let (bw, bh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        if bw == 0 || bh == 0 {
            return;
        }
        let mut buf: Vec<[u8; 3]> = Vec::with_capacity(bw * bh);
        for yy in y0..y1 {
            for xx in x0..x1 {
                let o = (yy as usize * self.w as usize + xx as usize) * 4;
                unsafe {
                    let p = self.base.as_ptr().add(o);
                    buf.push([*p, *p.add(1), *p.add(2)]);
                }
            }
        }
        let r = 6i32;
        let mut tmp = vec![[0u8; 3]; bw * bh];
        // 横向
        for yy in 0..bh {
            for xx in 0..bw {
                let lo = (xx as i32 - r).max(0) as usize;
                let hi = ((xx as i32 + r) as usize).min(bw - 1);
                let (mut sr, mut sg, mut sb, mut n) = (0u32, 0u32, 0u32, 0u32);
                for k in lo..=hi {
                    let p = buf[yy * bw + k];
                    sr += p[0] as u32;
                    sg += p[1] as u32;
                    sb += p[2] as u32;
                    n += 1;
                }
                tmp[yy * bw + xx] = [(sr / n) as u8, (sg / n) as u8, (sb / n) as u8];
            }
        }
        // 纵向 → 写回
        for yy in 0..bh {
            let lo = (yy as i32 - r).max(0) as usize;
            let hi = ((yy as i32 + r) as usize).min(bh - 1);
            for xx in 0..bw {
                let (mut sr, mut sg, mut sb, mut n) = (0u32, 0u32, 0u32, 0u32);
                for k in lo..=hi {
                    let p = tmp[k * bw + xx];
                    sr += p[0] as u32;
                    sg += p[1] as u32;
                    sb += p[2] as u32;
                    n += 1;
                }
                let o = ((y0 as usize + yy) * self.w as usize + (x0 as usize + xx)) * 4;
                unsafe {
                    let q = self.bits.add(o);
                    *q = (sr / n) as u8;
                    *q.add(1) = (sg / n) as u8;
                    *q.add(2) = (sb / n) as u8;
                }
            }
        }
    }

    /// 马赛克：把区域按 10px 块取平均（数据源是**原始冻结图**，不是已经压暗的）
    fn mosaic(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let bs = 10i32;
        let mut by = 0;
        while by < h {
            let mut bx = 0;
            while bx < w {
                let (mut sr, mut sg, mut sb, mut n) = (0u32, 0u32, 0u32, 0u32);
                let (x0, y0) = (x + bx, y + by);
                for yy in y0..(y0 + bs).min(y + h) {
                    for xx in x0..(x0 + bs).min(x + w) {
                        if xx < 0 || yy < 0 || xx >= self.w || yy >= self.h {
                            continue;
                        }
                        let o = ((yy as usize) * (self.w as usize) + xx as usize) * 4;
                        unsafe {
                            let p = self.base.as_ptr().add(o);
                            sb += *p as u32;
                            sg += *p.add(1) as u32;
                            sr += *p.add(2) as u32;
                        }
                        n += 1;
                    }
                }
                if n > 0 {
                    let (r, g, b) = ((sr / n) as u8, (sg / n) as u8, (sb / n) as u8);
                    for yy in y0..(y0 + bs).min(y + h) {
                        for xx in x0..(x0 + bs).min(x + w) {
                            if xx < 0 || yy < 0 || xx >= self.w || yy >= self.h {
                                continue;
                            }
                            let o = ((yy as usize) * (self.w as usize) + xx as usize) * 4;
                            unsafe {
                                let p = self.bits.add(o);
                                *p = b;
                                *p.add(1) = g;
                                *p.add(2) = r;
                            }
                        }
                    }
                }
                bx += bs;
            }
            by += bs;
        }
    }

    /// 画笔半径。跟着第二行的粗细档走 —— 否则画笔永远同一个粗细，
    /// 用户改粗细却看不出变化，会以为控件坏了。
    fn brush_r(&self) -> i32 {
        self.width + 8
    }

    /// 把"起点 + 当前点 + 路径"变成一条标注。
    /// 拖动过程（预览）和松手（落库）都走它 —— 以前两边各写一份 match，
    /// 加一种工具就要改两处，漏一处就"预览有、松手没了"。
    fn make_op(&self, a: (i32, i32), b: (i32, i32), pts: &[(i32, i32)]) -> Option<Op> {
        let r = norm4(a, b);
        match self.tool {
            Tool::Rect => Some(Op::Rect(r, self.color, self.width, self.fill)),
            Tool::Ellipse => Some(Op::Ellipse(r, self.color, self.width, self.fill)),
            Tool::Line => Some(Op::Line(a, b, self.color, self.width)),
            Tool::Arrow => Some(Op::Arrow(a, b, self.color, self.width)),
            Tool::Pen => Some(Op::Pen(pts.to_vec(), self.color, self.width)),
            Tool::Marker => Some(Op::Marker(pts.to_vec(), self.color, self.width + 8)),
            Tool::Mosaic => {
                if self.brush {
                    /* 画笔式：路径经过的地方都变成马赛克。形态在第二行选项行切，
                       再点一次工具按钮也能切（老手势保留）。 */
                    let mut p = pts.to_vec();
                    p.push(b);
                    Some(Op::MosaicPath(p, self.brush_r()))
                } else if r[2] > 2 || r[3] > 2 {
                    Some(Op::Mosaic(r))
                } else {
                    None
                }
            }
            Tool::Blur => {
                if self.brush {
                    /* 画笔式模糊（owner 2026-11-19 要的："高斯模糊也是如此"） */
                    let mut p = pts.to_vec();
                    p.push(b);
                    Some(Op::BlurPath(p, self.brush_r()))
                } else if r[2] > 3 && r[3] > 3 {
                    Some(Op::Blur(r))
                } else {
                    None
                }
            }
            /* 色块遮挡：拖出矩形，整块实心盖上去 */
            Tool::Block => {
                if r[2] > 2 && r[3] > 2 {
                    Some(Op::Block(r, self.color))
                } else {
                    None
                }
            }
            /* 引线气泡：拖出的矩形就是泡泡本体（最小 60×40 才像个泡泡） */
            Tool::Callout => {
                if r[2] > 12 && r[3] > 10 {
                    Some(Op::Callout(
                        [r[0], r[1], r[2].max(96), r[3].max(52)],
                        self.color,
                        String::new(),
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// 工具栏位置与尺寸（宽度由 `bar_w` 累加，尺寸见 `BTN_W` 那一段注释）。
    fn bar_layout(&mut self) {
        /* 原型 `.toolbar{padding:0 10px}` —— 左右各 10，不是各 5 */
        let mut bw = 20;
        let mut prev: Option<BarKind> = None;
        for k in BAR.iter().copied().chain(std::iter::once(BAR_TAIL)) {
            if let Some(p) = prev {
                bw += bar_gap(p, k);
            }
            bw += bar_w(k);
            prev = Some(k);
        }
        let bh = BAR_H;
        let (x, y) = match self.sel {
            Some([sx, sy, sw, sh]) if sw > 8 && sh > 8 => {
                /* 下方 14px（设计稿 §5.2）；放不下翻到上方，再不够就贴着顶边 */
                let y = if sy + sh + 14 + bh + 6 < self.h {
                    sy + sh + 14
                } else {
                    (sy - bh - 14).max(4)
                };
                /* v2：**居中于选区**，不再左对齐（左对齐时选区一窄，工具条就甩到右边） */
                (sx + sw / 2 - bw / 2, y)
            }
            _ => ((self.w - bw) / 2, self.h - bh - 56),
        };
        self.bar = [x.max(4).min((self.w - bw - 4).max(4)), y, bw, bh];
    }

    fn bar_len(&self) -> usize {
        BAR.len() + 1
    }

    fn bar_item(&self, i: usize) -> BarKind {
        if i < BAR.len() {
            BAR[i]
        } else {
            BAR_TAIL
        }
    }

    /// 第 i 项的左上角 x（顺序累加，画与点共用同一份）
    fn bar_item_x(&self, i: usize) -> i32 {
        let mut x = self.bar[0] + 10;
        for j in 0..i {
            if j > 0 {
                x += bar_gap(self.bar_item(j - 1), self.bar_item(j));
            }
            x += bar_w(self.bar_item(j));
        }
        x
    }

    /// 命中：返回按钮下标；点在分隔条上也返回它的下标（调用方忽略 Sep）。
    /// 没选区时工具栏不画，也不该命中 —— 否则会点到屏幕上根本看不见的按钮。
    fn bar_hit(&self, x: i32, y: i32) -> Option<usize> {
        if !self.has_sel() {
            return None;
        }
        let [_, by, _, bh] = self.bar;
        if y < by || y >= by + bh {
            return None;
        }
        for i in 0..self.bar_len() {
            let ix = self.bar_item_x(i);
            let w = bar_w(self.bar_item(i));
            if x >= ix && x < ix + w {
                return Some(i);
            }
        }
        None
    }

    /// 某一类属性胶囊的矩形（滚轮判断"是否停在粗细上"要用）
    fn chip_rect(&self, want: Chip) -> Option<[i32; 4]> {
        for i in 0..self.bar_len() {
            if self.bar_item(i) == BarKind::Chip(want) {
                return Some([
                    self.bar_item_x(i),
                    self.bar[1],
                    bar_w(BarKind::Chip(want)),
                    self.bar[3],
                ]);
            }
        }
        None
    }

    /// 组按钮对应的组下标
    fn group_of(&self, i: usize) -> Option<usize> {
        match self.bar_item(i) {
            BarKind::Group(g) => Some(g),
            _ => None,
        }
    }

    /// 某个工具属于哪一组（用于"组是否处于激活态"）
    fn group_of_tool(t: Tool) -> Option<usize> {
        GROUPS.iter().position(|g| g.contains(&t))
    }

    /// 选工具（键盘与点击共用）。遮罩类再选一次 = 切「框选 / 画笔」形态，老手势保留。
    fn pick_tool(&mut self, t: Tool) {
        if matches!(t, Tool::Mosaic | Tool::Blur) && self.tool == t {
            self.brush = !self.brush;
        } else {
            self.tool = t;
        }
        if let Some(g) = Self::group_of_tool(t) {
            self.face[g] = t;
        }
        self.open = None;
        self.tray_hover = None;
        self.palette = false;
        self.hover_due = None;
        self.render();
    }

    /* ---------- 托盘（悬停展开） ----------
       矩形算一次存进 `self.tray`，绘制（`draw_tray`）与命中（`tray_hit`）都读它。
       和 0.3.1–0.3.5 那套"面板"的区别：没有独立的打开状态，只有 `self.open` 一个下标。 */

    fn tray_layout(&mut self) {
        self.tray = [0; 4];
        let Some(i) = self.open else { return };
        let Some(g) = self.group_of(i) else {
            self.open = None;
            return;
        };
        let n = GROUPS[g].len() as i32;
        let tw = 10 + n * BTN_W + 2 * (n - 1);
        let th = 46;
        let cx = self.bar_item_x(i) + bar_w(self.bar_item(i)) / 2;
        let x = (cx - tw / 2).max(4).min((self.w - tw - 4).max(4));
        /* 默认在工具条上方 12px；工具条本身贴近屏幕顶时翻到下方 */
        let below = self.bar[1] < th + 24;
        let y = if below {
            self.bar[1] + self.bar[3] + 12
        } else {
            self.bar[1] - th - 12
        };
        self.tray = [x, y, tw, th];
    }

    fn tray_hit(&self, x: i32, y: i32) -> Option<usize> {
        let i = self.open?;
        let g = self.group_of(i)?;
        if !inside(self.tray, x, y) {
            return None;
        }
        let start = self.tray[0] + 5;
        for k in 0..GROUPS[g].len() {
            let ix = start + k as i32 * BTN_W;
            if x >= ix && x < ix + BTN_W {
                return Some(k);
            }
        }
        None
    }

    /* ---------- 调色板（21 色 7×3） ---------- */

    fn sw_layout(&mut self) {
        self.sw = [0; 4];
        if !self.palette {
            return;
        }
        let Some(bar) = self.chip_rect(Chip::Color) else {
            return;
        };
        let rows = (SWATCHES.len() as i32 + SW_COLS - 1) / SW_COLS;
        let w = SW_PAD * 2 + SW_COLS * SW_CELL + (SW_COLS - 1) * SW_GAP;
        let h = SW_PAD * 2 + rows * SW_CELL + (rows - 1) * SW_GAP;
        let cx = bar[0] + bar[2] / 2;
        let x = (cx - w / 2).max(4).min((self.w - w - 4).max(4));
        let below = self.bar[1] < h + 42;
        let y = if below {
            self.bar[1] + self.bar[3] + 30
        } else {
            self.bar[1] - h - 30
        };
        self.sw = [x, y, w, h];
    }

    fn sw_hit(&self, x: i32, y: i32) -> Option<u32> {
        /* 和 `bar_hit` 同一条原则：**没选区时这些东西根本不画，就不该命中** ——
           否则会点到屏幕上一块看不见的色板。回归测试早就这么断言了
           （`tray_and_palette_hit_each_item` 第 ③ 段），只是实现里漏了这一条。 */
        if !self.palette || !self.has_sel() || !inside(self.sw, x, y) {
            return None;
        }
        let lx = x - self.sw[0] - SW_PAD;
        let ly = y - self.sw[1] - SW_PAD;
        if lx < 0 || ly < 0 {
            return None;
        }
        let col = lx / (SW_CELL + SW_GAP);
        let row = ly / (SW_CELL + SW_GAP);
        /* 落在间隙里不算命中 */
        if lx % (SW_CELL + SW_GAP) >= SW_CELL || ly % (SW_CELL + SW_GAP) >= SW_CELL {
            return None;
        }
        let idx = row * SW_COLS + col;
        SWATCHES.get(idx as usize).copied()
    }

    /* ---------- 悬停状态机 ----------
       `hover_due` 是**唯一**的打开/收起来源：鼠标移动只改截止时间，WM_TIMER 到点落地。
       这样快速横穿不会弹一串托盘（<110ms 的悬停根本到不了截止时间）。 */

    fn update_bar_hover(&mut self, x: i32, y: i32) -> bool {
        if !self.has_sel() {
            return self.hover.take().is_some()
                || self.open.take().is_some()
                || self.hover_due.take().is_some()
                || self.tray_hover.take().is_some();
        }
        let now = std::time::Instant::now();
        let mut changed = false;

        /* ① 托盘里：保持展开，只换条目高亮（同时取消收起计时） */
        if self.open.is_some() {
            if let Some(t) = self.tray_hit(x, y) {
                if self.tray_hover != Some(t) {
                    self.tray_hover = Some(t);
                    changed = true;
                }
                if self.hover.take().is_some() || self.hover_due.take().is_some() {
                    changed = true;
                }
                return changed;
            }
            /* ② 指针还停在组按钮上：也算"在组里" */
            if let Some(i) = self.bar_hit(x, y) {
                if Some(i) == self.open {
                    if self.tray_hover.take().is_some() || self.hover_due.take().is_some() {
                        changed = true;
                    }
                    return changed;
                }
            }
        }

        let hit = self
            .bar_hit(x, y)
            .filter(|i| !matches!(self.bar_item(*i), BarKind::Sep));
        if self.hover != hit {
            self.hover = hit;
            changed = true;
        }

        if let Some(i) = hit {
            if self.group_of(i).is_some() {
                /* 组按钮：110ms 后展开（已经开着就不动） */
                if self.open != Some(i) {
                    /* 已经为同一个组在倒计时：**不要重置**。鼠标在按钮上轻微抖动时
                       每个 mousemove 都重置的话，110ms 永远到不了，托盘就"展不开"。 */
                    let pending_same =
                        matches!(self.hover_due, Some((_, true, j)) if j == i);
                    if !pending_same {
                        self.hover_due =
                            Some((now + std::time::Duration::from_millis(OPEN_MS), true, i));
                        changed = true;
                    }
                } else {
                    self.hover_due = None;
                }
                return changed;
            }
        }

        /* ③ 其他任何位置：190ms 后收起 */
        if self.open.is_some() || self.hover_due.is_some() {
            self.hover_due = Some((now + std::time::Duration::from_millis(CLOSE_MS), false, 0));
            changed = true;
        }
        changed
    }

    /// WM_TIMER 到点：把 `hover_due` 落地。返回是否需要重绘。
    fn tick(&mut self) -> bool {
        let Some((due, open, idx)) = self.hover_due else {
            return false;
        };
        if std::time::Instant::now() < due {
            return false;
        }
        self.hover_due = None;
        if open {
            self.open = Some(idx);
            self.palette = false;
        } else {
            self.open = None;
            self.tray_hover = None;
        }
        true
    }

    /* ---------- 像素原语 ----------

       为什么不用 GDI 的 Line/Rectangle/画笔：实测在 layered DIB 上，GDI 的
       pen/brush 状态会残留（同一批调用里只有后半段生效），工具栏一半图标画不出来。
       直接写 DIB 内存没有这个问题，而且抗锯齿、线宽完全可控（40 行代码的事）。 */
    fn pput(&mut self, x: i32, y: i32, c: u32) {
        let (r, g, b) = (
            ((c >> 16) & 0xFF) as u8,
            ((c >> 8) & 0xFF) as u8,
            (c & 0xFF) as u8,
        );
        self.put(x, y, r, g, b, 1.0);
    }

    fn pfill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: u32) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.pput(x, y, c);
            }
        }
    }

    /// 带透明度的实心矩形（柔影、半透明底用）
    fn pfill_a(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: u32, a: f32) {
        let (r, g, b) = (
            ((c >> 16) & 0xFF) as u8,
            ((c >> 8) & 0xFF) as u8,
            (c & 0xFF) as u8,
        );
        for y in y0..y1 {
            for x in x0..x1 {
                self.put(x, y, r, g, b, a);
            }
        }
    }

    /* ---------- 抗锯齿原语（图标专用） ----------

       像素直绘是无抗锯齿的：斜线是阶梯、圆边是锯齿，放大到 31px 按钮里就很"糙"
       （owner 2026-11-18「画出来的太丑」）。这里补一层**覆盖率绘制**：
       按"像素中心到图形边缘的距离"给 alpha，边缘 1px 内平滑过渡。
       代价是每个图标几千次浮点 —— 12 个图标合计不到 0.5ms，60fps 预算内。

       只给图标用。工具栏底、圆角、选中块仍走硬边 `pfill_round`：
       那些是 34px 高的大色块，1px 台阶看不出来，没必要为它做距离场。 */

    /// 按覆盖率叠加一个像素（cov = 该像素被图形盖住的比例）
    fn put_cov(&mut self, x: i32, y: i32, c: u32, cov: f32) {
        let a = cov.clamp(0.0, 1.0);
        if a <= 0.002 {
            return;
        }
        let (r, g, b) = (
            ((c >> 16) & 0xFF) as u8,
            ((c >> 8) & 0xFF) as u8,
            (c & 0xFF) as u8,
        );
        self.put(x, y, r, g, b, a);
    }

    /// 抗锯齿线段（圆头由距离场天然得到）
    fn pline_aa(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, c: u32) {
        let r = (w * 0.5).max(0.5);
        let pad = r + 1.0;
        let minx = (x0.min(x1) - pad).floor() as i32;
        let maxx = (x0.max(x1) + pad).ceil() as i32;
        let miny = (y0.min(y1) - pad).floor() as i32;
        let maxy = (y0.max(y1) + pad).ceil() as i32;
        for y in miny..=maxy {
            for x in minx..=maxx {
                let d = dist_seg(x as f32 + 0.5, y as f32 + 0.5, x0, y0, x1, y1);
                let cov = r + 0.5 - d;
                if cov > 0.0 {
                    self.put_cov(x, y, c, cov);
                }
            }
        }
    }

    fn pframe_aa(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, c: u32) {
        self.pline_aa(x0, y0, x1, y0, w, c);
        self.pline_aa(x0, y1, x1, y1, w, c);
        self.pline_aa(x0, y0, x0, y1, w, c);
        self.pline_aa(x1, y0, x1, y1, w, c);
    }

    /// 半透明抗锯齿线段（图标的 `.55` 部件用）
    fn pline_aa_a(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, c: u32, a: f32) {
        let r = (w * 0.5).max(0.5);
        let pad = r + 1.0;
        let minx = (x0.min(x1) - pad).floor() as i32;
        let maxx = (x0.max(x1) + pad).ceil() as i32;
        let miny = (y0.min(y1) - pad).floor() as i32;
        let maxy = (y0.max(y1) + pad).ceil() as i32;
        for y in miny..=maxy {
            for x in minx..=maxx {
                let d = dist_seg(x as f32 + 0.5, y as f32 + 0.5, x0, y0, x1, y1);
                let cov = r + 0.5 - d;
                if cov > 0.0 {
                    self.put_cov(x, y, c, cov * a);
                }
            }
        }
    }

    /// 实心圆
    fn pdisc_aa(&mut self, cx: f32, cy: f32, rad: f32, c: u32) {
        let pad = rad + 1.0;
        for y in (cy - pad).floor() as i32..=(cy + pad).ceil() as i32 {
            for x in (cx - pad).floor() as i32..=(cx + pad).ceil() as i32 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cov = rad + 0.5 - d;
                if cov > 0.0 {
                    self.put_cov(x, y, c, cov);
                }
            }
        }
    }

    /// 圆环（椭圆用 `pring_aa_xy_a`）
    fn pring_aa(&mut self, cx: f32, cy: f32, rad: f32, lw: f32, c: u32) {
        self.pring_aa_xy_a(cx, cy, rad, rad, lw, c, 1.0);
    }

    /// 椭圆环：`radx`/`rady` 两个半径。用归一化距离判断厚度，
    /// 比"参数化取 64 段短线"更稳（短线段在扁椭圆上会露接缝）。
    /// `a` 是整体透明度（模糊图标的渐隐外环用）。
    fn pring_aa_xy_a(&mut self, cx: f32, cy: f32, radx: f32, rady: f32, lw: f32, c: u32, a: f32) {
        let r = (lw * 0.5).max(0.5);
        let pad = r + 1.0;
        let (rx, ry) = (radx.max(0.5), rady.max(0.5));
        for y in (cy - ry - pad).floor() as i32..=(cy + ry + pad).ceil() as i32 {
            for x in (cx - rx - pad).floor() as i32..=(cx + rx + pad).ceil() as i32 {
                let dx = (x as f32 + 0.5 - cx) / rx;
                let dy = (y as f32 + 0.5 - cy) / ry;
                /* 归一化椭圆距离 → 近似还原成像素距离（乘以较小半径），判断环厚 */
                let k = (dx * dx + dy * dy).sqrt();
                let d = (k - 1.0) * rx.min(ry);
                let cov = r + 0.5 - d.abs();
                if cov > 0.0 {
                    self.put_cov(x, y, c, cov * a);
                }
            }
        }
    }

    /// 实心多边形（4×4 超采样）。图形都很小，超采样比"算边缘覆盖率"简单且够准。
    fn ppoly_aa(&mut self, pts: &[(f32, f32)], c: u32) {
        if pts.len() < 3 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in pts {
            x0 = x0.min(*x);
            y0 = y0.min(*y);
            x1 = x1.max(*x);
            y1 = y1.max(*y);
        }
        for y in (y0 - 1.0).floor() as i32..=(y1 + 1.0).ceil() as i32 {
            for x in (x0 - 1.0).floor() as i32..=(x1 + 1.0).ceil() as i32 {
                let mut hits = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                        let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                        if point_in_poly(px, py, pts) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    self.put_cov(x, y, c, hits as f32 / 16.0);
                }
            }
        }
    }

    /// 实心多边形（可带整体透明度，给图标的半透明部件用）
    fn ppoly_aa_a(&mut self, pts: &[(f32, f32)], c: u32, alpha: f32) {
        if pts.len() < 3 || alpha <= 0.002 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in pts {
            x0 = x0.min(*x);
            y0 = y0.min(*y);
            x1 = x1.max(*x);
            y1 = y1.max(*y);
        }
        for y in (y0 - 1.0).floor() as i32..=(y1 + 1.0).ceil() as i32 {
            for x in (x0 - 1.0).floor() as i32..=(x1 + 1.0).ceil() as i32 {
                let mut hits = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                        let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                        if point_in_poly(px, py, pts) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    self.put_cov(x, y, c, hits as f32 / 16.0 * alpha);
                }
            }
        }
    }

    /// 虚线描边（`blur` 图标的外环：dash 1.6 / gap 3.2，viewBox 单位）
    fn pstroke_dashed(&mut self, pts: &[(f32, f32)], w: f32, c: u32, dash: f32, gap: f32) {
        if pts.len() < 2 {
            return;
        }
        let period = (dash + gap).max(0.01);
        let mut acc = 0.0f32; // 沿折线累计的弧长
        for seg in pts.windows(2) {
            let (ax, ay) = seg[0];
            let (bx, by) = seg[1];
            let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
            if len < 1e-5 {
                continue;
            }
            let step = 1.0f32; // 每 1 单位再判一次虚实
            let n = (len / step).ceil() as i32;
            for k in 0..n {
                let t0 = k as f32 / n as f32;
                let t1 = (k + 1) as f32 / n as f32;
                let phase = (acc + len * t0) % period;
                if phase < dash {
                    self.pline_aa(
                        ax + (bx - ax) * t0,
                        ay + (by - ay) * t0,
                        ax + (bx - ax) * t1,
                        ay + (by - ay) * t1,
                        w,
                        c,
                    );
                }
            }
            acc += len;
        }
    }

    /// 圆角矩形填充。
    ///
    /// 为什么自己算而不是画"四个圆 + 两个矩形"拼：拼接会在圆与矩形的接缝处留下
    /// 一列重复像素（半透明时尤其明显，出现一道竖纹）。逐行算左右内缩只多 6 行代码，
    /// 而且**同一条曲线**能直接给柔影复用（透明度版）。
    /// 没有抗锯齿：半径只有 8px、屏幕上是 1px 台阶，肉眼看不出来，但省掉一张 mask。
    fn pfill_round(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: i32, c: u32) {
        self.pfill_round_a(x0, y0, x1, y1, r, c, 1.0);
    }

    /// 圆角矩形**描边**（1px 内描边 / 选中环）。
    ///
    /// 没有现成的"圆角环"原语：`pring_aa` 只画正圆，`pframe` 只画直角。
    /// 这里把圆角矩形的轮廓按 4 段直线 + 4 段 1/4 圆弧采样成折线，逐段 `pline_aa_a`，
    /// 好处是粗细和抗锯齿跟图标线条完全一致，不会出现"框比图标糙"的割裂感。
    fn pstroke_round_a(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: i32, c: u32, a: f32) {
        let (fx0, fy0) = (x0 as f32 + 0.5, y0 as f32 + 0.5);
        let (fx1, fy1) = (x1 as f32 - 0.5, y1 as f32 - 0.5);
        let rf = (r as f32 - 0.5).max(0.0);
        let mut pts: Vec<(f32, f32)> = Vec::with_capacity(40);
        /* 四个角：右下 → 左下 → 左上 → 右上，顺时针采样 90° */
        let corners = [
            (fx1 - rf, fy1 - rf, 0.0_f32),
            (fx0 + rf, fy1 - rf, 90.0),
            (fx0 + rf, fy0 + rf, 180.0),
            (fx1 - rf, fy0 + rf, 270.0),
        ];
        for (cx, cy, a0) in corners {
            for k in 0..=6 {
                let deg = a0 + 90.0 * (k as f32 / 6.0);
                let rad = deg.to_radians();
                pts.push((cx + rf * rad.cos(), cy + rf * rad.sin()));
            }
        }
        for w in pts.windows(2) {
            self.pline_aa_a(w[0].0, w[0].1, w[1].0, w[1].1, 1.0, c, a);
        }
    }

    fn pfill_round_a(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: i32, c: u32, a: f32) {
        let r = r.min((x1 - x0) / 2).min((y1 - y0) / 2).max(0);
        let rr = r as f32;
        for y in y0..y1 {
            /* 像素中心到圆心的纵向距离；只有落在上下两个 r 区间里才需要缩进 */
            let dy = if y < y0 + r {
                (y0 + r) as f32 - (y as f32 + 0.5)
            } else if y >= y1 - r {
                (y as f32 + 0.5) - (y1 - r) as f32
            } else {
                0.0
            };
            let inset = if dy > 0.0 {
                (rr - (rr * rr - dy * dy).max(0.0).sqrt()).round() as i32
            } else {
                0
            };
            if x1 - inset > x0 + inset {
                self.pfill_a(x0 + inset, y, x1 - inset, y + 1, c, a);
            }
        }
    }

    fn pline(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, lw: i32, c: u32) {
        self.pline_a(x0, y0, x1, y1, lw, c, 1.0);
    }

    /// 粗线（带透明度：记号笔 55%）
    fn pline_a(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, lw: i32, c: u32, a: f32) {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;
        let (mut x, mut y) = (x0, y0);
        let h = lw.max(1) / 2;
        let (cr, cg, cb) = (
            ((c >> 16) & 0xFF) as u8,
            ((c >> 8) & 0xFF) as u8,
            (c & 0xFF) as u8,
        );
        loop {
            for yy in (y - h)..(y - h + lw.max(1)) {
                for xx in (x - h)..(x - h + lw.max(1)) {
                    self.put(xx, yy, cr, cg, cb, a);
                }
            }
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }

    fn pframe(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, lw: i32, c: u32) {
        self.pline(x0, y0, x1, y0, lw, c);
        self.pline(x0, y1, x1, y1, lw, c);
        self.pline(x0, y0, x0, y1, lw, c);
        self.pline(x1, y0, x1, y1, lw, c);
    }

    fn pellipse(&mut self, cx: i32, cy: i32, rx: i32, ry: i32, lw: i32, c: u32) {
        let steps = 72;
        let mut prev: Option<(i32, i32)> = None;
        for i in 0..=steps {
            let a = i as f32 / steps as f32 * std::f32::consts::TAU;
            let x = cx + (rx as f32 * a.cos()).round() as i32;
            let y = cy + (ry as f32 * a.sin()).round() as i32;
            if let Some((px, py)) = prev {
                self.pline(px, py, x, y, lw, c);
            }
            prev = Some((x, y));
        }
    }

    /// 实心椭圆（形状填充用）：逐行算 x 跨度再叠 20% 透明色。
    /// 不用 GDI 的 Ellipse + 画刷：这块 layered DIB 上画刷状态会残留（见像素原语那段的说明）。
    fn fill_ellipse(&mut self, cx: i32, cy: i32, rx: i32, ry: i32, c: u32, a: f32) {
        if rx <= 0 || ry <= 0 {
            return;
        }
        for dy in -ry..=ry {
            let t = 1.0 - (dy as f32 / ry as f32).powi(2);
            if t <= 0.0 {
                continue;
            }
            let dx = (rx as f32 * t.sqrt()).round() as i32;
            if dx <= 0 {
                continue;
            }
            self.pfill_a(cx - dx, cy + dy, cx + dx, cy + dy + 1, c, a);
        }
    }

    /// 实心多边形（扫描线）
    fn ppoly(&mut self, pts: &[(i32, i32)], c: u32) {
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for (_, y) in pts {
            lo = lo.min(*y);
            hi = hi.max(*y);
        }
        for y in lo..=hi {
            let mut xs: Vec<f32> = Vec::new();
            for i in 0..pts.len() {
                let (x0, y0) = pts[i];
                let (x1, y1) = pts[(i + 1) % pts.len()];
                if (y0 <= y && y1 > y) || (y1 <= y && y0 > y) {
                    let t = (y - y0) as f32 / (y1 - y0) as f32;
                    xs.push(x0 as f32 + t * (x1 - x0) as f32);
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mut i = 0;
            while i + 1 < xs.len() {
                let a = xs[i].round() as i32;
                let b = xs[i + 1].round() as i32;
                if b > a {
                    self.pfill(a, y, b, y + 1, c);
                }
                i += 2;
            }
        }
    }

    /// 主工具条。配色与层次见 `Palette` 那一段。
    fn draw_bar(&mut self) {
        let [x, y, w, h] = self.bar;
        /* 分裂按钮两段共用一块底：hover 落在右段时左段也要跟着亮 */
        self.more_hover = (0..self.bar_len())
            .any(|i| self.hover == Some(i) && matches!(self.bar_item(i), BarKind::Act("copymore")));
        self.bar_plate(x, y, w, h, BAR_RADIUS);

        for i in 0..self.bar_len() {
            let k = self.bar_item(i);
            let ix = self.bar_item_x(i);
            let iw = bar_w(k);
            let hover = self.hover == Some(i);
            match k {
                BarKind::Sep => {
                    let px = ix + iw / 2;
                    self.pfill(px, y + 12, px + 1, y + h - 12, self.pal.sep);
                }
                BarKind::Tool(t) => {
                    let active = t == self.tool;
                    self.bar_btn(ix, y, iw, h, active, hover);
                    let ink = if active { self.pal.accent } else { self.pal.icon };
                    self.icon_px(ix + iw / 2, y + h / 2, t, ink);
                }
                BarKind::Group(g) => {
                    let active = Self::group_of_tool(self.tool) == Some(g);
                    let opened = self.open == Some(i);
                    self.bar_btn(ix, y, iw, h, active, hover || opened);
                    let t = self.face[g];
                    let ink = if active { self.pal.accent } else { self.pal.icon };
                    self.icon_px(ix + iw / 2, y + h / 2, t, ink);
                    /* 右下角 4px 三角 = "本组还有更多"；换过工具时变强调色 */
                    let cx = ix + iw - 9;
                    let cy = y + h - 12;
                    let tri = if t != GROUP_DEFAULT[g] {
                        self.pal.active
                    } else if active {
                        self.pal.accent
                    } else {
                        self.pal.icon
                    };
                    self.ppoly(&[(cx, cy + 4), (cx + 4, cy), (cx + 4, cy + 4)], tri);
                }
                BarKind::Chip(Chip::Color) => {
                    self.bar_btn(ix, y, iw, h, false, hover || self.palette);
                    let (cx, cy) = (ix + iw / 2, y + h / 2);
                    self.pdisc_aa(cx as f32, cy as f32, 9.5, self.color);
                    /* 白圈保证深色面板上也能看出圆点边界 */
                    self.pring_aa(cx as f32, cy as f32, 9.5, 1.6, self.pal.icon);
                }
                BarKind::Chip(Chip::Width) => {
                    self.bar_btn(ix, y, iw, h, false, hover);
                    /* 原型 `widthGlyph`：24 网格里三条线在 6.2 / 12 / 17.8，
                       线宽 = clamp(1.1 + w*0.26, 1.3, 3.9)，整组按 20px 图标框缩放。 */
                    let sc = 20.0 / 24.0;
                    let (cx, cy) = (ix as f32 + iw as f32 / 2.0, y as f32 + h as f32 / 2.0);
                    let lw = (1.1 + self.width as f32 * 0.26).clamp(1.3, 3.9) * sc;
                    for k in 0..3 {
                        let ly = cy + (6.2 + k as f32 * 5.8 - 12.0) * sc;
                        self.pline_aa(cx - 8.0 * sc, ly, cx + 8.0 * sc, ly, lw.max(1.0), self.pal.icon);
                    }
                }
                BarKind::Chip(Chip::Fill) => {
                    self.bar_btn(ix, y, iw, h, self.fill, hover);
                    let ink = if self.fill { self.pal.accent } else { self.pal.icon };
                    self.draw_icon(ix as f32 + iw as f32 / 2.0, y as f32 + h as f32 / 2.0, 20.0, "fill", ink);
                }
                BarKind::Act(a) => {
                    /* 复制是唯一实心主按钮（设计稿 §6.5）：accent 底 + 白图标。
                       它和右段 `copymore` 是**一颗分裂按钮**：底要连成一片，
                       两段之间只留一条 1px 白线（28%），中间不能有圆角缝。 */
                    if a == "copy" {
                        /* 原型 `.split`：高 36（在 56 的条里居中）、圆角 10、底 accent-solid。
                           左段 67 = 11 左内距 + 16 图标 + 6 间距 + 24 文本 + 10 右内距；右段 22。 */
                        let more_w = bar_w(BarKind::Act("copymore"));
                        let pill = iw + more_w;
                        let bh = 36.min(h);
                        let y0 = y + (h - bh) / 2;
                        let y1 = y0 + bh;
                        self.pfill_round(ix, y0, ix + pill, y1, SPLIT_RADIUS, self.pal.active);
                        if hover || self.more_hover {
                            self.pfill_round_a(ix, y0, ix + pill, y1, SPLIT_RADIUS, 0xFFFFFF, 0.14);
                        }
                        self.draw_icon(ix as f32 + 19.0, (y0 + y1) as f32 / 2.0, 16.0, "copy", 0xFFFFFF);
                        unsafe {
                            let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
                            let f = CreateFontW(-12, 0, 0, 0, 600, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
                            let oldf = SelectObject(self.mem, f);
                            SetBkMode(self.mem, TRANSPARENT);
                            SetTextColor(self.mem, 0xFFFFFF);
                            let ls: Vec<u16> = "复制".encode_utf16().collect();
                            let mut rc = RectPos {
                                left: ix + 33,
                                top: y0,
                                right: ix + iw - 2,
                                bottom: y1,
                            };
                            /* 0x20 = DT_SINGLELINE，0x4 = DT_VCENTER */
                            DrawTextW(self.mem, ls.as_ptr(), ls.len() as i32, &mut rc, 0x20 | 0x4);
                            SelectObject(self.mem, oldf);
                            DeleteObject(f);
                        }
                        /* 分隔线 = 原型右段的 `border-left`，**通高**（不是内缩 20px） */
                        let sx = ix + iw;
                        self.pfill_a(sx, y0, sx + 1, y1, 0xFFFFFF, 0.28);
                        self.draw_icon(
                            (sx + more_w / 2) as f32,
                            (y0 + y1) as f32 / 2.0,
                            12.0,
                            "chevron",
                            0xFFFFFF,
                        );
                    } else if a == "copymore" {
                        /* 底与图标已由左段一并画完，这里什么都不做（命中仍走这一格） */
                    } else {
                        if hover {
                            let bhh = 36.min(h - 8).max(24);
                            let yy0 = y + (h - bhh) / 2;
                            if a == "cancel" {
                                /* 原型 `.close:hover`：danger 14% 底 + danger 墨色，常态是普通墨色 */
                                self.pfill_round_a(
                                    ix + 2,
                                    yy0,
                                    ix + iw - 2,
                                    yy0 + bhh,
                                    BTN_RADIUS,
                                    self.pal.danger,
                                    0.14,
                                );
                            } else {
                                self.pfill_round(ix + 2, yy0, ix + iw - 2, yy0 + bhh, BTN_RADIUS, self.pal.hover);
                            }
                        }
                        let ink = if a == "cancel" && hover {
                            self.pal.danger
                        } else {
                            self.pal.icon
                        };
                        self.act_px(ix + iw / 2, y + h / 2, a, ink);
                    }
                }
            }
        }
        /* 图标是像素直绘（alpha 由 put 写满），但主条上还有分隔线等元素；一并固化更省心 */
        self.solidify([x, y, w, h]);
    }

    /// 悬停展开的托盘：5px 内距 + 36px 条目，条目数与组一致。
    fn draw_tray(&mut self) {
        if self.open.is_none() || self.tray[2] <= 0 {
            return;
        }
        let [tx, ty, tw, th] = self.tray;
        self.bar_plate(tx, ty, tw, th, 12);
        /* 原型 `.tray::after`：底部居中的 8px 指示三角（指回所属分组） */
        let ax = tx + tw / 2;
        let (ay, ac) = if ty > self.bar[1] {
            (ty, true) /* 弹在下方 → 三角朝上 */
        } else {
            (ty + th, false)
        };
        let edge = self.pal.edge;
        if ac {
            self.ppoly(&[(ax - 5, ay - 1), (ax + 5, ay - 1), (ax, ay + 5)], edge);
            self.ppoly(&[(ax - 4, ay), (ax + 4, ay), (ax, ay + 4)], self.pal.bg);
        } else {
            self.ppoly(&[(ax - 5, ay + 1), (ax + 5, ay + 1), (ax, ay - 5)], edge);
            self.ppoly(&[(ax - 4, ay), (ax + 4, ay), (ax, ay - 4)], self.pal.bg);
        }
        let Some(g) = self.open.and_then(|i| self.group_of(i)) else {
            return;
        };
        for (i, t) in GROUPS[g].iter().enumerate() {
            let ix = tx + 5 + i as i32 * (BTN_W + 2);
            let active = *t == self.tool;
            let hover = self.tray_hover == Some(i);
            self.bar_btn(ix, ty, BTN_W, th, active, hover);
            let ink = if active { 0xFFFFFF } else { self.pal.icon };
            self.icon_px(ix + BTN_W / 2, ty + th / 2, *t, ink);
        }
        self.solidify([tx, ty, tw, th]);
    }

    /// 调色板：21 色 7×3。选中态用 accent 描边（色块本身有颜色，填充会吃掉颜色）。
    /// 下拉位置：右对齐工具条右端、条下 8px（绘制与命中都读 `self.menu`）
    fn menu_layout(&mut self) {
        if !self.menu_open || !self.has_sel() {
            self.menu = [0; 4];
            return;
        }
        let [_, by, _, bh] = self.bar;
        let w = 186;
        let n = MENU_ITEMS.len() as i32;
        let h = 10 + n * MENU_ITEM_H + (n - 1) * MENU_GAP;
        /* `right:0` 相对分裂按钮：找右段定位 */
        let mut right = self.bar[0] + self.bar[2];
        for i in 0..self.bar_len() {
            if matches!(self.bar_item(i), BarKind::Act("copymore")) {
                right = self.bar_item_x(i) + bar_w(self.bar_item(i));
            }
        }
        self.menu = [(right - w).max(4), by + bh + 8, w, h];
    }

    fn menu_hit(&self, x: i32, y: i32) -> Option<usize> {
        if !self.menu_open || !inside(self.menu, x, y) {
            return None;
        }
        let step = MENU_ITEM_H + MENU_GAP;
        let rel = y - self.menu[1] - 5;
        if rel < 0 {
            return None;
        }
        /* 落在那 1px 缝里不算命中，避免"点缝隙误触发" */
        if rel % step >= MENU_ITEM_H {
            return None;
        }
        let i = rel / step;
        if (i as usize) < MENU_ITEMS.len() {
            Some(i as usize)
        } else {
            None
        }
    }

    fn draw_menu(&mut self) {
        if !self.menu_open || !self.has_sel() || self.menu[2] <= 0 {
            return;
        }
        let [x, y, w, h] = self.menu;
        self.bar_plate(x, y, w, h, 12);
        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        for (i, (label, key, _, icon)) in MENU_ITEMS.iter().enumerate() {
            let iy = y + 5 + i as i32 * (MENU_ITEM_H + MENU_GAP);
            /* 悬停底：和托盘条目同一套（居中、圆角、`pal.hover`）——
               不给反馈的话三行长得一模一样，用户不知道点下去会交付哪一条
               （owner 2026-11-21 截图报的"没有鼠标移过去的效果"）。 */
            if self.menu_hover == Some(i) {
                self.pfill_round(x + 5, iy, x + w - 5, iy + MENU_ITEM_H, 8, self.pal.hover);
            }
            /* 原型 `.menu-item`：左内距 9、图标 16、图标与文字间距 9 */
            self.draw_icon(
                x as f32 + 17.0,
                iy as f32 + MENU_ITEM_H as f32 / 2.0,
                16.0,
                icon,
                self.pal.icon,
            );
            unsafe {
                let f = CreateFontW(-12, 0, 0, 0, 400, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
                let oldf = SelectObject(self.mem, f);
                SetBkMode(self.mem, TRANSPARENT);
                let ls: Vec<u16> = label.encode_utf16().collect();
                SetTextColor(self.mem, rgb(self.pal.icon));
                let mut rc = RectPos {
                    left: x + 34,
                    top: iy,
                    right: x + w - 56,
                    bottom: iy + MENU_ITEM_H,
                };
                /* 0x20 = DT_SINGLELINE，0x4 = DT_VCENTER */
                DrawTextW(self.mem, ls.as_ptr(), ls.len() as i32, &mut rc, 0x20 | 0x4);
                /* 右侧键位：10px + 淡底胶囊（原型 `.menu-item kbd`） */
                let ks: Vec<u16> = key.encode_utf16().collect();
                let (kw, _) = self.text_metrics(&ks);
                let kx = x + w - 9 - (kw + 10);
                self.pfill_round(kx, iy + 9, kx + kw + 10, iy + 23, 4, self.pal.key_bg);
                SetTextColor(self.mem, rgb(self.pal.text3));
                let mut rk = RectPos {
                    left: kx + 5,
                    top: iy + 9,
                    right: kx + kw + 5,
                    bottom: iy + 23,
                };
                DrawTextW(self.mem, ks.as_ptr(), ks.len() as i32, &mut rk, 0x20 | 0x4);
                SelectObject(self.mem, oldf);
                DeleteObject(f);
            }
        }
        self.solidify([x, y, w, h]);
    }

    fn draw_palette(&mut self) {
        if !self.palette || self.sw[2] <= 0 {
            return;
        }
        let [x, y, w, h] = self.sw;
        self.bar_plate(x, y, w, h, 10);
        for (i, c) in SWATCHES.iter().enumerate() {
            let col = i as i32 % SW_COLS;
            let row = i as i32 / SW_COLS;
            let sx = x + SW_PAD + col * (SW_CELL + SW_GAP);
            let sy = y + SW_PAD + row * (SW_CELL + SW_GAP);
            if *c == self.color {
                self.pfill_round(
                    sx - 3,
                    sy - 3,
                    sx + SW_CELL + 3,
                    sy + SW_CELL + 3,
                    7,
                    self.pal.active,
                );
            }
            /* 浅色块必须有描边，否则落在白面板上等于隐形 */
            self.pfill_round(
                sx - 1,
                sy - 1,
                sx + SW_CELL + 1,
                sy + SW_CELL + 1,
                6,
                self.pal.edge,
            );
            self.pfill_round(sx, sy, sx + SW_CELL, sy + SW_CELL, 5, *c);
        }
        self.solidify([x, y, w, h]);
    }

    /// 一块浮层底（主条与展开面板共用）：柔影 → 1px 外沿 → 1px 内描边 → 底。
    /// 四层的必要性见 `Palette` 上面那段：只画其中一层，总有一类背景会糊掉；
    /// 深浅两版的"外沿/内描边"角色正好互换，所以顺序不变、只换色。
    /// 把一块矩形内的不透明像素的 alpha 压成 255。
    ///
    /// **为什么必须有这一步**：GDI 的 `DrawTextW` 只写 RGB，不动 32bpp DIB 的第 4 个字节；
    /// 而覆盖层是用 `UpdateLayeredWindow(ULW_ALPHA)` 上屏的 —— alpha=0 的字在真机上
    /// 直接透出桌面，看起来就是"字发虚 / 缺笔画 / 只显示一半"。
    /// 离屏预览是存 BMP（忽略 alpha），所以预览永远正常，这个 bug 只能在真机上看到，
    /// 之前几轮"字体看不清"的误判都是它。
    fn solidify(&mut self, r: [i32; 4]) {
        let (x0, y0) = (r[0].max(0), r[1].max(0));
        let (x1, y1) = ((r[0] + r[2]).min(self.w), (r[1] + r[3]).min(self.h));
        let row = self.w as usize * 4;
        for y in y0..y1 {
            let base = y as usize * row;
            for x in x0..x1 {
                unsafe { *self.bits.add(base + x as usize * 4 + 3) = 255 };
            }
        }
    }

    fn bar_plate(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32) {
        let (x1, y1) = (x + w, y + h);
        for k in 1..=3 {
            let a = 0.09 / k as f32;
            self.pfill_round_a(x - k, y - k + 2, x1 + k, y1 + k + 2, r + k, 0x000000, a);
        }
        self.pfill_round(x - 1, y - 1, x1 + 1, y1 + 1, r + 1, self.pal.edge_out);
        self.pfill_round(x, y, x1, y1, r, self.pal.edge);
        self.pfill_round(x + 1, y + 1, x1 - 1, y1 - 1, (r - 1).max(1), self.pal.bg);
    }

    /// 一个工具按钮的底：选中 / 悬停
    fn bar_btn(&mut self, ix: i32, y: i32, iw: i32, h: i32, active: bool, hover: bool) {
        /* 高亮是**居中 36px**（设计稿 §6.1），不是"贴着条上下各 3px"：
           条高 56 时后者会拉成一条 50px 的大色块，看着像整格被选中。 */
        let bh = 36.min(h - 8).max(24);
        let y0 = y + (h - bh) / 2;
        let (x0, x1, y1) = (ix + 2, ix + iw - 2, y0 + bh);
        if active {
            /* 原型 `.tool.is-active`：accent 16% 底 + 34% 1px 内环 + accent 墨色，
               不是"实心蓝块"。实心蓝只属于分裂按钮。 */
            self.pfill_round_a(x0, y0, x1, y1, BTN_RADIUS, self.pal.accent, 0.16);
            self.pstroke_round_a(x0, y0, x1, y1, BTN_RADIUS, self.pal.accent, 0.34);
        } else if hover {
            self.pfill_round(x0, y0, x1, y1, BTN_RADIUS, self.pal.hover);
        }
    }

    /// 12 个工具图标 —— 24px 网格、**图标框 22、线宽 1.8**、全部抗锯齿。
    ///
    /// 造型是 0.3.x 甲方认可的那一套（第一版 ±9px 硬像素"画得太丑"后重画的），
    /// 这一版只把图标框从 20 提到 22、线宽从 1.5 提到 1.8 —— 所以下面这些坐标
    /// 是**原值等比放大 1.1 后的结果**，不是重新设计。别按"看着更顺眼"再改形状。
    /// 画一个参考版图标：`size` 是渲染边长（viewBox 24 等比缩放）。
    /// stroke 1.6 是 viewBox 单位，缩放后约 1.33px（与原型 `index.html` 一致）。
    fn draw_icon(&mut self, cx: f32, cy: f32, size: f32, name: &str, ink: u32) {
        let parts = icon_parts(name);
        if parts.is_empty() {
            return;
        }
        let s = size / 24.0;
        let ox = cx - size * 0.5;
        let oy = cy - size * 0.5;
        let w = 1.6 * s;
        for (mode, alpha, d) in parts {
            let polys = svg_polys(d);
            for poly in &polys {
                if poly.len() < 2 {
                    continue;
                }
                let pts: Vec<(f32, f32)> = poly
                    .iter()
                    .map(|(x, y)| (ox + x * s, oy + y * s))
                    .collect();
                match mode {
                    1 => self.ppoly_aa_a(&pts, ink, *alpha),
                    2 => self.pstroke_dashed(&pts, w, ink, 1.6 * s, 3.2 * s),
                    _ => {
                        for seg in pts.windows(2) {
                            if *alpha >= 0.999 {
                                self.pline_aa(seg[0].0, seg[0].1, seg[1].0, seg[1].1, w, ink);
                            } else {
                                self.pline_aa_a(seg[0].0, seg[0].1, seg[1].0, seg[1].1, w, ink, *alpha);
                            }
                        }
                    }
                }
            }
        }
    }

    /// 工具图标 —— 直接映射到参考版的 icon id（`screenshot-toolbar-v2` §附录 A）
    fn icon_px(&mut self, cx: i32, cy: i32, t: Tool, ink: u32) {
        let name = match t {
            Tool::Select => "select",
            Tool::Rect => "rect",
            Tool::Ellipse => "ellipse",
            Tool::Line => "line",
            Tool::Arrow => "arrow",
            Tool::Pen => "pen",
            Tool::Marker => "marker",
            Tool::Mosaic => "mosaic",
            Tool::Blur => "blur",
            Tool::Text => "text",
            Tool::Number => "badge",
            /* 参考稿没有橡皮（它的"清除"是撤销）。功能保留，图标先借 `cross`，
               等参考稿补 `eraser` 再换一行。 */
            Tool::Eraser => "cross",
            Tool::Callout => "callout",
            Tool::Block => "block",
            Tool::Sticker => "sticker",
            Tool::Check => "check",
            Tool::Cross => "cross",
            Tool::Image => "image",
        };
        self.draw_icon(cx as f32, cy as f32, 20.0, name, ink);
    }

    /// 6 个动作图标（同一套线宽 1.8 与抗锯齿，别跟工具图标两种画风）
    /// 动作图标 —— 同样走参考版的图标表
    fn act_px(&mut self, cx: i32, cy: i32, a: &str, ink: u32) {
        let name = match a {
            "undo" => "undo",
            "redo" => "redo",
            "cancel" => "close",
            "pin" => "pin",
            "save" => "save",
            "copy" => "copy",
            "ocr" => "ocr",
            "share" => "share",
            "copymore" => "chevron",
            _ => "copy",
        };
        self.draw_icon(cx as f32, cy as f32, 20.0, name, ink);
    }

    /// 量一段文字的像素宽高（`DT_CALCRECT`）。中文只能靠 GDI 量 ——
    /// 像素直绘算不出字形宽度，气泡和快捷键面板的排版全靠这个。
    /// 前提：调用前已经把要用的字体选进 `mem`。
    fn text_metrics(&self, s: &[u16]) -> (i32, i32) {
        let mut rc = RectPos::default();
        unsafe {
            DrawTextW(self.mem, s.as_ptr(), s.len() as i32, &mut rc, DT_CALCRECT | DT_SINGLELINE);
        }
        ((rc.right - rc.left).max(1), (rc.bottom - rc.top).max(1))
    }

    /// 悬停气泡。主条与展开面板共用：`cx` 是目标按钮中心，`bottom` 是气泡要贴住的底边
    /// （主条/面板的顶边 —— 气泡永远画在它上面）。
    ///
    /// 两段两色：功能名用图标墨色、快捷键用第二级灰（设计稿 §01 的 tip）——
    /// 于是宽带要分两次量、分两次画，不能像以前拼成一个字符串。
    fn draw_tip(&mut self, k: BarKind, cx: i32, bottom: i32) {
        let (mut label, mut key) = bar_label(k);
        /* 遮罩类工具在气泡里直接报当前形态 —— 形态按钮在第二行，
           不写出来用户不知道工具现在是"拖框"还是"涂"。 */
        if let BarKind::Tool(t) = k {
            if matches!(t, Tool::Mosaic | Tool::Blur) {
                label = match (t, self.brush) {
                    (Tool::Mosaic, true) => "马赛克（画笔）",
                    (Tool::Mosaic, false) => "马赛克（矩形）",
                    (_, true) => "高斯模糊（画笔）",
                    (_, false) => "高斯模糊（矩形）",
                };
                key = "第二行可切形态";
            }
        }
        if label.is_empty() {
            return;
        }
        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        let lab: Vec<u16> = label.encode_utf16().collect();
        let key_s: Vec<u16> = key.encode_utf16().collect();
        let pal = self.pal;
        unsafe {
            /* -18 而不是 -16：owner 2026-11-18 说"气泡字看不清"。这台机器是 2560×1600 +
               125% 缩放，16px 物理字只相当于 12.8px 逻辑字，比系统正文小一档。 */
            let f = CreateFontW(-18, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let oldf = SelectObject(self.mem, f);
            SetBkMode(self.mem, TRANSPARENT);
            let (lab_w, th) = self.text_metrics(&lab);
            let (key_w, _) = self.text_metrics(&key_s);
            let gap = if key.is_empty() { 0 } else { 12 };
            let inner = lab_w + gap + key_w;
            let bx = (cx - inner / 2 - 8).max(4).min((self.w - inner - 20).max(4));
            let byy = (bottom - th - 20).max(4);
            /* 气泡的边界比工具栏面板再厚一档（外沿 + 内描边），因为它经常直接压在
               杂乱的截图内容上：浅色主题里只画一条 #e2e2dd 的淡边，白底上一融就"看不清"了。 */
            self.pfill_round(bx - 2, byy - 2, bx + inner + 18, byy + th + 12, 7, pal.edge_out);
            self.pfill_round(bx - 1, byy - 1, bx + inner + 17, byy + th + 11, 6, pal.edge);
            self.pfill_round(bx, byy, bx + inner + 16, byy + th + 10, 5, pal.bg);
            SetTextColor(self.mem, rgb(pal.icon));
            let mut trc = RectPos {
                left: bx + 8,
                top: byy + 5,
                right: bx + 8 + lab_w,
                bottom: byy + th + 5,
            };
            DrawTextW(self.mem, lab.as_ptr(), lab.len() as i32, &mut trc, DT_SINGLELINE);
            if !key.is_empty() {
                SetTextColor(self.mem, rgb(pal.text2));
                let mut krc = RectPos {
                    left: bx + 8 + lab_w + gap,
                    top: byy + 5,
                    right: bx + 8 + inner,
                    bottom: byy + th + 5,
                };
                DrawTextW(self.mem, key_s.as_ptr(), key_s.len() as i32, &mut krc, DT_SINGLELINE);
            }
            SelectObject(self.mem, oldf);
            DeleteObject(f);
            self.solidify([bx - 2, byy - 2, inner + 20, th + 14]);
        }
    }

    /// 气泡：托盘展开时只提示托盘里的条目（贴托盘上沿），否则提示主条上悬停的那一项。
    fn draw_bar_tip(&mut self) {
        if let Some(i) = self.open {
            if let Some(t) = self.tray_hover {
                if let Some(g) = self.group_of(i) {
                    let tool = GROUPS[g][t];
                    let cx = self.tray[0] + 5 + t as i32 * BTN_W + BTN_W / 2;
                    /* 托盘在工具条上方时气泡贴托盘上沿；托盘翻到下方时贴工具条上沿，
                       否则气泡会压住托盘本身。 */
                    let bottom = if self.tray[1] < self.bar[1] {
                        self.tray[1]
                    } else {
                        self.bar[1]
                    };
                    self.draw_tip(BarKind::Tool(tool), cx, bottom);
                }
            }
            return;
        }
        if let Some(i) = self.hover {
            let cx = self.bar_item_x(i) + bar_w(self.bar_item(i)) / 2;
            self.draw_tip(self.bar_item(i), cx, self.bar[1]);
        }
    }

    fn draw_sel(&mut self) {
        /* 其它已确认的块：1px 细蓝框（无把手）—— 多框选时每一块都看得见（0.4.24） */
        let act = self.sel;
        for i in 0..self.areas.len() {
            let r = self.areas[i];
            if Some(r) == act || r[2] <= 0 || r[3] <= 0 {
                continue;
            }
            self.pframe(r[0], r[1], r[0] + r[2], r[1] + r[3], 1, 0x6C3FD1);
        }
        let Some([x, y, w, h]) = self.sel else { return };
        if w <= 0 || h <= 0 {
            return;
        }
        // 2px 蓝框 + 四角把手（像素直绘，理由同上：GDI 画笔在这块 DIB 上不可靠）
        self.pframe(x, y, x + w, y + h, 2, 0x6C3FD1);
        for (cx, cy) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
            self.pfill(cx - 4, cy - 4, cx + 5, cy + 5, 0xFFFFFF);
            self.pframe(cx - 4, cy - 4, cx + 5, cy + 5, 1, 0x6C3FD1);
        }
        // 尺寸标签（文字只能是 GDI：中文/字形没法自己点像素）
        /* v2 的 HUD：宽高 + 屏幕坐标（等宽数字靠字体本身；缩放比拿不到——
           覆盖层全程物理像素，DPI 只在 main.rs 那边有）。 */
        let blocks = self.areas.len() + usize::from(!self.areas.contains(&[x, y, w, h]));
        let mut label = format!("{} × {}   x {}  y {}", w, h, self.ox + x, self.oy + y);
        /* 多框选时把块数写进 HUD：不然用户不知道自己已经加了几块、
           也不知道交付时会被竖着拼起来（owner 2026-11-21 要的"多次加入再合并"）。 */
        if blocks > 1 {
            label.push_str(&format!("   共 {} 块 · 纵向拼一张", blocks));
        }
        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        let wide: Vec<u16> = label.encode_utf16().collect();
        unsafe {
            let f = CreateFontW(-16, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let oldf = SelectObject(self.mem, f);
            SetBkMode(self.mem, TRANSPARENT);
            // 标签底衬：白字在浅色截图上会看不清，加一块半透明感的深底
            let mut m = RectPos {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            DrawTextW(
                self.mem,
                wide.as_ptr(),
                wide.len() as i32,
                &mut m,
                DT_CALCRECT | DT_SINGLELINE,
            );
            let tw = (m.right - m.left).max(10);
            let th = (m.bottom - m.top).max(12);
            let lx = x;
            /* 工具条翻到选区**上方**时，标签原来的位置会被它压住（bar 后画），
               这种情况把标签挪进选区内侧顶部。 */
            let bar_above = self.has_sel() && self.bar[1] + self.bar[3] <= y + 1;
            let ly = if bar_above {
                y + 6
            } else if y > th + 12 {
                y - th - 8
            } else {
                y + 4
            };
            self.pfill(lx, ly, lx + tw + 10, ly + th + 6, 0x1A1A1A);
            SetTextColor(self.mem, 0xFFFFFF);
            let mut rc = RectPos {
                left: lx + 5,
                top: ly + 3,
                right: lx + tw + 5,
                bottom: ly + th + 3,
            };
            DrawTextW(self.mem, wide.as_ptr(), wide.len() as i32, &mut rc, DT_SINGLELINE);
            SelectObject(self.mem, oldf);
            DeleteObject(f);
            self.solidify([lx, ly, tw + 10, th + 6]);
        }
    }

    fn present(&mut self) {
        unsafe {
            let hwnd = CUR.load(std::sync::atomic::Ordering::SeqCst);
            let size = Size {
                cx: self.w,
                cy: self.h,
            };
            let src = Point { x: 0, y: 0 };
            let blend = BlendFunction {
                blend_op: AC_SRC_OVER,
                blend_flags: 0,
                source_constant_alpha: 255,
                alpha_format: AC_SRC_ALPHA,
            };
            let mut r = RectPos::default();
            GetWindowRect(hwnd, &mut r);
            let dst = Point {
                x: r.left,
                y: r.top,
            };
            let sdc = GetDC(0);
            UpdateLayeredWindow(
                hwnd,
                sdc,
                &dst,
                &size,
                self.mem,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
            ReleaseDC(0, sdc);
        }
    }

    /// 是不是已经有"能用"的选区（>8px）。工具栏只在框选之后才出现。
    fn has_sel(&self) -> bool {
        matches!(self.sel, Some([_, _, w, h]) if w > 8 && h > 8)
    }

    fn render(&mut self) {
        self.bar_layout();
        self.tray_layout();
        self.menu_layout();
        self.sw_layout();
        self.fill_base();
        self.draw_ops();
        /* 正在拖的那一笔：先 take 出来画（draw_op 要 &mut self，而 Op 在 self 里会冲突），
           画完放回去。用户拖的时候就能看到结果，而不是松手才出现。 */
        let pv = self.preview.take();
        if let Some(op) = pv.as_ref() {
            self.draw_op(op);
        }
        self.preview = pv;
        /* 光标下窗口的强调色框：画在标注之上、选区框之下 —— 用户先看见"要截谁"，
           再看见选区把手。按下鼠标时 `win_hit` 已被清掉，所以不会有残影。 */
        self.draw_win_hit();
        self.draw_sel();
        /* 没选区就不画工具栏（owner 2026-11-18 当面提的）：按下 F1 的瞬间它飘在屏幕底部
           中间，既挡住内容、又没有任何可用动作。框选之后才贴到选区下方。 */
        if self.has_sel() {
            self.draw_bar();
            self.draw_tray();
            self.draw_menu();
            self.draw_palette();
            self.draw_bar_tip();
        }
        self.draw_keys();
        /* 放大镜永远画在最上层：它跟着光标走，压在工具栏/提示面板上也比被挡住有用
           （光标真正落到这些面板上时 `loupe_rect` 会返回 None）。 */
        self.draw_loupe();
        self.present();
    }

    /// 快捷键提示面板（照 Snipaste 放在**右下角**；设计稿 v2.1 §06 的键帽版式保留）。
    ///
    /// 为什么这轮又调：owner 2026-11-18 说"右下角的提示字太小，看不清楚"。
    /// 上一版说明文字只有 13px（125% 缩放下等于 10px 逻辑字），这次整体放大一档：
    /// 说明 15px、键帽 12px、行高 24、内距 12，键列仍锁 116px 保证竖线对齐。
    ///
    /// 中文只能走 `DrawTextW`（点像素画不了字形），所以顺序是"先量宽 → 算面板尺寸 → 再画"。
    fn draw_keys(&mut self) {
        const KCOL: i32 = 116; // 键列固定宽，说明列才能对齐
        const ROW_H: i32 = 24;
        const PAD: i32 = 12;
        const CAP_H: i32 = 18;

        let rows: [(&[&str], &str, &str); 7] = [
            (&["框外拖"], "再框一块（多框选）", "右键 移除当前块"),
            (&["悬停"], "组按钮展开更多工具", ""),
            (&["滚轮"], "线条粗细（Shift 4px）", "[ ] 也能调"),
            (&["F"], "形状填充", ""),
            (&["Ctrl", "A"], "选中整屏", "↑↓←→ 微调"),
            (&["Ctrl", "C"], "复制到剪贴板", "Ctrl+S 存到截图库"),
            (&["Esc"], "取消", "中键 贴图　　Ctrl+Z 撤销"),
        ];
        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        let pal = self.pal;
        unsafe {
            let f_key = CreateFontW(-12, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let f_txt = CreateFontW(-15, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let oldf = SelectObject(self.mem, f_key);
            SetBkMode(self.mem, TRANSPARENT);

            /* ① 键帽宽 = 字宽 + 左右各 4px，最小 18px（↑↓←→ 这种单字符靠这个保底） */
            let mut caps: Vec<Vec<i32>> = Vec::new();
            let mut cap_txt: Vec<Vec<Vec<u16>>> = Vec::new();
            for (keys, _, _) in rows {
                let mut ws = Vec::new();
                let mut ts = Vec::new();
                for k in keys {
                    let u: Vec<u16> = k.encode_utf16().collect();
                    ws.push((self.text_metrics(&u).0 + 8).max(18));
                    ts.push(u);
                }
                caps.push(ws);
                cap_txt.push(ts);
            }

            /* ② 说明文字改用 15px 量宽；次级说明前置 12px 间距 */
            SelectObject(self.mem, f_txt);
            let mut descs: Vec<(Vec<u16>, Vec<u16>, i32, i32)> = Vec::new();
            let mut inner_w = 0;
            for (_, desc, extra) in rows {
                let d: Vec<u16> = desc.encode_utf16().collect();
                let e: Vec<u16> = extra.encode_utf16().collect();
                let dw = self.text_metrics(&d).0;
                let ew = if e.is_empty() { 0 } else { self.text_metrics(&e).0 + 12 };
                /* +8 是右侧留白：不留的话最长那行（Ctrl+Shift+S 另存为…）会正好贴到面板边框上 */
                inner_w = inner_w.max(KCOL + dw + ew + 8);
                descs.push((d, e, dw, ew));
            }
            let pw = (inner_w + PAD * 2).min(self.w - 32);
            let ph = PAD * 2 + ROW_H * rows.len() as i32;
            /* 右下角，离屏幕边 24px（Snipaste 的提示位置） */
            let bx = (self.w - pw - 24).max(4);
            let by = (self.h - 24 - ph).max(4);
            self.keys = [bx, by, pw, ph];
            self.bar_plate(bx, by, pw, ph, 8);

            /* ③ 逐行画：键帽 → 说明 → 次级说明 */
            for i in 0..rows.len() {
                let ry = by + PAD + i as i32 * ROW_H;
                SelectObject(self.mem, f_key);
                let mut kx = bx + PAD;
                for (j, u) in cap_txt[i].iter().enumerate() {
                    let kw = caps[i][j];
                    let top = ry + (ROW_H - CAP_H) / 2;
                    self.pfill_round(kx, top, kx + kw, top + CAP_H, 3, pal.key_edge);
                    self.pfill_round(kx + 1, top + 1, kx + kw - 1, top + CAP_H - 1, 2, pal.key_bg);
                    SetTextColor(self.mem, rgb(pal.icon));
                    let mut rc = RectPos {
                        left: kx,
                        top,
                        right: kx + kw,
                        bottom: top + CAP_H,
                    };
                    DrawTextW(
                        self.mem,
                        u.as_ptr(),
                        u.len() as i32,
                        &mut rc,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                    kx += kw + 3;
                }

                SelectObject(self.mem, f_txt);
                let dx = bx + PAD + KCOL;
                SetTextColor(self.mem, rgb(pal.text2));
                let mut drc = RectPos {
                    left: dx,
                    top: ry,
                    right: bx + pw - PAD,
                    bottom: ry + ROW_H,
                };
                DrawTextW(
                    self.mem,
                    descs[i].0.as_ptr(),
                    descs[i].0.len() as i32,
                    &mut drc,
                    DT_VCENTER | DT_SINGLELINE,
                );
                if !descs[i].1.is_empty() {
                    SetTextColor(self.mem, rgb(pal.text3));
                    let mut erc = RectPos {
                        left: dx + descs[i].2 + 10,
                        top: ry,
                        right: bx + pw - PAD,
                        bottom: ry + ROW_H,
                    };
                    DrawTextW(
                        self.mem,
                        descs[i].1.as_ptr(),
                        descs[i].1.len() as i32,
                        &mut erc,
                        DT_VCENTER | DT_SINGLELINE,
                    );
                }
            }
            SelectObject(self.mem, oldf);
            DeleteObject(f_key);
            DeleteObject(f_txt);
            /* 面板上的字全是 GDI 写的，必须把这块的 alpha 压满，否则真机上字会透底 */
            self.solidify([bx, by, pw, ph]);
        }
    }

    fn handle_hit(&self, x: i32, y: i32) -> Option<usize> {
        let [sx, sy, sw, sh] = self.sel?;
        let pts = [(sx, sy), (sx + sw, sy), (sx, sy + sh), (sx + sw, sy + sh)];
        for (i, (cx, cy)) in pts.iter().enumerate() {
            if (x - cx).abs() <= 6 && (y - cy).abs() <= 6 {
                return Some(i);
            }
        }
        None
    }

    /* ---------------- 取色器 / 窗口识别 ---------------- */

    /// 读**冻结帧原始像素**（BGRA，`base`）。越界夹到边缘 —— 屏幕边上的取色不该返回黑。
    ///
    /// 绝不能读 `bits`：那是"压暗 + 标注"之后的合成结果，取色器会取到被压暗的颜色，
    /// 用户拿这串 RGB 去用（owner 的取色器就是给设计/开发对色值用的）会直接错。
    fn sample_base(&self, x: i32, y: i32) -> (u8, u8, u8) {
        if self.w <= 0 || self.h <= 0 {
            return (0, 0, 0);
        }
        let xx = x.clamp(0, self.w - 1);
        let yy = y.clamp(0, self.h - 1);
        let o = ((yy as usize) * (self.w as usize) + xx as usize) * 4;
        if o + 3 >= self.base.len() {
            return (0, 0, 0);
        }
        let p = unsafe { self.base.as_ptr().add(o) };
        unsafe { (*p.add(2), *p.add(1), *p) }
    }

    /// 这个点是不是压在要交互的面板上（放大镜要收起，别挡住按钮）。
    fn over_ui(&self, x: i32, y: i32) -> bool {
        (self.has_sel() && inside(self.bar, x, y))
            || (self.tray[2] > 0 && inside(self.tray, x, y))
            || (self.menu_open && inside(self.menu, x, y))
            || (self.palette && inside(self.sw, x, y))
            || inside(self.keys, x, y)
    }

    /// 放大镜面板矩形；None = 这一帧不显示。
    fn loupe_rect(&self) -> Option<[i32; 4]> {
        if self.editor != 0 {
            return None;
        }
        let (x, y) = self.mouse?;
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        if self.over_ui(x, y) {
            return None;
        }
        Some(loupe_at(x, y, self.w, self.h))
    }

    /// 光标下窗口矩形（客户区坐标，已 clamp 进虚拟桌面）。
    fn window_rect_at(&self, x: i32, y: i32) -> Option<[i32; 4]> {
        let h = unsafe { pick_window_hwnd(self.ox + x, self.oy + y) };
        if h == 0 {
            return None;
        }
        let r = unsafe { win_screen_rect(h) }?;
        clamp_rect(
            [r[0] - self.ox, r[1] - self.oy, r[2], r[3]],
            self.w,
            self.h,
        )
    }

    /// 这一刻该高亮的窗口。拖拽中 / 压在已有选区上 / 压在面板上都不识别：
    /// 前者是"按下即清高亮"，后两者是"正在改选区，别再闪一个框"。
    fn hover_window(&self, x: i32, y: i32) -> Option<[i32; 4]> {
        if self.editor != 0 || self.over_ui(x, y) {
            return None;
        }
        if self.areas.iter().any(|a| inside(*a, x, y)) {
            return None;
        }
        let r = self.window_rect_at(x, y)?;
        if Some(r) == self.sel {
            return None;
        }
        Some(r)
    }

    /// 取色器当前色值文本；放大镜没显示时 None（C 键退回「叉号图章」快捷键）。
    fn color_text(&self) -> Option<String> {
        let (x, y) = self.mouse?;
        self.loupe_rect()?;
        let (r, g, b) = self.sample_base(x, y);
        Some(if self.hex {
            fmt_hex(r, g, b)
        } else {
            fmt_rgb(r, g, b)
        })
    }

    /// 光标下窗口的 2px 强调色框。画在标注之上、选区框之下。
    fn draw_win_hit(&mut self) {
        let Some(r) = self.win_hit else { return };
        if r[2] <= 2 || r[3] <= 2 {
            return;
        }
        let c = self.pal.accent;
        self.pframe(r[0], r[1], r[0] + r[2], r[1] + r[3], 2, c);
    }

    /// 放大镜面板：取样网格 + 中心像素描边 + 坐标 + 色块/色值 + 两行提示。
    ///
    /// 取样走 `base`（冻结帧），所以压暗、窗口高亮、标注都不会污染取到的颜色。
    fn draw_loupe(&mut self) {
        let Some([px, py, pw, ph]) = self.loupe_rect() else {
            return;
        };
        let (mx, my) = match self.mouse {
            Some(v) => v,
            None => return,
        };
        let (cr, cg, cb) = self.sample_base(mx, my);

        /* 先把 23×17 个源像素抄进临时缓冲：draw 要 &mut self，不能一边借 base 一边 put。 */
        let sw = LOUPE_W / LOUPE_ZOOM;
        let sh = LOUPE_H / LOUPE_ZOOM;
        let sx0 = mx - sw / 2;
        let sy0 = my - sh / 2;
        let mut tile = Vec::with_capacity((sw * sh * 4) as usize);
        for yy in 0..sh {
            for xx in 0..sw {
                let (r, g, b) = self.sample_base(sx0 + xx, sy0 + yy);
                tile.extend_from_slice(&[r, g, b, 255]);
            }
        }

        self.bar_plate(px, py, pw, ph, 10);
        let gx = px + LOUPE_PAD;
        let gy = py + LOUPE_PAD;
        for yy in 0..LOUPE_H {
            let row = (yy / LOUPE_ZOOM) as usize * sw as usize;
            for xx in 0..LOUPE_W {
                let o = (row + (xx / LOUPE_ZOOM) as usize) * 4;
                self.put(gx + xx, gy + yy, tile[o], tile[o + 1], tile[o + 2], 1.0);
            }
        }
        /* 像素网格：每 8px 一条极淡的线。没有它 8 倍放大看着像一张模糊的图，
           用户分不清"取的是不是这一个像素"。 */
        for i in 1..sw {
            self.pline_a(
                gx + i * LOUPE_ZOOM,
                gy,
                gx + i * LOUPE_ZOOM,
                gy + LOUPE_H - 1,
                1,
                0x000000,
                0.10,
            );
        }
        for j in 1..sh {
            self.pline_a(
                gx,
                gy + j * LOUPE_ZOOM,
                gx + LOUPE_W - 1,
                gy + j * LOUPE_ZOOM,
                1,
                0x000000,
                0.10,
            );
        }
        self.pframe(gx - 1, gy - 1, gx + LOUPE_W, gy + LOUPE_H, 1, self.pal.edge);

        /* 中心像素描边：白色外框 + 黑色内框，中心那一像素本身要露出来（8×8）。
           参考 `Snip.tsx` 的 9px 取样框，但那里是白色实心，会盖住取到的颜色。 */
        let ccx = gx + LOUPE_W / 2;
        let ccy = gy + LOUPE_H / 2;
        self.pframe(ccx - 5, ccy - 5, ccx + 5, ccy + 5, 1, 0xFFFFFF);
        self.pframe(ccx - 4, ccy - 4, ccx + 4, ccy + 4, 1, 0x000000);

        /* 信息区：坐标 / 色块 + 色值 / 两行提示 */
        let iy = gy + LOUPE_H;
        let hy = iy + LOUPE_INFO_H;
        let coord = format!("({}, {})", self.ox + mx, self.oy + my);
        let value = if self.hex {
            fmt_hex(cr, cg, cb)
        } else {
            fmt_rgb(cr, cg, cb)
        };
        self.pfill(gx + 2, iy + 26, gx + 22, iy + 46, ((cr as u32) << 16) | ((cg as u32) << 8) | cb as u32);
        /* 边框用图标墨色：色块可能是纯黑，深色面板上用 `edge_out` 会整块看不见 */
        self.pframe(gx + 2, iy + 26, gx + 22, iy + 46, 1, self.pal.icon);

        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        unsafe {
            let f_small = CreateFontW(-12, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let f_val = CreateFontW(-15, 0, 0, 0, FW_BOLD, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let oldf = SelectObject(self.mem, f_small);
            SetBkMode(self.mem, TRANSPARENT);

            SelectObject(self.mem, f_small);
            SetTextColor(self.mem, rgb(self.pal.text2));
            let u: Vec<u16> = coord.encode_utf16().collect();
            let mut rc = RectPos {
                left: gx + 2,
                top: iy + 4,
                right: gx + LOUPE_W,
                bottom: iy + 24,
            };
            DrawTextW(self.mem, u.as_ptr(), u.len() as i32, &mut rc, DT_SINGLELINE);

            SelectObject(self.mem, f_val);
            SetTextColor(self.mem, rgb(self.pal.icon));
            let u2: Vec<u16> = value.encode_utf16().collect();
            let mut rc2 = RectPos {
                left: gx + 30,
                top: iy + 26,
                right: gx + LOUPE_W,
                bottom: iy + 48,
            };
            DrawTextW(
                self.mem,
                u2.as_ptr(),
                u2.len() as i32,
                &mut rc2,
                DT_SINGLELINE | DT_VCENTER,
            );

            SelectObject(self.mem, f_small);
            SetTextColor(self.mem, rgb(self.pal.text3));
            for (k, s) in ["按 C 复制颜色值", "按 Shift 切换 RGB / HEX"].iter().enumerate() {
                let u3: Vec<u16> = s.encode_utf16().collect();
                let mut rc3 = RectPos {
                    left: gx + 2,
                    top: hy + 1 + k as i32 * 16,
                    right: gx + LOUPE_W,
                    bottom: hy + 19 + k as i32 * 16,
                };
                DrawTextW(self.mem, u3.as_ptr(), u3.len() as i32, &mut rc3, DT_SINGLELINE);
            }
            SelectObject(self.mem, oldf);
            DeleteObject(f_val);
            DeleteObject(f_small);
        }
        /* GDI 写字只写 RGB，不写 alpha；不固化的话真机上字会透底（见 `solidify`）。 */
        self.solidify([px, py, pw, ph]);
    }
}

extern "system" {
    fn GetWindowRect(h: Hwnd, r: *mut RectPos) -> i32;
}

/* ---------------- 窗口识别（光标下的顶层窗口） ----------------

   为什么不能只调一次 `WindowFromPoint`：覆盖层本身是整屏 topmost 的 layered 窗口，
   那个 API 永远返回它自己。本进程的窗口要**排除**（截图覆盖层 / 贴图窗 / 托盘菜单 /
   启动页），于是拿到本进程句柄后必须沿 z 序往下找第一个"别人的、可见的、含这个点"的窗口。

   `GetAncestor(GA_ROOT)` 把子控件（按钮、输入框）归到顶层窗；窗口矩形优先用
   `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` —— 它不含 Win10+ 那圈
   不可见的 8px 缩放边框，截出来才是用户眼睛看到的窗口。 */

const GA_ROOT: u32 = 2;
const GW_HWNDNEXT: u32 = 2;
const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
/// 虚拟桌面 / UWP 挂起窗口会被 DWM "cloaked"：`IsWindowVisible` 仍为真，但用户看不见。
const DWMWA_CLOAKED: u32 = 14;

/// 桌面壳不算"可截的窗口"：桌面本身是整屏矩形，单击空白处会变成"截整屏"。
fn is_desktop_shell(h: Hwnd) -> bool {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return false;
    }
    let name = String::from_utf16_lossy(&buf[..n as usize]);
    matches!(name.as_str(), "Progman" | "WorkerW")
}

/// 这个窗口能不能被"识别"：别人家的、可见、没最小化、有尺寸、包含测试点。
unsafe fn window_ok(h: Hwnd, sx: i32, sy: i32) -> bool {
    if h == 0 {
        return false;
    }
    /* 排除本进程：覆盖层、贴图窗、托盘菜单、启动页全是我们的 */
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, &mut pid);
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    if IsWindowVisible(h) == 0 || IsIconic(h) != 0 {
        return false;
    }
    /* 别的虚拟桌面上的窗口：`IsWindowVisible` 仍是真，但截到的是看不见的窗口。
       DWM 查询失败时**不**排除（老系统没有这个属性），只有明确 cloaked 才跳过。 */
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(
        h,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut c_void,
        std::mem::size_of::<u32>() as u32,
    ) == 0
        && cloaked != 0
    {
        return false;
    }
    let mut r = RectPos::default();
    if GetWindowRect(h, &mut r) == 0 {
        return false;
    }
    if r.right <= r.left || r.bottom <= r.top {
        return false;
    }
    if sx < r.left || sx >= r.right || sy < r.top || sy >= r.bottom {
        return false;
    }
    !is_desktop_shell(h)
}

/// 窗口的真实可视矩形（屏幕坐标）。DWM 失败 / 返回空矩形时回落 `GetWindowRect`。
unsafe fn win_screen_rect(h: Hwnd) -> Option<[i32; 4]> {
    let mut r = RectPos::default();
    let mut ok = DwmGetWindowAttribute(
        h,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut r as *mut RectPos as *mut c_void,
        std::mem::size_of::<RectPos>() as u32,
    ) == 0;
    if !ok || r.right <= r.left || r.bottom <= r.top {
        r = RectPos::default();
        ok = GetWindowRect(h, &mut r) != 0;
    }
    if !ok || r.right <= r.left || r.bottom <= r.top {
        return None;
    }
    Some([r.left, r.top, r.right - r.left, r.bottom - r.top])
}

/// 屏幕坐标 (sx, sy) 下最顶层的"别人的窗口"句柄。
unsafe fn pick_window_hwnd(sx: i32, sy: i32) -> Hwnd {
    let hit = WindowFromPoint(PointI { x: sx, y: sy });
    if hit != 0 {
        let root = GetAncestor(hit, GA_ROOT);
        if window_ok(root, sx, sy) {
            return root;
        }
    }
    /* 覆盖层自己挡住了 WindowFromPoint → 从 z 序顶端往下走。
       `GetTopWindow(0)` 正常返回最顶层窗口；万一为空，就从覆盖层自己的下一个兄弟开始
       （覆盖层此时已经在 z 序里，两者等价）。
       4096 上限只是防 API 异常时死循环，正常桌面几十个窗口就结束。 */
    let mut h = GetTopWindow(0);
    if h == 0 {
        h = GetWindow(CUR.load(std::sync::atomic::Ordering::SeqCst), GW_HWNDNEXT);
    }
    let mut guard = 0;
    while h != 0 && guard < 4096 {
        if window_ok(h, sx, sy) {
            return h;
        }
        h = GetWindow(h, GW_HWNDNEXT);
        guard += 1;
    }
    0
}


/* ---------------- 文本输入 ----------------
   用真正的 EDIT 子控件，不用自己收 WM_CHAR —— 只有 EDIT 才吃输入法（中文标注要用）。 */

fn edit_class() -> &'static [u16] {
    &[69, 68, 73, 84, 0] // "EDIT"
}

impl Ctx {
    unsafe fn start_text(&mut self, x: i32, y: i32) {
        self.commit_text();
        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        let hwnd = CUR.load(std::sync::atomic::Ordering::SeqCst);
        let ed = CreateWindowExW(
            0,
            edit_class().as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | 0x0080_0000,
            x,
            y - 26,
            460,
            34,
            hwnd,
            0,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null_mut(),
        );
        if ed != 0 {
            let f = CreateFontW(-24, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            SendMessageW(ed, 0x0030, f as usize, 1); // WM_SETFONT
            SetFocus(ed);
            self.editor = ed;
            self.editing = Some((x, y));
        }
    }

    unsafe fn commit_text(&mut self) {
        if self.editor == 0 {
            return;
        }
        let n = GetWindowTextLengthW(self.editor);
        let mut buf = vec![0u16; (n + 1) as usize];
        GetWindowTextW(self.editor, buf.as_mut_ptr(), n + 1);
        let s = String::from_utf16_lossy(&buf[..n as usize]);
        DestroyWindow(self.editor);
        self.editor = 0;
        if let Some((x, y)) = self.editing.take() {
            if !s.trim().is_empty() {
                self.ops.push(Op::Text(s, x, y, self.color, 28));
                self.redo.clear();
            }
        }
        let hwnd = CUR.load(std::sync::atomic::Ordering::SeqCst);
        SetFocus(hwnd);
    }
}

/* ---------------- 交付 ---------------- */

fn crop_png(c: &Ctx, r: [i32; 4]) -> Option<Vec<u8>> {
    let (x, y, w, h) = (r[0].max(0), r[1].max(0), r[2], r[3]);
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for yy in y..(y + h).min(c.h) {
        for xx in x..(x + w).min(c.w) {
            let o = ((yy as usize) * (c.w as usize) + xx as usize) * 4;
            unsafe {
                let p = c.bits.add(o);
                rgba.extend_from_slice(&[*p.add(2), *p.add(1), *p, 255]);
            }
        }
    }
    screen::Shot {
        x: 0,
        y: 0,
        w: w as u32,
        h: h as u32,
        rgba,
    }
    .png()
    .ok()
}

/// 两个矩形有没有交叠（多框选：删块时判断标注还归不归剩下的块）
fn overlap4(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// 拼接的纯像素核心：`src` 是整屏 **BGRA** 缓冲，块按"先上后下、同行先左后右"排，
/// 一块接一块竖着摞，空隙不留；宽度取最宽那块，窄块右侧补全透明。
/// 不碰 GDI / 裸指针 —— 好让 `stack_tests` 直接单测像素。
fn stack_rows(src: &[u8], cw: i32, ch: i32, rs: &[[i32; 4]]) -> Option<(Vec<u8>, i32, i32)> {
    if cw <= 0 || ch <= 0 || rs.is_empty() {
        return None;
    }
    let mut blocks = rs.to_vec();
    blocks.sort_by_key(|r| (r[1], r[0]));
    let w = blocks.iter().map(|r| r[2]).max().unwrap_or(0);
    let h: i32 = blocks.iter().map(|r| r[3]).sum();
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    let mut oy = 0i32;
    for r in &blocks {
        let (x0, y0) = (r[0].max(0), r[1].max(0));
        let bw = r[2].min(cw - x0);
        let bh = r[3].min(ch - y0);
        if bw <= 0 || bh <= 0 {
            continue;
        }
        for yy in 0..bh as usize {
            let so = ((y0 as usize + yy) * (cw as usize) + x0 as usize) * 4;
            let dst = ((oy as usize + yy) * (w as usize)) * 4;
            let s = &src[so..so + (bw as usize) * 4];
            let d = &mut rgba[dst..dst + (bw as usize) * 4];
            for xx in 0..bw as usize {
                d[xx * 4] = s[xx * 4 + 2];
                d[xx * 4 + 1] = s[xx * 4 + 1];
                d[xx * 4 + 2] = s[xx * 4];
                d[xx * 4 + 3] = 255;
            }
        }
        oy += bh;
    }
    Some((rgba, w, oy))
}

/// 多框选的**纵向拼接**交付（0.4.24，owner 2026-11-21 定的方式）：
/// 块按先上后下、同行先左后右摞成一张长图，**块之间的空隙不留**（截聊天记录不留空行）。
/// 坐标取的是 `c.bits`（已压暗 + 已画标注）—— 和单块裁剪同源。
fn crop_png_stack(c: &Ctx, rs: &[[i32; 4]]) -> Option<(Vec<u8>, i32, i32)> {
    let src = unsafe { std::slice::from_raw_parts(c.bits, (c.w as usize) * (c.h as usize) * 4) };
    let (rgba, w, h) = stack_rows(src, c.w, c.h, rs)?;
    screen::Shot {
        x: 0,
        y: 0,
        w: w as u32,
        h: h as u32,
        rgba,
    }
    .png()
    .ok()
    .map(|png| (png, w, h))
}

/// **已经持有 CTX 锁时**用它（工具栏点击 / WM_KEYDOWN 都在锁内）。
/// 之前这些路径直接调 `deliver()`，而 `deliver` 内部又 `CTX.lock()` ——
/// std::sync::Mutex 不可重入，于是**整条 UI 线程死锁**：用户看到的就是
/// "Ctrl+C / Ctrl+S 按了没反应"「点复制按钮没反应」（0.3.0 的真实 bug）。
fn deliver_to(c: &mut Ctx, action: &str) {
    /* 埋点：按下生效的时刻（与 `am_snip_finish` 里的 `snip_*` 同一时钟） */
    crate::perf_tick("snip", "press");
    let sel = c.sel.unwrap_or([0, 0, c.w, c.h]);
    if sel[2] < 4 || sel[3] < 4 {
        return;
    }
    /* 交付前把画面重算成「冻结图（选区外压暗）+ 标注」，**不带选区框**。
       `c.bits` 是屏幕上的合成结果，里面有 2px 蓝框、四角 9×9 的白色把手、
       尺寸标签和工具栏 —— 直接裁剪的话这些全都跟着进 PNG。截图贴图四周
       那圈"边框"就是这么来的（owner 2026-11-19 反复报的那个）。
       重放标注用的是 `c.ops`，所以箭头/文字/涂鸦一个都不少。 */
    c.fill_base();
    c.draw_ops();
    /* 0.4.24 多框选：活动块若还没进集合（Ctrl+A / 移动 / 缩放路径），先入集合。
       之后要么单块（行为与老版本逐字节一致），要么多块纵向拼成一张。 */
    if !c.areas.contains(&sel) {
        c.areas.push(sel);
    }
    if c.areas.len() <= 1 {
        if let Some(png) = crop_png(c, sel) {
            c.done = Some((
                action.to_string(),
                png,
                [sel[0] as f64, sel[1] as f64, sel[2] as f64, sel[3] as f64],
            ));
        }
    } else {
        let mut rs = c.areas.clone();
        rs.sort_by_key(|r| (r[1], r[0]));
        rs.dedup();
        if let Some((png, w, h)) = crop_png_stack(c, &rs) {
            /* 贴图位置：落在最上面那一块的左上角，尺寸就是拼好的图 —— 不能再按包围盒，
               拼接后高度变了，照包围盒贴会被拉变形。 */
            let first = rs[0];
            c.done = Some((
                action.to_string(),
                png,
                [first[0] as f64, first[1] as f64, w as f64, h as f64],
            ));
        }
    }
    /* 交付失败（写盘出错等）时窗口还开着，把选区框补回来，别让用户看着一块没有框的画面。 */
    if c.done.is_none() {
        c.draw_sel();
    }
}

fn deliver(action: &str) {
    let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = g.as_mut() {
        deliver_to(c, action);
    }
}

/* ---------------- 事件 ---------------- */

unsafe fn on_mouse(down: bool, x: i32, y: i32) {
    let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
    let Some(c) = g.as_mut() else { return };
    if down {
        /* 光标位置 + 手势状态：放大镜跟手、单击截窗都靠这两个字段。
           `win_hit` 在按下的这一刻就清掉（owner 要求：进入拖拽/按下后不许留高亮残影）。 */
        c.mouse = Some((x, y));
        c.moved = false;
        c.press_win = None;
        let had_hit = c.win_hit.take().is_some();
        /* 有些按下分支会直接 return（拖把手 / 移动选区），不在这里补一帧的话，
           屏幕上会留着"按下前的窗口高亮框"，看起来像卡住。 */
        if had_hit {
            c.render();
        }
        /* 正在输入文字时，点屏幕任何地方 = 提交。
           以前只有 Enter 能提交，用户习惯性去点下一个工具，文字就卡在编辑框里。 */
        if c.editor != 0 {
            c.commit_text();
            c.render();
        }
        /* ① 调色板：它是一块浮层，最优先命中。
           点在面板内距/间隙上要"吃掉"，否则会顺手在选区里画一笔。 */
        if c.palette {
            if let Some(col) = c.sw_hit(x, y) {
                c.color = col;
                c.palette = false;
                c.render();
                return;
            }
            if inside(c.sw, x, y) {
                return;
            }
            /* 点在外面：**这里不急着关**。点的如果是颜色胶囊本身，下面 bar 分支会
               toggle 成关闭；点的是画布，则由末尾"点到别处"统一收。否则会出现
               "点胶囊想关、关了又立刻被 toggle 开"的抖动。 */
        }
        /* ② 托盘条目 */
        if let Some(t) = c.tray_hit(x, y) {
            let g = c.open.and_then(|i| c.group_of(i)).unwrap_or(0);
            let tool = GROUPS[g][t];
            /* 遮罩类有两个形态：再选一次同一个 = 切「矩形 / 画笔」（老手势保留） */
            if matches!(tool, Tool::Mosaic | Tool::Blur) && c.tool == tool {
                c.brush = !c.brush;
            } else {
                c.tool = tool;
            }
            c.face[g] = tool;
            c.open = None;
            c.tray_hover = None;
            c.hover_due = None;
            c.render();
            return;
        }
        /* 下拉菜单：命中条目就交付；点在菜单外则收起（不吞这一下，继续往下走） */
        if let Some(i) = c.menu_hit(x, y) {
            let act = MENU_ITEMS[i].2;
            c.menu_open = false;
            c.menu_hover = None;
            deliver_to(c, act);
            c.render();
            if c.done.is_some() {
                PostQuitMessage(0);
            }
            return;
        }
        if c.menu_open {
            c.menu_open = false;
            c.menu_hover = None;
            c.render();
        }
        // 主条：选择 | 5 组 | 颜色 粗细 填充 | 撤销 重做 | [复制|▾] | 取消
        if let Some(i) = c.bar_hit(x, y) {
            match c.bar_item(i) {
                BarKind::Sep => {}
                BarKind::Tool(t) => {
                    /* 遮罩类（马赛克 / 高斯模糊）有两个形态：拖矩形 / 当画笔涂。
                       再点一次同组工具切换（老手势保留）。 */
                    if matches!(t, Tool::Mosaic | Tool::Blur) && c.tool == t {
                        c.brush = !c.brush;
                    } else {
                        c.tool = t;
                    }
                    if let Some(g) = Ctx::group_of_tool(t) {
                        c.face[g] = t;
                    }
                    c.open = None;
                    c.tray_hover = None;
                    c.palette = false;
                    c.hover_due = None;
                }
                BarKind::Group(g) => {
                    /* 点组按钮 = **一定选中**该组当前那个工具（默认工具 / 上次用的），并收起托盘。
                       以前这里写成"托盘已经开着就只收托盘"，而悬浮 110ms 就会把托盘打开 ——
                       于是"先悬浮、再点"的人永远选不中，表现就是"这些按钮点不了/无法选中，
                       必须悬浮后再从折叠里挑"（owner 2026-11-21 报的）。
                       换组里的别的工具走悬浮（110ms 展开），这条手势本来就有。 */
                    c.tool = c.face[g];
                    c.open = None;
                    c.tray_hover = None;
                    c.palette = false;
                    c.hover_due = None;
                }
                BarKind::Chip(Chip::Color) => {
                    c.palette = !c.palette;
                    c.open = None;
                    c.hover_due = None;
                }
                BarKind::Chip(Chip::Width) => {
                    /* 单击不弹菜单：粗细只走滚轮 / [ ]（设计稿 §5.5） */
                }
                BarKind::Chip(Chip::Fill) => {
                    c.fill = !c.fill;
                }
                BarKind::Act(a) => match a {
                    "undo" => {
                        if let Some(op) = c.ops.pop() {
                            c.redo.push(op);
                        }
                    }
                    "redo" => {
                        if let Some(op) = c.redo.pop() {
                            c.ops.push(op);
                        }
                    }
                    "copy" => deliver_to(c, "copy"),
                    "copymore" => {
                        c.menu_open = !c.menu_open;
                        /* 每次开合都清一次：否则重开时还留着上次那一行的高亮 */
                        c.menu_hover = None;
                    }
                    "save" => deliver_to(c, "save"),
                    "pin" => deliver_to(c, "peek"),
                    _ => c.done = Some(("cancel".into(), Vec::new(), [0.0; 4])),
                },
            }
            let finish = c.done.is_some();
            c.render();
            /* 交付/取消：必须让消息循环退出。以前工具栏这条路**漏了这一步**，
               表现就是"点了复制/保存，画面闪一下然后什么都没发生"。 */
            if finish {
                PostQuitMessage(0);
            }
            return;
        }
        /* ③ 点到别处：浮层收起（选区手势照常开始） */
        if c.open.is_some() || c.palette || c.tray_hover.is_some() || c.hover_due.is_some() {
            c.open = None;
            c.tray_hover = None;
            c.palette = false;
            c.hover_due = None;
            c.render();
        }
        if let Some(i) = c.handle_hit(x, y) {
            c.drag = Drag::Resize(i);
            c.sel0 = c.sel.unwrap_or([x, y, 0, 0]);
            c.anchor = (x, y);
            return;
        }
        if let Some(sel) = c.sel {
            if inside(sel, x, y) && c.tool == Tool::Select {
                c.drag = Drag::MoveSel;
                c.sel0 = sel;
                c.anchor = (x, y);
                return;
            }
            if c.tool == Tool::Text {
            c.start_text(x, y);
            c.render();
            return;
        }
        /* 序号是"点一下放一个"，拖拽没意义（Snipaste 同款）。
           序号自动递增，撤销能整条撤掉（它就是一个普通 op）。 */
        if c.tool == Tool::Number && inside(sel, x, y) {
            let n = c.number_next;
            c.number_next += 1;
            c.ops.push(Op::Number(x, y, n, c.color));
            c.redo.clear();
            c.render();
            return;
        }
        /* 图章 / 贴纸同样是"点一下落一个"：拖拽对它没意义（参考稿 §5.4 的交互） */
        if matches!(c.tool, Tool::Check | Tool::Cross | Tool::Sticker) && inside(sel, x, y) {
            let k = match c.tool {
                Tool::Check => Stamp::Check,
                Tool::Cross => Stamp::Cross,
                _ => Stamp::Smiley,
            };
            c.ops.push(Op::Stamp(x, y, c.color, k));
            c.redo.clear();
            c.render();
            return;
        }
        if inside(sel, x, y) && c.tool != Tool::Select {
                c.drag = Drag::Draw;
                c.anchor = (x, y);
                c.cur = (x, y);
                c.pts = vec![(x, y)];
                return;
            }
        }
        /* 光标是不是落在**所有**已确认块之外（决定"开新块"还是"改老块"） */
        let outside_all = !c.areas.iter().any(|a| inside(*a, x, y));
        /* 选择工具点到**别的**已确认块里：把它切成活动块（能接着挪 / 拉把手 / 右键删）。
           没有这一步，第二块一框完第一块就"冻住"了 —— 只能右键删掉重框。 */
        if c.tool == Tool::Select && !outside_all {
            if let Some(a) = c.areas.iter().rev().copied().find(|a| inside(*a, x, y)) {
                c.sel = Some(a);
                c.sel0 = a;
                c.drag = Drag::MoveSel;
                c.anchor = (x, y);
                c.render();
                return;
            }
        }
        /* 起点落在**所有已确认块之外** = 开一块新的（多框选，0.4.24）。
           不必先切回选择工具：框外本来没有别的用途（标注只能画在块内）。
           `sel0` 必须在这里重置 —— 收尾靠它认"这次动的是哪一块"，留着上一次
           缩放的原矩形，新框会把上一块原地覆盖掉（多框选定案前实测踩到）。 */
        if c.tool == Tool::Select || (!c.areas.is_empty() && outside_all) {
            c.sel = Some([x, y, 0, 0]);
            c.sel0 = [x, y, 0, 0];
            c.drag = Drag::NewSel;
            c.anchor = (x, y);
            c.moved = false;
            /* 单击截窗的候选：按下时抓一次。抬起前若发生拖动（`moved`），这个候选作废。 */
            c.press_win = c.hover_window(x, y);
        }
    } else {
        /* 收尾要按**手势类型**分派（0.4.24 多框选）：以前 NewSel 与 Draw 共用一个分支、
           再回头看当前工具，于是"非选择工具在框外拖出的新块"会被当成画一笔。
           现在 NewSel 永远只改选区，Draw 永远只落一条标注。 */
        c.mouse = Some((x, y));
        let d = c.drag;
        match d {
            Drag::NewSel => {
                /* 单击（位移 < CLICK_SLOP）= 截"光标下那个窗口"：直接拿它的矩形当选区，
                   选区一出来工具栏就跟着出现，后面和自由框选完全同一条链路。
                   拖动超过阈值 = 老行为，自由框选。 */
                if !c.moved {
                    match c.press_win.take() {
                        Some(wr) => {
                            c.sel = Some(wr);
                            c.sel0 = wr;
                        }
                        None => c.sel = c.areas.last().copied(),
                    }
                    c.preview = None;
                } else {
                    let r = norm4(c.anchor, (x, y));
                    /* 太小的框不算一块：把活动块还给集合里最后一块，
                       工具栏和 HUD 不会因为"手抖点了一下"整条消失。 */
                    c.sel = if r[2] > 3 && r[3] > 3 {
                        Some(r)
                    } else {
                        c.areas.last().copied()
                    };
                    c.preview = None;
                }
            }
            Drag::Draw => {
                let a = c.anchor;
                let b = (x, y);
                let pts = c.pts.clone();
                if let Some(op) = c.make_op(a, b, &pts) {
                    c.ops.push(op);
                    c.redo.clear();
                }
                c.preview = None;
            }
            Drag::MoveSel => {
                let (dx, dy) = (x - c.anchor.0, y - c.anchor.1);
                let mut r = c.sel0;
                r[0] += dx;
                r[1] += dy;
                c.sel = Some(r);
            }
            Drag::Resize(i) => {
                let mut r = c.sel0;
                let (x1, y1) = (r[0] + r[2], r[1] + r[3]);
                match i {
                    0 => {
                        r[0] = c.sel0[0] + (x - c.anchor.0);
                        r[1] = c.sel0[1] + (y - c.anchor.1);
                        r[2] = x1 - r[0];
                        r[3] = y1 - r[1];
                    }
                    1 => {
                        r[1] = c.sel0[1] + (y - c.anchor.1);
                        r[2] = x - c.sel0[0];
                        r[3] = y1 - r[1];
                    }
                    2 => {
                        r[0] = c.sel0[0] + (x - c.anchor.0);
                        r[2] = x1 - r[0];
                        r[3] = y - c.sel0[1];
                    }
                    _ => {
                        r[2] = x - c.sel0[0];
                        r[3] = y - c.sel0[1];
                    }
                }
                if r[2] > 6 && r[3] > 6 {
                    c.sel = Some(r);
                }
            }
            _ => {}
        }
        c.drag = Drag::None;
        /* 活动选区 ↔ 块集合同步（0.4.24 多框选）：
           · 新拖出来的块（sel0 是起点，匹配不到集合）→ 入集合
           · 移动 / 缩放过的块（sel0 = 改动前的矩形）→ 原地替换
           **只认改选区的三种手势**：画标注也跑这段的话，`sel0`（上一次缩放留下的）
           会把某一块替换成本次选区，属于典型的"改了 A 坏了 B"。 */
        if matches!(d, Drag::NewSel | Drag::MoveSel | Drag::Resize(_)) {
            if let Some(s) = c.sel {
                match c.areas.iter().position(|&a| a == c.sel0) {
                    Some(i) => c.areas[i] = s,
                    None => {
                        if !c.areas.contains(&s) {
                            c.areas.push(s);
                        }
                    }
                }
            }
        }
        if let Some(op) = c.ops.last() {
            let _ = op.bbox();
        }
    }
    c.render();
}

unsafe fn on_move(x: i32, y: i32) {
    let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
    let Some(c) = g.as_mut() else { return };
    /* 放大镜跟着光标走：每移动一像素都要重画一帧。用 `painted` 收口，
       保证一次 mousemove 最多 render 一次（整屏重绘 4~8ms，翻倍会掉帧）。 */
    c.mouse = Some((x, y));
    let mut painted = false;
    if c.update_bar_hover(x, y) {
        c.render();
        painted = true;
    }
    /* 「复制 ▾」下拉的悬停：它挂在条下方，坐标不落在 bar/tray 上，所以单独记
       `menu_hover`（`menu_hit` 里已经排掉了 1px 缝隙，和点击用的是同一套几何）。 */
    let mh = c.menu_hit(x, y);
    if mh != c.menu_hover {
        c.menu_hover = mh;
        c.render();
        painted = true;
    }
    /* 窗口识别：拖拽中一律不识别（按下那一刻已经清空高亮，避免"上一个悬停框"残留）。 */
    let wh = if matches!(c.drag, Drag::None) {
        c.hover_window(x, y)
    } else {
        None
    };
    if wh != c.win_hit {
        c.win_hit = wh;
        c.render();
        painted = true;
    }
    match c.drag {
        Drag::NewSel => {
            /* 拖出阈值就标记为"真拖拽"，抬起时不再当成单击截窗 */
            if (x - c.anchor.0).abs() > CLICK_SLOP || (y - c.anchor.1).abs() > CLICK_SLOP {
                c.moved = true;
            }
            c.sel = Some(norm4(c.anchor, (x, y)));
            c.render();
            painted = true;
        }
        Drag::Draw => {
            c.cur = (x, y);
            if c.tool == Tool::Eraser {
                let r = [x - 12, y - 12, 24, 24];
                let before = c.ops.len();
                c.ops.retain(|op| {
                    let b = op.bbox();
                    !(b[0] < r[0] + r[2] && b[0] + b[2] > r[0] && b[1] < r[1] + r[3] && b[1] + b[3] > r[1])
                });
                if c.ops.len() != before {
                    c.redo.clear();
                }
            } else {
                c.pts.push((x, y));
                let pts = c.pts.clone();
                c.preview = c.make_op(c.anchor, (x, y), &pts);
            }
            c.render();
            painted = true;
        }
        Drag::MoveSel | Drag::Resize(_) => {
            // 复用 up 里的逻辑：这里直接算
            let sel0 = c.sel0;
            let anchor = c.anchor;
            let mut r = sel0;
            match c.drag {
                Drag::MoveSel => {
                    r[0] = sel0[0] + (x - anchor.0);
                    r[1] = sel0[1] + (y - anchor.1);
                }
                Drag::Resize(i) => {
                    let (x1, y1) = (sel0[0] + sel0[2], sel0[1] + sel0[3]);
                    match i {
                        0 => {
                            r[0] = sel0[0] + (x - anchor.0);
                            r[1] = sel0[1] + (y - anchor.1);
                            r[2] = x1 - r[0];
                            r[3] = y1 - r[1];
                        }
                        1 => {
                            r[1] = sel0[1] + (y - anchor.1);
                            r[2] = x - sel0[0];
                            r[3] = y1 - r[1];
                        }
                        2 => {
                            r[0] = sel0[0] + (x - anchor.0);
                            r[2] = x1 - r[0];
                            r[3] = y - sel0[1];
                        }
                        _ => {
                            r[2] = x - sel0[0];
                            r[3] = y - sel0[1];
                        }
                    }
                }
                _ => {}
            }
            if r[2] > 6 && r[3] > 6 {
                c.sel = Some(r);
                c.render();
                painted = true;
            }
        }
        _ => {}
    }
    /* 没走过任何需要重绘的分支：仍然要重画 —— 否则放大镜不跟手 */
    if !painted {
        c.render();
    }
    /* 悬停状态机有截止时间就挂 20ms 计时器；到点由 WM_TIMER 落地 */
    if c.hover_due.is_some() {
        SetTimer(c.hwnd, 1, 20, 0);
    }
}

unsafe extern "system" fn wnd_proc(h: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    match msg {
        WM_LBUTTONDOWN => {
            SetCapture(h);
            on_mouse(true, lo(l), hi(l));
            0
        }
        WM_LBUTTONUP => {
            on_mouse(false, lo(l), hi(l));
            ReleaseCapture();
            0
        }
        WM_MOUSEMOVE => {
            on_move(lo(l), hi(l));
            0
        }
        /* 悬停展开/收起的唯一落地口：到点才真的开/关，鼠标移动只改截止时间 */
        WM_TIMER => {
            let arm = {
                let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
                match g.as_mut() {
                    Some(c) => {
                        if c.tick() {
                            c.render();
                        }
                        c.hover_due.is_some()
                    }
                    None => false,
                }
            };
            if !arm {
                KillTimer(h, 1);
            }
            0
        }
        /* 粗细：只在「粗细胶囊」上滚轮才生效（设计稿 §5.5，不弹菜单） */
        WM_MOUSEWHEEL => {
            let mut pt = PointI {
                x: lo(l),
                y: hi(l),
            };
            ScreenToClient(h, &mut pt);
            let delta = (((w >> 16) & 0xFFFF) as i16) as i32;
            let shift = GetKeyState(0x10) < 0;
            let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(c) = g.as_mut() {
                if let Some(r) = c.chip_rect(Chip::Width) {
                    if inside(r, pt.x, pt.y) {
                        let step = if shift { 4 } else { 1 };
                        let d = if delta > 0 { step } else { -step };
                        c.width = (c.width + d).clamp(W_MIN, W_MAX);
                        c.render();
                    }
                }
            }
            0
        }
        WM_RBUTTONDOWN => {
            let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(c) = g.as_mut() {
                if c.sel.is_some() {
                    /* 0.4.24 多框选：右键 = **移除当前块**，其余块保留（继续加选）。
                       只删块不清标注是错的（owner 2026-11-21 报的"截图取消后，之前标记的
                       还在"）：箭头/框/文字画在整屏坐标系里，块没了它们照样画在压暗的
                       底图上。所以这里**连标注一起收**：包围盒碰不到剩下任何一块的 op
                       全部丢弃；把最后一块也移除时，等于"取消这次框选"，标注自然清空。 */
                    if let Some(cur) = c.sel {
                        c.areas.retain(|&a| a != cur);
                        let rest = c.areas.clone();
                        c.ops.retain(|op| rest.iter().any(|&a| overlap4(a, op.bbox())));
                        c.redo.clear();
                    }
                    /* 还有块就让它当活动块：整条工具栏不会因为"刚删掉一块"闪没 */
                    c.sel = c.areas.last().copied();
                    c.open = None;
                    c.tray_hover = None;
                    c.palette = false;
                    c.hover_due = None;
                    c.render();
                } else {
                    c.done = Some(("cancel".into(), Vec::new(), [0.0; 4]));
                }
            }
            0
        }
        WM_MBUTTONDOWN => {
            deliver("peek");
            0
        }
        WM_KEYDOWN => {
            let ctrl = GetKeyState(0x11) < 0;
            let shift = GetKeyState(0x10) < 0;
            let mut close = false;
            {
                let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(c) = g.as_mut() {
                    match w as u32 {
                        /* Shift：RGB ↔ HEX。长按会连发 WM_KEYDOWN（lparam bit30=1），
                           只认第一次，否则文本会疯狂来回跳。 */
                        0x10 => {
                            if (l as usize & 0x4000_0000) == 0 {
                                c.hex = !c.hex;
                                c.render();
                            }
                        }
                        0x1B => {
                            /* 先关浮层，再谈退出：托盘/调色板开着时 Esc 只收浮层 */
                            if c.palette || c.open.is_some() || c.hover_due.is_some() || c.menu_open {
                                c.palette = false;
                                c.open = None;
                                c.tray_hover = None;
                                c.hover_due = None;
                                c.menu_open = false;
                                c.menu_hover = None;
                                c.render();
                            } else {
                                c.done = Some(("cancel".into(), Vec::new(), [0.0; 4]))
                            }
                        }
                        /* [ ] 调粗细（Shift 4px），和滚轮同一套边界 */
                        0xDB | 0xDD => {
                            let step = if shift { 4 } else { 1 };
                            let d = if w as u32 == 0xDD { step } else { -step };
                            c.width = (c.width + d).clamp(W_MIN, W_MAX);
                            c.render();
                        }
                        0x46 => {
                            c.fill = !c.fill;
                            c.render();
                        }
                        0x5A if ctrl => {
                            if let Some(op) = c.ops.pop() {
                                c.redo.push(op);
                            }
                            c.render();
                        }
                        0x59 if ctrl => {
                            if let Some(op) = c.redo.pop() {
                                c.ops.push(op);
                            }
                            c.render();
                        }
                        // Ctrl+A：选中整屏（网页版同键）。多框选时**整屏替换**所有块
                        0x41 if ctrl => {
                            c.sel = Some([0, 0, c.w, c.h]);
                            c.sel0 = [0, 0, c.w, c.h];
                            c.areas = vec![[0, 0, c.w, c.h]];
                            c.render();
                        }
                        // Ctrl+Shift+S 另存为（自己选路径），Ctrl+S 存到截图库
                        0x53 if ctrl && shift => deliver_to(c, "saveas"),
                        0x53 if ctrl => deliver_to(c, "save"),
                        0x43 if ctrl => deliver_to(c, "copy"),
                        /* 取色器：C = 复制当前色值（owner 给的界面稿上就写着
                           「按 C 复制颜色值」）。**必须排在 Ctrl+C 之后**，否则会把
                           "复制截图"截胡（0.4.19 的坑）。
                           叉号图章的字母键让给取色（原 C → Z），两条都留在工具栏提示里。 */
                        0x43 if !ctrl => {
                            if let Some(text) = c.color_text() {
                                let _ = am_core::clipimg::set_text(&text);
                            }
                        }
                        /* Ctrl+P：菜单里写着「贴到桌面 Ctrl+P」，但这颗键**从来没人接**
                           （0.4.24 实测：菜单能点、Ctrl+P 按了没反应）。用户报的
                           "快捷键贴图用不了"里有一半就是它。必须放在 `0x50 if !ctrl`
                           （画笔）之前，否则又是一次"裸键截胡 Ctrl 组合"（见下面那条注释）。 */
                        0x50 if ctrl => deliver_to(c, "peek"),
                        /* 工具字母键（参考稿 §9）。**必须放在所有 `if ctrl` 分支之后**：
                           Rust 的 match 从上往下匹配，裸 `0x43 =>` 写在前面会把
                           Ctrl+C 截胡 —— 0.4.19 就是这个坑（复制、保存、全选全失效）。
                           再加 `if !ctrl` 双保险。 */
                        0x56 if !ctrl => c.pick_tool(Tool::Select),
                        0x52 if !ctrl => c.pick_tool(Tool::Rect),
                        0x45 if !ctrl => c.pick_tool(Tool::Ellipse),
                        0x4C if !ctrl => c.pick_tool(Tool::Line),
                        0x41 if !ctrl => c.pick_tool(Tool::Arrow),
                        0x50 if !ctrl => c.pick_tool(Tool::Pen),
                        0x48 if !ctrl => c.pick_tool(Tool::Marker),
                        0x54 if !ctrl => c.pick_tool(Tool::Text),
                        0x4E if !ctrl => c.pick_tool(Tool::Number),
                        0x4B if !ctrl => c.pick_tool(Tool::Callout),
                        0x4D if !ctrl => c.pick_tool(Tool::Mosaic),
                        0x42 if !ctrl => c.pick_tool(Tool::Blur),
                        0x47 if !ctrl => c.pick_tool(Tool::Block),
                        0x53 if !ctrl => c.pick_tool(Tool::Sticker),
                        0x58 if !ctrl => c.pick_tool(Tool::Check),
                        /* 叉号图章的字母键：C 让给取色器的"复制色值"（owner 2026-11-23），
                           这里换到 Z，工具栏上的键位提示同步改过。 */
                        0x5A if !ctrl => c.pick_tool(Tool::Cross),
                        0x49 if !ctrl => c.pick_tool(Tool::Image),
                        // 方向键：微调选区；Shift+方向键改大小（网页版同手感）
                        0x25 | 0x26 | 0x27 | 0x28 => {
                            if let Some(mut r) = c.sel {
                                let d = 1;
                                match w as u32 {
                                    0x25 => {
                                        if shift {
                                            r[2] -= d
                                        } else {
                                            r[0] -= d
                                        }
                                    }
                                    0x26 => {
                                        if shift {
                                            r[3] -= d
                                        } else {
                                            r[1] -= d
                                        }
                                    }
                                    0x27 => {
                                        if shift {
                                            r[2] += d
                                        } else {
                                            r[0] += d
                                        }
                                    }
                                    _ => {
                                        if shift {
                                            r[3] += d
                                        } else {
                                            r[1] += d
                                        }
                                    }
                                }
                                if r[2] > 4 && r[3] > 4 {
                                    c.sel = Some(r);
                                    c.render();
                                }
                            }
                        }
                        _ => {}
                    }
                    close = c.done.is_some();
                }
            }
            if close {
                PostQuitMessage(0);
            }
            0
        }
        WM_PAINT => 0,
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

/* ---------------- 入口 ---------------- */

pub struct Delivered {
    pub action: String,
    pub png: Vec<u8>,
    pub place: [f64; 4],
}

/// 阻塞运行一次截图会话。返回 None = 用户取消。**必须在有自己的线程里调用**
/// （这里会跑一条 Win32 消息循环）。
///
/// `theme` 是主窗的 `UI_THEME` 口径（0 未知 / 1 浅 / 2 深）。覆盖层必须跟主窗一个主题 ——
/// 否则深色软件里按 F1 弹出一块白工具栏，看着像别人的软件。
pub fn run(shot: &screen::Shot, theme: u8, keep: &[[i32; 4]]) -> Option<Delivered> {
    let (w, h) = (shot.w as i32, shot.h as i32);
    if w == 0 || h == 0 {
        return None;
    }
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let wc = WndClassW {
            style: 0,
            wnd_proc: Some(wnd_proc),
            cls_extra: 0,
            wnd_extra: 0,
            hinstance: inst,
            hicon: 0,
            hcursor: LoadCursorW(0, 32512 as *const u16), // IDC_ARROW
            hbr_background: 0,
            menu_name: std::ptr::null(),
            class_name: cls().as_ptr(),
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 && CUR.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            // 类已注册过也会返回 0，继续建窗
        }
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            cls().as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            shot.x,
            shot.y,
            w,
            h,
            0,
            0,
            inst,
            std::ptr::null_mut(),
        );
        if hwnd == 0 {
            return None;
        }
        CUR.store(hwnd, std::sync::atomic::Ordering::SeqCst);

        // DIB
        let mut bi = BitmapInfo::default();
        bi.header.bi_size = std::mem::size_of::<BitmapInfoHeader>() as u32;
        bi.header.bi_width = w;
        bi.header.bi_height = -h;
        bi.header.bi_planes = 1;
        bi.header.bi_bit_count = 32;
        let sdc = GetDC(0);
        let mem = CreateCompatibleDC(sdc);
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(mem, &bi, 0, &mut bits, 0, 0);
        if bmp == 0 || bits.is_null() {
            DestroyWindow(hwnd);
            ReleaseDC(0, sdc);
            return None;
        }
        SelectObject(mem, bmp);
        ReleaseDC(0, sdc);

        // RGBA → BGRA；预压暗
        let n = (w as usize) * (h as usize);
        let mut base = vec![0u8; n * 4];
        for i in 0..n {
            let o = i * 4;
            base[o] = shot.rgba[o + 2];
            base[o + 1] = shot.rgba[o + 1];
            base[o + 2] = shot.rgba[o];
            base[o + 3] = 255;
        }
        let mut dim = base.clone();
        for p in dim.chunks_exact_mut(4) {
            p[0] = ((p[0] as u32 * 45) / 100) as u8;
            p[1] = ((p[1] as u32 * 45) / 100) as u8;
            p[2] = ((p[2] as u32 * 45) / 100) as u8;
        }

        {
            let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
            let mut c = Box::new(Ctx {
                w,
                h,
                ox: shot.x,
                oy: shot.y,
                pal: Palette::from_code(theme),
                mem,
                bits: bits as *mut u8,
                base,
                dim,
                keep: keep.to_vec(),
                sel: None,
                sel0: [0; 4],
                areas: Vec::new(),
                ops: Vec::new(),
                redo: Vec::new(),
                tool: Tool::Select,
                color: 0xF5222D,
                width: 3,
                drag: Drag::None,
                anchor: (0, 0),
                cur: (0, 0),
                pts: Vec::new(),
                bar: [0; 4],
                preview: None,
                brush: false,
                number_next: 1,
                hover: None,
                open: None,
                tray: [0; 4],
                tray_hover: None,
                hover_due: None,
                face: GROUP_DEFAULT,
                more_hover: false,
                menu_open: false,
                menu: [0; 4],
                menu_hover: None,
                palette: false,
                sw: [0; 4],
                fill: false,
                hwnd,
                keys: [0; 4],
                editor: 0,
                editing: None,
                mouse: None,
                hex: false,
                win_hit: None,
                press_win: None,
                moved: false,
                done: None,
            });
            /* 第一帧就把放大镜摆在光标旁边：不能等第一次 WM_MOUSEMOVE ——
               用户按下 F1 后手不动的话，屏幕上会什么都没有。 */
            let mut pt = PointI { x: 0, y: 0 };
            if GetCursorPos(&mut pt) != 0 {
                let mx = pt.x - shot.x;
                let my = pt.y - shot.y;
                if mx >= 0 && my >= 0 && mx < w && my < h {
                    c.mouse = Some((mx, my));
                    c.win_hit = c.hover_window(mx, my);
                }
            }
            *g = Some(c);
            if let Some(c) = g.as_mut() {
                c.render();
            }
        }

        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);

        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            /* 文本输入框是子控件，Enter/Esc 被它吃掉，所以在这里探一下键状态 */
            {
                let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(c) = g.as_mut() {
                    if c.editor != 0 {
                        /* GetAsyncKeyState 的低位是"自上次调用以来按过"，
                           只判高位（当前按下）会漏掉快速点一下 Enter（30ms 内抬起）。
                           两个位都判，用户怎么按都能提交。 */
                        let enter = GetAsyncKeyState(0x0D);
                        let esc = GetAsyncKeyState(0x1B);
                        if enter < 0 || (enter & 1) != 0 {
                            c.commit_text();
                            c.render();
                        } else if esc < 0 || (esc & 1) != 0 {
                            DestroyWindow(c.editor);
                            c.editor = 0;
                            c.editing = None;
                            c.render();
                        }
                    }
                }
            }
            let finished = {
                let g = CTX.lock().unwrap_or_else(|e| e.into_inner());
                g.as_ref().map(|c| c.done.is_some()).unwrap_or(true)
            };
            if finished {
                break;
            }
        }

        let out = {
            let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
            let taken = g.take();
            taken.and_then(|mut c| {
                c.done.take().and_then(|(action, png, place)| {
                    if action == "cancel" {
                        None
                    } else {
                        Some(Delivered {
                            action,
                            png,
                            place,
                        })
                    }
                })
            })
        };
        DestroyWindow(hwnd);
        DeleteObject(bmp);
        DeleteDC(mem);
        CUR.store(0, std::sync::atomic::Ordering::SeqCst);
        out
    }
}

#[cfg(test)]
mod bar_preview {
    /*! 工具栏的离屏渲染预览。

        为什么需要它：工具栏是**像素直绘**，既没有 DOM 也没有热重载，改完配色/圆角/间距
        平时只能"装上看一眼"。这里把工具栏画进一块 DIB 再导出 BMP，几秒钟就能比对，
        避免为了看一像素差异反复打包安装。

        跑法：`cargo test --manifest-path src-tauri/Cargo.toml --release bar_preview -- --ignored`
        产物：`%TEMP%\am-bar-preview.bmp`（左半深色背景、右半浅色 —— 专门验"两边都看得清"）。 */

    use super::*;

    /// 32bpp top-down BMP（只为肉眼检查，不进产品）
    fn write_bmp(path: &std::path::Path, w: i32, h: i32, px: &[u8]) {
        let stride = w as usize * 4;
        let data = stride * h as usize;
        let mut out = Vec::with_capacity(54 + data);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((54 + data) as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&(-h).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(data as u32).to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&px[..data]);
        std::fs::write(path, out).expect("write bmp");
    }

    /// 造一块离屏 DIB + 一个可画的 `Ctx`（三个测试共用）。
    ///
    /// 以前每个测试各写一份二十来个字段的字面量：`Ctx` 加一个字段就要改三处，漏一处就编不过。
    unsafe fn mk_ctx(w: i32, h: i32, pal: Palette) -> (Box<Ctx>, isize) {
        let mut bi = BitmapInfo::default();
        bi.header.bi_size = std::mem::size_of::<BitmapInfoHeader>() as u32;
        bi.header.bi_width = w;
        bi.header.bi_height = -h;
        bi.header.bi_planes = 1;
        bi.header.bi_bit_count = 32;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(0, &bi, 0, &mut bits, 0, 0);
        let mem = CreateCompatibleDC(0);
        SelectObject(mem, bmp);
        let c = Box::new(Ctx {
            w,
            h,
            ox: 0,
            areas: Vec::new(),
            oy: 0,
            pal,
            mem,
            bits: bits as *mut u8,
            base: vec![0u8; (w * h * 4) as usize],
            dim: vec![0u8; (w * h * 4) as usize],
            keep: Vec::new(),
            sel: None,
            sel0: [0; 4],
            ops: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Arrow,
            color: 0xF5222D,
            width: 3,
            drag: Drag::None,
            anchor: (0, 0),
            cur: (0, 0),
            pts: Vec::new(),
            bar: [0; 4],
            preview: None,
            brush: false,
            number_next: 1,
            hover: None,
            open: None,
            tray: [0; 4],
            tray_hover: None,
            hover_due: None,
            face: GROUP_DEFAULT,
            palette: false,
            sw: [0; 4],
            fill: false,
            hwnd: 0,
            keys: [0; 4],
            editor: 0,
            editing: None,
            /* 取色器 / 窗口识别（0.5.x）：离屏测试没有光标，默认不显示放大镜。 */
            mouse: None,
            hex: false,
            win_hit: None,
            press_win: None,
            moved: false,
            done: None,
            /* 工具栏 v3 新增的三个字段（分裂按钮的下拉）：测试用的 Ctx 也得给初值，
               否则 `cargo test` 直接编不过 —— 0.4.23 加字段时漏了这一处，
               0.4.24 才发现（测试红了没人看，等于没有测试）。 */
            more_hover: false,
            menu_open: false,
            menu: [0; 4],
            menu_hover: None,
        });
        (c, bmp)
    }

    /// 主条总宽：6px 内距 + 所有项 + 末尾「取消」
    fn bar_total_w() -> i32 {
        let mut bw = 10;
        for k in BAR {
            bw += bar_w(k);
        }
        bw + bar_w(BAR_TAIL)
    }

    /// 背景左半深、右半浅 —— 面板要在这两类截图上都看得清，这是预览的主要目的
    unsafe fn paint_bg(c: &Ctx, w: i32, h: i32) {
        let px = std::slice::from_raw_parts_mut(c.bits, (w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                let (r, g, b) = if x < w / 2 {
                    (0x2A, 0x2A, 0x2E)
                } else {
                    (0xF0, 0xF0, 0xEC)
                };
                px[o] = b;
                px[o + 1] = g;
                px[o + 2] = r;
                px[o + 3] = 0xFF;
            }
        }
    }

    /// 把一块区域按整数倍放大后另存（1px 的边、6px 的三角在 1:1 下看不清）
    unsafe fn write_zoom(
        c: &Ctx,
        name: &str,
        x0: i32,
        y0: i32,
        zw: i32,
        zh: i32,
        s: i32,
        w: i32,
        h: i32,
    ) {
        let px = std::slice::from_raw_parts(c.bits, (w * h * 4) as usize);
        let mut tile = vec![0u8; (zw * s * zh * s * 4) as usize];
        for y in 0..zh * s {
            for x in 0..zw * s {
                let sx = (x0 + x / s).clamp(0, w - 1);
                let sy = (y0 + y / s).clamp(0, h - 1);
                let so = ((sy * w + sx) * 4) as usize;
                let dof = ((y * zw * s + x) * 4) as usize;
                tile[dof..dof + 4].copy_from_slice(&px[so..so + 4]);
            }
        }
        write_bmp(&std::env::temp_dir().join(name), zw * s, zh * s, &tile);
    }

    /// 贴图矩形（`keep`）在压暗之后必须保持**原始亮度**（owner 2026-11-19：
    /// 「截图时原本贴在图上的不许消失」）。
    ///
    /// 钉住三件事：矩形内 = base 的像素；矩形外 = dim 的像素；右边界不漏。
    /// 这条测试是纯像素断言，不依赖窗口 —— 真机按键在 CI/沙箱里送不进去。
    #[test]
    fn keep_rects_stay_bright() {
        unsafe {
            let (mut c, _bmp) = mk_ctx(200, 120, Palette::dark());
            for p in c.base.chunks_exact_mut(4) {
                p[0] = 200;
                p[1] = 200;
                p[2] = 200;
                p[3] = 255;
            }
            for p in c.dim.chunks_exact_mut(4) {
                p[0] = 40;
                p[1] = 40;
                p[2] = 40;
                p[3] = 255;
            }
            c.keep = vec![[50, 20, 40, 40]];
            c.sel = None;
            c.fill_base();
            let at = |x: i32, y: i32| -> u8 {
                let o = ((y as usize) * (c.w as usize) + x as usize) * 4;
                *c.bits.add(o)
            };
            assert_eq!(at(60, 30), 200, "贴图矩形里必须保持原始亮度");
            assert_eq!(at(10, 10), 40, "矩形外照常压暗");
            assert_eq!(at(95, 30), 40, "矩形右边界外要压暗（差 1px 就会漏一条亮边）");
        }
    }

    /// 离屏预览：两套主题各出一张「工具栏 + 快捷键面板」对照图。
    ///
    /// 验收标准来自设计稿 v2.1：深浅各有对照图；斜置图钉在 22px 下要清楚；
    /// 面板压在浅色/深色两种背景上都不能融掉。
    #[test]
    #[ignore = "导出工具栏预览图，人工看效果"]
    fn bar_preview() {
        unsafe {
            let (w, h) = (1500i32, 620i32);
            for (tag, pal) in [("dark", Palette::dark()), ("light", Palette::light())] {
                let (mut c, bmp) = mk_ctx(w, h, pal);
                paint_bg(&c, w, h);

                /* 第一张：主条 + 展开的托盘（选中 / 悬停 / 组 / 胶囊 四态都看得到） */
                c.sel = Some([0, 0, 40, 40]); // 只为了让 has_sel() 成立（预览不画选区框）
                let bw = bar_total_w();
                c.bar = [(w - bw) / 2, 280, bw, BAR_H];
                c.tool = Tool::Mosaic;
                c.face[3] = Tool::Mosaic;
                c.open = Some(5); // 第 4 组（遮挡 / 打码）
                c.tray_layout();
                c.tray_hover = Some(1);
                c.hover = Some(13); // 橡皮擦
                c.draw_bar();
                c.draw_tray();
                c.draw_bar_tip();

                let out = std::env::temp_dir().join(format!("am-bar-{tag}.bmp"));
                let px = std::slice::from_raw_parts(c.bits, (w * h * 4) as usize);
                write_bmp(&out, w, h, px);

                /* 放大两块：复制主按钮（唯一实心）、颜色胶囊（19px 圆点） */
                let (zy, zh) = (c.bar[1] - 6, BAR_H + 12);
                write_zoom(
                    &c,
                    &format!("am-zoom-copy-{tag}.bmp"),
                    c.bar_item_x(15) - 4,
                    zy,
                    46,
                    zh,
                    8,
                    w,
                    h,
                );
                write_zoom(
                    &c,
                    &format!("am-zoom-color-{tag}.bmp"),
                    c.bar_item_x(7) - 4,
                    zy,
                    40,
                    zh,
                    8,
                    w,
                    h,
                );

                /* 第二张：调色板（重画一遍底，避免和托盘叠在一起） */
                paint_bg(&c, w, h);
                c.open = None;
                c.tray_hover = None;
                c.hover = Some(7);
                c.palette = true;
                c.sw_layout();
                c.draw_bar();
                c.draw_palette();
                let out2 = std::env::temp_dir().join(format!("am-swatch-{tag}.bmp"));
                let px2 = std::slice::from_raw_parts(c.bits, (w * h * 4) as usize);
                write_bmp(&out2, w, h, px2);
                println!("preview: {}", out.display());

                DeleteDC(c.mem);
                DeleteObject(bmp);
            }
        }
    }

    /// 托盘与调色板的命中回归：绘制与命中共用 `tray` / `sw` 两块矩形，
    /// 每一项中心都必须命回自己（差 1px 就是"点上去没反应"）。
    #[test]
    fn tray_and_palette_hit_each_item() {
        unsafe {
            let (mut c, bmp) = mk_ctx(1200, 620, Palette::dark());
            c.sel = Some([40, 40, 500, 300]);
            c.bar = [200, 420, bar_total_w(), BAR_H];

            /* ① 五个组：逐个展开，托盘里每一项中心都命中自己 */
            let group_is: Vec<usize> = (0..c.bar_len())
                .filter(|i| matches!(c.bar_item(*i), BarKind::Group(_)))
                .collect();
            assert_eq!(group_is.len(), 5, "必须有 5 个折叠组");
            for i in group_is {
                let g = c.group_of(i).expect("组下标");
                c.open = Some(i);
                c.tray_layout();
                assert!(c.tray[2] > 0, "组 {g} 的托盘没算出来");
                let cy = c.tray[1] + c.tray[3] / 2;
                for (k, _t) in GROUPS[g].iter().enumerate() {
                    let cx = c.tray[0] + 5 + k as i32 * BTN_W + BTN_W / 2;
                    assert_eq!(c.tray_hit(cx, cy), Some(k), "组 {g} 第 {k} 项没命中");
                }
                /* 托盘左侧 3px 内距不该命中任何条目 */
                assert_eq!(c.tray_hit(c.tray[0] + 2, cy), None);
            }

            /* ② 调色板 21 格逐个命中 */
            c.open = None;
            c.palette = true;
            c.sw_layout();
            assert!(c.sw[2] > 0, "调色板矩形没算出来");
            for (i, want) in SWATCHES.iter().enumerate() {
                let col = i as i32 % SW_COLS;
                let row = i as i32 / SW_COLS;
                let x = c.sw[0] + SW_PAD + col * (SW_CELL + SW_GAP) + SW_CELL / 2;
                let y = c.sw[1] + SW_PAD + row * (SW_CELL + SW_GAP) + SW_CELL / 2;
                assert_eq!(c.sw_hit(x, y), Some(*want), "第 {i} 个色块没命中");
            }
            /* 色块之间的缝不算命中 */
            let gap = c.sw[0] + SW_PAD + SW_CELL + SW_GAP / 2;
            assert_eq!(c.sw_hit(gap, c.sw[1] + SW_PAD + SW_CELL / 2), None);

            /* ③ 没选区：托盘与调色板都不响应（它们根本不画） */
            c.sel = None;
            assert_eq!(c.tray_hit(c.tray[0] + 8, c.tray[1] + 8), None);
            assert_eq!(c.sw_hit(c.sw[0] + SW_PAD + 8, c.sw[1] + SW_PAD + 8), None);

            DeleteDC(c.mem);
            DeleteObject(bmp);
        }
    }

    /// 12 个工具的"都能画"回归测试（owner 2026-11-18 要求逐个试一遍）。
    ///
    /// 真机 GUI 自动化在多屏虚拟桌面下坐标换算不可靠（截图 2560×1600，鼠标主屏 1600×1025），
    /// 所以这里直接走产品自己的 `make_op` + `draw_op`：每个工具都产出真实像素而不是空操作，
    /// 箭头方向、马赛克两种形态、序号自动递增也一并钉住。
    #[test]
    fn every_tool_draws_pixels() {
        unsafe {
            let (w, h) = (420i32, 320i32);
            let (mut c, bmp) = mk_ctx(w, h, Palette::dark());
            c.tool = Tool::Select;
            c.color = 0xFF2244;

            let a = (40i32, 40i32);
            let b = (180i32, 140i32);
            let pts = vec![(50, 160), (90, 190), (140, 170), (190, 210)];
            /* 每个 Tool 经 make_op 生成 Op，再经真实 draw_op 画进 DIB。
               文字不走 make_op —— 它点击后弹 EDIT 输入框，由 `start_text` 生成 Op::Text，
               所以下面单独用 Op::Text 验证绘制。 */
            for tool in [
                Tool::Rect,
                Tool::Ellipse,
                Tool::Line,
                Tool::Arrow,
                Tool::Pen,
                Tool::Marker,
            ] {
                c.tool = tool;
                let op = c.make_op(a, b, &pts).unwrap_or_else(|| panic!("{tool:?} 没产出 Op"));
                eprintln!("drawing {tool:?}");
                c.draw_op(&op);
            }
            /* 马赛克 / 高斯模糊都是两种形态：矩形 + 画笔（owner 2026-11-19 要的） */
            for tool in [Tool::Mosaic, Tool::Blur] {
                c.tool = tool;
                c.brush = false;
                let op = c.make_op(a, b, &pts).unwrap_or_else(|| panic!("{tool:?} 矩形没产出 Op"));
                c.draw_op(&op);
                c.brush = true;
                let op = c.make_op(a, b, &pts).unwrap_or_else(|| panic!("{tool:?} 画笔没产出 Op"));
                assert!(
                    matches!(op, Op::MosaicPath(..) | Op::BlurPath(..)),
                    "{tool:?} 画笔形态生成的不是路径 Op"
                );
                c.draw_op(&op);
            }
            c.brush = false;
            /* 序号。文字不在这里画：DrawTextW 依赖真实窗口 DC + EDIT 输入流程，
               在这个裸内存 DC 上会崩（离屏预览只验工具栏图标，不验文字）。 */
            c.draw_op(&Op::Number(240, 80, 1, 0xFF2244));
            assert_eq!(Op::Text("测试".into(), 240, 180, 0xFFFFFF, 16).bbox()[2] > 0, true);

            /* 像素真的变了（不是空函数） */
            let px = std::slice::from_raw_parts(c.bits as *const u8, (w * h * 4) as usize);
            let painted = px.chunks_exact(4).filter(|p| p[0] != 0 || p[1] != 0 || p[2] != 0).count();
            assert!(painted > 300, "12 个工具合计只画了 {painted} 个像素");

            /* 直线必须保留拖动方向（和箭头同一个坑）：从右下拖到左上，
               起点仍是右下、终点仍是左上 —— 不能被 norm4 归一化成"左上→右下" */
            c.tool = Tool::Line;
            match c.make_op((300, 250), (100, 60), &[]) {
                Some(Op::Line(p0, p1, _, _)) => {
                    assert_eq!(p0, (300, 250), "直线起点被改了");
                    assert_eq!(p1, (100, 60), "直线方向不能被 norm4 吃掉");
                }
                _ => panic!("直线没生成 Op::Line"),
            }

            /* 箭头必须保留拖动方向：从右下拖到左上，终点就是左上 */
            c.tool = Tool::Arrow;
            match c.make_op((300, 250), (100, 60), &[]) {
                Some(Op::Arrow(p0, p1, _, _)) => {
                    assert_eq!(p0, (300, 250));
                    assert_eq!(p1, (100, 60), "箭头方向不能被 norm4 吃掉");
                }
                _ => panic!("箭头没生成 Op::Arrow"),
            }

            DeleteDC(c.mem);
            DeleteObject(bmp);
        }
    }

    /// 布局与主题的确定性回归（这台多屏机器上 GUI 自动化坐标不可靠，靠它钉死）。
    ///
    /// ① 顺序：交付三连必须是 复制/贴图/保存，取消单独在末尾（设计稿 §05）；
    /// ② 绘制与命中共用步长：主条每个按钮、面板每一项的中心都要命回自己 ——
    ///    只改 `BTN_W` 一处而忘了改命中，用户看到的就是"点了没反应"；
    /// ③ 深浅两套色真的不同，且"外沿"和"内描边"不是同一个色（那样就少一层边界）。
    #[test]
    fn layout_and_palette_are_consistent() {
        unsafe {
            let (mut c, bmp) = mk_ctx(900, 260, Palette::dark());
            c.sel = Some([0, 0, 200, 150]); // 有选区工具栏才存在
            let bw = bar_total_w();
            c.bar = [(900 - bw) / 2, 200, bw, BAR_H];

            /* ① 顺序 */
            assert_eq!(BAR_TAIL, BarKind::Act("cancel"), "取消必须在末尾单独一格");
            let acts: Vec<&str> = (0..c.bar_len())
                .filter_map(|i| match c.bar_item(i) {
                    BarKind::Act(a) => Some(a),
                    _ => None,
                })
                .collect();
            /* 0.4.23 工具栏 v3：`保存` / `贴图` 并进了「复制 ▾」分裂按钮的右段，
               主条上现在的动作是 undo / redo / copy / copymore / cancel。
               这条断言原来写的是 v2 的 `…"save","pin"…`，改版后没人更新 → 一直红。 */
            assert_eq!(
                acts,
                vec!["undo", "redo", "copy", "copymore", "cancel"]
            );

            /* ② 主条第 i 项中心必须命回第 i 项 */
            for i in 0..c.bar_len() {
                if matches!(c.bar_item(i), BarKind::Sep) {
                    continue;
                }
                let cx = c.bar_item_x(i) + bar_w(c.bar_item(i)) / 2;
                let cy = c.bar[1] + BAR_H / 2;
                assert_eq!(c.bar_hit(cx, cy), Some(i), "主条第 {i} 项中心没命中自己");
            }

            /* ③ 12 个工具全部可达：选择与橡皮常驻，其余 10 个分在 4 个组里 */
            let mut tools: Vec<Tool> = Vec::new();
            let mut groups = 0;
            for i in 0..c.bar_len() {
                match c.bar_item(i) {
                    BarKind::Tool(t) => tools.push(t),
                    BarKind::Group(g) => {
                        groups += 1;
                        tools.extend(GROUPS[g].iter().copied());
                    }
                    _ => {}
                }
            }
            /* 0.4.23 起是 5 个折叠组（形状 / 画笔 / 文字 / 遮挡 / 素材），共 17 个工具 */
            assert_eq!(groups, 5, "必须有 5 个折叠组");
            assert_eq!(tools.len(), 17, "17 个工具都要在工具栏上（组里的也算）");
            for t in [
                Tool::Select,
                Tool::Rect,
                Tool::Ellipse,
                Tool::Line,
                Tool::Arrow,
                Tool::Pen,
                Tool::Marker,
                Tool::Mosaic,
                Tool::Blur,
                Tool::Text,
                Tool::Number,
                /* 橡皮擦不在这个列表里：0.4.23 工具栏 v3 之后它**没有任何入口**
                   （不在 BAR、不在 GROUPS、也没有键位），已报给 owner 决定去留。
                   在这之前别把它写进断言，否则测试会假装它在。 */
            ] {
                assert!(tools.contains(&t), "{t:?} 不在工具栏上");
            }

            /* ④ 两套色 */
            let (d, l) = (Palette::dark(), Palette::light());
            assert_ne!(d.bg, l.bg, "深浅主题的面板底不能一样");
            assert_ne!(d.icon, l.icon, "深浅主题的图标墨色不能一样");
            for p in [d, l] {
                assert_ne!(p.edge_out, p.edge, "外沿与内描边同色 = 少一层边界");
                assert_ne!(p.bg, p.hover, "悬停底与面板底同色 = 看不出鼠标停在哪");
                assert_ne!(p.bg, p.active, "选中底与面板底同色 = 看不出当前工具");
            }

            /* ⑤ 没有选区时工具栏不该命中任何东西（它根本不画） */
            c.sel = None;
            assert_eq!(
                c.bar_hit(c.bar[0] + 20, c.bar[1] + BAR_H / 2),
                None,
                "没选区还能点中工具栏 = 会点到看不见的按钮"
            );

            DeleteDC(c.mem);
            DeleteObject(bmp);
        }
    }

    /// 数一块矩形里"画过"的像素（RGBA 任一非 0）
    unsafe fn painted_in(c: &Ctx, r: [i32; 4]) -> i32 {
        let px = std::slice::from_raw_parts(c.bits, (c.w * c.h * 4) as usize);
        let mut n = 0;
        for y in r[1].max(0)..(r[1] + r[3]).min(c.h) {
            for x in r[0].max(0)..(r[0] + r[2]).min(c.w) {
                let o = ((y * c.w + x) * 4) as usize;
                if px[o] != 0 || px[o + 1] != 0 || px[o + 2] != 0 || px[o + 3] != 0 {
                    n += 1;
                }
            }
        }
        n
    }

    /// alpha 回归（2026-11-19 真机事故）：GDI 写字只写 RGB、不动 alpha 字节，
    /// 而覆盖层用 `UpdateLayeredWindow(ULW_ALPHA)` 上屏 —— alpha=0 的字在真机上
    /// **直接透出桌面**，看起来就是"字体显示不出来"。离屏预览存 BMP（忽略 alpha），
    /// 所以预览永远正常、只有装机才看得见。
    /// 这条测试盯住：工具栏 / 选项行 / 提示面板三块，画完之后 alpha 必须全是 255。
    #[test]
    fn text_panels_are_opaque() {
        unsafe {
            let (mut c, bmp) = mk_ctx(1700, 640, Palette::dark());
            c.sel = Some([40, 40, 600, 300]);
            c.open = Some(2);
            c.palette = true;
            c.render();
            c.draw_keys(); // render 里已经画过，这里再画一次确保 keys 矩形是这一帧的

            let px = std::slice::from_raw_parts(c.bits, (c.w * c.h * 4) as usize);
            let check = |r: [i32; 4], name: &str| {
                let mut bad = 0;
                for y in r[1].max(0)..(r[1] + r[3]).min(c.h) {
                    for x in r[0].max(0)..(r[0] + r[2]).min(c.w) {
                        let o = ((y * c.w + x) * 4) as usize;
                        if px[o + 3] != 255 {
                            bad += 1;
                        }
                    }
                }
                assert_eq!(bad, 0, "{name} 有 {bad} 个像素 alpha != 255（真机上会透底）");
            };
            check(c.bar, "工具栏");
            check(c.tray, "托盘");
            check(c.sw, "调色板");
            check(c.keys, "提示面板");

            DeleteDC(c.mem);
            DeleteObject(bmp);
        }
    }

    /// 「工具栏只在框选之后出现」整条渲染链路的回归（owner 2026-11-18 要求删掉 F1 时飘在
    /// 屏幕底部的那条空工具条）。这条必须走真实 `render()`：只测 `bar_hit` 测不出"画没画"。
    #[test]
    fn toolbar_only_appears_with_selection() {
        unsafe {
            /* 画布要够宽：工具栏约 570px，提示面板又在右下角，
               两者在窄画布上会重叠，数出来的像素就分不清是谁画的。 */
            let (mut c, bmp) = mk_ctx(1700, 500, Palette::dark());

            /* ① 没选区：F1 刚按下那一帧，工具栏位置必须是干净的 */
            c.sel = None;
            c.render();
            let bar = c.bar;
            assert_eq!(
                painted_in(&c, bar),
                0,
                "没框选却在 {:?} 画出了工具栏",
                bar
            );

            /* ② 框选之后：整条贴在选区下方，画满 */
            c.sel = Some([40, 40, 300, 200]);
            c.render();
            let bar = c.bar;
            assert!(
                bar[1] >= 240,
                "工具栏应该贴在选区下方，实际 y={}",
                bar[1]
            );
            assert!(
                painted_in(&c, bar) > 3000,
                "框选后工具栏只画了 {} 个像素",
                painted_in(&c, bar)
            );

            DeleteDC(c.mem);
            DeleteObject(bmp);
        }
    }

    /// 取色必须取**冻结帧**（`base`），不能取压暗后的 `bits` / `dim`；BGRA 通道顺序也要对。
    /// 这条盯的是"取到的颜色等于用户眼睛看到的颜色"——压暗层一旦被误读，色值会整体偏暗。
    #[test]
    fn loupe_reads_frozen_frame_bgra() {
        unsafe {
            let (mut c, bmp) = mk_ctx(40, 30, Palette::dark());
            for p in c.base.chunks_exact_mut(4) {
                p[0] = 10; // B
                p[1] = 20; // G
                p[2] = 250; // R
                p[3] = 255;
            }
            /* 合成缓冲压成纯黑：只要 `sample_base` 误读 `bits`/`dim`，这里立刻红 */
            let px = std::slice::from_raw_parts_mut(c.bits, (c.w * c.h * 4) as usize);
            for p in px.chunks_exact_mut(4) {
                p[0] = 0;
                p[1] = 0;
                p[2] = 0;
                p[3] = 255;
            }
            for p in c.dim.chunks_exact_mut(4) {
                p[0] = 0;
                p[1] = 0;
                p[2] = 0;
                p[3] = 255;
            }
            assert_eq!(c.sample_base(10, 10), (250, 20, 10), "BGRA 通道顺序错了");
            assert_eq!(
                c.sample_base(-99, -99),
                (250, 20, 10),
                "越界要夹到边缘，不能返回黑"
            );
            DeleteDC(c.mem);
            DeleteObject(bmp);
        }
    }
}

#[cfg(test)]
mod stack_tests {
    /*! 多框选纵向拼接的像素单测（0.4.24）。

        为什么要它：拼接只发生在"复制/保存/贴图"那一刻，真机手测要装包、框三块、
        再粘出来量尺寸。这里直接喂一块假屏幕缓冲，把"高度=各块之和""窄块右侧补透明"
        "排序按上→下"三件事钉住，改像素循环时立刻能发现。 */

    use super::stack_rows;

    /// 假屏幕：B = x*10、G = y*10、R = 200，方便断言取到的是哪一行哪一列
    fn buf(w: i32, h: i32) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                v[o] = (x * 10) as u8;
                v[o + 1] = (y * 10) as u8;
                v[o + 2] = 200;
                v[o + 3] = 255;
            }
        }
        v
    }

    #[test]
    fn stacks_vertically_without_gap() {
        let src = buf(4, 6);
        /* 故意**倒着**传：下面那块先给，上面那块后给 —— 排序必须自己按 y 排 */
        let (out, w, h) = stack_rows(&src, 4, 6, &[[2, 4, 2, 2], [0, 0, 4, 2]]).unwrap();
        assert_eq!((w, h), (4, 4), "宽度取最宽块，高度 = 各块之和（空隙不留）");

        /* 第一行是上面那块（源 0,0）：B=0 G=0 R=200 → RGBA [200,0,0,255] */
        assert_eq!(&out[0..4], &[200, 0, 0, 255]);

        /* 第三行（y=2）是下面那块的第一行，左对齐；源 (2,4) → B=20 G=40 R=200 */
        let o = (2 * 4) * 4;
        assert_eq!(&out[o..o + 4], &[200, 40, 20, 255], "窄块左对齐贴到左边");

        /* 窄块只有 2 列，右边两列补全透明 */
        let o = (2 * 4 + 3) * 4;
        assert_eq!(&out[o..o + 4], &[0, 0, 0, 0], "窄块右侧补透明");
    }

    #[test]
    fn empty_input_is_none() {
        let src = buf(4, 4);
        assert!(stack_rows(&src, 4, 4, &[]).is_none());
        assert!(stack_rows(&src, 4, 4, &[[0, 0, 0, 5]]).is_none());
    }
}

#[cfg(test)]
mod pick_tests {
    /*! 取色器 / 窗口识别的纯逻辑单测（0.5.x）。

        为什么只测这几块：真窗口、真光标、真 DPI 在 CI/沙箱里都没有，
        `WindowFromPoint` / `DwmGetWindowAttribute` 只能装机手测。
        但"面板会不会出屏""窗口矩形夹不夹得住""色值格式对不对"全是纯算术，
        改一个边界条件就能在这里立刻发现。 */

    use super::*;

    /// 面板默认在光标右下；靠右/靠下时翻到左/上，且始终完整留在屏幕里。
    #[test]
    fn loupe_flips_and_stays_on_screen() {
        let (pw, ph) = (loupe_panel_w(), loupe_panel_h());
        let (sw, sh) = (1920, 1080);

        /* 右下角：必须翻到左上方 */
        let r = loupe_at(1900, 1000, sw, sh);
        assert_eq!((r[2], r[3]), (pw, ph));
        assert!(r[0] + pw <= sw, "贴右边时面板出屏：{r:?}");
        assert!(r[1] + ph <= sh, "贴下边时面板出屏：{r:?}");
        assert!(r[0] < 1900, "贴右边应该翻到光标左侧：{r:?}");
        assert!(r[1] < 1000, "贴下边应该翻到光标上方：{r:?}");

        /* 四角 + 中心：一个都不许出屏 */
        for (cx, cy) in [
            (0, 0),
            (sw - 1, 0),
            (0, sh - 1),
            (sw - 1, sh - 1),
            (sw / 2, sh / 2),
        ] {
            let r = loupe_at(cx, cy, sw, sh);
            assert!(
                r[0] >= 0 && r[1] >= 0 && r[0] + pw <= sw && r[1] + ph <= sh,
                "光标 ({cx},{cy}) 处面板出屏：{r:?}"
            );
        }
    }

    /// 屏幕比面板还小（极端窗口/远程桌面）：不能 panic，也不能返回负数。
    #[test]
    fn loupe_survives_tiny_screen() {
        let r = loupe_at(10, 10, 120, 90);
        assert!(r[0] >= 0 && r[1] >= 0, "小屏夹紧不应出现负坐标：{r:?}");
    }

    /// 窗口矩形要 clamp 到虚拟桌面：DWM 最大化窗口的边框常见负坐标。
    #[test]
    fn window_rect_clamps_to_desktop() {
        assert_eq!(clamp_rect([-10, -10, 100, 100], 1920, 1080), Some([0, 0, 90, 90]));
        assert_eq!(
            clamp_rect([1900, 1070, 100, 100], 1920, 1080),
            Some([1900, 1070, 20, 10])
        );
        /* 整块在屏幕外 / 零尺寸：没有可截的东西 */
        assert_eq!(clamp_rect([2000, 2000, 50, 50], 1920, 1080), None);
        assert_eq!(clamp_rect([10, 10, 0, 50], 1920, 1080), None);
    }

    /// 色值格式：默认 `R, G, B`，Shift 后 `#RRGGBB`；0 / 255 两个边界都要对。
    #[test]
    fn formats_rgb_and_hex_at_bounds() {
        assert_eq!(fmt_rgb(0, 0, 0), "0, 0, 0");
        assert_eq!(fmt_rgb(255, 255, 255), "255, 255, 255");
        assert_eq!(fmt_rgb(250, 250, 248), "250, 250, 248");

        assert_eq!(fmt_hex(0, 0, 0), "#000000");
        assert_eq!(fmt_hex(255, 255, 255), "#FFFFFF");
        /* 单个数字必须补零：10 / 171 / 255 → 0A AB FF */
        assert_eq!(fmt_hex(10, 171, 255), "#0AABFF");
    }
}
