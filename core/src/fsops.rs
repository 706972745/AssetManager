//! 真实文件操作（1:1 映射磁盘）。
//!
//! 纪律（MVP 方案 §3.3–§3.6）：
//! - 操作前校验：非法名 / 保留名 / 同名冲突 / 移进自己子目录
//! - 操作后 **re-stat**，绝不推算；失败不写库
//! - 每次批量操作写 `ops` 日志，可撤销（删除除外 —— 已进回收站）
//! - 跨盘移动 = 复制 → 删除，失败保留源
//! - 失败原因**翻译成人话**（`explain_io`），绝不把 `os error 32` 甩给用户

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{self, Res};

pub const OP_MKDIR: i32 = 0;
pub const OP_RENAME: i32 = 1;
pub const OP_MOVE: i32 = 2;
pub const OP_DELETE: i32 = 3;

const BAD_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/* ---------------------------------------------------------------- 报错说人话 */

/// 把 `std::io::Error` 翻成用户能看懂的一句话（规格 §3.4 的九类异常）。
///
/// 为什么必须做：Windows 的 `os error 32`、`os error 5` 对用户等于没说，
/// 他只会觉得「这软件坏了」。这里按 raw_os_error 逐条映射，兜底才拼原话。
pub fn explain_io(e: &std::io::Error, what: &str) -> String {
    if let Some(code) = e.raw_os_error() {
        let msg = match code {
            2 | 3 => "找不到这个文件，可能已经被移动或删除",
            5 => "没有权限修改这个位置（只读，或需要管理员）",
            19 | 21 => "磁盘写保护，改不了",
            32 => "文件正被别的程序占用（Photoshop、看图工具或预览窗口）",
            33 => "文件被锁定（有程序正打开其中一段）",
            39 | 112 => "磁盘空间不足",
            80 | 183 => "目标位置已经有同名文件",
            123 => "文件名里有系统不接受的字符",
            206 => "路径太长，超过系统上限",
            225 => "文件被安全软件锁住，稍后再试",
            17 => "跨盘移动要先复制再删除，但复制这一步失败了",
            _ => "",
        };
        if !msg.is_empty() {
            return format!("{}：{}", what, msg);
        }
    }
    format!("{}：{}", what, e)
}

/* ---------------------------------------------------------------- 名称校验 */

/// 校验并规范化一个文件名/目录名。Windows 口径。
pub fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim().trim_end_matches([' ', '.']).to_string();
    if name.is_empty() {
        return Err("名字不能为空".into());
    }
    if name == "." || name == ".." {
        return Err("这个名字不能用".into());
    }
    if let Some(c) = name.chars().find(|c| BAD_CHARS.contains(c)) {
        return Err(format!("名字里不能有 {} ", c));
    }
    if name.chars().any(|c| (c as u32) < 0x20) {
        return Err("名字里有不可见字符".into());
    }
    if name.len() > 255 {
        return Err("名字太长（超过 255 字节）".into());
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!("「{}」是系统保留名，换一个", name));
    }
    Ok(name)
}

fn lower(s: &Path) -> String {
    s.to_string_lossy().to_lowercase()
}

fn volume_of(p: &Path) -> String {
    p.to_string_lossy().chars().take(2).collect::<String>().to_lowercase()
}

pub fn same_volume(a: &Path, b: &Path) -> bool {
    volume_of(a) == volume_of(b)
}

fn is_inside(child: &Path, parent: &Path) -> bool {
    lower(child).starts_with(&format!("{}\\", lower(parent).trim_end_matches('\\')))
}

/* ---------------------------------------------------------------- 事前体检 */

/// 一批文件的体量：多少个文件、共多少字节（递归，最多 8 层，防环）。
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemStat {
    pub files: i64,
    pub bytes: i64,
}

fn walk_stat(p: &Path, depth: usize, acc: &mut ItemStat) {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return };
    if meta.is_dir() {
        if depth > 8 {
            return;
        }
        if let Ok(rd) = std::fs::read_dir(p) {
            for ent in rd.flatten() {
                walk_stat(&ent.path(), depth + 1, acc);
            }
        }
    } else {
        acc.files += 1;
        acc.bytes += meta.len() as i64;
    }
}

