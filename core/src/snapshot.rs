//! 快照（0.12.0）：把一批图片**按内容指纹**记下来，导出的是一个轻量清单。
//!
//! ## 为什么身份是「内容哈希」而不是 id 或路径
//!
//! 用户对快照的期待是：**导出一次，以后任何情况下导回来都还能对上那批图**。
//! 但下面三件事都会变，而且经常发生：
//!
//! | 变了什么 | id 方案 | 路径方案 | 内容哈希（本模块） |
//! |---|---|---|---|
//! | 图片改名 | ✅ id 不变 | ❌ 找不到 | ✅ 认得出 |
//! | 图片换目录 / 换盘 | ✅ | ❌ | ✅ |
//! | 移出旧库、作为另一个库重新入库（id 全新） | ❌ | ❌ | ✅ |
//! | 重建索引 / 拆分老库（id 全打乱） | ❌ | ✅ | ✅ |
//! | 换成同名的**另一张**图 | ❌ 会认错 | ❌ 会认错 | ✅ 不会认错 |
//!
//! 所以条目里存 `hash`（xxh3-128，32 位十六进制），id 只在当次运行里用，**绝不写进文件**。
//!
//! ## 匹配的两条路（`match_hash`）
//!
//! 1. **哈希直接命中**：库里 `assets.hash_x` 已经有值的，一步到位（有 `idx_assets_hash`）。
//! 2. **同体积候选 + 现算**：刚改名/刚入库的行 `hash_x` 还是 NULL（扫描不算哈希，
//!    只有查重和本模块按需算）。这时按文件体积捞候选，逐个算一遍哈希比对 ——
//!    体积相同的文件很少，算的是真字节，所以结论还是可靠的，不是"名字像就算"。
//!
//! 两条都不中才记为「找不到」，并且**永远不删条目**：用户可能只是暂时拔了移动硬盘。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dupes;

/// 快照里的一条。**序列化就是导出文件的格式**（刻意保持人类可读，出问题能手工看）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapItem {
    /// xxh3-128，32 位小写十六进制。身份就是它
    pub hash: String,
    /// 最后一次见到的文件名（只用于显示，匹配时不用）
    #[serde(default)]
    pub name: String,
    /// 文件体积（第二道匹配线索；也是"这张图多大"的显示值）
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub w: Option<i64>,
    #[serde(default)]
    pub h: Option<i64>,
    /// 最后一次见到的绝对路径（只用于显示 / "在资源管理器里显示"）
    #[serde(default)]
    pub path: String,
    /// 最后一次所在的库根（显示"它原来在哪个库"）
    #[serde(default)]
    pub lib: String,
}

/// 导出的快照文件（v1）。`kind` 是给人的保险：别把别的 json 当快照导进来。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapFile {
    pub app: String,
    pub kind: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub exported_at: i64,
    pub items: Vec<SnapItem>,
}

pub const SNAP_VERSION: u32 = 1;

impl SnapFile {
    pub fn new(name: &str, items: Vec<SnapItem>) -> Self {
        Self {
            app: "AssetManager".into(),
            kind: "snapshot".into(),
            version: SNAP_VERSION,
            name: name.into(),
            exported_at: crate::db::now_ts(),
            items,
        }
    }

    /// 解析导入的文件。**宁严勿松**：宽进来一个坏文件，用户会以为"快照坏了"。
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: SnapFile =
            serde_json::from_str(text).map_err(|e| format!("这不是快照文件（解析失败：{e}）"))?;
        if v.app != "AssetManager" || v.kind != "snapshot" {
            return Err("这不是 AssetManager 的快照文件".into());
        }
        if v.version > SNAP_VERSION {
            return Err(format!(
                "这份快照是更新版本的软件导出的（v{}），请先升级后再导入",
                v.version
            ));
        }
        if v.name.trim().is_empty() {
            return Err("快照没有名字".into());
        }
        Ok(v)
    }
}

