//! 解码链的兜底层（T10 L3）：借 Windows 自己的缩略图提供者。
//!
//! 为什么要这层：HEIC / PDF / AI / 视频 / Sketch 这些格式，`image` crate 根本解不了，
//! 但它们**已经在资源管理器里长出缩略图了** —— 系统装了对应的解码器（HEIF 扩展、
//! Adobe 的 PDF/AI 提供者、Media Foundation 的视频抽帧），走 `IShellItemImageFactory`
//! 就能直接拿到一张位图。等于白捡几十种格式，不用各自接一遍库。
//!
//! 为什么手写 FFI 而不是 `windows` crate（2026-11-16 踩过）：
//! 那个 crate 打开 `Win32_UI_Shell` 之后，rustc 编到一半工作集跑到 **37 GB**、
//! 六分钟推进不了 1% CPU，等于把构建搞死。这里只要三个系统调用 + 一个 COM 接口的
//! vtable 第 4 个槽，手写 90 行就够 —— 和 `fsops::copy_files_to_clipboard`
//! 走同一条路（都是直接 `#[link]` Win32，不引第三方绑定）。
//!
//! 三条纪律：
//! 1. **不同步阻塞太久**：`GetImage` 对慢格式（PDF 首页）可能几百毫秒，
//!    调用方在解码闸门里跑，最多 4 路并发；
//! 2. 拿到的质量**不可控**（可能是个小图标被放大），所以标 `Quality::Low`，
//!    详情面板如实写「系统预览」；
//! 3. 只在**自己解码失败之后**才走这里 —— 自己解出来的比系统预览清晰。

use std::path::Path;

/// 系统缩略图的目标边长。`GetImage` 给「不超过这个边长」的位图；
/// 512 够铺满 512 档网格，再大系统也可能只有 256 的缓存。
const SHELL_PX: u32 = 512;

/// 取系统缩略图 → (RGBA 像素, 宽, 高)。
///
/// 返回 `None` 表示系统也没有这张图的预览（格式没人注册、文件损坏，
/// 或者提供者只认「图标」模式而我们要求的是真缩略图）。
#[cfg(windows)]
pub fn shell_thumb(path: &Path, px: u32) -> Option<(Vec<u8>, u32, u32)> {
    win::grab(path, if px == 0 { SHELL_PX } else { px })
}

