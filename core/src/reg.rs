//! 注册库（`index.db`）：**库清单 + 标签体系 + 自动标签规则**。
//!
//! T28 把素材库拆成「每个库一个 SQLite 文件」（`libs/lib-<id>.db`），
//! 于是必须回答一个问题：哪些东西是**跨库**的？
//!
//! - 「我挂了哪几个文件夹」→ 跨库，只有一份（这份清单本身就是拆库的分片依据）；
//! - 「标签叫什么、什么颜色、属于哪个分组」→ 跨库。同一套标签分类跨库共用才说得通：
//!   用户不会希望「客户稿」在 A 库是红色、在 B 库是灰色；
//! - 「素材上打了哪些标签」→ 落在各自库里（`asset_tags`，用库内 id），
//!   这样删素材时靠外键级联就干净了，不需要跨库对账；
//! - 「缓存用了多少」→ 落在各自库里（`thumb_lru`），淘汰时跨库汇总排序。
//!
//! 拆库的好处是真实存在的：单个库坏了只坏一个（坏库留证后其余库照常打开）、
//! 整库搬走/备份只要拷一个文件、单个库的体积与 VACUUM 时间都可控。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::db;
use crate::db::Res;

const REG_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);

/* 库清单：每一行就是磁盘上的一个 `.db` 文件。
   `file` 存相对路径（相对数据目录），换机/换盘时整个目录一起搬仍然成立。 */
CREATE TABLE IF NOT EXISTS libs (
  id INTEGER PRIMARY KEY,
  root TEXT NOT NULL UNIQUE,
  added_at INTEGER NOT NULL,
  file TEXT NOT NULL DEFAULT ''
);

/* 标签体系（全局）：颜色只存语义键（red / teal …），不存 #RRGGBB ——
   同一个键在浅色/深色主题下映射到不同色值，存十六进制就跟着主题糊掉。
   `auto_rule`（T34）：JSON 规则，命中即视为打了这个标签（**不物化**成关联行，
   否则库一动就得全量重算；规则是"活"的，查询时才翻译成 SQL 条件）。 */
CREATE TABLE IF NOT EXISTS tags (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  color TEXT,
  group_id INTEGER,
  auto_rule TEXT
);