pub fn stat_paths(paths: &[PathBuf]) -> ItemStat {
    let mut acc = ItemStat::default();
    for p in paths {
        walk_stat(p, 0, &mut acc);
    }
    acc
}

/// 落盘前的体检结果：前端据此弹「>100 个 / >2GB」的二次确认、
/// 以及「目标已有同名 → 三选」的对话框。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovePlan {
    pub files: i64,
    pub bytes: i64,
    /// 目标目录里已经存在的同名项（只列名字，不列全路径）
    pub conflicts: Vec<String>,
    /// 跨盘：会走「复制 → 删源」，大文件更慢，要提前说
    pub cross_volume: bool,
    pub dirs: Vec<String>,
}

pub fn plan_move(paths: &[PathBuf], target_dir: &Path) -> Res<MovePlan> {
    if !target_dir.is_dir() {
        return Err("目标不是文件夹".into());
    }
    let st = stat_paths(paths);
    let mut conflicts = Vec::new();
    let mut cross = false;
    for p in paths {
        if !p.exists() {
            continue;
        }
        let Some(name) = p.file_name() else { continue };
        if target_dir.join(name).exists() {
            conflicts.push(name.to_string_lossy().to_string());
        }
        if !same_volume(p, target_dir) {
            cross = true;
        }
    }
    Ok(MovePlan {
        files: st.files,
        bytes: st.bytes,
        conflicts,
        cross_volume: cross,
        dirs: paths
            .iter()
            .filter(|p| p.is_dir())
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
    })
}

/// 同名三选：跳过 / 保留两者 / 替换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovePolicy {
    /// 目标同名就跳过（默认，最安全）
    Skip,
    /// 保留两者：目标位置改名 `名称 (2).ext`
    KeepBoth,
    /// 替换：目标位置原有的那个先送回收站，再放进来（可还原）
    Replace,
}

impl Default for MovePolicy {
    fn default() -> Self {
        Self::Skip
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveReport {
    pub moved: i64,
    /// 改名后放进去的（原名 → 新名）
    pub renamed: Vec<(String, String)>,
    /// 被替换掉、已进回收站的
    pub replaced: Vec<String>,
    /// 跳过的（含原因）
    pub skipped: Vec<String>,
}

/// 移动进度。跨盘是「复制 → 删源」，几百 MB 的单文件能跑十几秒，
/// 界面必须看得见走到第几个，否则用户会以为卡死了（缺陷 #7）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveProgress {
    pub done: i64,
    pub total: i64,
    pub current: String,
}

pub struct FsOps<'a> {
    pub conn: &'a Connection,
}

