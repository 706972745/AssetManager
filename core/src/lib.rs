//! am-core —— AssetManager 底层核心。
//!
//! 这个 crate **不依赖 Tauri**，可以独立 `cargo test` / 跑 CLI 冒烟。
//! Tauri 壳只负责把这里的能力暴露成命令 + 自定义协议。
//!
//! # 多库（T28）
//!
//! 每个素材库一个 SQLite 文件，**放在库自己的 `.AssetManager/lib.db`**（0.3.9 起；
//! 见 `libhome`），外加一个注册库（`<数据目录>/index.db`）只记住「挂了哪几个库」。
//! 标签 / 分组 / 评分这些跟着库文件走：换机器、挪盘、移除后重新导入都不会丢。
//! 对界面暴露的 id 是 `lib << 40 | 库内 id`（见 `ids`），所以前端完全无感。
//!
//! 跨库的操作（列全部、标签计数、撤销选库）都在这里收口 —— 别让命令层自己拼。

pub mod autotag;
pub mod clipimg;
pub mod dataroot;
pub mod db;
pub mod dupes;
pub mod exif;
pub mod formats;
pub mod fsops;
pub mod ids;
pub mod imageinfo;
pub mod imgops;
pub mod keywatch;
pub mod libhome;
pub mod libs_migrate;
pub mod model;
pub mod palette;
pub mod preview;
pub mod refs;
pub mod reg;
pub mod scan;
pub mod screen;
pub mod shellthumb;
pub mod snapshot;
pub mod thumb;
pub mod trash;
pub mod watch;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub use ids::{gid, lib_of, ungid};
pub use model::*;

/// 应用数据目录（Windows：`%LOCALAPPDATA%\AssetManager`）。
pub fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join("AssetManager");
        }
    }
    std::env::temp_dir().join("AssetManager")
}

/// 一个已打开的素材库。
pub struct Lib {
    pub id: i64,
    pub root: String,
    pub conn: rusqlite::Connection,
    /// 这个库自己的缩略图缓存（`<库根>/.AssetManager/thumbs`）。
    /// 库目录不可写时退回数据目录 —— 那种情况下"缓存跟着走"这件事只能放弃，
    /// 但软件必须照常能用。
    pub thumbs: thumb::ThumbStore,
}

pub struct App {
    /// 注册库：库清单 + 标签体系 + 自动规则
    pub reg: rusqlite::Connection,
    pub libs: Vec<Lib>,
    pub data_dir: PathBuf,
    /// 可再生成缓存的根（正常在安装目录，写不进去时退回 `data_dir/cache`）。
    /// 缩略图**不在这里**：0.3.10 起每库自己一份，见 `Lib::thumbs`。
    pub cache_dir: PathBuf,
    pub thumbs: thumb::ThumbStore,
    /// 当前"打开着"的那个库：侧栏的标签体系按它取（每个库一套标签）。
    /// 切库时由 `set_current_lib` 改；`None` 时退回第一个库。
    pub current: Option<i64>,
    /// 快照 → 全局 id 的换算缓存：`快照 id → (updated_at, ids)`。
    /// 列表按快照筛时每翻一页都要这批 id，重算哈希太贵；快照一改（updated_at 变）就作废。
    snap_ids_cache: std::sync::Mutex<std::collections::HashMap<i64, (i64, Vec<i64>)>>,
}

/// 缓存目录是否真的可写：建得出来、也写得进去才算。
///
/// 只读目录（Program Files、只读介质）常常 `create_dir_all` 成功而写文件失败，
/// 所以写一个探针再删掉 —— 缓存搬不过去时宁可退回数据目录，也不能让软件起不来。
fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".am-write-probe");
    match std::fs::write(&probe, b"1") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

impl App {
    /// 打开索引（缓存默认放数据目录 —— 测试与 CLI 走这条）。
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, String> {
        Self::open_with_cache(data_dir, None)
    }

    /// 打开索引，并指定**可再生成缓存的根**（owner 2026-11-19：缓存放安装目录，别占 C 盘）。
    ///
    /// `cache` 只表示"想放这儿"：写不进去就退回 `<数据目录>/cache`，
    /// 并把老位置的 `cache/` 一次性改名过来（同盘瞬时；跨盘或被占着就留着不动）。
    pub fn open_with_cache(
        data_dir: impl AsRef<Path>,
        cache: Option<PathBuf>,
    ) -> Result<Self, String> {
        let data_dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let reg_path = data_dir.join("index.db");
        let mut reg = reg::open(&reg_path)?;

        // 0.7.x 及以前是"单库多根"：`index.db` 里既有库清单也有 assets/dirs。
        // 手上这份如果是那种库，先原地拆成一堆库文件，再继续往下走。
        libs_migrate::maybe_migrate(&mut reg, &data_dir)?;

        let old_cache = data_dir.join("cache");
        let cache_dir = match cache {
            Some(c) if writable(&c) => c,
            _ => old_cache.clone(),
        };
        /* 缓存搬家：`writable` 已经把目标目录建出来了，所以"目标是空目录"也要认，
           否则这条搬家永远走不到（老缓存白留一份）。目标里已经有东西就不动，
           避免把两份缓存混在一起。 */
        let target_empty = !cache_dir.exists()
            || (cache_dir.is_dir()
                && std::fs::read_dir(&cache_dir)
                    .map(|mut it| it.next().is_none())
                    .unwrap_or(false));
        if cache_dir != old_cache && old_cache.is_dir() && target_empty {
            let _ = std::fs::remove_dir_all(&cache_dir);
            if let Err(e) = std::fs::rename(&old_cache, &cache_dir) {
                /* 搬不动（被占用/跨盘）就先留着老池子：缓存能重建，不能因此起不来。
                   但要留一句话，否则"C 盘怎么还有 300 MB"永远查不出来。 */
                eprintln!(
                    "[AssetManager] 缓存搬家失败，先继续用老目录 {}：{}",
                    old_cache.display(),
                    e
                );
                let _ = std::fs::create_dir_all(&cache_dir);
            }
        }

        let thumbs = thumb::ThumbStore::new(cache_dir.join("thumbs"));
        std::fs::create_dir_all(&thumbs.root).map_err(|e| e.to_string())?;

        let mut app = Self {
            reg,
            libs: Vec::new(),
            data_dir,
            cache_dir,
            thumbs,
            current: None,
            snap_ids_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        };
        /* 老库文件还在数据目录里飘着 → 搬到各自库根的 `.AssetManager/lib.db`。
           必须在 reload 之前做：搬完注册行里记的路径才是新位置。 */
        app.migrate_libs_into_home();
        app.reload_libs();
        app.seed_tags_into_existing_libs();
        app.migrate_thumbs_into_libs();
        app.migrate_snapshots_into_libs();
        app.current = app.libs.first().map(|l| l.id);
        Ok(app)
    }

    /// 注册行 → 库文件的真实位置。
    ///
    /// `file` 可能是绝对路径（0.3.9 起是 `<库根>/.AssetManager/lib.db`），
    /// 也可能是相对数据目录的老路径（0.8.x 的 `libs/lib-<id>.db`）。
    /// `Path::join` 遇到绝对路径会直接返回它，所以两种情况一句话就能收口。
    /// 这个库该用哪个缩略图目录：库内 `.AssetManager/thumbs` 能写就用它，
    /// 否则退回数据目录（只读盘 / 网络盘 / 权限不足）。
    fn thumb_root_for(&self, root: &str) -> PathBuf {
        let home = libhome::thumbs_for(root);
        if std::fs::create_dir_all(&home).is_ok() {
            home
        } else {
            self.cache_dir.join("thumbs")
        }
    }

    fn lib_path(&self, row: &reg::LibRow) -> PathBuf {
        let p = Path::new(&row.file);
        if p.is_absolute() {
            p.to_path_buf()
        } else if row.file.is_empty() {
            reg::lib_file(&self.data_dir, row.id)
        } else {
            self.data_dir.join(&row.file)
        }
    }

    /// 按注册库里的清单把每个库文件打开（坏库留证后跳过，不让一个库拖垮启动）。
    pub fn reload_libs(&mut self) {
        self.libs.clear();
        let rows = reg::libs(&self.reg).unwrap_or_default();
        for row in rows {
            let path = self.lib_path(&row);
            match db::open(&path) {
                Ok(conn) => {
                    let thumbs = thumb::ThumbStore::new(self.thumb_root_for(&row.root));
                    self.libs.push(Lib { id: row.id, root: row.root, conn, thumbs });
                }
                Err(e) => eprintln!("打开库 {} 失败（已跳过）：{}", path.display(), e),
            }
        }
        /* current 指着的库被移除了（或第一次启动）→ 退回第一个 */
        if !self.current.map(|id| self.libs.iter().any(|l| l.id == id)).unwrap_or(false) {
            self.current = self.libs.first().map(|l| l.id);
        }
    }

    /// 换「当前库」。传库根或它下面任意路径都行（内部按前缀归属）。
    ///
    /// 返回**是否真的换了**：前端只在换了的时候重拉标签清单 ——
    /// 同一个库里点开子目录不该把侧栏的标签重新拉一遍（会闪）。
    pub fn set_current_lib(&mut self, path: &str) -> Result<bool, String> {
        let id = self
            .lib_for_path(path)
            .ok_or_else(|| format!("这个路径不在任何素材库里：{path}"))?;
        let changed = self.current != Some(id);
        self.current = Some(id);
        Ok(changed)
    }

    /// 当前库（`current` 没设就退第一个）。一个库都没有时返回 None。
    fn cur_lib(&self) -> Option<&Lib> {
        self.libs
            .iter()
            .find(|l| Some(l.id) == self.current)
            .or_else(|| self.libs.first())
    }