CREATE TABLE IF NOT EXISTS tag_groups (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  color TEXT,
  sort INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

/* 快照（0.12.0）：一批图片的书签，**跨库存在**（同一块板可能横跨几个库）。
   条目身份是**内容哈希**，不是 id 也不是路径 —— 改名 / 换目录 / 换库都还认得出来，
   原因和取舍写在 `core/src/snapshot.rs` 的头注释里。 */
CREATE TABLE IF NOT EXISTS snapshots (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshot_items (
  snapshot_id INTEGER NOT NULL,
  hash TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  size INTEGER NOT NULL DEFAULT 0,
  w INTEGER,
  h INTEGER,
  path TEXT NOT NULL DEFAULT '',
  lib TEXT NOT NULL DEFAULT '',
  added_at INTEGER NOT NULL,
  PRIMARY KEY (snapshot_id, hash)
);
CREATE INDEX IF NOT EXISTS idx_snapshot_items_hash ON snapshot_items(hash);

/* 分组（0.2.x）：用户自己攒的「一撮素材」，**跨库存在**（同一组可以横跨几个素材库）。
   存的是**全局 asset id**（`ids::gid` = 库号 + 库内 id），不是哈希也不是路径：
   分组是"我现在挑出来的这几张"，跟着素材走；素材被删/丢了，这一行自动查不到，计数时跳过。
   与「标签」的区别（照 FontManager）：标签是**属性**（一张图可以有多个），
   分组是**收纳**（用户在侧栏点名要看的那一批）；所以分组进侧栏、标签进标签面板。 */
CREATE TABLE IF NOT EXISTS collections (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  sort INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS collection_assets (
  collection_id INTEGER NOT NULL,
  asset_gid INTEGER NOT NULL,
  added_at INTEGER NOT NULL,
  PRIMARY KEY (collection_id, asset_gid)
);
CREATE INDEX IF NOT EXISTS idx_collection_assets_gid ON collection_assets(asset_gid);

"#;

pub struct LibRow {
    pub id: i64,
    pub root: String,
    pub file: String,
}

pub fn open(path: &Path) -> Res<Connection> {
    let c = db::open_raw(path)?;
    c.execute_batch(REG_SCHEMA).map_err(|e| e.to_string())?;
    // 索引**建在补列之后**：0.4.x 的 tags 没有 group_id，把这条 CREATE INDEX 放进
    // REG_SCHEMA 会让老库直接在启动阶段报 `no such column: group_id`（真踩过）。
    if let Some(cols) = columns(&c, "tags")? {
        if cols.iter().any(|c| c == "group_id") {
            c.execute("CREATE INDEX IF NOT EXISTS idx_tags_group ON tags(group_id)", [])
                .map_err(|e| e.to_string())?;
        }
    }
    db::migrate_versioned(&c)?;
    Ok(c)
}

fn columns(c: &Connection, table: &str) -> Res<Option<Vec<String>>> {
    let mut st = match c.prepare(&format!("PRAGMA table_info({table})")) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let rows = st.query_map([], |r| r.get::<_, String>(1)).map_err(|e| e.to_string())?;
    Ok(Some(rows.flatten().collect()))
}

/* ------------------------------------------------------------------ 库清单 */

/// 登记一个库根。`file` 是相对数据目录的库文件名（`libs/lib-3.db`）。
pub fn add_lib(c: &Connection, root: &str, file: &str) -> Res<i64> {
    c.query_row(
        "INSERT INTO libs (root, added_at, file) VALUES (?1, ?2, ?3)
         ON CONFLICT(root) DO UPDATE SET file = excluded.file
         RETURNING id",
        params![root, db::now_ts(), file],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn libs(c: &Connection) -> Res<Vec<LibRow>> {
    let mut st = c
        .prepare("SELECT id, root, file FROM libs ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok(LibRow { id: r.get(0)?, root: r.get(1)?, file: r.get(2)? })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

pub fn lib_count(c: &Connection) -> Res<i64> {
    c.query_row("SELECT COUNT(*) FROM libs", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

pub fn lib_roots(c: &Connection) -> Res<Vec<String>> {
    let rows = libs(c)?;
    Ok(rows.into_iter().map(|l| l.root).collect())
}

/// 删掉库清单里的一行（**不删库文件** —— 文件由 `App::remove_lib` 负责挪走/删除）。
pub fn remove_lib_row(c: &Connection, root: &str) -> Res<()> {
    c.execute("DELETE FROM libs WHERE root = ?1", params![root])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 库文件的落点：`<数据目录>/libs/lib-<id>.db`。
///
/// 单独一个子目录而不是和 index.db 并列：用户打开数据目录时，一眼能分清
/// 「哪个是总账、哪些是我的库」，搬库/备份也不容易漏。
pub fn lib_file(data_dir: &Path, lib_id: i64) -> PathBuf {
    data_dir.join("libs").join(format!("lib-{}.db", lib_id))
}

/* ------------------------------------------------------------------ 标签 */

/// 颜色白名单。**只接受语义键，不接受色值**（见 SCHEMA 里的注释）。
/// 前端的 `TAG_COLORS`（`app/src/lib/tags.ts`）必须和这里逐字一致。
pub const TAG_COLORS: &[&str] = &[
    "gray", "red", "orange", "amber", "green", "teal", "blue", "indigo", "violet", "pink",
];

pub fn validate_color(color: Option<&str>) -> Res<Option<String>> {
    let c = color.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_lowercase());
    match c {
        None => Ok(None),
        Some(c) if TAG_COLORS.contains(&c.as_str()) => Ok(Some(c)),
        Some(c) => Err(format!("不认识的标签颜色：{c}")),
    }
}

/// 标签名校验：空、超长、带自动标签命名空间的都拒（后者会和 `fmt:` 那套撞车）
pub fn validate_tag_name(name: &str) -> Res<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("标签名不能为空".into());
    }
    if n.chars().count() > 32 {
        return Err("标签名最多 32 个字".into());
    }
    if crate::autotag::is_reserved_name(n) {
        return Err("fmt:/kind:/place:/orient:/size: 是自动标签的专用前缀，换个名字".into());
    }
    Ok(n.to_string())
}

pub fn ensure_tag(c: &Connection, name: &str) -> Res<i64> {
    c.execute(
        "INSERT OR IGNORE INTO tags (name, created_at) VALUES (?1, ?2)",
        params![name, db::now_ts()],
    )
    .map_err(|e| e.to_string())?;
    c.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| r.get(0))
        .map_err(|e| e.to_string())
}

pub fn tag_id_by_name(c: &Connection, name: &str) -> Res<Option<i64>> {
    c.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| r.get::<_, i64>(0))
        .optional()
        .map_err(|e| e.to_string())
}

/// 标签的元数据（跨库共用那一份）：id / 名字 / 颜色 / 分组 / 自动规则。
pub struct TagMeta {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub group: Option<String>,
    pub rule: Option<String>,
}

pub fn tags_meta(c: &Connection) -> Res<Vec<TagMeta>> {
    let mut st = c
        .prepare(
            "SELECT t.id, t.name, t.color, g.name, t.auto_rule
             FROM tags t LEFT JOIN tag_groups g ON g.id = t.group_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok(TagMeta {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                group: r.get(3)?,
                rule: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

pub fn set_tag_color(c: &Connection, name: &str, color: Option<&str>) -> Res<()> {
    let color = validate_color(color)?;
    let n = c
        .execute("UPDATE tags SET color = ?1 WHERE name = ?2", params![color, name])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("没有这个标签：{name}"));
    }
    Ok(())
}

/* ------------------------------------------------- 自动标签规则（T34） */

/// 规则里的一个"与"条件。全部条件都满足才算命中；空字段 = 不限制。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoRule {
    /// 目录前缀（大小写不敏感，命中即包含子目录）
    pub dir: String,
    /// 扩展名白名单（小写、不带点）
    pub ext: Vec<String>,
    /// 类型白名单（0 图片 / 1 视频 …）
    pub kind: Vec<i32>,
    /// 体积区间（字节，0 = 不限）
    pub size_min: i64,
    pub size_max: i64,
    /// 长边像素区间（0 = 不限）
    pub px_min: i64,
    pub px_max: i64,
    /// 仅当"有尺寸信息"的素材才算（默认 false）
    pub need_dims: bool,
}

impl AutoRule {
    pub fn is_empty(&self) -> bool {
        self.dir.trim().is_empty()
            && self.ext.is_empty()
            && self.kind.is_empty()
            && self.size_min <= 0
            && self.size_max <= 0
            && self.px_min <= 0
            && self.px_max <= 0
    }
}

pub fn parse_rule(json: &str) -> Option<AutoRule> {
    let r: AutoRule = serde_json::from_str(json).ok()?;
    if r.is_empty() {
        None
    } else {
        Some(r)
    }
}

pub fn set_tag_rule(c: &Connection, name: &str, rule: Option<&AutoRule>) -> Res<()> {
    let json = match rule {
        Some(r) if !r.is_empty() => Some(serde_json::to_string(r).map_err(|e| e.to_string())?),
        _ => None,
    };
    let n = c
        .execute("UPDATE tags SET auto_rule = ?1 WHERE name = ?2", params![json, name])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("没有这个标签：{name}"));
    }
    Ok(())
}

/* ------------------------------------------------------------- 标签分组 */

pub fn validate_group_name(name: &str) -> Res<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("分组名不能为空".into());
    }
    if n.chars().count() > 24 {
        return Err("分组名最多 24 个字".into());
    }
    Ok(n.to_string())
}

fn ensure_group(c: &Connection, name: &str) -> Res<i64> {
    let n = validate_group_name(name)?;
    c.execute(
        "INSERT OR IGNORE INTO tag_groups (name, created_at, sort)
         VALUES (?1, ?2, (SELECT COALESCE(MAX(sort), 0) + 1 FROM tag_groups))",
        params![n, db::now_ts()],
    )
    .map_err(|e| e.to_string())?;
    c.query_row("SELECT id FROM tag_groups WHERE name = ?1", params![n], |r| r.get(0))
        .map_err(|e| e.to_string())
}

/// 把标签放进某个分组；`group=None` 表示移回「未分组」。分组不存在就顺手建出来。
pub fn set_tag_group(c: &Connection, name: &str, group: Option<&str>) -> Res<()> {
    let gid = match group.map(str::trim).filter(|s| !s.is_empty()) {
        Some(g) => Some(ensure_group(c, g)?),
        None => None,
    };
    let n = c
        .execute("UPDATE tags SET group_id = ?1 WHERE name = ?2", params![gid, name])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("没有这个标签：{name}"));
    }
    Ok(())
}

