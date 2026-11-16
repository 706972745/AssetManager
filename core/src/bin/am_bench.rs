//! 压测台（T04）：不开界面也能打点。
//!
//! 两段：
//! 1. **合成索引**：直接往 SQLite 里灌 N 条 assets（含 FTS 行），量列表 / 搜索 /
//!    计数 / 目录树这些**随库规模增长**的查询；
//! 2. **真实扫描**：在临时目录里造 M 张小图，跑一遍扫描器，量吞吐与耗时。
//!
//! 用法：
//!   am-bench                          # 默认 10 万行 + 500 目录 + 2 万个真实文件
//!   am-bench --rows 1000000 --dirs 2000 --disk 0
//!   am-bench --data E:\tmp\am-bench   # 指定工作目录（会先清空）

use std::path::{Path, PathBuf};
use std::time::Instant;

use am_core::{db, model, App};

fn arg_u64(args: &[String], name: &str, dflt: u64) -> u64 {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(dflt)
}

fn arg_str(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn time<F: FnMut()>(reps: usize, mut f: F) -> f64 {
    // 先跑一次热身（页缓存 / 语句编译），再计时
    f();
    let mut samples = Vec::with_capacity(reps);
    for _ in 0..reps {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    median(samples)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rows = arg_u64(&args, "--rows", 100_000);
    let dirs = arg_u64(&args, "--dirs", 500).max(2);
    let disk = arg_u64(&args, "--disk", 20_000);
    let disk_dirs = arg_u64(&args, "--disk-dirs", 200).max(1);
    let data = arg_str(&args, "--data")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("am-bench"));

    println!("== am-bench ==");
    println!(
        "合成 {} 条素材 / {} 个目录 · 真实文件 {} 个 / {} 个目录",
        rows, dirs, disk, disk_dirs
    );
    println!("工作目录：{}\n", data.display());

    if data.exists() {
        let _ = std::fs::remove_dir_all(&data);
    }
    let mut app = match App::open(&data) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("打开数据目录失败：{e}");
            std::process::exit(1);
        }
    };

    // ---------------------------------------------------------------- 合成索引
    let root = data.join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let root_s = root.to_string_lossy().to_string();

    let t = Instant::now();
    {
        let c = &mut app.libs[0].conn;
        let tx = c.transaction().unwrap();
        let root_id = db::upsert_dir(&tx, &root_s, None, 1).unwrap();
        db::add_lib(&tx, &root_s).unwrap();
        let mut dir_ids = vec![root_id];
        for d in 0..dirs {
            let p = root.join(format!("组{:03}", d));
            std::fs::create_dir_all(&p).unwrap();
            let id = db::upsert_dir(&tx, &p.to_string_lossy(), Some(root_id), 1).unwrap();
            dir_ids.push(id);
        }
        // 一条 SQL 批量灌（1 万行一次），比逐条 upsert 快两个数量级
        let now = db::now_ts();
        let mut batch = String::with_capacity(1 << 20);
        let kinds = [0, 0, 0, 0, 1, 4, 3];
        for i in 0..rows as i64 {
            let dir_id = dir_ids[(i as usize) % dir_ids.len()];
            let kind = kinds[(i as usize) % kinds.len()];
            let ext = ["png", "jpg", "webp", "psd", "mp4", "pdf", "blend"][(i as usize) % 7];
            let name = format!("素材_{:08}.{}", i, ext);
            let size = 1_000 + (i * 37) % 5_000_000;
            let mtime = 1_700_000_000 + (i % 90_000);
            let w = 800 + (i % 3000);
            let h = 600 + (i % 2000);
            batch.push_str(&format!(
                "INSERT INTO assets (id,dir_id,name,ext,kind,size,mtime,width,height,added_at) \
                 VALUES ({i},{dir_id},'{name}','{ext}',{kind},{size},{mtime},{w},{h},{now});\n\
                 INSERT INTO assets_fts (rowid,name,note,tags) VALUES ({i},'{name}','','');\n"
            ));
            if batch.len() > (1 << 20) {
                tx.execute_batch(&batch).unwrap();
                batch.clear();
            }
        }
        tx.execute_batch(&batch).unwrap();
        tx.commit().unwrap();
    }
    println!("灌库耗时：{:.2} 秒", t.elapsed().as_secs_f64());

    // 生产中 cnt_all 是扫描时顺手维护的；这里批量灌库，补算一次（= 老库迁移的代价）
    let t2 = Instant::now();
    db::rebuild_cnt_all(&app.libs[0].conn).unwrap();
    println!("cnt_all 全量回填：{:.2} 秒", t2.elapsed().as_secs_f64());

    // ---------------------------------------------------------------- 查询打点
    let q = |sort: &str, text: Option<&str>, cursor: Option<(i64, i64)>| model::AssetQuery {
        dir_id: Some(1),
        recursive: true,
        kinds: None,
        text: text.map(|s| s.to_string()),
        sort: Some(sort.to_string()),
        cursor,
        limit: Some(200),
        tag: None,
        ..Default::default()
    };

    println!("\n-- 查询（中位数，10 次）--");
    let cases: Vec<(&str, f64)> = vec![
        (
            "列表 · 首页 200（mtime desc）",
            time(10, || {
                let _ = app.list(&q("mtime_desc", None, None)).unwrap();
            }),
        ),
        (
            "列表 · 第 2 页（keyset）",
            time(10, || {
                let _ = app.list(&q("mtime_desc", None, Some((1_700_089_999, 95_000)))).unwrap();
            }),
        ),
        (
            "列表 · 按大小（首页）",
            time(10, || {
                let _ = app.list(&q("size_desc", None, None)).unwrap();
            }),
        ),
        (
            "搜索 · 3 字符（FTS trigram）",
            time(10, || {
                let _ = app.list(&q("mtime_desc", Some("素材_0001"), None)).unwrap();
            }),
        ),
        (
            "搜索 · 2 字符（LIKE 兜底）",
            time(10, || {
                let _ = app.list(&q("mtime_desc", Some("01"), None)).unwrap();
            }),
        ),
        (
            "计数 · COUNT(*)",
            time(10, || {
                let _ = db::count_assets(&app.libs[0].conn).unwrap();
            }),
        ),
        (
            "目录树 · 根层（含子树递归计数）",
            time(5, || {
                let _ = app.tree(None).unwrap();
            }),
        ),
    ];
    for (name, ms) in &cases {
        println!("{:34} {:8.2} ms", name, ms);
    }

    // ---------------------------------------------------------------- 真实扫描
    if disk > 0 {
        println!("\n-- 真实扫描（{} 个文件）--", disk);
        let lib = data.join("disk-lib");
        std::fs::create_dir_all(&lib).unwrap();
        // 一张真 JPEG，重复写入（比每张都编码快得多，也能让 imageinfo 读到真尺寸）
        let mut jpg = Vec::new();
        {
            let img = image::RgbImage::from_fn(16, 16, |x, y| {
                image::Rgb([(x * 16) as u8, (y * 16) as u8, 128])
            });
            img.write_to(&mut std::io::Cursor::new(&mut jpg), image::ImageFormat::Jpeg)
                .unwrap();
        }
        let gen = Instant::now();
        for d in 0..disk_dirs {
            let dir = lib.join(format!("d{:03}", d));
            std::fs::create_dir_all(&dir).unwrap();
            let per = (disk / disk_dirs).max(1);
            for i in 0..per {
                let _ = std::fs::write(dir.join(format!("p{:05}.jpg", i)), &jpg);
            }
        }
        println!("造文件耗时：{:.2} 秒（{} 字节/张）", gen.elapsed().as_secs_f64(), jpg.len());

        // 用独立连接扫，别让合成库的事务影响计时
        let lib_s = lib.to_string_lossy().to_string();
        let mut disk_app = App::open(data.join("disk-data")).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let t = Instant::now();
        match disk_app.scan_cancelable(&lib_s, &cancel, |_p| {}) {
            Ok(s) => {
                let secs = t.elapsed().as_secs_f64();
                println!(
                    "扫描完成：{} 目录 / {} 文件 · {:.2} 秒 · {:.0} 文件/秒 · {} 张图有尺寸",
                    s.dirs,
                    s.files,
                    secs,
                    s.files as f64 / secs.max(0.001),
                    s.with_dims
                );
                let q2 = model::AssetQuery {
                    dir_id: None,
                    recursive: true,
                    kinds: None,
                    text: None,
                    sort: Some("mtime_desc".into()),
                    cursor: None,
                    limit: Some(200),
                    tag: None,
                    ..Default::default()
                };
                println!(
                    "扫描后列表首页（全局）：{:.2} ms",
                    time(10, || {
                        let _ = disk_app.list(&q2).unwrap();
                    })
                );
            }
            Err(e) => println!("扫描失败：{e}"),
        }
    }

    // 清理（保留数据目录便于复查时加 --keep）
    if !args.iter().any(|a| a == "--keep") {
        let _ = std::fs::remove_dir_all(&data);
        println!("\n（数据目录已清理；加 --keep 可保留）");
    }
    let _ = Path::new("");
}
