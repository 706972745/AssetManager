//! am-core —— AssetManager 底层核心。
//!
//! 这个 crate **不依赖 Tauri**，可以独立 `cargo test` / 跑 CLI 冒烟。
//! Tauri 壳只负责把这里的能力暴露成命令 + 自定义协议。
//!
//! # 多库（T28）
//!
//! 每个素材库一个 SQLite 文件（`<数据目录>/libs/lib-<id>.db`），
//! 外加一个注册库（`<数据目录>/index.db`）记住「挂了哪几个库 / 标签体系」。
//! 对界面暴露的 id 是 `lib << 40 | 库内 id`（见 `ids`），所以前端完全无感。
//!
//! 跨库的操作（列全部、标签计数、撤销选库）都在这里收口 —— 别让命令层自己拼。

pub mod autotag;
pub mod clipimg;
pub mod dataroot;
pub mod db;
pub mod dupes;
pub mod formats;
pub mod fsops;
pub mod ids;
pub mod imageinfo;
pub mod libs_migrate;
pub mod model;
pub mod palette;
pub mod preview;
pub mod refs;
pub mod reg;
pub mod scan;
pub mod screen;
pub mod shellthumb;
pub mod thumb;
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
}

pub struct App {
    /// 注册库：库清单 + 标签体系 + 自动规则
    pub reg: rusqlite::Connection,
    pub libs: Vec<Lib>,
    pub data_dir: PathBuf,
    pub thumbs: thumb::ThumbStore,
}

impl App {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, String> {
        let data_dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let reg_path = data_dir.join("index.db");
        let mut reg = reg::open(&reg_path)?;

        // 0.7.x 及以前是"单库多根"：`index.db` 里既有库清单也有 assets/dirs。
        // 手上这份如果是那种库，先原地拆成一堆库文件，再继续往下走。
        libs_migrate::maybe_migrate(&mut reg, &data_dir)?;

        let thumbs = thumb::ThumbStore::new(data_dir.join("cache").join("thumbs"));
        std::fs::create_dir_all(&thumbs.root).map_err(|e| e.to_string())?;

        let mut app = Self { reg, libs: Vec::new(), data_dir, thumbs };
        app.reload_libs();
        Ok(app)
    }

    /// 按注册库里的清单把每个库文件打开（坏库留证后跳过，不让一个库拖垮启动）。
    pub fn reload_libs(&mut self) {
        self.libs.clear();
        let rows = reg::libs(&self.reg).unwrap_or_default();
        for row in rows {
            let rel = if row.file.is_empty() {
                reg::lib_file(&self.data_dir, row.id)
            } else {
                self.data_dir.join(&row.file)
            };
            match db::open(&rel) {
                Ok(conn) => self.libs.push(Lib { id: row.id, root: row.root, conn }),
                Err(e) => eprintln!("打开库 {} 失败（已跳过）：{}", rel.display(), e),
            }
        }
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
        let store = thumb::ThumbStore::new(self.thumbs.root.clone());
        let l = self.lib_mut(lib_id)?;
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
    /// `scan` 与「加文件夹」都会先走这一句 —— 库文件是扫描的落点。
    pub fn ensure_lib(&mut self, root: &str) -> Result<i64, String> {
        let root = norm_keep_case(root);
        if let Some(l) = self.libs.iter().find(|l| norm_path(&l.root) == norm_path(&root)) {
            return Ok(l.id);
        }
        let existing = reg::libs(&self.reg)?
            .into_iter()
            .find(|r| norm_path(&r.root) == norm_path(&root));
        let id = match existing {
            Some(row) => {
                let rel = if row.file.is_empty() {
                    reg::lib_file(&self.data_dir, row.id)
                } else {
                    self.data_dir.join(&row.file)
                };
                let conn = db::open(&rel)?;
                let root_owned = row.root.clone();
                self.libs.push(Lib { id: row.id, root: row.root, conn });
                // 注册库里有、内存里没有（比如上次打开失败）→ 记下 file 后重开
                if row.file.is_empty() {
                    let rel_s = format!("libs/lib-{}.db", row.id);
                    reg::add_lib(&self.reg, &root_owned, &rel_s)?;
                }
                row.id
            }
            None => {
                // 先用占位 file 拿到 id，再按 id 生成正式文件名（文件名要稳定，
                // 不能拿库路径去拼：路径里有中文/空格/超长目录名都会出事）
                let id = reg::add_lib(&self.reg, &root, "")?;
                let rel = format!("libs/lib-{}.db", id);
                reg::add_lib(&self.reg, &root, &rel)?;
                let conn = db::open(&self.data_dir.join(&rel))?;
                self.libs.push(Lib { id, root: root.clone(), conn });
                id
            }
        };
        Ok(id)
    }

    pub fn lib_count(&self) -> Result<i64, String> {
        reg::lib_count(&self.reg)
    }

    pub fn lib_roots(&self) -> Result<Vec<String>, String> {
        reg::lib_roots(&self.reg)
    }

    /// 从素材库移除一个库根：清注册行 + 删库文件。**绝不碰磁盘上的素材**。
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

        // 先摘掉内存里的连接（文件被自己占着删不掉）
        if let Some(pos) = self.libs.iter().position(|l| l.id == row.id) {
            self.libs.remove(pos);
        }
        let rel = if row.file.is_empty() {
            reg::lib_file(&self.data_dir, row.id)
        } else {
            self.data_dir.join(&row.file)
        };
        // 库文件不直接删：挪到 `trash-libs/` 留一手（用户误点移除时还能找回评分/标签）
        let keep = self.data_dir.join("trash-libs");
        let _ = std::fs::create_dir_all(&keep);
        for suffix in ["", "-wal", "-shm"] {
            let src = PathBuf::from(format!("{}{}", rel.to_string_lossy(), suffix));
            if src.exists() {
                let name = src
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let dst = keep.join(format!("{}-{}", db::now_ts(), name));
                let _ = std::fs::rename(&src, dst);
            }
        }
        reg::remove_lib_row(&self.reg, &row.root)?;
        Ok(n)
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
        // 标签名 → 注册库里的 id / 规则（拆库后 asset_tags 存的是 id）
        if let Some(tag) = q.tag.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if autotag::parse_filter(tag).is_none() {
                let (ids, rule) = self.tag_lookup(tag)?;
                q.tag_ids = ids;
                q.tag_rule = rule;
            }
        }

