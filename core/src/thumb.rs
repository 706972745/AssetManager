//! 缩略图管线：取源（原图解码 / 内嵌预览）→ 缩放 → 编码 **WebP** → 落缓存 → LRU 记账。
//!
//! 单格式纪律（2026-11-16 owner 定）：缩略图**只做一种格式 WebP**，只用来在软件里看图，
//! 任何"复制 / 拖出 / 打开"出去的都必须是磁盘原文件（见 WORKLOG §1.5）。
//! 选 WebP 而不是 JPEG：同观感下体积小 25–35%（缓存与 WebView 磁盘缓存一起省），
//! 而解码内存由**像素数**决定、和容器格式无关，所以档位（256/384/512/1024）才是内存闸门。
//!
//! 三条硬规则（MVP 方案 §6.3）：
//! - R1 取源 = min(档位, 原图长边)，**永不放大**
//! - R2 永远从**原图**解码，绝不拿低档缩略图当高档的源
//! - 缓存 key 用 `(volume_serial, file_id, mtime, size, px)`，
//!   移动/重命名后 key 不变 —— 否则整理一次文件夹，缩略图全废。

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;
use xxhash_rust::xxh3::xxh3_128;

use crate::db::{self, Res};

/// 缩略图统一格式：WebP（q=80）。改这里就要同步 `src-tauri` 的 content-type 与界面文案。
pub const THUMB_EXT: &str = "webp";
const WEBP_QUALITY: f32 = 80.0;
/// 解码后像素上限：一张 80 MP 的图 ≈ 320 MB RGB，再高就是进程自杀。
const MAX_PIXELS: u64 = 80_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// 自己解码，质量可控
    High = 1,
    /// 系统/嵌入预览，质量不可控
    Low = 2,
}

/// 这张缩略图的像素**从哪来**（详情面板要如实写，不能糊弄成"本地解码"）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSource {
    /// `image` crate 解的原图
    Native,
    /// RAW / PSD 文件里抠出来的内嵌 JPEG 预览
    Embedded,
    /// Windows 缩略图提供者（HEIC / AI / PDF / 视频…）
    Shell,
}

impl ThumbSource {
    pub fn quality(self) -> Quality {
        match self {
            ThumbSource::Native => Quality::High,
            _ => Quality::Low,
        }
    }
}

pub struct ThumbOut {
    pub key: String,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// 主色板（`rrggbb,…`）。**只在本次真的解码了原图时才有**；
    /// 命中缓存时是空串 —— 别拿空串去覆盖库里已经算好的值。
    pub palette: String,
    /// 色桶（red / …）。同上，只在解码时算。
    pub color: String,
    /// 8×8 aHash（去重用的相似指纹）。0 = 本次没算（命中缓存）
    pub phash: i64,
    pub bytes: i64,
    pub quality: i32,
    /// 原图分辨率低于目标档位（界面要如实提示）
    pub upscaled: bool,
}

pub struct ThumbStore {
    pub root: PathBuf,
}