#[cfg(not(windows))]
pub fn shell_thumb(_path: &Path, _px: u32) -> Option<(Vec<u8>, u32, u32)> {
    None // macOS / Linux 走各自的 QuickLook / GIO 缩略图，M1 之后再说
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[repr(C)]
    struct Size {
        cx: i32,
        cy: i32,
    }

    #[repr(C)]
    struct BitmapInfoHeader {
        bi_size: u32,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: u32,
        bi_size_image: u32,
        bi_x_pels: i32,
        bi_y_pels: i32,
        bi_clr_used: u32,
        bi_clr_important: u32,
    }

    #[repr(C)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        /// 调色板跟着结构体走：32bpp BI_RGB 用不上，但要占够位置（系统会按 biClrUsed 读）
        colors: [u32; 3],
    }

    impl BitmapInfo {
        fn new() -> Self {
            BitmapInfo {
                header: BitmapInfoHeader {
                    bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    bi_width: 0,
                    bi_height: 0,
                    bi_planes: 1,
                    bi_bit_count: 32,
                    bi_compression: 0, // BI_RGB
                    bi_size_image: 0,
                    bi_x_pels: 0,
                    bi_y_pels: 0,
                    bi_clr_used: 0,
                    bi_clr_important: 0,
                },
                colors: [0; 3],
            }
        }
    }

    const SIIGBF_RESIZETOFIT: i32 = 0x00;
    const SIIGBF_BIGGERSIZEOK: i32 = 0x01;
    const SIIGBF_THUMBNAILONLY: i32 = 0x08;
    const DIB_RGB_COLORS: u32 = 0;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
    /// RPC_E_CHANGED_MODE：本线程已经被别人用别的套间初始化过
    const RPC_E_CHANGED_MODE: i32 = -2147417850;

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv: *mut c_void, coinit: u32) -> i32;
        fn CoUninitialize();
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn GetDC(hwnd: isize) -> isize;
        fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
        fn GetObjectW(obj: isize, size: i32, out: *mut c_void) -> i32;
        fn GetDIBits(
            hdc: isize,
            hbm: isize,
            start: u32,
            lines: u32,
            bits: *mut c_void,
            bi: *mut BitmapInfo,
            usage: u32,
        ) -> i32;
        fn DeleteObject(obj: isize) -> i32;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHCreateItemFromParsingName(
            path: *const u16,
            bind_ctx: *mut c_void,
            riid: *const Guid,
            out: *mut *mut c_void,
        ) -> i32;
    }

    #[repr(C)]
    struct Bitmap {
        bm_type: i32,
        bm_width: i32,
        bm_height: i32,
        bm_width_bytes: i32,
        bm_planes: u16,
        bm_bits_pixel: u16,
        bm_bits: *mut c_void,
    }

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    /// {bcc18b79-ba16-442f-80c4-8a59c30c463b}
    const IID_ISHELLITEMIMAGEFACTORY: Guid = Guid {
        data1: 0xbcc1_8b79,
        data2: 0xba16,
        data3: 0x442f,
        data4: [0x80, 0xc4, 0x8a, 0x59, 0xc3, 0x0c, 0x46, 0x3b],
    };

    /// 每线程只初始化一次 COM。重复 `CoInitializeEx` 在同一线程必须配对 `CoUninitialize`；
    /// 缩略图协议是多线程反复调进来的，乱配对会互相拆台。
    struct Com(bool);

    impl Com {
        fn ensure() -> Self {
            unsafe {
                let hr =
                    CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
                if hr == RPC_E_CHANGED_MODE {
                    // 别的套间已经初始化过：照样能用 IShellItemImageFactory，只是不用我们收尾
                    return Com(false);
                }
                Com(hr >= 0)
            }
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    pub fn grab(path: &Path, px: u32) -> Option<(Vec<u8>, u32, u32)> {
        let _com = Com::ensure();

        // 绝对路径：相对路径会被解析成「当前目录下的名字」，而当前目录在别的线程里已经变了
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let s = abs.to_string_lossy();
        // \\?\ 前缀 Shell 不认
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        let mut wide: Vec<u16> = std::ffi::OsStr::new(s).encode_wide().collect();
        wide.push(0);

        let mut raw: *mut c_void = std::ptr::null_mut();
        unsafe {
            let hr = SHCreateItemFromParsingName(
                wide.as_ptr(),
                std::ptr::null_mut(),
                &IID_ISHELLITEMIMAGEFACTORY,
                &mut raw,
            );
            if hr < 0 || raw.is_null() {
                eprintln!("[shell] SHCreateItemFromParsingName hr=0x{:08x} raw={:?}", hr, raw);
                return None;
            }
        }
        let item = ComPtr(raw);

        let size = Size { cx: px as i32, cy: px as i32 };
        // THUMBNAILONLY：没有真缩略图就失败，**不要**退回「一坨文件图标」——
        // 文件图标对我们毫无意义（界面上本来就画了类型徽标）
        let flags = SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK | SIIGBF_RESIZETOFIT;

        let hbm = unsafe { item.get_image(size, flags) }?;
        let out = unsafe { hbitmap_to_rgba(hbm) };
        unsafe {
            DeleteObject(hbm);
        }
        out
    }

    /// 极简 COM 持有者：只管 Release 那一半（拿到时引用计数已经是 +1）。
    struct ComPtr(*mut c_void);

    impl ComPtr {
        /// IShellItemImageFactory 的 vtable：
        ///   0 QueryInterface / 1 AddRef / 2 Release / 3 GetImage
        unsafe fn get_image(&self, size: Size, flags: i32) -> Option<isize> {
            let vt = *(self.0 as *const *const usize);
            let get_image: extern "system" fn(*mut c_void, Size, i32, *mut isize) -> i32 =
                std::mem::transmute(*vt.add(3));
            let mut hbm: isize = 0;
            let hr = get_image(self.0, size, flags, &mut hbm);
            if hr < 0 || hbm == 0 {
                eprintln!("[shell] GetImage hr=0x{:08x} hbm={}", hr, hbm);
                None
            } else {
                Some(hbm)
            }
        }
    }

    impl Drop for ComPtr {
        fn drop(&mut self) {
            unsafe {
                let vt = *(self.0 as *const *const usize);
                let release: extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vt.add(2));
                release(self.0);
            }
        }
    }

    /// HBITMAP → 紧密排列的 RGBA。统一用 32bpp 顶朝下的 DIB 再读，
    /// 不管原图是 24 位还是带 alpha 的 32 位，出来都是同一套布局。
    unsafe fn hbitmap_to_rgba(hbm: isize) -> Option<(Vec<u8>, u32, u32)> {
        // 先用 GetObjectW 拿尺寸。为什么不靠 GetDIBits 的「探测调用」（cLines=0 + lpvBits=NULL）：
        // 从 GetImage 拿回来的常常是 DIB section，探测调用会直接返回 0，
        // 于是"已经拿到句柄却读不出像素"（2026-11-16 实测踩到）。
        let mut bm: Bitmap = std::mem::zeroed();
        let got_obj = GetObjectW(
            hbm,
            std::mem::size_of::<Bitmap>() as i32,
            &mut bm as *mut Bitmap as *mut c_void,
        );
        if got_obj == 0 || bm.bm_width <= 0 || bm.bm_height == 0 {
            eprintln!("[shell] GetObjectW 失败 got={} w={} h={}", got_obj, bm.bm_width, bm.bm_height);
            return None;
        }
        // bmHeight 可能是负的（顶朝下的 DIB）
        let w = bm.bm_width as u32;
        let h = bm.bm_height.unsigned_abs();
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return None;
        }

        // GetDC(0) = 屏幕 DC，只用来做格式转换，读完立刻还
        let hdc = GetDC(0);
        if hdc == 0 {
            return None;
        }
        let mut bi = BitmapInfo::new();
        bi.header.bi_width = w as i32;
        // 负高度 = 顶朝下，省一次整图翻转
        bi.header.bi_height = -(h as i32);
        bi.header.bi_planes = 1;
        bi.header.bi_bit_count = 32;
        bi.header.bi_compression = 0;

        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let got = GetDIBits(
            hdc,
            hbm,
            0,
            h,
            buf.as_mut_ptr() as *mut c_void,
            &mut bi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(0, hdc);
        if got == 0 {
            eprintln!("[shell] GetDIBits 失败");
            return None;
        }

        // DIB 是 BGRA；alpha 常常全是 0（系统缩略图常见）—— 那时按不透明处理，
        // 否则整张图会变全透明（踩过一次，缩略图区域一片空白）
        let all_zero_alpha = buf.chunks_exact(4).all(|p| p[3] == 0);
        for pixel in buf.chunks_exact_mut(4) {
            let (b, g, r) = (pixel[0], pixel[1], pixel[2]);
            let a = if all_zero_alpha { 255 } else { pixel[3] };
            if a == 255 {
                pixel[0] = r;
                pixel[1] = g;
                pixel[2] = b;
            } else {
                // 半透明边缘铺到白底上：素材库背景是浅色的，留黑边很难看
                let f = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
                pixel[0] = f(r);
                pixel[1] = f(g);
                pixel[2] = f(b);
            }
            pixel[3] = 255;
        }
        Some((buf, w, h))
    }
}
