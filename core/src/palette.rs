//! 主色板 + 色桶（T24）。
//!
//! 两个产物，一次算完（图已经在内存里，多花不到 1ms）：
//! - `palette`：5 个主色，`rrggbb,rrggbb,…`，详情面板的色板 / 复制色值用；
//! - `color`：这张图"最像哪个颜色"，按颜色筛素材走这一列（枚举，一次索引查找）。
//!
//! 为什么不用聚类：百万级库里每张图跑 k-means 是拿帧预算换精度，
//! 而用户看色板只是"这张图什么调子"。4 bit/通道的直方图 + 去近色就够了，
//! 纯整数运算、零依赖（不引 `palette` crate，省体积也省编译时间）。

/// 色桶白名单。**前端 `COLOR_FILTERS` 必须和这里逐字一致**。
pub const BUCKETS: &[&str] = &[
    "red", "orange", "amber", "yellow", "green", "teal", "blue", "indigo", "violet", "pink",
    "brown", "black", "gray", "white",
];

pub struct Palette {
    /// 最多 5 个 `rrggbb`
    pub hexes: Vec<String>,
    /// 色桶（空串 = 图太小/读不到像素）
    pub bucket: String,
}

impl Palette {
    pub fn joined(&self) -> String {
        self.hexes.join(",")
    }
}

/// 从解码后的 RGBA 像素算主色板。`rgba` 长度不足时返回空结果（不做猜测）。
pub fn compute(rgba: &[u8], w: u32, h: u32) -> Palette {
    let pixels = (w as usize) * (h as usize);
    if pixels == 0 || rgba.len() < pixels * 4 {
        return Palette { hexes: Vec::new(), bucket: String::new() };
    }
    // 最多采 4096 个点：再多样本对结果没有影响，只是白花时间
    let step = ((pixels as f64 / 4096.0).sqrt().ceil() as usize).max(1);

    let mut bins = [0u32; 4096];
    let mut bucket_w: Vec<(u32, u32)> = vec![(0, 0); BUCKETS.len()]; // (权重, 像素数)
    let mut used = 0u32;
    let mut y = 0usize;
    while y < h as usize {
        let mut x = 0usize;
        while x < w as usize {
            let i = (y * w as usize + x) * 4;
            let a = rgba[i + 3];
            if a >= 16 {
                let (r, g, b) = (rgba[i], rgba[i + 1], rgba[i + 2]);
                bins[bin_index(r, g, b)] += 1;
                let (idx, sat) = bucket_of(r, g, b);
                // 饱和的颜色更能代表"这张图什么色"，灰调只占一票
                let weight = 1 + (sat as u32) * 2 / 255;
                bucket_w[idx].0 += weight;
                bucket_w[idx].1 += 1;
                used += 1;
            }
            x += step;
        }
        y += step;
    }
    if used == 0 {
        return Palette { hexes: Vec::new(), bucket: String::new() };
    }

    // 主色：按出现频次取前 5 个，彼此中心距 < 32 的算同一个色（否则 5 个格子全是近似的红）
    let mut order: Vec<usize> = (0..4096).filter(|i| bins[*i] > 0).collect();
    order.sort_by(|a, b| bins[*b].cmp(&bins[*a]));
    let mut hexes: Vec<String> = Vec::with_capacity(5);
    let mut centers: Vec<(i32, i32, i32)> = Vec::with_capacity(5);
    for idx in order {
        if hexes.len() >= 5 {
            break;
        }
        let (r, g, b) = bin_center(idx);
        if centers.iter().any(|c| dist2(*c, (r, g, b)) < 32 * 32) {
            continue;
        }
        centers.push((r, g, b));
        hexes.push(format!("{:02x}{:02x}{:02x}", r, g, b));
    }

    // 色桶：权重最高的那个。灰/黑/白只有"整张图基本都是它"才算（见 bucket_of 的权重）
    let mut best = 0usize;
    for i in 1..bucket_w.len() {
        if bucket_w[i].0 > bucket_w[best].0 {
            best = i;
        }
    }
    let bucket = if used > 0 && bucket_w[best].0 * 100 / (used * 3).max(1) >= 8 {
        BUCKETS[best].to_string()
    } else {
        String::new()
    };

    Palette { hexes, bucket }
}

