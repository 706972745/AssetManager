//! 数据文件夹（索引 + 缩略图缓存）放在哪里 —— 可配置，语义照 FontManager 的「字体库文件夹」。
//!
//! ## 为什么要「指针文件」而不是直接改注册表/环境变量
//!
//! 索引库（`index.db`）与缩略图缓存（`thumbs`）默认在**安装目录下的 `data\`**
//! （0.5.7 起；写不进去才回退 `%LOCALAPPDATA%\AssetManager`，见
//! [`crate::default_data_dir`]）。
//! 想把它们放到 D 盘（或者跟着一个便携目录走）时，必须有个地方记住「自定义目录在哪」——
//! 而这个地方**只能在默认目录里**，否则启动时不知道该去读哪个配置（先有鸡还是先有蛋）。
//!
//! 所以：`<默认目录>\data-root.json` 是唯一的事实来源，内容形如
//! `{ "enabled": true, "current": "D:\\素材库数据", "dirs": ["D:\\素材库数据", "E:\\备份"] }`。
//!
//! ## 三条纪律
//!
//! 1. **自定义目录不可用就回退默认**，不报错、不让软件打不开 —— 移动硬盘拔掉、
//!    网络盘断线都可能发生，用户要的是「软件能开」，不是「弹一句我们很抱歉」。
//!    回退时只在 stderr 记一行，界面上数据文件夹那一项会显示真实生效的路径，用户看得出来。
//! 2. **切换只在下次启动生效**：SQLite 连接在启动时就绑定了文件路径，
//!    运行中换目录要么关连接重开（所有正在跑的扫描/移动线程都会跟着崩），
//!    要么重启。重启最稳，而且用户能明确感知「换了」。
//! 3. **不自动搬数据**。切换时提供「把现有索引复制过去」，但**缩略图缓存不复制**
//!    （几万张图好几 GB，复制比重新生成还慢）。索引里记着 thumb_key，缓存没了会按需重建。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 最多记几个数据文件夹（与 FontManager 的库地址上限一致）
pub const MAX_DIRS: usize = 3;

const CONFIG_NAME: &str = "data-root.json";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DataRootConfig {
    /// 是否用自定义文件夹（false = 默认目录）
    pub enabled: bool,
    /// 当前生效的自定义文件夹
    pub current: Option<String>,
    /// 记过的文件夹清单（含 current），供设置面板列出
    pub dirs: Vec<String>,
}

/// 指针文件永远在默认目录里（见模块注释第 1 段）。
pub fn config_path(default_dir: &Path) -> PathBuf {
    default_dir.join(CONFIG_NAME)
}

/// 读配置。文件不存在 / 坏掉 / 字段类型不对 → 一律当成「用默认目录」，绝不抛错。
pub fn load(default_dir: &Path) -> DataRootConfig {
    let raw = match std::fs::read_to_string(config_path(default_dir)) {
        Ok(s) => s,
        Err(_) => return DataRootConfig::default(),
    };
    let mut cfg: DataRootConfig = serde_json::from_str(&raw).unwrap_or_default();
    // 清单去重 + 截断到上限：手改过配置文件也不该让界面列出十条
    let mut seen = std::collections::HashSet::new();
    cfg.dirs = cfg
        .dirs
        .into_iter()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty() && seen.insert(d.to_lowercase()))
        .collect();
    cfg.dirs.truncate(MAX_DIRS);
    // current 不在清单里 → 补进去（否则界面上「当前」那一行无处安放）
    if let Some(cur) = cfg.current.clone() {
        if !cfg.dirs.iter().any(|d| d.eq_ignore_ascii_case(&cur)) {
            cfg.dirs.insert(0, cur);
            cfg.dirs.truncate(MAX_DIRS);
        }
    }
    cfg
}

pub fn save(default_dir: &Path, cfg: &DataRootConfig) -> Result<(), String> {
    std::fs::create_dir_all(default_dir).map_err(|e| format!("建配置目录失败：{e}"))?;
    let body = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let path = config_path(default_dir);
    // 原子写：先写 .tmp 再改名，避免写一半断电留下坏 JSON（坏了会静默回退默认目录，
    // 用户看到的是「我设的自定义目录怎么没了」，很难查）。
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("写配置失败：{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换配置失败：{e}"))?;
    Ok(())
}