    fn lib(&self, id: i64) -> Result<&Lib, String> {
        self.libs
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| "这个素材已经不属于任何库了（可能刚被移除）".to_string())
    }

    /// 全局 id → 所在库的连接（写元数据 / 出缩略图都走这一句，
    /// 别让上层自己 `libs.iter().find(...)` —— 少一处就能少一处拿错库）
    pub fn conn_for(&self, g: i64) -> Result<&rusqlite::Connection, String> {
        let (lib_id, _) = ungid(g);
        Ok(&self.lib(lib_id)?.conn)
    }

    /// 路径 → 所在库的连接（文件操作写 `ops` 日志要用）
    pub fn conn_for_path(&self, path: &str) -> Result<&rusqlite::Connection, String> {
        let lib_id = self
            .lib_for_path(path)
            .ok_or_else(|| format!("这个路径不在任何素材库里：{path}"))?;
        Ok(&self.lib(lib_id)?.conn)
    }

    /// 出一张缩略图（**全局 id**）。字节不走 IPC，只走 `thumb://`。
    /// 这里顺手把 `thumb_key / 状态 / 尺寸回填` 写进它**自己那个库**。
    pub fn ensure_thumb(&mut self, g: i64, px: u32) -> Result<Option<thumb::ThumbOut>, String> {
        let (lib_id, local) = ungid(g);
        let l = self.lib_mut(lib_id)?;
        // 缓存是**这个库自己的**（0.3.10）：整库搬走时缩略图一起走
        let store = l.thumbs.clone();
        thumb::ensure_for_asset(&l.conn, &store, local, px)
    }

    fn lib_mut(&mut self, id: i64) -> Result<&mut Lib, String> {
        self.libs
            .iter_mut()
            .find(|l| l.id == id)
            .ok_or_else(|| "这个素材已经不属于任何库了（可能刚被移除）".to_string())
    }

    /// 路径 → 所属库（长前缀优先：嵌套库根时以更具体的那个为准）。
    pub fn lib_for_path(&self, path: &str) -> Option<i64> {
        let p = norm_path(path);
        let mut best: Option<(usize, i64)> = None;
        for l in &self.libs {
            let r = norm_path(&l.root);
            if p == r || p.starts_with(&format!("{r}\\")) || p.starts_with(&format!("{r}/")) {
                let len = r.len();
                if best.map(|(bl, _)| len > bl).unwrap_or(true) {
                    best = Some((len, l.id));
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /* ------------------------------------------------------------ 库管理 */

    /// 登记一个库根（已有就返回原 id），并把连接打开。
    ///
    /// 0.3.9 起库的索引在 `<库根>/.AssetManager/lib.db`（见 `libhome`）：
    /// · 目录可写 → 用库内那份（跟着库走，换机器/重导都还在）；
    /// · 目录不可写（只读盘、网络盘、权限不足）→ 退回数据目录，**不让软件打不开**。
    pub fn ensure_lib(&mut self, root: &str) -> Result<i64, String> {
        let root = norm_keep_case(root);
        if let Some(l) = self.libs.iter().find(|l| norm_path(&l.root) == norm_path(&root)) {
            return Ok(l.id);
        }
        let existing = reg::libs(&self.reg)?
            .into_iter()
            .find(|r| norm_path(&r.root) == norm_path(&root));

        /* ① id：已登记就用它；没登记但库里留着 lib.json（以前导入过）
           就复用旧 id —— 快照里存的是"库 id + 库内 id"，换 id 等于全断。 */
        let id = match &existing {
            Some(row) => row.id,
            None => {
                let reusable = libhome::read_marker(&root)
                    .map(|m| m.id)
                    .filter(|id| *id > 0 && !reg::lib_ids(&self.reg).unwrap_or_default().contains(id));
                match reusable {
                    Some(old) => {
                        reg::add_lib_at(&self.reg, old, &root, "")?;
                        old
                    }
                    None => reg::add_lib(&self.reg, &root, "")?,
                }
            }
        };

        /* ② 文件位置 */
        let writable = libhome::probe_writable(&root);
        let path = if writable {
            libhome::db_for(&root)
        } else if let Some(row) = &existing {
            self.lib_path(row)
        } else {
            reg::lib_file(&self.data_dir, id)
        };

        /* ③ 库里还没有索引、但数据目录里有一份（老库没搬成功）→ 补搬一次 */
        if writable && !path.exists() {
            if let Some(row) = &existing {
                let legacy = self.lib_path(row);
                if legacy.exists() {
                    if let Err(e) = copy_db(&legacy, &path) {
                        eprintln!("把库文件搬进 .AssetManager 失败（继续用老位置）：{e}");
                    }
                }
            }
        }

        let conn = db::open(&path)?;
        let thumbs = thumb::ThumbStore::new(self.thumb_root_for(&root));
        self.libs.push(Lib { id, root: root.clone(), conn, thumbs });

        /* ④ 注册行写成绝对路径：相对路径一旦库搬家就指向不存在的文件 */
        reg::add_lib(&self.reg, &root, &path.to_string_lossy())?;
        if writable {
            let _ = libhome::write_marker(&root, id, db::now_ts(), env!("CARGO_PKG_VERSION"));
        }

        /* ⑤ 老版本的标签体系在注册库里 → 第一次接进这个库时复制一份，
              之后各自独立（用户可以给不同库不同的标签体系）。 */
        if existing.is_some() {
            if let Some(l) = self.libs.last() {
                seed_tags_from_registry(&self.reg, &l.conn);
            }
        } else if let Some(l) = self.libs.last() {
            /* 新库：标记"种子已处理"，免得以后用户清空标签又被老库的标签灌回来 */
            mark_tags_seeded(&l.conn);
        }
        Ok(id)
    }

    pub fn lib_count(&self) -> Result<i64, String> {
        reg::lib_count(&self.reg)
    }

    pub fn lib_roots(&self) -> Result<Vec<String>, String> {
        reg::lib_roots(&self.reg)
    }

    /// 从素材库移除一个库根。
    ///
    /// **0.3.9 起只摘注册行，绝不动库里的 `.AssetManager`** ——
    /// 用户"移除后再导入回来"时数据必须还在（以前是把库文件挪进 `trash-libs/`，
    /// 重新导入等于建了个空库，标签评分全没，用户以为软件把数据弄丢了）。
    /// 仍然一个文件都不删：磁盘上的素材从来就不归我们管。
    pub fn remove_lib(&mut self, root: &str) -> Result<usize, String> {
        let Some(row) = reg::libs(&self.reg)?
            .into_iter()
            .find(|r| norm_path(&r.root) == norm_path(root))
        else {
            return Err("这个文件夹不在素材库里".into());
        };
        // 先数一下要清多少目录（界面要显示"清掉 N 个目录索引"）
        let n = self
            .libs
            .iter()
            .find(|l| l.id == row.id)
            .map(|l| db::dir_count(&l.conn).unwrap_or(0))
            .unwrap_or(0) as usize;

        // 先摘掉内存里的连接（文件被自己占着，后面就算要挪也挪不动）
        if let Some(pos) = self.libs.iter().position(|l| l.id == row.id) {
            self.libs.remove(pos);
        }
        if self.current == Some(row.id) {
            self.current = self.libs.first().map(|l| l.id);
        }

        let path = self.lib_path(&row);
        if !libhome::is_inside_home(&path) {
            /* 老库（元数据还在数据目录里）：按老规矩挪到 `trash-libs/` 留一手 */
            let keep = self.data_dir.join("trash-libs");
            let _ = std::fs::create_dir_all(&keep);
            for suffix in ["", "-wal", "-shm"] {
                let src = PathBuf::from(format!("{}{}", path.to_string_lossy(), suffix));
                if src.exists() {
                    let name = src
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let dst = keep.join(format!("{}-{}", db::now_ts(), name));
                    let _ = std::fs::rename(&src, dst);
                }
            }
        }
        reg::remove_lib_row(&self.reg, &row.root)?;
        Ok(n)
    }

    /// 老版本的缩略图缓存统一放在 `<数据目录>/cache/thumbs`。
    /// 0.3.10 起每个库有自己的 `.AssetManager/thumbs`，这里按各库的 `thumb_lru`
    /// （"这个库用过哪些 key"）把文件分发过去。
    ///
    /// 同一个 key 可能被两个库同时引用（内容相同）—— 第一个库**搬**，后面的**复制**，
    /// 否则第二个库会找不到文件、白重建一次。
    fn migrate_thumbs_into_libs(&mut self) {
        /* 老池子的位置有两处可能：数据目录（≤0.3.10）和安装目录（缓存搬家之后）。 */
        let mut legacy_root = self.cache_dir.join("thumbs");
        if !legacy_root.is_dir() {
            legacy_root = self.data_dir.join("cache").join("thumbs");
        }
        if !legacy_root.is_dir() {
            return;
        }
        let legacy = thumb::ThumbStore::new(legacy_root.clone());
        /* 先快照出「每个库要哪些 key」，避免边遍历 self.libs 边借用 self */
        let plan: Vec<(i64, PathBuf, Vec<String>)> = self
            .libs
            .iter()
            .map(|l| {
                let keys: Vec<String> = db::thumb_lru_rows(&l.conn)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, _, _)| k)
                    .collect();
                (l.id, l.thumbs.root.clone(), keys)
            })
            .collect();
        let homes: Vec<(i64, PathBuf)> = plan.iter().map(|(id, r, _)| (*id, r.clone())).collect();

        for (id, root, keys) in plan {
            if root == legacy_root || keys.is_empty() {
                continue; // 退回数据目录的库：本来就是同一个池子
            }
            let store = thumb::ThumbStore::new(root);
            for key in keys {
                let dst = store.file_for(&key);
                if dst.is_file() {
                    continue;
                }
                if let Some(p) = dst.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                /* 老池子里有就搬；已经被别的库搬走了，就从那个库复制一份 */
                let mut src = legacy.file_for(&key);
                if !src.is_file() {
                    let alt = src.with_extension("jpg");
                    if alt.is_file() {
                        src = alt;
                    } else if let Some((_, other)) = homes
                        .iter()
                        .find(|(oid, oroot)| *oid != id && *oroot != legacy_root && {
                            thumb::ThumbStore::new(oroot.clone()).file_for(&key).is_file()
                        })
                    {
                        src = thumb::ThumbStore::new(other.clone()).file_for(&key);
                    } else {
                        continue;
                    }
                }
                if std::fs::rename(&src, &dst).is_err() {
                    let _ = std::fs::copy(&src, &dst);
                }
            }
        }
    }

    /// 0.8.x～0.3.8 的标签体系是**全局一份**、住在注册库里。
    /// 升级到「每库一套」时，要把那份复制进每个已有库 —— 否则库里的
    /// `asset_tags.tag_id` 会指向一个不存在的标签（界面上就是"标签没了"）。
    ///
    /// 每个库只灌一次（`tags_seeded` 盖章），用户之后删空标签也不会被灌回来。
    fn seed_tags_into_existing_libs(&mut self) {
        let reg_has_tags = reg::tags_meta(&self.reg).map(|m| !m.is_empty()).unwrap_or(false);
        for l in &self.libs {
            if tags_seeded(&l.conn) {
                continue;
            }
            let own: i64 = l
                .conn
                .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
                .unwrap_or(0);
            if own == 0 && reg_has_tags {
                let _ = copy_tag_rows(&self.reg, &l.conn);
            }
            mark_tags_seeded(&l.conn);
        }
    }

    /// 把所有还住在数据目录里的老库，搬进各自库根的 `.AssetManager/lib.db`。
    ///
    /// 每一步都可失败可跳过：盘没挂上、目录只读、复制出错 —— 都只记一行日志，
    /// **绝不因此让软件打不开**。搬成功后老文件挪进 `trash-libs/` 留证（不删）。
    fn migrate_libs_into_home(&mut self) {
        let rows = match reg::libs(&self.reg) {
            Ok(r) => r,
            Err(_) => return,
        };
        for row in rows {
            let src = self.lib_path(&row);
            if !src.exists() || libhome::is_inside_home(&src) {
                continue; // 已经在库里了 / 本来就没有文件
            }
            if !Path::new(&row.root).is_dir() || !libhome::probe_writable(&row.root) {
                continue; // 盘没挂上 / 只读：下次启动再试
            }
            let dst = libhome::db_for(&row.root);
            if !dst.exists() {
                if let Err(e) = copy_db(&src, &dst) {
                    eprintln!("库 {src:?} 搬进 .AssetManager 失败（先用老位置）：{e}");
                    continue;
                }
                /* 校验：新文件能打开、目录数一致。宁可多读一次，也不要搬出个半截库 */
                let ok = match (db::open(&dst), db::open_raw(&src)) {
                    (Ok(a), Ok(b)) => {
                        db::dir_count(&a).unwrap_or(-1) == db::dir_count(&b).unwrap_or(-2)
                    }
                    _ => false,
                };
                if !ok {
                    eprintln!("库 {src:?} 复制后校验不通过，保留老位置不动");
                    let _ = std::fs::remove_file(&dst);
                    continue;
                }
            }
            let _ = libhome::write_marker(&row.root, row.id, db::now_ts(), env!("CARGO_PKG_VERSION"));
            let _ = reg::add_lib(&self.reg, &row.root, &dst.to_string_lossy());
            /* 老文件留证：不删。用户装回旧版本时还能看到数据 */
            let keep = self.data_dir.join("trash-libs");
            let _ = std::fs::create_dir_all(&keep);
            for suffix in ["", "-wal", "-shm"] {
                let f = PathBuf::from(format!("{}{}", src.to_string_lossy(), suffix));
                if f.exists() {
                    let name = f
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let _ = std::fs::rename(&f, keep.join(format!("moved-{}-{}", db::now_ts(), name)));
                }
            }
        }
    }

    /* ---------------------------------------------------------------- 扫描 */

    pub fn scan<F>(&mut self, root: &str, mut cb: F) -> Result<model::ScanStats, String>
    where
        F: FnMut(&model::ScanProgress),
    {
        let cancel = AtomicBool::new(false);
        self.scan_cancelable(root, &cancel, |p| cb(p))
    }

    /// 可取消的扫描（界面上的「取消」按钮走这条）。
    pub fn scan_cancelable<F>(
        &mut self,
        root: &str,
        cancel: &AtomicBool,
        mut cb: F,
    ) -> Result<model::ScanStats, String>
    where
        F: FnMut(&model::ScanProgress),
    {
        let id = self.ensure_lib(root)?;
        let opts = scan::ScanOptions::default();
        let lib = self.lib_mut(id)?;
        scan::scan_root(&mut lib.conn, root, &opts, cancel, |p| cb(p))
    }

    /// 启动增量扫描（T06）：只 stat 库内记过的目录，mtime 变了才重扫那一个目录，
    /// 目录没了就把行删掉。**不递归、不全量**，所以百万库也能在启动几秒内跑完一轮。
    pub fn incremental<F>(
        &mut self,
        cancel: &AtomicBool,
        limit: i64,
        mut cb: F,
    ) -> Result<model::ScanStats, String>
    where
        F: FnMut(&model::ScanProgress),
    {
        let t0 = std::time::Instant::now();
        let mut stats = model::ScanStats::default();
        // 每个库分一份配额：3 个库各 4000 目录，不要出现"A 库把预算吃光、B 库永远不检查"
        let per = (limit / self.libs.len().max(1) as i64).max(200);
        for l in self.libs.iter_mut() {
            let list = db::dirs_to_check(&l.conn, per)?;
            for (id, path, old_mtime) in list {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let p = Path::new(&path);
                if !p.is_dir() {
                    if db::remove_dir_if_leaf(&l.conn, id).unwrap_or(false) {
                        stats.removed_dirs += 1;
                    }
                    continue;
                }
                let now = watch::dir_mtime(p);
                if now == old_mtime && old_mtime != 0 {
                    continue;
                }
                match scan::scan_one_dir(&mut l.conn, &path) {
                    Ok(st) => {
                        stats.dirs += 1;
                        stats.files += st.files;
                        cb(&model::ScanProgress {
                            dirs: stats.dirs,
                            files: stats.files,
                            current: path.clone(),
                            done: false,
                        });
                    }
                    Err(_) => stats.errors += 1,
                }
            }
        }
        stats.elapsed_ms = t0.elapsed().as_millis();
        cb(&model::ScanProgress {
            dirs: stats.dirs,
            files: stats.files,
            current: String::new(),
            done: true,
        });
        Ok(stats)
    }

    /// 重建索引（T13）：清掉 assets/dirs（**库根保留**），再把每个库根全量重扫一遍。
    ///
    /// 拆库之后可以只重建一个库（`only`），这在"某个库的索引跟磁盘对不上"时很有用：
    /// 3 个库全量重扫是 3 倍的等待，而用户往往只有一个库出问题。
    pub fn rebuild<F>(
        &mut self,
        cancel: &AtomicBool,
        only: Option<i64>,
        mut cb: F,
    ) -> Result<model::ScanStats, String>
    where
        F: FnMut(&model::ScanProgress),
    {
        let t0 = std::time::Instant::now();
        let mut total = model::ScanStats::default();
        let targets: Vec<(i64, String)> = self
            .libs
            .iter()
            .filter(|l| only.map(|o| o == l.id).unwrap_or(true))
            .map(|l| (l.id, l.root.clone()))
            .collect();
        let opts = scan::ScanOptions::default();
        for (id, root) in targets {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Ok(lib) = self.lib_mut(id) else { continue };
            db::reset_index(&lib.conn)?;
            match scan::scan_root(&mut lib.conn, &root, &opts, cancel, |p| cb(p)) {
                Ok(s) => {
                    total.dirs += s.dirs;
                    total.files += s.files;
                    total.images += s.images;
                    total.with_dims += s.with_dims;
                    total.errors += s.errors;
                    total.skipped += s.skipped;
                }
                Err(e) => {
                    total.errors += 1;
                    eprintln!("重建索引时扫 {} 失败：{}", root, e);
                }
            }
        }
        total.elapsed_ms = t0.elapsed().as_millis();
        cb(&model::ScanProgress {
            dirs: total.dirs,
            files: total.files,
            current: String::new(),
            done: true,
        });
        Ok(total)
    }

    /* ---------------------------------------------------------------- 列表 */

    pub fn list(&self, q: &model::AssetQuery) -> Result<Vec<model::Asset>, String> {
        let mut q = q.clone();
        /* 标签名 → id 必须**按库**解析（0.3.9 起每个库有自己的 tags 表，
           同一个名字在不同库里是不同的 id）。这里只留下名字，等下面遍历库时再换算。 */
        let tag_name: Option<String> = q
            .tag
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && autotag::parse_filter(s).is_none())
            .map(|s| s.to_string());
        /* 注意：`q.tag` 这个**名字**要留着 —— db 层靠它区分
           「自动标签 / 规则标签 / 手工标签」三条路。只把调用方传进来的
           id / 规则清掉（那些是注册库时代的全局 id，现在已经没意义了）。 */
        q.tag_ids = None;
        q.tag_rule = None;

        // 搜索词里的标签名，同理按库换算
        let text = q.text.clone().unwrap_or_default();
        let needle = text.trim().trim_start_matches('#').trim().to_string();
        q.search_tag_ids = None;

        /* 分组筛选（0.2.x）：成员是**全局 asset id**，按库号拆成各库的库内 id。
           拆完每个库只查自己那一批 —— 跨库分组也能一次查对，不用把三个库全扫一遍。 */
        let coll_ids: Option<std::collections::HashMap<i64, Vec<i64>>> = match q.collection {
            Some(cid) => {
                let gids = reg::coll_gids(&self.reg, cid)?;
                let mut m: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
                for g in gids {
                    let (lib_id, local) = ungid(g);
                    m.entry(lib_id).or_default().push(local);
                }
                Some(m)
            }
            None => None,
        };

        /* 快照筛选（0.1.18）：条目是**内容哈希**，先换算成全局 id（带缓存），
           再走与分组完全相同的拆库路径 —— 跨库快照也能一次查对。 */
        let snap_ids: Option<std::collections::HashMap<i64, Vec<i64>>> = match q.snapshot {
            Some(sid) => {
                let gids = self.snapshot_gids(sid)?;
                let mut m: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
                for g in gids {
                    let (lib_id, local) = ungid(g);
                    m.entry(lib_id).or_default().push(local);
                }
                Some(m)
            }
            None => None,
        };
        q.collection = None; // 往下只认 `ids`，别让 db 层再理解一次分组
        q.snapshot = None;
        /* 两个筛选同时给时不叠加（界面是单选），**快照优先** —— 它更"具体"。 */
        let member_ids = snap_ids.or(coll_ids);

        if let Some(g) = q.dir_id {
            let (lib_id, local) = ungid(g);
            let l = self.lib(lib_id)?;
            let mut q2 = q.clone();
            q2.dir_id = Some(local);
            /* 标签 / 搜索词按**这个库**的标签体系换算成 id */
            let (tids, trule) = lib_tag_lookup(&l.conn, tag_name.as_deref());
            q2.tag_ids = tids;
            q2.tag_rule = trule;
            if !needle.is_empty() {
                q2.search_tag_ids = Some(lib_tag_ids_matching(&l.conn, &needle));
            }
            if let Some(m) = &member_ids {
                q2.ids = Some(m.get(&lib_id).cloned().unwrap_or_default());
            }
            if let Some((k, id)) = q2.cursor {
                q2.cursor = Some((k, ungid(id).1));
            }
            let mut rows = db::list_assets(&l.conn, &q2)?;
            for a in &mut rows {
                a.id = gid(lib_id, a.id);
                a.dir_id = gid(lib_id, a.dir_id);
            }
            return Ok(rows);
        }

        /* 跨库合并分页（N ≤ 3）。
           做法：每个库各取一页（同一套排序、同一个游标），再在内存里归并取前 limit 条。
           正确性靠一条性质：**整张表排好序的前 K 条，一定落在各库各自的前 K 条里**。
           游标的处理是这里唯一需要动脑的地方 —— 见下面三段注释。 */
        let limit = q.limit.unwrap_or(200).clamp(1, 1000);
        let sort = q.sort.clone().unwrap_or_else(|| "mtime_desc".into());
        let cursor = q.cursor;
        let (ck, cg) = match cursor {
            Some((k, g)) => (k, g),
            None => (0, 0),
        };
        let cursor_lib = lib_of(cg);
        // name_asc 的游标第一维是"上一页最后一行的名字"，得去它所在的库里查一次
        let name_cursor: Option<(String, i64)> = if sort == "name_asc" && cursor.is_some() {
            let (cl, local) = ungid(cg);
            match self.lib(cl) {
                Ok(l) => db::name_of(&l.conn, local)?.map(|n| (n, local)),
                Err(_) => None,
            }
        } else {
            None
        };

        let mut all: Vec<model::Asset> = Vec::new();
        for l in &self.libs {
            let mut q2 = q.clone();
            q2.dir_id = None;
            let (tids, trule) = lib_tag_lookup(&l.conn, tag_name.as_deref());
            q2.tag_ids = tids;
            q2.tag_rule = trule;
            if !needle.is_empty() {
                q2.search_tag_ids = Some(lib_tag_ids_matching(&l.conn, &needle));
            }
            q2.limit = Some(limit);
            q2.cursor = None;
            q2.name_cursor = None;
            if let Some(m) = &member_ids {
                let mine = m.get(&l.id).cloned().unwrap_or_default();
                // 这个库一张都不在分组/快照里 → 跳过整个库（省一次全表查）
                if mine.is_empty() {
                    continue;
                }
                q2.ids = Some(mine);
            }
            if cursor.is_some() {
                if sort == "name_asc" {
                    if let Some((name, local)) = &name_cursor {
                        /* 升序 + (name, id) 游标。库号小的库，同名的行排在游标行**之前**，
                           要让它们"从名字之后开始"，把局部 id 顶到 i64::MAX；
                           库号大的库同名行排在游标行**之后**，用 -1 把同名的也带上。 */
                        let adj = if l.id < cursor_lib {
                            i64::MAX
                        } else if l.id > cursor_lib {
                            -1
                        } else {
                            *local
                        };
                        q2.name_cursor = Some((name.clone(), adj));
                    }
                } else {
                    // 降序：库号小的同值行排在后面 → 全带上（id 顶满）；
                    // 库号大的排在前面 → 同值行一条都不该取（id 取 0 即可，因为 id>0）
                    let adj = if l.id < cursor_lib {
                        i64::MAX
                    } else if l.id > cursor_lib {
                        0
                    } else {
                        ungid(cg).1
                    };
                    q2.cursor = Some((ck, adj));
                }
            }
            let rows = db::list_assets(&l.conn, &q2)?;
            for mut a in rows {
                a.id = gid(l.id, a.id);
                a.dir_id = gid(l.id, a.dir_id);
                all.push(a);
            }
        }
        all.sort_by(|a, b| cmp_assets(a, b, &sort));
        all.truncate(limit as usize);
        Ok(all)
    }

    pub fn tree(&self, parent: Option<i64>) -> Result<Vec<model::DirNode>, String> {
        let mut out = Vec::new();
        match parent {
            None => {
                for l in &self.libs {
                    for mut n in db::dir_children(&l.conn, None)? {
                        n.id = gid(l.id, n.id);
                        n.parent_id = None;
                        out.push(n);
                    }
                }
                // 库本身按名字排（与目录树一致），库多时顺序稳定
                out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            Some(g) => {
                let (lib_id, local) = ungid(g);
                let l = self.lib(lib_id)?;
                for mut n in db::dir_children(&l.conn, Some(local))? {
                    n.id = gid(lib_id, n.id);
                    n.parent_id = Some(g);
                    out.push(n);
                }
            }
        }
        Ok(out)
    }

    pub fn roots(&self) -> Result<Vec<model::DirNode>, String> {
        self.tree(None)
    }

    /// 把**刚建出来的空目录**写进索引，返回它的目录 id。
    ///
    /// 为什么必须单独写一行：`scan_one_dir(parent)` 只索引**文件**，空目录在磁盘上
    /// 存在但 `dirs` 表里没有行 —— `children(parent)` 自然查不到，用户看到的就是
    /// "在软件里新建了文件夹，软件里却看不见"（owner 2026-11-19 报的）。
    ///
    /// 失败不影响磁盘上的目录（调用方已建好），只是这一层看不到 —— 所以调用点
    /// 可以只记日志，不必把整个动作判失败。
    pub fn index_new_dir(&mut self, path: &str, parent: &str) -> Result<i64, String> {
        let lib_id = self
            .lib_for_path(path)
            .ok_or_else(|| format!("这个目录不在任何素材库里：{path}"))?;
        let mtime = std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let l = self.lib_mut(lib_id)?;
        let pid = match db::dir_id_by_path(&l.conn, parent)? {
            Some(id) => Some(id),
            /* 前端的路径字符串和索引里存的可能一个斜杠/大小写不同（老的扫描版本写过别的写法），
               对不上就按规范化路径再找一次 —— 挂错父节点的话，新文件夹会跑到树根上去。 */
            None => db::find_dir_id_norm(&l.conn, parent)?,
        };
        db::upsert_dir(&l.conn, path, pid, mtime)
    }

    pub fn total(&self) -> Result<i64, String> {
        let mut n = 0;
        for l in &self.libs {
            n += db::count_assets(&l.conn)?;
        }
        Ok(n)
    }

    /// 按一批全局 id 取素材（查重/引用面板要显示缩略图与文件名）。
    /// 查不到的直接跳过 —— 列表里少一行，比整块报错好。
    pub fn assets_by_ids(&self, ids: &[i64]) -> Result<Vec<model::Asset>, String> {
        let mut out = Vec::with_capacity(ids.len());
        for (lib_id, locals) in ids::group_by_lib(ids) {
            let Ok(l) = self.lib(lib_id) else { continue };
            for local in locals {
                if let Some(mut a) = db::asset_by_id(&l.conn, local)? {
                    a.id = gid(lib_id, a.id);
                    a.dir_id = gid(lib_id, a.dir_id);
                    out.push(a);
                }
            }
        }
        Ok(out)
    }

    pub fn asset(&self, g: i64) -> Result<Option<model::Asset>, String> {
        let (lib_id, local) = ungid(g);
        let Ok(l) = self.lib(lib_id) else { return Ok(None) };
        let mut a = db::asset_by_id(&l.conn, local)?;
        if let Some(a) = &mut a {
            a.id = g;
            a.dir_id = gid(lib_id, a.dir_id);
        }
        Ok(a)
    }

    /* ------------------------------------------------------------ 人写字段 */

    pub fn set_rating(&mut self, ids: &[i64], rating: i32) -> Result<usize, String> {
        let mut n = 0;
        for (lib_id, locals) in ids::group_by_lib(ids) {
            let l = self.lib_mut(lib_id)?;
            n += db::set_rating(&l.conn, &locals, rating)?;
        }
        Ok(n)
    }

    pub fn set_note(&mut self, g: i64, note: &str) -> Result<(), String> {
        let (lib_id, local) = ungid(g);
        db::set_note(&self.lib(lib_id)?.conn, local, note)
    }

    pub fn set_source_url(&mut self, g: i64, url: &str) -> Result<(), String> {
        let (lib_id, local) = ungid(g);
        db::set_source_url(&self.lib(lib_id)?.conn, local, url)
    }

    /// 写主色板（出缩略图时顺带算出来的）
    pub fn set_palette(&mut self, g: i64, palette: &str, color: &str) -> Result<(), String> {
        let (lib_id, local) = ungid(g);
        db::set_palette(&self.lib(lib_id)?.conn, local, palette, color)
    }

    /// 只写相似指纹（出缩略图时顺带算的那个）
    pub fn set_phash(&mut self, g: i64, phash: i64) -> Result<(), String> {
        let (lib_id, local) = ungid(g);
        db::set_phash(&self.lib(lib_id)?.conn, local, phash)
    }

    pub fn set_thumb(&mut self, g: i64, key: &str, state: i32, quality: i32, bytes: i64) -> Result<(), String> {
        let (lib_id, local) = ungid(g);
        db::set_thumb(&self.lib(lib_id)?.conn, local, key, state, quality, bytes)
    }

    pub fn mark_thumb_state(&mut self, g: i64, state: i32) -> Result<(), String> {
        let (lib_id, local) = ungid(g);
        db::mark_thumb_state(&self.lib(lib_id)?.conn, local, state)
    }

    pub fn set_dims_if_missing(&mut self, g: i64, w: u32, h: u32) -> Result<(), String> {
        let (lib_id, local) = ungid(g);
        db::set_dims_if_missing(&self.lib(lib_id)?.conn, local, w, h)
    }

    /* ---------------------------------------------------------------- 标签 */

    /// 标签体系落在**当前库**自己的连接上（0.3.9 起每库一套）。
    /// 一个库都没有时退回注册库：老版本升级的中间态 / 单测里还没有库，
    /// 这时候至少还能把"以前建的标签"列出来，不会一片空白。
    fn tag_conn(&self) -> &rusqlite::Connection {
        self.cur_lib().map(|l| &l.conn).unwrap_or(&self.reg)
    }

    /// 手工标签 + 使用数（**当前库**）。
    /// 规则标签（T34）的计数现场算 —— 规则是"活"的，没有可数的关联行。
    pub fn tags(&self) -> Result<Vec<model::TagCount>, String> {
        let c = self.tag_conn();
        let meta = reg::tags_meta(c)?;
        let has_lib = self.cur_lib().is_some();
        let mut counts: HashMap<i64, i64> = if has_lib {
            db::counts_by_tag(c)?.into_iter().collect()
        } else {
            HashMap::new()
        };
        let mut out = Vec::new();
        for t in &meta {
            let mut n = counts.remove(&t.id).unwrap_or(0);
            if let Some(rule) = t.rule.as_deref().and_then(reg::parse_rule) {
                if has_lib {
                    n = db::count_rule(c, &rule)?;
                } else {
                    n = 0;
                }
            }
            out.push(model::TagCount {
                name: t.name.clone(),
                count: n,
                color: t.color.clone(),
                group: t.group.clone(),
            });
        }
        // 排序口径与 0.5.0 一致：先按分组（未分组最后），组内按命中数降序
        let group_rank: HashMap<String, i64> = self
            .groups()?
            .into_iter()
            .enumerate()
            .map(|(i, g)| (g.name, i as i64))
            .collect();
        out.sort_by(|a, b| {
            let ga = a.group.as_ref().and_then(|g| group_rank.get(g)).copied().unwrap_or(i64::MAX);
            let gb = b.group.as_ref().and_then(|g| group_rank.get(g)).copied().unwrap_or(i64::MAX);
            ga.cmp(&gb)
                .then(a.group.cmp(&b.group))
                .then(b.count.cmp(&a.count))
                .then(a.name.cmp(&b.name))
        });
        Ok(out)
    }

    /// 自动标签 + 使用数（当前库；规则推导只认这一份数据）
    pub fn auto_tags(&self) -> Result<Vec<model::TagCount>, String> {
        match self.cur_lib() {
            Some(l) => db::auto_tag_counts(&l.conn),
            None => Ok(Vec::new()),
        }
    }

    pub fn groups(&self) -> Result<Vec<model::TagGroup>, String> {
        let c = self.tag_conn();
        let meta = reg::tags_meta(c)?;
        let has_lib = self.cur_lib().is_some();
        let counts: HashMap<i64, i64> = if has_lib {
            db::counts_by_tag(c)?.into_iter().collect()
        } else {
            HashMap::new()
        };
        let rows = reg::groups(c, &meta, &counts)?;
        Ok(rows
            .into_iter()
            .map(|g| model::TagGroup { name: g.name, color: g.color, count: g.count, tags: g.tags })
            .collect())
    }

    /// 一批素材各自的标签名（`id` 是全局 id）。
    /// **按每个 id 所属的库取名字**：不同库可以有同名标签，但 id 是各库自己的。
    pub fn tags_for(&self, ids: &[i64]) -> Result<Vec<(i64, Vec<String>)>, String> {
        let mut out: Vec<(i64, Vec<String>)> = Vec::new();
        for (lib_id, locals) in ids::group_by_lib(ids) {
            let l = self.lib(lib_id)?;
            let name_of: HashMap<i64, String> =
                reg::tags_meta(&l.conn)?.into_iter().map(|t| (t.id, t.name)).collect();
            for (local, tids) in db::tag_ids_for_assets(&l.conn, &locals)? {
                let names: Vec<String> =
                    tids.iter().filter_map(|t| name_of.get(t).cloned()).collect();
                out.push((gid(lib_id, local), names));
            }
        }
        out.sort_by_key(|(id, _)| *id);
        Ok(out)
    }

    pub fn tag_assets(&mut self, ids: &[i64], name: &str) -> Result<i64, String> {
        let name = reg::validate_tag_name(name)?;
        let mut n = 0;
        for (lib_id, locals) in ids::group_by_lib(ids) {
            let l = self.lib(lib_id)?;
            // 同名标签在各自库里各建一份（标签体系跟着库走）
            let tag_id = reg::ensure_tag(&l.conn, &name)?;
            n += db::insert_asset_tags(&l.conn, &locals, tag_id)?;
        }
        Ok(n)
    }

    pub fn untag_assets(&mut self, ids: &[i64], name: &str) -> Result<i64, String> {
        let mut n = 0;
        for (lib_id, locals) in ids::group_by_lib(ids) {
            let l = self.lib(lib_id)?;
            let Some(tag_id) = reg::tag_id_by_name(&l.conn, name)? else { continue };
            n += db::delete_asset_tags(&l.conn, &locals, tag_id)?;
        }
        Ok(n)
    }

    /// 详情面板保存：整组替换（按名字给，内部换算成 id）
    pub fn set_asset_tags(&mut self, g: i64, names: &[String]) -> Result<(), String> {
        let (lib_id, local) = ungid(g);
        let l = self.lib(lib_id)?;
        let mut ids = Vec::new();
        for n in names {
            let n = reg::validate_tag_name(n)?;
            ids.push(reg::ensure_tag(&l.conn, &n)?);
        }
        ids.sort();
        ids.dedup();
        db::set_asset_tags(&l.conn, local, &ids)?;
        self.gc_tags()
    }

    /// 建一个**空标签**（还没有素材挂着它也算数）。
    ///
    /// 侧栏「+ 标签」用它：用户的心智是"我先建一个标签，回头再往上挂素材"，
    /// 而不是"必须先去素材上打一次"。`tags()` 是从 meta 表列出来的，count 可以是 0，
    /// 所以插一行就能出现在侧栏里。
    pub fn create_tag(&mut self, name: &str) -> Result<(), String> {
        let n = reg::validate_tag_name(name)?;
        reg::ensure_tag(self.tag_conn(), &n).map(|_| ())
    }

    pub fn set_tag_color(&mut self, name: &str, color: Option<&str>) -> Result<(), String> {
        reg::set_tag_color(self.tag_conn(), name, color)
    }

    pub fn set_tag_group(&mut self, name: &str, group: Option<&str>) -> Result<(), String> {
        reg::set_tag_group(self.tag_conn(), name, group)
    }

    /// 给标签设自动规则（T34）。**标签不存在就先建出来** —— 用户的心智是
    /// 「我建一条规则叫『PS 源文件』」，不是「先去打个标签再来设规则」。
    pub fn set_tag_rule(&mut self, name: &str, rule: Option<&reg::AutoRule>) -> Result<(), String> {
        let n = reg::validate_tag_name(name)?;
        let c = self.tag_conn();
        reg::ensure_tag(c, &n)?;
        reg::set_tag_rule(c, &n, rule)
    }

    pub fn tag_rule(&self, name: &str) -> Result<Option<reg::AutoRule>, String> {
        Ok(reg::tags_meta(self.tag_conn())?
            .into_iter()
            .find(|t| t.name == name)
            .and_then(|t| t.rule.as_deref().and_then(reg::parse_rule)))
    }

    /// 标签改名（当前库）；新名已存在时**合并**。
    pub fn rename_tag(&mut self, old: &str, new: &str) -> Result<i64, String> {
        let new = reg::validate_tag_name(new)?;
        if old == new {
            return Ok(0);
        }
        let merged = {
            let c = self.tag_conn();
            let Some(old_id) = reg::tag_id_by_name(c, old)? else {
                return Err(format!("没有这个标签：{old}"));
            };
            match reg::tag_id_by_name(c, &new)? {
                None => {
                    c.execute(
                        "UPDATE tags SET name = ?1 WHERE id = ?2",
                        rusqlite::params![new, old_id],
                    )
                    .map_err(|e| e.to_string())?;
                    0
                }
                Some(new_id) => {
                    let n = db::move_tag_refs(c, old_id, new_id)?;
                    // 合并过去的标签如果自己还有规则，把规则也带过去（不为空才覆盖）
                    let _ = c.execute(
                        "UPDATE tags SET auto_rule = COALESCE(auto_rule, (SELECT auto_rule FROM tags WHERE id = ?1)) WHERE id = ?2",
                        rusqlite::params![old_id, new_id],
                    );
                    c.execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![old_id])
                        .map_err(|e| e.to_string())?;
                    n
                }
            }
        };
        self.gc_tags()?;
        Ok(merged)
    }

    /// 从库里的素材上移除某标签（当前库），返回影响条数
    pub fn delete_tag(&mut self, name: &str) -> Result<i64, String> {
        let c = self.tag_conn();
        let Some(tag_id) = reg::tag_id_by_name(c, name)? else { return Ok(0) };
        let n = db::delete_tag_refs(c, tag_id)?;
        c.execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![tag_id])
            .map_err(|e| e.to_string())?;
        Ok(n)
    }

    /// 清掉"一个素材都没打"的标签行（只看当前库 —— 每个库的标签体系是独立的）。
    /// 什么时候会有：用户把某标签从最后一个素材上摘掉；或者合并后旧标签空了。
    fn gc_tags(&mut self) -> Result<(), String> {
        let Some(id) = self.cur_lib().map(|l| l.id) else { return Ok(()) };
        let l = self.lib(id)?;
        let used: HashSet<i64> = db::used_tag_ids(&l.conn)?.into_iter().collect();
        for t in reg::tags_meta(&l.conn)? {
            if !used.contains(&t.id) && t.rule.is_none() {
                l.conn
                    .execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![t.id])
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub fn group_new(&mut self, name: &str) -> Result<(), String> {
        reg::group_new(self.tag_conn(), name)
    }
    pub fn group_rename(&mut self, old: &str, new: &str) -> Result<(), String> {
        reg::group_rename(self.tag_conn(), old, new)
    }
    pub fn group_set_color(&mut self, name: &str, color: Option<&str>) -> Result<(), String> {
        reg::group_set_color(self.tag_conn(), name, color)
    }
    pub fn group_delete(&mut self, name: &str) -> Result<i64, String> {
        reg::group_delete(self.tag_conn(), name)
    }

    /// 各色桶的素材数（跨库汇总）
    pub fn color_counts(&self) -> Result<Vec<(String, i64)>, String> {
        let mut m: HashMap<String, i64> = HashMap::new();
        for l in &self.libs {
            for (c, n) in db::color_counts(&l.conn)? {
                *m.entry(c).or_insert(0) += n;
            }
        }
        let mut v: Vec<(String, i64)> = m.into_iter().collect();
        // 展示顺序：暖 → 冷 → 中性（和色板图例一致）
        let order = [
            "red", "orange", "amber", "yellow", "green", "teal", "blue", "indigo", "violet",
            "pink", "brown", "black", "gray", "white",
        ];
        v.sort_by_key(|(c, _)| order.iter().position(|x| x == c).unwrap_or(99));
        Ok(v)
    }

    /* ------------------------------------------------------------ 撤销 / 缓存 */

    /// 撤销上一次文件操作。拆库后要**先比时间戳**才知道该撤哪个库的那一条。
    pub fn undo(&mut self) -> Result<String, String> {
        let mut best: Option<(i64, i64)> = None; // (ts, lib_id)
        for l in &self.libs {
            if let Ok(Some((_, _, payload_ts))) = db::last_undoable_op_with_ts(&l.conn) {
                if best.map(|(ts, _)| payload_ts > ts).unwrap_or(true) {
                    best = Some((payload_ts, l.id));
                }
            }
        }
        let Some((_, lib_id)) = best else {
            return Err("没有可撤销的操作".into());
        };
        let l = self.lib_mut(lib_id)?;
        let ops = fsops::FsOps::new(&l.conn);
        ops.undo_last()
    }

    /// 缩略图缓存淘汰（跨库按 last_used 统一排序）。返回 (淘汰前总量, 释放字节)。
    pub fn gc_thumbs(&mut self, limit: i64, target: i64) -> Result<(i64, i64), String> {
        struct Entry {
            key: String,
            bytes: i64,
            last_used: i64,
            lib: i64,
        }
        let mut all: Vec<Entry> = Vec::new();
        for l in &self.libs {
            for (key, bytes, last_used) in db::thumb_lru_rows(&l.conn)? {
                all.push(Entry { key, bytes, last_used, lib: l.id });
            }
        }
        let total: i64 = all.iter().map(|e| e.bytes).sum();
        if total <= limit {
            return Ok((total, 0));
        }
        all.sort_by_key(|e| e.last_used);
        let mut cur = total;
        let mut freed = 0;
        for e in all {
            if cur <= target {
                break;
            }
            if let Ok(l) = self.lib(e.lib) {
                let _ = std::fs::remove_file(l.thumbs.file_for(&e.key));
                let _ = std::fs::remove_file(l.thumbs.file_for(&e.key).with_extension("jpg"));
            }
            if let Ok(l) = self.lib_mut(e.lib) {
                let _ = l
                    .conn
                    .execute("DELETE FROM thumb_lru WHERE key=?1", rusqlite::params![e.key]);
                let _ = l.conn.execute(
                    "UPDATE assets SET thumb_key=NULL, thumb_state=0 WHERE thumb_key=?1",
                    rusqlite::params![e.key],
                );
            }
            cur -= e.bytes;
            freed += e.bytes;
        }
        Ok((total, freed))
    }

    /* ---------------------------------------------------------- 导出 / 复制 */

    /// 用户数据导出（T13）：库根 + 每个素材的"用户层"字段（评分/备注/来源/标签）。
    pub fn export_userdata(&self) -> Result<String, String> {
        let mut items: Vec<serde_json::Value> = Vec::new();
        for l in &self.libs {
            /* 标签 id 是**每个库自己**的（0.3.9），名字必须按这个库的表换算 */
            let name_of: HashMap<i64, String> =
                reg::tags_meta(&l.conn)?.into_iter().map(|t| (t.id, t.name)).collect();
            let mut tagmap: HashMap<i64, Vec<String>> = HashMap::new();
            for (aid, tids) in db::all_asset_tags(&l.conn)? {
                let names: Vec<String> =
                    tids.iter().filter_map(|t| name_of.get(t).cloned()).collect();
                tagmap.insert(aid, names);
            }
            for row in db::user_rows(&l.conn)? {
                let (id, dir, name, ext, size, mtime, rating, note, source) = row;
                items.push(serde_json::json!({
                    "lib": l.root,
                    "dir": dir,
                    "name": name,
                    "ext": ext,
                    "size": size,
                    "mtime": mtime,
                    "rating": rating,
                    "note": note,
                    "sourceUrl": source,
                    "tags": tagmap.get(&id).cloned().unwrap_or_default(),
                }));
            }
        }
        let doc = serde_json::json!({
            "app": "AssetManager",
            "version": env!("CARGO_PKG_VERSION"),
            "exportedAt": db::now_ts(),
            "note": "索引可重建；这份文件只保留用户层数据（库根 / 评分 / 备注 / 来源 / 标签）",
            "libs": self.lib_roots()?,
            "assets": items,
        });
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
    }

    /// 把整套索引（注册库 + 每个库文件）复制到另一个数据目录（换「数据文件夹」时用）。
    ///
    /// 用 `VACUUM INTO` 而不是 `fs::copy`：拷文件会把 `-wal` 里还没落盘的事务丢掉，
    /// 而且 `VACUUM INTO` 顺手整理了空闲页、写出来的是单文件、无 WAL 的干净库。
    /// 目标已存在索引就拒绝 —— 覆盖别人的库比"多做一步"危险得多。
    ///
    /// **不复制缩略图缓存**（几 GB，重新生成更快）。
    pub fn copy_index_to(&self, dest_dir: &Path) -> Result<u64, String> {
        std::fs::create_dir_all(dest_dir).map_err(|e| format!("建目标目录失败：{e}"))?;
        let dest = db::db_path_for(dest_dir);
        if dest.exists() {
            return Err(format!("目标位置已经有索引了：{}", dest.display()));
        }
        let mut bytes = 0u64;
        self.reg
            .execute("VACUUM INTO ?1", [dest.to_string_lossy().to_string()])
            .map_err(|e| format!("复制索引失败：{e}"))?;
        bytes += std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);

        let rows = reg::libs(&self.reg)?;
        for row in rows {
            // 目标侧的库文件名沿用注册库记录（file 相对数据目录）
            let rel = if row.file.is_empty() {
                format!("libs/lib-{}.db", row.id)
            } else {
                row.file.clone()
            };
            let out = dest_dir.join(&rel);
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| format!("建目录失败：{e}"))?;
            }
            if let Some(l) = self.libs.iter().find(|l| l.id == row.id) {
                l.conn
                    .execute("VACUUM INTO ?1", [out.to_string_lossy().to_string()])
                    .map_err(|e| format!("复制库 {} 失败：{e}", row.root))?;
                bytes += std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            }
        }
        Ok(bytes)
    }

    /* ------------------------------------------- 主色板补齐（T24 的存量数据） */

    /// 给"还没算过主色板"的素材补算（限 `limit` 张，一次调用别太久）。
    ///
    /// 数据来源优先用**已经缓存的缩略图**（256 档），命中就直接解那张小图：
    /// 几百 KB、1–3ms，比重新解原图（几十到几百 ms）快两个数量级。
    /// 没有缓存的跳过 —— 等它第一次被看见时 `ensure` 会顺带算好。
    pub fn fill_palette(&mut self, limit: i64) -> Result<(i64, i64), String> {
        let mut done = 0i64;
        let mut left = 0i64;
        for l in self.libs.iter_mut() {
            let store = l.thumbs.clone();
            let rows = db::assets_without_palette_paths(&l.conn, limit)?;
            left += rows.len() as i64;
            for (id, path) in rows {
                /* 缓存里到底是哪一档？用户在网格里看到的是 384/512，
                   256 档可能从来没生成过。四档挨个试，命中就用（从大到小：
                   大图算出来的色板更稳，但 256 也够用）。 */
                let p = Path::new(&path);
                let mut hit = None;
                for px in [512u32, 384, 256, 1600] {
                    let f = store.file_for(&thumb::ThumbStore::key_for(p, px));
                    if f.is_file() {
                        hit = Some(f);
                        break;
                    }
                }
                let Some(f) = hit else { continue };
                let Ok(img) = image::open(&f) else { continue };
                let rgb = img.to_rgba8();
                let pal = palette::compute(rgb.as_raw(), rgb.width(), rgb.height());
                if pal.hexes.is_empty() {
                    continue;
                }
                db::set_palette(&l.conn, id, &pal.joined(), &pal.bucket)?;
                let ph = dupes::ahash64(rgb.as_raw(), rgb.width(), rgb.height());
                if ph != 0 {
                    let _ = db::set_phash(&l.conn, id, ph);
                }
                done += 1;
            }
        }
        Ok((done, left - done))
    }

    /// 还缺多少元数据：`(缺主色板的张数, 缺全文件哈希的张数)`
    pub fn meta_missing(&self) -> Result<(i64, i64), String> {
        let mut no_pal = 0;
        let mut no_hash = 0;
        for l in &self.libs {
            let (p, h) = db::count_missing_meta(&l.conn)?;
            no_pal += p;
            no_hash += h;
        }
        Ok((no_pal, no_hash))
    }

    /* ------------------------------------------------ 重复检测（T25） */

    /// 精确重复：先给还没算过哈希的文件算一遍（限 `limit` 个），再返回重复组。
    ///
    /// 哈希只读字节、不解码（xxh3 走 256KB 缓冲），一个 100 MB 的文件约 0.1s。
    /// 所以这条命令是"按批推进"的：界面点一次算一批，返回值里的 `剩余` 告诉用户还有多少。
    pub fn dupes_exact(&mut self, limit: i64) -> Result<(Vec<model::DupGroup>, i64), String> {
        let mut hashed = 0i64;
        for l in self.libs.iter_mut() {
            for (id, path, _size, kind) in db::assets_without_hash_paths(&l.conn, limit)? {
                // 只对图片/视频/工程稿算：文本和缓存文件的重复没有整理价值
                if !(0..=4).contains(&kind) {
                    continue;
                }
                if let Ok(h) = dupes::hash_file(Path::new(&path)) {
                    db::set_hash_x(&l.conn, id, &h)?;
                    hashed += 1;
                }
            }
        }
        let mut out: Vec<model::DupGroup> = Vec::new();
        for l in &self.libs {
            for (h, size, ids) in db::exact_duplicate_groups(&l.conn, 200)? {
                let hex: String = h.iter().map(|b| format!("{:02x}", b)).collect();
                out.push(model::DupGroup {
                    hash: hex,
                    size,
                    ids: ids.into_iter().map(|i| gid(l.id, i)).collect(),
                });
            }
        }
        // 组越大越值得看
        out.sort_by(|a, b| b.ids.len().cmp(&a.ids.len()).then(b.size.cmp(&a.size)));
        let left = self.meta_missing()?.1;
        let _ = hashed;
        Ok((out, left))
    }

    /// 相似重复：用缩略图阶段算好的 aHash 两两比。
    /// `max_dist` 一般 4–8（0–4 基本是同一张图的不同版本）。
    pub fn dupes_similar(&self, max_dist: u32, limit: usize) -> Result<Vec<model::SimilarPair>, String> {
        let mut rows: Vec<(i64, i64)> = Vec::new();
        for l in &self.libs {
            for (id, ph, _size) in db::phash_rows(&l.conn)? {
                if ph != 0 {
                    rows.push((gid(l.id, id), ph));
                }
            }
        }
        Ok(dupes::similar_pairs(&rows, max_dist, limit)
            .into_iter()
            .map(|(a, b, dist)| model::SimilarPair { a, b, dist })
            .collect())
    }

    /* ------------------------------------------------ 引用关系（T26） */

    /// 扫一批工程文件的外部引用（限 `limit` 个文件），写 `refs`。
    /// 返回 `(扫了几个文件, 记下几条引用)`。
    pub fn refs_scan(&mut self, limit: i64) -> Result<(i64, i64), String> {
        let mut files = 0i64;
        let mut refs_n = 0i64;
        for l in self.libs.iter_mut() {
            for (id, path, _size, _kind) in db::assets_needing_refs(&l.conn, limit)? {
                let list = refs::scan_file(Path::new(&path)).unwrap_or_default();
                let mut rows: Vec<(String, Option<i64>, i32)> = Vec::new();
                for raw in list {
                    let mut hit = None;
                    for cand in refs::resolve(Path::new(&path), &raw) {
                        if let Ok(Some(x)) = db::asset_id_by_path(&l.conn, &cand) {
                            hit = Some(x);
                            break;
                        }
                    }
                    if hit.is_none() {
                        if let Some(name) = Path::new(&raw.replace('\\', "/"))
                            .file_name()
                            .and_then(|s| s.to_str())
                        {
                            hit = db::asset_id_by_name(&l.conn, name).unwrap_or(None);
                        }
                    }
                    rows.push((raw, hit, 0));
                }
                db::set_refs(&l.conn, id, &rows)?;
                db::mark_refs_scanned(&l.conn, id)?;
                files += 1;
                refs_n += rows.len() as i64;
            }
        }
        Ok((files, refs_n))
    }

    /// `(引用总数, 其中找不到文件的条数)`
    pub fn refs_summary(&self) -> Result<(i64, i64), String> {
        let mut total = 0;
        let mut missing = 0;
        for l in &self.libs {
            let (t, m) = db::refs_summary(&l.conn)?;
            total += t;
            missing += m;
        }
        Ok((total, missing))
    }

    fn ref_row(&self, lib_id: i64, r: &db::RefRow) -> model::RefItem {
        model::RefItem {
            src_id: gid(lib_id, r.src_id),
            src_name: r.src_name.clone(),
            src_path: r.src_path.clone(),
            dst_path: r.dst_path.clone(),
            dst_name: r.dst_name.clone(),
            dst_id: r.dst_id.map(|d| gid(lib_id, d)),
            missing: r.missing,
        }
    }

    /// 全库的"丢了素材"的引用（工程还在，但引用的文件找不到）
    pub fn broken_refs(&self, limit: i64) -> Result<Vec<model::RefItem>, String> {
        let mut out = Vec::new();
        for l in &self.libs {
            for r in db::broken_refs(&l.conn, limit)? {
                out.push(self.ref_row(l.id, &r));
            }
        }
        Ok(out)
    }

    /// 这个工程文件引用了哪些素材
    pub fn refs_of(&self, g: i64, limit: i64) -> Result<Vec<model::RefItem>, String> {
        let (lib_id, local) = ungid(g);
        let l = self.lib(lib_id)?;
        Ok(db::refs_of_project(&l.conn, local, limit)?
            .iter()
            .map(|r| self.ref_row(lib_id, r))
            .collect())
    }

    /// 这张图被哪些工程文件引用
    pub fn refs_using(&self, g: i64, limit: i64) -> Result<Vec<model::RefItem>, String> {
        let (lib_id, local) = ungid(g);
        let l = self.lib(lib_id)?;
        Ok(db::refs_pointing_to(&l.conn, local, limit)?
            .iter()
            .map(|r| self.ref_row(lib_id, r))
            .collect())
    }

    /* ------------------------------------------------ 批量重命名（T33） */

    /// 只算名字、**不碰磁盘**：界面拿这份预览给用户看，确认后才 apply。
    pub fn rename_plan(
        &self,
        ids: &[i64],
        rule: &model::RenameRule,
    ) -> Result<Vec<model::RenameItem>, String> {
        let mut out: Vec<model::RenameItem> = Vec::new();
        let mut seq = rule.number_from.max(1);
        let mut used: std::collections::HashSet<String> = Default::default();
        for g in ids {
            let Some(a) = self.asset(*g)? else { continue };
            let rule_keep = rule.keep_ext;
            let (stem, ext) = match a.name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() => (s.to_string(), e.to_string()),
                _ => (a.name.clone(), String::new()),
            };
            let mut base = stem.clone();
            if !rule.find.is_empty() {
                base = base.replace(&rule.find, &rule.replace);
            }
            if rule.lower {
                base = base.to_lowercase();
            }
            if rule.upper {
                base = base.to_uppercase();
            }
            let mut name = format!("{}{}{}", rule.prefix, base, rule.suffix);
            if rule.number_from > 0 {
                let n = if rule.number_pad > 0 {
                    format!("{:0width$}", seq, width = rule.number_pad)
                } else {
                    seq.to_string()
                };
                name = format!("{}{}", name, n);
                seq += 1;
            }
            let full = if !rule_keep {
                name
            } else if ext.is_empty() {
                name
            } else {
                format!("{}.{}", name, ext)
            };

            // 校验：非法字符、空名、本批重复、磁盘上已存在
            let mut err = String::new();
            let clean = full.trim();
            if clean.is_empty() {
                err = "名字为空".into();
            } else if clean.chars().any(|c| "<>:\"/\\|?*".contains(c)) {
                err = "含有不能用于文件名的字符 <>:\"/\\|?*".into();
            } else if clean != full {
                err = "首尾不能是空白".into();
            } else if !used.insert(clean.to_lowercase()) {
                err = "这一批里有重名".into();
            } else {
                let p = Path::new(&a.path);
                if let Some(parent) = p.parent() {
                    let target = parent.join(clean);
                    if target.exists() && target != p {
                        err = "目标位置已经有同名文件".into();
                    }
                }
            }
            out.push(model::RenameItem { id: *g, old_name: a.name.clone(), new_name: full, error: err });
        }
        Ok(out)
    }

    /// 真正改名字（逐条走 `FsOps::rename`，自带 ops 日志 → 可撤销）。
    /// `dirs` 里是受影响的目录，命令层拿它去刷新索引（改名不改目录内容，扫一层就够）。
    pub fn rename_apply(&mut self, items: &[(i64, String)]) -> Result<model::RenameResult, String> {
        let mut out = model::RenameResult::default();
        for (g, new_name) in items {
            let (lib_id, local) = ungid(*g);
            let Ok(l) = self.lib_mut(lib_id) else { continue };
            let Some(a) = db::asset_by_id(&l.conn, local)? else { continue };
            let p = PathBuf::from(&a.path);
            let ops = fsops::FsOps::new(&l.conn);
            match ops.rename(&p, new_name) {
                Ok(_) => {
                    out.ok += 1;
                    if let Some(parent) = p.parent() {
                        let s = parent.to_string_lossy().to_string();
                        if !out.dirs.contains(&s) {
                            out.dirs.push(s);
                        }
                    }
                }
                Err(e) => out.errors.push(format!("{}：{}", a.name, e)),
            }
        }
        Ok(out)
    }
}


