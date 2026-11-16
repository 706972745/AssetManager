//! 数据结构与对外 DTO。

use serde::{Deserialize, Serialize};

pub const KIND_IMAGE: i32 = 0;
pub const KIND_VIDEO: i32 = 1;
pub const KIND_AUDIO: i32 = 2;
pub const KIND_MODEL: i32 = 3;
pub const KIND_DOC: i32 = 4;
pub const KIND_OTHER: i32 = 5;

/// 缩略图状态
pub const THUMB_PENDING: i32 = 0;
pub const THUMB_OK: i32 = 1;
pub const THUMB_FAILED: i32 = 2;
pub const THUMB_UNSUPPORTED: i32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: i64,
    pub dir_id: i64,
    pub name: String,
    pub ext: String,
    pub kind: i32,
    pub size: i64,
    pub mtime: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub thumb_key: Option<String>,
    pub thumb_state: i32,
    pub quality: i32,
    /// 由 dirs.path + name 拼出的绝对路径（不落库）
    pub path: String,
    /// 用户评分 0–5（0 = 未评分）。**是人写的东西，不是从文件推导的**，所以落库
    pub rating: i32,
    /// 用户注释（空串 = 没写）
    pub note: String,
    /// 来源 URL（空串 = 没填）。找素材出处用，不影响任何自动标签
    pub source_url: String,
    /// 5 个主色（`rrggbb,rrggbb,…`，最多 5 个；空串 = 还没算过）。详情面板的色板用
    pub palette: String,
    /// 色桶（red / orange / …，空串 = 还没算过或图是灰的没主色）。按颜色筛走这一列
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirNode {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub parent_id: Option<i64>,
    pub child_cnt: i64,
    pub asset_cnt: i64,
    pub has_children: bool,
}

/// 列表查询。cursor 用 keyset 分页（mtime, id）。
///
/// `default` 是必须的：前端只送它认识的字段，后端加的字段（color / tagIds …）
/// 缺省时按 `Default` 补齐，而不是让整条命令反序列化失败。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AssetQuery {
    pub dir_id: Option<i64>,
    /// 是否包含子目录
    pub recursive: bool,
    pub kinds: Option<Vec<i32>>,
    pub text: Option<String>,
    /// mtime_desc | name_asc | size_desc
    pub sort: Option<String>,
    pub cursor: Option<(i64, i64)>,
    pub limit: Option<i64>,
    /// 标签筛选：手动标签（`素材` / `参考`）走 asset_tags；
    /// 自动标签（`fmt:jpg`）按其维度翻译成列条件 —— 两者都在 SQL 里筛，不是取回来再过滤
    pub tag: Option<String>,
    /// 颜色筛选（T24）：色桶名（red / orange / …）。空 = 不筛
    pub color: Option<String>,
    /// **后端填的**：手工标签在注册库里的 id（拆库后 asset_tags 存的是 tag_id 不是名字）。
    /// 前端不送 —— `tag` 里给的是名字，`App::list` 查一次注册库换成 id。
    pub tag_ids: Option<Vec<i64>>,
    /// **后端填的**：搜索词命中的标签 id（T28 拆库后不能跨文件 JOIN 注册库的 tags 表，
    /// 名字 → id 的换算只能在 `App::list` 里做）
    pub search_tag_ids: Option<Vec<i64>>,
    /// **后端填的**：这个标签如果是"自动规则标签"（T34），这里是它的规则 JSON。
    /// 规则标签不物化关联行，筛选时翻译成列条件。
    pub tag_rule: Option<String>,
    /// **后端填的**：跨库合并分页时，`name_asc` 排序的游标（名字, 库内 id 或 ±调整值）。
    /// 前端不送 —— 它只知道 `(已取条数, 最后一行全局 id)`。
    pub name_cursor: Option<(String, i64)>,
}

/// 标签 + 使用数（侧栏每次刷新拉一次）。
///
/// `color` / `group` 只有**手工标签**才有意义：自动标签是推导出来的，
/// 既不能进分组也不能自选颜色（见 `autotag::NAMESPACES` 那三条纪律）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub name: String,
    pub count: i64,
    /// 颜色键（`db::TAG_COLORS` 白名单）；None = 默认灰
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 所属分组名；None = 未分组
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// 标签组：只做**一层**（组里放标签，组不可嵌套）。
/// 层级再深用户会迷失，而且拖拽/移动的成本会盖过收益。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGroup {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 组内素材数（同一素材被组内多个标签命中只算一次）
    pub count: i64,
    /// 组里标签个数（空组也要能在侧栏显示出来，否则用户建完就找不到了）
    pub tags: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    pub dirs: i64,
    pub files: i64,
    pub images: i64,
    pub with_dims: i64,
    pub errors: i64,
    /// 增量扫描时「磁盘上已经没有、索引行被删掉」的目录数
    pub removed_dirs: i64,
    /// 整棵跳过的「库中库」目录数（Eagle 的 `*.library` 之类，见 scan::is_foreign_library）
    pub skipped: i64,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub dirs: i64,
    pub files: i64,
    pub current: String,
    pub done: bool,
}

/* ------------------------------------------------ T24/T25/T26/T33 的对外 DTO */

/// 精确重复组（同一份文件的多份拷贝）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupGroup {
    /// xxh3-128 的十六进制（只用来做展示与去重键，不是安全哈希）
    pub hash: String,
    pub size: i64,
    /// **全局 id**（lib<<40 | 库内 id）
    pub ids: Vec<i64>,
}

/// 相似重复对
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarPair {
    pub a: i64,
    pub b: i64,
    /// 汉明距离（0 = 几乎一模一样）
    pub dist: u32,
}

/// 引用行（T26）
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefItem {
    /// 工程文件（或引用它的工程）的全局 id
    pub src_id: i64,
    pub src_name: String,
    pub src_path: String,
    /// 工程里写的那条路径（原样）
    pub dst_path: String,
    /// 解析到的素材名（空 = 没解析到）
    pub dst_name: String,
    pub dst_id: Option<i64>,
    pub missing: bool,
}

/// 批量重命名规则（T33）。空字段 = 不做这一步。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenameRule {
    pub find: String,
    pub replace: String,
    pub prefix: String,
    pub suffix: String,
    /// 追加序号：0 = 不加；≥1 = 从几开始
    pub number_from: i64,
    /// 序号补零到几位（`0` = 不补）
    pub number_pad: usize,
    /// 序号插在原扩展名之前（默认 true）
    pub number_before_ext: bool,
    pub lower: bool,
    pub upper: bool,
    /// 保留原扩展名（默认 true；false = 用户自己写了扩展名）
    pub keep_ext: bool,
}

/// 一条重命名预览
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameItem {
    pub id: i64,
    pub old_name: String,
    pub new_name: String,
    /// 非空 = 这一条不能改（同名冲突 / 非法字符 / 名字没变）
    pub error: String,
}

/// 批量重命名的执行结果（`dirs` = 受影响的目录，命令层拿它刷新索引）
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameResult {
    pub ok: i64,
    pub errors: Vec<String>,
    pub dirs: Vec<String>,
}