pub fn group_new(c: &Connection, name: &str) -> Res<()> {
    let n = validate_group_name(name)?;
    let exists: i64 = c
        .query_row("SELECT COUNT(*) FROM tag_groups WHERE name = ?1", params![n], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if exists > 0 {
        return Err(format!("已经有叫「{n}」的分组了"));
    }
    c.execute(
        "INSERT INTO tag_groups (name, created_at, sort)
         VALUES (?1, ?2, (SELECT COALESCE(MAX(sort), 0) + 1 FROM tag_groups))",
        params![n, db::now_ts()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn group_rename(c: &Connection, old: &str, new: &str) -> Res<()> {
    let n = validate_group_name(new)?;
    if n == old {
        return Ok(());
    }
    let exists: i64 = c
        .query_row("SELECT COUNT(*) FROM tag_groups WHERE name = ?1", params![n], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if exists > 0 {
        return Err(format!("已经有叫「{n}」的分组了"));
    }
    let m = c
        .execute("UPDATE tag_groups SET name = ?1 WHERE name = ?2", params![n, old])
        .map_err(|e| e.to_string())?;
    if m == 0 {
        return Err(format!("没有这个分组：{old}"));
    }
    Ok(())
}

pub fn group_set_color(c: &Connection, name: &str, color: Option<&str>) -> Res<()> {
    let color = validate_color(color)?;
    let n = c
        .execute("UPDATE tag_groups SET color = ?1 WHERE name = ?2", params![color, name])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("没有这个分组：{name}"));
    }
    Ok(())
}

/// 解散分组：**只解散，不删标签**（里面的标签回到「未分组」）。返回被解散的标签数。
pub fn group_delete(c: &Connection, name: &str) -> Res<i64> {
    let Some(gid) = c
        .query_row("SELECT id FROM tag_groups WHERE name = ?1", params![name], |r| r.get::<_, i64>(0))
        .optional()
        .map_err(|e| e.to_string())?
    else {
        return Ok(0);
    };
    let n = c
        .execute("UPDATE tags SET group_id = NULL WHERE group_id = ?1", params![gid])
        .map_err(|e| e.to_string())? as i64;
    c.execute("DELETE FROM tag_groups WHERE id = ?1", params![gid])
        .map_err(|e| e.to_string())?;
    Ok(n)
}

/// 标签组 + 「组里的命中数 / 多少个标签」。
///
/// 空组也要返回：用户刚建完的组如果立刻消失，他会以为没建成。
/// 口径：`count` = **组内各标签的命中数之和**（同一素材被组内两个标签命中会算两次）。
/// 为什么不去重：去重要跨库对 (tag_id, asset_id) 做并集，代价和"看一眼侧栏"完全不成比例。
/// 侧栏这个数字只用来判断"这组东西多不多"，不是审计口径。
pub fn groups(
    c: &Connection,
    tags: &[TagMeta],
    counts: &std::collections::HashMap<i64, i64>,
) -> Res<Vec<TagGroupRow>> {
    let mut st = c
        .prepare(
            "SELECT g.name, g.color,
                    (SELECT COUNT(*) FROM tags t WHERE t.group_id = g.id)
             FROM tag_groups g
             ORDER BY g.sort ASC, g.name COLLATE NOCASE ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(|e| e.to_string())?;
    let base: Vec<(String, Option<String>, i64)> = rows.filter_map(|x| x.ok()).collect();

    let mut out = Vec::new();
    for (name, color, tags_n) in base {
        let mut sum = 0i64;
        for t in tags {
            if t.group.as_deref() == Some(name.as_str()) {
                sum += counts.get(&t.id).copied().unwrap_or(0);
            }
        }
        out.push(TagGroupRow { name, color, count: sum, tags: tags_n });
    }
    Ok(out)
}

pub struct TagGroupRow {
    pub name: String,
    pub color: Option<String>,
    pub count: i64,
    pub tags: i64,
}

/* ------------------------------------------------------------------ 快照 */

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRow {
    pub id: i64,
    pub name: String,
    /// 条目数：**记下来的条数**，不是"现在找得到的张数"。
    /// 移动硬盘没插时也还是这么多条 —— 条目永远不会因为找不到而消失。
    pub count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_to_snapshot(r: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotRow> {
    Ok(SnapshotRow {
        id: r.get(0)?,
        name: r.get(1)?,
        count: r.get(2)?,
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
    })
}

const SNAP_SELECT: &str = "SELECT s.id, s.name,
        (SELECT COUNT(*) FROM snapshot_items i WHERE i.snapshot_id = s.id),
        s.created_at, s.updated_at
     FROM snapshots s ";

pub fn snapshots(c: &Connection) -> Res<Vec<SnapshotRow>> {
    let mut st = c
        .prepare(&format!("{SNAP_SELECT} ORDER BY s.created_at DESC, s.id DESC"))
        .map_err(|e| e.to_string())?;
    let rows = st.query_map([], |r| row_to_snapshot(r)).map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

pub fn snapshot(c: &Connection, id: i64) -> Res<Option<SnapshotRow>> {
    let mut st = c
        .prepare(&format!("{SNAP_SELECT} WHERE s.id = ?1"))
        .map_err(|e| e.to_string())?;
    st.query_row(params![id], |r| row_to_snapshot(r))
        .optional()
        .map_err(|e| e.to_string())
}

pub fn validate_snapshot_name(name: &str) -> Res<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("快照名不能为空".into());
    }
    if n.chars().count() > 60 {
        return Err("快照名太长了（最多 60 个字）".into());
    }
    Ok(n.to_string())
}

/// 新建快照。**同名直接返回已有的 id，不报错**：用户点两次不该出现两个一样的。
pub fn snapshot_create(c: &Connection, name: &str) -> Res<i64> {
    let name = validate_snapshot_name(name)?;
    let now = db::now_ts();
    c.query_row(
        "INSERT INTO snapshots (name, created_at, updated_at) VALUES (?1, ?2, ?2)
         ON CONFLICT(name) DO UPDATE SET updated_at = snapshots.updated_at
         RETURNING id",
        params![name, now],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn snapshot_rename(c: &Connection, id: i64, name: &str) -> Res<()> {
    let name = validate_snapshot_name(name)?;
    // 重名要在界面上说清楚，不能静默改成"名字 (2)"
    let taken: Option<i64> = c
        .query_row(
            "SELECT id FROM snapshots WHERE name = ?1 AND id <> ?2",
            params![name, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if taken.is_some() {
        return Err(format!("已经有一个叫「{name}」的快照了"));
    }
    c.execute(
        "UPDATE snapshots SET name = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, name, db::now_ts()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn snapshot_delete(c: &Connection, id: i64) -> Res<()> {
    c.execute("DELETE FROM snapshot_items WHERE snapshot_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    c.execute("DELETE FROM snapshots WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn snapshot_items(c: &Connection, id: i64) -> Res<Vec<crate::snapshot::SnapItem>> {
    let mut st = c
        .prepare(
            "SELECT hash, name, size, w, h, path, lib FROM snapshot_items
             WHERE snapshot_id = ?1 ORDER BY added_at ASC, name COLLATE NOCASE ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![id], |r| {
            Ok(crate::snapshot::SnapItem {
                hash: r.get(0)?,
                name: r.get(1)?,
                size: r.get(2)?,
                w: r.get(3)?,
                h: r.get(4)?,
                path: r.get(5)?,
                lib: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

/// 批量塞条目。返回**真正新增**的条数（同一张图重复加入是幂等的）。
pub fn snapshot_insert_items(
    c: &Connection,
    id: i64,
    items: &[crate::snapshot::SnapItem],
) -> Res<usize> {
    let now = db::now_ts();
    let mut n = 0usize;
    {
        let mut st = c
            .prepare(
                "INSERT OR IGNORE INTO snapshot_items
                   (snapshot_id, hash, name, size, w, h, path, lib, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .map_err(|e| e.to_string())?;
        for it in items {
            n += st
                .execute(params![
                    id,
                    it.hash,
                    it.name,
                    it.size,
                    it.w,
                    it.h,
                    it.path,
                    it.lib,
                    now
                ])
                .map_err(|e| e.to_string())?;
        }
    }
    snapshot_touch(c, id)?;
    Ok(n)
}

/// 刷新"最后一次见到的位置"（只影响显示：改名/换库后详情里显示的是新路径）
pub fn snapshot_touch_item(c: &Connection, id: i64, hash: &str, name: &str, path: &str, lib: &str) -> Res<()> {
    c.execute(
        "UPDATE snapshot_items SET name = ?3, path = ?4, lib = ?5
         WHERE snapshot_id = ?1 AND hash = ?2",
        params![id, hash, name, path, lib],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn snapshot_remove_items(c: &Connection, id: i64, hashes: &[String]) -> Res<usize> {
    let mut n = 0usize;
    {
        let mut st = c
            .prepare("DELETE FROM snapshot_items WHERE snapshot_id = ?1 AND hash = ?2")
            .map_err(|e| e.to_string())?;
        for h in hashes {
            n += st.execute(params![id, h]).map_err(|e| e.to_string())?;
        }
    }
    snapshot_touch(c, id)?;
    Ok(n)
}

pub fn snapshot_touch(c: &Connection, id: i64) -> Res<()> {
    c.execute(
        "UPDATE snapshots SET updated_at = ?2 WHERE id = ?1",
        params![id, db::now_ts()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/* ------------------------------------------------------------------ 分组 */

#[derive(Debug, Clone, serde::Serialize)]
pub struct CollRow {
    pub id: i64,
    pub name: String,
    /// **原始成员数**（含已经被删掉 / 丢失的素材）。界面上的数字由 `App::coll_list`
    /// 用真实存在的素材重算，不直接信这个 —— 见那里的注释。
    pub count: i64,
    pub created_at: i64,
}

pub fn collections(c: &Connection) -> Res<Vec<CollRow>> {
    let mut st = c
        .prepare(
            "SELECT c.id, c.name, c.created_at,
                    (SELECT COUNT(*) FROM collection_assets a WHERE a.collection_id = c.id)
             FROM collections c
             ORDER BY c.sort, c.created_at, c.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok(CollRow { id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?, count: r.get(3)? })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

pub fn validate_coll_name(name: &str) -> Res<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("分组名不能为空".into());
    }
    if n.chars().count() > 60 {
        return Err("分组名太长了（最多 60 个字）".into());
    }
    Ok(n.to_string())
}

/// 新建分组。**同名直接返回已有的 id**（和快照同一个口径：点两次不该出两个一样的）。
pub fn coll_create(c: &Connection, name: &str) -> Res<i64> {
    let name = validate_coll_name(name)?;
    c.query_row(
        "INSERT INTO collections (name, created_at) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET name = collections.name
         RETURNING id",
        params![name, db::now_ts()],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn coll_rename(c: &Connection, id: i64, name: &str) -> Res<()> {
    let name = validate_coll_name(name)?;
    let taken: Option<i64> = c
        .query_row(
            "SELECT id FROM collections WHERE name = ?1 AND id <> ?2",
            params![name, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if taken.is_some() {
        return Err(format!("已经有一个叫「{name}」的分组了"));
    }
    c.execute("UPDATE collections SET name = ?2 WHERE id = ?1", params![id, name])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 删分组：**只删这个分组和它的成员行，素材本身一个都不动**。
pub fn coll_delete(c: &Connection, id: i64) -> Res<()> {
    c.execute("DELETE FROM collection_assets WHERE collection_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    c.execute("DELETE FROM collections WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn coll_add(c: &Connection, id: i64, gids: &[i64]) -> Res<usize> {
    let mut n = 0usize;
    let now = db::now_ts();
    {
        let mut st = c
            .prepare(
                "INSERT INTO collection_assets (collection_id, asset_gid, added_at)
                 VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            )
            .map_err(|e| e.to_string())?;
        for g in gids {
            n += st.execute(params![id, g, now]).map_err(|e| e.to_string())?;
        }
    }
    Ok(n)
}

pub fn coll_remove(c: &Connection, id: i64, gids: &[i64]) -> Res<usize> {
    let mut n = 0usize;
    {
        let mut st = c
            .prepare("DELETE FROM collection_assets WHERE collection_id = ?1 AND asset_gid = ?2")
            .map_err(|e| e.to_string())?;
        for g in gids {
            n += st.execute(params![id, g]).map_err(|e| e.to_string())?;
        }
    }
    Ok(n)
}

pub fn coll_gids(c: &Connection, id: i64) -> Res<Vec<i64>> {
    let mut st = c
        .prepare("SELECT asset_gid FROM collection_assets WHERE collection_id = ?1 ORDER BY added_at, asset_gid")
        .map_err(|e| e.to_string())?;
    let rows = st.query_map(params![id], |r| r.get(0)).map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

/// 这张素材在哪些分组里（详情面板显示"[分组] 客户稿 ×"用）。
pub fn coll_of(c: &Connection, gid: i64) -> Res<Vec<i64>> {
    let mut st = c
        .prepare("SELECT collection_id FROM collection_assets WHERE asset_gid = ?1")
        .map_err(|e| e.to_string())?;
    let rows = st.query_map(params![gid], |r| r.get(0)).map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-reg-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn libs_and_tags_roundtrip() {
        let d = tmp("tags");
        let c = open(&d.join("index.db")).unwrap();

        assert_eq!(lib_count(&c).unwrap(), 0);
        let a = add_lib(&c, "E:/lib-a", "libs/lib-1.db").unwrap();
        let b = add_lib(&c, "E:/lib-b", "libs/lib-2.db").unwrap();
        assert_ne!(a, b);
        // 重复登记：同一库根只留一行（不能因为用户点两次"添加"就多出一个库）
        let a2 = add_lib(&c, "E:/lib-a", "libs/lib-1.db").unwrap();
        assert_eq!(a, a2);
        assert_eq!(lib_count(&c).unwrap(), 2);
        assert_eq!(lib_roots(&c).unwrap(), vec!["E:/lib-a", "E:/lib-b"]);

        // 标签：颜色白名单 + 分组 + 自动规则
        let id = ensure_tag(&c, "客户稿").unwrap();
        assert!(validate_color(Some("neon")).is_err());
        set_tag_color(&c, "客户稿", Some(" Blue ")).unwrap();
        set_tag_group(&c, "客户稿", Some("客户")).unwrap();
        let rule = AutoRule { ext: vec!["psd".into()], ..Default::default() };
        set_tag_rule(&c, "客户稿", Some(&rule)).unwrap();

        let meta = tags_meta(&c).unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].id, id);
        assert_eq!(meta[0].color.as_deref(), Some("blue"));
        assert_eq!(meta[0].group.as_deref(), Some("客户"));
        assert!(parse_rule(meta[0].rule.as_deref().unwrap()).unwrap().ext == vec!["psd"]);

        // 空规则 = 清掉（否则筛这个标签会永远 0 结果）
        set_tag_rule(&c, "客户稿", Some(&AutoRule::default())).unwrap();
        assert!(tags_meta(&c).unwrap()[0].rule.is_none());

        assert!(validate_tag_name("fmt:jpg").is_err());
        assert!(validate_tag_name("  ").is_err());

        // 删库行不碰别的库
        remove_lib_row(&c, "E:/lib-a").unwrap();
        assert_eq!(lib_roots(&c).unwrap(), vec!["E:/lib-b"]);

        let _ = std::fs::remove_dir_all(&d);
    }
}
