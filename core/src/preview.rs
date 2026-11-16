//! 内嵌预览提取：相机 RAW / PSD / CR3 / RAF 这些格式**自己就带一张 JPEG 预览**，
//! 直接把它抠出来当缩略图源，比"解码整张原图"快一到两个数量级，也不会吃掉几百 MB 内存。
//!
//! 为什么不全靠 demosaic（真解码 RAW）：
//! - 一张 45 MP 的 ARW 解码后 ≈ 480 MB RGBA，几路并发就是这个进程的死刑；
//! - 缩略图最长边只要 1024px，RAW 里那张 1616×1080 的预览绰绰有余；
//! - 用户要的是「快速转换成缩略图」，嵌入预览就是相机厂商自己做好的那一步。
//!
//! 实现策略是**容器无关的 JPEG 流扫描**：
//! 文件头 24 MB 里扫 `FF D8 FF … FF D9`，把候选按 SOF 里的面积排序，取最大的那张
//! 拿去真正解码验证。理由：
//! - TIFF 系（CR2/NEF/ARW/DNG/RW2/ORF/PEF/SRW）把预览放在 IFD 指向的偏移里，
//!   各厂商 tag 用法还不统一（0x0201 / SubIFD / 0x0111 前缀），逐个适配会写成一坨；
//! - CR3 / PSD / RAF 又是另一套容器（BMFF / resource block / 私有头）；
//! - 而"预览都是一段合法 JPEG"这件事是**共通的**。假阳性靠解码验证兜掉
//!   （RAW 的传感器数据凑不出一个能解出图的 JPEG 头和 EOI）。
//!
//! 代价：文件头 24 MB 要扫一遍。实测比解码原图快得多，且内存只有那 24 MB 的切片。

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// 扫描上限：预览几乎都在文件前几 MB；封顶是为了不让 200 MB 的 RAW 拖慢首屏。
const MAX_SCAN: u64 = 24 * 1024 * 1024;
/// 单张预览的合理下限：相机里最小的缩略图（160×120）也有 2 KB 上下
const MIN_PREVIEW: usize = 1024;
/// 单张预览的合理上限（超过基本是误判出来的假 EOI）
const MAX_PREVIEW: usize = 24 * 1024 * 1024;
/// 候选上限
const MAX_CANDIDATES: usize = 24;

pub struct Preview {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// 质量档说明，写进 assets.thumb_quality（1=自解码，2=嵌入预览）
    pub embedded: bool,
}

/// 这些扩展名值得先试内嵌预览（普通位图直接解码更快，不要多扫一遍文件）。
pub fn prefers_embedded(ext: &str) -> bool {
    matches!(
        ext,
        // 相机 RAW（TIFF 系）
        "cr2" | "crw" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "dng" | "orf" | "rw2" | "rwl"
            | "srw" | "pef" | "raw" | "3fr" | "fff" | "iiq" | "mos" | "mrw" | "mef" | "erf"
            | "kdc" | "dcr" | "x3f"
            // Canon 的新容器 / 富士 / 工程稿
            | "cr3" | "raf" | "psd" | "psb"
    )
}

/// 从文件里抠出最大的一张内嵌 JPEG 预览。
pub fn extract(path: &Path) -> Option<Preview> {
    let mut f = File::open(path).ok()?;
    let total = f.metadata().ok()?.len();
    if total < 4096 {
        return None;
    }
    let want = total.min(MAX_SCAN) as usize;
    let mut buf = vec![0u8; want];
    // 短读不算错：能读多少扫多少
    let mut got = 0usize;
    while got < want {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(_) => break,
        }
    }
    buf.truncate(got);
    if buf.len() < 4096 {
        return None;
    }

    let mut cands = collect_jpegs(&buf);
    // 面积优先、字节数次之：TIFF 里常同时有 160×120 缩略图和 1616×1080 预览
    cands.sort_by(|a, b| (b.w as u64 * b.h as u64, b.len).cmp(&(a.w as u64 * a.h as u64, a.len)));

    for c in cands.iter().take(8) {
        let slice = &buf[c.off..c.off + c.len];
        // 真解一次：既验证合法性，又拿到真实宽高（SOF 里的值偶尔和实际不符）
        let Ok(img) = image::load_from_memory(slice) else { continue };
        let (w, h) = (img.width(), img.height());
        if w < 64 || h < 64 {
            continue;
        }
        return Some(Preview {
            bytes: slice.to_vec(),
            width: w,
            height: h,
            embedded: true,
        });
    }
    None
}

