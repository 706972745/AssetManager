//! 应用内回收站（**照 FontManager 的回收机制**，owner 2026-11-17）。
//!
//! 和「直接丢系统回收站」的区别：
//! - 删除 → 文件**搬进** `<数据目录>/trash/`，旁边写一份 `<文件名>.amtrash` 的 JSON 账本；
//! - 回收站页面能列出「删了什么、原来在哪」，逐个 **还原** / **彻底删除**，或 **清空**；
//! - **彻底删除 / 清空** 才送 Windows 回收站（再兜一层，误点还有救）。
//!
//! 为什么用「文件 + 账本」而不是数据库表：账本就是文件自己的同伴，回收站目录被人手动
//! 拷走/删掉时状态不会和数据表打架；重建索引、换数据目录也不会丢记录 —— FontManager
//! 用同一套做法（`installer.rs` 的 `list_trash` / `restore`）。
//!
//! 纪律：
//! 1. **先写账本再动文件**：账本写失败 = 干净地什么都没发生，而不是"文件没了但没记录"。
//! 2. **不许覆盖用户文件**：还原时原位置若已有同名文件，改用一个不冲突的名字。
//! 3. **不信报错信文件系统**：跨盘搬运走「复制 + 删源」，中途报错时看目标在不在。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

type Res<T> = Result<T, String>;

/// 账本后缀（`.amtrash`）—— 回收站目录里只有带这个后缀的才是记录
const LEDGER_EXT: &str = "amtrash";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    /// 唯一键：回收站里那个文件的完整路径
    pub id: String,
    pub original_path: String,
    pub trashed_path: String,
    pub name: String,
    pub size: u64,
    /// 秒级时间戳
    pub deleted_at: u64,
}

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("trash")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn ledger(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".");
    s.push(LEDGER_EXT);
    PathBuf::from(s)
}

/// 同目录下找一个不冲突的名字（`a.png` → `a (2).png`）
fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|s| s.to_string_lossy().to_string());
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    for n in 2..10_000 {
        let cand = match &ext {
            Some(e) => format!("{} ({}).{}", stem, n, e),
            None => format!("{} ({})", stem, n),
        };
        let path = dir.join(cand);
        if !path.exists() {
            return path;
        }
    }
    dir.join(format!("{}-{}", now(), name))
}

/// 搬一个文件/目录：同盘 rename，跨盘「复制 + 删源」。
fn move_path(src: &Path, dest: &Path) -> Res<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
    }
    if fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    // 跨盘：rename 会报 ERROR_NOT_SAME_DEVICE / EXDEV
    if src.is_dir() {
        copy_dir(src, dest)?;
        fs::remove_dir_all(src).map_err(|e| format!("源目录删不掉：{e}"))?;
    } else {
        fs::copy(src, dest).map_err(|e| format!("复制失败：{e}"))?;
        fs::remove_file(src).map_err(|e| format!("源文件删不掉：{e}"))?;
    }
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> Res<()> {
    fs::create_dir_all(dest).map_err(|e| format!("建目录失败：{e}"))?;
    for e in fs::read_dir(src).map_err(|e| format!("读目录失败：{e}"))?.flatten() {
        let from = e.path();
        let to = dest.join(e.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("复制失败：{e}"))?;
        }
    }
    Ok(())
}

/// 删除 = 搬进回收站。返回每条记录（顺序与入参一致，跳过的项不在结果里）。
pub fn put(data_dir: &Path, paths: &[PathBuf]) -> Res<Vec<TrashEntry>> {
    let root = dir(data_dir);
    fs::create_dir_all(&root).map_err(|e| format!("建回收站目录失败：{e}"))?;
    let mut out = Vec::new();
    for p in paths {
        if !p.exists() {
            continue; // 已经不在了：没什么可回收的
        }
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "未命名".into());
        let dest = unique_dest(&root, &name);
        let size = if p.is_file() {
            fs::metadata(p).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        let entry = TrashEntry {
            id: dest.to_string_lossy().into_owned(),
            original_path: p.to_string_lossy().into_owned(),
            trashed_path: dest.to_string_lossy().into_owned(),
            name: name.clone(),
            size,
            deleted_at: now(),
        };
        /* 先落账：此刻文件还原地没动，账本写失败就是干净的「什么都没发生」 */
        let side = ledger(&dest);
        fs::write(&side, serde_json::to_vec(&entry).unwrap_or_default())
            .map_err(|e| format!("写入回收站记录失败：{e}"))?;
        if let Err(e) = move_path(p, &dest) {
            // 撤摊子：账本和半截副本都收掉，原文件原样留着
            let _ = fs::remove_file(&side);
            let _ = fs::remove_file(&dest);
            return Err(e);
        }
        out.push(entry);
    }
    Ok(out)
}

