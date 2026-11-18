//! 屏幕捕获（F1 截图 / Alt+F1 截图后复制）。
//!
//! 只用 GDI，不引第三方截屏库：`BitBlt` 一次把整块虚拟桌面拷到 DIB，再转 `RgbaImage`。
//! 和 `shellthumb.rs` 手写 COM、`fsops::copy_files_to_clipboard` 手写剪贴板是同一个路子 ——
//! 这类"调一次系统 API"的活，为它拉一个 crate 不值当。
//!
//! ## 两条要命的细节
//!
//! 1. **必须带 `CAPTUREBLT`**：不带的话分层窗口（很多播放器、部分悬浮窗）会被截成黑的。
//! 2. **`biHeight` 给负数**得到自上而下的 DIB，否则整张图会上下颠倒 —— 而我们后面要靠
//!    `y` 坐标去裁选区，颠倒了裁出来就是另一块地方（不会报错，只是截错）。


/// 一块屏幕截图（虚拟桌面坐标系，物理像素）
pub struct Shot {
    /// 虚拟桌面左上角的屏幕坐标（多屏时可能是负数）
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// RGBA，行优先，长度 = w*h*4
    pub rgba: Vec<u8>,
}

impl Shot {
    /// 按**物理像素**裁一块（越界会被夹到图内，裁空了返回 None）
    pub fn crop(&self, x: i32, y: i32, w: i32, h: i32) -> Option<image::RgbaImage> {
        if w <= 0 || h <= 0 {
            return None;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w as i32);
        let y1 = (y + h).min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let mut out = Vec::with_capacity((cw * ch * 4) as usize);
        for row in 0..ch {
            let start = (((y0 as u32 + row) * self.w + x0 as u32) * 4) as usize;
            out.extend_from_slice(&self.rgba[start..start + (cw * 4) as usize]);
        }
        image::RgbaImage::from_raw(cw, ch, out)
    }

    /// 整张图编码成 PNG
    pub fn png(&self) -> Result<Vec<u8>, String> {
        /* 直接拿 raw 编码：以前这里 `rgba.clone()` 会复制一份 16MB 的整屏缓冲区
           （2560×1600×4），白花 10~20ms —— 而这正在"按下 F1 到能框选"的路径上。 */
        png_of_raw(&self.rgba, self.w, self.h)
    }

    /// 整张图的平均色。
    ///
    /// 用途：截图覆盖层的窗口在「第一帧合成之前」只能显示 WebView 的背景色
    /// （深色主题下就是一片近黑 —— owner 说的"截图开始黑一下"）。
    /// 把覆盖层根节点的底色先铺成这张截图的平均色，那 200~400ms 就变成
    /// "屏幕整体压暗了一点点"，而不是"闪一下黑"。
    ///
    /// 抽样步长按「最多看 4096 个点」算：2560×1600 也只遍历 4096 次，几毫秒。
    pub fn avg_rgb(&self) -> [u8; 3] {
        let px = self.w as usize * self.h as usize;
        if px == 0 || self.rgba.len() < 3 {
            return [18, 18, 18];
        }
        let step = (px / 4096).max(1);
        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        let mut i = 0usize;
        while i < px {
            let o = i * 4;
            if o + 2 < self.rgba.len() {
                r += self.rgba[o] as u64;
                g += self.rgba[o + 1] as u64;
                b += self.rgba[o + 2] as u64;
                n += 1;
            }
            i += step;
        }
        if n == 0 {
            return [18, 18, 18];
        }
        [(r / n) as u8, (g / n) as u8, (b / n) as u8]
    }

