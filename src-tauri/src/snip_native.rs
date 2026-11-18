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

/* ---------------- 工具栏模型 ----------------

   12 个工具压成 6 个按钮（照 Snipaste 的分组）：单个工具直接显示，同类工具合成一组，
   组按钮画的是「这一组当前在用的那个」，右下角一个小三角表示「这里还有更多」。
   鼠标停在组按钮上 → 在它上方展开整组（`panel_layout` / `draw_panel`），点组内任意一项即切换。

   为什么不摊成一长排：12 工具 + 6 动作 = 18 个按钮约 620px，在 1366 宽的笔记本上会压掉
   小半个选区；分组后 12 个按钮约 390px。常用工具（矩形、箭头）仍然一步可达 ——
   组按钮本身就是「你上次用的那个」，点它等于直接用。 */

/// 形状：矩形 / 椭圆 / 直线 / 箭头
const G_SHAPE: &[Tool] = &[Tool::Rect, Tool::Ellipse, Tool::Line, Tool::Arrow];
/// 手绘：铅笔 / 记号笔
const G_DRAW: &[Tool] = &[Tool::Pen, Tool::Marker];
/// 打码：马赛克 / 高斯模糊
const G_MASK: &[Tool] = &[Tool::Mosaic, Tool::Blur];
/// 标记：文字 / 序号
const G_MARK: &[Tool] = &[Tool::Text, Tool::Number];