impl<'a> FsOps<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// 新建文件夹；重名自动加 ` (2)`。
    pub fn mkdir(&self, parent: &Path, name: &str) -> Res<PathBuf> {
        let mut base = validate_name(name)?;
        let mut target = parent.join(&base);
        let mut n = 2;
        while target.exists() {
            base = format!("{} ({})", validate_name(name)?, n);
            target = parent.join(&base);
            n += 1;
            if n > 999 {
                return Err("同名文件夹太多，换个名字".into());
            }
        }
        std::fs::create_dir(&target).map_err(|e| explain_io(&e, "建文件夹失败"))?;
        let payload = json!({ "items": [{ "from": "", "to": target.to_string_lossy() }] });
        db::log_op(self.conn, OP_MKDIR, &payload)?;
        Ok(target)
    }

    /// 重命名（文件或文件夹）。只改大小写走两步。
    pub fn rename(&self, path: &Path, new_name: &str) -> Res<PathBuf> {
        let new_name = validate_name(new_name)?;
        let parent = path.parent().ok_or("没有上级目录")?;
        let target = parent.join(&new_name);
        let old_name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();

        let case_only = old_name.to_lowercase() == new_name.to_lowercase();
        if !case_only && target.exists() {
            return Err(format!("「{}」已经存在，换一个名字", new_name));
        }

        let result = if case_only && old_name != new_name {
            // NTFS 认为同名不动：先改成临时名
            let tmp = parent.join(format!(".am-rename-{}", std::process::id()));
            std::fs::rename(path, &tmp).map_err(|e| explain_io(&e, "重命名失败"))?;
            match std::fs::rename(&tmp, &target) {
                Ok(_) => Ok(()),
                Err(e) => {
                    let _ = std::fs::rename(&tmp, path);
                    Err(explain_io(&e, "重命名失败"))
                }
            }
        } else {
            std::fs::rename(path, &target).map_err(|e| explain_io(&e, "重命名失败"))
        };
        result?;

        if !target.exists() {
            return Err("重命名后目标不存在，已放弃".into());
        }
        let payload = json!({
            "items": [{ "from": path.to_string_lossy(), "to": target.to_string_lossy() }]
        });
        db::log_op(self.conn, OP_RENAME, &payload)?;
        Ok(target)
    }

    /// 移动到目录（同盘 rename，跨盘复制→删源）。
    pub fn mv(&self, paths: &[PathBuf], target_dir: &Path) -> Res<Vec<(PathBuf, PathBuf)>> {
        let ext = self.mv_with(paths, target_dir, MovePolicy::Skip)?;
        if ext.report.moved == 0 && !ext.report.skipped.is_empty() {
            return Err(format!("没有移动任何文件：{}", ext.report.skipped.join("；")));
        }
        Ok(ext.pairs.into_iter().map(|(a, b)| (PathBuf::from(a), PathBuf::from(b))).collect())
    }

    /// 带冲突策略的移动（界面「三选」后调这个）。
    pub fn mv_with(
        &self,
        paths: &[PathBuf],
        target_dir: &Path,
        policy: MovePolicy,
    ) -> Res<MoveExt> {
        self.mv_with_progress(paths, target_dir, policy, |_p| {})
    }

    /// 同 `mv_with`，但每走完一项回调一次进度（100ms 节流，万级批量也不会把事件队列打爆）。
    /// 回调是 `FnMut`，Rust 侧直接 `emit`，前端拿 `move://progress` 更新进度条。
    pub fn mv_with_progress<F>(
        &self,
        paths: &[PathBuf],
        target_dir: &Path,
        policy: MovePolicy,
        mut on_progress: F,
    ) -> Res<MoveExt>
    where
        F: FnMut(MoveProgress),
    {
        if !target_dir.is_dir() {
            return Err("目标不是文件夹".into());
        }
        let mut rep = MoveReport::default();
        let mut pairs: Vec<(String, String)> = Vec::new();
        let total = paths.len() as i64;
        let mut last_tick = std::time::Instant::now();

        for (i, p) in paths.iter().enumerate() {
            if !p.exists() {
                rep.skipped.push(format!("{}（已不存在）", p.display()));
                continue;
            }
            if p.is_dir() && is_inside(target_dir, p) {
                return Err(format!("不能把「{}」移进它自己的子目录", p.display()));
            }
            let name = p.file_name().ok_or("路径异常")?.to_string_lossy().to_string();
            let mut dest = target_dir.join(&name);
            if lower(&dest) == lower(p) {
                continue;
            }
            if dest.exists() {
                match policy {
                    MovePolicy::Skip => {
                        rep.skipped.push(format!("{}（目标已有同名）", name));
                        continue;
                    }
                    MovePolicy::KeepBoth => {
                        dest = unique_dest(target_dir, &name);
                        let newname = dest.file_name().unwrap_or_default().to_string_lossy().to_string();
                        rep.renamed.push((name.clone(), newname));
                    }
                    MovePolicy::Replace => {
                        // 替换不是「直接删」：先送回收站，用户还能还原
                        trash::delete(&dest)
                            .map_err(|e| format!("替换失败（旧文件删不掉）：{}", e))?;
                        rep.replaced.push(name.clone());
                    }
                }
            }
            if same_volume(p, target_dir) {
                std::fs::rename(p, &dest).map_err(|e| explain_io(&e, "移动失败"))?;
            } else {
                copy_then_remove(p, &dest)?;
            }
            rep.moved += 1;
            pairs.push((p.to_string_lossy().to_string(), dest.to_string_lossy().to_string()));

            // 节流：100ms 一次，最后一项一定发（否则进度条停在 99%）
            if last_tick.elapsed().as_millis() >= 100 || i as i64 + 1 == total {
                last_tick = std::time::Instant::now();
                on_progress(MoveProgress { done: i as i64 + 1, total, current: name });
            }
        }

        if !pairs.is_empty() {
            let items: Vec<_> = pairs
                .iter()
                .map(|(f, t)| json!({ "from": f, "to": t }))
                .collect();
            db::log_op(self.conn, OP_MOVE, &json!({ "items": items }))?;
        }
        Ok(MoveExt { report: rep, pairs })
    }

    /// 删除 → 系统回收站（可恢复）。
    pub fn delete(&self, paths: &[PathBuf]) -> Res<()> {
        let list: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
        // 回收站 API 的错误是个壳子枚举，字符串里带着 HRESULT —— 这里按最常见两种翻译
        trash::delete_all(&list).map_err(|e| {
            let s = e.to_string();
            let low = s.to_lowercase();
            if low.contains("80070005") || low.contains("denied") || low.contains("拒绝") {
                "删除失败：没有权限（文件只读，或被系统保护）".to_string()
            } else if low.contains("80070020") || low.contains("80070021") || low.contains("sharing") {
                "删除失败：文件正被别的程序占用".to_string()
            } else {
                format!("删除失败：{}", s)
            }
        })?;
        let items: Vec<_> = paths
            .iter()
            .map(|p| json!({ "from": p.to_string_lossy(), "to": "" }))
            .collect();
        db::log_op(self.conn, OP_DELETE, &json!({ "items": items }))?;
        Ok(())
    }

    /// 撤销上一次操作，返回给用户看的一句话。
    pub fn undo_last(&self) -> Res<String> {
        let (id, kind, payload) = db::last_undoable_op(self.conn)?
            .ok_or_else(|| "没有可撤销的操作".to_string())?;
        let v: serde_json::Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        let items = v["items"].as_array().cloned().unwrap_or_default();

        match kind {
            OP_MKDIR => {
                for it in &items {
                    let to = it["to"].as_str().unwrap_or("");
                    if !to.is_empty() {
                        let _ = std::fs::remove_dir(Path::new(to));
                    }
                }
                db::mark_op_undone(self.conn, id)?;
                Ok("已撤销：新建文件夹".into())
            }
            OP_RENAME | OP_MOVE => {
                let mut failed = 0;
                for it in items.iter().rev() {
                    let from = it["from"].as_str().unwrap_or("");
                    let to = it["to"].as_str().unwrap_or("");
                    if from.is_empty() || to.is_empty() {
                        continue;
                    }
                    if Path::new(to).exists() && std::fs::rename(to, from).is_err() {
                        failed += 1;
                    }
                }
                db::mark_op_undone(self.conn, id)?;
                Ok(if failed == 0 {
                    "已撤销上一次移动/重命名".into()
                } else {
                    format!("撤销完成，但有 {} 项失败（可能已被占用）", failed)
                })
            }
            OP_DELETE => Ok("删除已送进回收站，无法自动撤销，请到回收站还原".into()),
            _ => Err("未知操作".into()),
        }
    }
}

