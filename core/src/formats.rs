//! 格式表：扩展名 → kind，文件头嗅探，徽标。
//!
//! 铁律（MVP 方案 §4.1）：**能识别和能出图是两件事**。
//! 这里只负责"认得出"，认得出就入库；能不能出图由 thumb 管线决定。

use crate::model::*;

pub const IMAGE_EXTS: &[&str] = &[
    // 常规
    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "gif", "webp", "bmp", "ico", "cur",
    // 高位深 / 3D 贴图
    "tif", "tiff", "tga", "dds", "exr", "hdr", "ktx", "ktx2",
    // 无损 / 小众 / 科研
    "pnm", "pbm", "pgm", "ppm", "qoi", "ff", "jp2", "j2k",
    // 新格式（avifs = AVIF 序列；heic/heif 是同族容器）
    "avif", "avifs", "jxl", "heic", "heif", "jxr", "wdp", "hdp",
    // 相机 RAW（TIFF 系 + Canon CR3 + 富士 RAF）
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "rwl",
    "srw", "pef", "raw", "3fr", "fff", "iiq", "mos", "mrw", "mef", "erf", "kdc", "dcr", "x3f",
    // Pentax 的 PEF 有两种容器写法，厂商工具会存成 ptx；Leaf 的 MOS 常有 mfw 伴生文件
    "ptx", "mfw",
    // 工程稿（先入库，插件再出图）
    "psd", "psb", "ai", "eps", "svg", "sketch", "clip", "xcf", "wmf", "emf", "cdr",
];

pub const VIDEO_EXTS: &[&str] = &["mp4", "mov", "m4v", "mkv", "avi", "webm", "wmv", "flv", "mpg", "mpeg"];
pub const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "aac", "ogg", "m4a"];
pub const MODEL_EXTS: &[&str] = &["blend", "glb", "gltf", "fbx", "obj", "stl", "ply", "usd", "usda", "usdc", "abc", "3ds", "max", "c4d"];
pub const DOC_EXTS: &[&str] = &["pdf", "txt", "md", "json", "csv", "doc", "docx", "xls", "xlsx", "ppt", "pptx"];

pub fn lower_ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

pub fn kind_of(ext: &str) -> i32 {
    let e = ext;
    if IMAGE_EXTS.contains(&e) { return KIND_IMAGE; }
    if VIDEO_EXTS.contains(&e) { return KIND_VIDEO; }
    if AUDIO_EXTS.contains(&e) { return KIND_AUDIO; }
    if MODEL_EXTS.contains(&e) { return KIND_MODEL; }
    if DOC_EXTS.contains(&e) { return KIND_DOC; }
    KIND_OTHER
}

pub fn badge(ext: &str) -> String {
    if ext.is_empty() { return "FILE".into(); }
    let mut s = ext.to_ascii_uppercase();
    if s.len() > 4 { s.truncate(4); }
    if s == "JPEG" { s = "JPG".into(); }
    s
}

/// 文件头嗅探：纠正"扩展名撒谎"的情况（下载工具改名很常见）。
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    let h = head;
    if h.len() < 4 { return None; }
    if h.starts_with(&[0xFF, 0xD8, 0xFF]) { return Some("jpg"); }
    if h.starts_with(&[0x89, b'P', b'N', b'G']) { return Some("png"); }
    if h.starts_with(b"GIF8") { return Some("gif"); }
    if h.len() >= 12 && &h[0..4] == b"RIFF" && &h[8..12] == b"WEBP" { return Some("webp"); }
    if h.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || h.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) { return Some("tiff"); }
    if h.starts_with(b"DDS ") { return Some("dds"); }
    if h.starts_with(&[0x76, 0x2F, 0x31, 0x01]) { return Some("exr"); }
    if h.starts_with(b"8BPS") { return Some("psd"); }
    if h.starts_with(b"%PDF") { return Some("pdf"); }
    if h.starts_with(b"qoif") { return Some("qoi"); }
    if h.starts_with(b"BM") { return Some("bmp"); }
    if h.starts_with(b"BLENDER") { return Some("blend"); }
    if h.starts_with(b"glTF") { return Some("glb"); }
    if h.starts_with(b"farbfeld") { return Some("ff"); }
    if h.starts_with(&[0x00, 0x00, 0x01, 0x00]) { return Some("ico"); }
    if h.len() >= 12 && &h[4..8] == b"ftyp" {
        // ISO-BMFF 的 brand 决定它其实是 HEIC 还是 AVIF（两者扩展名经常被搞混）
        match &h[8..12] {
            b"heic" | b"heix" | b"heis" | b"heim" | b"hevm" | b"mif1" | b"msf1" => {
                return Some("heic")
            }
            b"avif" => return Some("avif"),
            b"avis" => return Some("avifs"),
            _ => {}
        }
    }
    None
}

