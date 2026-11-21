//! 扫描器：广度优先、可中断、按目录对账入库。
//!
//! 关键点（MVP 方案 §3.6 / §4.4）：
//! - 只读元数据 + 文件头拿宽高，**绝不解码像素**
//! - 每个目录一个事务、一批提交
//! - 跳过 junction / symlink 回环，防无限递归
//! - 超长路径加 `\\?\` 前缀

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::Connection;

use crate::db::{self, Res};
use crate::formats;
use crate::imageinfo;
use crate::model::*;

#[derive(Debug, Clone)]
pub struct RawEntry {
    pub name: String,
    pub ext: String,
    pub kind: i32,
    pub size: i64,
    pub mtime: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

pub struct ScanOptions {
    pub max_dirs: usize,
    pub max_depth: usize,
    /// 拿宽高（读文件头）。关掉可让首扫更快。
    pub read_dims: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { max_dirs: 2_000_000, max_depth: 64, read_dims: true }
    }
}

/// Windows 超长路径：超过 240 字符加 `\\?\`（`std::fs` 才认）。
pub fn wide(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if s.len() > 240 && !s.starts_with(r"\\?\") {
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
            };
            let raw = abs.to_string_lossy().replace('/', "\\");
            return PathBuf::from(format!(r"\\?\{}", raw));
        }
    }
    p.to_path_buf()
}

#[cfg(windows)]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(_meta: &std::fs::Metadata) -> bool {
    false
}

/// 「库中库」：别的软件的私有素材库目录，里面的文件不是用户的素材。
///
/// 目前只认一种，但它是真事（2026-11-16 用户库）：第三方素材软件导出的 `<名字>.library` 包，
/// 里面是 `metadata.json` + `images/` + `thumbs/` + `mtime.json`，一个库动辄几万个
/// 内部文件（缩略图 + 元数据 JSON）。
///
/// 为什么不索引：
/// 1. 用户要的是「我的素材」，不是别的软件的缓存；把 7 万个 json/缩略图塞进列表，
///    首屏按「最近修改」排出来的全是 json，看起来像"缩略图全挂了"；
/// 2. 这些文件删了重建、改了没意义，索引它们纯属噪音，还把扫描时间拉长 3 倍。
///
/// 判定用**内容特征**而不是只看名字：必须叫 `*.library` **且** 里面有 `metadata.json`。
/// 只看名字会误伤用户自己起的 `xxx.library` 普通文件夹；看内容才敢跳。
/// 我们自己的元数据目录（0.3.9 起每个库根下都有一个 `.AssetManager`）。
///
/// **必须跳过**：里面是 `lib.db` / `lib.json`，扫进来的话
/// ① 用户会在素材列表里看到 `lib.db`；② 库写自己的索引会触发监听 → 反复重扫；
/// ③ 导出/移动时把库的元数据当成素材一起搬走。
fn is_meta_dir(dir: &Path) -> bool {
    dir.file_name()
        .map(|n| {
            let n = n.to_string_lossy();
            /* `_AssetManager/`（下划线）是「导出整个库」那个 zip 里放库数据的目录。
               用户把包解压回某个库里是很自然的动作（"我想把这些图导进来"），
               不跳过的话库里会凭空多出 `manifest.json` / `index.db` 两条素材 */
            crate::libhome::is_home_dir(&n) || n.eq_ignore_ascii_case(crate::LIB_META_DIR)
        })
        .unwrap_or(false)
}

/// 库根上的 `desktop.ini` 是**我们写的文件夹图标配置**（见 `libicon`），不是素材。
///
/// 不跳过它的后果很具体：桌面配置会以 `KIND_OTHER` 的身份进素材列表，
/// 用户库里凭空多出一行 `desktop.ini`，而且删了还会被我们重新写回来。
/// 只认这个确切的文件名（Windows 对文件名大小写不敏感，比较时也忽略大小写）。
fn is_icon_ini(file: &Path) -> bool {
    file.file_name()
        .map(|n| n.to_string_lossy().eq_ignore_ascii_case("desktop.ini"))
        .unwrap_or(false)
}