struct Cand {
    off: usize,
    len: usize,
    w: u32,
    h: u32,
}

/// 扫出所有「SOI … EOI」候选，并用 SOF 头估一下尺寸（不解码，纯字节）。
fn collect_jpegs(buf: &[u8]) -> Vec<Cand> {
    let mut out: Vec<Cand> = Vec::new();
    let n = buf.len();
    let mut i = 0usize;
    while i + 3 < n {
        // 找 FF D8 FF
        match buf[i..].iter().position(|&b| b == 0xFF) {
            Some(p) => i += p,
            None => break,
        }
        if i + 3 >= n {
            break;
        }
        if !(buf[i] == 0xFF && buf[i + 1] == 0xD8 && buf[i + 2] == 0xFF) {
            i += 1;
            continue;
        }
        // 从 SOI 往后找 EOI
        let mut j = i + 2;
        let mut end = None;
        while j + 1 < n {
            if buf[j] == 0xFF && buf[j + 1] == 0xD9 {
                end = Some(j + 2);
                break;
            }
            j += 1;
        }
        let Some(e) = end else { break };
        let len = e - i;
        if (MIN_PREVIEW..=MAX_PREVIEW).contains(&len) {
            let (w, h) = jpeg_dims(&buf[i..e]).unwrap_or((0, 0));
            out.push(Cand { off: i, len, w, h });
            if out.len() >= MAX_CANDIDATES {
                break;
            }
        }
        i = e.max(i + 2);
    }
    out
}

/// 只读 JPEG 的 SOF 段拿宽高，不碰像素数据。
pub fn jpeg_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    let n = bytes.len();
    let mut i = 2usize; // 跳过 SOI
    while i + 3 < n {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // 无长度的独立标记
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            return None; // 到 SOS 还没见到 SOF，说明没有（不是合法 baseline/progressive 头）
        }
        let seg = ((bytes[i + 2] as usize) << 8) | bytes[i + 3] as usize;
        if seg < 2 || i + 2 + seg > n {
            return None;
        }
        // SOF0..SOF15，除去 C4(DHT) / C8(JPG) / CC(DAC)
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            if i + 9 > n {
                return None;
            }
            let h = ((bytes[i + 5] as u32) << 8) | bytes[i + 6] as u32;
            let w = ((bytes[i + 7] as u32) << 8) | bytes[i + 8] as u32;
            if w > 0 && h > 0 {
                return Some((w, h));
            }
            return None;
        }
        i += 2 + seg;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个最小可用的 JPEG 头（SOI + SOF0 + EOI），只为验证 SOF 解析。
    fn fake_jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        // SOF0: len=11, precision=8, h, w, ncomp=1
        v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
        // COM 段填充到 > MIN_PREVIEW，模拟真实预览的体积
        let pad = 2000usize;
        v.extend_from_slice(&[0xFF, 0xFE]);
        v.extend_from_slice(&((pad + 2) as u16).to_be_bytes());
        v.extend(std::iter::repeat(0u8).take(pad));
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    }

    #[test]
    fn sof_dims() {
        let j = fake_jpeg(1616, 1080);
        assert_eq!(jpeg_dims(&j), Some((1616, 1080)));
        assert_eq!(jpeg_dims(&[0xFF, 0xD8, 0x00, 0x00]), None);
    }

    #[test]
    fn find_embedded() {
        let mut buf = vec![0u8; 8192];
        let j = fake_jpeg(640, 480);
        buf[1000..1000 + j.len()].copy_from_slice(&j);
        let c = collect_jpegs(&buf);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].off, c[0].w, c[0].h), (1000, 640, 480));
    }

    #[test]
    fn prefers() {
        assert!(prefers_embedded("cr3"));
        assert!(prefers_embedded("psd"));
        assert!(!prefers_embedded("jpg"));
    }
}
