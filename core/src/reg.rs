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

/// 错误 → 字符串（0.4.24 整理：全文件 80+ 处 `.map_err(e2s)` 收敛成一个名字）。
fn e2s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

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
/* 这里的索引**不能写进上面这段批量脚本**：一个库里已经有一张同名但结构不同的
   `collection_assets`（0.4.4 起库 db 里那份用 `asset_id`）时，`CREATE TABLE IF NOT EXISTS`
   会静默跳过，紧接着按 `asset_gid` 建索引就报 `no such column`。见 `open()` 里的条件建索引。 */

/* 灵感板的摆位（0.1.18）：板上每张图的**位置与尺寸**，跨库存在（一块板可以横跨几个库）。
   `board` = `tag:<标签名>` / `snap:<快照 id>`；`gid` 是全局素材 id。
   这只是一份**视图记忆**：删掉不会丢任何素材（下一板按瀑布流重排一遍就是新的排布），
   所以它不进素材库的 DB，也不参与任何对账。 */
CREATE TABLE IF NOT EXISTS board_items (
  board TEXT NOT NULL,
  gid INTEGER NOT NULL,
  x REAL NOT NULL,
  y REAL NOT NULL,
  w REAL NOT NULL,
  h REAL NOT NULL,
  /* 所属组（0.4.17）：同一板内相同的非零值 = 同一个组。0 = 没打组。
     组只是"黏在一起"的标记，没有名字、没有独立包围盒 —— 包围盒永远是成员现算的。 */
  grp INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (board, gid)
) WITHOUT ROWID;

/* 灵感板上的**文字便签**（0.4.14）：板上随手写的注释，位置/尺寸/文字都在这里。
   和 board_items 一样只是视图记忆，删掉不丢任何素材；
   `nid` 在一板内唯一（后端按 MAX(nid)+1 发号），前端用负数当 key 与图片 gid 区分。 */
CREATE TABLE IF NOT EXISTS board_notes (
  board TEXT NOT NULL,
  nid INTEGER NOT NULL,
  x REAL NOT NULL,
  y REAL NOT NULL,
  w REAL NOT NULL,
  h REAL NOT NULL,
  text TEXT NOT NULL DEFAULT '',
  /* 文字外观（0.4.17）：`font` 是字体族名（空 = 系统默认），`fs` 是字号（0 = 默认 12.5），
     `color` 是 #rrggbb（空 = 默认浅色文字），`bold` 0/1。
     字号跟着**框一起缩放**，所以它必须落库 —— 否则重开板就缩回默认大小。 */
  font TEXT NOT NULL DEFAULT '',
  fs REAL NOT NULL DEFAULT 0,
  color TEXT NOT NULL DEFAULT '',
  bold INTEGER NOT NULL DEFAULT 0,
  grp INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (board, nid)
) WITHOUT ROWID;

"#;

pub struct LibRow {
    pub id: i64,
    pub root: String,
    pub file: String,
}

pub fn open(path: &Path) -> Res<Connection> {
    let c = db::open_raw(path)?;
    c.execute_batch(REG_SCHEMA).map_err(e2s)?;
    // 索引**建在补列之后**：0.4.x 的 tags 没有 group_id，把这条 CREATE INDEX 放进
    // REG_SCHEMA 会让老库直接在启动阶段报 `no such column: group_id`（真踩过）。
    if let Some(cols) = columns(&c, "tags")? {
        if cols.iter().any(|c| c == "group_id") {
            c.execute("CREATE INDEX IF NOT EXISTS idx_tags_group ON tags(group_id)", [])
                .map_err(e2s)?;
        }
    }
    /* 同一个道理：`collection_assets` 可能已经是库 db 那一版（列叫 `asset_id`），
       只有确认列真的叫 `asset_gid` 才建索引 —— 否则打开就报 `no such column`。 */
    if let Some(cols) = columns(&c, "collection_assets")? {
        if cols.iter().any(|c| c == "asset_gid") {
            c.execute(
                "CREATE INDEX IF NOT EXISTS idx_collection_assets_gid ON collection_assets(asset_gid)",
                [],
            )
            .map_err(e2s)?;
        }
    }
    /* 灵感板的组与文字外观（0.4.17）：`CREATE TABLE IF NOT EXISTS` 不会给老表补列，
       老库必须在打开时补上，否则 `SELECT grp` 直接报 `no such column`。
       补列只加不改、全部带默认值 —— 老数据落进去就是"没打组 / 默认外观"。 */
    add_col(&c, "board_items", "grp", "grp INTEGER NOT NULL DEFAULT 0")?;
    add_col(&c, "board_notes", "grp", "grp INTEGER NOT NULL DEFAULT 0")?;
    add_col(&c, "board_notes", "font", "font TEXT NOT NULL DEFAULT ''")?;
    add_col(&c, "board_notes", "fs", "fs REAL NOT NULL DEFAULT 0")?;
    add_col(&c, "board_notes", "color", "color TEXT NOT NULL DEFAULT ''")?;
    add_col(&c, "board_notes", "bold", "bold INTEGER NOT NULL DEFAULT 0")?;
    db::migrate_versioned(&c)?;
    Ok(c)
}

