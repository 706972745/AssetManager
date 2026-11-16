//! 只读文件头拿宽高 —— 百万级扫描不解码像素的前提。
//!
//! 返回 None 表示"这种格式暂时读不出尺寸"，布局层按 4:3 兜底，
//! 出图后再平滑过渡（见 UI 规格 §6.5）。

use std::fs::File;
use std::io::Read;
use std::path::Path;

const HEAD: usize = 64 * 1024;

fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let mut buf = vec![0u8; HEAD];
    let mut n = 0;
    loop {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => {
                n += k;
                if n == HEAD { break; }
            }
            Err(_) => break,
        }
    }
    buf.truncate(n);
    Some(buf)
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o + 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn le32(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn le16(b: &[u8], o: usize) -> Option<u16> {
    let s = b.get(o..o + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

/// PNG：IHDR 紧跟 8 字节签名，宽高在 16..24（大端）。
fn png(b: &[u8]) -> Option<(u32, u32)> {
    Some((be32(b, 16)?, be32(b, 20)?))
}

/// GIF：Logical Screen Descriptor，LE。
fn gif(b: &[u8]) -> Option<(u32, u32)> {
    Some((le16(b, 6)? as u32, le16(b, 8)? as u32))
}

/// BMP：DIB 头，12 = CORE(u16)，其余 i32。
fn bmp(b: &[u8]) -> Option<(u32, u32)> {
    let hdr = le32(b, 14)?;
    if hdr == 12 {
        Some((le16(b, 18)? as u32, le16(b, 20)? as u32))
    } else {
        let w = le32(b, 18)? as i32;
        let h = le32(b, 22)? as i32;
        Some((w.unsigned_abs(), h.unsigned_abs()))
    }
}

/// ICO/CUR：第一个目录项，0 表示 256。
fn ico(b: &[u8]) -> Option<(u32, u32)> {
    let w = *b.get(6)? as u32;
    let h = *b.get(7)? as u32;
    Some((if w == 0 { 256 } else { w }, if h == 0 { 256 } else { h }))
}

/// JPEG：遍历段找 SOF0/1/2/9/10，宽高各 2 字节。
fn jpeg(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        // 填充字节
        if marker == 0xFF { i += 1; continue; }
        // 无长度的独立段
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let len = le16(b, i + 2)? as usize;
        if matches!(marker, 0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC9 | 0xCA | 0xCB) {
            let h = le16(b, i + 5)? as u32;
            let w = le16(b, i + 7)? as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

/// WebP：VP8X / VP8（有损）/ VP8L（无损）三种。
fn webp(b: &[u8]) -> Option<(u32, u32)> {
    let tag = b.get(12..16)?;
    match tag {
        b"VP8X" => {
            let w = *b.get(24)? as u32 | ((*b.get(25)? as u32) << 8) | ((*b.get(26)? as u32) << 16);
            let h = *b.get(27)? as u32 | ((*b.get(28)? as u32) << 8) | ((*b.get(29)? as u32) << 16);
            Some((w + 1, h + 1))
        }
        b"VP8 " => {
            // 关键帧起始码 9D 01 2A 之后是 14bit 宽、14bit 高
            if b.get(23..26)? != [0x9D, 0x01, 0x2A] { return None; }
            let w = le16(b, 26)? as u32 & 0x3FFF;
            let h = le16(b, 28)? as u32 & 0x3FFF;
            Some((w, h))
        }
        b"VP8L" => {
            if *b.get(20)? != 0x2F { return None; }
            let bits = le32(b, 21)?;
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            Some((w, h))
        }
        _ => None,
    }
}

/// PSD/PSB：26 字节文件头，14=高（rows）、18=宽（cols），大端。
/// 相机 RAW 与 PSD 都读不出尺寸的话，瀑布流只能按 4:3 兜底 → 出图后跳行，
/// 所以这两个在扫描阶段就要拿到真值。
fn psd(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 26 { return None; }
    let h = be32(b, 14)?;
    let w = be32(b, 18)?;
    Some((w, h))
}

/// TIFF 系 RAW（CR2/NEF/ARW/DNG/RW2/ORF/PEF/SRW…）：读 IFD0 的 ImageWidth/ImageLength。
///
/// 只读 IFD0（64 KB 头里基本都在），不追 SubIFD —— 那里面多半是缩略图尺寸，
/// 追进去反而把一张 45 MP 的图算成 160×120。读不到就让上层兜底。
fn tiff(b: &[u8]) -> Option<(u32, u32)> {
    let le = match b.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16at = |o: usize| -> Option<u16> {
        let s = b.get(o..o + 2)?;
        Some(if le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    };
    let u32at = |o: usize| -> Option<u32> {
        let s = b.get(o..o + 4)?;
        Some(if le {
            u32::from_le_bytes([s[0], s[1], s[2], s[3]])
        } else {
            u32::from_be_bytes([s[0], s[1], s[2], s[3]])
        })
    };
    // 魔数 42（ORF 是 0x4F52，CR2 仍是 42）
    let magic = u16at(2)?;
    if magic != 42 && magic != 0x4F52 { return None; }
    let ifd = u32at(4)? as usize;
    let count = u16at(ifd)? as usize;
    if count == 0 || count > 4096 { return None; }
    let mut w = 0u32;
    let mut h = 0u32;
    for i in 0..count {
        let e = ifd + 2 + i * 12;
        let tag = u16at(e)?;
        if tag != 0x0100 && tag != 0x0101 { continue; }
        let typ = u16at(e + 2)?;
        // 值只有 1 个：SHORT 落在 e+8 的前两字节，LONG 直接占 e+8..e+12
        let v = match typ {
            3 => u16at(e + 8)? as u32,
            4 => u32at(e + 8)?,
            _ => continue,
        };
        if tag == 0x0100 { w = v; } else { h = v; }
        if w > 0 && h > 0 { return Some((w, h)); }
    }
    None
}

/// 手动解析（覆盖最常见格式），失败再交给 image crate 读头。
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let b = read_head(path)?;
    let d = if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        png(&b)
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        jpeg(&b)
    } else if b.starts_with(b"GIF8") {
        gif(&b)
    } else if b.len() >= 16 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        webp(&b)
    } else if b.starts_with(b"BM") {
        bmp(&b)
    } else if b.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        ico(&b)
    } else if b.starts_with(b"8BPS") {
        psd(&b)
    } else if b.starts_with(b"II*\x00") || b.starts_with(b"MM\x00*") || b.starts_with(b"IIRO") || b.starts_with(b"MMOR") {
        // 相机 RAW（TIFF 容器）—— 放在 image 兜底前面，否则 RAW 会去猜格式失败
        tiff(&b)
    } else {
        None
    };
    if d.is_some() && d != Some((0, 0)) {
        return d;
    }
    // 兜底：只读头不解全图
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_png_header() {
        let mut b = vec![0u8; 32];
        b[0..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        b[12..16].copy_from_slice(b"IHDR");
        b[16..20].copy_from_slice(&1920u32.to_be_bytes());
        b[20..24].copy_from_slice(&1080u32.to_be_bytes());
        assert_eq!(png(&b), Some((1920, 1080)));
    }

    #[test]
    fn parse_gif_header() {
        let mut b = vec![0u8; 16];
        b[0..6].copy_from_slice(b"GIF89a");
        b[6..8].copy_from_slice(&640u16.to_le_bytes());
        b[8..10].copy_from_slice(&480u16.to_le_bytes());
        assert_eq!(gif(&b), Some((640, 480)));
    }

    #[test]
    fn parse_webp_vp8x() {
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(b"RIFF");
        b[8..12].copy_from_slice(b"WEBP");
        b[12..16].copy_from_slice(b"VP8X");
        // 宽 999 / 高 777（各减 1 存 24bit）
        let w = 999u32 - 1;
        let h = 777u32 - 1;
        b[24] = (w & 0xFF) as u8;
        b[25] = ((w >> 8) & 0xFF) as u8;
        b[26] = ((w >> 16) & 0xFF) as u8;
        b[27] = (h & 0xFF) as u8;
        b[28] = ((h >> 8) & 0xFF) as u8;
        b[29] = ((h >> 16) & 0xFF) as u8;
        assert_eq!(webp(&b), Some((999, 777)));
    }

    #[test]
    fn parse_psd_header() {
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(b"8BPS");
        b[4..6].copy_from_slice(&1u16.to_be_bytes());
        b[14..18].copy_from_slice(&4000u32.to_be_bytes()); // rows
        b[18..22].copy_from_slice(&6000u32.to_be_bytes()); // cols
        assert_eq!(psd(&b), Some((6000, 4000)));
    }

    #[test]
    fn parse_tiff_ifd0() {
        // II 42, IFD0 @8: 2 条记录（ImageWidth=SHORT 3000, ImageLength=LONG 2000）
        let mut b = vec![0u8; 64];
        b[0..4].copy_from_slice(&[0x49, 0x49, 0x2A, 0x00]);
        b[4..8].copy_from_slice(&8u32.to_le_bytes());
        b[8..10].copy_from_slice(&2u16.to_le_bytes());
        // 条目1 @10: tag 0x0100, type 3(SHORT), count 1, value 3000
        b[10..12].copy_from_slice(&0x0100u16.to_le_bytes());
        b[12..14].copy_from_slice(&3u16.to_le_bytes());
        b[14..18].copy_from_slice(&1u32.to_le_bytes());
        b[18..20].copy_from_slice(&3000u16.to_le_bytes());
        // 条目2 @22: tag 0x0101, type 4(LONG), count 1, value 2000
        b[22..24].copy_from_slice(&0x0101u16.to_le_bytes());
        b[24..26].copy_from_slice(&4u16.to_le_bytes());
        b[26..30].copy_from_slice(&1u32.to_le_bytes());
        b[30..34].copy_from_slice(&2000u32.to_le_bytes());
        assert_eq!(tiff(&b), Some((3000, 2000)));
    }

    #[test]
    fn parse_bmp_header() {
        let mut b = vec![0u8; 32];
        b[0..2].copy_from_slice(b"BM");
        b[14..18].copy_from_slice(&40u32.to_le_bytes());
        b[18..22].copy_from_slice(&300i32.to_le_bytes());
        b[22..26].copy_from_slice(&(-200i32).to_le_bytes());
        assert_eq!(bmp(&b), Some((300, 200)));
    }
}