/* ------------------------------------------------------------------ 哈希工具 */

pub fn hex(hash: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(hash.len() * 2);
    for b in hash {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() != 32 || b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(16);
    for i in (0..b.len()).step_by(2) {
        let h = (b[i] as char).to_digit(16)?;
        let l = (b[i + 1] as char).to_digit(16)?;
        out.push((h * 16 + l) as u8);
    }
    Some(out)
}

/// 算一张文件的内容哈希（大文件流式读，不整块进内存）
pub fn hash_path(path: &Path) -> Result<Vec<u8>, String> {
    dupes::hash_file(path).map_err(|e| format!("读不了 {}：{e}", path.display()))
}

/* ------------------------------------------------------------------ zip 打包 */

/// 生成一个 zip 的「条目名」：同名加 ` (2)`，避免一个包里两个同名文件。
fn entry_name(used: &mut std::collections::HashSet<String>, path: &Path) -> String {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".into());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((a, b)) if !a.is_empty() => (a.to_string(), format!(".{b}")),
        _ => (name.clone(), String::new()),
    };
    let mut cand = name.clone();
    let mut n = 2;
    while used.contains(&cand.to_ascii_lowercase()) {
        cand = format!("{stem} ({n}){ext}");
        n += 1;
    }
    used.insert(cand.to_ascii_lowercase());
    cand
}

/// 导出的 zip：**stored（不压缩）**。
/// 素材基本是 PNG/JPG/WebP，本身已经压过，deflate 再压通常只省 0–2%，
/// 却要把几百 MB 重新过一遍 CPU —— 为了那点体积不值得，用户要的是"打包成一个文件"。
pub struct ZipWriter {
    f: std::fs::File,
    /// 中心目录要攒到最后写
    central: Vec<u8>,
    count: u64,
    offset: u64,
}

impl ZipWriter {
    pub fn create(path: &Path) -> Result<Self, String> {
        let f = std::fs::File::create(path)
            .map_err(|e| format!("建不了 {}：{e}", path.display()))?;
        Ok(Self { f, central: Vec::new(), count: 0, offset: 0 })
    }

    /// 塞一个磁盘文件进去（单趟读：先写头占位，边读边算 CRC，最后补 data descriptor）
    pub fn add_file(&mut self, file: &Path, name: &str) -> Result<u64, String> {
        let meta = std::fs::metadata(file).map_err(|e| format!("读不了 {}：{e}", file.display()))?;
        let mut src =
            std::fs::File::open(file).map_err(|e| format!("打不开 {}：{e}", file.display()))?;

        let (dt, dd) = dos_datetime(meta_time(&meta));
        let name_b = name.as_bytes();
        let start = self.offset;

        // 本地头：crc/尺寸先写 0，靠 bit 3 + data descriptor 收尾（标准做法）
        let mut h = Vec::with_capacity(30 + name_b.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes()); // version needed
        h.extend_from_slice(&0x0808u16.to_le_bytes()); // UTF-8 + data descriptor
        h.extend_from_slice(&0u16.to_le_bytes()); // stored
        h.extend_from_slice(&dt.to_le_bytes());
        h.extend_from_slice(&dd.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes()); // crc32
        h.extend_from_slice(&0u32.to_le_bytes()); // comp size
        h.extend_from_slice(&0u32.to_le_bytes()); // uncomp size
        h.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // extra
        h.extend_from_slice(name_b);
        self.f.write_all(&h).map_err(|e| e.to_string())?;
        self.offset += h.len() as u64;

        // 数据 + 边读边算 CRC
        let mut crc = Crc32::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = src.read(&mut buf).map_err(|e| format!("读 {} 失败：{e}", file.display()))?;
            if n == 0 {
                break;
            }
            crc.update(&buf[..n]);
            self.f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            written += n as u64;
        }
        self.offset += written;

