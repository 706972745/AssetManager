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

/* ------------------------------------------------------------------ 资产包清单 */

/// 资产包（`.P-AssetManager`）清单里的一条 = 一张素材，`rel` 是相对库根的路径。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackItem {
    /// 相对库根，分隔符统一 `/`（zip 的规矩，也是跨平台唯一安全的写法）
    pub rel: String,
    #[serde(default)]
    pub rating: i64,
    #[serde(default)]
    pub note: String,
    /// **只放手工标签**：`fmt:` / `orient:` 这些自动标签是实时从素材列推导的，
    /// 写进包里等于把"当时的口径"钉死，以后改规则就对不上了。
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub mtime: i64,
    #[serde(default)]
    pub w: Option<i64>,
    #[serde(default)]
    pub h: Option<i64>,
}

/// 包内第一个条目 `manifest.json` 的内容（v1）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackManifest {
    pub app: String,
    pub kind: String,
    pub version: u32,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub lib_name: String,
    pub items: Vec<PackItem>,
}

pub const PACK_VERSION: u32 = 1;

impl PackManifest {
    pub fn new(lib_name: &str, items: Vec<PackItem>) -> Self {
        Self {
            app: "AssetManager".into(),
            kind: "package".into(),
            version: PACK_VERSION,
            created: crate::db::now_ts(),
            lib_name: lib_name.into(),
            items,
        }
    }

