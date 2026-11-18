//! 每个素材库**自己的**元数据目录：`<库根>/.AssetManager/`。
//!
//! ## 为什么从「数据目录里按 id 命名」改成「跟着库走」
//!
//! 0.3.x 及以前：所有库的索引都是 `<数据目录>/libs/lib-<id>.db`，注册库记着
//! 「id → 文件名」。三件事会因此丢数据：
//!
//! 1. 用户把素材库拷到另一台机器 / 拔了移动硬盘再插回来 —— 注册库里那条 id 没了，
//!    库文件还在数据目录里飘着，界面上就是「我的标签、评分全没了」；
//! 2. 在软件里「移除素材库」再重新加回来 —— 旧实现会把库文件挪进 `trash-libs/`，
//!    重新导入时新建一个空库，历史数据留在垃圾目录里没人知道；
//! 3. 多个库的元数据挤在一个目录、靠 id 区分 —— id 一旦被复用（重建索引、换机器），
//!    A 库的分组就挂到 B 库的素材上。
//!
//! 现在：**库的全部元数据写在库自己体内**，跟着文件夹一起走。换机器、挪盘、
//! 移除再导入，只要 `.AssetManager/lib.db` 还在，标签 / 分组 / 评分 / 目录树就都还在。
//!
//! ## 目录里有什么
//!
//! ```text
//! <库根>/.AssetManager/
//!   lib.db      索引：目录树、素材、标签关联、操作日志
//!   lib.json    库标识：id / schema / 根路径 / 创建时间（重新导入时靠它认领旧 id）
//! ```
//!
//! ## 三条纪律
//!
//! 1. **库根下只允许这一个目录**：除它之外不许往库里写任何自家文件。
//! 2. 扫描、监听、导出都必须跳过它（见 `scan::is_meta_dir` / `watch` 的过滤）。
//! 3. 目录不可写（只读盘 / 权限不足）时**回退到数据目录**，不让软件打不开 ——
//!    宁可少一层「跟着走」，也不能因为一个权限问题让人用不了。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 元数据目录名。**不要改**：老用户的库里有它，改了等于全部重新导入。
pub const DIR: &str = ".AssetManager";
/// 索引文件名
pub const DB: &str = "lib.db";
/// 库标识文件名
pub const MARKER: &str = "lib.json";
/// 标记文件格式版本
pub const MARKER_SCHEMA: u32 = 1;

/// `<库根>/.AssetManager`
pub fn dir_for(root: &str) -> PathBuf {
    Path::new(root).join(DIR)
}

/// `<库根>/.AssetManager/lib.db`
pub fn db_for(root: &str) -> PathBuf {
    dir_for(root).join(DB)
}

/// `<库根>/.AssetManager/thumbs` —— 这个库自己的缩略图缓存。
///
/// 为什么缓存也要跟着库走：库拷到另一台机器/换个盘符，缩略图 key 是按
/// 「创建时间 + 大小 + 路径」算的，重扫也会变 —— 缓存留原地等于全部重建。
/// 跟着库走之后，整库搬走再打开，图还是秒出。
pub fn thumbs_for(root: &str) -> PathBuf {
    dir_for(root).join("thumbs")
}

fn marker_for(root: &str) -> PathBuf {
    dir_for(root).join(MARKER)
}

/// 库标识。写它只是为了「重新导入时认得出这是同一个库」——
/// 真正的数据在 `lib.db` 里，这个文件丢了只会导致 id 变、不会丢内容。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Marker {
    /// 标记文件格式版本（不是库的 schema 版本）
    pub schema: u32,
    /// 曾经的库 id。注册库丢了、或换机器重导时优先复用它，
    /// 否则快照里那些 `lib<<40 | local` 的全局 id 会全部对不上。
    pub id: i64,
    /// 建库时的根路径（只作排查线索，重新导入时以用户实际选的目录为准）
    pub root: String,
    pub created_at: i64,
    /// 写这个文件的软件版本，方便以后排查「哪个版本开始有的」
    pub app: String,
}