        // data descriptor
        let mut tail = Vec::with_capacity(16);
        tail.extend_from_slice(&0x0807_4b50u32.to_le_bytes());
        tail.extend_from_slice(&crc.finish().to_le_bytes());
        tail.extend_from_slice(&(written as u32).to_le_bytes());
        tail.extend_from_slice(&(written as u32).to_le_bytes());
        self.f.write_all(&tail).map_err(|e| e.to_string())?;
        self.offset += tail.len() as u64;

        // 中心目录条目
        let mut c = Vec::with_capacity(46 + name_b.len());
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes()); // made by
        c.extend_from_slice(&20u16.to_le_bytes()); // needed
        c.extend_from_slice(&0x0808u16.to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes());
        c.extend_from_slice(&dt.to_le_bytes());
        c.extend_from_slice(&dd.to_le_bytes());
        c.extend_from_slice(&crc.finish().to_le_bytes());
        c.extend_from_slice(&(written as u32).to_le_bytes());
        c.extend_from_slice(&(written as u32).to_le_bytes());
        c.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes()); // extra
        c.extend_from_slice(&0u16.to_le_bytes()); // comment
        c.extend_from_slice(&0u16.to_le_bytes()); // disk
        c.extend_from_slice(&0u16.to_le_bytes()); // int attr
        c.extend_from_slice(&0u32.to_le_bytes()); // ext attr
        c.extend_from_slice(&(start as u32).to_le_bytes());
        c.extend_from_slice(name_b);
        self.central.extend_from_slice(&c);
        self.count += 1;
        Ok(written)
    }

    pub fn finish(mut self) -> Result<u64, String> {
        let cd_start = self.offset;
        let cd = std::mem::take(&mut self.central);
        self.f.write_all(&cd).map_err(|e| e.to_string())?;
        let mut e = Vec::with_capacity(22);
        e.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        e.extend_from_slice(&(self.count as u16).to_le_bytes());
        e.extend_from_slice(&(self.count as u16).to_le_bytes());
        e.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        e.extend_from_slice(&(cd_start as u32).to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        self.f.write_all(&e).map_err(|e| e.to_string())?;
        self.f.flush().map_err(|e| e.to_string())?;
        Ok(cd_start + cd.len() as u64 + e.len() as u64)
    }
}

struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Self(0xffff_ffff)
    }
    fn update(&mut self, data: &[u8]) {
        static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
        let t = TABLE.get_or_init(|| {
            let mut t = [0u32; 256];
            for (i, slot) in t.iter_mut().enumerate() {
                let mut c = i as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
                }
                *slot = c;
            }
            t
        });
        let mut c = self.0;
        for &b in data {
            c = t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
        }
        self.0 = c;
    }
    fn finish(&self) -> u32 {
        self.0 ^ 0xffff_ffff
    }
}

