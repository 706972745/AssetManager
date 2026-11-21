//! 双击导入时的「这是什么文件」判定 —— **只看内容，不看扩展名**。
//!
//! 职责：把一个用户双击（或从命令行传进来）的文件归到三类之一：
//! 用户数据（`.D-AssetManager`，0.4.5 起；旧的 `.L-AssetManager` 仍认）/ 快照清单（`.S-AssetManager`）/ 资源包（`.F-AssetManager`，0.4.24 起；旧的 `.P-AssetManager` 仍认）。
//!
//! 边界：
//! - 只读文件头 / 解析顶层 JSON 键，**不改文件、不写索引**（落库由调用方做）；
//! - 不认"扩展名对但内容是别的"的文件，也不认"扩展名被用户改过"的文件 ——
//!   扩展名只用于给人看和文件关联，真正的类型判据在这里；
//! - 错误一律说**人话**（"这个文件不像是 AssetManager 导出的数据"），
//!   不把 `os error 2` 之类甩给用户。
//!
//! 硬约束：
//! - 判定顺序固定：zip 魔数 → JSON 顶层键。**别靠文件名**（用户会改名、会漏后缀）；
//! - 三类导出文件的内容格式不许在这里"顺手兼容"——要放宽就改各自的解析端。

use std::path::Path;

/// 三种可双击导入的文件
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportKind {
    /// `.D-AssetManager`：用户层数据（库根 / 评分 / 备注 / 来源 / 标签）。
    /// 0.4.5 把后缀从 `.L` 改成 `.D`；判定只看内容，所以老 `.L` 文件照样认得。
    UserData,
    /// `.S-AssetManager`：快照清单（只有哈希与元数据，不含图片字节）
    Snapshot,
    /// `.F-AssetManager`：资源包（store-only zip，里面是原图）。0.4.24 把后缀从 `.P` 改成 `.F`；
    /// 判定只看内容，所以老 `.P` 包照样认得。
    Package,
}

impl ImportKind {
    /// 给界面用的短名（前端 toast / 确认框里那句"是什么"）
    pub fn label(&self) -> &'static str {
        match self {
            ImportKind::UserData => "用户数据",
            ImportKind::Snapshot => "快照清单",
            ImportKind::Package => "资源包",
        }
    }
}

/// zip 本地文件头签名（`PK\x03\x04`）。store-only 的包第一件事就是它。
const ZIP_LOCAL_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

/// 读不出来的统一措辞：把"为什么"说清楚，并给出"怎么办"。
fn not_ours(detail: &str) -> String {
    format!("这个文件不像是 AssetManager 导出的数据（{detail}）。请确认没有选错文件。")
}

/// 判定一个文件属于哪一类导出。
///
/// 规则（顺序固定）：
/// 1. 前 4 字节是 `PK\x03\x04` → 资源包；
/// 2. 否则按 UTF-8 读全文并解析 JSON：
///    - `libs` 是数组 → 用户数据（`App::export_userdata` 的顶层就有它）；
///    - `items` 是数组 → 快照清单（`SnapFile` 的条目数组）；
///    - `app`+`kind` 是快照标记 → 快照清单（空快照的 `items` 也是空数组，但键在）；
/// 3. 都不是 / 读不出来 → 中文人话的错误。
pub fn sniff(path: &Path) -> Result<ImportKind, String> {
    if !path.exists() {
        return Err(format!(
            "找不到这个文件：{}。它可能被移动或删除了。",
            path.display()
        ));
    }
    if !path.is_file() {
        return Err("这是一个文件夹，不是 AssetManager 导出的文件。".into());
    }

    /* ① 只看头 4 个字节：包可能是几百 MB，别为了判定类型把整包读进内存 */
    let head = read_head(path, 4)?;
    if head.len() >= 4 && head[..4] == ZIP_LOCAL_MAGIC {
        return Ok(ImportKind::Package);
    }

    /* ② 文本类：整份读（导出文件是几百 KB 到几 MB 的 JSON，整读最省事） */
    let bytes = std::fs::read(path)
        .map_err(|e| format!("读不了这个文件（{e}）。请确认它没有被别的程序占用。"))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        not_ours("前几个字节既不是 zip，也不是文本")
    })?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| not_ours(&format!("不是合法的 JSON：{e}")))?;

    if v.get("libs").map(|x| x.is_array()).unwrap_or(false) {
        return Ok(ImportKind::UserData);
    }
    if v.get("items").map(|x| x.is_array()).unwrap_or(false) {
        return Ok(ImportKind::Snapshot);
    }
    /* 空快照也是合法快照，但别的软件的 json 也可能有 app/kind 两个键，所以要求两个都对上 */
    let is_snap = v.get("app").and_then(|x| x.as_str()) == Some("AssetManager")
        && v.get("kind").and_then(|x| x.as_str()) == Some("snapshot");
    if is_snap {
        return Ok(ImportKind::Snapshot);
    }
    Err(not_ours("顶层既没有 libs，也没有 items"))
}

/// 读文件开头 n 个字节（文件比 n 短就返回实际读到的）。
fn read_head(path: &Path, n: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)
        .map_err(|e| format!("打不开这个文件（{e}）。请确认它没有被别的程序占用。"))?;
    let mut buf = vec![0u8; n];
    let mut got = 0usize;
    while got < n {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(e) => return Err(format!("读不了这个文件（{e}）。")),
        }
    }
    buf.truncate(got);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-kind-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d.join(name)
    }

    #[test]
    fn sniff_userdata_json() {
        let p = tmp("u.D-AssetManager");
        std::fs::write(
            &p,
            r#"{"app":"AssetManager","libs":["D:\\a"],"assets":[]}"#,
        )
        .unwrap();
        assert_eq!(sniff(&p).unwrap(), ImportKind::UserData);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sniff_snapshot_json_even_when_empty() {
        let p = tmp("s.S-AssetManager");
        std::fs::write(
            &p,
            r#"{"app":"AssetManager","kind":"snapshot","version":1,"name":"参考","items":[]}"#,
        )
        .unwrap();
        assert_eq!(sniff(&p).unwrap(), ImportKind::Snapshot);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sniff_package_by_magic_not_by_name() {
        /* 名字故意写成 .json：判定必须靠内容 */
        let p = tmp("pkg.json");
        let mut bytes = vec![0x50u8, 0x4b, 0x03, 0x04];
        bytes.extend_from_slice(&[0u8; 64]);
        std::fs::write(&p, &bytes).unwrap();
        assert_eq!(sniff(&p).unwrap(), ImportKind::Package);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sniff_rejects_foreign_json_and_binary() {
        let p = tmp("other.json");
        std::fs::write(&p, r#"{"hello":"world"}"#).unwrap();
        let e = sniff(&p).unwrap_err();
        assert!(e.contains("不像是 AssetManager 导出的数据"), "{e}");
        std::fs::write(&p, [0xffu8, 0x00, 0x01, 0x02, 0x03]).unwrap();
        assert!(sniff(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sniff_missing_file_says_where() {
        let p = tmp("nope.D-AssetManager");
        let _ = std::fs::remove_file(&p);
        let e = sniff(&p).unwrap_err();
        assert!(e.contains("找不到这个文件"), "{e}");
    }
}