/* -------------------------------------------------------------- 工具函数 */

fn norm_path(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn norm_keep_case(p: &str) -> String {
    let s = p.replace('/', "\\");
    s.trim_end_matches('\\').to_string()
}

/// 归并排序的比较器：**必须和 SQL 的 ORDER BY 一致**（含 id 倒序），
/// 否则跨库合并出来的页会在边界处重复或漏条。
fn cmp_assets(a: &model::Asset, b: &model::Asset, sort: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match sort {
        "name_asc" => a
            .name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id)),
        "size_desc" => b.size.cmp(&a.size).then(b.id.cmp(&a.id)),
        "rating_desc" => b.rating.cmp(&a.rating).then(b.id.cmp(&a.id)),
        _ => {
            let o: Ordering = b.mtime.cmp(&a.mtime).then(b.id.cmp(&a.id));
            o
        }
    }
}

/* ------------------------------------------------------------------ 快照
   一个快照 = 一批图片的「内容指纹清单」。做法和坑都写在 snapshot.rs 头注释里，
   这里只负责跨库的部分：算哈希、按哈希在三五个库里找回来、把结果对成全局 id。 */

/// 加入快照的结果（界面要分别说清"新加了几张 / 本来就有几张 / 哪几张失败"）
#[derive(Debug, Clone, serde::Serialize)]
pub struct SnapshotAdd {
    pub added: usize,
    pub total: usize,
    pub already: usize,
    pub failed: Vec<String>,
}