    /// **秒出用的低清底图**（缩到 `max_w` 宽，PNG 字节）。
    ///
    /// 为什么需要它：WebView2 在**隐藏窗口**里不会提前合成，`<img>` 的 `onLoad` 只代表
    /// "解码完了"，第一帧仍要等 `show()` 之后才提交；而 2560×1600 的 PNG 光是上传纹理
    /// 就要 300–400ms，用户看到的就是窗口背景色（深色 = "截图开始黑一下"）。
    /// 缩到 480 宽之后只有几十 KB，几十毫秒就能上屏，构图和颜色已经对上，
    /// 高清图后到再盖上去 —— 人眼基本看不到中间那一步。
    pub fn thumb_png(&self, max_w: u32) -> Result<Vec<u8>, String> {
        let img = image::RgbaImage::from_raw(self.w, self.h, self.rgba.clone())
            .ok_or_else(|| "截图缓冲区尺寸与像素数对不上".to_string())?;
        let w = max_w.min(self.w).max(1);
        let h = ((self.h as f64) * (w as f64) / (self.w as f64)).round().max(1.0) as u32;
        let small = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        png_of(&small)
    }
}

/// 任意 RGBA 图 → PNG 字节
pub fn png_of(img: &image::RgbaImage) -> Result<Vec<u8>, String> {
    png_of_raw(img.as_raw(), img.width(), img.height())
}

/// raw RGBA → PNG 字节（不复制像素）。
///
/// 用 `CompressionType::Fast` + `FilterType::Sub`：截图这一步是**交互关键路径**，
/// 全屏 PNG 用默认压缩要 ~180ms，Fast 只要 ~60ms；体积从 1.6MB 涨到 ~2.2MB，
/// 前端解码多花二十来毫秒 —— 净赚一百多毫秒，用户体感就是"F1 之后马上能框"。
pub fn png_of_raw(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let mut buf = Vec::new();
    PngEncoder::new_with_quality(&mut buf, CompressionType::Fast, FilterType::Sub)
        .write_image(rgba, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("编码 PNG 失败：{e}"))?;
    Ok(buf)
}

/// 把虚拟桌面整块抓下来
pub fn capture_virtual() -> Result<Shot, String> {
    imp::capture_virtual()
}

#[cfg(windows)]
mod imp {
    use super::Shot;
    use std::ffi::c_void;

