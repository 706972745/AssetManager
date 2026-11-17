//! 自动标签（派生标签）—— **纯函数**：只读 `Asset` 里已经扫描出来的「原始文件事实」，
//! 不读盘、不解码、不写库。
//!
//! 这套设计照搬 FontManager 的 `derivedTags.ts`，包括它那三条不可动摇的纪律：
//!
//! 1. **不写用户数据**。只有用户亲手打的标签才进 `tags` / `asset_tags`；
//!    这里算出来的是「运行时有效标签」，写进库就等于把推导当事实，规则一改还要写迁移脚本。
//! 2. **手工优先**。用户在同一维度手打过任意值（比如自己打了 `place:4k`），
//!    该维度就不再产出自动值 —— 见 [`NAMESPACES`] 的互斥集合。用户否决过的标签一律不产（否决名单在界面层）。
//! 3. **宁可少给，不要给错**。只从**确定性事实**推导（扩展名、像素、体积、宽高比），
//!    不做「看图猜内容」那种弱证据规则；没有尺寸的素材干脆不产 `place:` / `orient:`。
//!
//! 与 FontManager 的差别只有一处（owner 2026-11-16 指定）：**判定依据换成原始文件的客观规格**，
//! 而不是字体族名 / PANOSE 那套字体内涵。所以这里的维度是「格式 / 类型 / 像素档 / 比例 / 体积档」。

use crate::model::{Asset, KIND_AUDIO, KIND_DOC, KIND_IMAGE, KIND_MODEL, KIND_OTHER, KIND_VIDEO};

/// 一条自动标签：标签 id + 为什么给它（详情面板要如实回答「凭什么」）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoTag {
    pub tag: String,
    pub reason: String,
}

/// 自动标签的命名空间。**这些前缀不给用户手工建**（见 [`is_reserved_name`]），
/// 否则「手动 fmt:jpg」和「自动 fmt:jpg」会撞成一条，筛选时也分不清走哪条路。
pub const NAMESPACES: &[&str] = &["fmt", "kind", "place", "orient", "size"];

/// 互斥维度：一个素材在一个维度上只可能有一个值（不可能既是 4K 又是 HD）。
/// 用户手工给某维度打了值，该维度的自动值全部让位。
pub const EXCLUSIVE: &[&str] = NAMESPACES;

/// 像素档（按长边）。**顺序即优先级，从大到小**；SQL 聚合也从这张表生成，保证两边口径一致。
pub const PLACE_STEPS: &[(i64, &str)] = &[
    (7680, "place:8k"),
    (3840, "place:4k"),
    (2560, "place:2k"),
    (1920, "place:fhd"),
    (1280, "place:hd"),
    (720, "place:sd"),
];

/// 体积档。同样被 SQL 聚合复用，别在别处另写一套阈值。
pub const SIZE_STEPS: &[(i64, &str)] = &[
    (104_857_600, "size:xl"), // ≥ 100 MB
    (20_971_520, "size:lg"),  // ≥ 20 MB
    (2_097_152, "size:md"),   // ≥ 2 MB
    (209_715, "size:sm"),     // ≥ 200 KB
];

pub fn namespace_of(tag: &str) -> Option<&str> {
    let (ns, rest) = tag.split_once(':')?;
    if rest.is_empty() {
        return None;
    }
    NAMESPACES.iter().copied().find(|n| *n == ns)
}

pub fn is_auto_tag(tag: &str) -> bool {
    namespace_of(tag).is_some()
}

/// 手工标签不允许占用自动标签的命名空间（`fmt:` `kind:` …），避免两套口径打架。
pub fn is_reserved_name(name: &str) -> bool {
    namespace_of(name).is_some()
}

pub fn kind_tag(kind: i32) -> &'static str {
    match kind {
        KIND_IMAGE => "kind:image",
        KIND_VIDEO => "kind:video",
        KIND_AUDIO => "kind:audio",
        KIND_MODEL => "kind:model",
        KIND_DOC => "kind:doc",
        _ => "kind:other",
    }
}