impl ThumbStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 缓存 key：优先用文件系统身份（移动不变），退化才用路径。
    pub fn key_for(path: &Path, px: u32) -> String {
        let meta = std::fs::metadata(path).ok();
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

        #[cfg(windows)]
        {
            // 用「创建时间 + 大小」做身份：同盘移动/重命名不变，跨盘复制才会变。
            // 不用 file_index（std 里还是 unstable），够用且零依赖。
            if let Some(m) = &meta {
                use std::os::windows::fs::MetadataExt;
                let raw = format!("c{}:s{}:t{}:p{}", m.creation_time(), size, mtime, px);
                return hex(&xxh3_128(raw.as_bytes()).to_be_bytes());
            }
        }
        let raw = format!("path:{}:s{}:t{}:p{}", path.to_string_lossy(), size, mtime, px);
        hex(&xxh3_128(raw.as_bytes()).to_be_bytes())
    }

    /// 两级分桶，避免单目录堆百万文件。
    pub fn file_for(&self, key: &str) -> PathBuf {
        let a = &key[0..2.min(key.len())];
        let b = if key.len() >= 4 { &key[2..4] } else { "00" };
        self.root.join(a).join(b).join(format!("{}.{}", key, THUMB_EXT))
    }

    pub fn exists(&self, key: &str) -> bool {
        self.file_for(key).is_file()
    }

    /// 生成（或复用）缩略图。
    pub fn ensure(&self, path: &Path, px: u32) -> Result<ThumbOut, String> {
        let key = Self::key_for(path, px);
        let out = self.file_for(&key);
        if out.is_file() {
            let (w, h) = image::ImageReader::open(&out)
                .ok()
                .and_then(|r| r.into_dimensions().ok())
                .unwrap_or((px, px));
            let bytes = std::fs::metadata(&out).map(|m| m.len() as i64).unwrap_or(0);
            return Ok(ThumbOut {
                key,
                path: out,
                width: w,
                height: h,
                bytes,
                palette: String::new(),
                color: String::new(),
                phash: 0,
                quality: Quality::High as i32,
                upscaled: false,
            });
        }

        // 旧版留的是 .jpg：同一 key 只留一份，顺手清掉（不清也不会被引用，只会白占盘）
        let legacy = out.with_extension("jpg");
        if legacy.is_file() {
            let _ = std::fs::remove_file(&legacy);
        }

        let (rgb, src) = load_source(path, px)?;
        let (ow, oh) = (rgb.width(), rgb.height());
        let longest = ow.max(oh).max(1);
        // R1：不放大。原图（或嵌入预览）本身就比档位小 → 标记 upscaled，界面如实说
        let upscaled = longest < px * 3 / 4;
        let target = px.min(longest).max(1);
        let scale = target as f32 / longest as f32;
        let nw = ((ow as f32 * scale).round() as u32).max(1);
        let nh = ((oh as f32 * scale).round() as u32).max(1);
        let thumb: image::RgbImage = if longest <= target {
            rgb
        } else {
            // Triangle（双线性）而不是盒式采样：WebP 省下来的 30–40% 体积换成清晰度，
            // 速度仍比 Lanczos3 快一档 —— 缩略图要「小、快、看得清」三者兼顾。
            image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Triangle)
        };

        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // WebP 编码（libwebp 自带，纯 C 静态链接，不额外要 cmake）
        let mem = webp::Encoder::from_rgb(thumb.as_raw(), thumb.width(), thumb.height())
            .encode(WEBP_QUALITY);
        // 原子写：先写 .tmp 再改名，避免半张图被读到
        let tmp = out.with_extension(format!("{}.tmp", THUMB_EXT));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            f.write_all(&mem).map_err(|e| e.to_string())?;
            f.flush().map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;

        let bytes = std::fs::metadata(&out).map(|m| m.len() as i64).unwrap_or(0);
        /* 顺手算主色板与相似指纹（T24 / T25）。
           为什么在这里：图刚解码完、已经在内存里，多花不到 1ms；
           另起一遍扫描去解全库，那是几十分钟的事。
           用**缩略图**而不是原图算：色板和 aHash 都只需要"大致长什么样"，
           1024 档和原图算出来的结果对用户没有区别。 */
        let (tw, th) = (thumb.width(), thumb.height());
        let pal = crate::palette::compute(thumb.as_raw(), tw, th);
        let phash = crate::dupes::ahash64(thumb.as_raw(), tw, th);
        Ok(ThumbOut {
            key,
            path: out,
            width: tw,
            height: th,
            bytes,
            palette: pal.joined(),
            color: pal.bucket,
            phash,
            quality: src.quality() as i32,
            upscaled,
        })
    }
}

