//! SQLite 索引层。
//!
//! 三条纪律（MVP 方案 §3.6 / §4.2）：
//! 1. 索引是**派生数据**，任何时刻可删可重建；真相在文件系统。
//! 2. `assets` 只存 `dir_id + name`，**不存完整路径** —— 重命名目录才是一条 SQL。
//! 3. 每个目录做「按 name 对账」：更新/新增/删除，保留用户数据（thumb_key / rating）。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::*;

pub type Res<T> = Result<T, String>;

fn e2s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);

/* 库自己的根（T28）：这个文件**自己说明**它索引的是哪个文件夹。
   真正的"我挂了哪几个库"记在注册库（`index.db`）里；这里留一份是为了
   单个库文件被拷走/单独打开时仍然自洽（`App::rebuild` 也只认库内这份）。 */
CREATE TABLE IF NOT EXISTS libs (
  id INTEGER PRIMARY KEY,
  root TEXT NOT NULL UNIQUE,
  added_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS dirs (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  parent_id INTEGER REFERENCES dirs(id),
  mtime INTEGER NOT NULL DEFAULT 0,
  scanned_at INTEGER NOT NULL DEFAULT 0,
  child_cnt INTEGER NOT NULL DEFAULT 0,
  asset_cnt INTEGER NOT NULL DEFAULT 0,
  /* 整棵子树的素材数（自己 + 所有后代）。扫描时顺带维护 ——
     之前是每次列目录用递归 CTE 现算，100 万行要 534ms，树一开就卡。 */
  cnt_all INTEGER NOT NULL DEFAULT 0,
  needs_rescan INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_dirs_parent ON dirs(parent_id);

CREATE TABLE IF NOT EXISTS assets (
  id INTEGER PRIMARY KEY,
  dir_id INTEGER NOT NULL REFERENCES dirs(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  ext TEXT NOT NULL,
  kind INTEGER NOT NULL,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  width INTEGER,
  height INTEGER,
  thumb_key TEXT,
  thumb_state INTEGER NOT NULL DEFAULT 0,
  quality INTEGER NOT NULL DEFAULT 0,
  hash_x BLOB,
  phash INTEGER,
  /* 5 个主色（`rrggbb,rrggbb,…`）与"最像哪个色桶"（T24）。
     出缩略图时顺手从解码后的像素里算 —— 那时图已经在内存里，多花不到 1ms。 */
  palette TEXT,
  color TEXT,
  note TEXT,
  rating INTEGER NOT NULL DEFAULT 0,
  /* 来源 URL（T23）：找"这图哪来的"。空串与 NULL 同义，读的时候统一成空串 */
  source_url TEXT,
  /* 引用关系扫过没有（T26）：0 = 没扫过。工程文件是"扫一次就够"的，
     用时间戳而不是布尔，将来想按"很久没扫的优先"重扫也有依据。 */
  refs_at INTEGER NOT NULL DEFAULT 0,
  added_at INTEGER NOT NULL,
  missing INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_assets_dir   ON assets(dir_id, name);
/* 带 kind 前缀的那两条只能给「按类型筛」用；不筛类型时（默认视图就是全部类型）
   排序完全用不上它们 → 10 万行要全表排序（实测 117ms）。下面三条是**纯排序索引**，
   让 ORDER BY ... LIMIT 200 能直接顺着索引取前 200 行就停。 */
CREATE INDEX IF NOT EXISTS idx_assets_kind_mtime ON assets(kind, mtime DESC);
CREATE INDEX IF NOT EXISTS idx_assets_mtime2 ON assets(mtime DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_assets_size2  ON assets(size DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_assets_name2  ON assets(name COLLATE NOCASE ASC, id ASC);

-- 全文检索：trigram 分词，中文任意子串可命中
CREATE VIRTUAL TABLE IF NOT EXISTS assets_fts USING fts5(
  name, note, tags, content='', tokenize='trigram'
);

-- 缩略图 LRU（字节数记账，供 gc 使用）
CREATE TABLE IF NOT EXISTS thumb_lru (
  key TEXT PRIMARY KEY,
  bytes INTEGER NOT NULL,
  last_used INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lru_time ON thumb_lru(last_used);

/* 手动标签的关联（T21 / T28）。
   标签**本体**（名字 / 颜色 / 分组 / 自动规则）在注册库 `reg::tags` 里，只有一份；
   这里只存「这张素材打了这个标签」，`tag_id` 就是注册库里的 id。
   为什么不再建外键到 tags：跨库外键 SQLite 做不到，硬做就得在删标签时逐库对账 ——
   而删标签本来就要逐库清关联（`App::delete_tag`），那条路径是唯一的入口，写在那里更清楚。
   素材一侧的外键保留：删素材（重扫时的对账）靠 ON DELETE CASCADE 把关联一起带走。 */
/* 标签体系**每库一份**（0.3.9 起）。
   以前它住在注册库里、全库共享 —— 于是切到 A 库能看到 B 库的标签，
   换台机器或重装注册库就等于标签全丢。现在跟着库文件走，
   `asset_tags.tag_id` 指的就是**本库** `tags.id`（不再是全局 id）。 */
CREATE TABLE IF NOT EXISTS tag_groups (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  color TEXT,
  sort INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS tags (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL DEFAULT 0,
  color TEXT,
  group_id INTEGER,
  auto_rule TEXT
);

/* 快照（0.3.10 起每库一份）：一批图片的书签，条目身份是**内容哈希**。
   以前住在注册库、"跨库存在"；现在跟着库走 —— 整库搬走/移除再导入，
   灵感板还在。跨库的旧快照在升级时按条目归属拆到各库（见 `App::migrate_snapshots`）。 */
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

CREATE TABLE IF NOT EXISTS asset_tags (
  asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
  tag_id   INTEGER NOT NULL,
  PRIMARY KEY (asset_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_asset_tags_tag ON asset_tags(tag_id, asset_id);

/* 分组（0.4.4 起每库一份）：用户自己攒的一撮素材。
   以前住在注册库、成员是**全局** asset id（一个分组能横跨几个库）—— 切库时
   分组列表纹丝不动，用户看到的是"A 库的分组里装着 B 库的图"。
   现在跟着库走：成员是本库 `assets.id`，删素材靠外键级联一起清。
   老数据在升级时按成员的库号拆到各库（`App::migrate_collections_into_libs`）。 */
CREATE TABLE IF NOT EXISTS collections (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  sort INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS collection_assets (
  collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  asset_id      INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
  added_at      INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (collection_id, asset_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_collection_assets_asset ON collection_assets(asset_id);

/* 引用关系（T26）：**谁用了这张图 / 哪些引用已经找不到文件**。
   - `src_id` 是"工程文件"（.blend / .psd / .max / .c4d …）在本库的 id；
   - `dst_path` 是工程里写的那条路径（原样存，解析不出来时它就是唯一的线索）；
   - `dst_id` 解析成功时指向本库的素材，失败为 NULL 且 `missing=1`。
   扫工程文件是"看字符串"：二进制里出现的可打印路径段，只在结尾是已知媒体扩展名时才认，
   宁可漏也不能把一堆随机字节当引用（假阳性会让「这里丢了 200 个引用」变成噪音）。 */
CREATE TABLE IF NOT EXISTS refs (
  src_id   INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
  dst_path TEXT NOT NULL,
  dst_id   INTEGER,
  kind     INTEGER NOT NULL DEFAULT 0,
  missing  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (src_id, dst_path)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_refs_dst ON refs(dst_id);
CREATE INDEX IF NOT EXISTS idx_refs_missing ON refs(missing) WHERE missing = 1;

-- 真实文件操作日志，供撤销（定长 20，写满删最旧）
CREATE TABLE IF NOT EXISTS ops (
  id INTEGER PRIMARY KEY,
  kind INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  payload TEXT NOT NULL,
  undone INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_ops_ts ON ops(ts DESC);
"#;

/// 索引库结构版本（`PRAGMA user_version`）。
///
/// **0 = 0.1.16 及更早建的库**：那时没有版本号，只能靠 `migrate()` 逐列探测补齐。
/// **1 = 从 0.1.17 起由版本号驱动**：老库补完列后打上 1，以后每次改结构 +1，
/// 并在 `MIGRATIONS` 里对应一条 SQL。
///
/// 为什么非要这个：真实用户手上已经有几万行的库了，靠"探测列存不存在"能加列，
/// 但改不了**语义**（列类型变了、需要按新规则回填、需要删掉废弃列），
/// 一旦出现那种迁移，没有版本号就只能猜。
pub const SCHEMA_VERSION: i32 = 1;

/// 按版本号推进的迁移表：下标 = 目标版本（`MIGRATIONS[1]` 把库带到 v1）。
///
/// **只增不改**：已经发出去的版本对应的 SQL 永远不许动，否则老库和新库会走岔。
const MIGRATIONS: &[&str] = &[
    /* v0 → v1：纯打标。列已经由 `migrate()` 补齐，这里没有结构改动。 */
    "",
];

fn schema_version(c: &Connection) -> Res<i32> {
    c.query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(e2s)
}

fn set_schema_version(c: &Connection, v: i32) -> Res<()> {
    c.execute_batch(&format!("PRAGMA user_version={v}"))
        .map_err(e2s)
}

/// 把库推进到 `SCHEMA_VERSION`。返回是否真的动过。
pub fn migrate_versioned(c: &Connection) -> Res<bool> {
    let mut v = schema_version(c)?;
    if v >= SCHEMA_VERSION {
        return Ok(false);
    }
    /* v0 的老库：`migrate()` 已经把所有列补齐，直接算作 v1 */
    if v == 0 {
        v = 1;
    }
    while v < SCHEMA_VERSION {
        let sql = MIGRATIONS.get(v as usize).copied().unwrap_or("");
        if !sql.is_empty() {
            c.execute_batch(sql).map_err(e2s)?;
        }
        v += 1;
    }
    set_schema_version(c, SCHEMA_VERSION)?;
    Ok(true)
}

/// 打开一个库文件（`libs/lib-<id>.db`）：建表 + 迁移。
pub fn open(path: &Path) -> Res<Connection> {
    let c = open_raw(path)?;
    c.execute_batch(SCHEMA).map_err(e2s)?;
    migrate(&c)?;
    migrate_versioned(&c)?;
    Ok(c)
}

/// 只带 pragma 与坏库留证的打开（注册库 `reg::open` 也走这条）。
pub fn open_raw(path: &Path) -> Res<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(e2s)?;
    }
    match open_inner(path) {
        Ok(c) => Ok(c),
        Err(e) if is_corrupt(&e) => {
            // 坏库留证（T13）：索引是**派生数据**，但坏掉的那份是唯一的排查线索。
            // 直接删掉 = 以后再也说不清"为什么坏了"；改名留着，容量也就几十 MB。
            let stamp = now_ts();
            let bak = path.with_file_name(format!("{}.corrupt-{}.db", stem_of(path), stamp));
            let _ = std::fs::rename(path, &bak);
            // WAL / SHM 必须一起挪走，否则新库会读到旧日志
            for ext in ["-wal", "-shm"] {
                let side = PathBuf::from(format!("{}{}", path.to_string_lossy(), ext));
                if side.exists() {
                    let _ = std::fs::rename(&side, format!("{}{}", bak.to_string_lossy(), ext));
                }
            }
            eprintln!("索引文件损坏，已留证到 {}：{}", bak.to_string_lossy(), e);
            open_inner(path)
        }
        Err(e) => Err(e),
    }
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "index".into())
}

/// 只有"文件不是数据库 / 结构损坏 / 加密"才当成坏库；**被别的进程锁住不算** ——
/// 那种情况一改名，等于把正在用的库从另一个实例脚下抽走。
fn is_corrupt(e: &str) -> bool {
    let s = e.to_lowercase();
    s.contains("not a database")
        || s.contains("malformed")
        || s.contains("database disk image is corrupt")
        || s.contains("file is encrypted")
}

fn open_inner(path: &Path) -> Res<Connection> {
    let c = Connection::open(path).map_err(e2s)?;
    c.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA busy_timeout=3000;
         PRAGMA cache_size=-64000;
         PRAGMA temp_store=MEMORY;
         PRAGMA mmap_size=268435456;
         PRAGMA page_size=8192;
         PRAGMA foreign_keys=ON;",
    )
    .map_err(e2s)?;
    // 只读地自检一次：真损坏在这一步就会报 "database disk image is corrupt"，
    // 而不是等用户点了某个按钮才炸
    let _: i64 = c
        .query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))
        .map_err(e2s)?;
    Ok(c)
}

fn has_column(c: &Connection, table: &str, col: &str) -> bool {
    let sql = format!("PRAGMA table_info({})", table);
    let mut found = false;
    if let Ok(mut st) = c.prepare(&sql) {
        if let Ok(mut rows) = st.query_map([], |r| r.get::<_, String>(1)) {
            while let Some(Ok(n)) = rows.next() {
                if n == col {
                    found = true;
                    break;
                }
            }
        }
    }
    found
}

fn has_table(c: &Connection, table: &str) -> bool {    c.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![table],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// 老库加列 + `cnt_all` 一次性回填。
///
/// 为什么要版本标记：`ALTER TABLE` 只在第一次跑，但**新库建表就有这一列**、
/// 内容是 0 —— 得靠一次回填才有真数。用 meta 里的标记保证只补一次。
fn migrate(c: &Connection) -> Res<()> {
    // 0.6.x → 0.7.0：素材加了「来源 URL」（评分/注释的列本来就有）。
    if !has_column(c, "assets", "source_url") {
        c.execute("ALTER TABLE assets ADD COLUMN source_url TEXT", [])
            .map_err(e2s)?;
    }
    // 0.7.x → 0.8.0：主色板 / 色桶（T24）、引用扫描标记（T26）。
    // 三条都判存在，可反复执行（新库建表就带着，不会重复 ALTER）。
    for (col, ddl) in [
        ("color", "ALTER TABLE assets ADD COLUMN color TEXT"),
        ("refs_at", "ALTER TABLE assets ADD COLUMN refs_at INTEGER NOT NULL DEFAULT 0"),
        ("palette", "ALTER TABLE assets ADD COLUMN palette TEXT"),
    ] {
        if !has_column(c, "assets", col) {
            c.execute(ddl, []).map_err(e2s)?;
        }
    }
    // 按评分排序（T23）：不加这条，65k 行的库一排序就全表扫
    c.execute(
        "CREATE INDEX IF NOT EXISTS idx_assets_rating ON assets(rating DESC, id DESC)",
        [],
    )
    .map_err(e2s)?;
    // 颜色筛选（T24）：色桶是低频值（十几个），单列索引足够
    c.execute("CREATE INDEX IF NOT EXISTS idx_assets_color ON assets(color)", [])
        .map_err(e2s)?;
    // 重复检测（T25）：只索引"算过哈希的"，没算过的行不进索引，省体积
    c.execute(
        "CREATE INDEX IF NOT EXISTS idx_assets_hash ON assets(hash_x) WHERE hash_x IS NOT NULL",
        [],
    )
    .map_err(e2s)?;

    /* 老库（0.7.x 及以前）里 tags / tag_groups 还在这张表里。0.8.0 起它们搬到注册库,
       但**迁移不是在这里做**：单库文件不知道自己是不是"曾经的主库"。
       真实迁移在 `App::open`（见 `migrate_legacy`），这里只负责把老库新加的列补齐。 */
    if has_table(c, "tags") {
        if !has_column(c, "tags", "color") {
            c.execute("ALTER TABLE tags ADD COLUMN color TEXT", []).map_err(e2s)?;
        }
        if !has_column(c, "tags", "group_id") {
            c.execute("ALTER TABLE tags ADD COLUMN group_id INTEGER", []).map_err(e2s)?;
        }
        /* 索引必须建在补列**之后**：0.4.x 的 tags 没有 group_id，
           把这条放进 SCHEMA 会让老库在 `execute_batch(SCHEMA)` 阶段直接报错。 */
        c.execute(
            "CREATE INDEX IF NOT EXISTS idx_tags_group ON tags(group_id)",
            [],
        )
        .map_err(e2s)?;
    }

    if !has_column(c, "dirs", "cnt_all") {
        c.execute("ALTER TABLE dirs ADD COLUMN cnt_all INTEGER NOT NULL DEFAULT 0", [])
            .map_err(e2s)?;
        rebuild_cnt_all(c)?;
        c.execute(
            "INSERT INTO meta (k,v) VALUES ('cnt_all_v','1') ON CONFLICT(k) DO UPDATE SET v='1'",
            [],
        )
        .map_err(e2s)?;
        return Ok(());
    }
    let done: Option<String> = c
        .query_row("SELECT v FROM meta WHERE k='cnt_all_v'", [], |r| r.get(0))
        .optional()
        .map_err(e2s)?;
    if done.is_none() {
        rebuild_cnt_all(c)?;
        c.execute(
            "INSERT INTO meta (k,v) VALUES ('cnt_all_v','1') ON CONFLICT(k) DO UPDATE SET v='1'",
            [],
        )
        .map_err(e2s)?;
    }
    Ok(())
}

/// 全量重算 `dirs.cnt_all`：先数每个目录**直接**装了多少素材，再按深度从深到浅累加子树。
///
/// 为什么不在 SQL 里用递归 CTE：SQLite 的递归 CTE 只能自上而下展开，
/// 算「每棵子树的总数」要 N 次重复展开（1 万目录就是上亿行）。内存里两趟是线性的。
pub fn rebuild_cnt_all(c: &Connection) -> Res<()> {
    use std::collections::HashMap;

    let mut parent: HashMap<i64, Option<i64>> = HashMap::new();
    {
        let mut st = c.prepare("SELECT id, parent_id FROM dirs").map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))
            .map_err(e2s)?;
        for x in rows.filter_map(|x| x.ok()) {
            parent.insert(x.0, x.1);
        }
    }
    if parent.is_empty() {
        return Ok(());
    }
    let mut direct: HashMap<i64, i64> = HashMap::new();
    {
        let mut st = c
            .prepare("SELECT dir_id, COUNT(*) FROM assets WHERE missing=0 GROUP BY dir_id")
            .map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(e2s)?;
        for x in rows.filter_map(|x| x.ok()) {
            direct.insert(x.0, x.1);
        }
    }
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for (id, p) in &parent {
        if let Some(p) = p {
            children.entry(*p).or_default().push(*id);
        }
    }
    // 深度（带防环上限）
    let mut depth: HashMap<i64, i64> = HashMap::new();
    for &id in parent.keys() {
        let mut d = 0i64;
        let mut cur = parent.get(&id).copied().flatten();
        while let Some(p) = cur {
            d += 1;
            if d > 128 {
                break;
            }
            cur = parent.get(&p).copied().flatten();
        }
        depth.insert(id, d);
    }
    let mut order: Vec<i64> = parent.keys().copied().collect();
    order.sort_by_key(|id| -depth.get(id).copied().unwrap_or(0));

    let mut all: HashMap<i64, i64> = HashMap::with_capacity(order.len());
    for id in &order {
        let mut n = direct.get(id).copied().unwrap_or(0);
        for cid in children.get(id).map(|v| v.as_slice()).unwrap_or(&[]) {
            n += all.get(cid).copied().unwrap_or(0);
        }
        all.insert(*id, n);
    }

    let tx = c.unchecked_transaction().map_err(e2s)?;
    {
        let mut st = tx
            .prepare("UPDATE dirs SET cnt_all=?1, asset_cnt=?2 WHERE id=?3")
            .map_err(e2s)?;
        for (id, n) in &all {
            st.execute(params![n, direct.get(id).copied().unwrap_or(0), id]).map_err(e2s)?;
        }
    }
    tx.commit().map_err(e2s)?;
    Ok(())
}

/// 一个目录变了 → 沿父链把 `cnt_all` 重算到根。
///
/// 每层只是一次「自己直接几条 + 子目录 cnt_all 之和」，都走索引，
/// 深度最多几十层，比每次开树现算便宜得多。
pub fn recalc_dir_chain(c: &Connection, dir_id: i64) -> Res<()> {
    let mut cur = Some(dir_id);
    let mut guard = 0;
    while let Some(id) = cur {
        c.execute(
            "UPDATE dirs SET
               asset_cnt = (SELECT COUNT(*) FROM assets a WHERE a.dir_id=?1 AND a.missing=0),
               cnt_all   = (SELECT COUNT(*) FROM assets a WHERE a.dir_id=?1 AND a.missing=0)
                         + (SELECT COALESCE(SUM(x.cnt_all),0) FROM dirs x WHERE x.parent_id=?1)
             WHERE id=?1",
            params![id],
        )
        .map_err(e2s)?;
        cur = c
            .query_row("SELECT parent_id FROM dirs WHERE id=?1", params![id], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .optional()
            .map_err(e2s)?
            .flatten();
        guard += 1;
        if guard > 160 {
            break;
        }
    }
    Ok(())
}

pub fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

// ---------------------------------------------------------------- dirs

pub fn upsert_dir(c: &Connection, path: &str, parent: Option<i64>, mtime: i64) -> Res<i64> {
    let name = file_name_of(path);
    c.query_row(
        "INSERT INTO dirs (path, name, parent_id, mtime) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET mtime = excluded.mtime
         RETURNING id",
        params![path, name, parent, mtime],
        |r| r.get(0),
    )
    .map_err(e2s)
}

pub fn add_lib(c: &Connection, root: &str) -> Res<i64> {
    c.query_row(
        "INSERT INTO libs (root, added_at) VALUES (?1, ?2)
         ON CONFLICT(root) DO UPDATE SET added_at = excluded.added_at RETURNING id",
        params![root, now_ts()],
        |r| r.get(0),
    )
    .map_err(e2s)
}

/// 清空索引（`libs` 保留）：重建索引用。
///
/// 为什么连 `dirs` 一起清：目录行里挂着 `cnt_all` / `mtime` 这些缓存量，
/// 清一半会让目录树显示旧数字，反而更容易误导。整体重扫一遍最干净。
pub fn reset_index(c: &Connection) -> Res<(i64, i64)> {
    let a = c.execute("DELETE FROM assets", []).map_err(e2s)?;
    let d = c.execute("DELETE FROM dirs", []).map_err(e2s)?;
    let _ = c.execute("DELETE FROM thumb_lru", []).ok();
    c.execute("UPDATE libs SET added_at=added_at", []).ok();
    Ok((a as i64, d as i64))
}

/// 库根数量（界面上的「最多 3 个库」用它判断还能不能加）。
pub fn lib_count(c: &Connection) -> Res<i64> {
    c.query_row("SELECT COUNT(*) FROM libs", [], |r| r.get(0))
        .map_err(e2s)
}

/// 所有库根路径。用于「别把数据文件夹设进素材目录」这条防护。
pub fn lib_roots(c: &Connection) -> Res<Vec<String>> {
    let mut st = c.prepare("SELECT root FROM libs").map_err(e2s)?;
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(e2s)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 从素材库移除一个库根：只删索引行（`dirs` 整棵子树 + `libs` 那一行）。
///
/// **绝不碰磁盘**：移除库 = 「软件不再管这个文件夹」，不是删文件。
/// 子树用递归 CTE 按 `parent_id` 走，不靠 `path LIKE` —— 路径里有 `_` / `%`
/// 会被当成通配符，删错整棵树。
pub fn remove_lib(c: &Connection, root: &str) -> Res<usize> {
    let n = c
        .execute(
            "WITH RECURSIVE sub(id) AS (
               SELECT id FROM dirs WHERE path = ?1
               UNION ALL
               SELECT d.id FROM dirs d JOIN sub s ON d.parent_id = s.id
             )
             DELETE FROM dirs WHERE id IN (SELECT id FROM sub)",
            params![root],
        )
        .map_err(e2s)?;
    c.execute("DELETE FROM libs WHERE root = ?1", params![root])
        .map_err(e2s)?;
    Ok(n)
}

/// 用户数据导出（T13）：库根 + 每个素材的"用户层"字段（评分/标签/颜色）。
///
/// 为什么不用 SQLite 备份文件：那是二进制、跨版本、还带绝对路径，
/// 用户拿去换机恢复时装不上。JSON 能看能改，也能在索引重扫后按路径回填。
pub fn export_userdata(c: &Connection) -> Res<String> {
    let libs = list_libs(c)?;
    let mut dirs: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    {
        let mut st = c.prepare("SELECT id, path FROM dirs").map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(e2s)?;
        for x in rows.filter_map(|x| x.ok()) {
            dirs.insert(x.0, x.1);
        }
    }
    let mut items: Vec<serde_json::Value> = Vec::new();
    {
        let mut st = c
            .prepare(
                "SELECT dir_id, name, ext, size, mtime, rating, note
                 FROM assets WHERE missing=0 ORDER BY id",
            )
            .map_err(e2s)?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<i64>>(5).unwrap_or(None),
                    r.get::<_, Option<String>>(6).unwrap_or(None),
                ))
            })
            .map_err(e2s)?;
        for x in rows.filter_map(|x| x.ok()) {
            let dir = dirs.get(&x.0).cloned().unwrap_or_default();
            items.push(serde_json::json!({
                "dir": dir,
                "name": x.1,
                "ext": x.2,
                "size": x.3,
                "mtime": x.4,
                "rating": x.5,
                "note": x.6,
            }));
        }
    }
    let doc = serde_json::json!({
        "app": "AssetManager",
        "version": env!("CARGO_PKG_VERSION"),
        "exportedAt": now_ts(),
        "note": "索引可重建；这份文件只保留用户层数据（库根 / 评分 / 备注）",
        "libs": libs,
        "assets": items,
    });
    serde_json::to_string_pretty(&doc).map_err(e2s)
}

pub fn list_libs(c: &Connection) -> Res<Vec<String>> {
    let mut s = c.prepare("SELECT root FROM libs ORDER BY id").map_err(e2s)?;
    let v = s
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(e2s)?
        .filter_map(|x| x.ok())
        .collect();
    Ok(v)
}

pub fn dir_by_id(c: &Connection, id: i64) -> Res<Option<(String, Option<i64>)>> {
    c.query_row("SELECT path, parent_id FROM dirs WHERE id=?1", params![id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
    })
    .optional()
    .map_err(e2s)
}

/// 按路径查目录行 id（新建文件夹时要用父目录的 id 挂上去）。
pub fn dir_id_by_path(c: &Connection, path: &str) -> Res<Option<i64>> {
    c.query_row("SELECT id FROM dirs WHERE path=?1", params![path], |r| r.get(0))
        .optional()
        .map_err(e2s)
}

/// 同 `dir_id_by_path`，但按**规范化路径**比（斜杠、大小写、尾分隔符不同也能对上）。
///
/// 只给"新建文件夹"这种一次性动作兜底：全表扫一遍在库里只有几千个目录时是毫秒级，
/// 而在热路径（列表 / 树）上绝不能用。
pub fn find_dir_id_norm(c: &Connection, path: &str) -> Res<Option<i64>> {
    let want = crate::norm_path(path);
    let mut s = c.prepare("SELECT id, path FROM dirs").map_err(e2s)?;
    let rows = s
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(e2s)?;
    for row in rows {
        let (id, p) = row.map_err(e2s)?;
        if crate::norm_path(&p) == want {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// 目录树子节点（懒加载，只查一层）。
///
/// `asset_cnt` 是**整棵子树**的素材数，不是直接子项 ——
/// 库根通常自己不装素材（图都在子目录里），只算直接子项会显示 0，
/// 用户一眼就以为"没扫到"。这里用一条递归 CTE 把同层所有兄弟的子树一起算完，
/// 一次查询而不是每行一发。
pub fn dir_children(c: &Connection, parent: Option<i64>) -> Res<Vec<DirNode>> {
    // cnt_all 是扫描时维护好的，直接读：一条索引查询，不再现场跑递归 CTE
    let sql = "SELECT d.id, d.path, d.name, d.parent_id,
                      (SELECT COUNT(*) FROM dirs x WHERE x.parent_id = d.id) AS cc,
                      d.cnt_all AS ac
               FROM dirs d
               WHERE d.parent_id IS ?1
               ORDER BY d.name COLLATE NOCASE";
    let mut s = c.prepare(sql).map_err(e2s)?;
    let rows = s
        .query_map(params![parent], |r| {
            let cc: i64 = r.get(4)?;
            Ok(DirNode {
                id: r.get(0)?,
                path: r.get(1)?,
                name: r.get(2)?,
                parent_id: r.get(3)?,
                child_cnt: cc,
                asset_cnt: r.get(5)?,
                has_children: cc > 0,
            })
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 目录重命名后修 path 前缀（一条 SQL，不碰 assets）。
pub fn repath_subtree(c: &Connection, dir_id: i64, old_path: &str, new_path: &str) -> Res<usize> {
    c.execute(
        "UPDATE dirs SET path = ?1 || substr(path, length(?2) + 1)
         WHERE path = ?2 OR path LIKE ?2 || '/%' OR path LIKE ?2 || '\\%'",
        params![new_path, old_path],
    )
    .map_err(e2s)?;
    let _ = dir_id;
    Ok(c.execute(
        "UPDATE dirs SET name = ?1 WHERE id = ?2",
        params![file_name_of(new_path), dir_id],
    )
    .map_err(e2s)?)
}

// ---------------------------------------------------------------- assets

#[allow(clippy::too_many_arguments)]
pub fn upsert_asset(
    c: &Connection,
    dir_id: i64,
    name: &str,
    ext: &str,
    kind: i32,
    size: i64,
    mtime: i64,
    width: Option<i64>,
    height: Option<i64>,
) -> Res<i64> {
    c.query_row(
        "INSERT INTO assets (dir_id, name, ext, kind, size, mtime, width, height, added_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
         RETURNING id",
        params![dir_id, name, ext, kind, size, mtime, width, height, now_ts()],
        |r| r.get(0),
    )
    .map_err(e2s)
}

fn fts_add(c: &Connection, id: i64, name: &str, note: &str, tags: &str) -> Res<()> {
    c.execute(
        "INSERT INTO assets_fts (rowid, name, note, tags) VALUES (?1,?2,?3,?4)",
        params![id, name, note, tags],
    )
    .map_err(e2s)?;
    Ok(())
}

fn fts_del(c: &Connection, id: i64, name: &str, note: &str, tags: &str) -> Res<()> {
    c.execute(
        "INSERT INTO assets_fts (assets_fts, rowid, name, note, tags) VALUES ('delete',?1,?2,?3,?4)",
        params![id, name, note, tags],
    )
    .map_err(e2s)?;
    Ok(())
}

/// 一个目录的「对账式」入库：返回 (新增, 更新, 删除)。
///
/// 保留既有行的 thumb_key / rating / note —— 只要 mtime+size 没变就不清缩略图。
pub struct DirSync {
    pub inserted: i64,
    pub updated: i64,
    pub removed: i64,
}

pub fn sync_dir(c: &Connection, dir_id: i64, entries: &[crate::scan::RawEntry]) -> Res<DirSync> {
    use std::collections::HashMap;

    let mut existing: HashMap<String, i64> = HashMap::new();
    {
        let mut s = c
            .prepare("SELECT id, name FROM assets WHERE dir_id=?1")
            .map_err(e2s)?;
        let rows = s
            .query_map(params![dir_id], |r| {
                Ok((r.get::<_, String>(1)?, r.get::<_, i64>(0)?))
            })
            .map_err(e2s)?;
        for (name, id) in rows.filter_map(|x| x.ok()) {
            existing.insert(name, id);
        }
    }

    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut st = DirSync { inserted: 0, updated: 0, removed: 0 };

    for en in entries {
        seen.insert(en.name.as_str());
        match existing.get(&en.name) {
            None => {
                let id = upsert_asset(
                    c, dir_id, &en.name, &en.ext, en.kind, en.size, en.mtime, en.width, en.height,
                )?;
                if id > 0 {
                    fts_add(c, id, &en.name, "", "")?;
                }
                st.inserted += 1;
            }
            Some(id) => {
                // 只在 mtime 或 size 变了时清缩略图（key 由 file_id 派生，移动不清）
                c.execute(
                    "UPDATE assets SET size=?1, mtime=?2, width=?3, height=?4, missing=0,
                       thumb_key = CASE WHEN mtime<>?2 OR size<>?1 THEN NULL ELSE thumb_key END,
                       thumb_state = CASE WHEN mtime<>?2 OR size<>?1 THEN 0 ELSE thumb_state END,
                       /* 内容变了 → 指纹与主色一起作废。
                          不清的话，快照/查重/颜色筛选会拿**旧内容**的结论去匹配这个新文件
                          （实测：同名的另一张图被当成同一张，快照对账会误判为「还找得到」）。 */
                       hash_x  = CASE WHEN mtime<>?2 OR size<>?1 THEN NULL ELSE hash_x END,
                       phash   = CASE WHEN mtime<>?2 OR size<>?1 THEN NULL ELSE phash END,
                       palette = CASE WHEN mtime<>?2 OR size<>?1 THEN '' ELSE palette END,
                       color   = CASE WHEN mtime<>?2 OR size<>?1 THEN '' ELSE color END
                     WHERE id=?5",
                    params![en.size, en.mtime, en.width, en.height, id],
                )
                .map_err(e2s)?;
                st.updated += 1;
            }
        }
    }

    // 删除磁盘上已不存在的行
    let stale: Vec<(i64, String)> = c
        .prepare("SELECT id, name FROM assets WHERE dir_id=?1")
        .map_err(e2s)?
        .query_map(params![dir_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(e2s)?
        .filter_map(|x| x.ok())
        .filter(|(_, n)| !seen.contains(n.as_str()))
        .collect();
    for (id, name) in stale {
        let _ = fts_del(c, id, &name, "", "");
        c.execute("DELETE FROM assets WHERE id=?1", params![id]).map_err(e2s)?;
        st.removed += 1;
    }

    c.execute(
        "UPDATE dirs SET scanned_at=?1,
           child_cnt=(SELECT COUNT(*) FROM dirs WHERE parent_id=?2)
         WHERE id=?2",
        params![now_ts(), dir_id],
    )
    .map_err(e2s)?;
    recalc_dir_chain(c, dir_id)?;

    Ok(st)
}

fn path_join(dir: &str, name: &str) -> String {
    let sep = if dir.contains('\\') { '\\' } else { '/' };
    format!("{}{}{}", dir.trim_end_matches(['/', '\\']), sep, name)
}

fn row_to_asset(r: &rusqlite::Row) -> rusqlite::Result<Asset> {
    let dir_path: String = r.get("dpath")?;
    let name: String = r.get("name")?;
    Ok(Asset {
        id: r.get("id")?,
        dir_id: r.get("dir_id")?,
        path: path_join(&dir_path, &name),
        name,
        ext: r.get("ext")?,
        kind: r.get("kind")?,
        size: r.get("size")?,
        mtime: r.get("mtime")?,
        added_at: r.get::<_, Option<i64>>("added_at")?.unwrap_or(0),
        width: r.get("width")?,
        height: r.get("height")?,
        thumb_key: r.get("thumb_key")?,
        thumb_state: r.get("thumb_state")?,
        quality: r.get("quality")?,
        rating: r.get::<_, Option<i32>>("rating")?.unwrap_or(0),
        note: r.get::<_, Option<String>>("note")?.unwrap_or_default(),
        source_url: r.get::<_, Option<String>>("source_url")?.unwrap_or_default(),
        palette: r.get::<_, Option<String>>("palette")?.unwrap_or_default(),
        color: r.get::<_, Option<String>>("color")?.unwrap_or_default(),
    })
}

pub fn list_assets(c: &Connection, q: &AssetQuery) -> Res<Vec<Asset>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let sort = q.sort.as_deref().unwrap_or("mtime_desc");
    let kinds: Option<Vec<i32>> = q.kinds.clone().filter(|v| !v.is_empty());
    let text = q.text.clone().unwrap_or_default();
    let text = text.trim();

    // 目录过滤：recursive 时按 dirs.path 前缀
    let mut where_sql = String::from(" a.missing = 0 ");
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(did) = q.dir_id {
        if q.recursive {
            if let Some((base, _)) = dir_by_id(c, did)? {
                where_sql.push_str(" AND (d.path = ? OR d.path LIKE ? ) ");
                args.push(Box::new(base.clone()));
                args.push(Box::new(format!("{}%", base.trim_end_matches(['/', '\\']))));
            }
        } else {
            where_sql.push_str(" AND a.dir_id = ? ");
            args.push(Box::new(did));
        }
    }

    if let Some(ks) = &kinds {
        let ph = ks.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        where_sql.push_str(&format!(" AND a.kind IN ({}) ", ph));
        for k in ks {
            args.push(Box::new(*k));
        }
    }

    /* 评分下限：0 星不筛（那是"没打过分"，不是最低档），1–5 星走索引 idx_assets_rating */
    if let Some(n) = q.min_rating {
        if n > 0 {
            where_sql.push_str(" AND a.rating >= ? ");
            args.push(Box::new(n));
        }
    }

    /* 分组 / 指定 id 集合（0.2.x）：由 `App::list` 把全局 id 拆成各库的库内 id 再传下来。
       **空集合必须是空结果**（`AND 0`）—— 退化成"不筛"就会把整个库列出来，
       用户点一个空分组却看到全部素材，是最容易误判成"分组没生效"的那种 bug。 */
    if let Some(ids) = q.ids.as_ref() {
        if ids.is_empty() {
            where_sql.push_str(" AND 0 ");
        } else {
            let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            where_sql.push_str(&format!(" AND a.id IN ({ph}) "));
            for i in ids {
                args.push(Box::new(*i));
            }
        }
    }

    /* 搜索（0.5.0 起**同时命中标签名**）：
       - 普通词：文件名/备注（FTS trigram）**并上**标签名；
       - `#词`：只搜标签 —— 用户打 `#` 就是明确要找标签，别让文件名噪音混进来；
       - 1–2 字符退回 LIKE（trigram 索引对这么短的串用不上）。
       T28 之后标签**名字**在注册库、关联在库内，跨文件 JOIN 做不到：
       所以名字 → id 的那一步在 `App::list` 里查完，用 `search_tag_ids` 传下来。 */
    let tag_only = text.starts_with('#');
    let needle = if tag_only { text.trim_start_matches('#').trim() } else { text };
    if !needle.is_empty() {
        let tag_ids: Vec<i64> = q.search_tag_ids.clone().unwrap_or_default();
        let tag_placeholder = tag_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let tag_hit = if tag_ids.is_empty() {
            None
        } else {
            Some(format!(
                "a.id IN (SELECT asset_id FROM asset_tags WHERE tag_id IN ({tag_placeholder}))"
            ))
        };
        // 注意：占位符顺序必须和 args 的 push 顺序**逐字一致**，否则参数会错位
        if tag_only {
            match tag_hit {
                Some(h) => {
                    where_sql.push_str(&format!(" AND {h} "));
                    for i in &tag_ids {
                        args.push(Box::new(*i));
                    }
                }
                // 没有标签名字命中 → 空结果（不能退化成"不筛"，那会把全库都列出来）
                None => where_sql.push_str(" AND 0 "),
            }
        } else {
            match needle.chars().count() >= 3 {
                true => {
                    let h = tag_hit.map(|h| format!(" OR {h}")).unwrap_or_default();
                    where_sql.push_str(&format!(
                        " AND (a.id IN (SELECT rowid FROM assets_fts WHERE assets_fts MATCH ?){h}) "
                    ));
                    args.push(Box::new(format!("\"{}\"", needle.replace('"', "\"\""))));
                }
                false => {
                    let h = tag_hit.map(|h| format!(" OR {h}")).unwrap_or_default();
                    where_sql.push_str(&format!(" AND (a.name LIKE ? ESCAPE '\\'{h}) "));
                    args.push(Box::new(like_contains(needle)));
                }
            }
            for i in &tag_ids {
                args.push(Box::new(*i));
            }
        }
    }

    // 标签筛选：自动标签翻译成列条件，手工标签走 asset_tags —— 都在 SQL 里筛
    if let Some(tag) = q.tag.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        push_tag_filter(&mut where_sql, &mut args, tag, q.tag_ids.as_deref(), q.tag_rule.as_deref());
    }

    // 颜色筛选（T24）：色桶是枚举值，一次索引查找
    if let Some(col) = q.color.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        where_sql.push_str(" AND a.color = ? ");
        args.push(Box::new(col.to_string()));
    }

    let order = match sort {
        "name_asc" => " ORDER BY a.name COLLATE NOCASE ASC, a.id ASC ",
        "size_desc" => " ORDER BY a.size DESC, a.id DESC ",
        // 评分排序：同分按 id 倒序（= 后入先出）。不用 mtime 做第二维，
        // 因为游标只有两格 `(值, id)`，三维比不了 —— 而 id 本身随扫描递增，够稳定。
        "rating_desc" => " ORDER BY a.rating DESC, a.id DESC ",
        _ => " ORDER BY a.mtime DESC, a.id DESC ",
    };

    /* keyset 分页：三种排序全部走双列游标，**不用 OFFSET**。
       mtime / size 的游标就是 (值, id)；name 没法把字符串塞进 `(i64,i64)`，
       所以用上一页最后一行的 id 反查 name（主键点查，微秒级），再拼 (name, id) 游标。
       这样前端协议不用改（还是 `[offset, lastId]`），但深翻是索引 seek，不是「跳过 N 行」。
       万一那一行在这两步之间被删了 → 拿不到 name 就退回 OFFSET，宁可慢一次，
       也不能凭空从第 1 页重来（那会让用户看到重复条目、或者漏掉一整段）。 */
    let mut page_sql = String::new();
    let mut offset: Option<i64> = None;
    /* 跨库合并时上层已经把「上一页最后一行的名字」查好了（见 `App::list`），
       而且会故意把 id 调到 i64::MAX / -1 来表示"含/不含同名的行"。
       有它就不用再走"按 id 反查名字"那条路。 */
    let mut cursor_from_name = false;
    if sort == "name_asc" {
        if let Some((name, id)) = q.name_cursor.clone() {
            page_sql.push_str(
                " AND (a.name COLLATE NOCASE > ? COLLATE NOCASE \
                  OR (a.name COLLATE NOCASE = ? COLLATE NOCASE AND a.id > ?)) ",
            );
            args.push(Box::new(name.clone()));
            args.push(Box::new(name));
            args.push(Box::new(id));
            cursor_from_name = true;
        }
    }
    if !cursor_from_name {
        if let Some((k, id)) = q.cursor {
        match sort {
            "size_desc" => {
                page_sql.push_str(" AND (a.size, a.id) < (?, ?) ");
                args.push(Box::new(k));
                args.push(Box::new(id));
            }
            "rating_desc" => {
                page_sql.push_str(" AND (a.rating, a.id) < (?, ?) ");
                args.push(Box::new(k));
                args.push(Box::new(id));
            }
            "name_asc" => {
                /* name 没法塞进 `(i64,i64)` 的游标，所以有两种来源：
                   · 单库分页：前端只送 (已取条数, 最后一行 id)，这里用 id 反查名字；
                   · 跨库合并：上层已经把名字查好了（`name_cursor`），直接用它 ——
                     而且它会故意把 id 调到 i64::MAX / -1，用来"含/不含同名的行"。 */
                let explicit = q.name_cursor.clone();
                let last_name: Option<String> = match &explicit {
                    Some((n, _)) => Some(n.clone()),
                    None => c
                        .query_row("SELECT name FROM assets WHERE id = ?1", [id], |r| r.get(0))
                        .optional()
                        .map_err(e2s)?,
                };
                match last_name {
                    // 两边都显式写 COLLATE NOCASE：SQLite 的规则是「有显式排序规则的赢」，
                    // 和 ORDER BY a.name COLLATE NOCASE 完全对齐，翻页才不会错位。
                    Some(name) => {
                        page_sql.push_str(
                            " AND (a.name COLLATE NOCASE > ? COLLATE NOCASE \
                              OR (a.name COLLATE NOCASE = ? COLLATE NOCASE AND a.id > ?)) ",
                        );
                        args.push(Box::new(name.clone()));
                        args.push(Box::new(name));
                        args.push(Box::new(id));
                    }
                    None => offset = Some(k.max(0)),
                }
            }
            _ => {
                page_sql.push_str(" AND (a.mtime, a.id) < (?, ?) ");
                args.push(Box::new(k));
                args.push(Box::new(id));
            }
        }
    }
    }

    let offset_sql = offset.map(|o| format!(" OFFSET {} ", o)).unwrap_or_default();

    let sql = format!(
        "SELECT a.id, a.dir_id, a.name, a.ext, a.kind, a.size, a.mtime, a.width, a.height,
                a.thumb_key, a.thumb_state, a.quality, a.rating, a.note, a.source_url,
                a.palette, a.color, COALESCE(a.added_at,0) AS added_at, d.path AS dpath
         FROM assets a JOIN dirs d ON d.id = a.dir_id
         WHERE {where_sql} {page_sql} {order} LIMIT {limit}{offset_sql}"
    );

    let mut st = c.prepare(&sql).map_err(e2s)?;
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = st.query_map(refs.as_slice(), row_to_asset).map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/* ------------------------------------------------------------------ 标签 */
/* T28 之后：标签**本体**在注册库（`reg.rs`），这里只处理"素材 ↔ 标签"的关联。
   所以 db 层的函数一律收 / 返 `tag_id`，名字 → id 的换算在 `App`（它有注册库连接）。 */

/// LIKE 的「包含」模式。**必须转义**：用户搜 `50%` 或 `a_b` 时，
/// 不转义的话 `%`/`_` 会被当成通配符，搜出一堆不相干的东西（而且用户完全看不懂为什么）。
fn like_contains(s: &str) -> String {
    format!(
        "%{}%",
        s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
    )
}

/// 目录前缀模式（T34 规则用）：`E:\素材` → `E:\素材%`，末尾的分隔符先削掉再去重。
fn like_prefix(dir: &str) -> String {
    let d = dir.trim_end_matches(['/', '\\']);
    format!(
        "{}%",
        d.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
    )
}

/// 标签筛选：自动标签 → 列条件；规则标签 → 规则翻译成列条件；手工标签 → asset_tags。
///
/// 为什么不在前端过滤：百万级库前端只有当前页，按标签筛必须落到 SQL 才能正确分页。
fn push_tag_filter(
    where_sql: &mut String,
    args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    tag: &str,
    tag_ids: Option<&[i64]>,
    tag_rule: Option<&str>,
) {
    use crate::autotag::{self, AutoFilter};
    if let Some(f) = autotag::parse_filter(tag) {
        match f {
            AutoFilter::Ext(e) => {
                where_sql.push_str(" AND lower(a.ext) = ? ");
                args.push(Box::new(e));
            }
            AutoFilter::Kind(k) => {
                where_sql.push_str(" AND a.kind = ? ");
                args.push(Box::new(k));
            }
            AutoFilter::Place(lo, hi) => {
                where_sql.push_str(" AND MAX(COALESCE(a.width,0), COALESCE(a.height,0)) >= ? ");
                args.push(Box::new(lo));
                if let Some(hi) = hi {
                    where_sql.push_str(" AND MAX(COALESCE(a.width,0), COALESCE(a.height,0)) < ? ");
                    args.push(Box::new(hi));
                }
            }
            AutoFilter::Orient(o) => {
                where_sql.push_str(match o {
                    "landscape" => " AND a.width > 0 AND a.height > 0 AND a.width > a.height ",
                    "portrait" => " AND a.width > 0 AND a.height > 0 AND a.width < a.height ",
                    _ => " AND a.width > 0 AND a.height > 0 AND a.width = a.height ",
                });
            }
            AutoFilter::Size(lo, hi) => {
                where_sql.push_str(" AND a.size >= ? ");
                args.push(Box::new(lo));
                if let Some(hi) = hi {
                    where_sql.push_str(" AND a.size < ? ");
                    args.push(Box::new(hi));
                }
            }
        }
        return;
    }
    if let Some(rule) = tag_rule.and_then(crate::reg::parse_rule) {
        push_rule_conditions(where_sql, args, &rule);
        return;
    }
    match tag_ids {
        Some(ids) if !ids.is_empty() => {
            let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            where_sql.push_str(&format!(
                " AND a.id IN (SELECT asset_id FROM asset_tags WHERE tag_id IN ({ph})) "
            ));
            for id in ids {
                args.push(Box::new(*id));
            }
        }
        // 标签名在注册库里查不到（刚被别人删掉 / 拼错了）：给空结果，别悄悄"不筛"
        _ => where_sql.push_str(" AND 0 "),
    }
}

/// 把一条自动标签规则翻译成列条件（T34）。
///
/// 规则**不物化**成关联行：素材一被移动/改大小，物化的那批就行错；规则是"活"的，
/// 每次筛选现场翻译成 SQL 反而是唯一不会过期的做法（代价是一次索引扫描）。
pub fn push_rule_conditions(
    where_sql: &mut String,
    args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    r: &crate::reg::AutoRule,
) {
    let dir = r.dir.trim();
    if !dir.is_empty() {
        where_sql.push_str(" AND d.path LIKE ? ESCAPE '\\' ");
        args.push(Box::new(like_prefix(dir)));
    }
    if !r.ext.is_empty() {
        let ph = r.ext.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        where_sql.push_str(&format!(" AND lower(a.ext) IN ({ph}) "));
        for e in &r.ext {
            args.push(Box::new(e.trim_start_matches('.').to_lowercase()));
        }
    }
    if !r.kind.is_empty() {
        let ph = r.kind.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        where_sql.push_str(&format!(" AND a.kind IN ({ph}) "));
        for k in &r.kind {
            args.push(Box::new(*k));
        }
    }
    if r.size_min > 0 {
        where_sql.push_str(" AND a.size >= ? ");
        args.push(Box::new(r.size_min));
    }
    if r.size_max > 0 {
        where_sql.push_str(" AND a.size <= ? ");
        args.push(Box::new(r.size_max));
    }
    if r.px_min > 0 {
        where_sql.push_str(" AND MAX(COALESCE(a.width,0), COALESCE(a.height,0)) >= ? ");
        args.push(Box::new(r.px_min));
    }
    if r.px_max > 0 {
        where_sql.push_str(" AND MAX(COALESCE(a.width,0), COALESCE(a.height,0)) <= ? ");
        args.push(Box::new(r.px_max));
    }
    if r.need_dims {
        where_sql.push_str(" AND COALESCE(a.width,0) > 0 AND COALESCE(a.height,0) > 0 ");
    }
}

/// 一条规则命中多少素材（侧栏标签计数用）。没有条件 = 0（空规则不该匹配全库）。
pub fn count_rule(c: &Connection, r: &crate::reg::AutoRule) -> Res<i64> {
    if r.is_empty() {
        return Ok(0);
    }
    let mut where_sql = String::from(" a.missing = 0 ");
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    push_rule_conditions(&mut where_sql, &mut args, r);
    let sql = format!(
        "SELECT COUNT(*) FROM assets a JOIN dirs d ON d.id = a.dir_id WHERE {where_sql}"
    );
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    c.query_row(&sql, refs.as_slice(), |r| r.get(0)).map_err(e2s)
}

/// 自动标签 + 使用数：全部在 SQL 里按列聚合（百万行也只是几次扫索引），
/// 阈值直接复用 `autotag` 里的档位表，和详情面板的推导口径一致。
pub fn auto_tag_counts(c: &Connection) -> Res<Vec<TagCount>> {
    use crate::autotag::{PLACE_STEPS, SIZE_STEPS};
    let mut out: Vec<TagCount> = Vec::new();

    // 格式
    {
        let mut st = c
            .prepare("SELECT lower(ext), COUNT(*) FROM assets WHERE missing=0 GROUP BY lower(ext)")
            .map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(e2s)?;
        for (ext, n) in rows.flatten() {
            if !ext.is_empty() {
                out.push(TagCount { name: format!("fmt:{ext}"), count: n, ..Default::default() });
            }
        }
    }

    // 类型
    {
        let mut st = c
            .prepare("SELECT kind, COUNT(*) FROM assets WHERE missing=0 GROUP BY kind")
            .map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, i64>(1)?)))
            .map_err(e2s)?;
        for (k, n) in rows.flatten() {
            out.push(TagCount {
                name: crate::autotag::kind_tag(k).to_string(),
                count: n,
                ..Default::default()
            });
        }
    }

    // 像素档：按长边落区间（和 PLACE_STEPS 同一张表）
    let longest = "MAX(COALESCE(width,0), COALESCE(height,0))";
    for (i, (px, tag)) in PLACE_STEPS.iter().enumerate() {
        let upper = if i == 0 {
            String::new()
        } else {
            format!(" AND {longest} < {}", PLACE_STEPS[i - 1].0)
        };
        let sql = format!(
            "SELECT COUNT(*) FROM assets
             WHERE missing=0 AND kind IN (0,1,3) AND {longest} >= {px}{upper}"
        );
        let n: i64 = c.query_row(&sql, [], |r| r.get(0)).map_err(e2s)?;
        if n > 0 {
            out.push(TagCount { name: (*tag).to_string(), count: n, ..Default::default() });
        }
    }

    // 比例
    for (tag, cond) in [
        ("orient:landscape", "width > height"),
        ("orient:portrait", "width < height"),
        ("orient:square", "width = height"),
    ] {
        let sql = format!(
            "SELECT COUNT(*) FROM assets WHERE missing=0 AND kind IN (0,1,3) \
             AND width > 0 AND height > 0 AND {cond}"
        );
        let n: i64 = c.query_row(&sql, [], |r| r.get(0)).map_err(e2s)?;
        if n > 0 {
            out.push(TagCount { name: tag.to_string(), count: n, ..Default::default() });
        }
    }

    // 体积档
    for (i, (bytes, tag)) in SIZE_STEPS.iter().enumerate() {
        let upper = if i == 0 {
            String::new()
        } else {
            format!(" AND size < {}", SIZE_STEPS[i - 1].0)
        };
        let sql = format!("SELECT COUNT(*) FROM assets WHERE missing=0 AND size >= {bytes}{upper}");
        let n: i64 = c.query_row(&sql, [], |r| r.get(0)).map_err(e2s)?;
        if n > 0 {
            out.push(TagCount { name: (*tag).to_string(), count: n, ..Default::default() });
        }
    }
    {
        let smallest = SIZE_STEPS.last().map(|(b, _)| *b).unwrap_or(0);
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM assets WHERE missing=0 AND size < ?1",
                params![smallest],
                |r| r.get(0),
            )
            .map_err(e2s)?;
        if n > 0 {
            out.push(TagCount { name: "size:xs".into(), count: n, ..Default::default() });
        }
    }

    // 稳定顺序：先维度（fmt → kind → place → orient → size），同维度内计数降序
    out.sort_by(|a, b| {
        let rank = |s: &str| {
            ["fmt:", "kind:", "place:", "orient:", "size:"]
                .iter()
                .position(|p| s.starts_with(p))
                .unwrap_or(9)
        };
        rank(&a.name)
            .cmp(&rank(&b.name))
            .then(b.count.cmp(&a.count))
            .then(a.name.cmp(&b.name))
    });
    Ok(out)
}

/// 每个标签在本库的命中数（跨库由 `App` 汇总同一 tag_id 的数）。
pub fn counts_by_tag(c: &Connection) -> Res<Vec<(i64, i64)>> {
    let mut st = c
        .prepare(
            "SELECT att.tag_id, COUNT(a.id)
             FROM asset_tags att JOIN assets a ON a.id = att.asset_id AND a.missing = 0
             GROUP BY att.tag_id",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 一批素材各自的标签 id（当前页一次查完，别每行一次 IPC）
pub fn tag_ids_for_assets(c: &Connection, ids: &[i64]) -> Res<Vec<(i64, Vec<i64>)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT asset_id, tag_id FROM asset_tags WHERE asset_id IN ({ph}) ORDER BY tag_id"
    );
    let mut st = c.prepare(&sql).map_err(e2s)?;
    let args: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
    let rows = st
        .query_map(args.as_slice(), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(e2s)?;

    let mut map: std::collections::BTreeMap<i64, Vec<i64>> = Default::default();
    for (id, tid) in rows.flatten() {
        map.entry(id).or_default().push(tid);
    }
    Ok(map.into_iter().collect())
}

/// 批量打标签。返回**真正新增**的关联数（已经打过的重复调用无害）。
pub fn insert_asset_tags(c: &Connection, ids: &[i64], tag_id: i64) -> Res<i64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut n = 0i64;
    let mut st = c
        .prepare("INSERT OR IGNORE INTO asset_tags (asset_id, tag_id) VALUES (?1, ?2)")
        .map_err(e2s)?;
    for id in ids {
        n += st.execute(params![id, tag_id]).map_err(e2s)? as i64;
    }
    Ok(n)
}

/// 批量去掉标签（标签本身保留，可能别的素材还在用）
pub fn delete_asset_tags(c: &Connection, ids: &[i64], tag_id: i64) -> Res<i64> {
    let mut n = 0i64;
    let mut st = c
        .prepare("DELETE FROM asset_tags WHERE asset_id = ?1 AND tag_id = ?2")
        .map_err(e2s)?;
    for id in ids {
        n += st.execute(params![id, tag_id]).map_err(e2s)? as i64;
    }
    Ok(n)
}

/// 详情面板保存：整组替换（用户增删完点保存时一次写清）
pub fn set_asset_tags(c: &Connection, asset_id: i64, tag_ids: &[i64]) -> Res<()> {
    c.execute("DELETE FROM asset_tags WHERE asset_id = ?1", params![asset_id])
        .map_err(e2s)?;
    for tid in tag_ids {
        c.execute(
            "INSERT OR IGNORE INTO asset_tags (asset_id, tag_id) VALUES (?1, ?2)",
            params![asset_id, tid],
        )
        .map_err(e2s)?;
    }
    Ok(())
}

/// 去标签时连带清空所有库里的关联（`App::delete_tag` 逐库调）。
pub fn delete_tag_refs(c: &Connection, tag_id: i64) -> Res<i64> {
    let n = c
        .execute("DELETE FROM asset_tags WHERE tag_id = ?1", params![tag_id])
        .map_err(e2s)?;
    Ok(n as i64)
}

/// 标签改名撞已有名字时**合并**：把旧 id 的关联整体搬到新 id（逐库执行）。
pub fn move_tag_refs(c: &Connection, old_id: i64, new_id: i64) -> Res<i64> {
    let n = c
        .execute(
            "INSERT OR IGNORE INTO asset_tags (asset_id, tag_id)
             SELECT asset_id, ?1 FROM asset_tags WHERE tag_id = ?2",
            params![new_id, old_id],
        )
        .map_err(e2s)? as i64;
    c.execute("DELETE FROM asset_tags WHERE tag_id = ?1", params![old_id])
        .map_err(e2s)?;
    Ok(n)
}

/// 本库里还在用的标签 id（合并/清理时判断"这个标签彻底没人用了"）
pub fn used_tag_ids(c: &Connection) -> Res<Vec<i64>> {
    let mut st = c
        .prepare("SELECT DISTINCT tag_id FROM asset_tags")
        .map_err(e2s)?;
    let rows = st.query_map([], |r| r.get::<_, i64>(0)).map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/* --------------------------------------------------- 颜色 / 主色板（T24） */

/// 写回主色板 + 色桶。**只在这个素材还没有值时写**（缩略图会被反复请求，
/// 每次都跑一遍聚类是浪费；而且同一张图算出来的结果本来也一样）。
pub fn set_palette(c: &Connection, id: i64, palette: &str, color: &str) -> Res<()> {
    c.execute(
        "UPDATE assets SET palette=?1, color=?2 WHERE id=?3 AND (palette IS NULL OR palette='')",
        params![palette, color, id],
    )
    .map_err(e2s)?;
    Ok(())
}

/// 各色桶的素材数（侧栏颜色筛选的计数）
pub fn color_counts(c: &Connection) -> Res<Vec<(String, i64)>> {
    let mut st = c
        .prepare(
            "SELECT color, COUNT(*) FROM assets
             WHERE missing=0 AND color IS NOT NULL AND color <> '' GROUP BY color",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 还没算过主色的图片（预热 / 批量补算用）
pub fn assets_without_palette(c: &Connection, limit: i64) -> Res<Vec<(i64, String, i64)>> {
    let mut st = c
        .prepare(
            "SELECT a.id, a.name, a.width FROM assets a
             WHERE a.missing=0 AND a.kind=0 AND (a.palette IS NULL OR a.palette='')
             ORDER BY a.id LIMIT ?1",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2).unwrap_or(0)))
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/* ------------------------------------------------- 哈希（T25 重复检测） */

/// 只写相似指纹（出缩略图时顺带算的那个）。**不碰 hash_x** ——
/// 那个要读全文件字节（几十 MB），是独立的"查重"流程干的活。
pub fn set_phash(c: &Connection, id: i64, phash: i64) -> Res<()> {
    c.execute("UPDATE assets SET phash=?1 WHERE id=?2", params![phash, id])
        .map_err(e2s)?;
    Ok(())
}

pub fn set_hashes(c: &Connection, id: i64, hash: &[u8], phash: i64) -> Res<()> {
    c.execute(
        "UPDATE assets SET hash_x=?1, phash=?2 WHERE id=?3",
        params![hash, phash, id],
    )
    .map_err(e2s)?;
    Ok(())
}

/// 还没算哈希的素材（按体积从大到小：大文件重复更值钱，用户也更想先看到）。
/// 只算"值得算"的：0 字节文件跳过（空文件互相"重复"毫无意义）。
pub fn assets_without_hash(c: &Connection, limit: i64) -> Res<Vec<(i64, String, i64, i32)>> {
    let mut st = c
        .prepare(
            "SELECT a.id, a.name, a.size, a.kind FROM assets a
             WHERE a.missing=0 AND a.hash_x IS NULL AND a.size > 0
             ORDER BY a.size DESC, a.id ASC LIMIT ?1",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i32>(3)?))
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 精确重复组：同一 `size + hash_x` 出现多次。`size` 一起比是免费的保险
/// （哈希碰撞在 64 位上已经极小，但 size 不同就一定不是同一份文件）。
pub fn exact_duplicate_groups(c: &Connection, limit: i64) -> Res<Vec<(Vec<u8>, i64, Vec<i64>)>> {
    let mut st = c
        .prepare(
            "SELECT a.hash_x, a.size, a.id FROM assets a
             JOIN (SELECT size, hash_x FROM assets
                   WHERE missing=0 AND hash_x IS NOT NULL
                   GROUP BY size, hash_x HAVING COUNT(*) > 1
                   ORDER BY COUNT(*) DESC, size DESC LIMIT ?1) g
               ON g.size = a.size AND g.hash_x = a.hash_x
             WHERE a.missing=0
             ORDER BY a.size DESC, a.id ASC",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![limit], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(e2s)?;
    let mut out: Vec<(Vec<u8>, i64, Vec<i64>)> = Vec::new();
    for (h, size, id) in rows.flatten() {
        match out.iter_mut().find(|(ph, ps, _)| *ph == h && *ps == size) {
            Some((_, _, v)) => v.push(id),
            None => out.push((h, size, vec![id])),
        }
    }
    Ok(out)
}

/// 所有算过 pHash 的图片（相似重复分组在 `dupes` 里做，需要距离比较）。
pub fn phash_rows(c: &Connection) -> Res<Vec<(i64, i64, i64)>> {
    let mut st = c
        .prepare(
            "SELECT id, phash, size FROM assets
             WHERE missing=0 AND kind=0 AND phash IS NOT NULL AND phash <> 0",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 素材的完整路径（按 id 批量取，重复检测的收尾/定位要用）
pub fn paths_for(c: &Connection, ids: &[i64]) -> Res<Vec<(i64, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT a.id, d.path, a.name FROM assets a JOIN dirs d ON d.id = a.dir_id
         WHERE a.id IN ({ph})"
    );
    let mut st = c.prepare(&sql).map_err(e2s)?;
    let args: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
    let rows = st
        .query_map(args.as_slice(), |r| {
            let dir: String = r.get(1)?;
            let name: String = r.get(2)?;
            Ok((r.get::<_, i64>(0)?, path_join(&dir, &name)))
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/* ------------------------------------------------- 引用关系（T26） */

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefRow {
    /// 工程文件（src）的**库内** id；`App` 负责编成全局 id
    pub src_id: i64,
    pub src_name: String,
    /// 工程文件的绝对路径（界面上"在资源管理器里显示"要用）
    pub src_path: String,
    /// 工程里写的那条路径（原样）
    pub dst_path: String,
    /// 解析到的素材名（空 = 没解析到）
    pub dst_name: String,
    pub dst_id: Option<i64>,
    pub missing: bool,
}

/// 重写一个工程文件的引用清单（扫一次写一次，整体替换）
pub fn set_refs(c: &Connection, src_id: i64, rows: &[(String, Option<i64>, i32)]) -> Res<()> {
    c.execute("DELETE FROM refs WHERE src_id=?1", params![src_id])
        .map_err(e2s)?;
    let mut st = c
        .prepare(
            "INSERT OR IGNORE INTO refs (src_id, dst_path, dst_id, kind, missing)
             VALUES (?1,?2,?3,?4,?5)",
        )
        .map_err(e2s)?;
    for (path, dst, kind) in rows {
        st.execute(params![src_id, path, dst, kind, if dst.is_none() { 1 } else { 0 }])
            .map_err(e2s)?;
    }
    c.execute("UPDATE assets SET refs_at=?1 WHERE id=?2", params![now_ts(), src_id])
        .map_err(e2s)?;
    Ok(())
}

/// 谁引用了这个素材（详情面板「被引用」）
pub fn refs_pointing_to(c: &Connection, asset_id: i64, limit: i64) -> Res<Vec<RefRow>> {
    let mut st = c
        .prepare(
            "SELECT r.src_id, a.name, d.path || (CASE WHEN substr(d.path,-1) IN (char(92),'/') THEN '' ELSE char(92) END) || a.name,
                    r.dst_path, COALESCE(dst.name, ''), r.dst_id, r.missing
             FROM refs r JOIN assets a ON a.id = r.src_id
             JOIN dirs d ON d.id = a.dir_id
             LEFT JOIN assets dst ON dst.id = r.dst_id
             WHERE r.dst_id = ?1 AND a.missing = 0
             ORDER BY a.name COLLATE NOCASE LIMIT ?2",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![asset_id, limit], ref_row)
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 这个工程引用了谁（`dst_id` 可能为空 = 丢了）
pub fn refs_of_project(c: &Connection, src_id: i64, limit: i64) -> Res<Vec<RefRow>> {
    let mut st = c
        .prepare(
            "SELECT r.src_id, a.name, d.path || (CASE WHEN substr(d.path,-1) IN (char(92),'/') THEN '' ELSE char(92) END) || a.name,
                    r.dst_path, COALESCE(dst.name, ''), r.dst_id, r.missing
             FROM refs r JOIN assets a ON a.id = r.src_id
             JOIN dirs d ON d.id = a.dir_id
             LEFT JOIN assets dst ON dst.id = r.dst_id
             WHERE r.src_id = ?1 ORDER BY r.missing DESC, r.dst_path LIMIT ?2",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![src_id, limit], ref_row)
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

fn ref_row(r: &rusqlite::Row) -> rusqlite::Result<RefRow> {
    Ok(RefRow {
        src_id: r.get(0)?,
        src_name: r.get(1)?,
        src_path: r.get(2)?,
        dst_path: r.get(3)?,
        dst_name: r.get(4)?,
        dst_id: r.get(5)?,
        missing: r.get::<_, i64>(6)? != 0,
    })
}

/// 全库「引用丢了」的清单（同一路径可能被多个工程引用，所以按 (工程, 路径) 列）
pub fn broken_refs(c: &Connection, limit: i64) -> Res<Vec<RefRow>> {
    let mut st = c
        .prepare(
            "SELECT r.src_id, a.name, d.path || (CASE WHEN substr(d.path,-1) IN (char(92),'/') THEN '' ELSE char(92) END) || a.name,
                    r.dst_path, COALESCE(dst.name, ''), r.dst_id, r.missing
             FROM refs r JOIN assets a ON a.id = r.src_id
             JOIN dirs d ON d.id = a.dir_id
             LEFT JOIN assets dst ON dst.id = r.dst_id
             WHERE r.missing = 1 AND a.missing = 0
             ORDER BY a.name COLLATE NOCASE, r.dst_path LIMIT ?1",
        )
        .map_err(e2s)?;
    let rows = st.query_map(params![limit], ref_row).map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 还没扫过引用的"工程类"文件：文档 / 模型 / 其他，体积有限（大文件读起来是浪费）。
pub fn assets_needing_refs(c: &Connection, limit: i64) -> Res<Vec<(i64, String, i64, i32)>> {
    let mut st = c
        .prepare(
            "SELECT a.id, d.path || (CASE WHEN substr(d.path,-1) IN (char(92),'/') THEN '' ELSE char(92) END) || a.name,
                    a.size, a.kind
             FROM assets a JOIN dirs d ON d.id = a.dir_id
             WHERE a.missing=0 AND a.refs_at=0 AND a.kind IN (0,3,4,5) AND a.size <= ?2
             ORDER BY a.size DESC LIMIT ?1",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![limit, 16 * 1024 * 1024], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i32>(3)?))
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 标记"扫过了但一条引用都没有"（否则每次都要重读一遍这个文件）
pub fn mark_refs_scanned(c: &Connection, id: i64) -> Res<()> {
    c.execute("UPDATE assets SET refs_at=?1 WHERE id=?2", params![now_ts(), id])
        .map_err(e2s)?;
    Ok(())
}

/// 全库引用统计（设置/报告用）：(总引用数, 丢失数)
pub fn refs_summary(c: &Connection) -> Res<(i64, i64)> {
    let total: i64 = c
        .query_row("SELECT COUNT(*) FROM refs", [], |r| r.get(0))
        .map_err(e2s)?;
    let missing: i64 = c
        .query_row("SELECT COUNT(*) FROM refs WHERE missing=1", [], |r| r.get(0))
        .map_err(e2s)?;
    Ok((total, missing))
}

pub fn count_assets(c: &Connection) -> Res<i64> {
    c.query_row("SELECT COUNT(*) FROM assets WHERE missing=0", [], |r| r.get(0))
        .map_err(e2s)
}

pub fn set_thumb(c: &Connection, id: i64, key: &str, state: i32, quality: i32, bytes: i64) -> Res<()> {
    c.execute(
        "UPDATE assets SET thumb_key=?1, thumb_state=?2, quality=?3 WHERE id=?4",
        params![key, state, quality, id],
    )
    .map_err(e2s)?;
    c.execute(
        "INSERT INTO thumb_lru (key, bytes, last_used) VALUES (?1,?2,?3)
         ON CONFLICT(key) DO UPDATE SET last_used=excluded.last_used",
        params![key, bytes, now_ts()],
    )
    .map_err(e2s)?;
    Ok(())
}

/* ---------------- 人写的东西：评分 / 注释 / 来源 URL（T23）----------------
   这三样都**不是从文件推导出来的**，删了就永远找不回来，所以扫描、重建索引
   一律不许碰它们（`sync_dir` 只更新文件事实，不写这三列）。 */

/// 批量设评分（0–5，0 = 清掉）。返回真正改动的行数。
pub fn set_rating(c: &Connection, ids: &[i64], rating: i32) -> Res<usize> {
    let r = rating.clamp(0, 5);
    let mut n = 0;
    let mut st = c
        .prepare("UPDATE assets SET rating=?1 WHERE id=?2 AND rating<>?1")
        .map_err(e2s)?;
    for id in ids {
        n += st.execute(params![r, id]).map_err(e2s)?;
    }
    Ok(n)
}

pub fn set_note(c: &Connection, id: i64, note: &str) -> Res<()> {
    c.execute("UPDATE assets SET note=?1 WHERE id=?2", params![note, id])
        .map_err(e2s)?;
    Ok(())
}

pub fn set_source_url(c: &Connection, id: i64, url: &str) -> Res<()> {
    c.execute("UPDATE assets SET source_url=?1 WHERE id=?2", params![url, id])
        .map_err(e2s)?;
    Ok(())
}

/// 出图后回填原图尺寸：RAW / PSD 在扫描阶段可能读不出宽高，
/// 瀑布流按 4:3 兜底会跳行。**只在没有真值时写**，绝不覆盖扫描出来的数据。
pub fn set_dims_if_missing(c: &Connection, id: i64, w: u32, h: u32) -> Res<()> {
    if w == 0 || h == 0 {
        return Ok(());
    }
    c.execute(
        "UPDATE assets SET width=?1, height=?2 WHERE id=?3 AND (width IS NULL OR width=0)",
        params![w, h, id],
    )
    .map_err(e2s)?;
    Ok(())
}

pub fn mark_thumb_state(c: &Connection, id: i64, state: i32) -> Res<()> {
    c.execute("UPDATE assets SET thumb_state=?1 WHERE id=?2", params![state, id])
        .map_err(e2s)?;
    Ok(())
}

pub fn asset_by_id(c: &Connection, id: i64) -> Res<Option<Asset>> {
    let mut st = c
        .prepare(
            "SELECT a.id, a.dir_id, a.name, a.ext, a.kind, a.size, a.mtime, a.width, a.height,
                    a.thumb_key, a.thumb_state, a.quality, a.rating, a.note, a.source_url,
                    a.palette, a.color, COALESCE(a.added_at,0) AS added_at, d.path AS dpath
             FROM assets a JOIN dirs d ON d.id=a.dir_id WHERE a.id=?1",
        )
        .map_err(e2s)?;
    let v = st
        .query_row(params![id], row_to_asset)
        .optional()
        .map_err(e2s)?;
    Ok(v)
}

/// 重建全文索引。
///
/// **不能用 `INSERT INTO assets_fts(assets_fts) VALUES('rebuild')`**：这张表是
/// contentless（`content=''`，故意不存副本），没有"内容表"可读，rebuild 会静默什么都不干。
/// 所以只能自己灌：清空 → 逐行从 assets 取名字/备注写回去。
pub fn rebuild_fts(c: &Connection) -> Res<i64> {
    c.execute("INSERT INTO assets_fts(assets_fts) VALUES('delete-all')", [])
        .map_err(e2s)?;
    let mut n = 0i64;
    {
        let mut st = c
            .prepare("SELECT id, name, COALESCE(note,'') FROM assets")
            .map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .map_err(e2s)?;
        let mut ins = c
            .prepare("INSERT INTO assets_fts (rowid, name, note, tags) VALUES (?1,?2,?3,'')")
            .map_err(e2s)?;
        for row in rows.flatten() {
            ins.execute(params![row.0, row.1, row.2]).map_err(e2s)?;
            n += 1;
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------- ops

pub fn log_op(c: &Connection, kind: i32, payload: &serde_json::Value) -> Res<i64> {
    let id = c
        .query_row(
            "INSERT INTO ops (kind, ts, payload) VALUES (?1,?2,?3) RETURNING id",
            params![kind, now_ts(), payload.to_string()],
            |r| r.get(0),
        )
        .map_err(e2s)?;
    // 定长 20
    c.execute(
        "DELETE FROM ops WHERE id NOT IN (SELECT id FROM ops ORDER BY ts DESC, id DESC LIMIT 20)",
        [],
    )
    .map_err(e2s)?;
    Ok(id)
}

pub fn last_undoable_op(c: &Connection) -> Res<Option<(i64, i32, String)>> {
    c.query_row(
        "SELECT id, kind, payload FROM ops WHERE undone=0 ORDER BY ts DESC, id DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .map_err(e2s)
}

pub fn mark_op_undone(c: &Connection, id: i64) -> Res<()> {
    c.execute("UPDATE ops SET undone=1 WHERE id=?1", params![id])
        .map_err(e2s)?;
    Ok(())
}

/// 和 `last_undoable_op` 一样，但把 `ops.ts` 也带出来 —— 拆库后要跨库比"谁更晚"，
/// 不知道时间戳就只能猜该撤哪个库的（猜错 = 把用户刚做的别的操作撤掉）。
pub fn last_undoable_op_with_ts(c: &Connection) -> Res<Option<(i64, i32, i64)>> {
    c.query_row(
        "SELECT id, kind, ts FROM ops WHERE undone=0 ORDER BY ts DESC, id DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .map_err(e2s)
}

/// 缩略图 LRU 全表（跨库淘汰时按 last_used 统一排序）
pub fn thumb_lru_rows(c: &Connection) -> Res<Vec<(String, i64, i64)>> {
    let mut st = c
        .prepare("SELECT key, bytes, last_used FROM thumb_lru")
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 库里所有「素材 → 标签 id」的关联（导出用户数据用）
pub fn all_asset_tags(c: &Connection) -> Res<Vec<(i64, Vec<i64>)>> {
    let mut st = c
        .prepare("SELECT asset_id, tag_id FROM asset_tags ORDER BY asset_id, tag_id")
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(e2s)?;
    let mut map: std::collections::BTreeMap<i64, Vec<i64>> = Default::default();
    for (aid, tid) in rows.flatten() {
        map.entry(aid).or_default().push(tid);
    }
    Ok(map.into_iter().collect())
}

/// 用户层字段导出行：(id, 目录, 名字, 扩展名, 体积, 修改时间, 评分, 备注, 来源 URL)
pub type UserRow = (i64, String, String, String, i64, i64, i64, String, String);

/* ------------------------------------------------------------------ 分组
   0.4.4 起分组跟着库走（见 SCHEMA 里的注释）。这里的 `asset_id` 一律是**本库** id，
   `App` 层负责把全局 id 拆开、按库路由。 */

pub struct CollRow {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    /// **原始成员数**（含 missing=1 的行）。界面上的数字由 `App::coll_list` 重算。
    pub count: i64,
}

pub fn coll_list(c: &Connection) -> Res<Vec<CollRow>> {
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
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 新建分组。**同名直接返回已有的 id**（和快照同一口径：点两次不该出两个一样的）。
pub fn coll_create(c: &Connection, name: &str) -> Res<i64> {
    let name = crate::reg::validate_coll_name(name)?;
    c.query_row(
        "INSERT INTO collections (name, created_at) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET name = collections.name
         RETURNING id",
        rusqlite::params![name, now_ts()],
        |r| r.get(0),
    )
    .map_err(e2s)
}

pub fn coll_rename(c: &Connection, id: i64, name: &str) -> Res<()> {
    let name = crate::reg::validate_coll_name(name)?;
    let taken: Option<i64> = c
        .query_row(
            "SELECT id FROM collections WHERE name = ?1 AND id <> ?2",
            rusqlite::params![name, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(e2s)?;
    if taken.is_some() {
        return Err(format!("已经有一个叫「{name}」的分组了"));
    }
    c.execute("UPDATE collections SET name = ?2 WHERE id = ?1", rusqlite::params![id, name])
        .map_err(e2s)?;
    Ok(())
}

/// 删分组：**只删分组和成员行，素材本身一个都不动**。
pub fn coll_delete(c: &Connection, id: i64) -> Res<()> {
    c.execute("DELETE FROM collection_assets WHERE collection_id = ?1", rusqlite::params![id])
        .map_err(e2s)?;
    c.execute("DELETE FROM collections WHERE id = ?1", rusqlite::params![id])
        .map_err(e2s)?;
    Ok(())
}

pub fn coll_add(c: &Connection, id: i64, asset_ids: &[i64]) -> Res<usize> {
    let mut st = c
        .prepare(
            "INSERT INTO collection_assets (collection_id, asset_id, added_at)
             VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
        )
        .map_err(e2s)?;
    let now = now_ts();
    let mut n = 0usize;
    for aid in asset_ids {
        n += st
            .execute(rusqlite::params![id, aid, now])
            .map_err(e2s)?;
    }
    Ok(n)
}

pub fn coll_remove(c: &Connection, id: i64, asset_ids: &[i64]) -> Res<usize> {
    let mut st = c
        .prepare("DELETE FROM collection_assets WHERE collection_id = ?1 AND asset_id = ?2")
        .map_err(e2s)?;
    let mut n = 0usize;
    for aid in asset_ids {
        n += st.execute(rusqlite::params![id, aid]).map_err(e2s)?;
    }
    Ok(n)
}

/// 这个分组里的**本库**素材 id（含 missing 的，计数再筛）。
pub fn coll_ids(c: &Connection, id: i64) -> Res<Vec<i64>> {
    let mut st = c
        .prepare("SELECT asset_id FROM collection_assets WHERE collection_id = ?1 ORDER BY added_at, asset_id")
        .map_err(e2s)?;
    let rows = st.query_map(rusqlite::params![id], |r| r.get::<_, i64>(0)).map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/// 这张素材在**本库**的哪些分组里。
pub fn coll_of(c: &Connection, asset_id: i64) -> Res<Vec<i64>> {
    let mut st = c
        .prepare("SELECT collection_id FROM collection_assets WHERE asset_id = ?1 ORDER BY collection_id")
        .map_err(e2s)?;
    let rows = st.query_map(rusqlite::params![asset_id], |r| r.get::<_, i64>(0)).map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/// 资产包导出用的一行：(id, 目录, 名字, 体积, 修改时间, 宽, 高, 评分, 备注)。
///
/// 与 `user_rows` 的区别：**全部**素材都算（不只是有用户数据的）——
/// 资产包的语义是"把整个库搬到新机器"，没有评分备注的图同样要进去。
pub type PackRow = (i64, String, String, i64, i64, Option<i64>, Option<i64>, i64, String);

pub fn pack_rows(c: &Connection) -> Res<Vec<PackRow>> {
    let mut st = c
        .prepare(
            "SELECT a.id, d.path, a.name, a.size, a.mtime, a.width, a.height, a.rating, a.note
             FROM assets a JOIN dirs d ON d.id = a.dir_id
             WHERE a.missing = 0
             ORDER BY a.id",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get::<_, Option<i64>>(7)?.unwrap_or(0),
                r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            ))
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

pub fn user_rows(c: &Connection) -> Res<Vec<UserRow>> {
    let mut st = c
        .prepare(
            "SELECT a.id, d.path, a.name, a.ext, a.size, a.mtime, a.rating, a.note, a.source_url
             FROM assets a JOIN dirs d ON d.id = a.dir_id
             WHERE a.missing = 0
               AND (a.rating > 0 OR TRIM(COALESCE(a.note,'')) <> ''
                    OR TRIM(COALESCE(a.source_url,'')) <> ''
                    OR a.id IN (SELECT asset_id FROM asset_tags))
             ORDER BY a.id",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            ))
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 只写全文件哈希（不碰 phash：那是缩略图阶段算的）
pub fn set_hash_x(c: &Connection, id: i64, hash: &[u8]) -> Res<()> {
    c.execute("UPDATE assets SET hash_x=?1 WHERE id=?2", params![hash, id])
        .map_err(e2s)?;
    Ok(())
}

/// 还没算全文件哈希的素材（**带绝对路径**，查重时要读文件）
pub fn assets_without_hash_paths(c: &Connection, limit: i64) -> Res<Vec<(i64, String, i64, i32)>> {
    let mut st = c
        .prepare(
            "SELECT a.id, d.path || (CASE WHEN substr(d.path,-1) IN (char(92),'/') THEN '' ELSE char(92) END) || a.name,
                    a.size, a.kind
             FROM assets a JOIN dirs d ON d.id = a.dir_id
             WHERE a.missing=0 AND a.hash_x IS NULL AND a.size > 0
             ORDER BY a.size DESC, a.id ASC LIMIT ?1",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i32>(3)?))
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 还没算主色板的素材（带绝对路径），补算时优先读缓存里的缩略图
pub fn assets_without_palette_paths(c: &Connection, limit: i64) -> Res<Vec<(i64, String)>> {
    let mut st = c
        .prepare(
            "SELECT a.id, d.path || (CASE WHEN substr(d.path,-1) IN (char(92),'/') THEN '' ELSE char(92) END) || a.name
             FROM assets a JOIN dirs d ON d.id = a.dir_id
             WHERE a.missing=0 AND (a.palette IS NULL OR a.palette='') AND a.kind=0
             ORDER BY a.id ASC LIMIT ?1",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![limit], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 缺主色板 / 缺哈希的**条数**（只数不取行：百万级的库不能为了显示一个数字把路径全读出来）
pub fn count_missing_meta(c: &Connection) -> Res<(i64, i64)> {
    let pal: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE missing=0 AND kind=0 AND (palette IS NULL OR palette='')",
            [],
            |r| r.get(0),
        )
        .map_err(e2s)?;
    let hsh: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE missing=0 AND hash_x IS NULL AND size > 0",
            [],
            |r| r.get(0),
        )
        .map_err(e2s)?;
    Ok((pal, hsh))
}

/// 按绝对路径点查素材 id（引用里的绝对路径命中）。Windows 上大小写不敏感。
pub fn asset_id_by_path(c: &Connection, path: &str) -> Res<Option<i64>> {
    let p = path.replace('/', "\\");
    let Some((dir, name)) = p.rsplit_once('\\') else { return Ok(None) };
    c.query_row(
        "SELECT a.id FROM assets a JOIN dirs d ON d.id = a.dir_id
         WHERE a.missing=0 AND lower(a.name)=lower(?1) AND lower(d.path)=lower(?2)
         LIMIT 1",
        params![name, dir],
        |r| r.get(0),
    )
    .optional()
    .map_err(e2s)
}

/// 一棵目录子树下的全部素材：`(id, 绝对路径, 评分, 备注, 来源, 内容哈希)`。
///
/// 移动**整个文件夹**时用：`FsOps` 的 `pairs` 只记顶层项（目录本身），
/// 里面的图不会被逐个列出 —— 不按子树展开的话，跨库拖一个文件夹会把它
/// 里面所有图的标签/评分全丢掉（§10.5 #37 的同一个坑，只是入口不同）。
///
/// 前缀判定放在 Rust 侧（`norm_path` 统一斜杠 + 大小写），不用 SQL `LIKE`：
/// 路径里本来就有反斜杠，`LIKE ... ESCAPE` 的写法在 Windows 路径上极容易写错，
/// 而 `dirs` 表只有几百行，全取一遍再比是最不容易出错的做法。
pub type SubTreeRow = (i64, String, i64, String, String, Option<Vec<u8>>);

pub fn assets_under(c: &Connection, dir_path: &str) -> Res<Vec<SubTreeRow>> {
    let base = crate::norm_path(dir_path);
    let mut dir_ids: Vec<i64> = Vec::new();
    {
        let mut st = c.prepare("SELECT id, path FROM dirs").map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(e2s)?;
        for (id, p) in rows.flatten() {
            let np = crate::norm_path(&p);
            if np == base || np.starts_with(&format!("{base}\\")) {
                dir_ids.push(id);
            }
        }
    }
    if dir_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ph = dir_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        /* note / source_url 允许 NULL（老行、刚扫进来的行），按 String 直接取会
           报 `Invalid column type Null` 然后整行被丢掉 —— 必须 COALESCE */
        "SELECT a.id, d.path || '\\' || a.name,
                COALESCE(a.rating, 0), COALESCE(a.note, ''), COALESCE(a.source_url, ''), a.hash_x
         FROM assets a JOIN dirs d ON d.id = a.dir_id
         WHERE a.missing=0 AND a.dir_id IN ({ph})
         ORDER BY a.id"
    );
    let mut st = c.prepare(&sql).map_err(e2s)?;
    let rows = st
        .query_map(rusqlite::params_from_iter(dir_ids.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .map_err(e2s)?;
    let mut out: Vec<SubTreeRow> = Vec::new();
    for r in rows {
        out.push(r.map_err(e2s)?);
    }
    Ok(out)
}

/// 把一个目录子树从索引里**整棵删掉**，返回删掉的素材行数。
///
/// 移动文件夹跨库时，源路径已经不存在了：`scan_one_dir` 只清"这一层"，
/// 子目录里的行会留在源库里变成指向空路径的幽灵（列表里还能点、缩略图还在）。
/// 这里按 `dirs.path` 前缀把整棵子树（含自己）的行、目录、FTS 一起清掉。
///
/// 只动索引，**不碰磁盘**：文件已经由 `FsOps` 搬走了。
pub fn purge_subtree(c: &Connection, dir_path: &str) -> Res<usize> {
    let base = crate::norm_path(dir_path);
    let mut ids: Vec<(i64, String)> = Vec::new();
    {
        let mut st = c.prepare("SELECT id, path FROM dirs").map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(e2s)?;
        for (id, p) in rows.flatten() {
            let np = crate::norm_path(&p);
            if np == base || np.starts_with(&format!("{base}\\")) {
                ids.push((id, np));
            }
        }
    }
    /* **深的先删**：`dirs.parent_id` 引用了 dirs(id) 且没有 CASCADE，
       先删父目录会撞 FOREIGN KEY constraint failed */
    ids.sort_by_key(|(_, p)| std::cmp::Reverse(p.len()));
    let mut n = 0usize;
    for (id, _) in ids {
        let files: Vec<(i64, String)> = {
            let mut st = c
                .prepare("SELECT id, name FROM assets WHERE dir_id=?1")
                .map_err(e2s)?;
            let rows = st
                .query_map(params![id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(e2s)?;
            rows.filter_map(|x| x.ok()).collect()
        };
        for (aid, name) in &files {
            let _ = fts_del(c, *aid, name, "", "");
        }
        c.execute("DELETE FROM assets WHERE dir_id=?1", params![id])
            .map_err(e2s)?;
        c.execute("DELETE FROM dirs WHERE id=?1", params![id])
            .map_err(e2s)?;
        n += files.len();
    }
    Ok(n)
}

/* ---------------- 内容指纹（快照的身份） ----------------
   快照不存 id：拆库/重建索引之后 id 全会变。存的是**文件内容**的 xxh3-128，
   所以改名字、换目录、换到另一个库里，同一张图照样认得出来。 */

/// 这个素材算过内容哈希没有（NULL = 还没算，查重/加快照时才补）
pub fn asset_hash(c: &Connection, id: i64) -> Res<Option<Vec<u8>>> {
    let v: Option<Option<Vec<u8>>> = c
        .query_row("SELECT hash_x FROM assets WHERE id=?1", params![id], |r| r.get(0))
        .optional()
        .map_err(e2s)?;
    Ok(v.flatten())
}

/// 只写内容哈希。**不碰 `phash`**（那是出缩略图时算的相似指纹，两回事）
pub fn set_hash_only(c: &Connection, id: i64, hash: &[u8]) -> Res<()> {
    c.execute("UPDATE assets SET hash_x=?1 WHERE id=?2", params![hash, id])
        .map_err(e2s)?;
    Ok(())
}

/// 按内容哈希找素材（同一份内容可能有多份拷贝，全给出来；快照取第一个）
pub fn ids_by_hash(c: &Connection, hash: &[u8]) -> Res<Vec<i64>> {
    let mut st = c
        .prepare("SELECT id FROM assets WHERE missing=0 AND hash_x=?1 ORDER BY id")
        .map_err(e2s)?;
    let rows = st.query_map(params![hash], |r| r.get(0)).map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/// 按体积取候选（**改名/新入库的同一张图 hash_x 还是 NULL**，只能先按体积捞出来，再算哈希确认）
pub fn ids_by_size(c: &Connection, size: i64, limit: i64) -> Res<Vec<(i64, Option<Vec<u8>>)>> {
    let mut st = c
        .prepare("SELECT id, hash_x FROM assets WHERE missing=0 AND size=?1 ORDER BY id LIMIT ?2")
        .map_err(e2s)?;
    let rows = st
        .query_map(params![size, limit], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(e2s)?;
    Ok(rows.flatten().collect())
}

/// 按文件名在**全库**找（相对路径引用时文件名是唯一线索；同名取 id 最小，只当提示）
pub fn asset_id_by_name(c: &Connection, name: &str) -> Res<Option<i64>> {
    c.query_row(
        "SELECT id FROM assets WHERE missing=0 AND lower(name)=lower(?1) ORDER BY id LIMIT 1",
        params![name],
        |r| r.get(0),
    )
    .optional()
    .map_err(e2s)
}

/// 谁引用了这张图：`(工程 id, 工程名, 工程路径, 是否缺失)`
pub fn refs_using(c: &Connection, dst_id: i64, limit: i64) -> Res<Vec<(i64, String, String, i32)>> {
    let mut st = c
        .prepare(
            "SELECT s.id, s.name, d.path || (CASE WHEN substr(d.path,-1) IN (char(92),'/') THEN '' ELSE char(92) END) || s.name,
                    r.missing
             FROM refs r
             JOIN assets s ON s.id = r.src_id
             JOIN dirs d ON d.id = s.dir_id
             WHERE r.dst_id = ?1 AND s.missing = 0
             ORDER BY s.name LIMIT ?2",
        )
        .map_err(e2s)?;
    let rows = st
        .query_map(params![dst_id, limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 单个素材的名字（`name_asc` 跨库分页的游标要用）
pub fn name_of(c: &Connection, id: i64) -> Res<Option<String>> {
    c.query_row("SELECT name FROM assets WHERE id=?1", params![id], |r| r.get(0))
        .optional()
        .map_err(e2s)
}

/// 启动增量扫描的候选目录：按「上次扫得最早的先查」，限制条数防止百万目录把启动拖死。
pub fn dirs_to_check(c: &Connection, limit: i64) -> Res<Vec<(i64, String, i64)>> {
    let mut s = c
        .prepare(
            "SELECT id, path, mtime FROM dirs WHERE scanned_at > 0
             ORDER BY scanned_at ASC, id ASC LIMIT ?1",
        )
        .map_err(e2s)?;
    let rows = s
        .query_map(params![limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(e2s)?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 删掉一个**叶子**目录（子目录还在的话不动 —— 删了会让子项变孤儿）。
/// 返回是否真删了。
pub fn remove_dir_if_leaf(c: &Connection, id: i64) -> Res<bool> {
    let n = c
        .execute(
            "DELETE FROM dirs WHERE id=?1 AND NOT EXISTS (SELECT 1 FROM dirs x WHERE x.parent_id=?1)",
            params![id],
        )
        .map_err(e2s)?;
    Ok(n > 0)
}

pub fn dir_count(c: &Connection) -> Res<i64> {
    c.query_row("SELECT COUNT(*) FROM dirs", [], |r| r.get(0)).map_err(e2s)
}

pub fn db_path_for(data_dir: &Path) -> PathBuf {
    data_dir.join("index.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-dbt-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 第一个库（测试里都是单库场景）
    fn lib(app: &crate::App) -> &rusqlite::Connection {
        &app.libs.first().expect("至少要有一个库").conn
    }

    /// 第一个库里所有素材的**全局 id**（按名字排）
    fn gids(app: &crate::App) -> Vec<i64> {
        let lib_id = app.libs[0].id;
        let mut st = lib(app).prepare("SELECT id FROM assets ORDER BY name").unwrap();
        let rows = st.query_map([], |r| r.get::<_, i64>(0)).unwrap();
        rows.flatten().map(|id| crate::gid(lib_id, id)).collect()
    }

    /* ---------------- P3 T44 / T45：结构版本 与 崩溃恢复演练 ---------------- */

    /// T44：没有版本号的老库（v0）被打开时要打上当前版本，且**一行数据都不能少**
    #[test]
    fn old_db_gets_stamped_without_losing_rows() {
        let d = tmp("ver");
        let path = d.join("lib-old.db");
        {
            // 模拟 0.1.16 建的库：表结构在、user_version 还是 0
            let c = open_raw(&path).unwrap();
            c.execute_batch(SCHEMA).unwrap();
            c.execute("INSERT INTO dirs (id, path, name) VALUES (1, '/x', 'x')", [])
                .unwrap();
            c.execute(
                "INSERT INTO assets (dir_id, name, ext, kind, size, mtime, added_at)
                 VALUES (1, 'a.png', 'png', 0, 10, 1, 1)",
                [],
            )
            .unwrap();
            assert_eq!(schema_version(&c).unwrap(), 0);
        }
        let c = open(&path).unwrap();
        assert_eq!(schema_version(&c).unwrap(), SCHEMA_VERSION, "老库要被推进到当前版本");
        let n: i64 = c.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "打标不能碰数据");
        /* 幂等：再开一次不报错、版本不倒退 */
        drop(c);
        let c2 = open(&path).unwrap();
        assert_eq!(schema_version(&c2).unwrap(), SCHEMA_VERSION);
    }

    /// T45 演练 ①：进程被强杀时 WAL 里还有没 checkpoint 的数据 —— 重开必须能读到
    #[test]
    fn uncheckpointed_wal_survives_reopen() {
        let d = tmp("wal");
        let path = d.join("lib-wal.db");
        {
            let c = open(&path).unwrap();
            c.execute("INSERT INTO dirs (id, path, name) VALUES (1, '/w', 'w')", [])
                .unwrap();
            for i in 0..50 {
                c.execute(
                    "INSERT INTO assets (dir_id, name, ext, kind, size, mtime, added_at)
                     VALUES (1, ?1, 'png', 0, 1, 1, 1)",
                    params![format!("{i:02}.png")],
                )
                .unwrap();
            }
            /* 故意不 checkpoint 就"掉电"：连接直接 drop，WAL 留在盘上 */
        }
        let c = open(&path).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 50, "没 checkpoint 的 WAL 必须被重放，不能丢行");
    }

    /// T45 演练 ②：坏库（半个文件）要被留证改名，而不是让软件起不来
    #[test]
    fn truncated_db_is_quarantined_and_replaced() {
        let d = tmp("trunc");
        let path = d.join("lib-bad.db");
        let mut junk = vec![0u8; 4096];
        junk[..16].copy_from_slice(b"SQLite format 3\0");
        std::fs::write(&path, &junk).unwrap();
        let c = open(&path).expect("坏库留证后要能照常开新库");
        let n: i64 = c.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        let left: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".corrupt-"))
            .collect();
        assert!(!left.is_empty(), "坏掉的那份要被改名留证");
    }

    /// 导出（T13）：库根 + 素材的用户层字段都要在，且必须是能读的 JSON。
    #[test]
    fn export_userdata_has_libs_and_assets() {
        let d = tmp("export");
        let root = d.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();

        let mut app = crate::App::open(&d).unwrap();
        app.scan(&root.to_string_lossy(), |_p| {}).unwrap();

        let json = app.export_userdata(None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["app"], "AssetManager");
        let libs = v["libs"].as_array().unwrap();
        assert_eq!(libs.len(), 1);
        // 只导出"人写过东西"的素材：这张图什么都没写过 → 列表为空（不是漏导）
        assert_eq!(v["assets"].as_array().unwrap().len(), 0);

        // 写点评分/备注/标签之后必须出现在导出里
        let id = gids(&app)[0];
        app.set_rating(&[id], 4).unwrap();
        app.set_note(id, "留一手").unwrap();
        app.tag_assets(&[id], "客户稿").unwrap();
        let json = app.export_userdata(None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let assets = v["assets"].as_array().unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0]["name"], "a.png");
        assert_eq!(assets[0]["ext"], "png");
        assert_eq!(assets[0]["rating"], 4);
        assert_eq!(assets[0]["note"], "留一手");
        assert_eq!(assets[0]["tags"][0], "客户稿");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 重建索引（T13）：素材与目录清空，**库根保留**。
    #[test]
    fn reset_keeps_libs() {
        let d = tmp("reset");
        let root = d.join("lib");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("b.jpg"), b"y").unwrap();

        let mut app = crate::App::open(&d).unwrap();
        app.scan(&root.to_string_lossy(), |_p| {}).unwrap();
        assert_eq!(count_assets(lib(&app)).unwrap(), 1);

        let (a, dr) = reset_index(lib(&app)).unwrap();
        assert_eq!(a, 1);
        assert_eq!(dr, 2); // lib + sub
        assert_eq!(count_assets(lib(&app)).unwrap(), 0);
        assert_eq!(dir_count(lib(&app)).unwrap(), 0);
        assert_eq!(list_libs(lib(&app)).unwrap().len(), 1);
        assert_eq!(app.lib_count().unwrap(), 1, "库清单在注册库里，重建不许丢");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 坏库留证（T13）：文件不是数据库 → 改名成 index.corrupt-*.db 后照常起来。
    #[test]
    fn corrupt_db_is_quarantined() {
        let d = tmp("corrupt");
        let p = db_path_for(&d);
        std::fs::write(&p, b"this is definitely not a sqlite database, sorry").unwrap();

        let c = open(&p).expect("坏库应该能自动重建");
        assert_eq!(count_assets(&c).unwrap(), 0);

        let kept: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("index.corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1, "坏库必须留证，不能直接删");
        drop(c);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 标签（2026-11-16）：写入 / 计数 / 按手工标签筛 / 按自动标签筛 / 整组替换 / 移除。
    #[test]
    fn tags_crud_and_filter() {
        let d = tmp("tags");
        let root = d.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();
        std::fs::write(root.join("b.PNG"), b"y").unwrap();
        std::fs::write(root.join("c.pdf"), b"z").unwrap();

        let mut app = crate::App::open(&d).unwrap();
        app.scan(&root.to_string_lossy(), |_p| {}).unwrap();
        let ids = gids(&app);
        assert_eq!(ids.len(), 3);

        // 打标签：两条素材
        assert_eq!(app.tag_assets(&ids[0..2], "客户稿").unwrap(), 2);
        assert_eq!(app.tag_assets(&ids[0..2], "客户稿").unwrap(), 0, "重复打不报错也不重复计数");
        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "客户稿");
        assert_eq!(tags[0].count, 2);

        // 每素材的标签
        let per = app.tags_for(&ids).unwrap();
        assert_eq!(per.len(), 2);
        assert_eq!(per[0].1, vec!["客户稿".to_string()]);

        // 按手工标签筛 / 按自动标签筛：两张 png（大小写不同），一张 pdf
        let by_tag = |tag: &str| {
            app.list(&AssetQuery {
                recursive: true,
                tag: Some(tag.into()),
                limit: Some(50),
                ..Default::default()
            })
            .unwrap()
            .len()
        };
        assert_eq!(by_tag("客户稿"), 2);
        assert_eq!(by_tag("fmt:png"), 2);
        assert_eq!(by_tag("fmt:pdf"), 1);
        assert_eq!(by_tag("kind:doc"), 1);

        // 自动标签计数（跨库汇总那份）
        let auto = app.auto_tags().unwrap();
        assert!(auto.iter().any(|t| t.name == "fmt:png" && t.count == 2));

        // 整组替换：把第一张改成「交付/精选」
        app.set_asset_tags(ids[0], &["交付".into(), "精选".into()]).unwrap();
        let per = app.tags_for(&[ids[0]]).unwrap();
        assert_eq!(per[0].1, vec!["交付".to_string(), "精选".to_string()]);

        // 从所有素材移除
        assert_eq!(app.delete_tag("交付").unwrap(), 1);
        assert!(app.tags().unwrap().iter().all(|t| t.name != "交付"));

        // 保留命名空间不给手工用
        assert!(crate::reg::validate_tag_name("fmt:jpg").is_err());
        assert!(crate::reg::validate_tag_name("  ").is_err());
        assert!(app.tag_assets(&ids, "place:4k").is_err());

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 0.5.0：分组 / 颜色 / 改名合并 / **搜索命中标签名**（T28 之后名字在注册库）。
    #[test]
    fn tag_groups_colors_and_search() {
        let d = tmp("tagmeta");
        let root = d.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        for n in ["a.png", "b.jpg", "c.pdf"] {
            std::fs::write(root.join(n), b"x").unwrap();
        }

        let mut app = crate::App::open(&d).unwrap();
        app.scan(&root.to_string_lossy(), |_p| {}).unwrap();
        let ids = gids(&app);
        assert_eq!(ids.len(), 3);

        // 颜色：白名单之外一律拒（存任意字符串会让深浅主题都失控）
        assert!(crate::reg::validate_color(Some("neon")).is_err());
        assert_eq!(crate::reg::validate_color(Some(" Blue ")).unwrap().as_deref(), Some("blue"));

        // 打标签 + 上色 + 进组
        app.tag_assets(&[ids[0]], "客户稿").unwrap();
        app.set_tag_color("客户稿", Some("blue")).unwrap();
        app.set_tag_group("客户稿", Some("客户")).unwrap();

        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].color.as_deref(), Some("blue"));
        assert_eq!(tags[0].group.as_deref(), Some("客户"));
        assert_eq!(tags[0].count, 1);

        // 空组也要返回（用户刚建完就看不见会以为没建成）
        app.group_new("空组").unwrap();
        let groups = app.groups().unwrap();
        assert_eq!(groups.len(), 2);
        let g = groups.iter().find(|g| g.name == "客户").unwrap();
        assert_eq!(g.count, 1);
        assert_eq!(g.tags, 1);
        let e = groups.iter().find(|g| g.name == "空组").unwrap();
        assert_eq!((e.count, e.tags), (0, 0));

        // 分组换色 / 改名
        app.group_set_color("客户", Some("teal")).unwrap();
        app.group_rename("客户", "客户与项目").unwrap();
        assert!(app.groups().unwrap().iter().any(|g| g.name == "客户与项目"));
        assert_eq!(
            app.groups().unwrap().iter().find(|g| g.name == "客户与项目").unwrap().color.as_deref(),
            Some("teal")
        );

        // 改名撞已有标签 → **合并**，不是报错
        app.tag_assets(&[ids[1]], "归档").unwrap();
        let merged = app.rename_tag("归档", "客户稿").unwrap();
        assert_eq!(merged, 1);
        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1, "合并后只留一条标签");
        assert_eq!(tags[0].count, 2);

        // 搜索命中标签名（文件名里没有「客户」两个字）
        let by_text = |t: &str| {
            app.list(&AssetQuery {
                text: Some(t.into()),
                recursive: true,
                limit: Some(50),
                ..Default::default()
            })
            .unwrap()
            .len()
        };
        assert_eq!(by_text("客户"), 2, "标签名要能被普通搜索命中");
        assert_eq!(by_text("#客户"), 2, "`#` 前缀 = 只搜标签");
        assert_eq!(by_text("#不存在的标签"), 0);
        // 文件名照旧能搜（a.png）
        assert_eq!(by_text("a.png"), 1);

        // 解散分组：标签回到未分组，标签本身不能丢
        assert_eq!(app.group_delete("客户与项目").unwrap(), 1);
        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].group, None);
        assert_eq!(tags[0].color.as_deref(), Some("blue"), "解散分组不影响标签自己的颜色");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 老库升级（0.4.x → 0.5.0 / 0.8.0）：`tags` 表还没有 color / group_id。
    ///
    /// 这条是**真踩过的坑**：一开始把 `CREATE INDEX ... ON tags(group_id)` 放在 SCHEMA 里，
    /// 老库执行 SCHEMA 时列还不存在 → 直接 `no such column: group_id`，
    /// 软件连启动都做不到（setup hook 里报「打开索引失败」）。所以索引必须放到补列之后。
    /// T28 之后 tags 住在注册库，补列在 `libs_migrate::ensure_reg_columns`。
    #[test]
    fn migrates_old_tags_table() {
        let d = tmp("tagmigrate");
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("index.db");
        {
            // 0.4.x 的 tags：只有 id/name/created_at（连 libs 表都没有）
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE tags (
                   id INTEGER PRIMARY KEY,
                   name TEXT NOT NULL UNIQUE,
                   created_at INTEGER NOT NULL
                 );
                 INSERT INTO tags (name, created_at) VALUES ('老标签', 1);",
            )
            .unwrap();
        }

        // App::open 会走注册库建表 + 补列；老标签一条都不能丢
        let mut app = crate::App::open(&d).expect("老库必须能打开，不许在启动阶段崩");
        let tags = app.tags().unwrap();
        assert_eq!(tags.len(), 1, "老标签一条都不能丢");
        assert!(
            libs_migrate_has_column(&app, "tags", "color"),
            "libs_migrate 必须给 tags 补上 color / group_id / auto_rule"
        );

        // 升级后立刻能上色、进组
        app.set_tag_color("老标签", Some("red")).unwrap();
        app.set_tag_group("老标签", Some("老组")).unwrap();
        let t = &app.tags().unwrap()[0];
        assert_eq!(t.color.as_deref(), Some("red"));
        assert_eq!(t.group.as_deref(), Some("老组"));
        assert_eq!(app.groups().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&d);
    }

    fn libs_migrate_has_column(app: &crate::App, table: &str, col: &str) -> bool {
        let mut st = app
            .reg
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("读表结构");
        let names: Vec<String> =
            st.query_map([], |r| r.get::<_, String>(1)).unwrap().flatten().collect();
        names.iter().any(|n| n.eq_ignore_ascii_case(col))
    }

    /// name_asc 深翻（2026-11-16）：必须按 (name, id) keyset 翻页 ——
    /// 不重不漏、大小写不敏感、每页都从上一页最后一行接着走。
    #[test]
    fn name_asc_pages_without_dup_or_gap() {
        let d = tmp("namepage");
        let root = d.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        let names = ["a.png", "B.png", "c.png", "D.png", "e.png", "F.png", "g.png", "h.png"];
        for n in names {
            std::fs::write(root.join(n), b"x").unwrap();
        }

        let mut app = crate::App::open(&d).unwrap();
        app.scan(&root.to_string_lossy(), |_p| {}).unwrap();

        let page = |cursor: Option<(i64, i64)>| {
            let q = AssetQuery {
                recursive: true,
                sort: Some("name_asc".into()),
                cursor,
                limit: Some(3),
                ..Default::default()
            };
            list_assets(lib(&app), &q).unwrap()
        };

        let mut got: Vec<String> = Vec::new();
        let mut cursor: Option<(i64, i64)> = None;
        for _ in 0..10 {
            let rows = page(cursor);
            if rows.is_empty() {
                break;
            }
            let last = rows.last().unwrap();
            cursor = Some(((got.len() + rows.len()) as i64, last.id));
            got.extend(rows.into_iter().map(|a| a.name));
        }

        assert_eq!(got.len(), names.len(), "翻完页必须正好覆盖全库，不多不少");
        let mut uniq = got.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), names.len(), "不许有重复条目");

        let mut expect: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        expect.sort_by_key(|s| s.to_lowercase());
        assert_eq!(got, expect, "COLLATE NOCASE 升序，且翻页顺序与整表排序一致");

        // 游标那一行在翻页间隙被删掉 → 退回 OFFSET 兜底，不能报错也不能从第 1 页重来
        let second = page(Some((0, 1_000_000))).len();
        assert!(second > 0, "id 查不到时也要能给出结果");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 多库（最多 3 个）：移除一个库根只清索引，别的库与磁盘文件都不许受影响。
    ///
    /// 这条 SQL 是高危的（递归 CTE 删整棵子树），写错的后果是「移除 A 库把 B 库也删了」
    /// 或者留下孤儿目录 —— 两种都不会报错，只会在界面上少图 / 少目录。
    #[test]
    fn remove_lib_only_touches_its_own_subtree() {
        let d = tmp("rmlib");
        let a = d.join("lib-a");
        let b = d.join("lib-b");
        std::fs::create_dir_all(a.join("sub/deep")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.png"), b"x").unwrap();
        std::fs::write(a.join("sub/y.png"), b"y").unwrap();
        std::fs::write(a.join("sub/deep/z.png"), b"z").unwrap();
        std::fs::write(b.join("w.png"), b"w").unwrap();

        let mut app = crate::App::open(&d).unwrap();
        app.scan(&a.to_string_lossy(), |_p| {}).unwrap();
        app.scan(&b.to_string_lossy(), |_p| {}).unwrap();
        assert_eq!(app.lib_count().unwrap(), 2, "两个库都在");
        assert_eq!(app.roots().unwrap().len(), 2);

        let removed = app.remove_lib(&a.to_string_lossy()).unwrap();
        assert!(removed >= 3, "A 库的目录（含两层子目录）全要清掉，实际 {}", removed);

        assert_eq!(app.lib_count().unwrap(), 1, "只剩 B 库");
        let roots = app.roots().unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].path.ends_with("lib-b"), "留下的必须是 B 库");

        // 孤儿目录（父不在表里）必须为 0：那条递归 CTE 漏一层就会留下这种行
        for l in &app.libs {
            let orphans: i64 = l
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM dirs d WHERE d.parent_id IS NOT NULL
                     AND NOT EXISTS (SELECT 1 FROM dirs p WHERE p.id = d.parent_id)",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(orphans, 0, "不许留下孤儿目录");
        }

        // B 库的素材还在，A 库的素材没了
        let total = app
            .list(&AssetQuery { recursive: true, limit: Some(100), ..Default::default() })
            .unwrap();
        assert_eq!(total.len(), 1, "只剩 B 库那张 w.png");
        assert_eq!(total[0].name, "w.png");

        // **磁盘一个文件都没动** —— 移除库是「不再管这个文件夹」，不是删文件
        assert!(a.join("x.png").exists(), "A 库的文件必须还在");
        assert!(a.join("sub/deep/z.png").exists());
        assert!(b.join("w.png").exists());

        let _ = std::fs::remove_dir_all(&d);
    }

    /// T23：评分 / 注释 / 来源 URL 必须真的落库，而且**扫描不许把它们抹掉**。
    ///
    /// 这三样是"人写的东西"：`sync_dir` 每次扫描都会重写文件事实那一批列，
    /// 如果哪天有人图省事用 `INSERT OR REPLACE`，用户手写的评分会在下次扫描时全没 —— 不报错、不提示。
    #[test]
    fn user_fields_survive_rescan_and_sort_by_rating() {
        let d = tmp("t23");
        let dir = d.join("lib");
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["a.png", "b.png", "c.png"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let mut app = crate::App::open(&d).unwrap();
        app.scan(&dir.to_string_lossy(), |_p| {}).unwrap();
        let ids = gids(&app);
        assert_eq!(ids.len(), 3);

        // a=5 星 + 注释 + 来源；c=2 星
        app.set_rating(&[ids[0]], 5).unwrap();
        app.set_rating(&[ids[2]], 2).unwrap();
        app.set_note(ids[0], "客户终稿").unwrap();
        app.set_source_url(ids[0], "https://example.com/a").unwrap();

        // 再扫一遍（真实场景：启动增量扫描 / 手动重建）
        app.scan(&dir.to_string_lossy(), |_p| {}).unwrap();

        let a = app.asset(ids[0]).unwrap().unwrap();
        assert_eq!(a.rating, 5, "重扫不许抹掉评分");
        assert_eq!(a.note, "客户终稿", "重扫不许抹掉注释");
        assert_eq!(a.source_url, "https://example.com/a", "重扫不许抹掉来源");

        // 按评分排序：5 → 2 → 0
        let page = app
            .list(&AssetQuery {
                recursive: true,
                sort: Some("rating_desc".into()),
                limit: Some(10),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.len(), 3);
        assert_eq!(page[0].rating, 5);
        assert_eq!(page[1].rating, 2);
        assert_eq!(page[2].rating, 0);

        // 评分排序的 keyset 翻页：游标是 (rating, id)，不许重复也不许漏
        let first = app
            .list(&AssetQuery {
                recursive: true,
                sort: Some("rating_desc".into()),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        let rest = app
            .list(&AssetQuery {
                recursive: true,
                sort: Some("rating_desc".into()),
                cursor: Some((first[0].rating as i64, first[0].id)),
                limit: Some(10),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rest.len(), 2, "翻页后还剩 2 条");
        assert!(rest.iter().all(|r| r.id != first[0].id), "不许重复");

        let _ = std::fs::remove_dir_all(&d);
    }
}