/// 给老表补一列（幂等）。表不存在时什么也不做 —— 建表由 `REG_SCHEMA` 负责。
fn add_col(c: &Connection, table: &str, col: &str, ddl: &str) -> Res<()> {
    if let Some(cols) = columns(c, table)? {
        if !cols.iter().any(|x| x == col) {
            c.execute(&format!("ALTER TABLE {table} ADD COLUMN {ddl}"), [])
                .map_err(e2s)?;
        }
    }
    Ok(())
}

fn columns(c: &Connection, table: &str) -> Res<Option<Vec<String>>> {
    let mut st = match c.prepare(&format!("PRAGMA table_info({table})")) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let rows = st.query_map([], |r| r.get::<_, String>(1)).map_err(e2s)?;
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
    .map_err(e2s)
}

/// 已经用掉的库 id（重新导入一个老库时，用来判断"旧 id 还能不能复用"）。
pub fn lib_ids(c: &Connection) -> Res<Vec<i64>> {
    let mut st = c.prepare("SELECT id FROM libs").map_err(e2s)?;
    let rows = st.query_map([], |r| r.get::<_, i64>(0)).map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/// 用**指定 id** 登记库（旧 id 复用）。
///
/// 0.3.9 起库里带着 `lib.json`，里面记着当初的 id；换机器 / 移除后重新导入时
/// 优先复用它，否则快照里那些 `lib<<40 | local` 的全局 id 会全部对不上。
pub fn add_lib_at(c: &Connection, id: i64, root: &str, file: &str) -> Res<()> {
    c.execute(
        "INSERT INTO libs (id, root, added_at, file) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(root) DO UPDATE SET file = excluded.file",
        params![id, root, db::now_ts(), file],
    )
    .map(|_| ())
    .map_err(e2s)
}

pub fn libs(c: &Connection) -> Res<Vec<LibRow>> {
    let mut st = c
        .prepare("SELECT id, root, file FROM libs ORDER BY id")
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| {
            Ok(LibRow { id: r.get(0)?, root: r.get(1)?, file: r.get(2)? })
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

pub fn lib_count(c: &Connection) -> Res<i64> {
    c.query_row("SELECT COUNT(*) FROM libs", [], |r| r.get(0))
        .map_err(e2s)
}

pub fn lib_roots(c: &Connection) -> Res<Vec<String>> {
    let rows = libs(c)?;
    Ok(rows.into_iter().map(|l| l.root).collect())
}

/// 删掉库清单里的一行（**不删库文件** —— 文件由 `App::remove_lib` 负责挪走/删除）。
pub fn remove_lib_row(c: &Connection, root: &str) -> Res<()> {
    c.execute("DELETE FROM libs WHERE root = ?1", params![root])
        .map_err(e2s)?;
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
    .map_err(e2s)?;
    c.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| r.get(0))
        .map_err(e2s)
}

pub fn tag_id_by_name(c: &Connection, name: &str) -> Res<Option<i64>> {
    c.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| r.get::<_, i64>(0))
        .optional()
        .map_err(e2s)
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
        .map_err(e2s)?;
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
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

pub fn set_tag_color(c: &Connection, name: &str, color: Option<&str>) -> Res<()> {
    let color = validate_color(color)?;
    let n = c
        .execute("UPDATE tags SET color = ?1 WHERE name = ?2", params![color, name])
        .map_err(e2s)?;
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
        Some(r) if !r.is_empty() => Some(serde_json::to_string(r).map_err(e2s)?),
        _ => None,
    };
    let n = c
        .execute("UPDATE tags SET auto_rule = ?1 WHERE name = ?2", params![json, name])
        .map_err(e2s)?;
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
    .map_err(e2s)?;
    c.query_row("SELECT id FROM tag_groups WHERE name = ?1", params![n], |r| r.get(0))
        .map_err(e2s)
}

/// 把标签放进某个分组；`group=None` 表示移回「未分组」。分组不存在就顺手建出来。
pub fn set_tag_group(c: &Connection, name: &str, group: Option<&str>) -> Res<()> {
    let gid = match group.map(str::trim).filter(|s| !s.is_empty()) {
        Some(g) => Some(ensure_group(c, g)?),
        None => None,
    };
    let n = c
        .execute("UPDATE tags SET group_id = ?1 WHERE name = ?2", params![gid, name])
        .map_err(e2s)?;
    if n == 0 {
        return Err(format!("没有这个标签：{name}"));
    }
    Ok(())
}

pub fn group_new(c: &Connection, name: &str) -> Res<()> {
    let n = validate_group_name(name)?;
    let exists: i64 = c
        .query_row("SELECT COUNT(*) FROM tag_groups WHERE name = ?1", params![n], |r| r.get(0))
        .map_err(e2s)?;
    if exists > 0 {
        return Err(format!("已经有叫「{n}」的分组了"));
    }
    c.execute(
        "INSERT INTO tag_groups (name, created_at, sort)
         VALUES (?1, ?2, (SELECT COALESCE(MAX(sort), 0) + 1 FROM tag_groups))",
        params![n, db::now_ts()],
    )
    .map_err(e2s)?;
    Ok(())
}

pub fn group_rename(c: &Connection, old: &str, new: &str) -> Res<()> {
    let n = validate_group_name(new)?;
    if n == old {
        return Ok(());
    }
    let exists: i64 = c
        .query_row("SELECT COUNT(*) FROM tag_groups WHERE name = ?1", params![n], |r| r.get(0))
        .map_err(e2s)?;
    if exists > 0 {
        return Err(format!("已经有叫「{n}」的分组了"));
    }
    let m = c
        .execute("UPDATE tag_groups SET name = ?1 WHERE name = ?2", params![n, old])
        .map_err(e2s)?;
    if m == 0 {
        return Err(format!("没有这个分组：{old}"));
    }
    Ok(())
}

pub fn group_set_color(c: &Connection, name: &str, color: Option<&str>) -> Res<()> {
    let color = validate_color(color)?;
    let n = c
        .execute("UPDATE tag_groups SET color = ?1 WHERE name = ?2", params![color, name])
        .map_err(e2s)?;
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
        .map_err(e2s)?
    else {
        return Ok(0);
    };
    let n = c
        .execute("UPDATE tags SET group_id = NULL WHERE group_id = ?1", params![gid])
        .map_err(e2s)? as i64;
    c.execute("DELETE FROM tag_groups WHERE id = ?1", params![gid])
        .map_err(e2s)?;
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
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(e2s)?;
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
        .map_err(e2s)?;
    let rows = st.query_map([], |r| row_to_snapshot(r)).map_err(e2s)?;
    Ok(rows.flatten().collect())
}

pub fn snapshot(c: &Connection, id: i64) -> Res<Option<SnapshotRow>> {
    let mut st = c
        .prepare(&format!("{SNAP_SELECT} WHERE s.id = ?1"))
        .map_err(e2s)?;
    st.query_row(params![id], |r| row_to_snapshot(r))
        .optional()
        .map_err(e2s)
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
    .map_err(e2s)
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
        .map_err(e2s)?;
    if taken.is_some() {
        return Err(format!("已经有一个叫「{name}」的快照了"));
    }
    c.execute(
        "UPDATE snapshots SET name = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, name, db::now_ts()],
    )
    .map_err(e2s)?;
    Ok(())
}

pub fn snapshot_delete(c: &Connection, id: i64) -> Res<()> {
    c.execute("DELETE FROM snapshot_items WHERE snapshot_id = ?1", params![id])
        .map_err(e2s)?;
    c.execute("DELETE FROM snapshots WHERE id = ?1", params![id])
        .map_err(e2s)?;
    Ok(())
}

pub fn snapshot_items(c: &Connection, id: i64) -> Res<Vec<crate::snapshot::SnapItem>> {
    let mut st = c
        .prepare(
            "SELECT hash, name, size, w, h, path, lib FROM snapshot_items
             WHERE snapshot_id = ?1 ORDER BY added_at ASC, name COLLATE NOCASE ASC",
        )
        .map_err(e2s)?;
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
        .map_err(e2s)?;
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
            .map_err(e2s)?;
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
                .map_err(e2s)?;
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
    .map_err(e2s)?;
    Ok(())
}

pub fn snapshot_remove_items(c: &Connection, id: i64, hashes: &[String]) -> Res<usize> {
    let mut n = 0usize;
    {
        let mut st = c
            .prepare("DELETE FROM snapshot_items WHERE snapshot_id = ?1 AND hash = ?2")
            .map_err(e2s)?;
        for h in hashes {
            n += st.execute(params![id, h]).map_err(e2s)?;
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
    .map_err(e2s)?;
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
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| {
            Ok(CollRow { id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?, count: r.get(3)? })
        })
        .map_err(e2s)?;
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
    .map_err(e2s)
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
        .map_err(e2s)?;
    if taken.is_some() {
        return Err(format!("已经有一个叫「{name}」的分组了"));
    }
    c.execute("UPDATE collections SET name = ?2 WHERE id = ?1", params![id, name])
        .map_err(e2s)?;
    Ok(())
}

/// 删分组：**只删这个分组和它的成员行，素材本身一个都不动**。
pub fn coll_delete(c: &Connection, id: i64) -> Res<()> {
    c.execute("DELETE FROM collection_assets WHERE collection_id = ?1", params![id])
        .map_err(e2s)?;
    c.execute("DELETE FROM collections WHERE id = ?1", params![id])
        .map_err(e2s)?;
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
            .map_err(e2s)?;
        for g in gids {
            n += st.execute(params![id, g, now]).map_err(e2s)?;
        }
    }
    Ok(n)
}

pub fn coll_remove(c: &Connection, id: i64, gids: &[i64]) -> Res<usize> {
    let mut n = 0usize;
    {
        let mut st = c
            .prepare("DELETE FROM collection_assets WHERE collection_id = ?1 AND asset_gid = ?2")
            .map_err(e2s)?;
        for g in gids {
            n += st.execute(params![id, g]).map_err(e2s)?;
        }
    }
    Ok(n)
}

pub fn coll_gids(c: &Connection, id: i64) -> Res<Vec<i64>> {
    let mut st = c
        .prepare("SELECT asset_gid FROM collection_assets WHERE collection_id = ?1 ORDER BY added_at, asset_gid")
        .map_err(e2s)?;
    let rows = st.query_map(params![id], |r| r.get(0)).map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/// 这张素材在哪些分组里（详情面板显示"[分组] 客户稿 ×"用）。
pub fn coll_of(c: &Connection, gid: i64) -> Res<Vec<i64>> {
    let mut st = c
        .prepare("SELECT collection_id FROM collection_assets WHERE asset_gid = ?1")
        .map_err(e2s)?;
    let rows = st.query_map(params![gid], |r| r.get(0)).map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/* ------------------------------------------------------------------ 灵感板摆位 */

/// 一块板上记下来的摆位：`(gid, x, y, w, h, grp)`。
///
/// 只是"上次摆哪儿"的记忆 —— 板上的图被删了 / 换库了，这一行就自然对不上，
/// 调用方按 gid 过滤即可，**不要**因为对不上而删行（图可能只是暂时不在）。
pub fn board_layout(c: &Connection, board: &str) -> Res<Vec<(i64, f64, f64, f64, f64, i64)>> {
    let mut st = c
        .prepare("SELECT gid, x, y, w, h, grp FROM board_items WHERE board = ?1")
        .map_err(e2s)?;
    let rows = st
        .query_map(params![board], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })
        .map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/// 记住一批图的摆位（松手时逐张写，不整板重写）。
///
/// 整批一个事务（0.4.24）：拖 200 张松手原来要 200 次 autocommit，松手那一下会顿。
pub fn board_place(c: &Connection, board: &str, rows: &[(i64, f64, f64, f64, f64)]) -> Res<usize> {
    if rows.is_empty() {
        return Ok(0);
    }
    let now = db::now_ts();
    let tx = c.unchecked_transaction().map_err(e2s)?;
    let mut n = 0usize;
    {
        let mut st = tx
            .prepare(
                "INSERT INTO board_items (board, gid, x, y, w, h, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(board, gid) DO UPDATE SET
               x=excluded.x, y=excluded.y, w=excluded.w, h=excluded.h, updated_at=excluded.updated_at",
            )
            .map_err(e2s)?;
        for &(g, x, y, w, h) in rows {
            n += st
                .execute(params![board, g, x, y, w, h, now])
                .map_err(e2s)?;
        }
    }
    tx.commit().map_err(e2s)?;
    Ok(n)
}

/* ------ 灵感板文字便签（0.4.14，0.4.17 起带字体外观与组） ------ */

/// 一行便签。字段多了以后不再用元组 —— 十个值的元组在命令层/前端都容易错位。
#[derive(Debug, Clone, serde::Serialize)]
pub struct BoardNoteRow {
    pub nid: i64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub text: String,
    /// 字体族名（空 = 系统默认）
    pub font: String,
    /// 字号（画布单位，0 = 默认 12.5；缩放框时按比例跟着变）
    pub fs: f64,
    /// 文字颜色 `#rrggbb`（空 = 默认浅色）
    pub color: String,
    pub bold: bool,
    /// 所属组（0 = 没打组）
    pub grp: i64,
}

/// 一块板上的全部便签，按创建顺序（nid 升序）。
pub fn board_notes(c: &Connection, board: &str) -> Res<Vec<BoardNoteRow>> {
    let mut st = c
        .prepare(
            "SELECT nid, x, y, w, h, text, font, fs, color, bold, grp
             FROM board_notes WHERE board = ?1 ORDER BY nid",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![board], |r| {
            Ok(BoardNoteRow {
                nid: r.get(0)?,
                x: r.get(1)?,
                y: r.get(2)?,
                w: r.get(3)?,
                h: r.get(4)?,
                text: r.get(5)?,
                font: r.get(6)?,
                fs: r.get(7)?,
                color: r.get(8)?,
                bold: r.get::<_, i64>(9)? != 0,
                grp: r.get(10)?,
            })
        })
        .map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/// 新建或更新一张便签。`nid = None` 表示新建（发号 `MAX(nid)+1`），返回最终 nid。
///
/// `look` 是外观（字体/字号/颜色/加粗）；传 `None` 表示**保留库里原样** ——
/// 只挪位置时不该把外观冲成默认值。
#[allow(clippy::too_many_arguments)]
pub fn board_note_put(
    c: &Connection,
    board: &str,
    nid: Option<i64>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    text: &str,
    look: Option<(&str, f64, &str, bool)>,
) -> Res<i64> {
    let nid = match nid {
        Some(n) if n > 0 => n,
        _ => {
            let max: i64 = c
                .query_row(
                    "SELECT COALESCE(MAX(nid), 0) FROM board_notes WHERE board = ?1",
                    params![board],
                    |r| r.get(0),
                )
                .map_err(e2s)?;
            max + 1
        }
    };
    match look {
        Some((font, fs, color, bold)) => c.execute(
            "INSERT INTO board_notes (board, nid, x, y, w, h, text, font, fs, color, bold, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(board, nid) DO UPDATE SET
               x=excluded.x, y=excluded.y, w=excluded.w, h=excluded.h,
               text=excluded.text, font=excluded.font, fs=excluded.fs,
               color=excluded.color, bold=excluded.bold, updated_at=excluded.updated_at",
            params![board, nid, x, y, w, h, text, font, fs, color, bold as i64, db::now_ts()],
        ),
        None => c.execute(
            "INSERT INTO board_notes (board, nid, x, y, w, h, text, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(board, nid) DO UPDATE SET
               x=excluded.x, y=excluded.y, w=excluded.w, h=excluded.h,
               text=excluded.text, updated_at=excluded.updated_at",
            params![board, nid, x, y, w, h, text, db::now_ts()],
        ),
    }
    .map_err(e2s)?;
    Ok(nid)
}

/// 打组 / 解组：把 `keys` 里的成员设成同一个组。
///
/// **key 的编码**：`> 0` 是图片的全局 gid，`< 0` 是便签的 nid 取负 —— 和前端
/// `pos` 的 key 完全一致（图片 id 恒正，便签用负数当 key），两边不需要再翻译一次。
/// `grp <= 0` 表示**解组**；`grp == -1` 表示新建一组（发号：本板当前最大组号 + 1）。
/// 返回最终生效的组号（解组返回 0）。
pub fn board_group_set(c: &Connection, board: &str, keys: &[i64], grp: i64) -> Res<i64> {
    let g = if grp == -1 {
        let a: i64 = c
            .query_row(
                "SELECT COALESCE(MAX(grp), 0) FROM board_items WHERE board = ?1",
                params![board],
                |r| r.get(0),
            )
            .map_err(e2s)?;
        let b: i64 = c
            .query_row(
                "SELECT COALESCE(MAX(grp), 0) FROM board_notes WHERE board = ?1",
                params![board],
                |r| r.get(0),
            )
            .map_err(e2s)?;
        a.max(b) + 1
    } else {
        grp.max(0)
    };
    let now = db::now_ts();
    let mut st_img = c
        .prepare("UPDATE board_items SET grp = ?3, updated_at = ?4 WHERE board = ?1 AND gid = ?2")
        .map_err(e2s)?;
    let mut st_note = c
        .prepare("UPDATE board_notes SET grp = ?3, updated_at = ?4 WHERE board = ?1 AND nid = ?2")
        .map_err(e2s)?;
    for &k in keys {
        if k > 0 {
            st_img.execute(params![board, k, g, now]).map_err(e2s)?;
        } else if k < 0 {
            st_note.execute(params![board, -k, g, now]).map_err(e2s)?;
        }
    }
    Ok(g)
}

/// 删一张便签。
pub fn board_note_del(c: &Connection, board: &str, nid: i64) -> Res<usize> {
    c.execute(
        "DELETE FROM board_notes WHERE board = ?1 AND nid = ?2",
        params![board, nid],
    )
    .map_err(e2s)
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

    /// 便签：发号、改写、删、**板之间互不串**
    #[test]
    fn board_notes_roundtrip() {
        let d = tmp("notes");
        let c = open(&d.join("index.db")).unwrap();

        let a = board_note_put(&c, "tag:参考", None, 10.0, 20.0, 240.0, 120.0, "第一张", None)
            .unwrap();
        let b = board_note_put(&c, "tag:参考", None, 30.0, 40.0, 240.0, 90.0, "第二张", None)
            .unwrap();
        assert!(b > a, "新便签要拿到更大的 nid");

        let rows = board_notes(&c, "tag:参考").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text, "第一张");
        assert_eq!(rows[1].x, 30.0);
        assert_eq!(rows[0].grp, 0, "新建便签默认没打组");
        assert!(rows[0].font.is_empty() && rows[0].fs == 0.0, "默认外观为空");

        // 同一个 nid 再写一次 = 更新，不是多一行
        board_note_put(&c, "tag:参考", Some(a), 11.0, 21.0, 260.0, 140.0, "改过", None).unwrap();
        let rows = board_notes(&c, "tag:参考").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text, "改过");
        assert_eq!(rows[0].w, 260.0);

        // 外观单独写：字体 / 字号 / 颜色 / 加粗都要原样回来
        board_note_put(
            &c,
            "tag:参考",
            Some(a),
            11.0,
            21.0,
            260.0,
            140.0,
            "改过",
            Some(("Microsoft YaHei", 31.5, "#ffcc00", true)),
        )
        .unwrap();
        let rows = board_notes(&c, "tag:参考").unwrap();
        assert_eq!(rows[0].font, "Microsoft YaHei");
        assert_eq!(rows[0].fs, 31.5);
        assert_eq!(rows[0].color, "#ffcc00");
        assert!(rows[0].bold);

        // 只挪位置（look = None）不许把外观冲掉
        board_note_put(&c, "tag:参考", Some(a), 12.0, 22.0, 260.0, 140.0, "改过", None).unwrap();
        let rows = board_notes(&c, "tag:参考").unwrap();
        assert_eq!(rows[0].font, "Microsoft YaHei", "只挪位置不该重置字体");
        assert_eq!(rows[0].fs, 31.5);

        // 打组：图片 gid 为正、便签 nid 取负；解组写回 0
        let g = board_group_set(&c, "tag:参考", &[7, -a], -1).unwrap();
        assert!(g > 0);
        let rows = board_notes(&c, "tag:参考").unwrap();
        assert_eq!(rows[0].grp, g, "便签要进同一个组");
        assert_eq!(rows[1].grp, 0, "没点的便签不许跟着进组");
        board_group_set(&c, "tag:参考", &[7, -a], 0).unwrap();
        assert_eq!(board_notes(&c, "tag:参考").unwrap()[0].grp, 0, "解组后回到 0");

        // 别的板看不到这条
        assert!(board_notes(&c, "snap:7").unwrap().is_empty());
        // 别的板上发的号从 1 重新开始
        let o = board_note_put(&c, "snap:7", None, 0.0, 0.0, 240.0, 120.0, "别的板", None)
            .unwrap();
        assert_eq!(o, 1);

        assert_eq!(board_note_del(&c, "tag:参考", a).unwrap(), 1);
        assert_eq!(board_notes(&c, "tag:参考").unwrap().len(), 1);
        assert_eq!(board_note_del(&c, "tag:参考", a).unwrap(), 0);

        let _ = std::fs::remove_dir_all(&d);
    }
}