    /// 解析清单。**宁严勿松**：认错了就会往用户的库里乱写评分/标签。
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: PackManifest = serde_json::from_str(text)
            .map_err(|e| format!("这个包的清单看不懂（解析失败：{e}）"))?;
        if v.app != "AssetManager" || v.kind != "package" {
            return Err("这个包里的清单不是 AssetManager 的资产包".into());
        }
        if v.version > PACK_VERSION {
            return Err(format!(
                "这个包是更新版本的软件导出的（v{}），请先升级后再导入",
                v.version
            ));
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

/// zip32 的硬上限：单条目尺寸、中心目录偏移、条目数都写在**固定宽度的 u32/u16** 里。
/// 超过之后 `as u32` 会**静默截断**，写出来的包当场就是坏的（读端才报错，写端一声不响）。
/// 所以这里既不写 ZIP64，也绝不允许越过这条线 —— 越界直接报错，由调用方分包。
const ZIP32_MAX_BYTES: u64 = u32::MAX as u64;
const ZIP32_MAX_ENTRIES: u64 = u16::MAX as u64;

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

    /// 已经写进这个包的字节数（含头、数据、描述符）。分包判断靠它，不要自己记。
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// 已经装了几个条目。zip32 的上限是 65535，到了就必须换包。
    pub fn entries(&self) -> u64 {
        self.count
    }

    /// 塞一个磁盘文件进去（单趟读：先写头占位，边读边算 CRC，最后补 data descriptor）
    pub fn add_file(&mut self, file: &Path, name: &str) -> Result<u64, String> {
        let meta = std::fs::metadata(file).map_err(|e| format!("读不了 {}：{e}", file.display()))?;
        let mut src =
            std::fs::File::open(file).map_err(|e| format!("打不开 {}：{e}", file.display()))?;

        let (dt, dd) = dos_datetime(meta_time(&meta));
        let name_b = name.as_bytes();
        let start = self.offset;

        /* zip32 上限：**先算再写**。写了才发现越界，盘上已经留下半张坏图 */
        let need = 30u64 + name_b.len() as u64 + meta.len() + 16;
        if self.count + 1 > ZIP32_MAX_ENTRIES {
            return Err(format!(
                "这个包已经装了 {} 个条目，到 zip 上限（{}）了。请换一个包继续。",
                self.count, ZIP32_MAX_ENTRIES
            ));
        }
        if meta.len() > ZIP32_MAX_BYTES {
            return Err(format!(
                "{} 单个文件就有 {}，超过 zip 单包 4 GB 的上限，装不进去。",
                file.display(),
                meta.len()
            ));
        }
        if start + need > ZIP32_MAX_BYTES {
            return Err(format!(
                "这个包已经写到 {}，再装 {} 就超过 4 GB 上限了。请换一个包继续。",
                start,
                file.display()
            ));
        }

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

    /// 塞一段**内存里的字节**（`manifest.json` 用它：为一个几 KB 的清单写临时文件不值得）。
    pub fn add_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<u64, String> {
        let (dt, dd) = dos_datetime(crate::db::now_ts());
        let name_b = name.as_bytes();
        let start = self.offset;
        let need = 30u64 + name_b.len() as u64 + bytes.len() as u64;
        if self.count + 1 > ZIP32_MAX_ENTRIES {
            return Err(format!(
                "这个包已经装了 {} 个条目，到 zip 上限（{}）了。请换一个包继续。",
                self.count, ZIP32_MAX_ENTRIES
            ));
        }
        if start + need > ZIP32_MAX_BYTES {
            return Err(format!(
                "这个包已经写到 {start}，再装 {name} 就超过 4 GB 上限了。请换一个包继续。"
            ));
        }
        let mut crc = Crc32::new();
        crc.update(bytes);
        let crc = crc.finish();
        let len = bytes.len() as u32;

        // 本地头：这里 CRC/尺寸直接写实数（有内容在手，不必再走 data descriptor）
        let mut h = Vec::with_capacity(30 + name_b.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&0x0800u16.to_le_bytes()); // 只有 UTF-8，无 data descriptor
        h.extend_from_slice(&0u16.to_le_bytes()); // stored
        h.extend_from_slice(&dt.to_le_bytes());
        h.extend_from_slice(&dd.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&len.to_le_bytes());
        h.extend_from_slice(&len.to_le_bytes());
        h.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // extra
        h.extend_from_slice(name_b);
        self.f.write_all(&h).map_err(|e| e.to_string())?;
        self.f.write_all(bytes).map_err(|e| e.to_string())?;
        self.offset += (h.len() + bytes.len()) as u64;

        let mut c = Vec::with_capacity(46 + name_b.len());
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes()); // made by
        c.extend_from_slice(&20u16.to_le_bytes()); // needed
        c.extend_from_slice(&0x0800u16.to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes());
        c.extend_from_slice(&dt.to_le_bytes());
        c.extend_from_slice(&dd.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&len.to_le_bytes());
        c.extend_from_slice(&len.to_le_bytes());
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
        Ok(len as u64)
    }

    pub fn finish(mut self) -> Result<u64, String> {
        let cd_start = self.offset;
        let cd = std::mem::take(&mut self.central);
        /* 收尾这三个数字也是 u32/u16：越界写进去，包的"目录页"就是错的，
           解压软件只会说"文件已损坏"，不会告诉你是哪一步撑破的 */
        if self.count > ZIP32_MAX_ENTRIES {
            return Err(format!(
                "条目数 {} 超过 zip 上限（{}），请分包。",
                self.count, ZIP32_MAX_ENTRIES
            ));
        }
        if cd_start + cd.len() as u64 > ZIP32_MAX_BYTES {
            return Err("这个包超过 4 GB 上限（zip32），请分包。".into());
        }
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

/* ------------------------------------------------------------------ zip 解包 */

/// 解一个 **store-only** 的包，返回**解出来的文件数**。
///
/// 兼容旧包：老的「导出原始图片」是 `.zip`，对这里来说就是"一个没有清单的包"。
pub fn unzip_stored(zip_path: &Path, dest_dir: &Path) -> Result<usize, String> {
    unzip_package(zip_path, dest_dir).map(|(n, _)| n)
}

/// 资产包清单的文件名（**只认顶层**；解压时不为它落盘）
pub const PACK_MANIFEST: &str = "manifest.json";

/// 解包，并把顶层的 `manifest.json` 读出来交给调用方（**不落盘**）。
///
/// 为什么清单不能落盘：落下去下次扫描会被当成"一张素材"（`.json` 也是素材格式），
/// 用户会看到库里凭空多了一个 `manifest.json` —— 那是软件自己的东西，不该出现在素材库里。
///
/// 为什么自己写 reader 而不引 `zip` crate：写端就是这里手写的 store-only writer
/// （deflate 对我们这种已经压过的图片没有意义），读端只需要认得自己写的格式 ——
/// 多引一个 crate 换不来任何东西，还要多背一份供应链。
///
/// 支持：
/// - EOCD（`PK\x05\x06`）→ 中央目录（`PK\x01\x02`）→ 逐条本地头（`PK\x03\x04`）；
/// - data descriptor（本地头 flag bit 3，尺寸/CRC 写在数据后面）—— 我们的写出端就是这么写的，
///   所以 CRC 与尺寸**一律以中央目录为准**，不信任本地头里的 0。
///
/// 拒绝：
/// - 任何非 stored 的条目（压缩方式不支持 → 这个包不是我们导出的）；
/// - **zip-slip**：条目名带 `..`、绝对路径、盘符、反斜杠、空段落一律拒绝（见 `rel_entry_path`）；
/// - 目录条目（名字以 `/` 结尾）跳过不计。
pub fn unzip_package(
    zip_path: &Path,
    dest_dir: &Path,
) -> Result<(usize, Option<String>), String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(zip_path)
        .map_err(|e| format!("打不开这个包（{e}）。它可能被移动或删除了。"))?;
    let total_len = f
        .metadata()
        .map_err(|e| format!("读不了这个包的大小（{e}）。"))?
        .len();
    if total_len < 22 {
        return Err("这个包不完整（连 zip 的结尾都没有）。请重新导出一份。".into());
    }

    /* ① EOCD：结尾固定 22 字节 + 注释（最多 65535）。从尾部往前找签名 */
    let tail_len = total_len.min(66_000) as usize;
    let tail_start = total_len - tail_len as u64;
    let mut tail = vec![0u8; tail_len];
    f.seek(SeekFrom::Start(tail_start))
        .map_err(|e| format!("读包失败（{e}）。"))?;
    f.read_exact(&mut tail)
        .map_err(|e| format!("读包失败（{e}）。"))?;
    let eocd_at = find_last(&tail, &[0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| "这个包不是 AssetManager 导出的（找不到 zip 结尾标记）。".to_string())?;
    if tail.len() < eocd_at + 22 {
        return Err("这个包不完整（结尾标记后面缺字节）。请重新导出一份。".into());
    }
    let e = &tail[eocd_at..];
    let count = le16(e, 10) as usize;
    let cd_size = le32(e, 12) as u64;
    let cd_off = le32(e, 16) as u64;
    if count == 0xffff || cd_size == 0xffff_ffff || cd_off == 0xffff_ffff {
        return Err("这个包用了 zip64（超过 4 GB 或 6.5 万条）。请分几个包导出。".into());
    }
    if cd_off.saturating_add(cd_size) > total_len {
        return Err("这个包的目录区越界了（文件可能被截断）。请重新导出一份。".into());
    }

    /* ② 中央目录：一次读进来（几万条也就几百 KB） */
    let mut cd = vec![0u8; cd_size as usize];
    f.seek(SeekFrom::Start(cd_off))
        .map_err(|e| format!("读包失败（{e}）。"))?;
    f.read_exact(&mut cd)
        .map_err(|e| format!("读包失败（{e}）。"))?;

    std::fs::create_dir_all(dest_dir)
        .map_err(|e| format!("建不了目标文件夹（{e}）。请换一个位置再试。"))?;

    let mut done = 0usize;
    let mut at = 0usize;
    /* 资产包清单（顶层 manifest.json）：读进内存，不落盘 */
    let mut manifest: Option<String> = None;
    for _ in 0..count {
        if at + 46 > cd.len() || le32(&cd, at) != 0x0201_4b50 {
            return Err("这个包的目录结构不对（不是 AssetManager 导出的包）。".into());
        }
        let method = le16(&cd, at + 10);
        let crc_want = le32(&cd, at + 16);
        let comp_size = le32(&cd, at + 20) as u64;
        let name_len = le16(&cd, at + 28) as usize;
        let extra_len = le16(&cd, at + 30) as usize;
        let comment_len = le16(&cd, at + 32) as usize;
        let lho = le32(&cd, at + 42) as u64;
        let name_at = at + 46;
        if name_at + name_len > cd.len() {
            return Err("这个包的目录结构不对（条目名越界）。".into());
        }
        let raw_name = String::from_utf8_lossy(&cd[name_at..name_at + name_len]).to_string();
        at = name_at + name_len + extra_len + comment_len;

        /* 目录条目（我们写出端不产生，但别人打的包可能有）：跳过 */
        if raw_name.ends_with('/') {
            continue;
        }
        if method != 0 {
            return Err(format!(
                "这个包不是 AssetManager 导出的（「{raw_name}」用了压缩，我们只支持不压缩的包）。"
            ));
        }
        let Some(rel) = rel_entry_path(&raw_name) else {
            return Err(format!(
                "这个包里有可疑路径（{raw_name}），为安全起见不导入。它可能不是我们导出的包。"
            ));
        };

        /* ③ 本地头：只为了跳过它自带的文件名/扩展区，拿到数据起点 */
        if lho + 30 > total_len {
            return Err("这个包的条目越界了（文件可能被截断）。请重新导出一份。".into());
        }
        let mut lh = [0u8; 30];
        f.seek(SeekFrom::Start(lho))
            .map_err(|e| format!("读包失败（{e}）。"))?;
        f.read_exact(&mut lh)
            .map_err(|e| format!("读包失败（{e}）。"))?;
        if le32(&lh, 0) != 0x0403_4b50 {
            return Err("这个包的条目结构不对（不是 AssetManager 导出的包）。".into());
        }
        let l_name = le16(&lh, 26) as u64;
        let l_extra = le16(&lh, 28) as u64;
        let data_at = lho + 30 + l_name + l_extra;
        if data_at.saturating_add(comp_size) > total_len {
            return Err("这个包的条目越界了（文件可能被截断）。请重新导出一份。".into());
        }

        /* ④ 清单条目：读进内存就跳过（落盘的话会被当成一张素材入索引） */
        if raw_name == PACK_MANIFEST {
            f.seek(SeekFrom::Start(data_at))
                .map_err(|e| format!("读包失败（{e}）。"))?;
            let mut buf = vec![0u8; comp_size as usize];
            f.read_exact(&mut buf)
                .map_err(|e| format!("读包失败（{e}）。"))?;
            manifest = Some(String::from_utf8_lossy(&buf).to_string());
            continue;
        }

        /* ④ 落盘：先写临时名，校验完 CRC 再改成正式名 —— 中途失败不会留下半张图 */
        let target = dest_dir.join(&rel);
        /* 双保险：名字规则已经挡掉了 `..` / 绝对路径 / 盘符，这里再比一次前缀
           （万一以后有人放宽了 `rel_entry_path`，逃逸会在这一步被抓住） */
        if !target.starts_with(dest_dir) {
            return Err(format!(
                "这个包里有可疑路径（{raw_name}），为安全起见不导入。"
            ));
        }
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("建不了子文件夹 {}（{e}）。", dir.display()))?;
        }
        let part = {
            let fname = target
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "entry".into());
            target.with_file_name(format!("{fname}.part"))
        };
        f.seek(SeekFrom::Start(data_at))
            .map_err(|e| format!("读包失败（{e}）。"))?;
        let mut out = std::fs::File::create(&part)
            .map_err(|e| format!("写不了 {}（{e}）。请换一个位置或腾点空间。", target.display()))?;
        let mut crc = Crc32::new();
        let mut left = comp_size;
        let mut buf = vec![0u8; 256 * 1024];
        while left > 0 {
            let want = buf.len().min(left as usize);
            let n = f
                .read(&mut buf[..want])
                .map_err(|e| format!("读包失败（{e}）。"))?;
            if n == 0 {
                let _ = std::fs::remove_file(&part);
                return Err("这个包在解压途中结束了（文件被截断）。请重新导出一份。".into());
            }
            crc.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(|e| format!("写不了 {}（{e}）。", target.display()))?;
            left -= n as u64;
        }
        out.flush().map_err(|e| format!("写不了 {}（{e}）。", target.display()))?;
        drop(out);
        if crc.finish() != crc_want {
            let _ = std::fs::remove_file(&part);
            return Err(format!(
                "「{}」解出来的内容和校验值对不上（包可能损坏了）。请重新导出。",
                raw_name
            ));
        }
        std::fs::rename(&part, &target)
            .map_err(|e| format!("落不了盘 {}（{e}）。", target.display()))?;
        done += 1;
    }
    Ok((done, manifest))
}

