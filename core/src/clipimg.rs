//! 剪贴板里的图片（F3 贴图、Alt+F1 截图后复制）。
//!
//! 走 `arboard`：Windows 的 `CF_DIB`/`PNG` 格式细节太多（alpha 预乘、行序、BI_BITFIELDS…），
//! 手写容易在"某些软件复制的图"上翻车，这一类不值得自己造。
//!
//! 注意：`arboard` 在 Windows 上会**短暂占用剪贴板**，`Clipboard::new()` 可能因为
//! 别的程序正开着剪贴板而失败 —— 这不是错误，是常态，调用方要把话说人话（"剪贴板正被占用"）。

use std::borrow::Cow;
use std::io::Cursor;

use arboard::Clipboard;

/// 读剪贴板图片 → `(宽, 高, PNG 字节)`。
pub fn get_png() -> Result<(u32, u32, Vec<u8>), String> {
    let mut cb = Clipboard::new().map_err(|_| "剪贴板正被其他程序占用，请稍后再试".to_string())?;
    let img = match cb.get_image() {
        Ok(i) => i,
        Err(arboard::Error::ContentNotAvailable) => {
            return Err("剪贴板里没有图片".to_string());
        }
        Err(arboard::Error::ClipboardOccupied) => {
            return Err("剪贴板正被其他程序占用，请稍后再试".to_string());
        }
        Err(e) => return Err(format!("读取剪贴板图片失败：{e}")),
    };
    let (w, h) = (img.width as u32, img.height as u32);
    if w == 0 || h == 0 {
        return Err("剪贴板里的图片尺寸是 0".to_string());
    }
    let rgba = image::RgbaImage::from_raw(w, h, img.bytes.into_owned())
        .ok_or_else(|| "剪贴板图片的像素数据不完整".to_string())?;
    let mut buf = Cursor::new(Vec::new());
    rgba.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("编码剪贴板图片失败：{e}"))?;
    Ok((w, h, buf.into_inner()))
}

/// PNG 字节 → 剪贴板（截图后的 Alt+F1、F1 都走这里）
pub fn set_png(png: &[u8]) -> Result<(), String> {
    let img = image::load_from_memory(png).map_err(|e| format!("解码要复制的图片失败：{e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let mut cb = Clipboard::new().map_err(|_| "剪贴板正被其他程序占用，请稍后再试".to_string())?;
    cb.set_image(arboard::ImageData {
        width: w,
        height: h,
        bytes: Cow::Owned(rgba.into_raw()),
    })
    .map_err(|e| format!("写入剪贴板失败：{e}"))
}

/// 纯文本进剪贴板（"复制当前显示尺寸"这类）
pub fn set_text(text: &str) -> Result<(), String> {
    let mut cb = Clipboard::new().map_err(|_| "剪贴板正被其他程序占用，请稍后再试".to_string())?;
    cb.set_text(text.to_string())
        .map_err(|e| format!("写入剪贴板失败：{e}"))
}
