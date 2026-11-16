//! 缩略图生成耗时压测：**真实源文件**（几十 MB 到上百 MB 的 PNG / JPG）→ WebP 缩略图。
//!
//! 和 `am-bench` 的分工：
//! - `am-bench` 量的是**索引与查询**（扫描只读文件头，跟源文件大小基本无关）；
//! - 这个量的是**出图**：从磁盘读整张原图 → 解码 → 缩放 → WebP 编码。
//!   用户真正的等待感（滚到一屏新图时那一下）就来自这里。
//!
//! 用法：
//!   am-thumbbench                # 默认 5 档源文件 × 3 个档位
//!   am-thumbbench --keep         # 保留生成的源文件（复查用）
//!   am-thumbbench --data E:\tmp\am-thumbbench

use std::path::{Path, PathBuf};
use std::time::Instant;

use am_core::thumb::ThumbStore;

/// 简易 LCG：只需要"看起来像随机噪点"，不要求统计质量。
struct Rnd(u64);
impl Rnd {
    fn next(&mut self) -> u8 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u8
    }
}

fn make_noise(w: u32, h: u32) -> image::RgbImage {
    let mut r = Rnd(20261116);
    image::RgbImage::from_fn(w, h, |x, y| {
        // 加一点大块渐变，避免纯噪声让 PNG 压缩率失真（真实照片有低频结构）
        let base = ((x / 64 + y / 64) % 12) as u8 * 8;
        let n = r.next() % 26;
        image::Rgb([base + n, base.wrapping_add(n * 2), base.wrapping_add(n / 2)])
    })
}

fn make_photo(w: u32, h: u32) -> image::RgbImage {
    // 平滑渐变 + 少量噪点：更像真实照片，PNG 压得动、JPEG 也压得动
    let mut r = Rnd(7);
    image::RgbImage::from_fn(w, h, |x, y| {
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        let v = (fx * 0.6 + fy * 0.4).sin() * 60.0 + 140.0;
        let n = (r.next() % 9) as f32 - 4.0;
        image::Rgb([(v + n) as u8, (v * 0.8 + n) as u8, (v * 0.6 + n) as u8])
    })
}

fn human(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1048576.0)
    } else {
        format!("{:.0} KB", n as f64 / 1024.0)
    }
}

fn build(dir: &Path, name: &str, w: u32, h: u32, kind: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.exists() {
        return Some(p);
    }
    let img = if kind == "noise" { make_noise(w, h) } else { make_photo(w, h) };
    let t = Instant::now();
    let ok = if name.ends_with(".png") {
        img.save_with_format(&p, image::ImageFormat::Png).is_ok()
    } else {
        // JPEG 质量：用 image 的默认编码器（≈ q80），够接近真实照片体积
        img.save_with_format(&p, image::ImageFormat::Jpeg).is_ok()
    };
    if !ok {
        eprintln!("生成失败：{}", p.display());
        return None;
    }
    println!(
        "  造好 {:<22} {}×{}  {:<9} 用时 {:.1}s",
        name,
        w,
        h,
        human(std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)),
        t.elapsed().as_secs_f64()
    );
    Some(p)
}