impl Default for Marker {
    fn default() -> Self {
        Self {
            schema: MARKER_SCHEMA,
            id: 0,
            root: String::new(),
            created_at: 0,
            app: String::new(),
        }
    }
}

pub fn read_marker(root: &str) -> Option<Marker> {
    let raw = std::fs::read_to_string(marker_for(root)).ok()?;
    let m: Marker = serde_json::from_str(&raw).ok()?;
    (m.schema > 0).then_some(m)
}

/// 原子写标记：先写 `.tmp` 再改名 —— 写一半断电留下坏 JSON 会让人以为库坏了。
pub fn write_marker(root: &str, id: i64, created_at: i64, app: &str) -> Result<(), String> {
    let dir = dir_for(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 {DIR} 失败：{e}"))?;
    let m = Marker {
        schema: MARKER_SCHEMA,
        id,
        root: root.to_string(),
        created_at,
        app: app.to_string(),
    };
    let body = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
    let path = marker_for(root);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("写 {MARKER} 失败：{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换 {MARKER} 失败：{e}"))?;
    Ok(())
}

/// 这个库根下已经有索引了吗
pub fn has_db(root: &str) -> bool {
    db_for(root).is_file()
}

/// 这个目录名是不是我们的元数据目录（扫描/监听按名字跳）
pub fn is_home_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(DIR)
}

/// 这条路径是不是落在某个 `.AssetManager` 里（含它下面任意深度）
pub fn is_inside_home(p: &Path) -> bool {
    p.components().any(|c| match c {
        std::path::Component::Normal(s) => s.to_string_lossy().eq_ignore_ascii_case(DIR),
        _ => false,
    })
}

/// 能不能在这里建元数据目录（真的建一次 + 写个探针再删）。
/// **不只看权限位**：网络盘 / 只读盘 / 杀软拦写，只有真写一次才知道。
pub fn probe_writable(root: &str) -> bool {
    let dir = dir_for(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let probe = dir.join(".probe");
    let ok = std::fs::write(&probe, b"1").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-home-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn marker_roundtrip() {
        let d = tmp("marker");
        let root = d.to_string_lossy().to_string();
        write_marker(&root, 7, 123, "0.3.9").unwrap();
        let m = read_marker(&root).unwrap();
        assert_eq!(m.id, 7);
        assert_eq!(m.created_at, 123);
        assert_eq!(m.app, "0.3.9");
        // 标记文件必须在 .AssetManager 里，库根下不能多出东西来
        let names: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec![DIR.to_string()], "库根下只允许有 {DIR}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_marker_is_ignored() {
        let d = tmp("badmarker");
        let root = d.to_string_lossy().to_string();
        std::fs::create_dir_all(dir_for(&root)).unwrap();
        std::fs::write(marker_for(&root), b"{ this is not json").unwrap();
        assert!(read_marker(&root).is_none(), "坏 JSON 必须当成没有，而不是 panic");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn detects_home_paths() {
        assert!(is_home_dir(".AssetManager"));
        assert!(is_home_dir(".assetmanager"));
        assert!(!is_home_dir("AssetManager"));
        assert!(is_inside_home(Path::new(r"E:\素材\.AssetManager\lib.db")));
        assert!(is_inside_home(Path::new(r"E:\素材\.assetmanager\lib.db-wal")));
        assert!(!is_inside_home(Path::new(r"E:\素材\图\a.png")));
    }

    #[test]
    fn probe_reports_unwritable() {
        let d = tmp("ro");
        let root = d.to_string_lossy().to_string();
        assert!(probe_writable(&root));
        assert!(!has_db(&root), "探针只碰 .probe，不该造出 lib.db");
        let _ = std::fs::remove_dir_all(&d);
    }
}