/// 像素档：长边落在哪一档。比最小档还小返回 `None`（小图标不值得打 `place:`）。
pub fn place_of(longest: i64) -> Option<&'static str> {
    PLACE_STEPS
        .iter()
        .find(|(px, _)| longest >= *px)
        .map(|(_, tag)| *tag)
}

pub fn size_of(bytes: i64) -> &'static str {
    SIZE_STEPS
        .iter()
        .find(|(b, _)| bytes >= *b)
        .map(|(_, tag)| *tag)
        .unwrap_or("size:xs")
}

/// 宽高比：完全相等才算 square（1px 的误差在视觉上就是方的，但比例判定要严格，宁可给 portrait/landscape）。
pub fn orient_of(w: i64, h: i64) -> Option<&'static str> {
    if w <= 0 || h <= 0 {
        return None;
    }
    Some(if w == h {
        "orient:square"
    } else if w > h {
        "orient:landscape"
    } else {
        "orient:portrait"
    })
}

/// 从一个素材的事实推导全部自动标签。**顺序固定**，界面上不会每次刷新乱跳。
pub fn derive(a: &Asset) -> Vec<AutoTag> {
    let mut out: Vec<AutoTag> = Vec::with_capacity(5);

    let ext = a.ext.to_lowercase();
    if !ext.is_empty() {
        out.push(AutoTag {
            tag: format!("fmt:{ext}"),
            reason: "按扩展名".into(),
        });
    }

    out.push(AutoTag {
        tag: kind_tag(a.kind).into(),
        reason: "按扫描判定的类型".into(),
    });

    // 尺寸类只对「有像素」的素材有意义：文档/音频算长边毫无意义，不给。
    if matches!(a.kind, KIND_IMAGE | KIND_VIDEO | KIND_MODEL) {
        if let (Some(w), Some(h)) = (a.width, a.height) {
            if let Some(p) = place_of(w.max(h)) {
                out.push(AutoTag {
                    tag: p.into(),
                    reason: format!("长边 {} px", w.max(h)),
                });
            }
            if let Some(o) = orient_of(w, h) {
                out.push(AutoTag {
                    tag: o.into(),
                    reason: if w == h {
                        "宽高相等".to_string()
                    } else if w > h {
                        "宽大于高".to_string()
                    } else {
                        "高大于宽".to_string()
                    },
                });
            }
        }
    }

    out.push(AutoTag {
        tag: size_of(a.size).into(),
        reason: "按文件体积".into(),
    });

    out
}