/// 对账后的一条：`gid` 为 None = 这次没找到（**不删条目**，用户可能只是没插硬盘）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedItem {
    pub hash: String,
    pub name: String,
    pub gid: Option<i64>,
    /// 现在能找到的路径（找不到时是快照里记的最后一次路径）
    pub path: String,
    pub lib: String,
    pub size: i64,
    pub w: Option<i64>,
    pub h: Option<i64>,
    /// 用来给 `thumb://` 做缓存失效（文件一改 URL 就变）
    pub mtime: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SnapshotResolve {
    pub items: Vec<ResolvedItem>,
    pub found: usize,
    pub missing: usize,
}

impl App {
    /// 灵感板摆位（0.1.18）：`board` = `tag:<标签>` / `snap:<快照 id>`。
    /// 只是视图记忆 —— 读不到就按瀑布流重排，不报错。
    pub fn board_layout(&self, board: &str) -> Result<Vec<(i64, f64, f64, f64, f64)>, String> {
        reg::board_layout(&self.reg, board)
    }

    pub fn board_place(
        &self,
        board: &str,
        rows: &[(i64, f64, f64, f64, f64)],
    ) -> Result<usize, String> {
        reg::board_place(&self.reg, board, rows)
    }

    /* ---------------- 分组（0.2.x） ----------------
       分组 = 用户自己攒的「一撮素材」，跨库存在（注册库里存的是全局 asset id）。
       与标签的分工：标签是属性（一张图可以有多个），分组是收纳（侧栏里点名要看的那一批）。 */