    // 和 `shellthumb.rs` 里的声明逐字一致（都用 isize）：
    // 同一个符号被声明成两种类型 rustc 会警告，而 warning 攒多了会盖住真问题。
    type HWND = isize;
    type HDC = isize;
    type HBITMAP = isize;
    type HGDIOBJ = isize;

    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        xppm: i32,
        yppm: i32,
        clr_used: u32,
        clr_important: u32,
    }

    #[repr(C)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 3],
    }

    /// `BITMAPINFO` 在 32 位 DIB 下只要 header + 3 个占位色
    const BI_RGB: u32 = 0;
    const SRCCOPY: u32 = 0x00CC_0020;
    /// 把分层窗口也一起抓进来（不加这项，播放器/悬浮窗会是一片黑）
    const CAPTUREBLT: u32 = 0x4000_0000;

    const SM_XVIRTUALSCREEN: i32 = 76;
    const SM_YVIRTUALSCREEN: i32 = 77;
    const SM_CXVIRTUALSCREEN: i32 = 78;
    const SM_CYVIRTUALSCREEN: i32 = 79;

    #[link(name = "user32")]
    extern "system" {
        fn GetDC(hwnd: HWND) -> HDC;
        fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
        fn GetSystemMetrics(index: i32) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleDC(hdc: HDC) -> HDC;
        fn CreateCompatibleBitmap(hdc: HDC, w: i32, h: i32) -> HBITMAP;
        fn SelectObject(hdc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
        fn BitBlt(dst: HDC, x: i32, y: i32, w: i32, h: i32, src: HDC, sx: i32, sy: i32, rop: u32) -> i32;
        fn GetDIBits(
            hdc: HDC,
            bmp: HBITMAP,
            start: u32,
            lines: u32,
            bits: *mut c_void,
            bi: *mut BitmapInfo,
            usage: u32,
        ) -> i32;
        fn DeleteObject(obj: HGDIOBJ) -> i32;
        fn DeleteDC(hdc: HDC) -> i32;
    }

    pub fn capture_virtual() -> Result<Shot, String> {
        let (x, y, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        if w <= 0 || h <= 0 {
            return Err("拿不到屏幕尺寸".into());
        }
        unsafe {
            let screen = GetDC(0);
            if screen == 0 {
                return Err("拿不到屏幕 DC".into());
            }
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            if mem == 0 || bmp == 0 {
                if bmp != 0 {
                    DeleteObject(bmp);
                }
                if mem != 0 {
                    DeleteDC(mem);
                }
                ReleaseDC(0, screen);
                return Err("创建截图缓冲区失败（内存不足？）".into());
            }
            let old = SelectObject(mem, bmp);
            let ok = BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
            if ok == 0 {
                SelectObject(mem, old);
                DeleteObject(bmp);
                DeleteDC(mem);
                ReleaseDC(0, screen);
                return Err("BitBlt 截屏失败（可能被安全软件拦住）".into());
            }

            let mut bi = BitmapInfo {
                header: BitmapInfoHeader {
                    size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    width: w,
                    // 负数 = 自上而下的 DIB，见模块注释第 2 条
                    height: -h,
                    planes: 1,
                    bit_count: 32,
                    compression: BI_RGB,
                    size_image: 0,
                    xppm: 0,
                    yppm: 0,
                    clr_used: 0,
                    clr_important: 0,
                },
                colors: [0; 3],
            };
            let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
            let lines = GetDIBits(
                mem,
                bmp,
                0,
                h as u32,
                buf.as_mut_ptr() as *mut c_void,
                &mut bi,
                0,
            );
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(0, screen);
            if lines == 0 {
                return Err("读取截图像素失败".into());
            }
            // BGRA → RGBA。GDI 给的 alpha 恒为 0，这里统一补成不透明，
            // 否则编码出来的 PNG 是"全透明"的（看图软件里一片空白）。
            for px in buf.chunks_exact_mut(4) {
                px.swap(0, 2);
                px[3] = 255;
            }
            Ok(Shot { x, y, w: w as u32, h: h as u32, rgba: buf })
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Shot;
    pub fn capture_virtual() -> Result<Shot, String> {
        Err("截图只支持 Windows".into())
    }
}


/// 本地时间戳：`("2026-11-17 09-30-12", 毫秒)`。
///
/// 用 `GetLocalTime` 而不是自己从 epoch 算：北京时间是 +8 且没有夏令时，
/// 但"算时区"这件事一旦要跨机器就必然出错，直接问系统最省事（也省掉 chrono）。
pub fn local_stamp() -> (String, i64) {
    stamp::local_stamp()
}

#[cfg(windows)]
mod stamp {
    #[repr(C)]
    #[derive(Default)]
    pub struct SystemTime {
        pub year: u16,
        pub month: u16,
        pub day_of_week: u16,
        pub day: u16,
        pub hour: u16,
        pub minute: u16,
        pub second: u16,
        pub milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(t: *mut SystemTime);
    }

    pub fn local_stamp() -> (String, i64) {
        unsafe {
            let mut t = SystemTime::default();
            GetLocalTime(&mut t);
            let s = format!(
                "{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
                t.year, t.month, t.day, t.hour, t.minute, t.second
            );
            let ms = (t.milliseconds as i64)
                + (t.second as i64) * 1000
                + (t.minute as i64) * 60_000
                + (t.hour as i64) * 3_600_000;
            (s, ms)
        }
    }
}

#[cfg(not(windows))]
mod stamp {
    pub fn local_stamp() -> (String, i64) {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        (format!("{ms}"), ms)
    }
}

/// 只读文件头拿宽高（不解码像素）
pub fn dims(p: &std::path::Path) -> (u32, u32) {
    image::image_dimensions(p).unwrap_or((0, 0))
}
