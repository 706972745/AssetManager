//! 图片「另存为」与「调整尺寸」(P2.5 T38)。
//!
//! 两件事分开做，因为它们的语义不一样：
//! - **另存为** = 原样复制字节。动图（GIF/APNG）、RAW、PSB 这些一旦重新编码就废了，
//!   而且用户要的就是"原图那份"，不是我们解码重压的那份。
//! - **调整尺寸** = 真解码 → 缩放 → 按目标扩展名重新编码。只对 `image` 能解的格式开放
//!   （pdf / psd / raw 之类在界面上就不给这一项）。

use crate::db::Res;
use std::path::Path;

/// 原样复制（另存为）。返回字节数。
pub fn save_copy(src: &Path, dst: &Path) -> Res<u64> {
    if !src.is_file() {
        return Err(format!("文件不在了：{}", src.display()));
    }
    if same_path(src, dst) {
        return Err("存到同一个位置了，换个名字或换个文件夹".into());
    }
    if let Some(dir) = dst.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("建不了目录：{e}"))?;
        }
    }
    std::fs::copy(src, dst).map_err(|e| format!("写不进去：{e}"))
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.canonicalize()
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// 缩放并另存。`max_edge` = 长边上限（0 = 不缩放，只转格式）。
///
/// 返回 `(宽, 高, 字节数)`。
pub fn resized_to(src: &Path, dst: &Path, max_edge: u32) -> Res<(u32, u32, u64)> {
    if !src.is_file() {
        return Err(format!("文件不在了：{}", src.display()));
    }
    if same_path(src, dst) {
        return Err("存到同一个位置了，换个名字或换个文件夹".into());
    }
    let img = image::open(src).map_err(|e| format!("这张图解不开（{}）：{e}", ext_of(src)))?;
    let img = img.to_rgba8();
    let (ow, oh) = (img.width().max(1), img.height().max(1));
    let longest = ow.max(oh);
    /* 只缩不放：用户点"调到 512"不是要一张糊掉的放大图 */
    let (nw, nh) = if max_edge > 0 && longest > max_edge {
        let k = max_edge as f32 / longest as f32;
        (
            ((ow as f32 * k).round() as u32).max(1),
            ((oh as f32 * k).round() as u32).max(1),
        )
    } else {
        (ow, oh)
    };
    let out = if (nw, nh) == (ow, oh) {
        img
    } else {
        /* Lanczos3：这是"另存一份给别人"的图，清晰度优先，不在乎多花 100ms */
        image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3)
    };
    if let Some(dir) = dst.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("建不了目录：{e}"))?;
        }
    }
    let ext = dst
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bytes: Vec<u8> = match ext.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => {
            let rgb = image::DynamicImage::ImageRgba8(out).to_rgb8();
            let mut buf: Vec<u8> = Vec::new();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
            enc.encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
                .map_err(|e| format!("编码 JPEG 失败：{e}"))?;
            buf
        }
        /* webp：单边上限 16383（libwebp 的硬限制）。原尺寸导出（max_edge = 0）
           碰到超长图时以前会 panic —— `webp` crate 的 `encode()` 里是 unwrap。
           现在走统一出口，报错让人换个格式，而不是把软件带走。 */
        "webp" => {
            let rgb = image::DynamicImage::ImageRgba8(out).to_rgb8();
            crate::thumb::webp_encode(rgb.as_raw(), rgb.width(), rgb.height(), 88.0)?
        }
        /* png 与其他：PNG 最安全（带 alpha、无损） */
        _ => {
            let mut buf: Vec<u8> = Vec::new();
            image::DynamicImage::ImageRgba8(out)
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .map_err(|e| format!("编码 PNG 失败：{e}"))?;
            buf
        }
    };
    std::fs::write(dst, &bytes).map_err(|e| format!("写不进去：{e}"))?;
    Ok((nw, nh, bytes.len() as u64))
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 这个格式能不能"调整尺寸"（另存为是复制字节，任何格式都行）
pub fn resizable(src: &Path) -> bool {
    matches!(
        ext_of(src).as_str(),
        "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "webp" | "bmp" | "gif" | "tif" | "tiff" | "avif"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("am-imgops-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    fn make_png(path: &Path, w: u32, h: u32) {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255])
        });
        image::DynamicImage::ImageRgba8(img)
            .save(path)
            .unwrap();
    }

    #[test]
    fn resize_keeps_aspect_and_both_directions() {
        let src = tmp("src.png");
        make_png(&src, 400, 200);
        let out = tmp("out.png");
        let (w, h, n) = resized_to(&src, &out, 100).unwrap();
        assert_eq!((w, h), (100, 50), "长边压到 100，比例不能变");
        assert!(n > 0 && out.is_file());
        /* 目标上限比原图大 → 保持原尺寸，不放大 */
        let big = tmp("big.png");
        let (w2, h2, _) = resized_to(&src, &big, 4000).unwrap();
        assert_eq!((w2, h2), (400, 200));
    }

    #[test]
    fn save_copy_is_byte_identical() {
        let src = tmp("copy-src.png");
        make_png(&src, 8, 8);
        let out = tmp("copy-dst.png");
        let n = save_copy(&src, &out).unwrap();
        assert_eq!(n, std::fs::metadata(&src).unwrap().len());
        assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&out).unwrap());
    }

    #[test]
    fn same_path_is_refused() {
        let src = tmp("same.png");
        make_png(&src, 4, 4);
        assert!(save_copy(&src, &src).is_err());
        assert!(resized_to(&src, &src, 10).is_err());
    }
}
