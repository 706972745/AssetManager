//! 老库迁移（0.7.x 单库多根 → T28 每库一个文件）。
//!
//! 0.7.x 及以前：`index.db` 一个文件里装着"库清单 + 全部 dirs/assets/asset_tags"。
//! 0.8.0 起：`index.db` 只是注册库（库清单 + 标签体系），素材内容分到 `libs/lib-<id>.db`。
//!
//! 迁移只做一次，判据是**注册库里还有没有 `assets` 表**（拆完就 drop 掉）。
//! 三条纪律：
//! 1. 动手前把老 `index.db` 拷一份到 `trash-libs/index-pre-split-<ts>.db` —— 拆库是
//!    结构手术，出问题时用户手里必须还有原样的一份；
//! 2. **保留旧 id**（dirs / assets 都带 id 插入）：不做 id 映射表，重跑也不会造重复
//!    （`INSERT OR IGNORE`），代价只是各库 id 有空洞，而 id 从不外露给用户；
//! 3. 缩略图缓存文件是**全局共用**的（`cache/thumbs`），所以 `thumb_lru` 只把"本库素材
//!    真正引用到的 key"抄过来，否则同一个 key 会在三个库里各记一次，GC 时重复计数。

use std::path::Path;

use rusqlite::{params, Connection};

use crate::db;
use crate::db::Res;