fn meta_time(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// unix 秒 → DOS (time, date)。用 UTC，误差只是时区偏移，不影响可读性。
fn dos_datetime(ts: i64) -> (u16, u16) {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = (secs / 3600) as u16;
    let mm = ((secs % 3600) / 60) as u16;
    let ss = (secs % 60) as u16;
    let date = (((y - 1980).clamp(0, 127) as u16) << 9) | ((m as u16) << 5) | d as u16;
    let time = (hh << 11) | (mm << 5) | (ss / 2);
    (time, date)
}

/// Howard Hinnant 的 civil_from_days（1970-01-01 起的天数 → 年月日）
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 把一批磁盘文件打成一个 zip。返回 `(成功条数, 失败清单)`
pub fn zip_files(
    zip_path: &Path,
    files: &[PathBuf],
) -> Result<(usize, Vec<String>), String> {
    let mut z = ZipWriter::create(zip_path)?;
    let mut used = std::collections::HashSet::new();
    let mut ok = 0usize;
    let mut bad: Vec<String> = Vec::new();
    for f in files {
        if !f.is_file() {
            bad.push(format!("{}（文件不在了）", f.display()));
            continue;
        }
        let name = entry_name(&mut used, f);
        match z.add_file(f, &name) {
            Ok(_) => ok += 1,
            Err(e) => bad.push(format!("{}：{e}", f.display())),
        }
    }
    z.finish()?;
    Ok((ok, bad))
}

/// 导出文件默认名里不能有 `\ / : * ? " < > |`
pub fn safe_file_stem(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    out = out.trim().trim_end_matches('.').to_string();
    if out.is_empty() {
        out = "快照".into();
    }
    out.chars().take(60).collect()
}

/// 用来给"同一条快照重复加入"做幂等：库里同一张图可能有多个副本，
/// 快照只保留一条（按哈希），所以直接哈希去重。
pub fn dedupe_items(items: &mut Vec<SnapItem>) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|it| seen.insert(it.hash.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_validate() {
        let b = vec![0x00, 0x12, 0xab, 0xff, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let s = hex(&b);
        assert_eq!(s.len(), 32);
        assert_eq!(unhex(&s).unwrap(), b);
        assert!(unhex("zz").is_none());
        assert!(unhex(&s[..30]).is_none());
    }

    #[test]
    fn zip_is_readable_by_scripts() {
        let d = std::env::temp_dir().join(format!("am-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("一.png");
        let b = d.join("二.png");
        std::fs::write(&a, b"hello zip").unwrap();
        std::fs::write(&b, b"second file").unwrap();
        let zip = d.join("out.zip");
        let (ok, bad) = zip_files(&zip, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(ok, 2);
        assert!(bad.is_empty());
        let bytes = std::fs::read(&zip).unwrap();
        // 本地头 + 中心目录 + EOCD 三个签名都在
        assert!(bytes.windows(4).any(|w| w == 0x0403_4b50u32.to_le_bytes()));
        assert!(bytes.windows(4).any(|w| w == 0x0201_4b50u32.to_le_bytes()));
        assert!(bytes.windows(4).any(|w| w == 0x0605_4b50u32.to_le_bytes()));
        // 内容原样在里面（stored，不压缩）
        assert!(bytes.windows(9).any(|w| w == b"hello zip"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn snap_file_parse_rejects_foreign_json() {
        assert!(SnapFile::parse(r#"{"app":"Other","kind":"snapshot","version":1,"name":"x","items":[]}"#).is_err());
        assert!(SnapFile::parse(r#"{"app":"AssetManager","kind":"tag","version":1,"name":"x","items":[]}"#).is_err());
        assert!(SnapFile::parse(
            r#"{"app":"AssetManager","kind":"snapshot","version":99,"name":"x","items":[]}"#
        )
        .is_err());
        let ok = SnapFile::parse(
            r#"{"app":"AssetManager","kind":"snapshot","version":1,"name":"参考","items":[{"hash":"00112233445566778899aabbccddeeff","name":"a.png","size":1}]}"#,
        )
        .unwrap();
        assert_eq!(ok.name, "参考");
        assert_eq!(ok.items.len(), 1);
    }

    #[test]
    fn dos_time_is_plausible() {
        let (t, d) = dos_datetime(1_763_000_000);
        assert!(t > 0 && d > 0);
        let (_, m, day) = civil_from_days(1_763_000_000 / 86_400);
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&day));
    }

    #[test]
    fn entry_names_do_not_collide() {
        let mut used = std::collections::HashSet::new();
        assert_eq!(entry_name(&mut used, Path::new("a/b/photo.png")), "photo.png");
        assert_eq!(entry_name(&mut used, Path::new("c/PHOTO.png")), "PHOTO (2).png");
        assert_eq!(entry_name(&mut used, Path::new("d/photo.png")), "photo (3).png");
    }

    #[test]
    fn safe_stem_strips_windows_reserved() {
        assert_eq!(safe_file_stem("a/b:c*?"), "a_b_c__");
        assert_eq!(safe_file_stem("   "), "快照");
    }
}