/// 取缩略图的**源像素**。
///
/// 顺序（快了不止一倍，见 `preview.rs` 的注释）：
/// 1. RAW / PSD / CR3 / RAF → 先抠文件里那张内嵌 JPEG 预览（不解原图）；
/// 2. 其余（或没抠到）→ 走 `image` 正常解码，解码前先读头拿尺寸，超 80 MP 直接放弃。
fn load_source(path: &Path, target_px: u32) -> Result<(image::RgbImage, ThumbSource), String> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = crate::formats::lower_ext(&name);

    // ① RAW / PSD：先抠内嵌预览（不解传感器像素，快一到两个数量级）
    if crate::preview::prefers_embedded(&ext) {
        if let Some(pv) = crate::preview::extract(path) {
            if let Ok(img) = image::load_from_memory(&pv.bytes) {
                return Ok((img.to_rgb8(), ThumbSource::Embedded));
            }
        }
        // 抠不到就往下走正常解码（PSD 会在这里失败，落到系统兜底）
    }

    // ② 自己解码。这里失败**不再直接报错** —— 下面还有系统兜底
    match decode_native(path, target_px) {
        Ok(img) => Ok((img, ThumbSource::Native)),
        Err(native_err) => {
            // ③ 系统缩略图兜底（T10 L3）：HEIC / AI / PDF / 视频 / 装了提供者的格式
            if let Some(rgb) = shell_source(path) {
                return Ok((rgb, ThumbSource::Shell));
            }
            Err(native_err)
        }
    }
}

/// JPEG 降采样解码：**在 DCT 域就出小图**，不解全尺寸再缩。
///
/// 为什么值得单独写一条路：`image` 0.25 的 JPEG 解码器（zune-jpeg）没有缩放选项，
/// 只能全解 —— 一张 24MP 照片解出 72 MB RGB 再缩到 384px，99% 的像素都被扔掉。
/// 而缩略图 40 张一屏，每张 250ms 就是 10 秒的等待，这就是"滑一下等很久"。
///
/// 路线：先只读文件头拿尺寸 → 算一个「刚好不小于目标」的 1/2^n 比例 →
/// 让 jpeg-decoder 在 IDCT 阶段就只算需要的块（1/8 时像素少 64 倍）。
///
/// 返回 `None` 表示「这条路没走通」，调用方回落到通用解码：
/// 原图本来就比档位小（缩了也省不了）、CMYK/16 位等 jpeg-decoder 不支持的编码。
fn decode_jpeg_scaled(bytes: &[u8], target: u32) -> Option<image::RgbImage> {
    let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    d.read_info().ok()?; // 只解文件头（几毫秒），拿尺寸用
    let info = d.info()?;
    if info.width == 0 || info.height == 0 {
        return None;
    }
    let (ow, oh) = (info.width as u32, info.height as u32);
    let longest = ow.max(oh);

    /* 比例选择：让输出长边落在 [target, 2×target) 之间。
       · 缩到比 target 还小 → 缩略图会糊（多余损失，宁可多解一点）；
       · 只缩一点点 → 省不下时间，还不如走通用路径（它还带 80MP 保护）。 */
    let mut div: u32 = 1;
    while div < 8 && longest / (div * 2) >= target {
        div *= 2;
    }
    if div == 1 {
        return None;
    }
    let want_w = (ow / div).max(1) as u16;
    let want_h = (oh / div).max(1) as u16;
    // scale() 给的是"最接近的 1/2^n 比例"，实际尺寸以 info() 为准
    d.scale(want_w, want_h).ok()?;

    let px = d.decode().ok()?;
    let out = d.info()?;
    let (w, h) = (out.width as u32, out.height as u32);
    match out.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => image::RgbImage::from_raw(w, h, px),
        jpeg_decoder::PixelFormat::L8 => {
            // 灰度图：jpeg-decoder 给的是单通道，铺成 RGB（缩略图管线统一吃 RGB）
            let mut rgb = Vec::with_capacity(px.len() * 3);
            for v in px {
                rgb.extend_from_slice(&[v, v, v]);
            }
            image::RgbImage::from_raw(w, h, rgb)
        }
        _ => None,
    }
}