/// 移动的完整结果（报告 + 逐项 from/to，供上层刷新索引）。
#[derive(Debug, Clone, Default)]
pub struct MoveExt {
    pub report: MoveReport,
    pub pairs: Vec<(String, String)>,
}

fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let path = Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.to_string());
    let ext = path.extension().map(|s| s.to_string_lossy().to_string());
    for n in 2..10_000 {
        let candidate = match &ext {
            Some(e) => format!("{} ({}).{}", stem, n, e),
            None => format!("{} ({})", stem, n),
        };
        let p = dir.join(candidate);
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{} (copy)", stem))
}

fn copy_then_remove(src: &Path, dest: &Path) -> Res<()> {
    if src.is_dir() {
        copy_dir(src, dest)?;
    } else {
        std::fs::copy(src, dest).map_err(|e| explain_io(&e, "复制失败"))?;
        let a = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
        let b = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        if a != b {
            let _ = std::fs::remove_file(dest);
            return Err("复制后大小不一致，已放弃移动（源文件还在）".into());
        }
    }
    // 复制成功才删源
    if src.is_dir() {
        std::fs::remove_dir_all(src).map_err(|e| explain_io(&e, "源目录删不掉"))?;
    } else {
        std::fs::remove_file(src).map_err(|e| explain_io(&e, "源文件删不掉"))?;
    }
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> Res<()> {
    std::fs::create_dir_all(dest).map_err(|e| explain_io(&e, "建目录失败"))?;
    for ent in std::fs::read_dir(src).map_err(|e| explain_io(&e, "读目录失败"))?.flatten() {
        let p = ent.path();
        let d = dest.join(ent.file_name());
        if p.is_dir() {
            copy_dir(&p, &d)?;
        } else {
            std::fs::copy(&p, &d).map_err(|e| explain_io(&e, "复制失败"))?;
        }
    }
    Ok(())
}

/// 计算 explorer 的命令参数（抽成纯函数，便于单测；不 spawn 任何进程）。
///
/// 为什么要 `raw_arg` 而不是 `arg`：`Command::arg` 会给**整个**参数套引号，
/// 变成 `"/select,C:\...\截图 2026-11-18 09-59-51.png"`。explorer 只认
/// `/select,"C:\..."`（引号只包路径），整条被包住时它解析失败，退回「文档」——
/// owner 2026-11-18 报的「点位置打开的地方不对」就是这条。
///
/// 目录与文件分开：目录要**进去**（explorer "路径"），文件要**选中**
/// （explorer /select,"路径"）。以前一律 /select，点素材库那行的「打开」
/// 会跳到父目录并把库文件夹高亮 —— 也不是用户要的「打开这个文件夹」。
#[cfg(windows)]
fn reveal_arg(path: &Path) -> String {
    let s = path.to_string_lossy();
    if path.is_dir() {
        format!("\"{}\"", s)
    } else {
        format!("/select,\"{}\"", s)
    }
}

/// 在资源管理器中定位（Windows 兜底入口）。
///
/// 文件 → 打开所在文件夹并选中它；目录 → 直接进这个目录。
/// 目标不存在时**如实报错**：交给 explorer 一个不存在的路径，它只会静默退回
/// 某个默认文件夹，用户以为「软件乱跳」，其实文件早被移走了。
pub fn reveal_in_explorer(path: &Path) -> Res<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        if !path.exists() {
            return Err(format!(
                "这个位置已经不在了：{}\n可能被移动或删除过，重新扫描一下这个目录就能对上。",
                path.display()
            ));
        }
        Command::new("explorer.exe")
            .raw_arg(reveal_arg(path))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("暂只支持 Windows".into())
    }
}

