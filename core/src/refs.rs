//! 引用关系与丢失检测（T26）：**这张图被哪些工程文件用着 / 哪个工程丢了素材**。
//!
//! 做法是"看字符串"：工程文件（psd / blend / max / c4d / ai …）里引用外部素材都是
//! 明文的路径段。扫可打印字符，只在**结尾是已知媒体扩展名**时才认。
//!
//! 三条纪律：
//! 1. **宁可漏，不可假**：假阳性会让「这里丢了 200 个引用」变成噪音，用户就再也不看这个提醒了；
//! 2. 只扫前 `MAX_SCAN` 字节：百万级库不能为了一条引用把 1 GB 的 psb 读完；
//! 3. 相对路径**不猜**：解析不出绝对路径就如实记 `missing=1` 并把原样的字符串留给用户看。

use std::path::Path;

use crate::db::Res;

/// 会出现"外部素材引用"的工程类扩展名（扫这些文件的字节）
pub const PROJECT_EXTS: &[&str] = &[
    "psd", "psb", "ai", "eps", "indd", "blend", "max", "c4d", "ma", "mb", "hip", "hiplc",
    "nk", "nuke", "prproj", "aep", "aepx", "drp", "fcpxml", "sketch", "clip", "xcf", "kra",
    "afdesign", "afphoto", "fig", "cdr", "spp", "zpr", "ztl", "sbsar", "uproject",
];

/// 单个工程最多读这么多字节（16 MB）。超出部分直接放弃 —— 引用几乎都在头部区。
pub const MAX_SCAN: u64 = 16 * 1024 * 1024;

pub fn is_project_ext(ext: &str) -> bool {
    PROJECT_EXTS.contains(&ext)
}

/// 媒体扩展名白名单 = `formats` 里所有"能给用户看见"的类型 + Office/PDF（贴图/参考文档也算引用）
fn is_media_ext(ext: &str) -> bool {
    use crate::formats::*;
    IMAGE_EXTS.contains(&ext)
        || VIDEO_EXTS.contains(&ext)
        || AUDIO_EXTS.contains(&ext)
        || MODEL_EXTS.contains(&ext)
        || DOC_EXTS.contains(&ext)
}

/// 从字节里抠出媒体路径候选（已去重、已排序，最多 2000 条）。
pub fn extract_paths(bytes: &[u8]) -> Vec<String> {
    let mut out: std::collections::BTreeSet<String> = Default::default();

    // ① ASCII / UTF-8：连续可打印字符算一段
    let mut start: Option<usize> = None;
    for (i, b) in bytes.iter().enumerate() {
        let printable = *b >= 0x20 && *b < 0x7F;
        match (printable, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= 5 {
                    push_candidate(&mut out, &bytes[s..i]);
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        if bytes.len() - s >= 5 {
            push_candidate(&mut out, &bytes[s..]);
        }
    }

    // ② UTF-16LE（Windows 工程文件里很常见）：高字节为 0 的成对字节
    let mut buf: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        let (lo, hi) = (bytes[i], bytes[i + 1]);
        if hi == 0 && (0x20..0x7F).contains(&lo) {
            buf.push(lo);
            i += 2;
        } else {
            if buf.len() >= 5 {
                push_candidate(&mut out, &buf);
            }
            buf.clear();
            i += 1;
        }
    }
    if buf.len() >= 5 {
        push_candidate(&mut out, &buf);
    }

    out.into_iter().take(2000).collect()
}

fn push_candidate(out: &mut std::collections::BTreeSet<String>, raw: &[u8]) {
    let Ok(s) = std::str::from_utf8(raw) else { return };
    // 一段里可能粘着前一个字符串的尾巴（`...margin.png`），从后往前找最后一个"像起点"的位置
    for cand in split_candidates(s) {
        let c = cand.trim().trim_matches(|ch: char| ch == '.' || ch == '\u{0}');
        if c.len() < 5 || c.len() > 512 {
            continue;
        }
        let ext = crate::formats::lower_ext(c);
        if ext.is_empty() || !is_media_ext(&ext) {
            continue;
        }
        if !c.contains('.') {
            continue;
        }
        out.insert(c.to_string());
        if out.len() >= 4000 {
            return;
        }
    }
}

/// 一段可打印字符里可能塞了多条路径（用 `;` `,` `|` 分隔，或者粘了别的字符串），
/// 按分隔符切开，并尝试从每个分隔符之后重新开始。
fn split_candidates(s: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for part in s.split([';', ',', '|', '\t']) {
        if part.is_empty() {
            continue;
        }
        v.push(part.to_string());
        // 常见的粘连：`C:\a\tex.pngC:\a\b.png`（无分隔符）→ 按扩展名断句
        let ext = crate::formats::lower_ext(part);
        if !ext.is_empty() && part.len() > ext.len() + 4 {
            let cut = part.len() - ext.len() - 1;
            if let Some(pos) = part[..cut].rfind([':', '\\', '/']) {
                // 保留从 0 开始的整段（可能真是长路径），另外补一条"从最后一个分隔处断开"的
                let tail_start = part[..cut]
                    .char_indices()
                    .rev()
                    .find(|(_, c)| *c == ':' || *c == '\\' || *c == '/')
                    .map(|(i, _)| i)
                    .unwrap_or(pos);
                if tail_start > 0 && tail_start + 1 < part.len() {
                    v.push(part[tail_start + 1..].to_string());
                }
            }
        }
    }
    v
}

/// 读一个工程文件的前 `MAX_SCAN` 字节，返回它引用的媒体路径（原样字符串）。
pub fn scan_file(path: &Path) -> Res<Vec<String>> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() == 0 {
        return Ok(Vec::new());
    }
    let take = meta.len().min(MAX_SCAN) as usize;
    let mut buf = vec![0u8; take];
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut read = 0usize;
    while read < take {
        match f.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) => return Err(format!("读工程文件失败：{e}")),
        }
    }
    buf.truncate(read);
    Ok(extract_paths(&buf))
}