/// `image` crate 本家解码。解码前先读头拿尺寸，超上限直接放弃 ——
/// 一张 12000×12000 的图解码后是 576 MB，宁可不出图也不能把进程打死。
fn decode_native(path: &Path, target_px: u32) -> Result<image::RgbImage, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    // >1 GB 的单文件基本不可能是「一张图」，直接交给系统层（省一次整读）
    if meta.len() > 1024 * 1024 * 1024 {
        return Err("文件过大，改用系统预览".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;

    /* JPEG 走降采样解码（最快的那条路）：
       DCT 域直接出 1/2、1/4、1/8 的图，不解全尺寸再缩。
       24MP 照片全解 250ms、1/8 降采样只要十几毫秒 —— 滚动时"图慢慢出来"的主因就是这一步。 */
    let ext = crate::formats::lower_ext(
        &path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
    );
    // 逃生开关：`AM_NO_JPEG_SCALE=1` 关掉降采样，用来 A/B 或排查"某张图只有降采样才错"
    let allow_scale = std::env::var_os("AM_NO_JPEG_SCALE").is_none();
    if allow_scale && matches!(ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif") {
        if let Some(img) = decode_jpeg_scaled(&bytes, target_px) {
            return Ok(img);
        }
        // 降采样失败（极少数 CMYK / 16 位）→ 落到下面的通用路径
    }
    let (ow, oh) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if ow as u64 * oh as u64 > MAX_PIXELS {
        return Err(format!("图片过大（{}×{}），改用系统预览", ow, oh));
    }
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    Ok(img.to_rgb8())
}

/// 系统缩略图 → RgbImage。`shellthumb` 给的是 RGBA（已铺白底），这里只做格式转换。
fn shell_source(path: &Path) -> Option<image::RgbImage> {
    let (rgba, w, h) = crate::shellthumb::shell_thumb(path, 512)?;
    if w == 0 || h == 0 || rgba.len() < (w as usize) * (h as usize) * 4 {
        return None;
    }
    let buf = image::RgbaImage::from_raw(w, h, rgba)?;
    Some(image::DynamicImage::ImageRgba8(buf).to_rgb8())
}

fn hex(b: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}

/// 清掉历史上的 JPEG 缓存（单格式纪律：现在只认 WebP）。
///
/// 老版本的 `.jpg` 不会被任何代码引用（`file_for` 只拼 `.webp`），留着纯占盘。
/// 走一遍 `cache/thumbs` 删掉，返回 (文件数, 字节数)。在后台线程里调，别卡首屏。
pub fn purge_legacy(store: &ThumbStore) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![store.root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let p = ent.path();
            match ent.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                _ => {
                    let is_legacy_jpg = p
                        .extension()
                        .map(|e| e.eq_ignore_ascii_case("jpg"))
                        .unwrap_or(false);
                    if is_legacy_jpg {
                        let len = ent.metadata().map(|m| m.len()).unwrap_or(0);
                        if std::fs::remove_file(&p).is_ok() {
                            files += 1;
                            bytes += len;
                        }
                    }
                }
            }
        }
    }
    (files, bytes)
}

/// 超上限时按最久未用淘汰到 `target`。
pub fn gc(conn: &Connection, store: &ThumbStore, limit_bytes: i64, target_bytes: i64) -> Res<(i64, i64)> {
    let total: i64 = conn
        .query_row("SELECT COALESCE(SUM(bytes),0) FROM thumb_lru", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if total <= limit_bytes {
        return Ok((total, 0));
    }
    let mut freed = 0i64;
    let mut cur = total;
    let mut st = conn
        .prepare("SELECT key, bytes FROM thumb_lru ORDER BY last_used ASC")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, i64)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|x| x.ok())
        .collect();
    for (key, bytes) in rows {
        if cur <= target_bytes {
            break;
        }
        let _ = std::fs::remove_file(store.file_for(&key));
        let _ = std::fs::remove_file(store.file_for(&key).with_extension("jpg"));
        conn.execute("DELETE FROM thumb_lru WHERE key=?1", rusqlite::params![key])
            .map_err(|e| e.to_string())?;
        conn.execute("UPDATE assets SET thumb_key=NULL, thumb_state=0 WHERE thumb_key=?1", rusqlite::params![key])
            .map_err(|e| e.to_string())?;
        cur -= bytes;
        freed += bytes;
    }
    Ok((cur, freed))
}

