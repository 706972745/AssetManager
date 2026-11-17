//! EXIF 拍摄信息（P2.5 T41）。
//!
//! 为什么自己写而不引 crate：只有"读 JPEG 的 APP1 里那几个常用 tag"这一个需求，
//! 为它多一个依赖（还要联网拉注册表）不值得。这里就是一个最小的 TIFF/IFD 走读器，
//! 60 行核心逻辑 + 一张 tag 表。
//!
//! 支持的容器：**JPEG**（APP1 Exif）。PNG/WebP 里也可能有 EXIF 块，
//! 但真实素材库里极少见，需要时再加（接口已经按"返回 Option"设计，读不到就不显示那一段）。
//!
//! 读的字段：机身 / 镜头 / 光圈 / 快门 / ISO / 焦距 / 拍摄时间 / 方向。

use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Default, Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exif {
    pub make: String,
    pub model: String,
    pub lens: String,
    /// 光圈值（f/1.8 → 1.8）
    pub f_number: Option<f64>,
    /// 快门，已经格式化成 `1/250` 或 `2.5`
    pub exposure: Option<String>,
    pub iso: Option<u32>,
    /// 焦距（mm）
    pub focal: Option<f64>,
    /// 原始拍摄时间字符串，EXIF 的 `YYYY:MM:DD HH:MM:SS`
    pub taken: Option<String>,
    pub software: String,
}

impl Exif {
    pub fn is_empty(&self) -> bool {
        self.make.is_empty()
            && self.model.is_empty()
            && self.lens.is_empty()
            && self.f_number.is_none()
            && self.exposure.is_none()
            && self.iso.is_none()
            && self.focal.is_none()
            && self.taken.is_none()
    }

    /// 一行摘要，给详情面板的小字用
    pub fn summary(&self) -> String {
        let mut v: Vec<String> = Vec::new();
        if !self.model.is_empty() {
            v.push(self.model.clone());
        } else if !self.make.is_empty() {
            v.push(self.make.clone());
        }
        if let Some(f) = self.f_number {
            v.push(format!("f/{f}"));
        }
        if let Some(e) = &self.exposure {
            v.push(format!("{e}s"));
        }
        if let Some(i) = self.iso {
            v.push(format!("ISO {i}"));
        }
        if let Some(f) = self.focal {
            v.push(format!("{}mm", fmt_num(f)));
        }
        v.join(" · ")
    }
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// 读一个文件的 EXIF。非 JPEG / 没有 Exif 段 → `None`。
pub fn read(path: &Path) -> Option<Exif> {
    let head = read_head(path, 512 * 1024)?;
    parse_jpeg(&head)
}

fn read_head(path: &Path, max: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max];
    let mut got = 0usize;
    loop {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => {
                got += n;
                if got >= max {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    buf.truncate(got);
    Some(buf)
}

fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(o)?, *b.get(o + 1)?]))
}
fn le16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]))
}
fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}

/// 走 JPEG 段结构，找到 APP1 里的 "Exif\0\0"，再做 TIFF 解析。
fn parse_jpeg(b: &[u8]) -> Option<Exif> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        /* 0xD8/0x01/0xD0-0xD7 没有长度字段 */
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            /* 到扫描数据了，后面不再有元数据段 */
            return None;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if len < 2 || i + 2 + len > b.len() {
            return None;
        }
        let seg = &b[i + 4..i + 2 + len];
        if marker == 0xE1 && seg.len() > 6 && &seg[..6] == b"Exif\0\0" {
            return parse_tiff(&seg[6..]);
        }
        i += 2 + len;
    }
    None
}