/// 把引用字符串落成「绝对路径」的几种尝试（顺序即优先级）。
/// 解析不出来就返回空 —— 上层按 `missing=1` 记，别猜。
pub fn resolve(project_path: &Path, raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let s = raw.trim();
    if s.is_empty() {
        return out;
    }
    // Windows 绝对路径 / UNC
    let looks_abs = s.len() > 2 && (s.as_bytes()[1] == b':' || s.starts_with("\\\\"));
    if looks_abs {
        out.push(s.replace('/', "\\"));
        return out;
    }
    if let Some(parent) = project_path.parent() {
        out.push(parent.join(s).to_string_lossy().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ascii_and_utf16_paths() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"\x00\x01\x02junk");
        buf.extend_from_slice(b"C:\\Users\\me\\tex\\wood.png");
        buf.extend_from_slice(&[0u8, 0x01, 0x02]);
        // UTF-16LE 的 D:\shot\final.psd
        for ch in "D:\\shot\\final.jpg".chars() {
            buf.push(ch as u8);
            buf.push(0);
        }
        buf.extend_from_slice(b"\xff\xfe random bytes");
        let got = extract_paths(&buf);
        assert!(got.iter().any(|p| p.ends_with("wood.png")), "{got:?}");
        assert!(got.iter().any(|p| p.ends_with("final.jpg")), "{got:?}");
    }

    /// 乱码 / 非媒体文件不该被当成引用（假阳性会让提醒变成噪音）
    #[test]
    fn rejects_noise() {
        let got = extract_paths(b"SELECT * FROM users WHERE name = ?; hello.txt.zip");
        assert!(!got.iter().any(|p| p.contains("SELECT")), "{got:?}");
        assert!(extract_paths(b"abcdefgh").is_empty());
        assert!(extract_paths(&[0u8; 64]).is_empty());
    }

    #[test]
    fn resolves_relative_against_project_dir() {
        let p = Path::new("E:/proj/scene.blend");
        let r = resolve(p, "textures/wood.png");
        assert_eq!(r.len(), 1);
        assert!(r[0].replace('/', "\\").ends_with("proj\\textures\\wood.png"), "{r:?}");
        let abs = resolve(p, "D:/lib/a.jpg");
        assert_eq!(abs, vec!["D:\\lib\\a.jpg"]);
        assert!(resolve(p, "  ").is_empty());
    }

    #[test]
    fn scans_real_file_head() {
        let d = std::env::temp_dir().join(format!("am-refs-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.psd");
        let mut v = b"8BPS".to_vec();
        v.extend_from_slice(b"\x00\x00C:/tex/brick.tif\x00\x00");
        std::fs::write(&f, v).unwrap();
        let got = scan_file(&f).unwrap();
        assert!(got.iter().any(|p| p.ends_with("brick.tif")), "{got:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