        // 搜索词里的标签名：标签**名字**在注册库，这里换成 id 再交给库查询（拆库后不能 JOIN）
        let text = q.text.clone().unwrap_or_default();
        let needle = text.trim().trim_start_matches('#').trim().to_string();
        if !needle.is_empty() {
            q.search_tag_ids = Some(self.search_tag_ids(&needle)?);
        }

        if let Some(g) = q.dir_id {
            let (lib_id, local) = ungid(g);
            let l = self.lib(lib_id)?;
            let mut q2 = q.clone();
            q2.dir_id = Some(local);
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
            q2.limit = Some(limit);
            q2.cursor = None;
            q2.name_cursor = None;
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

    /// 标签名 → (id 列表, 规则 JSON)。找不到就是 (None, None)，让上层给空结果。
    fn tag_lookup(&self, name: &str) -> Result<(Option<Vec<i64>>, Option<String>), String> {
        let meta = reg::tags_meta(&self.reg)?;
        match meta.iter().find(|t| t.name == name) {
            Some(t) => Ok((Some(vec![t.id]), t.rule.clone())),
            None => Ok((None, None)),
        }
    }

    /// 标签名模糊匹配 → id 列表。注册库里标签是"一屏能看完"的量级，
    /// 内存里 contains 比来回拼 SQL 更省事，也不用处理 LIKE 转义。
    fn search_tag_ids(&self, needle: &str) -> Result<Vec<i64>, String> {
        let n = needle.to_lowercase();
        let meta = reg::tags_meta(&self.reg)?;
        Ok(meta
            .iter()
            .filter(|t| t.name.to_lowercase().contains(&n))
            .map(|t| t.id)
            .collect())
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

    /// 手工标签 + 使用数（跨库汇总）。
    /// 规则标签（T34）的计数现场算 —— 规则是"活"的，没有可数的关联行。
    pub fn tags(&self) -> Result<Vec<model::TagCount>, String> {
        let meta = reg::tags_meta(&self.reg)?;
        let mut counts: HashMap<i64, i64> = HashMap::new();
        for l in &self.libs {
            for (tid, n) in db::counts_by_tag(&l.conn)? {
                *counts.entry(tid).or_insert(0) += n;
            }
        }
        let mut out = Vec::new();
        for t in &meta {
            let mut n = counts.get(&t.id).copied().unwrap_or(0);
            if let Some(rule) = t.rule.as_deref().and_then(reg::parse_rule) {
                let mut sum = 0i64;
                for l in &self.libs {
                    sum += db::count_rule(&l.conn, &rule)?;
                }
                n = sum;
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

    /// 自动标签 + 使用数（跨库同名合并）
    pub fn auto_tags(&self) -> Result<Vec<model::TagCount>, String> {
        let mut merged: Vec<model::TagCount> = Vec::new();
        for l in &self.libs {
            for t in db::auto_tag_counts(&l.conn)? {
                match merged.iter_mut().find(|m| m.name == t.name) {
                    Some(m) => m.count += t.count,
                    None => merged.push(t),
                }
            }
        }
        Ok(merged)
    }

    pub fn groups(&self) -> Result<Vec<model::TagGroup>, String> {
        let meta = reg::tags_meta(&self.reg)?;
        let mut counts: HashMap<i64, i64> = HashMap::new();
        for l in &self.libs {
            for (tid, n) in db::counts_by_tag(&l.conn)? {
                *counts.entry(tid).or_insert(0) += n;
            }
        }
        let rows = reg::groups(&self.reg, &meta, &counts)?;
        Ok(rows
            .into_iter()
            .map(|g| model::TagGroup { name: g.name, color: g.color, count: g.count, tags: g.tags })
            .collect())
    }

    /// 一批素材各自的标签名（`id` 是全局 id）
    pub fn tags_for(&self, ids: &[i64]) -> Result<Vec<(i64, Vec<String>)>, String> {
        let meta = reg::tags_meta(&self.reg)?;
        let name_of: HashMap<i64, String> = meta.into_iter().map(|t| (t.id, t.name)).collect();
        let mut out: Vec<(i64, Vec<String>)> = Vec::new();
        for (lib_id, locals) in ids::group_by_lib(ids) {
            let l = self.lib(lib_id)?;
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
        let tag_id = reg::ensure_tag(&self.reg, &name)?;
        let mut n = 0;
        for (lib_id, locals) in ids::group_by_lib(ids) {
            let l = self.lib_mut(lib_id)?;
            n += db::insert_asset_tags(&l.conn, &locals, tag_id)?;
        }
        Ok(n)
    }

    pub fn untag_assets(&mut self, ids: &[i64], name: &str) -> Result<i64, String> {
        let Some(tag_id) = reg::tag_id_by_name(&self.reg, name)? else { return Ok(0) };
        let mut n = 0;
        for (lib_id, locals) in ids::group_by_lib(ids) {
            let l = self.lib_mut(lib_id)?;
            n += db::delete_asset_tags(&l.conn, &locals, tag_id)?;
        }
        Ok(n)
    }

    /// 详情面板保存：整组替换（按名字给，内部换算成 id）
    pub fn set_asset_tags(&mut self, g: i64, names: &[String]) -> Result<(), String> {
        let mut ids = Vec::new();
        for n in names {
            let n = reg::validate_tag_name(n)?;
            ids.push(reg::ensure_tag(&self.reg, &n)?);
        }
        ids.sort();
        ids.dedup();
        let (lib_id, local) = ungid(g);
        db::set_asset_tags(&self.lib(lib_id)?.conn, local, &ids)?;
        self.gc_tags()
    }

    pub fn set_tag_color(&mut self, name: &str, color: Option<&str>) -> Result<(), String> {
        reg::set_tag_color(&self.reg, name, color)
    }

    pub fn set_tag_group(&mut self, name: &str, group: Option<&str>) -> Result<(), String> {
        reg::set_tag_group(&self.reg, name, group)
    }

    /// 给标签设自动规则（T34）。**标签不存在就先建出来** —— 用户的心智是
    /// 「我建一条规则叫『PS 源文件』」，不是「先去打个标签再来设规则」。
    pub fn set_tag_rule(&mut self, name: &str, rule: Option<&reg::AutoRule>) -> Result<(), String> {
        let n = reg::validate_tag_name(name)?;
        reg::ensure_tag(&self.reg, &n)?;
        reg::set_tag_rule(&self.reg, &n, rule)
    }

    pub fn tag_rule(&self, name: &str) -> Result<Option<reg::AutoRule>, String> {
        Ok(reg::tags_meta(&self.reg)?
            .into_iter()
            .find(|t| t.name == name)
            .and_then(|t| t.rule.as_deref().and_then(reg::parse_rule)))
    }

    /// 标签改名；新名已存在时**合并**（跨所有库搬关联）。
    pub fn rename_tag(&mut self, old: &str, new: &str) -> Result<i64, String> {
        let new = reg::validate_tag_name(new)?;
        if old == new {
            return Ok(0);
        }
        let Some(old_id) = reg::tag_id_by_name(&self.reg, old)? else {
            return Err(format!("没有这个标签：{old}"));
        };
        let merged = match reg::tag_id_by_name(&self.reg, &new)? {
            None => {
                self.reg
                    .execute("UPDATE tags SET name = ?1 WHERE id = ?2", rusqlite::params![new, old_id])
                    .map_err(|e| e.to_string())?;
                0
            }
            Some(new_id) => {
                let mut n = 0;
                for l in self.libs.iter_mut() {
                    n += db::move_tag_refs(&l.conn, old_id, new_id)?;
                }
                self.reg
                    .execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![old_id])
                    .map_err(|e| e.to_string())?;
                // 合并过去的标签如果自己还有规则，把规则也带过去（不为空才覆盖）
                let _ = self.reg.execute(
                    "UPDATE tags SET auto_rule = COALESCE(auto_rule, (SELECT auto_rule FROM tags WHERE id = ?1)) WHERE id = ?2",
                    rusqlite::params![old_id, new_id],
                );
                n
            }
        };
        self.gc_tags()?;
        Ok(merged)
    }

    /// 从所有素材上移除某标签（跨库），返回影响条数
    pub fn delete_tag(&mut self, name: &str) -> Result<i64, String> {
        let Some(tag_id) = reg::tag_id_by_name(&self.reg, name)? else { return Ok(0) };
        let mut n = 0;
        for l in self.libs.iter_mut() {
            n += db::delete_tag_refs(&l.conn, tag_id)?;
        }
        self.reg
            .execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![tag_id])
            .map_err(|e| e.to_string())?;
        Ok(n)
    }

    /// 清掉"一个素材都没打"的标签行（跨库判断：任何库里还在用就保留）。
    /// 什么时候会有：用户把某标签从最后一个素材上摘掉；或者合并后旧标签空了。
    fn gc_tags(&mut self) -> Result<(), String> {
        let mut used: HashSet<i64> = HashSet::new();
        for l in &self.libs {
            for id in db::used_tag_ids(&l.conn)? {
                used.insert(id);
            }
        }
        let meta = reg::tags_meta(&self.reg)?;
        for t in meta {
            if !used.contains(&t.id) && t.rule.is_none() {
                self.reg
                    .execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![t.id])
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub fn group_new(&mut self, name: &str) -> Result<(), String> {
        reg::group_new(&self.reg, name)
    }
    pub fn group_rename(&mut self, old: &str, new: &str) -> Result<(), String> {
        reg::group_rename(&self.reg, old, new)
    }
    pub fn group_set_color(&mut self, name: &str, color: Option<&str>) -> Result<(), String> {
        reg::group_set_color(&self.reg, name, color)
    }
    pub fn group_delete(&mut self, name: &str) -> Result<i64, String> {
        reg::group_delete(&self.reg, name)
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
            let _ = std::fs::remove_file(self.thumbs.file_for(&e.key));
            let _ = std::fs::remove_file(self.thumbs.file_for(&e.key).with_extension("jpg"));
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
        let meta = reg::tags_meta(&self.reg)?;
        let name_of: HashMap<i64, String> = meta.iter().map(|t| (t.id, t.name.clone())).collect();
        let mut items: Vec<serde_json::Value> = Vec::new();
        for l in &self.libs {
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
        let store = thumb::ThumbStore::new(self.thumbs.root.clone());
        let mut done = 0i64;
        let mut left = 0i64;
        for l in self.libs.iter_mut() {
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

        let files: Vec<_> = std::fs::read_dir(d.join("libs"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".db"))
            .collect();
        assert_eq!(files.len(), 2, "每个库一个 .db 文件（-wal/-shm 不算）");

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
        // 库文件被挪进 trash-libs 留证，而不是直接删
        let kept: Vec<_> = std::fs::read_dir(d.join("trash-libs")).unwrap().flatten().collect();
        assert_eq!(kept.len(), 1, "被移除的库文件要留一手");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 标签体系是全局的：在 A 库打的标签，B 库的素材能按同名标签筛到，颜色也只有一份。
    #[test]
    fn tags_are_global_across_libs() {
        let d = tmp("globaltag");
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

        app.tag_assets(&[ia], "跨库标签").unwrap();
        app.tag_assets(&[ib], "跨库标签").unwrap();
        app.set_tag_color("跨库标签", Some("teal")).unwrap();

        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].count, 2, "两个库的命中数要加在一起");
        assert_eq!(tags[0].color.as_deref(), Some("teal"));

        // 按标签筛：跨库都能命中
        let q = model::AssetQuery {
            recursive: true,
            tag: Some("跨库标签".into()),
            limit: Some(10),
            ..Default::default()
        };
        let rows = app.list(&q).unwrap();
        assert_eq!(rows.len(), 2);

        // 每素材的标签名（id 要换成全局 id）
        let per = app.tags_for(&[ia, ib]).unwrap();
        assert_eq!(per.len(), 2);
        assert_eq!(per[0].1, vec!["跨库标签".to_string()]);

        // 改名合并：撞名时搬关联，旧标签消失。
        // 先让 x.png 只留「另一个」（否则两条关联都已存在，搬过去是 0 行 —— 幂等但测不到搬）
        app.tag_assets(&[ia], "另一个").unwrap();
        app.untag_assets(&[ia], "跨库标签").unwrap();
        let merged = app.rename_tag("另一个", "跨库标签").unwrap();
        assert_eq!(merged, 1);
        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].count, 2);

        // 删标签：两个库的关联都清掉，标签行也没了
        let n = app.delete_tag("跨库标签").unwrap();
        assert_eq!(n, 2);
        assert!(app.tags().unwrap().is_empty());

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
}
