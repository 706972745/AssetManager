//! 集合的跨库查询护栏（0.5.7）。
//!
//! owner 2026-11-23：「在截图库建的分组/快照/标签，到导入库点开是空的」。
//! 根因是 `App::list` 最后一步只扫当前库，而侧栏的集合元数据是合并显示的。
//!
//! 这个文件守住三条口径，避免以后又把 targets 收回当前库：
//! 1. **分组**查它**所属的那个库**（与当前选中的库无关）；
//! 2. **标签**按名字跨**所有库**合并（同名不同 id）；
//! 3. **普通浏览**（无集合筛选）仍然只返回当前库（0.4.5 的隔离不能回退）。

use am_core::model::AssetQuery;
use am_core::{lib_of, App};

/// 1×1 透明 PNG
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

struct Fixture {
    _base: std::path::PathBuf,
    a: std::path::PathBuf,
    b: std::path::PathBuf,
    app: App,
}

/// 建两个库（各一张图），扫进来，当前库先落在 A。
fn fixture(tag: &str) -> Fixture {
    let base = std::env::temp_dir().join(format!("am-shelf-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let a = base.join("libA");
    let b = base.join("libB");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("a.png"), PNG).unwrap();
    /* B 用不同内容：快照按**内容哈希**找图，两库同名同内容会让断言依赖库的遍历顺序 */
    std::fs::write(b.join("b.txt"), b"B").unwrap();

    let mut app = App::open(base.join("data")).unwrap();
    app.scan(&a.to_string_lossy(), |_| {}).unwrap();
    app.scan(&b.to_string_lossy(), |_| {}).unwrap();
    app.set_current_lib(&a.to_string_lossy()).unwrap();

    Fixture { _base: base, a, b, app }
}

fn all(app: &App) -> Vec<am_core::model::Asset> {
    app.list(&AssetQuery {
        limit: Some(100),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn collection_opened_from_another_library_returns_its_own_members() {
    let mut f = fixture("coll");

    // 在 B 库建分组：成员只能是 B 的素材
    f.app.set_current_lib(&f.b.to_string_lossy()).unwrap();
    let b_ids: Vec<i64> = all(&f.app).iter().map(|a| a.id).collect();
    assert_eq!(b_ids.len(), 1, "B 库应该刚好一张图");
    let coll = f.app.coll_create("B 的分组").unwrap();
    assert_eq!(f.app.coll_add(coll, &b_ids).unwrap(), 1);

    // 切回 A 库再点开 B 的分组：必须能查到 B 的那张
    f.app.set_current_lib(&f.a.to_string_lossy()).unwrap();
    let rows = f
        .app
        .list(&AssetQuery {
            collection: Some(coll),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1, "分组属于 B 库，切到 A 库也必须查得到");
    assert_eq!(rows[0].id, b_ids[0]);
    assert_eq!(lib_of(rows[0].id), lib_of(b_ids[0]));

    /* 计数与列表同口径：把 B 标成内置库（侧栏可见），coll_list 的 count 必须等于列表长度 */
    f.app.set_builtin_roots(vec![f.b.to_string_lossy().to_string()]);
    let count = f
        .app
        .coll_list()
        .unwrap()
        .into_iter()
        .find(|c| c.id == coll)
        .map(|c| c.count)
        .expect("内置库的分组该在侧栏里");
    assert_eq!(count, rows.len() as i64, "侧栏写几张，点开就该有几张");
}

#[test]
fn same_tag_name_merges_across_two_libraries() {
    let mut f = fixture("tag");

    let a_ids: Vec<i64> = all(&f.app).iter().map(|a| a.id).collect();
    f.app.set_current_lib(&f.b.to_string_lossy()).unwrap();
    let b_ids: Vec<i64> = all(&f.app).iter().map(|a| a.id).collect();

    // 两个库各建一份同名标签（id 不同，名字相同）
    f.app.tag_assets(&a_ids, "共享").unwrap();
    f.app.tag_assets(&b_ids, "共享").unwrap();

    f.app.set_current_lib(&f.a.to_string_lossy()).unwrap();
    let rows = f
        .app
        .list(&AssetQuery {
            tag: Some("共享".into()),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(rows.len(), 2, "同名标签必须跨两库合并");
    let libs: std::collections::HashSet<i64> = rows.iter().map(|r| lib_of(r.id)).collect();
    assert_eq!(libs.len(), 2, "两库的素材都要出现");

    // 计数口径与列表一致
    let count = f
        .app
        .tags()
        .unwrap()
        .into_iter()
        .find(|t| t.name == "共享")
        .map(|t| t.count)
        .unwrap_or(-1);
    assert_eq!(count, rows.len() as i64, "侧栏计数必须等于点开后的张数");
}

#[test]
fn plain_browse_stays_in_current_library() {
    let f = fixture("plain");

    let rows = all(&f.app);
    assert_eq!(rows.len(), 1, "A 库只有一张，普通浏览不该把 B 库的带进来");
    let want = f.app.roots().unwrap();
    let a_lib = want
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case("liba"))
        .map(|r| lib_of(r.id))
        .unwrap();
    assert!(rows.iter().all(|r| lib_of(r.id) == a_lib));
}

#[test]
fn collection_of_removed_library_returns_empty_not_everything() {
    let mut f = fixture("gone");

    f.app.set_current_lib(&f.b.to_string_lossy()).unwrap();
    let b_ids: Vec<i64> = all(&f.app).iter().map(|a| a.id).collect();
    let coll = f.app.coll_create("B 的分组").unwrap();
    f.app.coll_add(coll, &b_ids).unwrap();

    // 移除 B 库（只摘注册，不删库文件）
    f.app.remove_lib(&f.b.to_string_lossy()).unwrap();
    f.app.set_current_lib(&f.a.to_string_lossy()).unwrap();

    let rows = f
        .app
        .list(&AssetQuery {
            collection: Some(coll),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap();
    assert!(rows.is_empty(), "所属库没了就是 0 条，绝不能退化成把 A 库列出来");
}

#[test]
fn snapshot_opened_from_another_library_finds_members_by_hash() {
    let mut f = fixture("snap");

    // 快照建在 A 库，成员是 A 的图
    let a_ids: Vec<i64> = all(&f.app).iter().map(|a| a.id).collect();
    let snap = f.app.snapshot_create("A 的快照").unwrap();
    let added = f.app.snapshot_add_assets(snap, &a_ids).unwrap();
    assert_eq!(added.added, 1);

    // 切到 B 库再点开 A 的快照：内容哈希命中 A 的图
    f.app.set_current_lib(&f.b.to_string_lossy()).unwrap();
    let rows = f
        .app
        .list(&AssetQuery {
            snapshot: Some(snap),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1, "快照按哈希跨库，切库后照样查得到");
    assert_eq!(rows[0].id, a_ids[0]);

    let stats = f.app.snapshot_stats(snap).unwrap();
    assert_eq!((stats.total, stats.found, stats.missing), (1, 1, 0));
}