fn bin_index(r: u8, g: u8, b: u8) -> usize {
    ((r as usize >> 4) << 8) | ((g as usize >> 4) << 4) | (b as usize >> 4)
}

fn bin_center(i: usize) -> (i32, i32, i32) {
    let r = ((i >> 8) & 0xF) as i32 * 17;
    let g = ((i >> 4) & 0xF) as i32 * 17;
    let b = (i & 0xF) as i32 * 17;
    (r, g, b)
}

fn dist2(a: (i32, i32, i32), b: (i32, i32, i32)) -> i32 {
    let dr = a.0 - b.0;
    let dg = a.1 - b.1;
    let db = a.2 - b.2;
    dr * dr + dg * dg + db * db
}

/// 单像素 → (色桶下标, 饱和度 0–255)
fn bucket_of(r: u8, g: u8, b: u8) -> (usize, u8) {
    let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d <= f32::EPSILON {
        0.0
    } else if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let sat = (s * 255.0) as u8;

    // 无彩色：靠亮度分黑 / 灰 / 白
    if s < 0.12 {
        let idx = if l < 0.16 {
            11 // black
        } else if l > 0.90 {
            13 // white
        } else {
            12 // gray
        };
        return (idx, sat);
    }
    let mut hue = if d <= f32::EPSILON {
        0.0
    } else if max == rf {
        60.0 * (((gf - bf) / d) % 6.0)
    } else if max == gf {
        60.0 * ((bf - rf) / d + 2.0)
    } else {
        60.0 * ((rf - gf) / d + 4.0)
    };
    if hue < 0.0 {
        hue += 360.0;
    }
    // 暗而偏橙的按棕算（否则深咖啡色会被说成"橙"）
    if (10.0..50.0).contains(&hue) && l < 0.42 {
        return (10, sat);
    }
    let idx = match hue {
        h if h < 14.0 => 0,   // red
        h if h < 40.0 => 1,   // orange
        h if h < 66.0 => 2,   // amber
        h if h < 82.0 => 3,   // yellow
        h if h < 160.0 => 4,  // green
        h if h < 196.0 => 5,  // teal
        h if h < 234.0 => 6,  // blue
        h if h < 262.0 => 7,  // indigo
        h if h < 290.0 => 8,  // violet
        h if h < 330.0 => 9,  // pink
        _ => 0,               // 回到红
    };
    (idx, sat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8, w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            v.extend_from_slice(&[r, g, b, 255]);
        }
        v
    }

    #[test]
    fn buckets_and_palette() {
        let p = compute(&solid(220, 30, 40, 40, 40), 40, 40);
        assert_eq!(p.bucket, "red");
        assert!(p.hexes.len() >= 1);
        assert!(p.hexes[0].starts_with("dd") || p.hexes[0].starts_with("d"), "{:?}", p.hexes);

        assert_eq!(compute(&solid(0, 0, 0, 8, 8), 8, 8).bucket, "black");
        assert_eq!(compute(&solid(250, 250, 250, 8, 8), 8, 8).bucket, "white");
        assert_eq!(compute(&solid(128, 128, 128, 8, 8), 8, 8).bucket, "gray");
        assert_eq!(compute(&solid(20, 90, 230, 8, 8), 8, 8).bucket, "blue");
        assert_eq!(compute(&solid(30, 160, 80, 8, 8), 8, 8).bucket, "green");
    }

    /// 半透明 / 全透明像素不该被算进去；空图不该崩
    #[test]
    fn ignores_transparent_and_empty() {
        let mut v = solid(255, 0, 0, 4, 4);
        for i in (0..v.len()).step_by(4) {
            v[i + 3] = 0;
        }
        let p = compute(&v, 4, 4);
        assert!(p.hexes.is_empty());
        assert_eq!(p.bucket, "");
        assert!(compute(&[], 0, 0).hexes.is_empty());
    }
}
