//! 加库可见性：`am_scan_start` 用**独立连接的 App** 扫描，新库只写进注册库；
//! 界面那个常驻实例必须 `reload_libs()` 才认得它。
//!
//! 2026-11-23 owner 报「导入素材库后左侧面板 0/3、里面的图片一张都不出来」：
//! 侧栏读的是 `App.libs`（内存快照），扫描线程写完注册库没人同步 → 库不可见。
//! 壳层现在在 `scan://done` 之前调 `reload_libs()`，这个测试守住那一步的前提。

use am_core::App;

/// 1×1 透明 PNG
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

#[test]
fn reload_libs_picks_up_lib_scanned_by_other_instance() {
    let base = std::env::temp_dir().join(format!("am-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    let lib = base.join("libA");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("a.png"), PNG).unwrap();

    // 主实例（界面常驻的那个）
    let mut main_app = App::open(&data).unwrap();
    assert_eq!(main_app.roots().unwrap().len(), 0, "一开始没有库");

    // 扫描线程里的实例：新开连接、新开 App
    {
        let mut scanner = App::open(&data).unwrap();
        scanner.scan(&lib.to_string_lossy(), |_| {}).unwrap();
    }

    // 不 reload → 主实例看不到（这就是缺陷本身：要重启软件才认）
    assert_eq!(
        main_app.roots().unwrap().len(),
        0,
        "扫描线程的写入不该凭空出现在主实例的内存快照里"
    );

    // reload 之后必须看到
    main_app.reload_libs();
    let roots = main_app.roots().unwrap();
    assert_eq!(roots.len(), 1, "reload_libs 之后主实例应看到刚扫进来的库");
    assert_eq!(roots[0].name.to_lowercase(), "liba");
}