    /// 分组清单 + **真实计数**。
    ///
    /// 计数必须按"素材现在还在不在"重算：注册库里的成员行不知道别的库删了东西，
    /// 直接拿行数当数字，侧栏会显示 12 项、点进去只有 9 张（用户只会说"分组坏了"）。
    pub fn coll_list(&self) -> Result<Vec<reg::CollRow>, String> {
        let mut rows = reg::collections(&self.reg)?;
        for r in &mut rows {
            let gids = reg::coll_gids(&self.reg, r.id)?;
            r.count = self.count_gids(&gids)? as i64;
        }
        Ok(rows)
    }

    fn count_gids(&self, gids: &[i64]) -> Result<usize, String> {
        use std::collections::HashMap;
        let mut per: HashMap<i64, Vec<i64>> = HashMap::new();
        for g in gids {
            let (lib, local) = ungid(*g);
            per.entry(lib).or_default().push(local);
        }
        let mut n = 0usize;
        for (lib, ids) in per {
            let Ok(l) = self.lib(lib) else { continue };
            let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("SELECT COUNT(*) FROM assets WHERE missing = 0 AND id IN ({ph})");
            let mut st = l.conn.prepare(&sql).map_err(|e| e.to_string())?;
            let c: i64 = st
                .query_row(rusqlite::params_from_iter(ids.iter()), |r| r.get(0))
                .map_err(|e| e.to_string())?;
            n += c as usize;
        }
        Ok(n)
    }

    pub fn coll_create(&mut self, name: &str) -> Result<i64, String> {
        reg::coll_create(&self.reg, name)
    }

    pub fn coll_rename(&mut self, id: i64, name: &str) -> Result<(), String> {
        reg::coll_rename(&self.reg, id, name)
    }

    pub fn coll_delete(&mut self, id: i64) -> Result<(), String> {
        reg::coll_delete(&self.reg, id)
    }

    /// 某个标签下的全部全局 id。
    ///
    /// 「把整个标签加进分组 / 加进快照」这类操作要的是**一整批**，不是一屏；
    /// 分页口径必须和列表一致（keyset + mtime_desc），否则加进去的和看到的不一样。
    /// 上限 20 万：再大一屏操作本来也不该由界面一次性完成，防止把向量撑到几十兆。
    pub fn ids_by_tag(&self, tag: &str) -> Result<Vec<i64>, String> {
        let mut out: Vec<i64> = Vec::new();
        let mut cursor: Option<(i64, i64)> = None;
        loop {
            let q = model::AssetQuery {
                tag: Some(tag.to_string()),
                limit: Some(1000),
                cursor,
                sort: Some("mtime_desc".into()),
                ..Default::default()
            };
            let rows = self.list(&q)?;
            if rows.is_empty() {
                break;
            }
            out.extend(rows.iter().map(|r| r.id));
            if out.len() >= 200_000 {
                break;
            }
            match rows.last() {
                Some(last) => cursor = Some((last.mtime, last.id)),
                None => break,
            }
        }
        Ok(out)
    }

    pub fn coll_add(&mut self, id: i64, ids: &[i64]) -> Result<usize, String> {
        reg::coll_add(&self.reg, id, ids)
    }

    pub fn coll_remove(&mut self, id: i64, ids: &[i64]) -> Result<usize, String> {
        reg::coll_remove(&self.reg, id, ids)
    }

    /// 这张素材在哪些分组里（详情面板 / 右键菜单用）
    pub fn coll_of(&self, gid: i64) -> Result<Vec<i64>, String> {
        reg::coll_of(&self.reg, gid)
    }

    /// 快照 id → (库, 库内 id)。对外一律用**全局 id**（库号 + 库内 id）。
    fn snap_split(&self, snap: i64) -> Result<(&Lib, i64), String> {
        let (lib_id, local) = ungid(snap);
        let l = self.lib(lib_id)?;
        Ok((l, local))
    }

    /// 快照清单（**所有库汇总**）。id 换成全局 id，名称冲突不合并 ——
    /// 两个库可以各自有一份「交付稿」，它们是两个东西。
    pub fn snapshot_list(&self) -> Result<Vec<reg::SnapshotRow>, String> {
        let mut out = Vec::new();
        for l in &self.libs {
            for mut r in reg::snapshots(&l.conn)? {
                r.id = gid(l.id, r.id);
                out.push(r);
            }
        }
        /* 与单库内部同一口径：新建的在前 */
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Ok(out)
    }

    pub fn snapshot_get(&self, id: i64) -> Result<Option<reg::SnapshotRow>, String> {
        let (l, local) = self.snap_split(id)?;
        Ok(reg::snapshot(&l.conn, local)?.map(|mut r| {
            r.id = id;
            r
        }))
    }

    /// 新建快照：落在**当前库**（侧栏在哪个库，快照就属于哪个库）
    pub fn snapshot_create(&mut self, name: &str) -> Result<i64, String> {
        let id = self.cur_lib().map(|l| l.id).ok_or("还没有素材库")?;
        let l = self.lib(id)?;
        Ok(gid(id, reg::snapshot_create(&l.conn, name)?))
    }

    pub fn snapshot_rename(&mut self, id: i64, name: &str) -> Result<(), String> {
        let (l, local) = self.snap_split(id)?;
        reg::snapshot_rename(&l.conn, local, name)
    }

    pub fn snapshot_delete(&mut self, id: i64) -> Result<(), String> {
        let (l, local) = self.snap_split(id)?;
        reg::snapshot_delete(&l.conn, local)?;
        self.snap_ids_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        Ok(())
    }

    pub fn snapshot_items(&self, id: i64) -> Result<Vec<snapshot::SnapItem>, String> {
        let (l, local) = self.snap_split(id)?;
        reg::snapshot_items(&l.conn, local)
    }

    /// 快照 → 库里那批素材的**全局 id**（主列表按快照筛选用）。
    ///
    /// 换算走 `match_by_hash`（哈希命中优先，没有哈希的同体积候选现算），
    /// 结果按快照的 `updated_at` 缓存：同一个快照翻页只算一次，
    /// 快照被改了（加入/删除/对账）`updated_at` 自变，缓存自然作废。
    pub fn snapshot_gids(&self, snap: i64) -> Result<Vec<i64>, String> {
        let (l, local) = self.snap_split(snap)?;
        let row = reg::snapshot(&l.conn, local)?.ok_or_else(|| "这个快照已经不在了".to_string())?;
        {
            let g = self.snap_ids_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((ts, ids)) = g.get(&snap) {
                if *ts == row.updated_at {
                    return Ok(ids.clone());
                }
            }
        }
        let mut out: Vec<i64> = Vec::new();
        for it in reg::snapshot_items(&l.conn, local)? {
            let Some(hash) = snapshot::unhex(&it.hash) else { continue };
            if let Some((g, _, _)) = self.match_by_hash(&hash, it.size)? {
                out.push(g);
            }
        }
        out.sort_unstable();
        out.dedup();
        self.snap_ids_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(snap, (row.updated_at, out.clone()));
        Ok(out)
    }

    /// 合并 / 差集（P2.9）：把 `ids` 里的快照按 `mode` 合成**一份新快照**，原件一条不动。
    ///
    /// - `union`（默认）：并集，按哈希去重；
    /// - `diff`：差集，第一份减去其余各份。
    ///
    /// 新快照落在**第一个快照所在的库**（用户是从那个库里点的合并）。
    /// 返回 `(新快照 id, 条数, 其中新写入的行数)`。重名自动加 ` (2)`，不静默覆盖。
    pub fn snapshot_combine(
        &mut self,
        ids: &[i64],
        name: &str,
        mode: &str,
    ) -> Result<(i64, usize, usize), String> {
        if ids.is_empty() {
            return Err("先选一份快照".into());
        }
        let (home_lib, _) = self.snap_split(ids[0])?;
        let home_id = home_lib.id;

        let mut merged: Vec<snapshot::SnapItem> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        if mode == "diff" {
            let mut drop: std::collections::HashSet<String> = std::collections::HashSet::new();
            for &other in &ids[1..] {
                for it in self.snapshot_items(other)? {
                    drop.insert(it.hash);
                }
            }
            for it in self.snapshot_items(ids[0])? {
                if !drop.contains(&it.hash) && seen.insert(it.hash.clone()) {
                    merged.push(it);
                }
            }
        } else {
            for &id in ids {
                for it in self.snapshot_items(id)? {
                    if seen.insert(it.hash.clone()) {
                        merged.push(it);
                    }
                }
            }
        }
        let total = merged.len();
        /* 重名不覆盖：与导入同一口径，追加 ` (2)`、` (3)`…（只在目标库内判重） */
        let base = name.trim();
        let base = if base.is_empty() {
            if mode == "diff" {
                "快照差集"
            } else {
                "合并快照"
            }
        } else {
            base
        };
        let mut final_name = base.to_string();
        let mut n = 2;
        while self
            .snapshot_list()?
            .iter()
            .any(|s| lib_of(s.id) == home_id && s.name.eq_ignore_ascii_case(&final_name))
        {
            final_name = format!("{base} ({n})");
            n += 1;
        }
        let l = self.lib(home_id)?;
        let local = reg::snapshot_create(&l.conn, &final_name)?;
        let added = reg::snapshot_insert_items(&l.conn, local, &merged)?;
        Ok((gid(home_id, local), total, added))
    }

    pub fn snapshot_remove(&mut self, id: i64, hashes: &[String]) -> Result<usize, String> {
        let (l, local) = self.snap_split(id)?;
        let n = reg::snapshot_remove_items(&l.conn, local, hashes)?;
        self.snap_ids_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        Ok(n)
    }

