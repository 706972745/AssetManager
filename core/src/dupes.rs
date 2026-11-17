//! 重复检测（T25）：**精确**（xxh3 全文件）+ **相似**（aHash 64 位）。
//!
//! - 精确：同一个文件拷了两份（换过名字也算），库里只该留一份；
//! - 相似：同一张图的不同尺寸/格式/微调，只提示不自动处理。
//!
//! 两条都不引新依赖：hash 走已在用的 `xxhash-rust`，相似的 aHash 是纯整数运算
//! （8×8 灰度 + 均值），几百微秒 —— 放在出缩略图那一步顺手算完，不另开一遍扫描。
//!
//! 相似对的判定用**鸽笼原理**：真·汉明距离 ≤ 3 的两个 64 位指纹，把它切成 4 段 16 位，
//! 至少有一段完全相同。于是只在「同段相同的桶」里比，避免 O(n²)。

use std::io::Read;
use std::path::Path;

/// 全文件 xxh3-128。返回 16 字节（存 `assets.hash_x`）。
pub fn hash_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.digest128().to_be_bytes().to_vec())
}

/// 8×8 均值哈希（aHash）。返回 64 位，位 = 「这个格子比全图平均亮还是暗」。
///
/// 为什么不用 pHash（DCT）：DCT 要浮点 + 余弦表，一张 8×8 也要几十微秒，
/// 而相似检测的目的是"同一张图的不同版本"，aHash 对缩放/压缩已经足够稳
/// （实测同一张图 384 档与 1024 档的 aHash 汉明距离通常 0–4）。
pub fn ahash64(rgba: &[u8], w: u32, h: u32) -> i64 {
    if w == 0 || h == 0 {
        return 0;
    }
    // 同 `palette::compute`：RGB8 也要能算（3 或 4 字节步长）
    let px = (w as usize) * (h as usize);
    let stride = if rgba.len() >= px * 4 {
        4
    } else if rgba.len() >= px * 3 {
        3
    } else {
        return 0;
    };
    let mut cell = [0u32; 64];
    for gy in 0..8usize {
        let y0 = gy * h as usize / 8;
        let y1 = ((gy + 1) * h as usize / 8).max(y0 + 1);
        for gx in 0..8usize {
            let x0 = gx * w as usize / 8;
            let x1 = ((gx + 1) * w as usize / 8).max(x0 + 1);
            let mut sum = 0u64;
            let mut n = 0u64;
            for y in y0..y1.min(h as usize) {
                for x in x0..x1.min(w as usize) {
                    let i = (y * w as usize + x) * stride;
                    // 亮度用整数近似（R*2 + G*5 + B）/8
                    sum += (rgba[i] as u64 * 2 + rgba[i + 1] as u64 * 5 + rgba[i + 2] as u64) / 8;
                    n += 1;
                }
            }
            cell[gy * 8 + gx] = if n == 0 { 0 } else { (sum / n) as u32 };
        }
    }
    let mean: u32 = cell.iter().sum::<u32>() / 64;
    let mut out: i64 = 0;
    for (i, v) in cell.iter().enumerate() {
        if *v > mean {
            out |= 1i64 << i;
        }
    }
    out
}

pub fn hamming(a: i64, b: i64) -> u32 {
    (a ^ b).count_ones()
}