#[derive(Clone, Copy, PartialEq)]
enum BarKind {
    Tool(Tool),
    /// 工具组：折叠时显示当前工具，悬浮展开 `members`
    Group(&'static [Tool]),
    Sep,
    Act(&'static str),
}

const BAR: [BarKind; 13] = [
    BarKind::Tool(Tool::Select),
    BarKind::Group(G_SHAPE),
    BarKind::Group(G_DRAW),
    BarKind::Group(G_MASK),
    BarKind::Group(G_MARK),
    BarKind::Tool(Tool::Eraser),
    BarKind::Sep,
    BarKind::Act("undo"),
    BarKind::Act("redo"),
    BarKind::Sep,
    BarKind::Act("cancel"),
    BarKind::Act("pin"),
    BarKind::Act("save"),
];

/// 「复制」固定放最末尾（放进 BAR 会让它和「保存」的归属说不清）
const BAR_TAIL: BarKind = BarKind::Act("copy");

/* ---------------- 工具栏配色（照主界面 tokens.css 的纸墨体系） ----------------

   主界面是深色纸墨：`--ui-menu #1c1c1a` / `--ui-line #2a2a27` / `--accent #5b8cff`。
   以前工具栏是浅底 + 亮蓝边（网页版 `.snip-bar` 的残留），浮在深色界面上像外挂控件。

   深色面板会不会"糊在深色截图里"？0.2.x 踩过这个坑，所以这里用了三层边界：
   ① 面板底比黑更浅一档，靠 ② 的亮边定轮廓；
   ② 描边 `#3a3a37`，比 `--ui-line` 亮两档 —— 专门用来在深色背景上勾边；
   ③ 外面再压 1px 纯黑 + 三层柔影 —— 在浅色截图/浅色桌面上靠影子分离。
   三层缺一层就会出现"在某类背景上看不见"的老问题。 */
const BAR_BG: u32 = 0x1C1C1A;
const BAR_EDGE: u32 = 0x3A3A37;
const BAR_EDGE_OUT: u32 = 0x000000;
const BAR_ICON: u32 = 0xE6E6E2;
/// 分隔线：比 `BAR_BG` 亮三档。第一版用的 `#333330`，离屏预览里几乎看不见
const BAR_SEP: u32 = 0x3F3F3B;
/// 悬停底：必须比 `BAR_BG` 明确亮一档，否则"鼠标到底停在哪"看不出来
const BAR_HOVER: u32 = 0x2E2E2B;
/// 选中底：accent 22% 叠在 `BAR_BG` 上取实色（像素直绘没有 alpha 合成层，只能先算好）
const BAR_ACTIVE: u32 = 0x2E3A5C;
const BAR_DANGER: u32 = 0xE05A4A;
/// 「还有更多」的小三角；未选中时用它，选中时用纯白
const BAR_TRI: u32 = 0x8A8A84;
/// 工具条与展开面板的圆角
const BAR_RADIUS: i32 = 8;
const BAR_H: i32 = 34;

fn bar_w(k: BarKind) -> i32 {
    match k {
        BarKind::Sep => 7,
        _ => 31,
    }
}

/// 单个工具的名字与快捷键（组内 tooltip 与单工具按钮共用）
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
        /* 组按钮按「组」说，不按当前工具说 —— 否则用户不知道这里还藏着别的工具 */
        BarKind::Group(m) => match m[0] {
            Tool::Rect => ("形状", "矩形 / 椭圆 / 直线 / 箭头"),
            Tool::Pen => ("手绘", "铅笔 / 记号笔"),
            Tool::Mosaic => ("打码", "马赛克 / 高斯模糊"),
            _ => ("标记", "文字 / 序号"),
        },
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
    Line([i32; 4], u32, i32),
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
            Op::Rect(a, _, _) | Op::Ellipse(a, _, _) | Op::Line(a, _, _) | Op::Mosaic(a) => *a,
            Op::Blur(a) => *a,
            Op::Arrow(p0, p1, _, w) => {
                let r = norm4(*p0, *p1);
                let pad = w + 2;
                [r[0] - pad, r[1] - pad, r[2] + pad * 2, r[3] + pad * 2]
            }
            Op::Number(x, y, _, _) => [*x - 14, *y - 14, 28, 28],
            Op::MosaicPath(p, r) => {
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
    /// 马赛克模式：false = 拖矩形，true = 当画笔随手涂
    mosaic_brush: bool,
    /// 下一个序号（放下一个数字标记时 +1）
    number_next: i32,
    /// 主条上悬停的按钮下标（分隔条不算）
    hover: Option<usize>,
    /// 展开的面板属于主条哪一项；None = 没展开
    open_group: Option<usize>,
    /// 面板矩形。`panel_layout` 算一次存下来，绘制与命中共用 —— 两边各算一遍迟早会差 1px。
    panel: [i32; 4],
    /// 面板内悬停第几项
    panel_hover: Option<usize>,
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
            Op::Line(r, c, w) => self.pline(r[0], r[1], r[0] + r[2], r[1] + r[3], *w, *c),
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

    /// 工具栏布局（尺寸照网页版 `.snip-bar`：高 34、按钮 31×32、圆角、蓝边）。
    /// 把"起点 + 当前点 + 路径"变成一条标注。
    /// 拖动过程（预览）和松手（落库）都走它 —— 以前两边各写一份 match，
    /// 加一种工具就要改两处，漏一处就"预览有、松手没了"。
    fn make_op(&self, a: (i32, i32), b: (i32, i32), pts: &[(i32, i32)]) -> Option<Op> {
        let r = norm4(a, b);
        match self.tool {
            Tool::Rect => Some(Op::Rect(r, self.color, self.width)),
            Tool::Ellipse => Some(Op::Ellipse(r, self.color, self.width)),
            Tool::Line => Some(Op::Line(r, self.color, self.width)),
            Tool::Arrow => Some(Op::Arrow(a, b, self.color, self.width)),
            Tool::Pen => Some(Op::Pen(pts.to_vec(), self.color, self.width)),
            Tool::Marker => Some(Op::Marker(pts.to_vec(), self.color, self.width + 8)),
            Tool::Mosaic => {
                if self.mosaic_brush {
                    /* 画笔式：路径经过的地方都变成马赛克（owner 2026-11-18 要的），
                       和拖矩形是同一个工具，再点一次按钮切换。 */
                    let mut p = pts.to_vec();
                    p.push(b);
                    Some(Op::MosaicPath(p, 9))
                } else if r[2] > 2 || r[3] > 2 {
                    Some(Op::Mosaic(r))
                } else {
                    None
                }
            }
            Tool::Blur => {
                if r[2] > 3 && r[3] > 3 {
                    Some(Op::Blur(r))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

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

    /// 命中：返回按钮下标；点在分隔条上也返回它的下标（调用方忽略 Sep）
    fn bar_hit(&self, x: i32, y: i32) -> Option<usize> {
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

    /// 组按钮上该画哪个图标：组里当前在用的那个；当前工具不在组里就用第一个。
    /// 这是"点组按钮 = 直接用上次用的工具"能成立的前提。
    fn group_cur(&self, members: &[Tool]) -> Tool {
        if members.contains(&self.tool) {
            self.tool
        } else {
            members[0]
        }
    }

    /* ---------- 展开面板 ----------

       Snipaste 的手感：鼠标停在组按钮上，这一组就在按钮上方铺开，点其中一项即切换。
       面板与主条之间留 6px 空隙，所以命中判定必须把这段空隙也算进来（`in_panel_zone`）；
       否则从按钮移向面板的途中会先判定"都不在" → 面板消失 → 永远点不到。 */

    fn panel_layout(&mut self, i: usize) {
        let BarKind::Group(members) = self.bar_item(i) else {
            return;
        };
        let pw = members.len() as i32 * 31 + 6;
        let bx = self.bar_item_x(i) + bar_w(self.bar_item(i)) / 2 - pw / 2;
        let bx = bx.max(4).min((self.w - pw - 4).max(4));
        /* 默认贴在主条上方 6px；上面放不下（主条贴着屏幕顶）才翻到下方 */
        let above = self.bar[1] - BAR_H - 6;
        let by = if above >= 4 {
            above
        } else {
            self.bar[1] + self.bar[3] + 6
        };
        self.panel = [bx, by, pw, BAR_H];
    }

    fn panel_hit(&self, x: i32, y: i32) -> Option<usize> {
        let gi = self.open_group?;
        let BarKind::Group(members) = self.bar_item(gi) else {
            return None;
        };
        let [px, py, pw, ph] = self.panel;
        if x < px || x >= px + pw || y < py || y >= py + ph {
            return None;
        }
        let i = ((x - px - 3) / 31).max(0) as usize;
        (i < members.len()).then_some(i)
    }

    /// 面板本身 + 面板到主条之间的空隙（整条竖带），用于"别让面板在半路上消失"
    fn in_panel_zone(&self, x: i32, y: i32) -> bool {
        let [px, py, pw, ph] = self.panel;
        let (top, bot) = if py < self.bar[1] {
            (py, self.bar[1] + self.bar[3])
        } else {
            (self.bar[1], py + ph)
        };
        x >= px && x < px + pw && y >= top && y < bot
    }

    /// 鼠标移动时的工具栏 / 面板命中状态更新。返回是否需要重绘。
    ///
    /// 为什么单独一个方法：悬浮展开这一块连着出了三次"展不开"（0.3.1 之前没有展开、
    /// 0.3.2 的 `panel_muted` 锁死、0.3.3 点完强制关面板），靠"装机看一眼"成本太高。
    /// 抽出来后能用单测直接钉住"悬浮 → 展开 → 穿过空隙 → 点组内项"整条链路。
    fn update_bar_hover(&mut self, x: i32, y: i32) -> bool {
        let mut dirty = false;
        let bar_i = self
            .bar_hit(x, y)
            .filter(|i| !matches!(self.bar_item(*i), BarKind::Sep));
        if let Some(i) = bar_i {
            match self.bar_item(i) {
                BarKind::Group(_) => {
                    if self.open_group != Some(i) {
                        self.panel_layout(i);
                        self.open_group = Some(i);
                        dirty = true;
                    }
                }
                _ => {
                    if self.open_group.is_some() {
                        self.open_group = None;
                        dirty = true;
                    }
                }
            }
            if self.hover != Some(i) {
                self.hover = Some(i);
                dirty = true;
            }
            if self.panel_hover.take().is_some() {
                dirty = true;
            }
        } else if self.open_group.is_some() && self.in_panel_zone(x, y) {
            /* 面板 + 到主条的那段空隙：都算"还在面板区"，否则鼠标移过去途中面板会闪没 */
            let ph = self.panel_hit(x, y);
            if self.panel_hover != ph {
                self.panel_hover = ph;
                dirty = true;
            }
            if self.hover.take().is_some() {
                dirty = true;
            }
        } else {
            if self.open_group.take().is_some() {
                dirty = true;
            }
            if self.panel_hover.take().is_some() {
                dirty = true;
            }
            if self.hover.take().is_some() {
                dirty = true;
            }
        }
        dirty
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

    /// 主工具条。配色与层次见 `BAR_BG` 那一段。
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
                    self.pfill(px, y + 9, px + 1, y + h - 9, BAR_SEP);
                }
                BarKind::Tool(t) => {
                    let active = t == self.tool;
                    self.bar_btn(ix, y, iw, h, active, hover, false);
                    let ink = if active { 0xFFFFFF } else { BAR_ICON };
                    self.icon_px(ix + iw / 2, y + h / 2, t, ink);
                }
                BarKind::Group(members) => {
                    /* 图标画"这一组当前在用的那个" —— 组按钮于是本身就是快捷入口：
                       点它 = 用上次那个工具，想换才需要展开。 */
                    let active = members.contains(&self.tool);
                    self.bar_btn(ix, y, iw, h, active, hover, true);
                    let cur = self.group_cur(members);
                    let ink = if active { 0xFFFFFF } else { BAR_ICON };
                    self.icon_px(ix + iw / 2, y + h / 2, cur, ink);
                }
                BarKind::Act(a) => {
                    if hover {
                        let bg = if a == "cancel" { BAR_DANGER } else { BAR_HOVER };
                        self.pfill_round(ix + 2, y + 3, ix + iw - 2, y + h - 3, 5, bg);
                    }
                    let ink = match (a, hover) {
                        ("cancel", true) => 0xFFFFFF,
                        ("cancel", false) => BAR_DANGER,
                        _ => BAR_ICON,
                    };
                    self.act_px(ix + iw / 2, y + h / 2, a, ink);
                }
            }
        }
    }

    /// 一块浮层底（主条与展开面板共用）：柔影 → 1px 纯黑外沿 → 1px 亮边 → 底。
    /// 三层的必要性见 `BAR_BG` 上面那段：只画其中一层，总有一类背景会糊掉。
    fn bar_plate(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32) {
        let (x1, y1) = (x + w, y + h);
        for k in 1..=3 {
            let a = 0.09 / k as f32;
            self.pfill_round_a(x - k, y - k + 2, x1 + k, y1 + k + 2, r + k, 0x000000, a);
        }
        self.pfill_round(x - 1, y - 1, x1 + 1, y1 + 1, r + 1, BAR_EDGE_OUT);
        self.pfill_round(x, y, x1, y1, r, BAR_EDGE);
        self.pfill_round(x + 1, y + 1, x1 - 1, y1 - 1, (r - 1).max(1), BAR_BG);
    }

    /// 一个工具按钮的底：选中 / 悬停 / 普通，外加"还有更多"的小三角
    fn bar_btn(&mut self, ix: i32, y: i32, iw: i32, h: i32, active: bool, hover: bool, more: bool) {
        let (x0, y0, x1, y1) = (ix + 2, y + 3, ix + iw - 2, y + h - 3);
        if active {
            self.pfill_round(x0, y0, x1, y1, 5, BAR_ACTIVE);
        } else if hover {
            self.pfill_round(x0, y0, x1, y1, 5, BAR_HOVER);
        }
        if more {
            /* "还有更多"的三角。两点经验：
               ① 颜色不能在选中时还用蓝（蓝底蓝三角 = 看不见）；
               ② 贴到内角、画 6px —— 再大就会和图标抢注意力。 */
            let (bx, by) = (ix + iw - 6, y + h - 6);
            let pts = [(bx, by), (bx - 5, by), (bx, by - 5)];
            self.ppoly(&pts, if active { 0xFFFFFF } else { BAR_TRI });
        }
    }

    /// 展开的组面板（在 `draw_bar` 之后画，永远压在主条上层）
    fn draw_panel(&mut self) {
        let Some(gi) = self.open_group else { return };
        let BarKind::Group(members) = self.bar_item(gi) else {
            return;
        };
        let [px, py, pw, ph] = self.panel;
        self.bar_plate(px, py, pw, ph, BAR_RADIUS);
        for (i, t) in members.iter().enumerate() {
            let ix = px + 3 + i as i32 * 31;
            let active = *t == self.tool;
            let hover = self.panel_hover == Some(i);
            if active {
                self.pfill_round(ix + 2, py + 3, ix + 29, py + ph - 3, 5, BAR_ACTIVE);
            } else if hover {
                self.pfill_round(ix + 2, py + 3, ix + 29, py + ph - 3, 5, BAR_HOVER);
            }
            let ink = if active { 0xFFFFFF } else { BAR_ICON };
            self.icon_px(ix + 15, py + ph / 2, *t, ink);
        }
    }

    /// 10 个工具图标（约 20×20，线宽 2）
    /// 12 个工具图标 —— 24px 网格、线宽 1.5、全部抗锯齿。
    ///
    /// 为什么重画（owner 2026-11-18「画出来的太丑」）：第一版是 ±9px 的硬像素，
    /// 斜线呈阶梯、圆边有锯齿，图标还几乎撑满按钮，整体像像素画。
    /// 现在统一收进 ±7.5，四周留 8px 呼吸空间，斜线/圆/三角全部走覆盖率绘制。
    fn icon_px(&mut self, cx: i32, cy: i32, t: Tool, ink: u32) {
        let (x, y) = (cx as f32, cy as f32);
        let lw = 1.5;
        match t {
            Tool::Select => {
                /* 经典箭头光标。第一版画成实心大三角，在 16px 里糊成一坨 */
                let pts = [
                    (x - 4.5, y - 7.5),
                    (x - 4.5, y + 5.0),
                    (x - 1.6, y + 2.2),
                    (x + 0.6, y + 7.5),
                    (x + 3.2, y + 6.3),
                    (x + 1.0, y + 1.3),
                    (x + 4.8, y + 1.3),
                ];
                self.ppoly_aa(&pts, ink);
            }
            Tool::Rect => self.pframe_aa(x - 7.5, y - 6.0, x + 7.5, y + 6.0, lw, ink),
            Tool::Ellipse => self.pring_aa_xy_a(x, y, 7.5, 6.0, lw, ink, 1.0),
            Tool::Line => self.pline_aa(x - 6.5, y + 5.5, x + 6.5, y - 5.5, lw, ink),
            Tool::Arrow => {
                self.pline_aa(x - 6.5, y + 5.5, x + 3.8, y - 4.6, lw, ink);
                self.ppoly_aa(
                    &[(x + 7.5, y - 7.5), (x + 0.6, y - 6.2), (x + 6.2, y - 0.6)],
                    ink,
                );
            }
            Tool::Pen => {
                /* 铅笔 = 细长笔身 + 小尖头（第一版"斜线+大三角"像一支矛） */
                self.pline_aa(x - 4.5, y + 4.5, x + 5.0, y - 5.0, 3.2, ink);
                self.ppoly_aa(
                    &[(x - 7.5, y + 7.5), (x - 4.8, y + 5.8), (x - 5.8, y + 4.8)],
                    ink,
                );
                self.pline_aa(x - 6.6, y + 6.6, x + 2.6, y - 2.6, 1.0, 0x1C1C1A);
            }
            Tool::Marker => {
                /* 记号笔 = 斜笔身 + 下面一道粗线（荧光笔语义） */
                self.pline_aa(x - 3.8, y - 5.8, x + 3.8, y + 1.8, 4.0, ink);
                self.pline_aa(x - 7.0, y + 6.5, x + 7.0, y + 6.5, 2.0, ink);
            }
            Tool::Mosaic => {
                /* 3×3 交错五格，比"四块 + 一个角块"更像像素化 */
                let (cell, step) = (4, 5);
                for (gx, gy) in [(0i32, 0i32), (1, 0), (2, 1), (0, 2), (1, 2)] {
                    let px = cx + (gx - 1) * step - cell / 2;
                    let py = cy + (gy - 1) * step - cell / 2;
                    self.pfill(px, py, px + cell, py + cell, ink);
                }
            }
            Tool::Blur => {
                /* 失焦 = 实心核 + 两道渐隐环（第一版五个小圆点像散点） */
                self.pdisc_aa(x, y, 2.4, ink);
                self.pring_aa(x, y, 5.2, 1.2, ink);
                self.pring_aa_xy_a(x, y, 7.6, 7.6, 1.0, ink, 0.45);
            }
            Tool::Number => {
                self.pring_aa(x, y, 7.2, 1.5, ink);
                self.pline_aa(x, y - 3.2, x, y + 3.6, 1.5, ink);
                self.pline_aa(x - 1.8, y - 1.8, x, y - 3.4, 1.5, ink);
            }
            Tool::Text => {
                self.pline_aa(x - 6.0, y - 6.0, x + 6.0, y - 6.0, 1.8, ink);
                self.pline_aa(x, y - 6.0, x, y + 7.0, 1.8, ink);
            }
            Tool::Eraser => {
                /* 斜放的橡皮块 + 内部一道分隔线 */
                let pts = [
                    (x - 7.0, y + 0.6),
                    (x - 1.6, y - 5.6),
                    (x + 6.4, y + 1.4),
                    (x + 1.0, y + 7.6),
                ];
                for i in 0..4 {
                    let (ax, ay) = pts[i];
                    let (bx, by) = pts[(i + 1) % 4];
                    self.pline_aa(ax, ay, bx, by, 1.5, ink);
                }
                self.pline_aa(x - 3.2, y + 4.8, x + 1.6, y - 0.4, 1.2, ink);
            }
        }
    }

    /// 6 个动作图标（同一套线宽与抗锯齿，别跟工具图标两种画风）
    fn act_px(&mut self, cx: i32, cy: i32, a: &str, ink: u32) {
        let (x, y) = (cx as f32, cy as f32);
        match a {
            "undo" | "redo" => {
                let s = if a == "undo" { -1.0 } else { 1.0 };
                let (hx, hy) = (x + 5.0 * s, y - 5.0);
                self.pline_aa(x - 5.0 * s, hy, hx, hy, 1.6, ink);
                self.pline_aa(hx, hy, hx, y + 2.0, 1.6, ink);
                self.ppoly_aa(
                    &[(hx + 2.8 * s, hy), (hx - 1.8 * s, hy - 2.4), (hx - 1.8 * s, hy + 2.4)],
                    ink,
                );
            }
            "cancel" => {
                self.pline_aa(x - 6.0, y - 6.0, x + 6.0, y + 6.0, 1.8, ink);
                self.pline_aa(x + 6.0, y - 6.0, x - 6.0, y + 6.0, 1.8, ink);
            }
            "pin" => {
                self.pframe_aa(x - 3.5, y - 7.0, x + 3.5, y - 0.5, 1.6, ink);
                self.pline_aa(x, y - 0.5, x, y + 7.0, 1.6, ink);
            }
            "save" => {
                self.pframe_aa(x - 7.0, y - 7.0, x + 7.0, y + 7.0, 1.6, ink);
                self.pfill((x - 3.5) as i32, (y - 6.0) as i32, (x + 3.5) as i32, (y - 1.0) as i32, ink);
                self.pfill((x - 5.0) as i32, (y + 1.5) as i32, (x + 5.0) as i32, (y + 6.0) as i32, ink);
            }
            _ => {
                /* copy：两个错开的方框 */
                self.pframe_aa(x - 7.5, y - 7.5, x + 1.5, y + 1.5, 1.5, ink);
                self.pframe_aa(x - 1.5, y - 1.5, x + 7.5, y + 7.5, 1.5, ink);
            }
        }
    }

    /// 悬停气泡。主条与展开面板共用：`cx` 是目标按钮中心，`bottom` 是气泡要贴住的底边
    /// （主条/面板的顶边 —— 气泡永远画在它上面）。
    fn draw_tip(&mut self, k: BarKind, cx: i32, bottom: i32) {
        let (mut label, mut key) = bar_label(k);
        if matches!(k, BarKind::Tool(Tool::Mosaic)) {
            label = if self.mosaic_brush {
                "马赛克（画笔）"
            } else {
                "马赛克（矩形）"
            };
            key = "再点一次切换形态";
        }
        if label.is_empty() {
            return;
        }
        let text = if key.is_empty() {
            label.to_string()
        } else {
            format!("{label}   {key}")
        };
        let wide: Vec<u16> = text.encode_utf16().collect();
        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        unsafe {
            let f = CreateFontW(-16, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let oldf = SelectObject(self.mem, f);
            let mut rc = RectPos {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            DrawTextW(
                self.mem,
                wide.as_ptr(),
                wide.len() as i32,
                &mut rc,
                DT_CALCRECT | DT_SINGLELINE,
            );
            let tw = (rc.right - rc.left).max(10);
            let th = (rc.bottom - rc.top).max(12);
            let bx = (cx - tw / 2 - 8).max(4).min((self.w - tw - 20).max(4));
            let byy = (bottom - th - 20).max(4);
            /* 气泡也照纸墨体系：深底 + 1px 亮边（和工具栏同一套层次语言） */
            self.pfill_round(bx - 1, byy - 1, bx + tw + 17, byy + th + 11, 6, BAR_EDGE);
            self.pfill_round(bx, byy, bx + tw + 16, byy + th + 10, 5, BAR_BG);
            SetBkMode(self.mem, TRANSPARENT);
            SetTextColor(self.mem, 0xFFFFFF);
            let mut trc = RectPos {
                left: bx + 8,
                top: byy + 5,
                right: bx + tw + 8,
                bottom: byy + th + 5,
            };
            DrawTextW(self.mem, wide.as_ptr(), wide.len() as i32, &mut trc, DT_SINGLELINE);
            SelectObject(self.mem, oldf);
            DeleteObject(f);
        }
    }

    /// 气泡调度：面板内优先（它在最上层），否则看主条
    fn draw_bar_tip(&mut self) {
        if let Some(gi) = self.open_group {
            if let Some(pi) = self.panel_hover {
                if let BarKind::Group(members) = self.bar_item(gi) {
                    if let Some(t) = members.get(pi).copied() {
                        let cx = self.panel[0] + 3 + pi as i32 * 31 + 15;
                        self.draw_tip(BarKind::Tool(t), cx, self.panel[1]);
                    }
                }
                return;
            }
        }
        if let Some(i) = self.hover {
            /* 组面板展开时，主条 tooltip 会画在**完全相同的位置**（两者都贴在按钮上方），
               而且它后画 —— 直接把整个面板盖住。面板在、用户却只看到一行"形状 矩形/椭圆…"，
               这就是 owner 连报几次"展开不了"的真因（2026-11-18 真机截图确认）。
               面板本身就是提示，组内每一项还有自己的 tooltip，这里直接让位。 */
            if self.open_group.is_some() && matches!(self.bar_item(i), BarKind::Group(_)) {
                return;
            }
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

    fn render(&mut self) {
        self.bar_layout();
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
        self.draw_bar();
        /* 面板压在主条上，气泡再压在面板上 —— 顺序反了会被盖住 */
        self.draw_panel();
        self.draw_bar_tip();
        self.draw_keys();
        self.present();
    }

    /// 左下角常驻快捷键面板（网页版 `.snip-keys` 那块）。
    /// 用户反馈"之前右下角那些文字没有了" —— 指的就是这块：它把"能按什么键"写在屏幕上，
    /// 不写的话原生版看起来只剩一排图标，用户不知道有快捷键。
    fn draw_keys(&mut self) {
        let rows: [(&str, &str); 5] = [
            ("Ctrl+A", "选中整屏"),
            ("↑ ↓ ← →", "微调选区（按住 Shift 改大小）"),
            ("Ctrl+C", "复制到剪贴板"),
            ("Ctrl+S", "保存到截图库　Ctrl+Shift+S 另存为…"),
            ("Esc", "取消　　中键 贴图　　Ctrl+Z 撤销"),
        ];
        let face: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        unsafe {
            let f = CreateFontW(-15, 0, 0, 0, FW_NORMAL, 0, 0, 0, 134, 0, 0, 4, 0, face.as_ptr());
            let oldf = SelectObject(self.mem, f);
            SetBkMode(self.mem, TRANSPARENT);
            let texts: Vec<Vec<u16>> = rows
                .iter()
                .map(|(k, d)| format!("{k}　{d}").encode_utf16().collect())
                .collect();
            let (mut tw, mut th) = (0, 0);
            for t in &texts {
                let mut rc = RectPos {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                DrawTextW(
                    self.mem,
                    t.as_ptr(),
                    t.len() as i32,
                    &mut rc,
                    DT_CALCRECT | DT_SINGLELINE,
                );
                tw = tw.max(rc.right);
                th = th.max(rc.bottom);
            }
            let pad = 10;
            let lh = th + 3;
            let bx = 10;
            let by = self.h - 10 - lh * rows.len() as i32 - pad * 2;
            self.pfill(
                bx,
                by,
                bx + tw + pad * 2 + 8,
                by + lh * rows.len() as i32 + pad * 2,
                0x121212,
            );
            SetTextColor(self.mem, rgb(0xEDEDED));
            for (i, t) in texts.iter().enumerate() {
                let mut rc = RectPos {
                    left: bx + pad + 4,
                    top: by + pad + i as i32 * lh,
                    right: bx + pad + 4 + tw,
                    bottom: by + pad + (i as i32 + 1) * lh,
                };
                DrawTextW(self.mem, t.as_ptr(), t.len() as i32, &mut rc, DT_SINGLELINE);
            }
            SelectObject(self.mem, oldf);
            DeleteObject(f);
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
        /* 展开面板先处理：它在主条上方、不属于选区。必须排在 `bar_hit` 和选区判定之前，
           否则点面板会落成"在选区里落笔"，画出一条莫名其妙的标注。 */
        if let Some(pi) = c.panel_hit(x, y) {
            let picked = match c.open_group.map(|gi| c.bar_item(gi)) {
                Some(BarKind::Group(members)) => members.get(pi).copied(),
                _ => None,
            };
            if let Some(t) = picked {
                if t == Tool::Mosaic && c.tool == Tool::Mosaic {
                    c.mosaic_brush = !c.mosaic_brush;
                } else {
                    c.tool = t;
                }
            }
            c.open_group = None;
            c.panel_hover = None;
            c.render();
            return;
        }
        // 工具栏（顺序见 BAR：选择 / 形状 / 手绘 / 打码 / 标记 / 橡皮 | 撤销重做 | 取消·贴图·保存·复制）
        if let Some(i) = c.bar_hit(x, y) {
            match c.bar_item(i) {
                BarKind::Sep => {}
                BarKind::Tool(t) => {
                    /* 马赛克有两个形态：拖矩形 / 当画笔涂。再点一次按钮切换
                       （和网页版"再点切换形状"同一套手势），当前形态写在悬停气泡里。 */
                    if t == Tool::Mosaic && c.tool == Tool::Mosaic {
                        c.mosaic_brush = !c.mosaic_brush;
                    } else {
                        c.tool = t;
                    }
                }
                BarKind::Group(members) => {
                    /* 组按钮：悬浮已经把面板展开了；点这里只切工具，**不要关面板**。
                       0.3.2/0.3.3 在这里写 `open_group = None` 再用一个"防抖"标记等鼠标移动才重开，
                       结果鼠标停在按钮上时看到的就是"没展开"（owner 连报三次）。整套防抖已删。 */
                    let iw = bar_w(BarKind::Group(members));
                    let on_tri = x >= c.bar_item_x(i) + iw - 13 && y >= c.bar[1] + c.bar[3] - 13;
                    if on_tri {
                        c.panel_layout(i);
                        c.open_group = Some(i);
                    } else {
                        let t = c.group_cur(members);
                        if t == Tool::Mosaic && c.tool == Tool::Mosaic {
                            c.mosaic_brush = !c.mosaic_brush;
                        } else {
                            c.tool = t;
                        }
                    }
                    c.panel_hover = None;
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
pub fn run(shot: &screen::Shot) -> Option<Delivered> {
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
                mosaic_brush: false,
                number_next: 1,
                hover: None,
                open_group: None,
                panel: [0, 0, 0, 0],
                panel_hover: None,
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

    #[test]
    #[ignore = "导出工具栏预览图，人工看效果"]
    fn bar_preview() {
        unsafe {
            let (w, h) = (780i32, 170i32);
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

            let mut c = Box::new(Ctx {
                w,
                h,
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
                mosaic_brush: false,
                number_next: 1,
                hover: None,
                open_group: None,
                panel: [0, 0, 0, 0],
                panel_hover: None,
                editor: 0,
                editing: None,
                done: None,
            });

            /* 背景：左半深色 / 右半浅色 —— 这两类截图上都要能看清 */
            let px = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
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

            let mut bw = 6;
            for k in BAR {
                bw += bar_w(k);
            }
            bw += bar_w(BAR_TAIL);

            /* 主条居中，展开「形状」组并让箭头处于选中 —— 一次看全三种态 */
            c.bar = [(w - bw) / 2, 70, bw, BAR_H];
            c.tool = Tool::Arrow;
            c.hover = Some(9);
            c.draw_bar();

            c.panel_layout(1);
            c.open_group = Some(1);
            c.panel_hover = Some(3);
            c.draw_panel();
            c.draw_bar_tip();

            let out = std::env::temp_dir().join("am-bar-preview.bmp");
            write_bmp(&out, w, h, px);

            /* 局部 8 倍放大：「形状」组按钮。1px 的边、7px 的三角在 1:1 下看不清，
               放大后才看得出"描边断没断、三角有没有被图标压住"。 */
            let (zx0, zy0, zw0, zh0, s) = (c.bar[0] + 30, c.bar[1] - 6, 44, BAR_H + 12, 8i32);
            let mut tile = vec![0u8; (zw0 * s * zh0 * s * 4) as usize];
            for y in 0..zh0 * s {
                for x in 0..zw0 * s {
                    let sx = (zx0 + x / s).clamp(0, w - 1);
                    let sy = (zy0 + y / s).clamp(0, h - 1);
                    let so = ((sy * w + sx) * 4) as usize;
                    let dof = ((y * zw0 * s + x) * 4) as usize;
                    tile[dof..dof + 4].copy_from_slice(&px[so..so + 4]);
                }
            }
            write_bmp(&std::env::temp_dir().join("am-bar-zoom.bmp"), zw0 * s, zh0 * s, &tile);
            println!("preview: {}", out.display());

            DeleteDC(mem);
            DeleteObject(bmp);
        }
    }

    /// 悬浮展开的回归测试 —— 这块连出三次 bug，直接用单测钉死，不再靠装机肉眼看。
    #[test]
    fn hover_group_stays_open() {
        unsafe {
            let (w, h) = (780i32, 170i32);
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
            let mut c = Box::new(Ctx {
                w,
                h,
                mem,
                bits: bits as *mut u8,
                base: vec![0u8; (w * h * 4) as usize],
                dim: vec![0u8; (w * h * 4) as usize],
                sel: None,
                sel0: [0; 4],
                ops: Vec::new(),
                redo: Vec::new(),
                tool: Tool::Rect,
                color: 0xF5222D,
                width: 3,
                drag: Drag::None,
                anchor: (0, 0),
                cur: (0, 0),
                pts: Vec::new(),
                bar: [0; 4],
                preview: None,
                mosaic_brush: false,
                number_next: 1,
                hover: None,
                open_group: None,
                panel: [0, 0, 0, 0],
                panel_hover: None,
                editor: 0,
                editing: None,
                done: None,
            });
            let mut bw = 6;
            for k in BAR {
                bw += bar_w(k);
            }
            bw += bar_w(BAR_TAIL);
            c.bar = [(w - bw) / 2, 70, bw, BAR_H];
            let gi = 1; // 「形状」组

            /* ① 悬浮组按钮 → 必须展开 */
            let bx = c.bar_item_x(gi) + 15;
            let by = c.bar[1] + BAR_H / 2;
            assert!(c.update_bar_hover(bx, by));
            assert_eq!(c.open_group, Some(gi), "悬浮组按钮必须展开面板");

            /* ② 穿过按钮与面板之间的 6px 空隙 → 面板不能消失 */
            let gap_y = (c.panel[1] + c.bar[1]) / 2;
            c.update_bar_hover(bx, gap_y);
            assert_eq!(c.open_group, Some(gi), "移向面板的途中面板不能消失");

            /* ③ 进入面板第 4 项 → 高亮生效，面板仍开着 */
            let px = c.panel[0] + 3 + 3 * 31 + 15;
            let py = c.panel[1] + BAR_H / 2;
            c.update_bar_hover(px, py);
            assert_eq!(c.panel_hover, Some(3));
            assert_eq!(c.open_group, Some(gi));

            /* ④ 鼠标彻底移开 → 收起 */
            c.update_bar_hover(4, 4);
            assert_eq!(c.open_group, None);

            DeleteDC(mem);
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
            let mut c = Box::new(Ctx {
                w,
                h,
                mem,
                bits: bits as *mut u8,
                base: vec![0u8; (w * h * 4) as usize],
                dim: vec![0u8; (w * h * 4) as usize],
                sel: None,
                sel0: [0; 4],
                ops: Vec::new(),
                redo: Vec::new(),
                tool: Tool::Select,
                color: 0xFF2244,
                width: 3,
                drag: Drag::None,
                anchor: (0, 0),
                cur: (0, 0),
                pts: Vec::new(),
                bar: [0; 4],
                preview: None,
                mosaic_brush: false,
                number_next: 1,
                hover: None,
                open_group: None,
                panel: [0, 0, 0, 0],
                panel_hover: None,
                editor: 0,
                editing: None,
                done: None,
            });

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
            /* 马赛克两种形态：矩形 + 画笔 */
            c.tool = Tool::Mosaic;
            c.mosaic_brush = false;
            c.draw_op(&c.make_op(a, b, &pts).unwrap());
            c.mosaic_brush = true;
            c.draw_op(&c.make_op(a, b, &pts).unwrap());
            /* 模糊、序号。文字不在这里画：DrawTextW 依赖真实窗口 DC + EDIT 输入流程，
               在这个裸内存 DC 上会崩（离屏预览只验工具栏图标，不验文字）。 */
            c.tool = Tool::Blur;
            c.draw_op(&c.make_op(a, b, &pts).unwrap());
            c.draw_op(&Op::Number(240, 80, 1, 0xFF2244));
            assert_eq!(Op::Text("测试".into(), 240, 180, 0xFFFFFF, 16).bbox()[2] > 0, true);

            /* 像素真的变了（不是空函数） */
            let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let painted = px.chunks_exact(4).filter(|p| p[0] != 0 || p[1] != 0 || p[2] != 0).count();
            assert!(painted > 300, "12 个工具合计只画了 {painted} 个像素");

            /* 箭头必须保留拖动方向：从右下拖到左上，终点就是左上 */
            c.tool = Tool::Arrow;
            match c.make_op((300, 250), (100, 60), &[]) {
                Some(Op::Arrow(p0, p1, _, _)) => {
                    assert_eq!(p0, (300, 250));
                    assert_eq!(p1, (100, 60), "箭头方向不能被 norm4 吃掉");
                }
                _ => panic!("箭头没生成 Op::Arrow"),
            }

            DeleteDC(mem);
            DeleteObject(bmp);
        }
    }
}