    /// 把一批素材加进快照。**身份是文件内容哈希**：算过的直接用，没算过的现算并写回
    /// `assets.hash_x`（顺手，下次查重也省事）。
    pub fn snapshot_add_assets(&mut self, snap: i64, ids: &[i64]) -> Result<SnapshotAdd, String> {
        let (home, home_local) = self.snap_split(snap)?;
        let home_id = home.id;
        reg::snapshot(&home.conn, home_local)?
            .ok_or_else(|| "这个快照已经不在了".to_string())?;
        let mut items: Vec<snapshot::SnapItem> = Vec::with_capacity(ids.len());
        let mut failed: Vec<String> = Vec::new();

        for &g in ids {
            let (lib_id, local) = ungid(g);
            let Some(l) = self.libs.iter().find(|l| l.id == lib_id) else {
                failed.push(format!("#{g} 已经不属于任何库了"));
                continue;
            };
            let lib_root = l.root.clone();
            let a = match db::asset_by_id(&l.conn, local)? {
                Some(a) => a,
                None => {
                    failed.push(format!("#{g} 已经不在了"));
                    continue;
                }
            };
            let hash = match db::asset_hash(&l.conn, local)? {
                Some(h) if !h.is_empty() => h,
                _ => match snapshot::hash_path(Path::new(&a.path)) {
                    Ok(h) => {
                        let _ = db::set_hash_only(&l.conn, local, &h);
                        h
                    }
                    Err(e) => {
                        failed.push(e);
                        continue;
                    }
                },
            };
            items.push(snapshot::SnapItem {
                hash: snapshot::hex(&hash),
                name: a.name,
                size: a.size,
                w: a.width,
                h: a.height,
                path: a.path,
                lib: lib_root,
            });
        }

        let total = items.len();
        let l = self.lib(home_id)?;
        let added = reg::snapshot_insert_items(&l.conn, home_local, &items)?;
        /* updated_at 只到秒：同一秒内连加两次时缓存会认成"没变"，所以显式作废 */
        self.snap_ids_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&snap);
        Ok(SnapshotAdd { added, total, already: total - added, failed })
    }

    /// 把某个标签下的**全部**素材加进快照。不移动任何文件，语义等价于"给这批图打书签"。
    /// 只取**快照所属那个库**的素材 —— 快照是库内资产，不该被别的库的素材灌进来。
    pub fn snapshot_add_tag(&mut self, snap: i64, tag: &str) -> Result<SnapshotAdd, String> {
        let tag = tag.trim().to_string();
        if tag.is_empty() {
            return Err("没有指定标签".into());
        }
        let (home, _) = self.snap_split(snap)?;
        let home_id = home.id;
        let mut ids: Vec<i64> = Vec::new();
        let mut cursor: Option<(i64, i64)> = None;
        loop {
            let q = model::AssetQuery {
                tag: Some(tag.clone()),
                limit: Some(1000),
                cursor,
                sort: Some("mtime_desc".into()),
                ..Default::default()
            };
            let rows = self.list(&q)?;
            if rows.is_empty() {
                break;
            }
            for r in &rows {
                if lib_of(r.id) == home_id {
                    ids.push(r.id);
                }
            }
            match rows.last() {
                Some(last) => cursor = Some((last.mtime, last.id)),
                None => break,
            }
            if ids.len() > 500_000 {
                break; // 保险：真到这个量级，用户该改用整库快照了
            }
        }
        self.snapshot_add_assets(snap, &ids)
    }

    /// 对账：把快照里的每一条**按内容哈希**在库里找回来。
    ///
    /// 三条路，按代价从低到高：① 哈希命中（走索引，一条 SQL）；② 同体积候选逐个现算哈希
    /// （改名 / 新入库的行 `hash_x` 还是 NULL，只能这样）；③ 都不中 = 记为找不到。
    /// 找到之后**顺手把条目里的路径/名字更新成现在这份**（显示用，不动身份）。
    pub fn snapshot_resolve(&self, snap: i64) -> Result<SnapshotResolve, String> {
        let (l, local) = self.snap_split(snap)?;
        let stored = reg::snapshot_items(&l.conn, local)?;
        let mut items = Vec::with_capacity(stored.len());
        let mut found = 0usize;

        for it in stored {
            let Some(hash) = snapshot::unhex(&it.hash) else {
                items.push(ResolvedItem {
                    hash: it.hash,
                    name: it.name,
                    gid: None,
                    path: it.path,
                    lib: it.lib,
                    size: it.size,
                    w: it.w,
                    h: it.h,
                    mtime: 0,
                });
                continue;
            };
            match self.match_by_hash(&hash, it.size)? {
                Some((g, a, lib_root)) => {
                    found += 1;
                    // 位置变了就更新（改名 / 换库 / 换目录都走这条），失败不影响对账
                    let _ = reg::snapshot_touch_item(
                        &l.conn,
                        local,
                        &it.hash,
                        &a.name,
                        &a.path,
                        &lib_root,
                    );
                    items.push(ResolvedItem {
                        hash: it.hash,
                        name: a.name,
                        gid: Some(g),
                        path: a.path,
                        lib: lib_root,
                        size: a.size,
                        w: a.width,
                        h: a.height,
                        mtime: a.mtime,
                    });
                }
                None => items.push(ResolvedItem {
                    hash: it.hash,
                    name: it.name,
                    gid: None,
                    path: it.path,
                    lib: it.lib,
                    size: it.size,
                    w: it.w,
                    h: it.h,
                    mtime: 0,
                }),
            }
        }

        let missing = items.len() - found;
        Ok(SnapshotResolve { items, found, missing })
    }

    /// 按内容哈希在**所有库**里找一张图。命中返回 `(全局 id, 素材, 库根)`。
    ///
    /// 快照按库拆开之后仍然是"全局找"：素材被移到另一个库时，
    /// 快照里那条还能靠内容认回来（这正是哈希身份的意义）。
    fn match_by_hash(
        &self,
        hash: &[u8],
        size: i64,
    ) -> Result<Option<(i64, model::Asset, String)>, String> {
        // ① 哈希已经在库里（查重算过，或本功能算过）
        for l in &self.libs {
            if let Some(&local) = db::ids_by_hash(&l.conn, hash)?.first() {
                if let Some(mut a) = db::asset_by_id(&l.conn, local)? {
                    a.dir_id = gid(l.id, a.dir_id);
                    a.id = gid(l.id, local);
                    return Ok(Some((a.id, a, l.root.clone())));
                }
            }
        }
        // ② 刚改名 / 刚入库的行没有哈希：按体积捞候选，真读字节算一遍再比。体积相同的文件很少，
        //    候选只取 50 个，代价可控。
        if size <= 0 {
            return Ok(None);
        }
        for l in &self.libs {
            for (local, known) in db::ids_by_size(&l.conn, size, 50)? {
                let same = match known {
                    Some(bytes) => bytes == hash,
                    None => {
                        let Some(a) = db::asset_by_id(&l.conn, local)? else { continue };
                        match snapshot::hash_path(Path::new(&a.path)) {
                            Ok(bytes) => {
                                // 顺便补进库：下次快照/查重就不用再算一遍
                                let _ = db::set_hash_only(&l.conn, local, &bytes);
                                bytes == hash
                            }
                            Err(_) => false,
                        }
                    }
                };
                if same {
                    if let Some(mut a) = db::asset_by_id(&l.conn, local)? {
                        a.dir_id = gid(l.id, a.dir_id);
                        a.id = gid(l.id, local);
                        return Ok(Some((a.id, a, l.root.clone())));
                    }
                }
            }
        }
        Ok(None)
    }

    /// 导出用：把快照记成一份可读的清单（**只有哈希和元数据，不含任何图片字节**）
    pub fn snapshot_export(&self, id: i64) -> Result<snapshot::SnapFile, String> {
        let (l, local) = self.snap_split(id)?;
        let row = reg::snapshot(&l.conn, local)?
            .ok_or_else(|| "这个快照已经不在了".to_string())?;
        let items = reg::snapshot_items(&l.conn, local)?;
        Ok(snapshot::SnapFile::new(&row.name, items))
    }

    /// 导入：落在**当前库**；重名自动加 ` (2)`，**一条都不丢**
    /// （解析不了的那条会被跳过并计入跳过量）
    pub fn snapshot_import(&mut self, text: &str) -> Result<(i64, String, usize, usize), String> {
        let file = snapshot::SnapFile::parse(text)?;
        let home = self.cur_lib().map(|l| l.id).ok_or("还没有素材库")?;
        let mut name = file.name.trim().to_string();
        let mut n = 2;
        while self
            .snapshot_list()?
            .iter()
            .any(|s| lib_of(s.id) == home && s.name.eq_ignore_ascii_case(&name))
        {
            name = format!("{} ({n})", file.name.trim());
            n += 1;
            if n > 999 {
                return Err("同名快照太多了".into());
            }
        }
        let mut clean: Vec<snapshot::SnapItem> = Vec::new();
        let total = file.items.len();
        for mut it in file.items {
            // 只认 32 位十六进制的哈希；别的一律当坏数据丢掉（宁少不错）
            it.hash = it.hash.trim().to_ascii_lowercase();
            if snapshot::unhex(&it.hash).is_none() {
                continue;
            }
            clean.push(it);
        }
        let skipped = total - clean.len();
        let l = self.lib(home)?;
        let local = reg::snapshot_create(&l.conn, &name)?;
        let added = reg::snapshot_insert_items(&l.conn, local, &clean)?;
        Ok((gid(home, local), name, added, skipped))
    }

    /// 老版本：快照住在注册库、跨库存在。升级时按条目**归属的库**拆成每库一份。
    ///
    /// 归属判断：条目里记着 `lib`（当时的库根路径）→ 认这个库；路径对不上
    /// （库搬过/改名）就用内容哈希在现有库里找；再找不到就留在第一个库
    /// （宁可挂错位置，也不能把用户记下的书签丢掉）。
    fn migrate_snapshots_into_libs(&mut self) {
        let done: Option<String> = self
            .reg
            .query_row("SELECT v FROM meta WHERE k='snapshots_split'", [], |r| r.get(0))
            .ok();
        if done.as_deref() == Some("1") {
            return;
        }
        if self.libs.is_empty() {
            return;
        }
        let rows = reg::snapshots(&self.reg).unwrap_or_default();
        for row in rows {
            let items = reg::snapshot_items(&self.reg, row.id).unwrap_or_default();
            /* 按库分桶 */
            let mut buckets: std::collections::BTreeMap<i64, Vec<snapshot::SnapItem>> =
                std::collections::BTreeMap::new();
            for it in items {
                let lib_id = self
                    .lib_for_path(&it.lib)
                    .or_else(|| {
                        snapshot::unhex(&it.hash).and_then(|h| {
                            self.libs.iter().find_map(|l| {
                                db::ids_by_hash(&l.conn, &h)
                                    .ok()
                                    .filter(|v| !v.is_empty())
                                    .map(|_| l.id)
                            })
                        })
                    })
                    .unwrap_or(self.libs[0].id);
                buckets.entry(lib_id).or_default().push(it);
            }
            /* 空快照（建了还没往里加图）：条目分不出库，落到第一个库，
               不能因为"没有条目"就把它弄丢 */
            if buckets.is_empty() {
                buckets.insert(self.libs[0].id, Vec::new());
            }
            let mut landed: Vec<i64> = Vec::new();
            for (lib_id, items) in buckets {
                let Ok(l) = self.lib(lib_id) else { continue };
                /* 同名已存在就合并进去（拆库后可能出现两个同名快照） */
                let local = match reg::snapshot_create(&l.conn, &row.name) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let _ = reg::snapshot_insert_items(&l.conn, local, &items);
                landed.push(gid(lib_id, local));
            }
            /* 看板摆位（`snap:<旧 id>`）只在"整份都落到一个库"时能搬过去 */
            if landed.len() == 1 {
                let _ = self.reg.execute(
                    "UPDATE board_items SET board = ?1 WHERE board = ?2",
                    rusqlite::params![format!("snap:{}", landed[0]), format!("snap:{}", row.id)],
                );
            }
        }
        let _ = self.reg.execute(
            "INSERT INTO meta (k,v) VALUES ('snapshots_split','1') ON CONFLICT(k) DO UPDATE SET v='1'",
            [],
        );
    }
}

/* ------------------------------------------------------------ 标签：按库解析 */

/// 标签名 → (本库的 tag id 列表, 规则 JSON)。
///
/// **库里没有这个标签时必须给 `Some(vec![])`**（= 空结果），不能给 `None` ——
/// 在 db 层 `None` 表示"根本不过滤"，那样会把整个库的素材都列出来。
fn lib_tag_lookup(
    conn: &rusqlite::Connection,
    name: Option<&str>,
) -> (Option<Vec<i64>>, Option<String>) {
    let Some(name) = name else { return (None, None) };
    match reg::tags_meta(conn) {
        Ok(meta) => match meta.iter().find(|t| t.name == name) {
            Some(t) => (Some(vec![t.id]), t.rule.clone()),
            None => (Some(Vec::new()), None),
        },
        Err(_) => (Some(Vec::new()), None),
    }
}

/// 搜索词里的标签名模糊匹配 → 本库的 tag id 列表（标签就一屏，内存里 contains 足够）
fn lib_tag_ids_matching(conn: &rusqlite::Connection, needle: &str) -> Vec<i64> {
    let n = needle.to_lowercase();
    reg::tags_meta(conn)
        .map(|m| {
            m.iter()
                .filter(|t| t.name.to_lowercase().contains(&n))
                .map(|t| t.id)
                .collect()
        })
        .unwrap_or_default()
}

/* ---------------------------------------------------------------- 库文件搬运 */

/// 把 SQLite 库**完整**复制一份到 `dst`（含 WAL 里的最新提交）。
///
/// 为什么不用 `fs::copy`：库跑在 WAL 模式下，最近的写还在 `-wal` 里，
/// 单拷 `.db` 会丢最后一批改动（用户看到的就是"刚打的标签没了"）。
/// `VACUUM INTO` 走 SQLite 自己的读取路径，出来的是一个干净的单文件库。
fn copy_db(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败：{e}"))?;
    }
    let _ = std::fs::remove_file(dst); // VACUUM INTO 要求目标不存在
    let c = db::open_raw(src)?;
    c.execute("VACUUM INTO ?1", [dst.to_string_lossy().to_string()])
        .map_err(|e| format!("复制库失败：{e}"))?;
    Ok(())
}

/// 老版本的标签体系住在注册库里。第一次把一个**已存在**的库接进来时，
/// 把那份标签/分组复制进库内（id 原样保留 —— `asset_tags.tag_id` 指着它们）。
///
/// 只做一次：复制完就在库的 `meta` 里盖个章。否则用户把标签删空之后，
/// 下次打开又被老数据灌回来（"我删了它怎么又出现了"）。
fn seed_tags_from_registry(reg: &rusqlite::Connection, lib: &rusqlite::Connection) {
    if tags_seeded(lib) {
        return;
    }
    let has_own: i64 = lib
        .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
        .unwrap_or(0);
    if has_own == 0 {
        let _ = copy_tag_rows(reg, lib);
    }
    mark_tags_seeded(lib);
}

fn copy_tag_rows(from: &rusqlite::Connection, to: &rusqlite::Connection) -> Result<(), String> {
    let e = |x: rusqlite::Error| x.to_string();
    let mut st = from
        .prepare("SELECT id, name, color, sort, created_at FROM tag_groups")
        .map_err(e)?;
    let groups: Vec<(i64, String, Option<String>, i64, i64)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(e)?
        .flatten()
        .collect();
    for (id, name, color, sort, created) in groups {
        to.execute(
            "INSERT OR IGNORE INTO tag_groups (id, name, color, sort, created_at) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![id, name, color, sort, created],
        )
        .map_err(e)?;
    }
    let mut st = from
        .prepare("SELECT id, name, created_at, color, group_id, auto_rule FROM tags")
        .map_err(e)?;
    let tags: Vec<(i64, String, i64, Option<String>, Option<i64>, Option<String>)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .map_err(e)?
        .flatten()
        .collect();
    for (id, name, created, color, group_id, rule) in tags {
        to.execute(
            "INSERT OR IGNORE INTO tags (id, name, created_at, color, group_id, auto_rule)
             VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![id, name, created, color, group_id, rule],
        )
        .map_err(e)?;
    }
    Ok(())
}

