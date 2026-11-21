//! 冒烟 CLI —— 不开界面也能验底层：扫描、列数据、出缩略图、撤销。
//!
//! 用法：
//!   am-cli scan <目录> [--data <数据目录>]
//!   am-cli list [--data <目录>] [--limit 20] [--dir <id>] [--recursive] [--text 关键词]
//!   am-cli tree [--data <目录>] [--parent <id>]
//!   am-cli thumb <asset_id> [--px 256]
//!   am-cli shell <文件路径>            ← 只测系统缩略图那一层（T10 排查用）
//!   am-cli import <文件> [--to <解压目录>] ← 双击导入（按内容嗅探类型）
//!   am-cli import <文件...> --dest <库内目录> ← 界面导入（复制进来 + 入库）
//!   am-cli undo

use am_core::{db, fsops, model, App};

/// CLI 是开发工具：库里有多根时统一用第一个（界面才需要精确定位到某个库）
fn conn(app: &App) -> &rusqlite::Connection {
    app.libs.first().map(|l| &l.conn).unwrap_or_else(|| {
        eprintln!("还没有任何素材库：先 scan 一个目录");
        std::process::exit(2)
    })
}

fn arg(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: am-cli <scan|list|tree|thumb|undo|gc> ...");
        std::process::exit(2);
    }
    let data = arg(&args, "--data").unwrap_or_else(|| am_core::default_data_dir().to_string_lossy().to_string());
    let mut app = match App::open(&data) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("打开数据库失败: {}", e);
            std::process::exit(1);
        }
    };

    match args[0].as_str() {
        "scan" => {
            let Some(root) = args.get(1).filter(|s| !s.starts_with("--")) else {
                eprintln!("缺目录参数");
                std::process::exit(2);
            };
            println!("扫描: {}  数据: {}", root, data);
            let t = std::time::Instant::now();
            match app.scan(root, |p| {
                if p.dirs % 50 == 0 {
                    println!("  {} 目录 / {} 文件 · {}", p.dirs, p.files, p.current);
                }
            }) {
                Ok(s) => println!(
                    "完成: {} 目录 / {} 文件 / {} 张图 / {} 有尺寸 / {} 错误{} · {} ms（总 {} ms）",
                    s.dirs,
                    s.files,
                    s.images,
                    s.with_dims,
                    s.errors,
                    if s.skipped > 0 { format!(" / 跳过 {} 个外部库目录", s.skipped) } else { String::new() },
                    s.elapsed_ms,
                    t.elapsed().as_millis()
                ),
                Err(e) => {
                    eprintln!("扫描失败: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "list" => {
            let q = model::AssetQuery {
                dir_id: arg(&args, "--dir").and_then(|s| s.parse().ok()),
                recursive: has(&args, "--recursive"),
                kinds: None,
                text: arg(&args, "--text"),
                sort: arg(&args, "--sort"),
                cursor: None,
                limit: arg(&args, "--limit").and_then(|s| s.parse().ok()).or(Some(20)),
                tag: arg(&args, "--tag"),
                ..Default::default()
            };
            // `--dir 1` 是库内 id，界面传的是全局 id（lib<<40|id）
            let mut q = q;
            if let Some(d) = q.dir_id {
                if d > 0 && d < (1i64 << 40) {
                    if let Some(l) = app.libs.first() {
                        q.dir_id = Some(am_core::gid(l.id, d));
                    }
                }
            }
            match app.list(&q) {
                Ok(rows) => {
                    println!("共 {} 条（本页）", rows.len());
                    for a in rows {
                        println!(
                            "#{:<6} {:<8} {:>10}  {:>5}x{:<5}  {}",
                            a.id,
                            am_core::formats::badge(&a.ext),
                            a.size,
                            a.width.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                            a.height.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                            a.name
                        );
                    }
                }
                Err(e) => eprintln!("查询失败: {}", e),
            }
        }
        "rmlib" => {
            // 从素材库移除一个库根（只清索引，不动磁盘）。多库上限 3 个，界面移除走同一条路径。
            let Some(root) = args.get(1).filter(|s| !s.starts_with("--")) else {
                eprintln!("缺库根路径");
                std::process::exit(2);
            };
            match app.remove_lib(root) {
                Ok(n) => println!("已移除 {}：清掉 {} 个目录索引（文件未动）", root, n),
                Err(e) => eprintln!("移除失败: {}", e),
            }
        }
        "libs" => match app.lib_count() {
            Ok(n) => println!("已加入 {} 个库（上限 3）", n),
            Err(e) => eprintln!("查询失败: {}", e),
        },
        "tree" => {
            let parent = arg(&args, "--parent").and_then(|s| s.parse().ok());
            match app.tree(parent) {
                Ok(nodes) => {
                    for n in nodes {
                        println!("[{:<5}] {:<40} 目录{:<4} 文件{}", n.id, n.name, n.child_cnt, n.asset_cnt);
                    }
                }
                Err(e) => eprintln!("查询失败: {}", e),
            }
        }
        "shell" => {
            // 单独探一层：系统缩略图提供者给不给这张图。
            // 排查 HEIC / PDF "为什么没出图" 时，先跑这个就知道是系统没提供者还是我们的错。
            let Some(p) = args.get(1) else {
                eprintln!("缺文件路径");
                std::process::exit(2);
            };
            let t = std::time::Instant::now();
            match am_core::shellthumb::shell_thumb(std::path::Path::new(p), 512) {
                Some((buf, w, h)) => println!(
                    "系统缩略图 OK: {}×{}  {} KB  {} ms",
                    w,
                    h,
                    buf.len() / 1024,
                    t.elapsed().as_millis()
                ),
                None => println!("系统没有给缩略图（{} ms）", t.elapsed().as_millis()),
            }
        }
        "thumb" => {
            let Some(gid) = args.get(1).and_then(|s| s.parse::<i64>().ok()) else {
                eprintln!("缺 asset_id（用 `list` 打印的 # 后面那个数，是全局 id）");
                std::process::exit(2);
            };
            let px = arg(&args, "--px").and_then(|s| s.parse().ok()).unwrap_or(256u32);
            /* 传进来的是**全局 id**（`list` 打的就是它，形如 1099511627777 = lib<<40 | local）。
               以前直接把 gid 丢进 `libs[0]` 里查 —— 一行也查不到，于是永远「资产不存在」，
               排查缩略图时被这条假失败骗过。这里按 gid 拆库，找不到那个库才退回"当本地 id 用"。 */
            let (lib_id, local_id) = am_core::ids::ungid(gid);
            let (conn, aid) = match app.libs.iter().find(|l| l.id == lib_id) {
                Some(l) => (&l.conn, local_id),
                None => match app.libs.first() {
                    Some(l) => (&l.conn, gid),
                    None => {
                        eprintln!("还没有任何素材库：先 scan 一个目录");
                        std::process::exit(2);
                    }
                },
            };
            let t = std::time::Instant::now();
            match am_core::thumb::ensure_for_asset(conn, &app.thumbs, aid, px) {
                Ok(Some(o)) => println!(
                    "OK {}  {}x{}  {} B  q={}  {} ms\n{}",
                    o.key,
                    o.width,
                    o.height,
                    o.bytes,
                    o.quality,
                    t.elapsed().as_millis(),
                    o.path.display()
                ),
                Ok(None) => println!("这张图出不来（状态已记录）"),
                Err(e) => eprintln!("失败: {}", e),
            }
        }
        "gc" => {
            let (cur, freed) = am_core::thumb::gc(conn(&app), &app.thumbs, 2 * 1024 * 1024 * 1024, 1400 * 1024 * 1024)
                .unwrap_or((0, 0));
            println!("缓存 {} MB，释放 {} MB", cur / 1048576, freed / 1048576);
        }
        "ops" => {
            let ops = fsops::FsOps::new(conn(&app));
            match ops.undo_last() {
                Ok(msg) => println!("{}", msg),
                Err(e) => println!("{}", e),
            }
        }
        /* 双击导入的无界面验收入口：嗅探 → 分派（用户数据 / 快照 / 资源包）。
           资源包会真的解压到 `--to <目录>`（不传就落在数据目录下 import-<时间戳>），
           解完顺便扫一遍，方便确认"文件数对不对得上"。 */
        "import" => {
            /* 两种用法，用 `--dest` 区分：
               · `am-cli import <导出文件>`                —— 双击导入那条路（按内容嗅探类型）
               · `am-cli import <文件...> --dest <目录>`   —— 界面导入那条路（复制进库内目录） */
            let dest_arg = arg(&args, "--dest");
            let mut files: Vec<std::path::PathBuf> = Vec::new();
            let mut i = 1;
            while i < args.len() {
                let a = &args[i];
                if a == "--data" || a == "--dest" || a == "--to" {
                    i += 2;
                    continue;
                }
                if a.starts_with("--") {
                    i += 1;
                    continue;
                }
                files.push(std::path::PathBuf::from(a));
                i += 1;
            }
            if files.is_empty() {
                eprintln!(
                    "用法:\n  am-cli import <导出文件> [--data <数据目录>]\n  am-cli import <文件...> --dest <库内目录> [--data <数据目录>]"
                );
                std::process::exit(2);
            }

            if let Some(dest) = dest_arg {
                let target = std::path::PathBuf::from(&dest);
                let t = std::time::Instant::now();
                match app.import_files(&files, &target) {
                    Ok(r) => {
                        println!(
                            "导入完成：复制 {} 个 / 跳过 {} 个 / 新入库 {} 张（{} ms）",
                            r.copied,
                            r.skipped,
                            r.added,
                            t.elapsed().as_millis()
                        );
                        for (src, why) in &r.failed {
                            println!("  失败：{src} —— {why}");
                        }
                        if !r.failed.is_empty() {
                            std::process::exit(1);
                        }
                    }
                    Err(e) => {
                        eprintln!("导入失败：{e}");
                        std::process::exit(1);
                    }
                }
            } else {
                if files.len() > 1 {
                    eprintln!("双击导入一次只能给一个文件；多个文件请用 --dest <库内目录>");
                    std::process::exit(2);
                }
                let src = files[0].clone();
                let kind = match am_core::file_kind::sniff(&src) {
                    Ok(k) => k,
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(1);
                    }
                };
                println!("识别为：{}", kind.label());
                match kind {
                    am_core::file_kind::ImportKind::UserData => {
                        let text = match std::fs::read_to_string(&src) {
                            Ok(t) => t,
                            Err(e) => {
                                eprintln!("读不了 {}：{e}", src.display());
                                std::process::exit(1);
                            }
                        };
                        match app.import_userdata(&text) {
                            Ok(r) => println!(
                                "导入完成：新增 {} 个库 / 回填 {} 张 / 找不到 {} 条",
                                r.libs_added, r.assets_updated, r.assets_unknown
                            ),
                            Err(e) => {
                                eprintln!("导入失败：{e}");
                                std::process::exit(1);
                            }
                        }
                    }
                    am_core::file_kind::ImportKind::Snapshot => {
                        let text = match std::fs::read_to_string(&src) {
                            Ok(t) => t,
                            Err(e) => {
                                eprintln!("读不了 {}：{e}", src.display());
                                std::process::exit(1);
                            }
                        };
                        match app.snapshot_import(&text) {
                            Ok((id, name, added, skipped)) => println!(
                                "导入完成：快照「{name}」（id={id}）新增 {added} 条，跳过 {skipped} 条"
                            ),
                            Err(e) => {
                                eprintln!("导入失败：{e}");
                                std::process::exit(1);
                            }
                        }
                    }
                    am_core::file_kind::ImportKind::Package => {
                        let dest = match arg(&args, "--to") {
                            Some(d) => std::path::PathBuf::from(d),
                            None => std::path::PathBuf::from(&data)
                                .join(format!("import-{}", db::now_ts())),
                        };
                        let t = std::time::Instant::now();
                        let (n, manifest) = match am_core::snapshot::unzip_package(&src, &dest) {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!("解压失败：{e}");
                                std::process::exit(1);
                            }
                        };
                        println!(
                            "解压完成：{} 个文件 → {}（{} ms）",
                            n,
                            dest.display(),
                            t.elapsed().as_millis()
                        );
                        /* 路径统一成 Windows 口径：清单回填是按 `dirs.path` 原样比对的 */
                        let dest_str = dest.to_string_lossy().replace('/', "\\");
                        match app.scan(&dest_str, |_| {}) {
                            Ok(s) => println!(
                                "扫描完成：{} 目录 / {} 文件 / {} 张图 / {} 错误",
                                s.dirs, s.files, s.images, s.errors
                            ),
                            Err(e) => {
                                eprintln!("扫描失败：{e}");
                                std::process::exit(1);
                            }
                        }
                        if let Some(m) = manifest {
                            match app.import_package_manifest(&m, &dest) {
                                Ok(r) => println!(
                                    "清单还原：评分 {} 张 / 标签 {} 张",
                                    r.rated, r.tagged
                                ),
                                Err(e) => eprintln!("清单还原失败（文件已经解好了）：{e}"),
                            }
                        }
                    }
                }
            }
        }
        "rebuild-fts" => match db::rebuild_fts(conn(&app)) {
            Ok(n) => println!("重建完成，{} 条", n),
            Err(e) => eprintln!("失败: {}", e),
        },
        // 冒烟：把原文件放进剪贴板，PowerShell 里 `Get-Clipboard -Format FileDropList` 能验到
        "copy" => {
            let mut list: Vec<std::path::PathBuf> = Vec::new();
            let mut i = 1;
            while i < args.len() {
                if args[i] == "--data" {
                    i += 2;
                    continue;
                }
                if args[i].starts_with("--") {
                    i += 1;
                    continue;
                }
                list.push(std::path::PathBuf::from(&args[i]));
                i += 1;
            }
            match fsops::copy_files_to_clipboard(&list) {
                Ok(()) => println!("已把 {} 个原文件放进剪贴板", list.len()),
                Err(e) => eprintln!("失败: {}", e),
            }
        }
        // 冒烟：用默认程序打开原文件（这一条会真的弹窗，谨慎）
        "open" => match args.get(1) {
            Some(p) => match fsops::open_with_default(std::path::Path::new(p)) {
                Ok(()) => println!("已交给系统默认程序：{}", p),
                Err(e) => eprintln!("失败: {}", e),
            },
            None => eprintln!("用法: am-cli open <原文件路径>"),
        },
        // 冒烟：整库导成普通 zip —— 不起界面就能验证分包与包内结构
        // 用法：am-cli zips <库根> <输出目录> [--max-bytes N] [--stem 前缀]
        "zips" => {
            let Some(root) = args.get(1).filter(|s| !s.starts_with("--")) else {
                eprintln!("用法: am-cli zips <库根> <输出目录> [--max-bytes N] [--stem 前缀] [--data DIR]");
                std::process::exit(2);
            };
            let Some(out) = args.get(2).filter(|s| !s.starts_with("--")) else {
                eprintln!("缺输出目录");
                std::process::exit(2);
            };
            let max_bytes = arg(&args, "--max-bytes")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(3_600_000_000);
            let stem = arg(&args, "--stem").unwrap_or_else(|| {
                format!("{}-cli", am_core::snapshot::safe_file_stem(root))
            });
            let out_dir = std::path::PathBuf::from(out);
            std::fs::create_dir_all(&out_dir).ok();
            /* 这个库的索引文件：和 am_lib_files 一样的解析（0.3.9 起在库根 .AssetManager 里） */
            let want = root.replace('/', "\\");
            let index_db = am_core::reg::libs(&app.reg).unwrap_or_default().into_iter().find_map(|r| {
                if !r.root.replace('/', "\\").eq_ignore_ascii_case(&want) {
                    return None;
                }
                let rel = if r.file.is_empty() {
                    format!("libs/lib-{}.db", r.id)
                } else {
                    r.file.clone()
                };
                Some(std::path::PathBuf::from(&data).join(rel))
            });
            match app.export_library_zip(
                root,
                &out_dir,
                &stem,
                max_bytes,
                60_000,
                index_db.as_deref(),
            ) {
                Ok(r) => {
                    println!(
                        "完成: {} 张 / {} 片 / 跳过 {} / 超大 {}",
                        r.files,
                        r.parts.len(),
                        r.skipped,
                        r.oversized
                    );
                    for p in &r.parts {
                        println!("  {}  {} 张  {} 字节", p.name, p.files, p.bytes);
                    }
                }
                Err(e) => {
                    eprintln!("失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!("未知命令: {}", other);
            std::process::exit(2);
        }
    }
}