/// 迁移入口。没有老表时什么都不做（正常情况下只花两条 `sqlite_master` 查询）。
pub fn maybe_migrate(reg: &mut Connection, data_dir: &Path) -> Res<()> {
    ensure_reg_columns(reg)?;

    if !has_table(reg, "assets")? {
        return Ok(());
    }

    let index_path = data_dir.join("index.db");
    let keep = data_dir.join("trash-libs");
    std::fs::create_dir_all(&keep).map_err(|e| e.to_string())?;
    let backup = keep.join(format!("index-pre-split-{}.db", db::now_ts()));
    // 备份失败就**不拆**：宁可这次仍旧走老结构，也不能在没有退路的情况下动结构
    std::fs::copy(&index_path, &backup)
        .map_err(|e| format!("拆库前备份失败，已中止：{e}"))?;

    // 老库的库根（按 id 升序，和侧栏顺序一致）
    let mut roots: Vec<(i64, String, i64)> = Vec::new();
    {
        let mut st = reg
            .prepare("SELECT id, root, added_at FROM libs ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        roots = rows.filter_map(|x| x.ok()).collect();
    }
    // 老库里可能有素材但库里一行都没有（早期版本只 add_lib 没写 root）→ 从 dirs 顶层补
    if roots.is_empty() {
        let mut st = reg
            .prepare(
                "SELECT id, path FROM dirs WHERE parent_id IS NULL AND id IN
                 (SELECT DISTINCT dir_id FROM assets) ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
        roots = rows.filter_map(|x| x.ok()).map(|(i, p): (i64, String)| (i, p, db::now_ts())).collect();
    }

    for (lib_id, root, added_at) in &roots {
        let rel = format!("libs/lib-{}.db", lib_id);
        let abs = data_dir.join(&rel);
        if let Some(p) = abs.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut lib = db::open(&abs)?;
        // 库文件自述根（单文件拷走时仍自洽）
        lib.execute(
            "INSERT OR IGNORE INTO libs (id, root, added_at) VALUES (?1,?2,?3)",
            params![lib_id, root, added_at],
        )
        .map_err(|e| e.to_string())?;

        split_one(&mut lib, &index_path, root)?;

        // 注册库补 file 列（相对数据目录，换机/换盘后仍成立）
        reg.execute(
            "UPDATE libs SET file = ?1 WHERE id = ?2",
            params![rel, lib_id],
        )
        .map_err(|e| e.to_string())?;
        let _ = db::rebuild_fts(&lib);
    }

    // 拆完把老表清掉：不清的话下次启动会再拆一遍（而且会把已删素材复活）
    drop_old_tables(reg)?;
    reg.execute(
        "INSERT INTO meta (k, v) VALUES ('split_at', ?1)
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![db::now_ts().to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 把一个库根下的内容搬进它自己的库文件。`old` 是挂上来的老 index.db。
fn split_one(lib: &mut Connection, index_path: &Path, root: &str) -> Res<()> {
    lib.execute("ATTACH DATABASE ?1 AS old", params![index_path.to_string_lossy()])
        .map_err(|e| format!("挂载老索引失败：{e}"))?;

    let prefix = format!("{}%", like_escape(root));
    let args = params![root, prefix];

    // 1) 目录：保留 id 与 parent_id。根之上的父目录（多根老库里 parent 指向别处）会变成空引用，
    //    所以 parent_id 用 CASE 兜一下：不在本库范围内的父 → NULL（它就是新库的根）。
    lib.execute(
        &format!(
            "INSERT OR IGNORE INTO dirs
             (id, path, name, parent_id, mtime, scanned_at, child_cnt, asset_cnt, cnt_all, needs_rescan)
             SELECT o.id, o.path, o.name,
                    CASE WHEN o.parent_id IS NULL THEN NULL
                         WHEN EXISTS (SELECT 1 FROM old.dirs p WHERE p.id = o.parent_id
                                      AND (p.path = ?1 OR p.path LIKE ?2 ESCAPE '\\'))
                         THEN o.parent_id ELSE NULL END,
                    o.mtime, o.scanned_at, o.child_cnt, o.asset_cnt,
                    COALESCE(o.cnt_all, 0), o.needs_rescan
             FROM old.dirs o WHERE o.path = ?1 OR o.path LIKE ?2 ESCAPE '\\'"
        ),
        args,
    )
    .map_err(|e| format!("搬目录失败：{e}"))?;

    // 2) 素材：新列（palette / color / source_url / refs_at）在老库里可能不存在，用 0/空串兜
    let has = |c: &str| has_column_in(lib, "old", "assets", c).unwrap_or(false);
    let col = |name: &str, fallback: &str| {
        if has(name) {
            format!("o.{name}")
        } else {
            fallback.to_string()
        }
    };
    let sql = format!(
        "INSERT OR IGNORE INTO assets
         (id, dir_id, name, ext, kind, size, mtime, width, height, thumb_key, thumb_state, quality,
          hash_x, phash, palette, color, note, rating, source_url, refs_at, added_at, missing)
         SELECT o.id, o.dir_id, o.name, o.ext, o.kind, o.size, o.mtime, o.width, o.height,
                o.thumb_key, o.thumb_state, o.quality,
                {}, {}, {}, {}, {}, {}, {}, {},
                COALESCE(o.added_at, 0), COALESCE(o.missing, 0)
         FROM old.assets o JOIN old.dirs d ON d.id = o.dir_id
         WHERE d.path = ?1 OR d.path LIKE ?2 ESCAPE '\\'",
        col("hash_x", "NULL"),
        col("phash", "NULL"),
        col("palette", "NULL"),
        col("color", "NULL"),
        col("note", "NULL"),
        col("rating", "0"),
        col("source_url", "NULL"),
        col("refs_at", "0"),
    );
    lib.execute(&sql, args).map_err(|e| format!("搬素材失败：{e}"))?;

    // 3) 标签关联（tag_id 是注册库的 id，拆库不动它）
    if has_table_in(lib, "old", "asset_tags")? {
        lib.execute(
            "INSERT OR IGNORE INTO asset_tags (asset_id, tag_id)
             SELECT t.asset_id, t.tag_id FROM old.asset_tags t
             WHERE t.asset_id IN (SELECT o.id FROM old.assets o JOIN old.dirs d ON d.id = o.dir_id
                                  WHERE d.path = ?1 OR d.path LIKE ?2 ESCAPE '\\')",
            args,
        )
        .map_err(|e| format!("搬标签失败：{e}"))?;
    }

    // 4) 缓存记账：只抄本库素材真用到的 key（缓存目录是全局的，抄多了 GC 会重复算）
    if has_table_in(lib, "old", "thumb_lru")? {
        lib.execute(
            "INSERT OR IGNORE INTO thumb_lru (key, bytes, last_used)
             SELECT l.key, l.bytes, l.last_used FROM old.thumb_lru l
             WHERE l.key IN (SELECT o.thumb_key FROM old.assets o JOIN old.dirs d ON d.id = o.dir_id
                             WHERE (d.path = ?1 OR d.path LIKE ?2 ESCAPE '\\')
                               AND o.thumb_key IS NOT NULL)",
            args,
        )
        .map_err(|e| format!("搬缓存记账失败：{e}"))?;
    }

    lib.execute("DETACH DATABASE old", []).map_err(|e| e.to_string())?;
    Ok(())
}

fn drop_old_tables(reg: &Connection) -> Res<()> {
    for t in ["assets", "dirs", "asset_tags", "refs", "thumb_lru", "assets_fts"] {
        // 表名是常量，不存在注入风险；FTS 的辅助表会跟着主表一起走
        reg.execute_batch(&format!("DROP TABLE IF EXISTS {t};"))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 注册库补列：老 `libs` 没有 `file`，老 `tags` 没有 color/group_id/auto_rule。
fn ensure_reg_columns(reg: &Connection) -> Res<()> {
    if !has_column(reg, "libs", "file")? && has_table(reg, "libs")? {
        reg.execute("ALTER TABLE libs ADD COLUMN file TEXT NOT NULL DEFAULT ''", [])
            .map_err(|e| e.to_string())?;
    }
    for (name, ddl) in [
        ("color", "COLOR TEXT"),
        ("group_id", "INTEGER"),
        ("auto_rule", "TEXT"),
    ] {
        if has_table(reg, "tags")? && !has_column(reg, "tags", name)? {
            reg.execute(&format!("ALTER TABLE tags ADD COLUMN {name} {ddl}"), [])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn has_table(c: &Connection, t: &str) -> Res<bool> {
    has_table_in(c, "main", t)
}

fn has_table_in(c: &Connection, schema: &str, t: &str) -> Res<bool> {
    c.query_row(
        &format!("SELECT COUNT(*) FROM {schema}.sqlite_master WHERE type='table' AND name=?1"),
        params![t],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(|e| e.to_string())
}

fn has_column(c: &Connection, table: &str, col: &str) -> Res<bool> {
    has_column_in(c, "main", table, col)
}

fn has_column_in(c: &Connection, schema: &str, table: &str, col: &str) -> Res<bool> {
    let mut st = c
        .prepare(&format!("PRAGMA {schema}.table_info({table})"))
        .map_err(|e| e.to_string())?;
    let rows = st.query_map([], |r| r.get::<_, String>(1)).map_err(|e| e.to_string())?;
    let names: Vec<String> = rows.flatten().collect();
    Ok(names.iter().any(|n| n.eq_ignore_ascii_case(col)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-split-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 老库（单文件多根）→ 每库一个文件：素材、父子目录、标签关联都要跟过去，
    /// 而且**老表必须被清掉**，否则下次启动会重复拆。
    #[test]
    fn splits_old_single_db_into_libs() {
        let d = tmp("one");
        let roots = [d.join("lib-a"), d.join("lib-b"), d.join("lib-a").join("sub")];
        for r in &roots {
            std::fs::create_dir_all(r).unwrap();
        }
        std::fs::write(roots[0].join("x.png"), b"a").unwrap();
        std::fs::write(roots[2].join("y.png"), b"b").unwrap();
        std::fs::write(roots[1].join("z.png"), b"c").unwrap();

        // 造一个 0.7.x 结构的老 index.db：libs + dirs + assets + asset_tags
        {
            let old = db::open(&d.join("index.db")).unwrap();
            old.execute_batch(
                "CREATE TABLE IF NOT EXISTS asset_tags (asset_id INTEGER, tag_id INTEGER);",
            )
            .unwrap();
            let a = db::add_lib(&old, &roots[0].to_string_lossy()).unwrap();
            let b = db::add_lib(&old, &roots[1].to_string_lossy()).unwrap();
            assert_ne!(a, b);
            let ts = 1_700_000_000i64;
            let dir_a = db::upsert_dir(&old, &roots[0].to_string_lossy(), None, ts).unwrap();
            let dir_b = db::upsert_dir(&old, &roots[1].to_string_lossy(), None, ts).unwrap();
            let dir_sub = db::upsert_dir(&old, &roots[2].to_string_lossy(), Some(dir_a), ts).unwrap();
            db::upsert_asset(&old, dir_a, "x.png", "png", 0, 1, ts, Some(10), Some(10)).unwrap();
            db::upsert_asset(&old, dir_b, "z.png", "png", 0, 1, ts, Some(10), Some(10)).unwrap();
            let aid = db::upsert_asset(&old, dir_sub, "y.png", "png", 0, 1, ts, Some(8), Some(8))
                .unwrap();
            old.execute("INSERT INTO asset_tags (asset_id, tag_id) VALUES (?1, 7)", params![aid])
                .unwrap();
        }

        let mut reg = crate::reg::open(&d.join("index.db")).unwrap();
        maybe_migrate(&mut reg, &d).unwrap();

        // 两个库各自完整：lib-a（根 + sub，2 张图），lib-b（1 张图）
        let libs = crate::reg::libs(&reg).unwrap();
        assert_eq!(libs.len(), 2);
        let mut counts = Vec::new();
        for row in &libs {
            assert!(row.file.starts_with("libs/lib-"), "{}", row.file);
            let c = db::open(&d.join(&row.file)).unwrap();
            let n: i64 = c.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0)).unwrap();
            let dirs: i64 = c.query_row("SELECT COUNT(*) FROM dirs", [], |r| r.get(0)).unwrap();
            counts.push((n, dirs));
            // 子目录的 parent 必须落在同一库里
            let dangling: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM dirs d WHERE d.parent_id IS NOT NULL
                     AND NOT EXISTS (SELECT 1 FROM dirs p WHERE p.id = d.parent_id)",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(dangling, 0);
        }
        counts.sort();
        assert_eq!(counts, vec![(1, 1), (2, 2)], "两个库的素材/目录数都要对得上");

        // 标签关联跟着所属库走（只有 lib-a 那条）
        let lib_a = libs
            .iter()
            .find(|l| l.root.ends_with("lib-a"))
            .expect("lib-a 应该在注册库里");
        let ca = db::open(&d.join(&lib_a.file)).unwrap();
        let tags: i64 = ca.query_row("SELECT COUNT(*) FROM asset_tags", [], |r| r.get(0)).unwrap();
        assert_eq!(tags, 1);
        // FTS 是 contentless 表：拆完必须自己灌一遍，否则搜索全空。
        // 注意 trigram 分词器**最短要 3 个字符**才建索引（拿 'y' 去 MATCH 永远是 0）。
        let fts_rows: i64 =
            ca.query_row("SELECT COUNT(*) FROM assets_fts", [], |r| r.get(0)).unwrap();
        assert_eq!(fts_rows, 2, "拆完要把 FTS 行灌回去");
        let hit: i64 = ca
            .query_row(
                // 查询串要整体加引号：`y.png` 里的点会被 FTS5 当成语法符号
                "SELECT COUNT(*) FROM assets_fts WHERE assets_fts MATCH '\"y.png\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "按名字能搜到");

        // 老表已清，再跑一次是 no-op
        assert!(!has_table(&reg, "assets").unwrap());
        maybe_migrate(&mut reg, &d).unwrap();
        let n: i64 = crate::reg::lib_count(&reg).unwrap();
        assert_eq!(n, 2);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 新库（没有 assets 表）不该被碰，也不该多出备份文件
    #[test]
    fn fresh_db_is_untouched() {
        let d = tmp("fresh");
        let mut reg = crate::reg::open(&d.join("index.db")).unwrap();
        crate::reg::add_lib(&reg, "E:/x", "libs/lib-1.db").unwrap();
        maybe_migrate(&mut reg, &d).unwrap();
        assert!(!d.join("trash-libs").exists());
        assert_eq!(crate::reg::lib_count(&reg).unwrap(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