/// 列出回收站（新的在前）。账本坏掉或文件被人手动删掉的记录直接跳过。
pub fn list(data_dir: &Path) -> Vec<TrashEntry> {
    let root = dir(data_dir);
    let Ok(rd) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<TrashEntry> = rd
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x.eq_ignore_ascii_case(LEDGER_EXT))
                .unwrap_or(false)
        })
        .filter_map(|e| fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<TrashEntry>(&b).ok())
        .filter(|t| Path::new(&t.trashed_path).exists())
        .collect();
    out.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
    out
}

/// 还原：搬回原路径（父目录不在就建）。原位置已有同名文件时**改用不冲突的名字**。
/// 返回实际落盘的新路径（调用方拿它刷新索引）。
pub fn restore(data_dir: &Path, ids: &[String]) -> Res<Vec<String>> {
    restore_to(data_dir, ids, None)
}

/// 还原到**指定目录**（`to = None` 就是回原位置）。
///
/// 为什么需要它（owner 2026-11-19）：截图贴图默认进回收站，用户觉得这张值得留，
/// 应该能一句话"存到我的素材文件夹"，而不是先还原到 screenshots 再手动搬一次。
pub fn restore_to(data_dir: &Path, ids: &[String], to: Option<&Path>) -> Res<Vec<String>> {
    let all = list(data_dir);
    let mut restored = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    if let Some(d) = to {
        fs::create_dir_all(d).map_err(|e| format!("建目标目录失败：{e}"))?;
    }
    for id in ids {
        let Some(entry) = all.iter().find(|t| &t.id == id) else {
            errors.push(format!("找不到回收站记录：{id}"));
            continue;
        };
        let orig = match to {
            Some(d) => d.join(&entry.name),
            None => PathBuf::from(&entry.original_path),
        };
        let target = if orig.exists() {
            let parent = orig.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            unique_dest(&parent, &entry.name)
        } else {
            orig
        };
        match move_path(Path::new(&entry.trashed_path), &target) {
            Ok(_) => {
                let _ = fs::remove_file(ledger(Path::new(&entry.trashed_path)));
                restored.push(target.to_string_lossy().into_owned());
            }
            Err(e) => errors.push(format!("{}：{}", entry.name, e)),
        }
    }
    if restored.is_empty() && !errors.is_empty() {
        return Err(errors.join("；"));
    }
    Ok(restored)
}

/// 彻底删除：送 Windows 回收站（再兜一层），然后删账本。
pub fn purge(data_dir: &Path, ids: &[String]) -> Res<usize> {
    let all = list(data_dir);
    let mut done = 0;
    let mut errors: Vec<String> = Vec::new();
    for id in ids {
        let Some(entry) = all.iter().find(|t| &t.id == id) else {
            continue;
        };
        let p = Path::new(&entry.trashed_path);
        // 报错但文件已经没了 = 成功（IFileOperation 的 aborted 是常态）
        if let Err(e) = trash::delete(p) {
            if p.exists() {
                errors.push(format!("{}：{}", entry.name, e));
                continue;
            }
        }
        let _ = fs::remove_file(ledger(p));
        done += 1;
    }
    if done == 0 && !errors.is_empty() {
        return Err(errors.join("；"));
    }
    Ok(done)
}

/// 清空回收站
pub fn empty(data_dir: &Path) -> Res<usize> {
    let ids: Vec<String> = list(data_dir).into_iter().map(|t| t.id).collect();
    if ids.is_empty() {
        // 顺手把空目录/残留账本收掉
        let _ = fs::remove_dir_all(dir(data_dir));
        return Ok(0);
    }
    let n = purge(data_dir, &ids)?;
    let _ = fs::remove_dir_all(dir(data_dir));
    Ok(n)
}