/// **纯文件系统**的缩略图准备：查缓存 / 解码 / 缩放 / 编码 / 原子落盘，全程不碰数据库。
///
/// 为什么单独暴露：`thumb://` 协议层原来调 `ensure_for_asset`，而后者需要一个 `&Connection`，
/// 于是协议处理期间必须一直握着全局 `Mutex<App>` —— 一张 36MP 的图解码 0.5 秒，这 0.5 秒里
/// 列表 / 目录树 / 详情全部排在锁后面，滚到一屏新图就是"卡住"。现在协议层先短锁取路径，
/// 再用这个函数**不持锁**慢慢解码，最后短锁回写元数据。
pub fn ensure_path(path: &Path, store: &ThumbStore, px: u32) -> Result<ThumbOut, String> {
    store.ensure(path, px)
}

/// 给一批资产补缩略图（CLI / 测试用；协议层走 `ensure_path` 以免持锁）。
pub fn ensure_for_asset(conn: &Connection, store: &ThumbStore, asset_id: i64, px: u32) -> Res<Option<ThumbOut>> {
    let a = db::asset_by_id(conn, asset_id)?.ok_or_else(|| "资产不存在".to_string())?;
    let p = Path::new(&a.path);
    if !p.is_file() {
        db::mark_thumb_state(conn, asset_id, crate::model::THUMB_FAILED)?;
        return Ok(None);
    }
    match store.ensure(p, px) {
        Ok(o) => {
            db::set_thumb(conn, asset_id, &o.key, crate::model::THUMB_OK, o.quality, o.bytes)?;
            if !o.palette.is_empty() {
                db::set_palette(conn, asset_id, &o.palette, &o.color)?;
            }
            if o.phash != 0 {
                db::set_phash(conn, asset_id, o.phash)?;
            }
            // 扫描没读出尺寸的（RAW / PSD），拿缩略图的长宽比回填 —— 同一比例，布局不再跳行
            if a.width.unwrap_or(0) == 0 {
                db::set_dims_if_missing(conn, asset_id, o.width, o.height)?;
            }
            Ok(Some(o))
        }
        Err(_) => {
            // 「不支持」只在两条路都走不通时才成立：自己解不了 **且** 系统也没给缩略图。
            // 以前只判 builtin_thumbable，HEIC/PDF 明明系统有图却被标成"不支持"。
            let unsupported = !crate::formats::builtin_thumbable(&a.ext)
                && !crate::formats::shell_thumbable(&a.ext);
            db::mark_thumb_state(
                conn,
                asset_id,
                if unsupported { crate::model::THUMB_UNSUPPORTED } else { crate::model::THUMB_FAILED },
            )?;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg_of(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)
            .unwrap();
        buf
    }

    /// JPEG 降采样：目标远小于原图时必须走「DCT 直出小图」，且输出尺寸落在合理区间。
    ///
    /// 这条测试守的是**性能契约**：如果哪天 `decode_jpeg_scaled` 静默退化成
    /// 全解再缩（返回 None → 回落通用路径），功能看不出问题，但滚动的卡顿会回来。
    #[test]
    fn jpeg_scaled_decode_actually_scales() {
        let src = jpeg_of(1024, 768);

        let out = decode_jpeg_scaled(&src, 128).expect("目标 128 < 原图 1024，应该走降采样");
        // 1/8 → 128×96；允许 jpeg-decoder 向上取整到 MCU 边界
        assert!(out.width() <= 256 && out.height() <= 256, "输出必须远小于原图，实际 {}×{}", out.width(), out.height());
        assert!(out.width() >= 96, "也不该缩过头，实际 {}", out.width());

        // 目标比原图大：没得省，交给通用路径（返回 None）
        assert!(decode_jpeg_scaled(&src, 2048).is_none(), "不能放大，应回落");
        // 目标只小一点点：也回落，别为了省 20% 冒糊的风险
        assert!(decode_jpeg_scaled(&src, 700).is_none(), "缩得不够多就回落通用路径");
    }

    /// 坏数据不许 panic（协议处理器里 panic 会把 WebView 的请求线程带崩）。
    #[test]
    fn jpeg_scaled_decode_survives_garbage() {
        assert!(decode_jpeg_scaled(b"not a jpeg at all", 256).is_none());
        assert!(decode_jpeg_scaled(&[], 256).is_none());
    }
}