/// 该扩展名能否交给**系统缩略图提供者**（T10 L3）。
///
/// 判据只有一个：资源管理器里它有没有缩略图。有，我们就能拿到同一张。
/// 典型来源：Windows 自带的 HEIF 扩展、Adobe 的 PDF/AI 提供者、Media Foundation 的视频抽帧、
/// 各种网盘/设计工具装的 Shell Extension。
pub fn shell_thumbable(ext: &str) -> bool {
    matches!(
        ext,
        // 图片类（系统有解码器或扩展装过）
        "heic" | "heif" | "avif" | "avifs" | "jxl" | "jp2" | "j2k" | "svg" | "eps"
            // Windows 自己对图元文件有缩略图（装了打印/图形组件就有）
            | "wmf" | "emf"
            // 设计稿
            | "psd" | "psb" | "ai" | "indd" | "sketch" | "fig" | "xd" | "afdesign" | "afphoto"
            // 文档
            | "pdf"
            // 视频 / 音频
            | "mp4" | "m4v" | "mov" | "avi" | "mkv" | "webm" | "wmv" | "flv" | "mpg" | "mpeg" | "ts"
            | "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg"
            // 3D / 工程
            | "blend" | "obj" | "fbx" | "3ds" | "stl" | "gltf" | "glb" | "max"
    )
}

/// 该扩展名理论上能否由内置管线出图（不含插件）。
///
/// 2026-11-16 起把 **RAW / PSD 也算进来**：它们的缩略图不走"解码原图"，
/// 而是从文件里抠内嵌 JPEG 预览（`preview.rs`），所以能出图。
/// 真正的失败（文件损坏、预览缺失）由 `thumb::ensure` 标 THUMB_FAILED，不再谎报"不支持"。
pub fn builtin_thumbable(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "apng" | "jpg" | "jpeg" | "jpe" | "jfif" | "gif" | "webp" | "bmp" | "ico" | "cur"
            | "tif" | "tiff" | "tga" | "dds" | "hdr" | "exr" | "pnm" | "pbm" | "pgm" | "ppm"
            | "qoi" | "ff" | "avif" | "avifs"
            // 内嵌预览路线（预览.rs）
            | "cr2" | "cr3" | "crw" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "dng" | "raf"
            | "orf" | "rw2" | "rwl" | "srw" | "pef" | "raw" | "3fr" | "fff" | "iiq" | "mos"
            | "mrw" | "mef" | "erf" | "kdc" | "dcr" | "x3f" | "psd" | "psb"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_and_kind() {
        assert_eq!(lower_ext("a.PNG"), "png");
        assert_eq!(lower_ext("noext"), "");
        assert_eq!(lower_ext(".hidden"), "");
        assert_eq!(kind_of("webp"), KIND_IMAGE);
        assert_eq!(kind_of("blend"), KIND_MODEL);
        assert_eq!(kind_of("zzz"), KIND_OTHER);
    }

    #[test]
    fn sniff_magic() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0x0D]), Some("png"));
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("webp"));
        assert_eq!(sniff(b"not-an-image"), None);
    }

    #[test]
    fn badge_short() {
        assert_eq!(badge("jpeg"), "JPG");
        assert_eq!(badge("psd"), "PSD");
        assert_eq!(badge(""), "FILE");
    }
}