/// TIFF 头 + IFD0 / ExifIFD
fn parse_tiff(t: &[u8]) -> Option<Exif> {
    if t.len() < 8 {
        return None;
    }
    let le = match &t[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| if le { le16(t, o) } else { be16(t, o) };
    let u32_at = |o: usize| if le { le32(t, o) } else { be32(t, o) };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd0 = u32_at(4)? as usize;
    let tags0 = read_ifd(t, ifd0, le)?;
    let exif_ptr = tags0.get(&0x8769).and_then(|v| v.first()).copied().unwrap_or(0) as usize;
    let tags1 = if exif_ptr > 0 {
        read_ifd(t, exif_ptr, le).unwrap_or_default()
    } else {
        HashMap::new()
    };

    let ascii = |tags: &HashMap<u16, Vec<u64>>, k: u16| -> String {
        tags.get(&k)
            .and_then(|v| v.first().copied())
            .and_then(|off| read_ascii(t, off as usize))
            .unwrap_or_default()
    };
    let num = |tags: &HashMap<u16, Vec<u64>>, k: u16| -> Option<f64> {
        tags.get(&k).and_then(|v| v.first().copied()).map(|raw| {
            /* 值存储（见 read_ifd）：有理数是 `分子<<32 | 分母`，普通整数就是它自己。
               高 32 位为 0 一定不是有理数（分子不可能是 0），据此区分。 */
            let hi = (raw >> 32) as u32;
            let lo = raw as u32;
            if hi == 0 {
                lo as f64
            } else {
                hi as f64 / lo as f64
            }
        })
    };

    let mut e = Exif {
        make: ascii(&tags0, 0x010F),
        model: ascii(&tags0, 0x0110),
        software: ascii(&tags0, 0x0131),
        lens: ascii(&tags1, 0xA434),
        taken: {
            let s = ascii(&tags1, 0x9003);
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        },
        iso: num(&tags1, 0x8827).map(|v| v as u32).filter(|v| *v > 0),
        f_number: num(&tags1, 0x829D).filter(|v| *v > 0.0),
        focal: num(&tags1, 0x920A).filter(|v| *v > 0.0),
        exposure: None,
    };
    /* 快门：>1 秒写成 "2.5"，其余写成 "1/250"（摄影界的习惯写法） */
    if let Some(sec) = num(&tags1, 0x829A).filter(|v| *v > 0.0) {
        e.exposure = Some(if sec >= 1.0 {
            format!("{sec:.1}")
        } else {
            format!("1/{}", (1.0 / sec).round() as i64)
        });
    }
    if e.is_empty() {
        None
    } else {
        Some(e)
    }
}

/// 读一个 IFD，返回 tag → 值列表。
///
/// 值统一压成 `u64`：**有理数**（type 5/10）压成 `分子<<32 | 分母`，
/// 其它整数类型直接取数值 —— 这样上层只需要一套取值逻辑。
fn read_ifd(t: &[u8], off: usize, le: bool) -> Option<HashMap<u16, Vec<u64>>> {
    let u16_at = |o: usize| if le { le16(t, o) } else { be16(t, o) };
    let u32_at = |o: usize| if le { le32(t, o) } else { be32(t, o) };
    let n = u16_at(off)? as usize;
    if n > 512 {
        return None;
    }
    let mut out = HashMap::new();
    for i in 0..n {
        let e = off + 2 + i * 12;
        if e + 12 > t.len() {
            break;
        }
        let tag = u16_at(e)?;
        let ty = u16_at(e + 2)?;
        let cnt = u32_at(e + 4)? as usize;
        let (unit, is_ratio) = match ty {
            1 | 7 => (1usize, false),  // BYTE / UNDEFINED
            2 => (1, false),           // ASCII
            3 => (2, false),           // SHORT
            4 | 9 => (4, false),       // LONG / SLONG
            5 | 10 => (8, true),       // RATIONAL / SRATIONAL
            /* FLOAT / DOUBLE 用不上，跳过（别拿整数逻辑去读，会读出垃圾值） */
            _ => continue,
        };
        let total = unit * cnt;
        let val_off = if total <= 4 {
            e + 8
        } else {
            u32_at(e + 8)? as usize
        };
        /* ASCII：值本身就是"字符串在哪儿"，直接记偏移，别按字节展开 */
        if ty == 2 {
            out.insert(tag, vec![val_off as u64]);
            continue;
        }
        if cnt == 0 || val_off + total > t.len() {
            continue;
        }
        let mut vals = Vec::with_capacity(cnt.min(64));
        for k in 0..cnt.min(64) {
            let o = val_off + k * unit;
            let v = if is_ratio {
                let (num, den) = if le {
                    (le32(t, o)? as u64, le32(t, o + 4)? as u64)
                } else {
                    (be32(t, o)? as u64, be32(t, o + 4)? as u64)
                };
                (num << 32) | den
            } else {
                match ty {
                    1 | 7 => *t.get(o)? as u64,
                    3 => u16_at(o)? as u64,
                    _ => u32_at(o)? as u64,
                }
            };
            vals.push(v);
        }
        out.insert(tag, vals);
    }
    Some(out)
}

/// tag 2（ASCII）的值是"偏移量"，真正字符串在 TIFF 内偏移处
fn read_ascii(t: &[u8], off: usize) -> Option<String> {
    if off >= t.len() {
        return None;
    }
    let end = t[off..]
        .iter()
        .position(|c| *c == 0)
        .map(|n| off + n)
        .unwrap_or_else(|| (off + 128).min(t.len()));
    let s = String::from_utf8_lossy(&t[off..end]).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手搓一个最小 Exif（大端）：IFD0 里放 Make/Model，ExifIFD 里放 f/1.8、1/250、ISO 200、50mm
    fn build() -> Vec<u8> {
        let mut t: Vec<u8> = Vec::new();
        t.extend_from_slice(b"MM\0\x2a"); // BE + 42
        t.extend_from_slice(&8u32.to_be_bytes()); // IFD0 偏移
        /* IFD0：3 个条目 */
        let ifd0 = t.len();
        t.extend_from_slice(&3u16.to_be_bytes());
        // Make → 偏移（写在后面）
        let make_at = 200usize;
        let model_at = 216usize;
        let exif_ifd_at = 240usize;
        let entry = |tag: u16, ty: u16, cnt: u32, val: u32| {
            let mut b = Vec::new();
            b.extend_from_slice(&tag.to_be_bytes());
            b.extend_from_slice(&ty.to_be_bytes());
            b.extend_from_slice(&cnt.to_be_bytes());
            if ty == 3 && cnt == 1 {
                b.extend_from_slice(&(val as u16).to_be_bytes());
                b.extend_from_slice(&[0, 0]);
            } else {
                b.extend_from_slice(&val.to_be_bytes());
            }
            b
        };
        t.extend(entry(0x010F, 2, 7, make_at as u32));
        t.extend(entry(0x0110, 2, 5, model_at as u32)); // "R6II\0"
        t.extend(entry(0x8769, 4, 1, exif_ifd_at as u32));
        t.extend_from_slice(&0u32.to_be_bytes()); // 下一个 IFD = 0
        assert_eq!(t.len(), ifd0 + 2 + 36 + 4);
        t.resize(make_at, 0);
        t.extend_from_slice(b"Canon\0\0"); // 7 字节（"Canon"+2 个 0）
        t.resize(model_at, 0);
        t.extend_from_slice(b"R6II\0"); // 5 字节（>4 才是偏移式存储）
        t.resize(exif_ifd_at, 0);
        /* ExifIFD：4 个条目 + 有理数区 */
        let ratio_at = exif_ifd_at + 2 + 4 * 12 + 4;
        t.extend_from_slice(&4u16.to_be_bytes());
        t.extend(entry(0x829D, 5, 1, ratio_at as u32)); // FNumber 18/10
        t.extend(entry(0x829A, 5, 1, (ratio_at + 8) as u32)); // 1/250
        t.extend(entry(0x8827, 3, 1, 200));
        t.extend(entry(0x920A, 5, 1, (ratio_at + 16) as u32)); // 50/1
        t.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(t.len(), ratio_at);
        t.extend_from_slice(&18u32.to_be_bytes());
        t.extend_from_slice(&10u32.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());
        t.extend_from_slice(&250u32.to_be_bytes());
        t.extend_from_slice(&50u32.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());
        /* 包成 JPEG APP1 */
        let mut jpg: Vec<u8> = vec![0xFF, 0xD8];
        jpg.extend_from_slice(&[0xFF, 0xE1]);
        let len = (2 + 6 + t.len()) as u16;
        jpg.extend_from_slice(&len.to_be_bytes());
        jpg.extend_from_slice(b"Exif\0\0");
        jpg.extend_from_slice(&t);
        jpg.extend_from_slice(&[0xFF, 0xD9]);
        jpg
    }

    #[test]
    fn parses_common_tags() {
        let e = parse_jpeg(&build()).expect("应该能读出 Exif");
        assert_eq!(e.make, "Canon");
        assert_eq!(e.model, "R6II");
        assert!((e.f_number.unwrap() - 1.8).abs() < 0.001);
        assert_eq!(e.exposure.as_deref(), Some("1/250"));
        assert_eq!(e.iso, Some(200));
        assert!((e.focal.unwrap() - 50.0).abs() < 0.001);
        assert!(e.summary().contains("R6II"));
    }

    #[test]
    fn non_jpeg_and_garbage_are_none() {
        assert!(parse_jpeg(b"\x89PNG\r\n\x1a\n").is_none());
        assert!(parse_jpeg(b"").is_none());
        assert!(parse_jpeg(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x08, b'E', b'x']).is_none());
    }
}