/// 筛选用的解析结果：把 `place:4k` 这类标签翻译成「列上的区间条件」，
/// 好让筛选在 SQL 里走索引，而不是把素材全取回来再用 JS 过滤。
#[derive(Debug, Clone, PartialEq)]
pub enum AutoFilter {
    Ext(String),
    Kind(i32),
    /// (下界闭, 上界开)
    Place(i64, Option<i64>),
    /// 'landscape' | 'portrait' | 'square'
    Orient(&'static str),
    Size(i64, Option<i64>),
}

/// 有序档位表 → 区间。`steps` 必须是从大到小。
fn range_of(steps: &[(i64, &'static str)], tag: &str) -> Option<(i64, Option<i64>)> {
    let idx = steps.iter().position(|(_, t)| *t == tag)?;
    let lower = steps[idx].0;
    let upper = if idx == 0 { None } else { Some(steps[idx - 1].0) };
    Some((lower, upper))
}

pub fn parse_filter(tag: &str) -> Option<AutoFilter> {
    let (ns, val) = tag.split_once(':')?;
    match ns {
        "fmt" => {
            // 扩展名只允许字母数字：这个值直接进 SQL 参数，白名单式校验挡掉怪东西
            if val.is_empty() || !val.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            Some(AutoFilter::Ext(val.to_lowercase()))
        }
        "kind" => Some(AutoFilter::Kind(match val {
            "image" => KIND_IMAGE,
            "video" => KIND_VIDEO,
            "audio" => KIND_AUDIO,
            "model" => KIND_MODEL,
            "doc" => KIND_DOC,
            _ => KIND_OTHER,
        })),
        "place" => range_of(PLACE_STEPS, tag).map(|(lo, hi)| AutoFilter::Place(lo, hi)),
        "orient" => match val {
            "landscape" | "portrait" | "square" => Some(AutoFilter::Orient(match val {
                "landscape" => "landscape",
                "portrait" => "portrait",
                _ => "square",
            })),
            _ => None,
        },
        "size" => {
            if tag == "size:xs" {
                Some(AutoFilter::Size(0, Some(SIZE_STEPS.last()?.0)))
            } else {
                range_of(SIZE_STEPS, tag).map(|(lo, hi)| AutoFilter::Size(lo, hi))
            }
        }
        _ => None,
    }
}

/// 手工优先：把被用户「盖住」的自动标签剔掉。///
/// 规则和 FontManager 一样硬：只要用户在某个互斥维度手打过**任意**值，
/// 这个维度的自动值就全部不出现（用户把它改成 `place:2k`，软件不能再补一条 `place:4k`）。
pub fn apply_manual_priority(derived: Vec<AutoTag>, manual: &[String]) -> Vec<AutoTag> {
    if manual.is_empty() {
        return derived;
    }
    let manual_ns: Vec<&str> = manual.iter().filter_map(|t| namespace_of(t)).collect();
    derived
        .into_iter()
        .filter(|d| match namespace_of(&d.tag) {
            Some(ns) => {
                // 同维度被手工覆盖（含手打了完全相同的值）→ 让位给手动那条
                !manual_ns.iter().any(|m| *m == ns)
            }
            None => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(ext: &str, kind: i32, w: Option<i64>, h: Option<i64>, size: i64) -> Asset {
        Asset {
            id: 1,
            dir_id: 1,
            name: format!("x.{ext}"),
            ext: ext.into(),
            kind,
            size,
            mtime: 0,
            /* 自动标签只看文件事实，不看入库时间 —— 测试里给 0（= 老数据"未知"） */
            added_at: 0,
            width: w,
            height: h,
            thumb_key: None,
            thumb_state: 0,
            quality: 0,
            path: format!("D:/lib/x.{ext}"),
            rating: 0,
            note: String::new(),
            source_url: String::new(),
            palette: String::new(),
            color: String::new(),
        }
    }

    #[test]
    fn derives_facts_not_guesses() {
        let a = asset("jpg", KIND_IMAGE, Some(3840), Some(2160), 5_000_000);
        let tags: Vec<String> = derive(&a).into_iter().map(|t| t.tag).collect();
        assert_eq!(
            tags,
            vec!["fmt:jpg", "kind:image", "place:4k", "orient:landscape", "size:md"]
        );

        // 没有尺寸的（扫描没读到）→ 一个尺寸类标签都不给，别猜
        let b = asset("psd", KIND_IMAGE, None, None, 300_000);
        let tags: Vec<String> = derive(&b).into_iter().map(|t| t.tag).collect();
        assert_eq!(tags, vec!["fmt:psd", "kind:image", "size:sm"]);

        // 文档不给 place/orient
        let c = asset("pdf", KIND_DOC, Some(100), Some(100), 10);
        let tags: Vec<String> = derive(&c).into_iter().map(|t| t.tag).collect();
        assert_eq!(tags, vec!["fmt:pdf", "kind:doc", "size:xs"]);
    }

    #[test]
    fn manual_wins_in_same_namespace() {
        let a = asset("jpg", KIND_IMAGE, Some(3900), Some(2160), 5_000_000);
        let manual = vec!["place:2k".to_string(), "客户".to_string()];
        let tags: Vec<String> = apply_manual_priority(derive(&a), &manual)
            .into_iter()
            .map(|t| t.tag)
            .collect();
        assert!(tags.contains(&"fmt:jpg".into()));
        assert!(!tags.contains(&"place:4k".into()), "同维度必须让位给手工值");
        assert!(tags.contains(&"orient:landscape".into()), "别的维度不受影响");
    }

    #[test]
    fn reserved_namespace_guard() {
        assert!(is_reserved_name("fmt:jpg"));
        assert!(is_reserved_name("place:4k"));
        assert!(!is_reserved_name("客户"));
        assert!(!is_reserved_name("fmt:"));
        assert!(!is_reserved_name("我的:标签"));
    }
}