/// 打开系统回收站。
pub fn open_recycle_bin() -> Res<()> {
    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("explorer.exe")
            .arg("shell:RecycleBinFolder")
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Err("暂只支持 Windows".into())
    }
}

/// 用系统默认程序打开**原文件**。
///
/// 为什么不做字符串拼接去调 shell：路径里带 `&` `%` 时会被解析掉半截；
/// ShellExecuteW 直接把宽字符路径交给系统关联，不经过任何命令解析。
#[cfg(windows)]
pub fn open_with_default(path: &Path) -> Res<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            op: *const u16,
            file: *const u16,
            params: *const u16,
            dir: *const u16,
            show: i32,
        ) -> isize;
    }

    let file: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let r = unsafe {
        ShellExecuteW(0, op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1)
    };
    // 返回值 <= 32 都是错误码（32 以上才算成功）
    if r as usize > 32 {
        Ok(())
    } else {
        Err(format!(
            "系统里没有能打开 .{} 的默认程序",
            path.extension().and_then(|e| e.to_str()).unwrap_or("")
        ))
    }
}

#[cfg(not(windows))]
pub fn open_with_default(_path: &Path) -> Res<()> {
    Err("暂只支持 Windows".into())
}

/// 用系统默认浏览器打开一个链接（T23「来源 URL」那一行的打开按钮）。
///
/// **只放行 http / https**：ShellExecuteW 会把任意协议交给注册好的程序处理，
/// 用户若粘一个 `ms-settings:` 或自定义协议进来就等于"点一下就执行别的程序"。
/// 协议白名单是这里唯一的安全线，别为了方便去掉。
#[cfg(windows)]
pub fn open_url(url: &str) -> Res<()> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("只支持 http / https 链接".into());
    }
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            op: *const u16,
            file: *const u16,
            params: *const u16,
            dir: *const u16,
            show: i32,
        ) -> isize;
    }
    let file: Vec<u16> = u.encode_utf16().chain(std::iter::once(0)).collect();
    let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let r = unsafe {
        ShellExecuteW(0, op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1)
    };
    if r as usize > 32 {
        Ok(())
    } else {
        Err("没能唤起浏览器".into())
    }
}

