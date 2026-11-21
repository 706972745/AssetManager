//! 枚举本机已安装的字体族（只给灵感板的字体下拉用）。
//!
//! 为什么手写 FFI：`windows` crate 打开 Win32_UI_Shell 之后 rustc 工作集能到 37 GB
//! （2026-11-16 实测，见 `shellthumb.rs` 的注释），这边只用到三个函数，`#[link]` 就够。
//!
//! 只枚举一次并缓存：`EnumFontFamiliesExW` 会回调几百次，字体不会在运行中变。

use std::sync::OnceLock;

/// 本机字体族名：去重、去掉竖排 `@` 变体、按名字排好。
pub fn system_fonts() -> &'static [String] {
    static CACHE: OnceLock<Vec<String>> = OnceLock::new();
    CACHE.get_or_init(|| enumerate().unwrap_or_default())
}

#[cfg(windows)]
fn enumerate() -> Option<Vec<String>> {
    use std::ffi::c_void;

    /// `LOGFONTW`：5 个 LONG + 8 个 BYTE + `WCHAR[LF_FACESIZE]`（LF_FACESIZE = 32）
    #[repr(C)]
    struct LogFontW {
        lf_height: i32,
        lf_width: i32,
        lf_escapement: i32,
        lf_orientation: i32,
        lf_weight: i32,
        lf_italic: u8,
        lf_underline: u8,
        lf_strike_out: u8,
        lf_char_set: u8,
        lf_out_precision: u8,
        lf_clip_precision: u8,
        lf_quality: u8,
        lf_pitch_and_family: u8,
        lf_face_name: [u16; 32],
    }

    type EnumProc = unsafe extern "system" fn(*const LogFontW, *const c_void, u32, isize) -> i32;

    #[link(name = "gdi32")]
    extern "system" {
        fn EnumFontFamiliesExW(
            hdc: isize,
            lf: *const LogFontW,
            proc: EnumProc,
            lparam: isize,
            flags: u32,
        ) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        /* 句柄用 `isize`，和 `snip_native.rs` / `shellthumb.rs` 的声明**逐字一致** ——
           同一个符号在同一个 crate 里出现两种签名会报
           "redeclared with a different signature"，链接期没法保证调用约定。 */
        fn GetDC(hwnd: isize) -> isize;
        fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
    }

    unsafe extern "system" fn on_font(
        lf: *const LogFontW,
        _tm: *const c_void,
        _kind: u32,
        lparam: isize,
    ) -> i32 {
        let out = &mut *(lparam as *mut Vec<String>);
        let raw = &(*lf).lf_face_name;
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        let name = String::from_utf16_lossy(&raw[..end]).trim().to_string();
        /* `@` 开头的是同一套字体的竖排变体，下拉里列出来只会让人选错 */
        if !name.is_empty() && !name.starts_with('@') {
            out.push(name);
        }
        1 // 非 0 = 继续枚举
    }

    unsafe {
        let hdc = GetDC(0);
        if hdc == 0 {
            return None;
        }
        let mut lf: LogFontW = std::mem::zeroed();
        /* DEFAULT_CHARSET(1)：中英日韩的字体都能枚举到（ANSI_CHARSET 会漏掉一批中文族） */
        lf.lf_char_set = 1;
        let mut out: Vec<String> = Vec::new();
        EnumFontFamiliesExW(hdc, &lf, on_font, &mut out as *mut _ as isize, 0);
        ReleaseDC(0, hdc);
        /* 同一个族会因为粗体/斜体等字形被回调多次 → 去重 */
        out.sort();
        out.dedup();
        Some(out)
    }
}

#[cfg(not(windows))]
fn enumerate() -> Option<Vec<String>> {
    None
}