fn tags_seeded(lib: &rusqlite::Connection) -> bool {
    lib.query_row("SELECT v FROM meta WHERE k='tags_seeded'", [], |r| r.get::<_, String>(0))
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

fn mark_tags_seeded(lib: &rusqlite::Connection) {
    let _ = lib.execute(
        "INSERT INTO meta (k,v) VALUES ('tags_seeded','1') ON CONFLICT(k) DO UPDATE SET v='1'",
        [],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-app-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 拆库之后**跨库合并分页**必须不重不漏 —— 这是整个 T28 里最容易出错的一块。
    #[test]
    fn merged_paging_across_libs_is_complete() {
        let d = tmp("merge");
        let a = d.join("lib-a");
        let b = d.join("lib-b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for i in 0..7 {
            std::fs::write(a.join(format!("a{}.png", i)), b"x").unwrap();
            std::fs::write(b.join(format!("b{}.png", i)), b"y").unwrap();
        }
        // 故意让两边 mtime 有交叉：同一秒里写，排序靠 id 兜底
        let mut app = App::open(&d).unwrap();
        app.scan(&a.to_string_lossy(), |_| {}).unwrap();
        app.scan(&b.to_string_lossy(), |_| {}).unwrap();
        assert_eq!(app.libs.len(), 2);
        assert_eq!(app.total().unwrap(), 14);

        for sort in ["mtime_desc", "name_asc", "size_desc"] {
            let mut got: Vec<i64> = Vec::new();
            let mut cursor: Option<(i64, i64)> = None;
            for _ in 0..20 {
                let q = model::AssetQuery {
                    dir_id: None,
                    recursive: true,
                    sort: Some(sort.into()),
                    cursor,
                    limit: Some(3),
                    ..Default::default()
                };
                let rows = app.list(&q).unwrap();
                if rows.is_empty() {
                    break;
                }
                let last = rows.last().unwrap();
                cursor = Some(match sort {
                    "name_asc" => (got.len() as i64 + rows.len() as i64, last.id),
                    "size_desc" => (last.size, last.id),
                    _ => (last.mtime, last.id),
                });
                got.extend(rows.into_iter().map(|r| r.id));
            }
            assert_eq!(got.len(), 14, "{sort}：翻完必须正好 14 条，实际 {}", got.len());
            let mut uniq = got.clone();
            uniq.sort();
            uniq.dedup();
            assert_eq!(uniq.len(), 14, "{sort}：不许有重复条目");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 新建的空文件夹必须立刻出现在目录树里（owner 2026-11-19：建完看不见）。
    #[test]
    fn new_empty_folder_shows_in_tree() {
        let d = tmp("mkdir");
        let lib = d.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let mut app = App::open(&d).unwrap();
        app.scan(&lib.to_string_lossy(), |_| {}).unwrap();
        let root = app.roots().unwrap().into_iter().next().unwrap();

        let made = {
            let ops = crate::fsops::FsOps::new(app.conn_for_path(&root.path).unwrap());
            ops.mkdir(std::path::Path::new(&root.path), "相册").unwrap()
        };
        /* 走 am_mkdir 同一条路：建完把空目录写进索引 */
        app.index_new_dir(&made.to_string_lossy(), &root.path).unwrap();

        let kids = app.tree(Some(root.id)).unwrap();
        assert!(
            kids.iter().any(|k| k.name == "相册"),
            "新建的空目录必须出现在目录树里，实际：{:?}",
            kids.iter().map(|k| &k.name).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 每个库一个文件；移除一个库只影响它自己（另一个库的素材/文件都还在）。
    #[test]
    fn split_libs_are_independent() {
        let d = tmp("split");
        let a = d.join("lib-a");
        let b = d.join("lib-b");
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.png"), b"x").unwrap();
        std::fs::write(a.join("sub/y.png"), b"y").unwrap();
        std::fs::write(b.join("w.png"), b"w").unwrap();

        let mut app = App::open(&d).unwrap();
        app.scan(&a.to_string_lossy(), |_| {}).unwrap();
        app.scan(&b.to_string_lossy(), |_| {}).unwrap();

        /* 0.3.9：库文件在**各自的库根**下，数据目录里不再有 libs/ */
        for root in [&a, &b] {
            let home = crate::libhome::db_for(&root.to_string_lossy());
            assert!(home.is_file(), "{} 里应该有 lib.db", root.display());
            assert_eq!(
                home.parent().map(|p| p.file_name().unwrap().to_string_lossy().to_string()),
                Some(crate::libhome::DIR.to_string()),
                "库文件必须放在 .AssetManager 里"
            );
        }
        assert!(!d.join("libs").exists(), "数据目录里不该再有 libs/（库跟着库根走）");

        // 全局 id 必须落到正确的库
        let roots = app.roots().unwrap();
        assert_eq!(roots.len(), 2);
        for r in &roots {
            let q = model::AssetQuery {
                dir_id: Some(r.id),
                recursive: true,
                limit: Some(50),
                ..Default::default()
            };
            let rows = app.list(&q).unwrap();
            assert!(rows.iter().all(|a| lib_of(a.id) == lib_of(r.id)), "素材必须属于所在库");
        }

        let removed = app.remove_lib(&a.to_string_lossy()).unwrap();
        assert!(removed >= 2);
        assert_eq!(app.libs.len(), 1);
        assert_eq!(app.total().unwrap(), 1, "只剩 B 库那张 w.png");
        // 磁盘：A 库的文件一个没动（移除库 ≠ 删文件）
        assert!(a.join("x.png").exists() && a.join("sub/y.png").exists());
        assert!(b.join("w.png").exists());
        /* 0.3.9 的关键行为变化：**移除库只摘注册，库里的 .AssetManager 原样留着**。
           以前是把库文件挪进 trash-libs，用户重新导入时等于建了个空库，
           标签评分全没（owner 2026-11-19 要求修掉的正是不这件事）。 */
        let home_a = crate::libhome::dir_for(&a.to_string_lossy());
        assert!(home_a.join(crate::libhome::DB).is_file(), "移除后库内索引必须还在");
        assert!(home_a.join("lib.json").is_file(), "库标识也要留着，重新导入才能认回旧 id");
        assert!(
            !d.join("trash-libs")
                .read_dir()
                .map(|mut r| r.next().is_some())
                .unwrap_or(false),
            "库内索引不该被搬去 trash-libs"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 标签体系**每库一份**（0.3.9）：在 A 库打的标签不会出现在 B 库的侧栏里，
    /// 但按标签名筛选仍然能跨库命中（名字是稳定的，id 是各库自己的）。
    #[test]
    fn tags_follow_each_lib() {
        let d = tmp("libtag");
        let a = d.join("lib-a");
        let b = d.join("lib-b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.png"), b"x").unwrap();
        std::fs::write(b.join("w.png"), b"w").unwrap();

        let mut app = App::open(&d).unwrap();
        app.scan(&a.to_string_lossy(), |_| {}).unwrap();
        app.scan(&b.to_string_lossy(), |_| {}).unwrap();

        let all: Vec<model::Asset> = app
            .list(&model::AssetQuery { recursive: true, limit: Some(10), ..Default::default() })
            .unwrap();
        assert_eq!(all.len(), 2);
        let ia = all.iter().find(|a| a.name == "x.png").unwrap().id;
        let ib = all.iter().find(|a| a.name == "w.png").unwrap().id;

        // 两个库各打一次同名标签（内部各建一份，互不影响）
        app.tag_assets(&[ia], "跨库标签").unwrap();
        app.tag_assets(&[ib], "跨库标签").unwrap();
        app.set_tag_color("跨库标签", Some("teal")).unwrap(); // 落在当前库

        // ① 侧栏只显示**当前库**的标签：A 库 1 条命中
        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].count, 1, "当前库只有一个素材挂了这个标签");
        assert_eq!(tags[0].color.as_deref(), Some("teal"));

        // ② 切到 B 库：同名标签还在，但计数是 B 库自己的，
        //    颜色是 B 库自己那份（没设过 = None，说明确实是两套数据）
        let root_b = b.to_string_lossy().to_string();
        assert!(app.set_current_lib(&root_b).unwrap(), "切库必须报告变化");
        let tags_b = app.tags().unwrap();
        assert_eq!(tags_b.len(), 1);
        assert_eq!(tags_b[0].count, 1);
        assert_eq!(tags_b[0].color, None, "B 库的标签不该继承 A 库的颜色");

        // ③ 按标签名筛选仍然跨库命中（名字稳定，id 按库换算）
        let q = model::AssetQuery {
            recursive: true,
            tag: Some("跨库标签".into()),
            limit: Some(10),
            ..Default::default()
        };
        assert_eq!(app.list(&q).unwrap().len(), 2);

        // ④ 每素材的标签名（id 要换成全局 id）
        let per = app.tags_for(&[ia, ib]).unwrap();
        assert_eq!(per.len(), 2);
        assert_eq!(per[0].1, vec!["跨库标签".to_string()]);

        // ⑤ 在 B 库删标签，A 库那条不受影响
        app.delete_tag("跨库标签").unwrap();
        assert!(app.tags().unwrap().is_empty(), "B 库删完就空了");
        assert!(app.set_current_lib(&a.to_string_lossy()).unwrap());
        assert_eq!(app.tags().unwrap().len(), 1, "A 库的标签不能被 B 库的删除影响");
        assert_eq!(app.tags_for(&[ia]).unwrap()[0].1, vec!["跨库标签".to_string()]);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 缩略图缓存每库一份（0.3.10）：生成在 `<库根>/.AssetManager/thumbs`，
    /// 老版本散在数据目录里的缓存按 `thumb_lru` 自动搬回来。
    #[test]
    fn thumbs_follow_the_library() {
        let d = tmp("thumbhome");
        let lib = d.join("lib");
        img_at(&lib.join("a.png"), 40, 40, [10, 120, 200]);

        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&lib.to_string_lossy(), |_| {}).unwrap();
        let g = app
            .list(&model::AssetQuery { limit: Some(10), ..Default::default() })
            .unwrap()[0]
            .id;
        let out = app.ensure_thumb(g, 256).unwrap().expect("应当生成缩略图");
        let in_lib = app.libs[0].thumbs.file_for(&out.key);
        assert!(in_lib.is_file(), "缩略图必须落在库自己的目录里：{}", in_lib.display());
        assert!(
            crate::libhome::is_inside_home(&in_lib),
            "缩略图必须在 .AssetManager 里，不能散在库的其他地方"
        );

        /* 手造"老版本状态"：文件搬回数据目录的公共缓存 */
        let legacy_root = d.join("data").join("cache").join("thumbs");
        let legacy = thumb::ThumbStore::new(legacy_root.clone());
        let old_path = legacy.file_for(&out.key);
        std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        std::fs::rename(&in_lib, &old_path).unwrap();
        drop(app);

        /* 再打开：应当按 thumb_lru 搬回库内 */
        let app = App::open(d.join("data")).unwrap();
        let back = app.libs[0].thumbs.file_for(&out.key);
        assert!(back.is_file(), "老缓存要搬回库内：{}", back.display());
        assert!(!old_path.is_file(), "搬完老位置不该再留一份（白占空间）");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 缓存根跟着**壳层给的目录**走（owner 2026-11-19：缓存放安装目录，别占数据目录）。
    /// 两条要点：老缓存要跟过去（不然用户白等一次重建）；目录写不进去必须退回数据目录。
    #[test]
    fn cache_follows_the_install_dir() {
        let d = tmp("cacheroot");
        let data = d.join("data");
        let cache = d.join("install").join("cache");
        std::fs::create_dir_all(data.join("cache").join("thumbs")).unwrap();
        std::fs::write(data.join("cache").join("thumbs").join("x.webp"), b"1").unwrap();

        let app = App::open_with_cache(&data, Some(cache.clone())).unwrap();
        assert_eq!(app.cache_dir, cache, "给了可写的缓存目录就必须用它");
        assert!(
            cache.join("thumbs").join("x.webp").is_file(),
            "老缓存要跟着搬过来，不能让用户白重建一次"
        );
        assert!(!data.join("cache").exists(), "搬完老目录不该留着");
        drop(app);

        /* 路径被一个文件挡住 = 写不进去 → 必须退回数据目录，而不是打不开 */
        let blocked = d.join("blocked");
        std::fs::write(&blocked, b"x").unwrap();
        let app = App::open_with_cache(&data, Some(blocked.join("cache"))).unwrap();
        assert_eq!(app.cache_dir, data.join("cache"), "不可写就要退回数据目录");

        let _ = std::fs::remove_dir_all(&d);
    }

    /* ---------------- 库元数据跟着库走（0.3.9） ----------------
       这三条是 owner 2026-11-19 提的验收点，任何一条红了都说明「数据会丢」。 */

    /// 扫描完就把索引写进 `<库根>/.AssetManager/lib.db`，而且那个目录**不许进素材列表**。
    #[test]
    fn lib_metadata_lives_in_the_library() {
        let d = tmp("home");
        let root = d.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();

        let mut app = App::open(&d).unwrap();
        app.scan(&root.to_string_lossy(), |_| {}).unwrap();

        let home = crate::libhome::dir_for(&root.to_string_lossy());
        assert!(home.join(crate::libhome::DB).is_file(), "索引必须在 库根/.AssetManager/lib.db");
        assert!(home.join("lib.json").is_file(), "库标识要写进去，重新导入才认得出旧 id");

        /* 元数据目录不能被当成素材/子目录扫进来 */
        let rows = app
            .list(&model::AssetQuery { recursive: true, limit: Some(50), ..Default::default() })
            .unwrap();
        assert_eq!(rows.len(), 1, "只有 a.png，.AssetManager 不该出现在库里");
        assert!(rows.iter().all(|r| !r.name.ends_with(".db")));

        /* 库根下只允许出现这一个自家目录 */
        let extra: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with('.') && n != crate::libhome::DIR)
            .collect();
        assert!(extra.is_empty(), "库根下多出了别的自家文件：{extra:?}");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 0.8.x～0.3.8 的标签住在注册库里：升级后要**复制进每个库**，
    /// 否则 `asset_tags.tag_id` 指向不存在的标签 —— 界面上就是"标签全没了"。
    #[test]
    fn global_tags_are_seeded_into_the_library() {
        let d = tmp("seedtag");
        let root = d.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();

        /* 先正常用一次，造出一个带标签的库 */
        let (lib_id, g) = {
            let mut app = App::open(&d).unwrap();
            app.scan(&root.to_string_lossy(), |_| {}).unwrap();
            let g = app.list(&model::AssetQuery { recursive: true, ..Default::default() }).unwrap()[0].id;
            app.tag_assets(&[g], "老标签").unwrap();
            (app.libs[0].id, g)
        };

        /* 手造"老版本状态"：标签搬回注册库，库里的 tags 表清空 + 去掉已灌标记 */
        {
            let conn = crate::db::open(&crate::libhome::db_for(&root.to_string_lossy())).unwrap();
            conn.execute("DELETE FROM tags", []).unwrap();
            conn.execute("DELETE FROM meta WHERE k='tags_seeded'", []).unwrap();
            let reg = crate::reg::open(&d.join("index.db")).unwrap();
            crate::reg::ensure_tag(&reg, "老标签").unwrap();
        }

        /* 再打开：必须自动灌回库里，标签名和关联行都对得上 */
        let app = App::open(&d).unwrap();
        assert_eq!(app.libs[0].id, lib_id);
        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1, "老标签必须被灌回库内");
        assert_eq!(tags[0].name, "老标签");
        assert_eq!(tags[0].count, 1, "关联行还要能对上（tag_id 必须一致）");
        assert_eq!(app.tags_for(&[g]).unwrap()[0].1, vec!["老标签".to_string()]);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 老版本把库文件放在 `<数据目录>/libs/lib-<id>.db`：
    /// 升级后要**自动搬进库目录**，而且一条素材都不许丢。
    #[test]
    fn legacy_lib_file_is_moved_into_the_library() {
        let d = tmp("legacyhome");
        let root = d.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();

        let lib_id = {
            let mut app = App::open(&d).unwrap();
            app.scan(&root.to_string_lossy(), |_| {}).unwrap();
            let id = app.libs[0].id;
            /* 先关连接再动文件：SQLite 开着的时候 Windows 不让改名 */
            drop(app);
            /* 手造"老版本布局"：库文件搬回数据目录，注册行写相对路径 */
            let home_db = crate::libhome::db_for(&root.to_string_lossy());
            let legacy_dir = d.join("libs");
            std::fs::create_dir_all(&legacy_dir).unwrap();
            let legacy = legacy_dir.join(format!("lib-{id}.db"));
            std::fs::rename(&home_db, &legacy).unwrap();
            let _ = std::fs::remove_dir_all(crate::libhome::dir_for(&root.to_string_lossy()));
            let reg = crate::reg::open(&d.join("index.db")).unwrap();
            crate::reg::add_lib(&reg, &root.to_string_lossy(), &format!("libs/lib-{id}.db"))
                .unwrap();
            id
        };

        /* 再打开一次：应当自动搬到 .AssetManager 下，素材还在 */
        let app = App::open(&d).unwrap();
        let home_db = crate::libhome::db_for(&root.to_string_lossy());
        assert!(home_db.is_file(), "老库文件必须被搬进库目录");
        assert_eq!(app.libs.len(), 1);
        assert_eq!(app.libs[0].id, lib_id, "搬运不许换 id（快照按全局 id 记）");
        assert_eq!(app.total().unwrap(), 1, "搬运后素材数不能变");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 移除库 → 重新导入同一个文件夹：标签 / 评分 必须还在（这是以前真丢过数据的路径）。
    #[test]
    fn reimport_keeps_tags_and_rating() {
        let d = tmp("reimport");
        let root = d.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();
        let root_s = root.to_string_lossy().to_string();

        let (lib_id, gid) = {
            let mut app = App::open(&d).unwrap();
            app.scan(&root_s, |_| {}).unwrap();
            let g = app.list(&model::AssetQuery { recursive: true, ..Default::default() }).unwrap()[0].id;
            app.tag_assets(&[g], "客户终稿").unwrap();
            app.set_rating(&[g], 5).unwrap();
            app.set_note(g, "改过的第三版").unwrap();
            let id = app.libs[0].id;
            /* 用户点了「从素材库移除」 */
            app.remove_lib(&root_s).unwrap();
            assert_eq!(app.libs.len(), 0);
            (id, g)
        };

        /* 移除只是摘注册：库里的元数据必须原样留着 */
        assert!(crate::libhome::db_for(&root_s).is_file());

        /* 重新导入同一个文件夹 */
        let mut app = App::open(&d).unwrap();
        app.scan(&root_s, |_| {}).unwrap();
        assert_eq!(app.libs.len(), 1);
        assert_eq!(app.libs[0].id, lib_id, "重新导入要认回原来的库 id");

        let a = app.list(&model::AssetQuery { recursive: true, ..Default::default() }).unwrap();
        assert_eq!(a.len(), 1);
        let g = gid; // 同一张图，id 由库 id + 库内 id 决定，应当完全一样
        assert_eq!(a[0].id, g, "重新导入后素材的全局 id 不能变（快照/分组靠它）");
        assert_eq!(a[0].rating, 5, "评分丢了");
        assert_eq!(a[0].note, "改过的第三版", "备注丢了");
        assert_eq!(
            app.tags_for(&[g]).unwrap(),
            vec![(g, vec!["客户终稿".to_string()])],
            "标签丢了"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// T34：规则标签按"现场翻译成 SQL"来筛，不需要物化关联行。
    #[test]
    fn rule_tag_filters_by_dir_and_ext() {
        let d = tmp("rule");
        let lib = d.join("lib");
        std::fs::create_dir_all(lib.join("ref")).unwrap();
        std::fs::write(lib.join("a.psd"), b"x").unwrap();
        std::fs::write(lib.join("b.png"), b"y").unwrap();
        std::fs::write(lib.join("ref/c.psd"), b"z").unwrap();

        let mut app = App::open(&d).unwrap();
        app.scan(&lib.to_string_lossy(), |_| {}).unwrap();

        // 建规则：扩展名 psd
        let rule = reg::AutoRule { ext: vec!["psd".into()], ..Default::default() };
        app.set_tag_rule("PS 源文件", Some(&rule)).unwrap();
        let q = model::AssetQuery {
            recursive: true,
            tag: Some("PS 源文件".into()),
            limit: Some(10),
            ..Default::default()
        };
        assert_eq!(app.list(&q).unwrap().len(), 2);
        let tags = app.tags().unwrap();
        assert_eq!(tags[0].count, 2, "规则标签的计数要现算出来");

        // 再加目录条件：只命中 ref 下的那个
        let rule = reg::AutoRule {
            ext: vec!["psd".into()],
            dir: lib.join("ref").to_string_lossy().to_string(),
            ..Default::default()
        };
        app.set_tag_rule("只 ref", Some(&rule)).unwrap();
        let q = model::AssetQuery {
            recursive: true,
            tag: Some("只 ref".into()),
            limit: Some(10),
            ..Default::default()
        };
        let rows = app.list(&q).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "c.psd");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 灵感板粘贴那条链（0.11.0）：
    /// 图片落进**截图目录** → 扫描入库 → 按**绝对路径**点查素材 id → 打上这块板的标签 → 按标签能查回来。
    /// 这条链断哪一环，用户看到的现象都是「粘进去了，下次开板那张图不在」，所以值得单测钉住。
    #[test]
    fn pasted_file_is_lookupable_by_path_and_tagged() {
        let d = tmp("paste");
        let shots = d.join("screenshots");
        std::fs::create_dir_all(&shots).unwrap();
        // 真的写一张能解码的 PNG（不是几个随便的字节）：扫描要读文件头拿宽高
        let f = shots.join("粘贴 2026-11-17 12-00-00.png");
        image::RgbaImage::from_pixel(4, 3, image::Rgba([200, 30, 30, 255]))
            .save(&f)
            .unwrap();

        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&shots.to_string_lossy(), |_| {}).unwrap();

        let path = f.to_string_lossy().to_string();
        let lib = app.lib_for_path(&path).expect("截图目录扫描后应当就是库根");
        let local = db::asset_id_by_path(app.conn_for_path(&path).unwrap(), &path)
            .unwrap()
            .expect("按绝对路径必须能点查到刚入库的那张");
        let g = gid(lib, local);
        app.tag_assets(&[g], "灵感板·参考").unwrap();

        let q = model::AssetQuery {
            tag: Some("灵感板·参考".into()),
            limit: Some(10),
            ..Default::default()
        };
        let rows = app.list(&q).unwrap();
        assert_eq!(rows.len(), 1, "粘贴后打上的标签必须能把它筛出来");
        assert_eq!(rows[0].id, g);
        assert_eq!((rows[0].width, rows[0].height), (Some(4), Some(3)));

        let _ = std::fs::remove_dir_all(&d);
    }

    /* ---------------- 快照（0.12.0） ----------------
       这几条守的是**用户唯一的硬要求**：改名 / 换库之后，快照里的图还必须认得出来。
       身份是内容哈希，所以下面每一步都不看文件名、不看 id、不看路径。 */

    /// 「把整个标签加进分组」：标签下有几张就进去几张，而且只是建关联 ——
    /// 原文件一个不动、素材行也不复制（owner 2026-11-19 的验收点）。
    #[test]
    fn tag_batch_lands_in_collection() {
        let d = tmp("tagcoll");
        let lib = d.join("lib");
        for i in 0..3u8 {
            img_at(&lib.join(format!("a{i}.png")), 20, 20, [i * 40, 100, 200]);
        }
        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&lib.to_string_lossy(), |_| {}).unwrap();
        let all = app
            .list(&model::AssetQuery {
                limit: Some(10),
                ..Default::default()
            })
            .unwrap();
        let picked: Vec<i64> = all.iter().take(2).map(|a| a.id).collect();
        app.tag_assets(&picked, "参考").unwrap();

        let ids = app.ids_by_tag("参考").unwrap();
        assert_eq!(ids.len(), 2, "标签下有几张就该取回几张");

        let coll = app.coll_create("待选").unwrap();
        assert_eq!(app.coll_add(coll, &ids).unwrap(), 2);
        let in_coll = app
            .list(&model::AssetQuery {
                collection: Some(coll),
                limit: Some(10),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(in_coll.len(), 2, "加进分组的那两张要能在分组筛里看到");
        assert_eq!(
            app.list(&model::AssetQuery { limit: Some(10), ..Default::default() })
                .unwrap()
                .len(),
            3,
            "不许复制素材行：库里仍然只有 3 张"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    fn img_at(path: &std::path::Path, w: u32, h: u32, rgb: [u8; 3]) {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        // .jpg 不支持 RGBA，统一写 RGB8（PNG 写 RGB8 也一样能读）
        image::RgbImage::from_pixel(w, h, image::Rgb(rgb)).save(path).unwrap();
    }

    fn png_at(path: &std::path::Path, r: u8, g: u8, b: u8) {
        img_at(path, 6, 4, [r, g, b]);
    }

    #[test]
    fn snapshot_survives_rename_and_library_move() {
        let d = tmp("snap-move");
        let lib_a = d.join("lib-a");
        let f = lib_a.join("原图.png");
        png_at(&f, 10, 20, 30);

        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&lib_a.to_string_lossy(), |_| {}).unwrap();
        let snap = app.snapshot_create("参考板").unwrap();
        let ids: Vec<i64> = app
            .list(&model::AssetQuery { limit: Some(50), ..Default::default() })
            .unwrap()
            .iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(app.snapshot_add_assets(snap, &ids).unwrap().added, 1);

        let r = app.snapshot_resolve(snap).unwrap();
        assert_eq!((r.found, r.missing), (1, 0), "刚加进去就该找得到");

        // ① 改名（内容不变）：重扫之后老行 missing、新行 hash_x 还是 NULL
        let renamed = lib_a.join("改了名字.png");
        std::fs::rename(&f, &renamed).unwrap();
        app.scan(&lib_a.to_string_lossy(), |_| {}).unwrap();
        let r = app.snapshot_resolve(snap).unwrap();
        assert_eq!((r.found, r.missing), (1, 0), "改名之后必须还认得出（靠内容，不靠文件名）");
        assert_eq!(r.items[0].name, "改了名字.png", "条目里记的位置要跟着更新");

        // ② 把图**挪**到另一个库（A 里那份删掉）：靠内容仍要认得出来，
        //    并把条目里的位置更新成现在这份。
        //    注意（0.3.10 起）：快照跟着库走 —— 把库整个移除后它那份快照也跟着
        //    "下架"（数据在库目录里，重新导入就回来），所以这里用
        //    "同库删文件 + 另一库放进来"来模拟真正的搬迁。
        let lib_b = d.join("lib-b");
        std::fs::create_dir_all(&lib_b).unwrap();
        std::fs::copy(&renamed, lib_b.join("全新的名字.png")).unwrap();
        std::fs::remove_file(&renamed).unwrap();
        app.scan(&lib_a.to_string_lossy(), |_| {}).unwrap();
        app.scan(&lib_b.to_string_lossy(), |_| {}).unwrap();
        let r = app.snapshot_resolve(snap).unwrap();
        assert_eq!((r.found, r.missing), (1, 0), "换到另一个素材库之后必须还找得到");
        assert_eq!(r.items[0].name, "全新的名字.png");
        assert_eq!(r.items[0].lib, lib_b.to_string_lossy().to_string());

        // ③ 内容真的不是同一张图 → 才算找不到（同名也不行）
        std::fs::remove_file(lib_b.join("全新的名字.png")).unwrap();
        img_at(&lib_b.join("全新的名字.png"), 9, 7, [200, 10, 10]);
        app.scan(&lib_b.to_string_lossy(), |_| {}).unwrap();
        let r = app.snapshot_resolve(snap).unwrap();
        assert_eq!((r.found, r.missing), (0, 1), "同名的另一张图不许被当成同一张");

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn snapshot_export_import_roundtrip_keeps_identity() {
        let d = tmp("snap-io");
        let lib = d.join("lib");
        png_at(&lib.join("a.png"), 1, 2, 3);
        png_at(&lib.join("b.png"), 4, 5, 6);

        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&lib.to_string_lossy(), |_| {}).unwrap();
        let snap = app.snapshot_create("导出板").unwrap();
        let ids: Vec<i64> = app
            .list(&model::AssetQuery { limit: Some(50), ..Default::default() })
            .unwrap()
            .iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(app.snapshot_add_assets(snap, &ids).unwrap().added, 2);

        let file = app.snapshot_export(snap).unwrap();
        assert_eq!(file.items.len(), 2);
        assert_eq!(file.kind, "snapshot");
        let text = serde_json::to_string_pretty(&file).unwrap();
        // 导出的是**编号**，不是图片字节
        assert!(!text.contains("iVBORw0KGgo"), "快照文件里不许出现图片数据");

        // 导回来：重名自动加后缀，条目一条不丢，还能对上现在库里的图
        let (id2, name, added, skipped) = app.snapshot_import(&text).unwrap();
        assert_eq!((added, skipped), (2, 0));
        assert_eq!(name, "导出板 (2)", "重名要自动区分，不能覆盖原来那块");
        let r = app.snapshot_resolve(id2).unwrap();
        assert_eq!((r.found, r.missing), (2, 0));

        // 坏条目跳过而不是整份失败
        let bad = r#"{"app":"AssetManager","kind":"snapshot","version":1,"name":"半坏",
            "items":[{"hash":"not-a-hash","name":"x.png"},{"hash":"00112233445566778899aabbccddeeff","name":"y.png"}]}"#;
        let (_, _, added, skipped) = app.snapshot_import(bad).unwrap();
        assert_eq!((added, skipped), (1, 1));

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn snapshot_add_tag_takes_every_asset_under_it() {
        let d = tmp("snap-tag");
        let lib = d.join("lib");
        png_at(&lib.join("a.png"), 9, 9, 9);
        png_at(&lib.join("b.png"), 8, 8, 8);
        png_at(&lib.join("c.jpg"), 7, 7, 7);

        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&lib.to_string_lossy(), |_| {}).unwrap();
        let ids: Vec<i64> = app
            .list(&model::AssetQuery { limit: Some(50), ..Default::default() })
            .unwrap()
            .iter()
            .map(|a| a.id)
            .collect();
        app.tag_assets(&ids, "场景参考").unwrap();

        let snap = app.snapshot_create("场景板").unwrap();
        let res = app.snapshot_add_tag(snap, "场景参考").unwrap();
        assert_eq!((res.added, res.total), (3, 3), "标签下有几张就进几张");
        // 幂等：再点一次不重复
        let again = app.snapshot_add_tag(snap, "场景参考").unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.already, 3);

        // 文件一个都没动
        assert!(lib.join("a.png").is_file() && lib.join("c.jpg").is_file());

        let _ = std::fs::remove_dir_all(&d);
    }

    /* ---------------- 快照筛选 / 合并（0.1.18，P2.9） ---------------- */

    /// 主列表按快照筛：只列快照里那几张，且**跨库**也对（哈希换算 + 拆库查询）。
    #[test]
    fn snapshot_filter_lists_only_its_members() {
        let d = tmp("snap-filter");
        let lib_a = d.join("lib-a");
        let lib_b = d.join("lib-b");
        png_at(&lib_a.join("a1.png"), 1, 2, 3);
        png_at(&lib_a.join("a2.png"), 4, 5, 6);
        png_at(&lib_b.join("b1.png"), 7, 8, 9);

        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&lib_a.to_string_lossy(), |_| {}).unwrap();
        app.scan(&lib_b.to_string_lossy(), |_| {}).unwrap();
        let all = app
            .list(&model::AssetQuery { limit: Some(50), ..Default::default() })
            .unwrap();
        assert_eq!(all.len(), 3, "两个库各扫一遍后应当是 3 张");

        // 从**两个库各取一张**放进快照：这样同时验证"跨库"这件事
        let mut picked: Vec<i64> = Vec::new();
        for root in [lib_a.to_string_lossy().to_string(), lib_b.to_string_lossy().to_string()] {
            if let Some(a) = all.iter().find(|a| a.path.starts_with(&root)) {
                picked.push(a.id);
            }
        }
        assert_eq!(picked.len(), 2);
        let snap = app.snapshot_create("筛选板").unwrap();
        app.snapshot_add_assets(snap, &picked).unwrap();

        let rows = app
            .list(&model::AssetQuery { snapshot: Some(snap), limit: Some(50), ..Default::default() })
            .unwrap();
        let mut got: Vec<i64> = rows.iter().map(|a| a.id).collect();
        got.sort_unstable();
        let mut want = picked.clone();
        want.sort_unstable();
        assert_eq!(got, want, "按快照筛必须把两个库里的那两张都列出来，且一张不多");

        // 第二页（游标）不能因为换了条路就漏 / 重
        let p1 = app
            .list(&model::AssetQuery {
                snapshot: Some(snap),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p1.len(), 1);
        let last = &p1[0];
        let p2 = app
            .list(&model::AssetQuery {
                snapshot: Some(snap),
                limit: Some(1),
                cursor: Some((last.mtime, last.id)),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p2.len(), 1);
        assert_ne!(p2[0].id, last.id, "翻页不能把同一张再给一次");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 快照按库拆（0.3.10）：清单只列本库的；移除库 = 快照跟着下架，
    /// 重新导入这个库 = 快照原样回来（数据在 `.AssetManager/lib.db` 里）。
    #[test]
    fn snapshots_follow_the_library() {
        let d = tmp("snap-lib");
        let lib_a = d.join("lib-a");
        let lib_b = d.join("lib-b");
        png_at(&lib_a.join("a.png"), 10, 10, 10);
        png_at(&lib_b.join("b.png"), 20, 20, 20);

        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&lib_a.to_string_lossy(), |_| {}).unwrap();
        app.scan(&lib_b.to_string_lossy(), |_| {}).unwrap();

        /* 两个库各建一个**同名**快照：拆库之后它们是两份独立的东西 */
        for root in [&lib_a, &lib_b] {
            app.set_current_lib(&root.to_string_lossy()).unwrap();
            let snap = app.snapshot_create("交付稿").unwrap();
            assert_eq!(lib_of(snap), app.lib_for_path(&root.to_string_lossy()).unwrap());
            let ids: Vec<i64> = app
                .list(&model::AssetQuery { limit: Some(50), ..Default::default() })
                .unwrap()
                .iter()
                .filter(|a| lib_of(a.id) == lib_of(snap))
                .map(|a| a.id)
                .collect();
            assert_eq!(ids.len(), 1);
            assert_eq!(app.snapshot_add_assets(snap, &ids).unwrap().added, 1);
        }
        assert_eq!(app.snapshot_list().unwrap().len(), 2, "两个库各一份同名快照");

        /* 移除 A：它的快照跟着下架（B 那条还在） */
        let snap_a = app
            .snapshot_list()
            .unwrap()
            .into_iter()
            .find(|s| lib_of(s.id) == app.lib_for_path(&lib_a.to_string_lossy()).unwrap())
            .unwrap()
            .id;
        app.remove_lib(&lib_a.to_string_lossy()).unwrap();
        assert_eq!(app.snapshot_list().unwrap().len(), 1, "移除库后只剩 B 的快照");
        assert!(app.snapshot_get(snap_a).is_err() || app.snapshot_get(snap_a).unwrap().is_none());

        /* 重新导入 A：快照回来，条目也还在 */
        app.scan(&lib_a.to_string_lossy(), |_| {}).unwrap();
        let back = app.snapshot_list().unwrap();
        assert_eq!(back.len(), 2, "重新导入后两份快照都在");
        let again = back.iter().find(|s| s.count == 1).unwrap();
        assert_eq!(app.snapshot_items(again.id).unwrap().len(), 1, "快照条目不能丢");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 合并 / 差集：结果落到**新快照**，原件一条不动。
    #[test]
    fn snapshot_combine_union_and_diff() {
        let d = tmp("snap-combine");
        let lib = d.join("lib");
        png_at(&lib.join("a.png"), 10, 10, 10);
        png_at(&lib.join("b.png"), 20, 20, 20);
        png_at(&lib.join("c.png"), 30, 30, 30);

        let mut app = App::open(d.join("data")).unwrap();
        app.scan(&lib.to_string_lossy(), |_| {}).unwrap();
        let ids: Vec<i64> = app
            .list(&model::AssetQuery { limit: Some(50), ..Default::default() })
            .unwrap()
            .iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids.len(), 3);

        let a = app.snapshot_create("A").unwrap();
        app.snapshot_add_assets(a, &ids[0..2]).unwrap(); // a + b
        let b = app.snapshot_create("B").unwrap();
        app.snapshot_add_assets(b, &ids[1..3]).unwrap(); // b + c

        // 并集：3 条（b 只算一次）
        let (u, total, added) = app.snapshot_combine(&[a, b], "A∪B", "union").unwrap();
        assert_eq!((total, added), (3, 3));
        assert_eq!(app.snapshot_get(u).unwrap().unwrap().name, "A∪B");

        // 差集：A − B 应当正好是「A 有、B 没有」的那一张
        let names_of = |app: &App, id: i64| -> std::collections::BTreeSet<String> {
            app.snapshot_items(id).unwrap().into_iter().map(|i| i.name).collect()
        };
        let (an, bn) = (names_of(&app, a), names_of(&app, b));
        let expect: Vec<String> = an.difference(&bn).cloned().collect();
        assert_eq!(expect.len(), 1);
        let (df, total, _) = app.snapshot_combine(&[a, b], "A−B", "diff").unwrap();
        assert_eq!(total, 1);
        let items = app.snapshot_items(df).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, expect[0], "差集只能留下 A 独有的那一张");

        // 原件没被改
        assert_eq!(app.snapshot_items(a).unwrap().len(), 2);
        assert_eq!(app.snapshot_items(b).unwrap().len(), 2);

        // 重名不覆盖
        let (_u2, _, _) = app.snapshot_combine(&[a, b], "A∪B", "union").unwrap();
        assert!(app
            .snapshot_list()
            .unwrap()
            .iter()
            .any(|s| s.name == "A∪B (2)"));

        let _ = std::fs::remove_dir_all(&d);
    }
}
