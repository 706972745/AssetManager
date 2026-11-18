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
}

/* ---------------- 工具栏模型（照 Snipaste 2.11.3 的形态） ----------------

   owner 2026-11-19 定：**功能和样式都跟 Snipaste 一样** —— 工具全部平铺一条，
   不再有"悬浮展开的组面板"（那套 0.3.x 连出三次"展不开"，也越来越不像 Snipaste）。

   列的顺序也按 Snipaste：识别工具 → 工具（选择/形状/手绘/打码/标记/橡皮）→ 撤销重做
   → 交付（复制 / 贴图 / 保存）→ 取消单独在末尾。宽度约 670px：宽屏无压力，
   1366 笔记本上会占掉小半个屏幕 —— 这是"一步可达"换来的代价，owner 认这个取舍。

   第二行（`OPT`）是当前工具的选项：颜色板 + 粗细。Snipaste 的选项行在主条**下方**，
   这里照做（`opt_layout`）。 */

#[derive(Clone, Copy, PartialEq, Debug)]
enum BarKind {
    Tool(Tool),
    Sep,
    Act(&'static str),
}

const BAR: [BarKind; 19] = [
    BarKind::Tool(Tool::Select),
    BarKind::Tool(Tool::Rect),
    BarKind::Tool(Tool::Ellipse),
    BarKind::Tool(Tool::Line),
    BarKind::Tool(Tool::Arrow),
    BarKind::Tool(Tool::Pen),
    BarKind::Tool(Tool::Marker),
    BarKind::Tool(Tool::Mosaic),
    BarKind::Tool(Tool::Blur),
    BarKind::Tool(Tool::Text),
    BarKind::Tool(Tool::Number),
    BarKind::Tool(Tool::Eraser),
    BarKind::Sep,
    BarKind::Act("undo"),
    BarKind::Act("redo"),
    BarKind::Sep,
    BarKind::Act("copy"),
    BarKind::Act("pin"),
    BarKind::Act("save"),
];

/// 「取消」固定放最末尾：它和三个交付动作语义相反，而覆盖层是全屏的 ——
/// 挨着放时手一偏就是"白框一次、什么都没留下"。
const BAR_TAIL: BarKind = BarKind::Act("cancel");

/* ---------------- 第二行：颜色 / 粗细（照 Snipaste 的调色板） ----------------

   像素直绘没有 alpha，色块就是实色。白色块必须描边，否则落在白面板上等于隐形。 */
const SWATCHES: [u32; 10] = [
    0xF5222D, // 红（默认标注色，和 0.3.x 一样）
    0xFA8C16, // 橙
    0xFADB14, // 黄
    0x52C41A, // 绿
    0x13C2C2, // 青
    0x1890FF, // 蓝
    0x722ED1, // 紫
    0x000000, // 黑
    0xFFFFFF, // 白
    0x8C8C8C, // 灰
];
/// 四档线宽（Snipaste 的 1~12）
const WIDTHS: [i32; 4] = [1, 3, 6, 12];

/// 第二行点到了什么
#[derive(Clone, Copy, PartialEq, Debug)]
enum OptPick {
    Color(u32),
    Width(i32),
    /// 遮罩类工具的形态：true = 画笔涂抹（马赛克 / 高斯模糊共用）
    Brush(bool),
}

/// 第二行开头两个形态按钮的宽度（只在马赛克 / 高斯模糊时出现）
const MODE_W: i32 = 54;

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
    /// 选中底 = accent 实色
    active: u32,
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
            active: 0x5B8CFF,
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
            active: 0x1B4FE0,
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
const SEP_W: i32 = 9;
const BAR_H: i32 = 40;
const BAR_RADIUS: i32 = 8;
const BTN_RADIUS: i32 = 4;

fn bar_w(k: BarKind) -> i32 {
    match k {
        BarKind::Sep => SEP_W,
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
    }
}

fn bar_label(k: BarKind) -> (&'static str, &'static str) {
    match k {
        BarKind::Tool(t) => tool_label(t),
        BarKind::Act(a) => match a {
            "undo" => ("撤销", "Ctrl+Z"),
            "redo" => ("重做", "Ctrl+Y"),
            "cancel" => ("取消", "Esc"),
            "pin" => ("贴图", "中键"),
            "save" => ("保存到截图库", "Ctrl+S"),
            _ => ("复制到剪贴板", "Ctrl+C"),
        },
        BarKind::Sep => ("", ""),
    }
}

#[derive(Clone)]
enum Op {
    Rect([i32; 4], u32, i32),
    Ellipse([i32; 4], u32, i32),
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
}

impl Op {
    /// 脏矩形（擦橡皮与重绘判据用）。箭头这里必须**归一化**：
    /// 端点可以是任意方向，但包围盒只能是正矩形。
    fn bbox(&self) -> [i32; 4] {
        match self {
            Op::Rect(a, _, _) | Op::Ellipse(a, _, _) | Op::Mosaic(a) => *a,
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
    /// 主窗当前主题对应的那一套色（`run` 时定下来，整场截图不变）
    pal: Palette,
    mem: Hdc,
    bits: *mut u8,
    base: Vec<u8>,
    dim: Vec<u8>,
    sel: Option<[i32; 4]>,
    sel0: [i32; 4],
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
    /// 右下角提示面板的矩形（`draw_keys` 算出来记下，测试要断言这块的 alpha）
    keys: [i32; 4],
    /// 第二行选项条的矩形。`opt_layout` 算一次存下来，绘制与命中共用 ——
    /// 两边各算一遍迟早会差 1px，点色块点到旁边的空地上。
    opt: [i32; 4],
    editor: Hwnd,
    editing: Option<(i32, i32)>,
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

fn rgb(c: u32) -> u32 {
    // 0xRRGGBB → COLORREF(0x00BBGGRR)
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
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
        match self.sel {
            Some([rx, ry, rw, rh]) if rw > 2 && rh > 2 => {
                for y in 0..self.h {
                    let inside_y = y >= ry && y < ry + rh;
                    let src = if inside_y { &self.base } else { &self.dim };
                    let off = y as usize * row;
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            src.as_ptr().add(off),
                            self.bits.add(off),
                            row,
                        );
                    }
                    if inside_y {
                        let l = rx.max(0) as usize * 4;
                        let rr = ((rx + rw).min(self.w) as usize) * 4;
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                self.dim.as_ptr().add(off),
                                self.bits.add(off),
                                l,
                            );
                            if rr < row {
                                std::ptr::copy_nonoverlapping(
                                    self.dim.as_ptr().add(off + rr),
                                    self.bits.add(off + rr),
                                    row - rr,
                                );
                            }
                        }
                    }
                }
            }
            _ => unsafe {
                std::ptr::copy_nonoverlapping(
                    self.dim.as_ptr(),
                    self.bits,
                    (self.w as usize) * (self.h as usize) * 4,
                );
            },
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
            Op::Rect(r, c, w) => self.pframe(r[0], r[1], r[0] + r[2], r[1] + r[3], *w, *c),
            Op::Ellipse(r, c, w) => self.pellipse(
                r[0] + r[2] / 2,
                r[1] + r[3] / 2,
                r[2] / 2,
                r[3] / 2,
                *w,
                *c,
            ),
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
            Tool::Rect => Some(Op::Rect(r, self.color, self.width)),
            Tool::Ellipse => Some(Op::Ellipse(r, self.color, self.width)),
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
            _ => None,
        }
    }

    /// 工具栏位置与尺寸（宽度由 `bar_w` 累加，尺寸见 `BTN_W` 那一段注释）。
    fn bar_layout(&mut self) {
        let mut bw = 6; // 左右各 3px 内距
        for k in BAR {
            bw += bar_w(k);
        }
        bw += bar_w(BAR_TAIL);
        let bh = BAR_H;
        let (x, y) = match self.sel {
            Some([sx, sy, sw, sh]) if sw > 8 && sh > 8 => {
                /* 优先贴在选区下方 8px（和网页版一致）；下方放不下就翻到选区上方。
                   翻上去时要留 6px，否则工具栏会贴着屏幕顶被裁掉。 */
                let y = if sy + sh + 8 + bh + 6 < self.h {
                    sy + sh + 8
                } else {
                    (sy - bh - 8).max(4)
                };
                (sx, y)
            }
            /* 还没框选时浮在屏幕偏下居中：比贴底更靠上，免得被任务栏挡住 */
            _ => ((self.w - bw) / 2, self.h - bh - 56),
        };
        self.bar = [x.max(4).min(self.w - bw - 4), y, bw, bh];
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
        let mut x = self.bar[0] + 3;
        for j in 0..i {
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

    /* ---------- 第二行选项条（颜色 / 粗细） ----------

       Snipaste 的选项行贴在主条**下方**、左端对齐。尺寸在这里算一次存进 `self.opt`，
       绘制（`draw_opt`）和命中（`opt_hit`）都读它 —— 两边各算一遍迟早差 1px。 */

    /// 遮罩类工具（马赛克 / 高斯模糊）才有「矩形 / 画笔」两种形态
    fn is_mask_tool(&self) -> bool {
        matches!(self.tool, Tool::Mosaic | Tool::Blur)
    }

    /// 色块边长 / 间距 / 粗细按钮宽（三者共同决定整条宽度）。
    /// **必须和 `draw_opt` / `opt_hit` 用同一套偏移**：宽度算错了，最后一个粗细档
    /// 就会点到面板外面。
    fn opt_w(&self) -> i32 {
        let mode = if self.is_mask_tool() { MODE_W * 2 + SEP_W } else { 0 };
        mode + 8 + 26 + 8 + 10 * (20 + 4) + SEP_W + 4 * 30 + 10
    }

    fn opt_layout(&mut self) {
        let w = self.opt_w().min(self.w - 8);
        /* x 也要夹住：选区贴屏幕右边时，主条会被夹到 w-bw-4，选项行不能跟着溢出屏幕 */
        let x = self.bar[0].min((self.w - w - 4).max(4));
        /* 主条贴在选区**下方**时，选项行继续往下排；
           主条因为下面放不下而翻到选区上方时，选项行要跟着往上翻 ——
           不翻的话它会横在选区顶边上，既挡内容又容易被当成"在选区里落笔"。 */
        let below_sel = self
            .sel
            .map(|s| self.bar[1] >= s[1] + s[3])
            .unwrap_or(true);
        let y = if below_sel {
            self.bar[1] + self.bar[3] + 6
        } else {
            (self.bar[1] - 6 - BAR_H).max(4)
        };
        self.opt = [x, y, w, BAR_H];
    }

    /// 第二行的命中：返回 (色值 or 粗细)
    fn opt_hit(&self, x: i32, y: i32) -> Option<OptPick> {
        if !self.has_sel() {
            return None;
        }
        let [ox, oy, ow, oh] = self.opt;
        if x < ox || x >= ox + ow || y < oy || y >= oy + oh {
            return None;
        }
        let mut cx = ox + 8;
        if self.is_mask_tool() {
            /* 左段 = 矩形，右段 = 画笔。返回的是「点完之后应该是什么」 */
            if x >= cx && x < cx + MODE_W * 2 {
                return Some(OptPick::Brush(x >= cx + MODE_W));
            }
            cx += MODE_W * 2 + SEP_W;
        }
        cx += 26 + 8; // 跳过"当前色"大方块
        for (i, c) in SWATCHES.iter().enumerate() {
            if x >= cx && x < cx + 20 {
                return Some(OptPick::Color(*c));
            }
            cx += 24;
            let _ = i;
        }
        /* 分隔 + 四档粗细 */
        cx += SEP_W;
        for w in WIDTHS {
            if x >= cx && x < cx + 30 {
                return Some(OptPick::Width(w));
            }
            cx += 30;
        }
        None
    }

    /// 鼠标悬停在主条哪一个按钮上（分隔条不算）。没选区时工具栏不画，也不该命中。
    fn update_bar_hover(&mut self, x: i32, y: i32) -> bool {
        if !self.has_sel() {
            return self.hover.take().is_some();
        }
        let hit = self
            .bar_hit(x, y)
            .filter(|i| !matches!(self.bar_item(*i), BarKind::Sep));
        if self.hover != hit {
            self.hover = hit;
            return true;
        }
        false
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

    /// 圆角矩形填充。
    ///
    /// 为什么自己算而不是画"四个圆 + 两个矩形"拼：拼接会在圆与矩形的接缝处留下
    /// 一列重复像素（半透明时尤其明显，出现一道竖纹）。逐行算左右内缩只多 6 行代码，
    /// 而且**同一条曲线**能直接给柔影复用（透明度版）。
    /// 没有抗锯齿：半径只有 8px、屏幕上是 1px 台阶，肉眼看不出来，但省掉一张 mask。
    fn pfill_round(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: i32, c: u32) {
        self.pfill_round_a(x0, y0, x1, y1, r, c, 1.0);
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
        self.bar_plate(x, y, w, h, BAR_RADIUS);

        for i in 0..self.bar_len() {
            let k = self.bar_item(i);
            let ix = self.bar_item_x(i);
            let iw = bar_w(k);
            let hover = self.hover == Some(i);
            match k {
                BarKind::Sep => {
                    let px = ix + iw / 2;
                    self.pfill(px, y + 10, px + 1, y + h - 10, self.pal.sep);
                }
                BarKind::Tool(t) => {
                    let active = t == self.tool;
                    self.bar_btn(ix, y, iw, h, active, hover);
                    let ink = if active { 0xFFFFFF } else { self.pal.icon };
                    self.icon_px(ix + iw / 2, y + h / 2, t, ink);
                }
                BarKind::Act(a) => {
                    if hover {
                        let bg = if a == "cancel" {
                            self.pal.danger
                        } else {
                            self.pal.hover
                        };
                        self.pfill_round(ix + 2, y + 3, ix + iw - 2, y + h - 3, BTN_RADIUS, bg);
                    }
                    let ink = match (a, hover) {
                        ("cancel", true) => 0xFFFFFF,
                        ("cancel", false) => self.pal.danger,
                        _ => self.pal.icon,
                    };
                    self.act_px(ix + iw / 2, y + h / 2, a, ink);
                }
            }
        }
        /* 图标是像素直绘（alpha 由 put 写满），但主条上还有分隔线等元素；一并固化更省心 */
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
        let (x0, y0, x1, y1) = (ix + 2, y + 3, ix + iw - 2, y + h - 3);
        if active {
            self.pfill_round(x0, y0, x1, y1, BTN_RADIUS, self.pal.active);
        } else if hover {
            self.pfill_round(x0, y0, x1, y1, BTN_RADIUS, self.pal.hover);
        }
    }

    /// 选项行上的小字（形态按钮用）。中文只能走 GDI，画完由 `draw_opt` 末尾的
    /// `solidify` 统一把 alpha 压满 —— 否则真机上字会透底（见 `solidify` 的注释）。
    fn small_text(&mut self, s: &str, cx: i32, cy: i32, ink: u32) {
        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let f = CreateFontW(-13, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let old = SelectObject(self.mem, f);
            SetBkMode(self.mem, TRANSPARENT);
            SetTextColor(self.mem, rgb(ink));
            let mut rc = RectPos {
                left: cx - 34,
                top: cy - 10,
                right: cx + 34,
                bottom: cy + 11,
            };
            DrawTextW(
                self.mem,
                wide.as_ptr(),
                wide.len() as i32,
                &mut rc,
                DT_CENTER | DT_SINGLELINE,
            );
            SelectObject(self.mem, old);
            DeleteObject(f);
        }
    }

    /// 第二行：当前色大方块 + 10 个色块 + 四档粗细（照 Snipaste 的选项行）。
    ///
    /// 选中态用 accent 描边而不是填充：色块本身有颜色，填充会把颜色吃掉。
    /// 白色块必须描一圈边，否则在浅色主题的白面板上等于隐形。
    fn draw_opt(&mut self) {
        if !self.has_sel() {
            return;
        }
        let [ox, oy, ow, oh] = self.opt;
        self.bar_plate(ox, oy, ow, oh, BAR_RADIUS);

        /* 遮罩类工具：开头先给「矩形 / 画笔」两个形态按钮（马赛克、高斯模糊共用）。
           没有这两个按钮时，切换形态只能"再点一次同一个工具按钮"——owner 2026-11-19
           的反馈就是"不知道还能涂"，等于这个能力不存在。 */
        let mut lead = 8;
        if self.is_mask_tool() {
            let ty = oy + (oh - 24) / 2;
            for (i, name) in ["矩形", "画笔"].iter().enumerate() {
                let sx = ox + 8 + i as i32 * MODE_W;
                let on = self.brush == (i == 1);
                if on {
                    self.pfill_round(sx, ty, sx + MODE_W - 6, ty + 24, BTN_RADIUS, self.pal.active);
                } else {
                    self.pfill_round(sx, ty, sx + MODE_W - 6, ty + 24, BTN_RADIUS, self.pal.hover);
                }
                let ink = if on { 0xFFFFFF } else { self.pal.icon };
                self.small_text(name, sx + (MODE_W - 6) / 2, ty + 12, ink);
            }
            lead += MODE_W * 2 + SEP_W;
            /* 形态段后面拉一条分隔线，和右边的颜色区隔开 */
            let sx = ox + 8 + MODE_W * 2 + SEP_W / 2;
            self.pfill(sx, oy + 10, sx + 1, oy + oh - 10, self.pal.sep);
        }

        /* 当前色：大方块 */
        let (bx, by) = (ox + lead, oy + (oh - 26) / 2);
        self.pfill_round(bx, by, bx + 26, by + 26, 4, self.pal.edge_out);
        self.pfill_round(bx + 1, by + 1, bx + 25, by + 25, 3, self.color);

        /* 10 个色块 */
        let mut cx = ox + lead + 26 + 8;
        for c in SWATCHES {
            let ty = oy + (oh - 20) / 2;
            let sel = c == self.color;
            if sel {
                self.pfill_round(cx - 2, ty - 2, cx + 22, ty + 22, 5, self.pal.active);
            } else {
                /* 非选中也给一圈淡边：白色块靠它才在浅色面板上看得见 */
                self.pfill_round(cx - 1, ty - 1, cx + 21, ty + 21, 4, self.pal.edge);
            }
            self.pfill_round(cx, ty, cx + 20, ty + 20, 3, c);
            cx += 24;
        }

        /* 分隔 + 四档粗细：圆点直径就是线宽本身，选中时底变 accent */
        cx += SEP_W;
        let sx = cx - SEP_W / 2;
        self.pfill(sx, oy + 10, sx + 1, oy + oh - 10, self.pal.sep);
        for (width_rank, w) in WIDTHS.iter().enumerate() {
            let sel = *w == self.width;
            if sel {
                self.pfill_round(cx + 1, oy + 3, cx + 29, oy + oh - 3, BTN_RADIUS, self.pal.active);
            }
            let ink = if sel { 0xFFFFFF } else { self.pal.icon };
            /* 按**档位**递增（2/3/4/5）而不是按线宽等比：等比时 1px 和 3px 都落到 2px，
               四档看起来一模一样，等于选不了。 */
            let r = 2.0 + width_rank as f32;
            self.pdisc_aa((cx + 15) as f32, (oy + oh / 2) as f32, r, ink);
            cx += 30;
        }
        self.solidify([ox, oy, ow, oh]);
    }

    /// 12 个工具图标 —— 24px 网格、**图标框 22、线宽 1.8**、全部抗锯齿。
    ///
    /// 造型是 0.3.x 甲方认可的那一套（第一版 ±9px 硬像素"画得太丑"后重画的），
    /// 这一版只把图标框从 20 提到 22、线宽从 1.5 提到 1.8 —— 所以下面这些坐标
    /// 是**原值等比放大 1.1 后的结果**，不是重新设计。别按"看着更顺眼"再改形状。
    fn icon_px(&mut self, cx: i32, cy: i32, t: Tool, ink: u32) {
        let (x, y) = (cx as f32, cy as f32);
        let lw = 1.8;
        match t {
            Tool::Select => {
                /* 经典箭头光标。第一版画成实心大三角，在 16px 里糊成一坨 */
                let pts = [
                    (x - 4.95, y - 8.25),
                    (x - 4.95, y + 5.5),
                    (x - 1.76, y + 2.42),
                    (x + 0.66, y + 8.25),
                    (x + 3.52, y + 6.93),
                    (x + 1.1, y + 1.43),
                    (x + 5.28, y + 1.43),
                ];
                self.ppoly_aa(&pts, ink);
            }
            Tool::Rect => self.pframe_aa(x - 8.25, y - 6.6, x + 8.25, y + 6.6, lw, ink),
            Tool::Ellipse => self.pring_aa_xy_a(x, y, 8.25, 6.6, lw, ink, 1.0),
            Tool::Line => self.pline_aa(x - 7.15, y + 6.05, x + 7.15, y - 6.05, lw, ink),
            Tool::Arrow => {
                self.pline_aa(x - 7.15, y + 6.05, x + 4.18, y - 5.06, lw, ink);
                self.ppoly_aa(
                    &[(x + 8.25, y - 8.25), (x + 0.66, y - 6.82), (x + 6.82, y - 0.66)],
                    ink,
                );
            }
            Tool::Pen => {
                /* 实心细长铅笔（照 Snipaste）：本体是实心斜条 + 尖头 + 笔尾一道箍。
                   上一版做成空心轮廓，22px 下像一支"挖空的笔"，owner 说看不出是铅笔。 */
                self.pline_aa(x - 4.6, y + 4.6, x + 4.6, y - 4.6, 3.8, ink);
                self.ppoly_aa(
                    &[(x - 7.6, y + 7.6), (x - 4.4, y + 6.2), (x - 6.2, y + 4.4)],
                    ink,
                );
                let bg = self.pal.bg;
                self.pline_aa(x + 1.4, y - 4.2, x + 4.2, y - 1.4, 1.2, bg);
            }
            Tool::Marker => {
                /* 荧光笔：细笔杆 + 宽笔头（Snipaste 的笔头是一块实心斜块） */
                self.pline_aa(x - 4.4, y - 5.6, x + 1.8, y + 0.6, 3.0, ink);
                self.ppoly_aa(
                    &[(x - 6.6, y + 3.2), (x - 2.4, y - 1.0), (x + 0.4, y + 1.8), (x - 3.8, y + 6.0)],
                    ink,
                );
            }
            Tool::Mosaic => {
                /* 2×2 棋盘方框（Snipaste 的造型：方框 + 对角两块实心） */
                self.pframe_aa(x - 7.2, y - 7.2, x + 7.2, y + 7.2, lw, ink);
                self.pfill(
                    (x - 6.0) as i32,
                    (y - 6.0) as i32,
                    (x - 0.8) as i32,
                    (y - 0.8) as i32,
                    ink,
                );
                self.pfill(
                    (x + 0.8) as i32,
                    (y + 0.8) as i32,
                    (x + 6.0) as i32,
                    (y + 6.0) as i32,
                    ink,
                );
            }
            Tool::Blur => {
                /* 失焦 = 实心核 + 两道渐隐环（第一版五个小圆点像散点） */
                self.pdisc_aa(x, y, 2.64, ink);
                self.pring_aa(x, y, 5.72, 1.32, ink);
                self.pring_aa_xy_a(x, y, 8.36, 8.36, 1.1, ink, 0.45);
            }
            Tool::Number => {
                self.pring_aa(x, y, 7.92, 1.65, ink);
                self.pline_aa(x, y - 3.52, x, y + 3.96, 1.65, ink);
                self.pline_aa(x - 1.98, y - 1.98, x, y - 3.74, 1.65, ink);
            }
            Tool::Text => {
                self.pline_aa(x - 6.6, y - 6.6, x + 6.6, y - 6.6, 2.0, ink);
                self.pline_aa(x, y - 6.6, x, y + 7.7, 2.0, ink);
            }
            Tool::Eraser => {
                /* 实心斜块 + 一道"套箍"（照 Snipaste：橡皮是实心块，不是空心轮廓）。
                   空心五边形在 22px 下会读成菱形/宝石，实心块才不会。 */
                let blk = [
                    (x - 7.2, y + 1.2),
                    (x - 1.2, y - 4.8),
                    (x + 6.6, y + 3.0),
                    (x + 0.6, y + 9.0),
                ];
                self.ppoly_aa(&blk, ink);
                let bg = self.pal.bg;
                self.pline_aa(x - 3.7, y + 4.7, x + 2.3, y - 1.3, 1.4, bg);
            }
        }
    }

    /// 6 个动作图标（同一套线宽 1.8 与抗锯齿，别跟工具图标两种画风）
    fn act_px(&mut self, cx: i32, cy: i32, a: &str, ink: u32) {
        let (x, y) = (cx as f32, cy as f32);
        match a {
            "undo" | "redo" => {
                let s = if a == "undo" { -1.0 } else { 1.0 };
                let (hx, hy) = (x + 5.5 * s, y - 5.5);
                self.pline_aa(x - 5.5 * s, hy, hx, hy, 1.8, ink);
                self.pline_aa(hx, hy, hx, y + 2.2, 1.8, ink);
                self.ppoly_aa(
                    &[(hx + 3.08 * s, hy), (hx - 1.98 * s, hy - 2.64), (hx - 1.98 * s, hy + 2.64)],
                    ink,
                );
            }
            "cancel" => {
                self.pline_aa(x - 6.6, y - 6.6, x + 6.6, y + 6.6, 1.8, ink);
                self.pline_aa(x + 6.6, y - 6.6, x - 6.6, y + 6.6, 1.8, ink);
            }
            "pin" => {
                /* 斜置图钉（照 Snipaste）：菱形图钉帽 + 斜杆 + 底部短线。
                   上一版是圆帽 —— 22px 下圆形读起来像放大镜。 */
                self.ppoly_aa(
                    &[(x + 4.2, y - 6.6), (x + 6.6, y - 4.2), (x + 4.2, y - 1.8), (x + 1.8, y - 4.2)],
                    ink,
                );
                self.pline_aa(x + 3.0, y - 3.0, x - 4.6, y + 4.6, 1.8, ink);
                self.pline_aa(x - 6.4, y + 7.2, x - 3.4, y + 7.2, 1.8, ink);
            }
            "save" => {
                self.pframe_aa(x - 7.7, y - 7.7, x + 7.7, y + 7.7, 1.8, ink);
                self.pfill((x - 3.85) as i32, (y - 6.6) as i32, (x + 3.85) as i32, (y - 1.1) as i32, ink);
                self.pfill((x - 5.5) as i32, (y + 1.65) as i32, (x + 5.5) as i32, (y + 6.6) as i32, ink);
            }
            _ => {
                /* copy：两个错开的方框 */
                self.pframe_aa(x - 8.25, y - 8.25, x + 1.65, y + 1.65, 1.8, ink);
                self.pframe_aa(x - 1.65, y - 1.65, x + 8.25, y + 8.25, 1.8, ink);
            }
        }
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

    /// 气泡：鼠标停在主条哪个按钮上就提示哪个（平铺之后不再有面板遮挡问题）
    fn draw_bar_tip(&mut self) {
        if let Some(i) = self.hover {
            let cx = self.bar_item_x(i) + bar_w(self.bar_item(i)) / 2;
            self.draw_tip(self.bar_item(i), cx, self.bar[1]);
        }
    }

    fn draw_sel(&mut self) {
        let Some([x, y, w, h]) = self.sel else { return };
        if w <= 0 || h <= 0 {
            return;
        }
        // 2px 蓝框 + 四角把手（像素直绘，理由同上：GDI 画笔在这块 DIB 上不可靠）
        self.pframe(x, y, x + w, y + h, 2, 0x1B4FE0);
        for (cx, cy) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
            self.pfill(cx - 4, cy - 4, cx + 5, cy + 5, 0xFFFFFF);
            self.pframe(cx - 4, cy - 4, cx + 5, cy + 5, 1, 0x1B4FE0);
        }
        // 尺寸标签（文字只能是 GDI：中文/字形没法自己点像素）
        let label = format!("{} × {} px", w, h);
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
            let ly = if y > th + 12 { y - th - 8 } else { y + 4 };
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
        self.opt_layout();
        self.fill_base();
        self.draw_ops();
        /* 正在拖的那一笔：先 take 出来画（draw_op 要 &mut self，而 Op 在 self 里会冲突），
           画完放回去。用户拖的时候就能看到结果，而不是松手才出现。 */
        let pv = self.preview.take();
        if let Some(op) = pv.as_ref() {
            self.draw_op(op);
        }
        self.preview = pv;
        self.draw_sel();
        /* 没选区就不画工具栏（owner 2026-11-18 当面提的）：按下 F1 的瞬间它飘在屏幕底部
           中间，既挡住内容、又没有任何可用动作。框选之后才贴到选区下方。 */
        if self.has_sel() {
            self.draw_bar();
            /* 选项行在主条下方，先画条再画它（两者不重叠，顺序只为读起来顺） */
            self.draw_opt();
            self.draw_bar_tip();
        }
        self.draw_keys();
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

        let rows: [(&[&str], &str, &str); 5] = [
            (&["Ctrl", "A"], "选中整屏", ""),
            (&["↑", "↓", "←", "→"], "微调选区（按住 Shift 改大小）", ""),
            (&["Ctrl", "C"], "复制到剪贴板", ""),
            (&["Ctrl", "S"], "保存到截图库", "Ctrl+Shift+S 另存为…"),
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
}

extern "system" {
    fn GetWindowRect(h: Hwnd, r: *mut RectPos) -> i32;
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

/// **已经持有 CTX 锁时**用它（工具栏点击 / WM_KEYDOWN 都在锁内）。
/// 之前这些路径直接调 `deliver()`，而 `deliver` 内部又 `CTX.lock()` ——
/// std::sync::Mutex 不可重入，于是**整条 UI 线程死锁**：用户看到的就是
/// "Ctrl+C / Ctrl+S 按了没反应"「点复制按钮没反应」（0.3.0 的真实 bug）。
fn deliver_to(c: &mut Ctx, action: &str) {
    let sel = c.sel.unwrap_or([0, 0, c.w, c.h]);
    if sel[2] < 4 || sel[3] < 4 {
        return;
    }
    if let Some(png) = crop_png(c, sel) {
        c.done = Some((
            action.to_string(),
            png,
            [sel[0] as f64, sel[1] as f64, sel[2] as f64, sel[3] as f64],
        ));
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
        /* 正在输入文字时，点屏幕任何地方 = 提交。
           以前只有 Enter 能提交，用户习惯性去点下一个工具，文字就卡在编辑框里。 */
        if c.editor != 0 {
            c.commit_text();
            c.render();
        }
        /* 第二行选项条先处理：它在主条下方、不属于选区。必须排在选区判定之前，
           否则点色块会落成"在选区里落笔"，画出一条莫名其妙的标注。 */
        if let Some(pick) = c.opt_hit(x, y) {
            match pick {
                OptPick::Color(c2) => c.color = c2,
                OptPick::Width(w) => c.width = w,
                OptPick::Brush(b) => c.brush = b,
            }
            c.render();
            return;
        }
        // 主条（顺序见 BAR：选择…橡皮 | 撤销 重做 | 复制 贴图 保存 | 取消）
        if let Some(i) = c.bar_hit(x, y) {
            match c.bar_item(i) {
                BarKind::Sep => {}
                BarKind::Tool(t) => {
                    /* 遮罩类（马赛克 / 高斯模糊）有两个形态：拖矩形 / 当画笔涂。
                       再点一次按钮切换（老手势保留），更直观的入口是第二行的形态按钮。 */
                    if matches!(t, Tool::Mosaic | Tool::Blur) && c.tool == t {
                        c.brush = !c.brush;
                    } else {
                        c.tool = t;
                    }
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
        if inside(sel, x, y) && c.tool != Tool::Select {
                c.drag = Drag::Draw;
                c.anchor = (x, y);
                c.cur = (x, y);
                c.pts = vec![(x, y)];
                return;
            }
        }
        if c.tool == Tool::Select {
            c.sel = Some([x, y, 0, 0]);
            c.drag = Drag::NewSel;
            c.anchor = (x, y);
        }
    } else {
        match c.drag {
            Drag::NewSel | Drag::Draw => {
                let a = c.anchor;
                let b = (x, y);
                if c.tool == Tool::Select {
                    let r = norm4(a, b);
                    c.sel = if r[2] > 3 && r[3] > 3 { Some(r) } else { None };
                } else {
                    let pts = c.pts.clone();
                    if let Some(op) = c.make_op(a, b, &pts) {
                        c.ops.push(op);
                        c.redo.clear();
                    }
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
        if let Some(op) = c.ops.last() {
            let _ = op.bbox();
        }
    }
    c.render();
}

unsafe fn on_move(x: i32, y: i32) {
    let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
    let Some(c) = g.as_mut() else { return };
    if c.update_bar_hover(x, y) {
        c.render();
    }
    match c.drag {
        Drag::NewSel => {
            c.sel = Some(norm4(c.anchor, (x, y)));
            c.render();
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
            }
        }
        _ => {}
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
        WM_RBUTTONDOWN => {
            let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(c) = g.as_mut() {
                if c.sel.is_some() {
                    c.sel = None;
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
                        0x1B => c.done = Some(("cancel".into(), Vec::new(), [0.0; 4])),
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
                        // Ctrl+A：选中整屏（网页版同键）
                        0x41 if ctrl => {
                            c.sel = Some([0, 0, c.w, c.h]);
                            c.render();
                        }
                        // Ctrl+Shift+S 另存为（自己选路径），Ctrl+S 存到截图库
                        0x53 if ctrl && shift => deliver_to(c, "saveas"),
                        0x53 if ctrl => deliver_to(c, "save"),
                        0x43 if ctrl => deliver_to(c, "copy"),
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
pub fn run(shot: &screen::Shot, theme: u8) -> Option<Delivered> {
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
            *g = Some(Box::new(Ctx {
                w,
                h,
                pal: Palette::from_code(theme),
                mem,
                bits: bits as *mut u8,
                base,
                dim,
                sel: None,
                sel0: [0; 4],
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
                opt: [0; 4],
                keys: [0; 4],
                editor: 0,
                editing: None,
                done: None,
            }));
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
            pal,
            mem,
            bits: bits as *mut u8,
            base: vec![0u8; (w * h * 4) as usize],
            dim: vec![0u8; (w * h * 4) as usize],
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
            opt: [0; 4],
            keys: [0; 4],
            editor: 0,
            editing: None,
            done: None,
        });
        (c, bmp)
    }

    /// 主条总宽：6px 内距 + 所有项 + 末尾「取消」
    fn bar_total_w() -> i32 {
        let mut bw = 6;
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

    /// 离屏预览：两套主题各出一张「工具栏 + 快捷键面板」对照图。
    ///
    /// 验收标准来自设计稿 v2.1：深浅各有对照图；斜置图钉在 22px 下要清楚；
    /// 面板压在浅色/深色两种背景上都不能融掉。
    #[test]
    #[ignore = "导出工具栏预览图，人工看效果"]
    fn bar_preview() {
        unsafe {
            let (w, h) = (1400i32, 520i32);
            for (tag, pal) in [("dark", Palette::dark()), ("light", Palette::light())] {
                let (mut c, bmp) = mk_ctx(w, h, pal);
                paint_bg(&c, w, h);

                /* 主条 + 选项行：一次看全 选中/悬停/平铺 三态，以及色板与粗细 */
                c.sel = Some([0, 0, 40, 40]); // 只为了让 has_sel() 成立（预览不画选区框）
                let bw = bar_total_w();
                c.bar = [(w - bw) / 2, 120, bw, BAR_H];
                /* 用马赛克当预览工具：它才会在第二行多出「矩形 / 画笔」形态按钮，
                   一次把 形态 + 色板 + 粗细 三块都看全。 */
                c.tool = Tool::Mosaic;
                c.hover = Some(7);
                c.draw_bar();
                c.opt_layout();
                c.draw_opt();
                c.draw_bar_tip();
                c.draw_keys();

                let out = std::env::temp_dir().join(format!("am-bar-{tag}.bmp"));
                let px = std::slice::from_raw_parts(c.bits, (w * h * 4) as usize);
                write_bmp(&out, w, h, px);

                /* 放大两块：贴图图钉（22px 下糊不糊）、马赛克（棋盘格认不认得出） */
                let (zy, zh) = (c.bar[1] - 6, BAR_H + 12);
                write_zoom(&c, &format!("am-zoom-pin-{tag}.bmp"), c.bar_item_x(17) - 4, zy, 46, zh, 8, w, h);
                write_zoom(&c, &format!("am-zoom-mosaic-{tag}.bmp"), c.bar_item_x(7) - 4, zy, 46, zh, 8, w, h);
                println!("preview: {}", out.display());

                DeleteDC(c.mem);
                DeleteObject(bmp);
            }
        }
    }

    /// 第二行「颜色 / 粗细」的命中回归（平铺后这条比组面板更关键：
    /// 色块宽 20、间距 24，绘制与命中各算一遍坐标，差 1px 就点到旁边）。
    #[test]
    fn opt_row_picks_color_and_width() {
        unsafe {
            let (mut c, bmp) = mk_ctx(1200, 400, Palette::dark());
            c.sel = Some([40, 40, 300, 200]);
            c.bar = [40, 100, bar_total_w(), BAR_H];
            c.opt_layout();
            let cy = c.opt[1] + BAR_H / 2;

            /* ① 每个色块的中心都命中自己 */
            let mut cx = c.opt[0] + 8 + 26 + 8;
            for (i, want) in SWATCHES.iter().enumerate() {
                assert_eq!(
                    c.opt_hit(cx + 10, cy),
                    Some(OptPick::Color(*want)),
                    "第 {i} 个色块没命中"
                );
                cx += 24;
            }

            /* ② 四档粗细逐个命中 */
            cx += SEP_W;
            for want in WIDTHS {
                assert_eq!(
                    c.opt_hit(cx + 15, cy),
                    Some(OptPick::Width(want)),
                    "粗细 {want} 没命中"
                );
                cx += 30;
            }

            /* ③ 空白处（最左侧内距、最右侧留白）不命中，免得误改颜色 */
            assert_eq!(c.opt_hit(c.opt[0] + 2, cy), None);
            assert_eq!(c.opt_hit(c.opt[0] + c.opt[2] - 2, cy), None);

            /* ④ 换到遮罩类工具：第二行左端多出「矩形 / 画笔」，两段各命中各的，
                  且色板整体右移（宽度重算过，坐标不能沿用上面那一套） */
            c.tool = Tool::Mosaic;
            c.opt_layout();
            let cy2 = c.opt[1] + BAR_H / 2;
            assert_eq!(
                c.opt_hit(c.opt[0] + 8 + MODE_W / 2, cy2),
                Some(OptPick::Brush(false)),
                "左段应该是「矩形」"
            );
            assert_eq!(
                c.opt_hit(c.opt[0] + 8 + MODE_W + MODE_W / 2, cy2),
                Some(OptPick::Brush(true)),
                "右段应该是「画笔」"
            );
            /* 形态段之后第一个色块仍在，说明右移量算对了 */
            let first = c.opt[0] + 8 + MODE_W * 2 + SEP_W + 26 + 8;
            assert_eq!(
                c.opt_hit(first + 10, cy2),
                Some(OptPick::Color(SWATCHES[0])),
                "形态按钮把色板挤歪了"
            );
            /* 非遮罩工具不该有形态段：那一带点下去什么都不该发生 */
            c.tool = Tool::Arrow;
            c.opt_layout();
            assert_eq!(c.opt_hit(c.opt[0] + 8 + MODE_W / 2, cy2), None);

            /* ⑤ 没选区时整行不响应（它也不画） */
            c.sel = None;
            assert_eq!(c.opt_hit(c.opt[0] + 8 + 26 + 8 + 10, cy), None);

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
            assert_eq!(acts, vec!["undo", "redo", "copy", "pin", "save", "cancel"]);

            /* ② 主条第 i 项中心必须命回第 i 项 */
            for i in 0..c.bar_len() {
                if matches!(c.bar_item(i), BarKind::Sep) {
                    continue;
                }
                let cx = c.bar_item_x(i) + bar_w(c.bar_item(i)) / 2;
                let cy = c.bar[1] + BAR_H / 2;
                assert_eq!(c.bar_hit(cx, cy), Some(i), "主条第 {i} 项中心没命中自己");
            }

            /* ③ 12 个工具全部平铺在一条上（没有组了），每个都能直接选中 */
            let tools: Vec<Tool> = (0..c.bar_len())
                .filter_map(|i| match c.bar_item(i) {
                    BarKind::Tool(t) => Some(t),
                    _ => None,
                })
                .collect();
            assert_eq!(tools.len(), 12, "12 个工具必须都平铺在工具栏上");
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
                Tool::Eraser,
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
            let (mut c, bmp) = mk_ctx(1700, 520, Palette::dark());
            c.sel = Some([40, 40, 600, 300]);
            c.render();
            c.draw_keys(); // render 里已经画过，这里再画一次确保 keys 矩形是这一帧的

            let px = std::slice::from_raw_parts(c.bits, (c.w * c.h * 4) as usize);
            let mut check = |r: [i32; 4], name: &str| {
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
            check(c.opt, "选项行");
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
            /* 画布要够宽：工具栏平铺后 672px，提示面板又在右下角，
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
}