/// 条目名 → 相对路径。**返回 `None` = 这个条目可疑，整个包都不导**（zip-slip 防护）。
///
/// 判据（任何一条命中就拒）：
/// - 空名、以 `/` 或 `\` 开头（绝对路径）；
/// - 任何 `..` 段落（逃逸到目标目录之外）；
/// - 含 `\`（Windows 上它也是分隔符，`a\..\b` 会被资源管理器当路径）；
/// - 含 `:`（`C:` 盘符、NTFS 数据流）；
/// - 段落里出现 Windows 保留字符 `<>"|?*` 或控制字符。
fn rel_entry_path(name: &str) -> Option<std::path::PathBuf> {
    if name.is_empty() || name.starts_with('/') || name.starts_with('\\') {
        return None;
    }
    if name.contains('\\') || name.contains(':') || name.contains('\0') {
        return None;
    }
    let mut out = std::path::PathBuf::new();
    for seg in name.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        if seg.chars().any(|c| c.is_control() || "<>\"|?*".contains(c)) {
            return None;
        }
        // 去掉结尾的点与空格：Windows 会把「a.」和「a」当成同一个文件，
        // 留着等于给"同名覆盖"开后门
        let trimmed = seg.trim_end_matches(['.', ' ']);
        if trimmed.is_empty() {
            return None;
        }
        out.push(trimmed);
    }
    Some(out)
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// 从后往前找子串（EOCD 可能在注释里出现假签名，所以取最后一个）
fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).rev().find(|&i| &hay[i..i + needle.len()] == needle)
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

    /// 自己打包 → 自己解包：文件数、文件名、字节内容都必须一模一样。
    #[test]
    fn unzip_roundtrip_restores_every_file() {
        let d = std::env::temp_dir().join(format!("am-unzip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let src = d.join("src");
        let dest = d.join("dest");
        std::fs::create_dir_all(&src).unwrap();
        let a = src.join("一.png");
        let b = src.join("二.jpg");
        std::fs::write(&a, b"hello zip").unwrap();
        std::fs::write(&b, vec![7u8; 40_000]).unwrap();
        let zip = d.join("pack.P-AssetManager");
        let (ok, bad) = zip_files(&zip, &[a.clone(), b.clone()]).unwrap();
        assert_eq!((ok, bad.len()), (2, 0));

        let n = unzip_stored(&zip, &dest).unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read(dest.join("一.png")).unwrap(), b"hello zip");
        assert_eq!(std::fs::read(dest.join("二.jpg")).unwrap(), vec![7u8; 40_000]);
        // 没有留下 .part 半成品
        let left: Vec<String> = std::fs::read_dir(&dest)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".part"))
            .collect();
        assert!(left.is_empty(), "残留：{left:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// zip-slip：条目名 `../evil.txt` 必须整包拒绝，且目标目录里不能出现任何东西。
    #[test]
    fn unzip_rejects_parent_escape_entry() {
        let d = std::env::temp_dir().join(format!("am-slip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // 源文件必须放在别的目录：如果直接放在 d/evil.txt，那它正好就是
        // `../evil.txt` 的逃逸目标，断言「不该存在」永远为假（这版测试原先就栽在这）
        let src = d.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let payload = src.join("evil.txt");
        std::fs::write(&payload, b"pwned").unwrap();
        let zip = d.join("evil.P-AssetManager");
        let mut z = ZipWriter::create(&zip).unwrap();
        /* 故意把条目名写成逃逸路径：写出端不校验名字（它只负责塞字节），
           校验点就在读端 —— 这才是攻击面 */
        z.add_file(&payload, "../evil.txt").unwrap();
        z.finish().unwrap();

        let dest = d.join("dest");
        let e = unzip_stored(&zip, &dest).unwrap_err();
        assert!(e.contains("可疑路径"), "{e}");
        assert!(!d.join("evil.txt").exists(), "逃出去的副本不该落地");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 不是 zip（比如把 JSON 改名成 .P-AssetManager）→ 人话报错，不是 panic。
    #[test]
    fn unzip_rejects_non_zip_file() {
        let d = std::env::temp_dir().join(format!("am-notzip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("x.P-AssetManager");
        std::fs::write(&p, br#"{"app":"AssetManager","libs":[]}"#).unwrap();
        let e = unzip_stored(&p, &d.join("dest")).unwrap_err();
        assert!(e.contains("不是 AssetManager 导出的"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn entry_path_rules_reject_traversal_and_absolute() {
        assert!(rel_entry_path("ok.png").is_some());
        assert!(rel_entry_path("sub/dir/ok.png").is_some());
        for bad in [
            "../evil.txt",
            "a/../../b.png",
            "/abs.png",
            "\\abs.png",
            "C:/x.png",
            "a\\b.png",
            "a//b.png",
            "./a.png",
            "a/",
            "",
            "a/<>|?.png",
        ] {
            assert!(rel_entry_path(bad).is_none(), "应该拒绝：{bad}");
        }
    }
}