#[cfg(not(windows))]
pub fn open_url(_url: &str) -> Res<()> {
    Err("暂只支持 Windows".into())
}

/// 把**原文件**放进剪贴板（CF_HDROP）。
///
/// 这是「缩略图只是显示层」的关键一环：用户在缩略图上点「复制」，
/// 粘到资源管理器 / Photoshop 里的必须是磁盘上的原始文件，不是那张 WebP 缩略图。
/// 用 CF_HDROP 而不是 CF_DIB —— 后者粘出去是一张位图，文件名、格式、EXIF 全丢。
#[cfg(windows)]
pub fn copy_files_to_clipboard(paths: &[PathBuf]) -> Res<()> {
    use std::os::windows::ffi::OsStrExt;

    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        pt_x: i32,
        pt_y: i32,
        f_nc: i32,
        f_wide: i32,
    }

    const GMEM_MOVEABLE: u32 = 0x0002;
    const CF_HDROP: u32 = 15;

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
        fn GlobalLock(h: isize) -> *mut u8;
        fn GlobalUnlock(h: isize) -> i32;
        fn GlobalFree(h: isize) -> isize;
    }
    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hwnd: isize) -> i32;
        fn EmptyClipboard() -> i32;
        fn CloseClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: isize) -> isize;
    }

    if paths.is_empty() {
        return Err("没有要复制的文件".into());
    }

    // DROPFILES.pFiles 是文件列表的字节偏移；列表为「宽字符 + 双 null 结尾」
    let header = std::mem::size_of::<DropFiles>();
    let mut list: Vec<u16> = Vec::new();
    for p in paths {
        list.extend(p.as_os_str().encode_wide());
        list.push(0);
    }
    list.push(0);
    let bytes = header + list.len() * 2;

    unsafe {
        if OpenClipboard(0) == 0 {
            return Err("剪贴板正被别的程序占用，稍后再试".into());
        }
        let _ = EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if h == 0 {
            CloseClipboard();
            return Err("内存不足".into());
        }
        let p = GlobalLock(h);
        if p.is_null() {
            GlobalFree(h);
            CloseClipboard();
            return Err("锁内存失败".into());
        }
        let df = DropFiles { p_files: header as u32, pt_x: 0, pt_y: 0, f_nc: 0, f_wide: 1 };
        std::ptr::copy_nonoverlapping(&df as *const DropFiles as *const u8, p, header);
        std::ptr::copy_nonoverlapping(list.as_ptr() as *const u8, p.add(header), list.len() * 2);
        GlobalUnlock(h);
        // SetClipboardData 成功后所有权归系统，**不能**再 GlobalFree
        if SetClipboardData(CF_HDROP, h) == 0 {
            GlobalFree(h);
            CloseClipboard();
            return Err("写入剪贴板失败".into());
        }
        CloseClipboard();
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn copy_files_to_clipboard(_paths: &[PathBuf]) -> Res<()> {
    Err("暂只支持 Windows".into())
}

/// 拖拽预览图：把缓存里的 WebP 缩成一枚小 PNG。
///
/// 为什么要转 PNG：`drag` 在 Windows 上走 WIC 解码，WebP 需要用户装过
/// 「Webp Image Extensions」才认；PNG 是系统自带解码，绝不会因为环境不同失灵。
#[cfg(windows)]
pub fn drag_preview_png(thumb: &Path, longest: u32) -> Option<Vec<u8>> {
    let img = image::open(thumb).ok()?;
    let small = img.thumbnail(longest, longest);
    let mut out = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

#[cfg(not(windows))]
pub fn drag_preview_png(_thumb: &Path, _longest: u32) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert_eq!(validate_name("  a.png  ").unwrap(), "a.png");
        assert!(validate_name("a/b.png").is_err());
        assert!(validate_name("con.png").is_err());
        assert!(validate_name("   ").is_err());
        assert_eq!(validate_name("正常名字.jpg").unwrap(), "正常名字.jpg");
    }

    #[test]
    fn inside_check() {
        assert!(is_inside(Path::new(r"C:\a\b\c"), Path::new(r"C:\a\b")));
        assert!(!is_inside(Path::new(r"C:\a\bc"), Path::new(r"C:\a\b")));
    }

    #[test]
    fn explain_maps_common_codes() {
        let e = std::io::Error::from_raw_os_error(32);
        assert!(explain_io(&e, "移动失败").contains("占用"));
        let e = std::io::Error::from_raw_os_error(5);
        assert!(explain_io(&e, "改名失败").contains("权限"));
    }

    /// 新建文件夹：**同名绝不覆盖**，自动加 `(2)`；目录要真的建在磁盘上。
    /// （owner 2026-11-19 报「在软件里新建的文件夹看不到」—— 那条真因是 `am_mkdir`
    ///  只建磁盘不写索引，这里先把"建出来的是什么"钉住。）
    #[test]
    fn mkdir_auto_renames_instead_of_overwriting() {
        let base = std::env::temp_dir().join(format!("am-mkdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("已有")).unwrap();

        let conn = crate::db::open(&base.join("t.db")).unwrap();
        let ops = FsOps::new(&conn);

        let a = ops.mkdir(&base, "新文件夹").unwrap();
        assert_eq!(a.file_name().unwrap().to_string_lossy(), "新文件夹");
        assert!(a.is_dir(), "要真的建在磁盘上");

        let b = ops.mkdir(&base, "新文件夹").unwrap();
        assert_eq!(b.file_name().unwrap().to_string_lossy(), "新文件夹 (2)");
        assert_ne!(a, b);

        // 磁盘上已经有同名目录时也不许覆盖
        let c = ops.mkdir(&base, "已有").unwrap();
        assert_eq!(c.file_name().unwrap().to_string_lossy(), "已有 (2)");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 同名三选的真实验证：Skip 不动、KeepBoth 改名进去、plan 能提前报冲突。
    #[test]
    fn move_policies() {
        let base = std::env::temp_dir().join(format!("am-mv-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"new").unwrap();
        std::fs::write(dst.join("a.txt"), b"old").unwrap();

        let conn = crate::db::open(&base.join("t.db")).unwrap();
        let ops = FsOps::new(&conn);

        // 体检：目标已有同名，同盘不算跨盘
        let plan = plan_move(&[src.join("a.txt")], &dst).unwrap();
        assert_eq!(plan.conflicts, vec!["a.txt".to_string()]);
        assert_eq!(plan.files, 1);
        assert!(!plan.cross_volume);

        // Skip：一个都不动
        let r = ops.mv_with(&[src.join("a.txt")], &dst, MovePolicy::Skip).unwrap();
        assert_eq!(r.report.moved, 0);
        assert_eq!(r.report.skipped.len(), 1);
        assert!(src.join("a.txt").exists());

        // KeepBoth：改成「a (2).txt」进去，源还在原地就不合预期了 → 应该搬走
        let r = ops.mv_with(&[src.join("a.txt")], &dst, MovePolicy::KeepBoth).unwrap();
        assert_eq!(r.report.moved, 1);
        assert!(dst.join("a (2).txt").exists());
        assert!(!src.join("a.txt").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 施工图纸 0.3.16：**整个文件夹**也能当被移动对象（目录树里直接拖一个文件夹）。
    ///
    /// 钉住三件事：① 目录连内容一起搬走、源目录不再存在；
    /// ② `plan_move` 把它列进 `dirs`（前端据此提示"这是移动整个文件夹"）；
    /// ③ 拖进它自己的子目录必须**拒绝**，否则整个目录树会被搬进自己里，等于数据消失。
    #[test]
    fn folder_move_takes_the_whole_tree() {
        let base = std::env::temp_dir().join(format!("am-mvdir-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(src.join("品牌").join("子目录")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("品牌").join("a.txt"), b"a").unwrap();
        std::fs::write(src.join("品牌").join("子目录").join("b.txt"), b"b").unwrap();

        let conn = crate::db::open(&base.join("t.db")).unwrap();
        let ops = FsOps::new(&conn);

        let plan = plan_move(&[src.join("品牌")], &dst).unwrap();
        assert_eq!(plan.files, 2, "递归统计子文件");
        assert_eq!(plan.dirs.len(), 1, "目录要单独列出来");

        // 自己拖进自己的子目录：必须报错，且**什么都不动**
        let bad = ops.mv_with(&[src.join("品牌")], &src.join("品牌").join("子目录"), MovePolicy::Skip);
        assert!(bad.is_err(), "拖进自己的子目录必须拒绝");
        assert!(src.join("品牌").join("a.txt").exists(), "拒绝时源文件不能被碰");

        let r = ops.mv_with(&[src.join("品牌")], &dst, MovePolicy::Skip).unwrap();
        assert_eq!(r.report.moved, 1);
        assert!(dst.join("品牌").join("子目录").join("b.txt").exists(), "子目录要跟着走");
        assert!(!src.join("品牌").exists(), "源目录不该还在");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 后台移动（缺陷 #7）：逐项进度必须真的回调，且结束时的计数与磁盘一致。
    #[test]
    fn move_reports_progress_per_item() {
        let base = std::env::temp_dir().join(format!("am-mvp-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        for i in 0..5 {
            std::fs::write(src.join(format!("f{}.txt", i)), b"x").unwrap();
        }

        let conn = crate::db::open(&base.join("t.db")).unwrap();
        let ops = FsOps::new(&conn);
        let paths: Vec<std::path::PathBuf> =
            (0..5).map(|i| src.join(format!("f{}.txt", i))).collect();

        let mut ticks: Vec<(i64, i64)> = Vec::new();
        let r = ops
            .mv_with_progress(&paths, &dst, MovePolicy::Skip, |p| ticks.push((p.done, p.total)))
            .unwrap();

        assert_eq!(r.report.moved, 5);
        assert_eq!(ticks.last().map(|t| *t), Some((5, 5)), "最后一项必须报满");
        assert!(ticks.windows(2).all(|w| w[0].0 <= w[1].0), "进度只能往前走");
        for i in 0..5 {
            assert!(dst.join(format!("f{}.txt", i)).exists());
            assert!(!src.join(format!("f{}.txt", i)).exists());
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unique_dest_keeps_extension() {
        let dir = std::env::temp_dir().join("am-unique-test");
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("x.png");
        std::fs::write(&a, b"1").unwrap();
        let d = unique_dest(&dir, "x.png");
        assert_eq!(d.file_name().unwrap().to_string_lossy(), "x (2).png");
        let _ = std::fs::remove_file(&a);
    }

    /* ---- 打开位置（0.2.1）：这三条钉住 owner 报的「点位置打开的地方不对」 ---- */

    #[cfg(windows)]
    #[test]
    fn reveal_arg_quotes_only_the_path() {
        let f = std::env::temp_dir().join("am test 截图.png");
        let a = super::reveal_arg(&f);
        assert_eq!(a, format!("/select,\"{}\"", f.display()));
        // 整个参数被套引号（"/select,C:\..."）就是 explorer 退回「文档」的那条老 bug
        assert!(!a.starts_with("\"/select"), "整条参数不许被引号包住：{a}");
    }

    #[cfg(windows)]
    #[test]
    fn reveal_arg_enters_directory_instead_of_selecting_it() {
        let d = std::env::temp_dir();
        let a = super::reveal_arg(&d);
        assert_eq!(a, format!("\"{}\"", d.display()));
        assert!(!a.contains("/select"), "目录要进去，不是父目录高亮：{a}");
    }

    #[test]
    fn missing_path_is_reported_instead_of_opening_somewhere() {
        let p = std::env::temp_dir().join("am-missing-0.2.1-does-not-exist.png");
        let _ = std::fs::remove_file(&p);
        let e = super::reveal_in_explorer(&p).unwrap_err();
        assert!(e.contains("已经不在了"), "要给出人话原因，实际：{e}");
    }
}