/// 这个目录能不能当数据文件夹：**已经存在**、是目录、能写（真的写一个探针文件再删）。
///
/// 故意**不自动创建**：
/// · 移动硬盘拔掉 → 盘符都不在，建不出来，回退默认（保住默认目录里的索引）；
/// · 目录被手动删掉 → 如果这里 `create_dir_all` 一把，就会**偷偷在空目录上重建一个空库**，
///   用户看到的是「我的库怎么空了」，而默认目录里那份索引其实还在。
///   回退默认 + 设置面板显示「当前生效：默认目录」，用户一眼能看出发生了什么。
pub fn is_usable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(".am-write-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 真正要用的数据目录：启用了自定义 + 目录可用 → 用它，否则回退默认。
pub fn resolve(default_dir: &Path) -> PathBuf {
    let cfg = load(default_dir);
    if !cfg.enabled {
        return default_dir.to_path_buf();
    }
    let Some(cur) = cfg.current.as_deref().filter(|s| !s.trim().is_empty()) else {
        return default_dir.to_path_buf();
    };
    let path = PathBuf::from(cur);
    if is_usable(&path) {
        path
    } else {
        // 回退不弹窗、不阻塞启动：界面上的「数据文件夹」会显示真实生效的路径
        eprintln!(
            "[AssetManager] 自定义数据目录不可用，回退默认目录：{} → {}",
            path.display(),
            default_dir.display()
        );
        default_dir.to_path_buf()
    }
}

/// 这个候选目录是不是落在某个素材库根里面（或者就是库根本身）。
///
/// **为什么必须拦**：数据目录里放的是 `index.db` 和 `cache/thumbs`。如果它被设成
/// 某个素材库（甚至库根的子目录），软件的索引文件就会直接出现在用户的素材目录里 ——
/// 用户看到的是一堆看不懂的 `.db` / WebView 缓存，删了还可能连索引一起丢。
/// （2026-11-16 真出过一次：数据目录被指到测试素材库根，库根里冒出 index.db + cache/。）
///
/// 比较用**小写 + 统一分隔符**的字符串前缀：Windows 路径大小写不敏感，
/// `Path::starts_with` 是逐组件精确比较，`D:\Lib` 与 `d:\lib\sub` 会判不中。
pub fn conflicts_with_libs(candidate: &Path, lib_roots: &[String]) -> Option<String> {
    let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let cand = norm(&candidate.to_string_lossy());
    for root in lib_roots {
        let r = norm(root);
        if r.is_empty() {
            continue;
        }
        if cand == r || cand.starts_with(&format!("{r}\\")) {
            return Some(root.clone());
        }
    }
    None
}

/// 目录体检结果：判断用户选的到底是「素材目录」还是「空的存放位置」。
///
/// 2026-11-16 用户把**素材库根**设成了数据文件夹 —— 界面上只多了一条 toast，
/// 他以为「设置好了素材库」，回到主界面看到 0/3 和空状态，结论是「没有任何效果」。
/// 名字叫「数据文件夹」，用户理解成「我的素材放哪」，这是词的问题；
/// 但词改不了所有情况，所以选完目录要**看一眼里面有什么**，不对劲就当场问清楚。
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirProbe {
    /// 近层里看到的图片张数（数到上限就停）
    pub images: i64,
    /// 近层里看到的视频数（视频库同理）
    pub videos: i64,
    /// 顶层条目数
    pub entries: i64,
    /// 里面已经有 index.db —— 说明这个目录以前被当成过数据目录
    pub has_index_db: bool,
}

/// 只往下看两层：素材库的典型形状是「库根/<分类>/<图>」，两层足够判断；
/// 再深就会在几万文件的大库上把设置面板卡住。
const PROBE_DEPTH: usize = 2;
/// 数够这么多就停 —— 要的是「像不像素材库」，不是精确统计。
const PROBE_MAX: i64 = 500;

/// 看一个目录里有什么（不递归太深、不 follow 符号链接、出错就跳过）。
pub fn probe(dir: &Path) -> DirProbe {
    let mut out = DirProbe::default();
    if !dir.is_dir() {
        return out;
    }
    out.has_index_db = dir.join("index.db").exists();

    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    while let Some((cur, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&cur) else {
            continue;
        };
        for ent in rd.flatten() {
            let path = ent.path();
            let Ok(ft) = ent.file_type() else { continue };
            if ft.is_dir() {
                if depth + 1 < PROBE_DEPTH {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            if depth == 0 {
                out.entries += 1;
            }
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let ext = crate::formats::lower_ext(name);
            if crate::formats::IMAGE_EXTS.contains(&ext.as_str()) {
                out.images += 1;
            } else if crate::formats::VIDEO_EXTS.contains(&ext.as_str()) {
                out.videos += 1;
            }
            if out.images + out.videos >= PROBE_MAX {
                return out;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-dr-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 没配置 → 用默认；配了且可用 → 用自定义；目录没了 → 回退默认（软件必须能开）。
    #[test]
    fn resolve_falls_back_when_custom_dir_is_gone() {
        let base = tmp("resolve");
        let custom = base.join("custom");

        assert_eq!(resolve(&base), base, "没配置就是默认目录");

        std::fs::create_dir_all(&custom).unwrap();
        save(
            &base,
            &DataRootConfig {
                enabled: true,
                current: Some(custom.to_string_lossy().to_string()),
                dirs: vec![custom.to_string_lossy().to_string()],
            },
        )
        .unwrap();
        assert_eq!(resolve(&base), custom, "启用了且目录可用就用它");

        // 模拟移动硬盘拔掉
        std::fs::remove_dir_all(&custom).unwrap();
        assert_eq!(resolve(&base), base, "目录不在了必须回退默认，不能打不开");
    }

    /// 手改坏的配置 / 空清单 / current 不在清单里 —— 都要能读成合理状态。
    #[test]
    fn load_sanitizes_dirty_config() {
        let base = tmp("dirty");
        std::fs::write(config_path(&base), b"{not json").unwrap();
        let cfg = load(&base);
        assert!(!cfg.enabled && cfg.dirs.is_empty(), "坏 JSON 当成没配置");

        std::fs::write(
            config_path(&base),
            r#"{"enabled":true,"current":"D:/x","dirs":["D:/a","d:/A","D:/b","D:/c","D:/d"]}"#,
        )
        .unwrap();
        let cfg = load(&base);
        assert!(cfg.dirs.iter().any(|d| d == "D:/x"), "current 要补进清单");
        assert!(cfg.dirs.len() <= MAX_DIRS, "清单不许超过上限");
        let lower: Vec<String> = cfg.dirs.iter().map(|d| d.to_lowercase()).collect();
        let mut uniq = lower.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), lower.len(), "去重要忽略大小写");
    }

    /// 数据文件夹不许落在素材库里（2026-11-16 真事故的防复发测试）。
    #[test]
    fn rejects_data_dir_inside_a_library() {
        // 用 PathBuf 拼路径，避免在 raw string 里数反斜杠（数错过一次：r"E:\\x" 是**两个**字符的 \\）
        let base = std::env::temp_dir().join("am-资源库测试");
        let other = std::env::temp_dir().join("Material");
        let roots = vec![
            base.to_string_lossy().to_string(),
            other.to_string_lossy().to_string(),
        ];
        // 就是库根本身
        assert!(conflicts_with_libs(&base, &roots).is_some(), "库根本身");
        // 库根的子目录
        assert!(
            conflicts_with_libs(&base.join("cache"), &roots).is_some(),
            "库根的子目录"
        );
        // 库根内部更深的层
        assert!(
            conflicts_with_libs(&base.join("a").join("b"), &roots).is_some(),
            "深层子目录"
        );
        // 库根的父目录：允许（「素材库 D:\\Material、数据 D:\\」这种是正常的）
        assert!(
            conflicts_with_libs(base.parent().unwrap(), &roots).is_none(),
            "父目录允许"
        );
        // 同名前缀的兄弟目录不算（Material2 不是 Material 的子目录）
        assert!(
            conflicts_with_libs(&std::env::temp_dir().join("Material2"), &roots).is_none(),
            "同名兄弟目录"
        );
        // 完全不相干
        assert!(
            conflicts_with_libs(&std::env::temp_dir().join("am-data"), &roots).is_none(),
            "不相干目录"
        );
    }

    /// 探针要能把「素材目录」和「空的存放位置」分开（2026-11-16 那次误设的防复发）。
    #[test]
    fn probe_tells_library_from_empty_dir() {
        let base = tmp("probe");
        let lib = base.join("lib");
        let empty = base.join("empty");
        std::fs::create_dir_all(lib.join("分类")).unwrap();
        std::fs::create_dir_all(lib.join("分类").join("更深")).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(lib.join("a.png"), b"x").unwrap();
        std::fs::write(lib.join("b.JPG"), b"x").unwrap();
        std::fs::write(lib.join("分类").join("c.webp"), b"x").unwrap();
        std::fs::write(lib.join("分类").join("v.mp4"), b"x").unwrap();
        // 第三层不算（只在两层内看），但它不该让探针崩
        std::fs::write(lib.join("分类").join("更深").join("d.png"), b"x").unwrap();

        let p = probe(&lib);
        assert_eq!(p.images, 3, "两层内 3 张图（第三层那张不算）");
        assert_eq!(p.videos, 1);
        assert_eq!(p.entries, 2, "顶层 2 个文件");
        assert!(!p.has_index_db);

        let e = probe(&empty);
        assert_eq!(e.images, 0, "空目录不许误判成素材库");
        assert_eq!(e.entries, 0);

        // 已经被当过数据目录的目录要认得出来
        std::fs::write(empty.join("index.db"), b"x").unwrap();
        assert!(probe(&empty).has_index_db);

        let _ = std::fs::remove_dir_all(&base);
    }
}