/// 真实素材模式：在一个真实目录里抓 N 张图，按 `jobs` 路并发出图，量「一屏要多久」。
///
/// 这是最接近用户感受的数字：合成噪声图对 WebP 编码极不友好（压缩率低、编码慢），
/// 真实设计稿 / 照片要快得多，所以真实目录的数字才是可信的那一个。
fn bench_real(src: &Path, px: u32, jobs: usize, limit: usize, keep: bool) {
    fn walk(d: &Path, out: &mut Vec<PathBuf>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut ents: Vec<_> = rd.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            if out.len() >= limit {
                return;
            }
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                walk(&p, out, limit);
                continue;
            }
            let ext = p
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if matches!(
                ext.as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "bmp" | "tif" | "tiff" | "avif"
            ) {
                out.push(p);
            }
        }
    }

    let mut files = Vec::new();
    walk(src, &mut files, limit);
    if files.is_empty() {
        println!("{} 下没找到图片", src.display());
        return;
    }
    println!(
        "== 真实素材出图：{} 张，{} 路并发，{} 档 ==",
        files.len(),
        jobs,
        px
    );

    let store_root = std::env::temp_dir().join(format!("am-thumb-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&store_root);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let total_ms = std::sync::Mutex::new(Vec::<f64>::new());
    let t0 = Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..jobs {
            sc.spawn(|| {
                let store = ThumbStore::new(store_root.clone());
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= files.len() {
                        break;
                    }
                    let t = Instant::now();
                    match store.ensure(&files[i], px) {
                        Ok(_) => {
                            total_ms
                                .lock()
                                .unwrap()
                                .push(t.elapsed().as_secs_f64() * 1000.0)
                        }
                        Err(_) => {}
                    }
                }
            });
        }
    });
    let wall = t0.elapsed().as_secs_f64();
    let per = total_ms.lock().unwrap().clone();
    let avg = if per.is_empty() { 0.0 } else { per.iter().sum::<f64>() / per.len() as f64 };
    let mut sorted = per.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p90 = sorted
        .get((sorted.len() as f64 * 0.9) as usize)
        .copied()
        .unwrap_or(avg);
    println!(
        "总耗时 {:.2}s（{} 张，平均 {:.0}ms，P90 {:.0}ms，吞吐 {:.1} 张/秒）",
        wall,
        per.len(),
        avg,
        p90,
        per.len() as f64 / wall.max(0.001)
    );
    if !keep {
        let _ = std::fs::remove_dir_all(&store_root);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keep = args.iter().any(|a| a == "--keep");
    let arg_of = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(dir) = arg_of("--real") {
        let px = arg_of("--px").and_then(|v| v.parse().ok()).unwrap_or(384);
        let jobs = arg_of("--jobs").and_then(|v| v.parse().ok()).unwrap_or(10);
        let limit = arg_of("--limit").and_then(|v| v.parse().ok()).unwrap_or(40);
        bench_real(Path::new(&dir), px, jobs.max(1), limit, keep);
        return;
    }

    let data = args
        .iter()
        .position(|a| a == "--data")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("am-thumbbench"));

    let src = data.join("src");
    std::fs::create_dir_all(&src).unwrap();
    println!("== 缩略图生成压测（真实源文件 → WebP）==");
    println!("源文件目录：{}\n", src.display());

    // 覆盖真实素材库里的常见体量：手机/相机/截图/合成大图
    let files = vec![
        ("png-100MB.png", 6000, 6000, "noise"),
        ("png-24MP.png", 6000, 4000, "photo"),
        ("jpg-24MP.jpg", 6000, 4000, "noise"),
        ("jpg-12MP.jpg", 4000, 3000, "photo"),
        ("jpg-2MP.jpg", 1600, 1200, "noise"),
    ];

    println!("-- 准备源文件 --");
    let mut made: Vec<(String, PathBuf, u64, u32, u32)> = Vec::new();
    for (name, w, h, kind) in &files {
        if let Some(p) = build(&src, name, *w, *h, kind) {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            made.push((name.to_string(), p, size, *w, *h));
        }
    }

    println!("\n-- 出图耗时（每次都是冷生成，缓存目录独立）--");
    println!(
        "{:<18} {:>9} {:>7} {:>6} {:>10} {:>9} {:>8}",
        "源文件", "体积", "像素", "档位", "耗时", "输出", "读速"
    );

    let mut total_384 = 0.0;
    for (name, path, size, w, h) in &made {
        for px in [256u32, 384, 512, 1024] {
            let root = data.join(format!("cache-{}-{}-{}", name.replace('.', "_"), px, std::process::id()));
            let store = ThumbStore::new(root.clone());
            let t = Instant::now();
            match store.ensure(path, px) {
                Ok(out) => {
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    if px == 384 {
                        total_384 += ms;
                    }
                    println!(
                        "{:<18} {:>9} {:>7} {:>6} {:>8.0}ms {:>7} {:>6.0}MB/s",
                        name,
                        human(*size),
                        format!("{}MP", (w * h) / 1_000_000),
                        px,
                        ms,
                        human(out.bytes as u64),
                        (*size as f64 / 1048576.0) / (ms / 1000.0)
                    );
                }
                Err(e) => println!("{:<18} {:>9} {:>7} {:>6}  失败：{}", name, human(*size), "", px, e),
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    println!("\n-- 一屏的感受（按 384 档算，40 格）--");
    println!(
        "平均单张 {:.0}ms → 40 格串行 {:.1}s；4 路并发（解码闸门）约 {:.1}s",
        total_384 / made.len().max(1) as f64,
        total_384 / made.len().max(1) as f64 * 40.0 / 1000.0,
        total_384 / made.len().max(1) as f64 * 10.0 / 1000.0
    );

    if !keep {
        let _ = std::fs::remove_dir_all(&data);
        println!("\n（源文件已清理；加 --keep 保留复查）");
    }
}