fn is_foreign_library(dir: &Path) -> bool {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    /* 别的素材库（Eagle 那种）或者**另一个 AssetManager 库**：都不许往下扫。
       我们的库根里一定有 `.AssetManager/lib.db` —— 嵌套库时按这条识别。 */
    if crate::libhome::has_db(&dir.to_string_lossy()) {
        return true;
    }
    if !name.ends_with(".library") {
        return false;
    }
    dir.join("metadata.json").is_file()
}

fn entry_of(path: &Path, meta: &std::fs::Metadata, read_dims: bool) -> RawEntry {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = formats::lower_ext(&name);
    let kind = formats::kind_of(&ext);
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (width, height) = if read_dims && kind == KIND_IMAGE {
        imageinfo::dimensions(path)
            .map(|(w, h)| (Some(w as i64), Some(h as i64)))
            .unwrap_or((None, None))
    } else {
        (None, None)
    };
    RawEntry { name, ext, kind, size: meta.len() as i64, mtime, width, height }
}

/// 扫描一个根目录（或单个目录），返回统计。
pub fn scan_root<F>(
    conn: &mut Connection,
    root: &str,
    opts: &ScanOptions,
    cancel: &AtomicBool,
    mut progress: F,
) -> Res<ScanStats>
where
    F: FnMut(&ScanProgress),
{
    let t0 = std::time::Instant::now();
    let mut stats = ScanStats::default();
    let root_path = wide(Path::new(root));
    if !root_path.is_dir() {
        return Err(format!("不是目录：{}", root));
    }

    let root_mtime = std::fs::metadata(&root_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let root_id = db::upsert_dir(&tx, root, None, root_mtime)?;
    db::add_lib(&tx, root)?;
    tx.commit().map_err(|e| e.to_string())?;

    // 广度优先：先让用户看到结构
    let mut queue: std::collections::VecDeque<(PathBuf, i64, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((root_path.clone(), root_id, 0));
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut last_report = std::time::Instant::now();

    while let Some((dir, dir_id, depth)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) || stats.dirs as usize > opts.max_dirs {
            break;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => {
                stats.errors += 1;
                continue;
            }
        };

        let mut files: Vec<RawEntry> = Vec::new();
        let mut subdirs: Vec<(PathBuf, i64)> = Vec::new();

        for ent in rd.flatten() {
            let path = ent.path();
            let meta = match ent.metadata() {
                Ok(m) => m,
                Err(_) => {
                    stats.errors += 1;
                    continue;
                }
            };
            if meta.is_dir() {
                if is_reparse(&meta) || depth + 1 > opts.max_depth {
                    continue;
                }
                if is_meta_dir(&path) {
                    stats.skipped += 1;
                    continue;
                }
                if is_foreign_library(&path) {
                    stats.skipped += 1;
                    continue;
                }
                // 用 canonicalize 解掉 junction / symlink 后判重，防止无限递归
                if let Ok(real) = std::fs::canonicalize(&path) {
                    if !visited.insert(real) {
                        continue;
                    }
                }
                let dm = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                subdirs.push((path, dm));
            } else if meta.is_file() {
                /* 库图标配置不是素材（见 `is_icon_ini`） */
                if is_icon_ini(&path) {
                    stats.skipped += 1;
                    continue;
                }
                files.push(entry_of(&path, &meta, opts.read_dims));
                if formats::kind_of(&formats::lower_ext(
                    &path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                )) == KIND_IMAGE
                {
                    stats.images += 1;
                }
            }
        }

        // 一个目录一个事务
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let st = db::sync_dir(&tx, dir_id, &files)?;
        for (p, dm) in &subdirs {
            let ps = p.to_string_lossy().to_string();
            let cid = db::upsert_dir(&tx, &ps, Some(dir_id), *dm)?;
            queue.push_back((p.clone(), cid, depth + 1));
        }
        stats.with_dims += files.iter().filter(|f| f.width.is_some()).count() as i64;
        tx.commit().map_err(|e| e.to_string())?;

        stats.dirs += 1;
        stats.files += (st.inserted + st.updated) as i64;
        let _ = st;

        // 进度按时间报（每 120ms 一次）：按目录数报的话，一个大目录卡 3 秒、界面就静止 3 秒
        if last_report.elapsed().as_millis() >= 120 {
            last_report = std::time::Instant::now();
            progress(&ScanProgress {
                dirs: stats.dirs,
                files: stats.files,
                current: dir.to_string_lossy().to_string(),
                done: false,
            });
        }
    }

    stats.elapsed_ms = t0.elapsed().as_millis();
    progress(&ScanProgress {
        dirs: stats.dirs,
        files: stats.files,
        current: String::new(),
        done: true,
    });
    Ok(stats)
}

/// 单个目录（不需要递归的轻量刷新）。
pub fn scan_one_dir(conn: &mut Connection, dir_path: &str) -> Res<ScanStats> {
    let t0 = std::time::Instant::now();
    let mut stats = ScanStats::default();
    let p = wide(Path::new(dir_path));
    // mtime 必须写真实值：启动增量扫描就是拿它比对「这个目录动过没有」，
    // 写 0 等于以后每次启动都判定「变了」，白扫一遍。
    let dm = std::fs::metadata(&p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let dir_id = db::upsert_dir(&tx, dir_path, None, dm)?;
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&p) {
        for ent in rd.flatten() {
            if let Ok(meta) = ent.metadata() {
                if meta.is_file() {
                    /* 库图标配置不是素材（两条扫描路径都要一致，否则导入后它会冒出来） */
                    if is_icon_ini(&ent.path()) {
                        stats.skipped += 1;
                        continue;
                    }
                    files.push(entry_of(&ent.path(), &meta, true));
                }
            }
        }
    }
    let st = db::sync_dir(&tx, dir_id, &files)?;
    tx.commit().map_err(|e| e.to_string())?;
    stats.files = (st.inserted + st.updated) as i64;
    stats.dirs = 1;
    stats.elapsed_ms = t0.elapsed().as_millis();
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-scan-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 「库中库」判定：必须**名字像 + 里面有 metadata.json** 才跳。
    /// 只看名字会误伤用户自己起的 `我的素材.library`，那种是真素材目录，不许跳。
    #[test]
    fn foreign_library_needs_both_name_and_metadata() {
        let base = tmp("foreign");
        let eagle = base.join("的VS发.library");
        std::fs::create_dir_all(eagle.join("images")).unwrap();
        std::fs::write(eagle.join("metadata.json"), b"{}").unwrap();
        assert!(is_foreign_library(&eagle), "第三方素材库的 .library 包要跳");

        // 名字像但没有 metadata.json → 普通目录，不许跳
        let lookalike = base.join("我的素材.library");
        std::fs::create_dir_all(&lookalike).unwrap();
        std::fs::write(lookalike.join("a.png"), b"x").unwrap();
        assert!(!is_foreign_library(&lookalike), "同名的普通文件夹不许跳");

        // 有 metadata.json 但名字不像 → 也不跳（可能是用户自己的工程）
        let plain = base.join("工程");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("metadata.json"), b"{}").unwrap();
        assert!(!is_foreign_library(&plain), "名字不像就不跳");

        // 大小写：Windows 上 `X.LIBRARY` 也得认
        let upper = base.join("X.LIBRARY");
        std::fs::create_dir_all(&upper).unwrap();
        std::fs::write(upper.join("metadata.json"), b"{}").unwrap();
        assert!(is_foreign_library(&upper), "大小写不敏感");

        let _ = std::fs::remove_dir_all(&base);
    }
}