/// 自动清理：删掉进回收站超过 `days` 天的条目（0.5.7，默认 7 天）。
///
/// 判据只用账本里的 `deleted_at`（现有字段，不改存储格式）——文件的 mtime 可能被
/// 复制/解压改过，而"什么时候被删的"是账本独有的信息。
/// `days == 0` = 永不自动清理（owner 语义：给用户一个明确的"别动我的回收站"）。
///
/// 走 `purge()` 而不是直接 `fs::remove_file`：和手动「彻底删除 / 清空」同一条路，
/// 先进 Windows 回收站兜一层 —— 自动清理误伤时还有一次捞回来的机会。
/// 只吃 `data_dir/trash` 下的账本条目，绝不碰素材库和 `screenshots`。
pub fn purge_older_than(data_dir: &Path, days: u64) -> Res<usize> {
    if days == 0 {
        return Ok(0);
    }
    let cutoff = now().saturating_sub(days.saturating_mul(24 * 60 * 60));
    let ids: Vec<String> = list(data_dir)
        .into_iter()
        .filter(|t| t.deleted_at < cutoff)
        .map(|t| t.id)
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }
    purge(data_dir, &ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("am-trash-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn put_list_restore_roundtrip() {
        let root = tmp("roundtrip");
        let data = root.join("data");
        let lib = root.join("lib");
        fs::create_dir_all(&lib).unwrap();
        let file = lib.join("a.png");
        fs::write(&file, b"hello").unwrap();

        let entries = put(&data, &[file.clone()]).expect("put");
        assert_eq!(entries.len(), 1);
        assert!(!file.exists(), "原文件应该已经搬走");
        assert!(Path::new(&entries[0].trashed_path).exists());

        let listed = list(&data);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "a.png");
        assert_eq!(listed[0].size, 5);
        assert_eq!(listed[0].original_path, file.to_string_lossy());

        let back = restore(&data, &[entries[0].id.clone()]).expect("restore");
        assert_eq!(back.len(), 1);
        assert!(file.exists(), "还原后应该回到原位置");
        assert!(list(&data).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_never_overwrites_user_file() {
        let root = tmp("nooverwrite");
        let data = root.join("data");
        let lib = root.join("lib");
        fs::create_dir_all(&lib).unwrap();
        let file = lib.join("a.png");
        fs::write(&file, b"old").unwrap();
        let entries = put(&data, &[file.clone()]).unwrap();
        // 删完之后用户在原位置放了同名的另一个文件
        fs::write(&file, b"new").unwrap();

        let back = restore(&data, &[entries[0].id.clone()]).unwrap();
        assert_eq!(back.len(), 1);
        assert_ne!(back[0], file.to_string_lossy(), "不能占用原路径");
        assert_eq!(fs::read(&file).unwrap(), b"new", "用户文件不能被覆盖");
        assert_eq!(fs::read(Path::new(&back[0])).unwrap(), b"old");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_to_custom_dir() {
        let root = tmp("to-dir");
        let data = root.join("data");
        let lib = root.join("lib");
        let out = root.join("out");
        fs::create_dir_all(&lib).unwrap();
        let file = lib.join("shot.png");
        fs::write(&file, b"png").unwrap();
        let entries = put(&data, &[file.clone()]).unwrap();

        let back = restore_to(&data, &[entries[0].id.clone()], Some(&out)).expect("restore_to");
        assert_eq!(back.len(), 1);
        assert_eq!(Path::new(&back[0]).parent(), Some(out.as_path()));
        assert_eq!(fs::read(&back[0]).unwrap(), b"png");
        assert!(!file.exists(), "不能再回原位置");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ledger_survives_missing_file() {
        let root = tmp("missing");
        let data = root.join("data");
        let lib = root.join("lib");
        fs::create_dir_all(&lib).unwrap();
        let file = lib.join("b.png");
        fs::write(&file, b"x").unwrap();
        let entries = put(&data, &[file]).unwrap();
        // 有人手动把回收站里的文件删了（账本还在）
        fs::remove_file(&entries[0].trashed_path).unwrap();
        assert!(list(&data).is_empty(), "文件不在的记录不该出现在列表里");
        let _ = fs::remove_dir_all(&root);
    }

    /// 造一条"`age_days` 天前被删"的回收站记录（文件 + 账本直接摆好，
    /// 不走 `put`：`put` 的 `deleted_at` 永远是"现在"，没法测时间判断）。
    fn plant(data: &Path, name: &str, age_days: u64) -> PathBuf {
        let root = dir(data);
        fs::create_dir_all(&root).unwrap();
        let file = root.join(name);
        fs::write(&file, b"x").unwrap();
        let entry = TrashEntry {
            id: file.to_string_lossy().into_owned(),
            original_path: file.to_string_lossy().into_owned(),
            trashed_path: file.to_string_lossy().into_owned(),
            name: name.to_string(),
            size: 1,
            deleted_at: now().saturating_sub(age_days * 24 * 60 * 60),
        };
        fs::write(ledger(&file), serde_json::to_vec(&entry).unwrap()).unwrap();
        file
    }

    #[test]
    fn purge_older_than_deletes_only_expired() {
        let root = tmp("retention");
        let data = root.join("data");
        let old = plant(&data, "old.png", 8); // 8 天前：超过 7 天，该删
        let recent = plant(&data, "recent.png", 6); // 6 天前：还没到期，该留
        let n = purge_older_than(&data, 7).expect("purge_older_than");
        assert_eq!(n, 1, "只该删掉过期的这一条");
        assert!(!old.exists(), "过期条目应被清理");
        assert!(recent.exists(), "未到期条目不能被碰");
        assert_eq!(list(&data).len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn purge_older_than_zero_keeps_everything() {
        let root = tmp("retention-zero");
        let data = root.join("data");
        let old = plant(&data, "old.png", 365);
        let n = purge_older_than(&data, 0).expect("purge_older_than");
        assert_eq!(n, 0, "0 = 永不自动清理");
        assert!(old.exists());
        assert_eq!(list(&data).len(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