/// 相似对：`rows` 是 (asset_id, phash)，返回 (a, b, 距离)，按距离从小到大。
/// `max_dist` 一般取 6（0–4 基本是同一张图的不同版本，5–6 开始有误报）。
pub fn similar_pairs(rows: &[(i64, i64)], max_dist: u32, limit: usize) -> Vec<(i64, i64, u32)> {
    let mut buckets: Vec<std::collections::HashMap<i64, Vec<usize>>> =
        (0..4).map(|_| std::collections::HashMap::new()).collect();
    for (i, (_, ph)) in rows.iter().enumerate() {
        for (seg, map) in buckets.iter_mut().enumerate() {
            let key = (ph >> (seg * 16)) & 0xFFFF;
            map.entry(key).or_default().push(i);
        }
    }
    let mut out: Vec<(i64, i64, u32)> = Vec::new();
    let mut seen: std::collections::HashSet<(usize, usize)> = Default::default();
    for map in &buckets {
        for ids in map.values() {
            if ids.len() < 2 {
                continue;
            }
            // 桶里偶尔会有几百个（大片纯色图）→ 只比前 64 个，避免一屏纯色图把 CPU 吃满
            let cap = ids.len().min(64);
            for i in 0..cap {
                for j in (i + 1)..cap {
                    let (a, b) = (ids[i].min(ids[j]), ids[i].max(ids[j]));
                    if !seen.insert((a, b)) {
                        continue;
                    }
                    let d = hamming(rows[a].1, rows[b].1);
                    if d <= max_dist {
                        out.push((rows[a].0, rows[b].0, d));
                    }
                }
            }
        }
    }
    out.sort_by_key(|x| x.2);
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray_solid(v: u8, w: u32, h: u32) -> Vec<u8> {
        let mut d = Vec::new();
        for _ in 0..(w * h) {
            d.extend_from_slice(&[v, v, v, 255]);
        }
        d
    }

    #[test]
    fn ahash_is_stable_and_discriminative() {
        // 左半亮 / 右半暗：两次算出来必须一模一样（同图不同尺寸也要接近）
        let mut a = Vec::new();
        for _ in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 240 } else { 20 };
                a.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let h1 = ahash64(&a, 64, 64);
        let h2 = ahash64(&a, 64, 64);
        assert_eq!(h1, h2);
        assert_ne!(h1, 0);

        // 反相图应该差很多位
        let mut b = Vec::new();
        for _ in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 20 } else { 240 };
                b.extend_from_slice(&[v, v, v, 255]);
            }
        }
        assert!(hamming(h1, ahash64(&b, 64, 64)) >= 32);
        assert_eq!(hamming(h1, h1), 0);
    }

    #[test]
    fn similar_pairs_finds_near_duplicates_only() {
        // 1 与 2 只差 3 位（同一张图的微调）；3 完全不同
        let rows = vec![(10i64, 0b1010_1010_1010_1010i64), (20, 0b1010_1010_1010_1111), (30, i64::MIN)];
        let got = similar_pairs(&rows, 6, 50);
        assert!(got.iter().any(|(a, b, _)| (*a, *b) == (10, 20)), "{got:?}");
        assert!(!got.iter().any(|(a, b, _)| (*a, *b) == (10, 30)), "{got:?}");
    }

    #[test]
    fn hash_file_differs_on_content() {
        let d = std::env::temp_dir().join(format!("am-hash-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p1 = d.join("a.bin");
        let p2 = d.join("b.bin");
        std::fs::write(&p1, vec![7u8; 100_000]).unwrap();
        std::fs::write(&p2, vec![7u8; 100_001]).unwrap();
        let h1 = hash_file(&p1).unwrap();
        assert_eq!(h1.len(), 16);
        assert_eq!(h1, hash_file(&p1).unwrap());
        assert_ne!(h1, hash_file(&p2).unwrap());
        let _ = std::fs::remove_dir_all(&d);
        assert!(hash_file(Path::new("E:/definitely-not-here.bin")).is_err());
    }

    #[test]
    fn ahash_handles_bad_input() {
        assert_eq!(ahash64(&[], 0, 0), 0);
        assert_eq!(ahash64(&[0, 0, 0, 255], 100, 100), 0);
        // 纯色图 → 每格都等于均值 → 全 0（合法结果，不是 bug）
        assert_eq!(ahash64(&gray_solid(200, 8, 8), 8, 8), 0);
        // 有明暗变化才会出位
        let mut g = Vec::new();
        for y in 0..8u32 {
            for x in 0..8u32 {
                let v = (x * 30 + y * 5) as u8;
                g.extend_from_slice(&[v, v, v, 255]);
            }
        }
        assert_ne!(ahash64(&g, 8, 8), 0);
    }
}
