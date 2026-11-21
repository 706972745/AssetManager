//! 素材库文件夹图标（`desktop.ini` + `folder.ico`）。
//!
//! 职责：让资源管理器里**每个素材库根目录**显示我们那枚 "A / files" 图标，
//! 而不是系统默认的黄文件夹。三样东西缺一不可：
//!
//! 1. `<库根>/.AssetManager/folder.ico` —— 图标文件。**必须放在 meta 目录里**：
//!    扫描器已经跳过 `.AssetManager`（见 `scan::is_meta_dir`），放库根会被当成一张素材；
//! 2. `<库根>/desktop.ini` —— **UTF-16LE + BOM**（Windows 只认这个编码），内容
//!    `[.ShellClassInfo]` + `IconResource=.AssetManager\folder.ico,0`。
//!    用相对路径：库整体搬到别的盘也不会失效；
//! 3. 属性 —— 库根要 `SYSTEM`（否则 Explorer 根本不读 desktop.ini），
//!    ini 要 `HIDDEN|SYSTEM`（否则用户在资源管理器里会看到一个 desktop.ini）。
//!
//! 边界：
//! - **用户改过就不碰**：desktop.ini 存在但不是我们写的那份（内容里没有那行 IconResource）
//!   → `Kept`，一个字节都不改 —— 这是最重要的一条，改别人的配置属于数据事故；
//! - 只操作**库根这一层**：不递归、不碰素材、不动 `.AssetManager/lib.db` 等元数据；
//! - 只读盘 / 网络盘 / 权限不足 → `Failed(人话)`，调用方记日志即可，**不弹窗**；
//! - 卸载程序也不碰库目录（库是用户数据，卸载不该进去删东西）。
//!
//! 硬约束：不引新 crate —— `GetFileAttributesW` / `SetFileAttributesW` 手写
//! `extern "system"`，和 `fsops` / `shellthumb` 同一条路线。

use std::path::{Path, PathBuf};

/// 图标文件名（放 `<库根>/.AssetManager/` 里）
pub const ICO_NAME: &str = "folder.ico";
/// 配置文件名（放库根）
pub const INI_NAME: &str = "desktop.ini";
/// 我们写进 ini 的那一行。**判断"是不是我们那份"就靠它**
pub const ICON_LINE: &str = r"IconResource=.AssetManager\folder.ico,0";

/// 图标本体（7 帧，58,205 B；与 `src-tauri/icons/file-types/folder.ico` 同一份）
const ICO_BYTES: &[u8] = include_bytes!("../assets/folder.ico");

/// `ensure()` 的结果。`Kept` = 什么都没动（已是最新，或者用户自己那份）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconState {
    /// 之前没有 desktop.ini，这次写上了
    Applied,
    /// 有我们的 ini，但内容 / 图标变了，重写了
    Updated,
    /// 没动：已经是最新，或者桌面配置是用户自己的
    Kept,
    /// 失败（人话原因）。调用方记日志即可，不要弹窗
    Failed(String),
}

/* ---------------- Win32 属性（手写声明，零新依赖） ---------------- */

#[cfg(windows)]
mod win {
    pub const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    pub const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    pub const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;

    extern "system" {
        fn GetFileAttributesW(path: *const u16) -> u32;
        fn SetFileAttributesW(path: *const u16, attrs: u32) -> i32;
    }

    pub fn wide(p: &std::path::Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// 读文件/目录的属性位。读不到返回 `None`。
    pub fn attrs(p: &std::path::Path) -> Option<u32> {
        let w = wide(p);
        let a = unsafe { GetFileAttributesW(w.as_ptr()) };
        if a == INVALID_FILE_ATTRIBUTES {
            None
        } else {
            Some(a)
        }
    }

    /// 直接写整套属性位。
    pub fn set_attrs(p: &std::path::Path, attrs: u32) -> bool {
        let w = wide(p);
        unsafe { SetFileAttributesW(w.as_ptr(), attrs) != 0 }
    }
}

/// 给路径加上 `SYSTEM`（保留其它位）。已存在就不写 —— 省一次属性变更。
#[cfg(windows)]
fn add_system(p: &Path) -> Result<(), String> {
    let Some(cur) = win::attrs(p) else {
        return Err(format!("读不到属性：{}", p.display()));
    };
    if cur & win::FILE_ATTRIBUTE_SYSTEM != 0 {
        return Ok(());
    }
    if !win::set_attrs(p, cur | win::FILE_ATTRIBUTE_SYSTEM) {
        return Err(format!(
            "改不了属性：{}（只读盘或没有权限）",
            p.display()
        ));
    }
    Ok(())
}

/// 去掉 `SYSTEM`（保留其它位）。
#[cfg(windows)]
fn clear_system(p: &Path) -> Result<(), String> {
    let Some(cur) = win::attrs(p) else {
        return Err(format!("读不到属性：{}", p.display()));
    };
    if cur & win::FILE_ATTRIBUTE_SYSTEM == 0 {
        return Ok(());
    }
    if !win::set_attrs(p, cur & !win::FILE_ATTRIBUTE_SYSTEM) {
        return Err(format!("改不了属性：{}", p.display()));
    }
    Ok(())
}

/// 把 ini 设成恰好 `HIDDEN | SYSTEM`（不留其它位：它不该是只读，我们还要能改它）。
#[cfg(windows)]
fn set_ini_attrs(p: &Path) -> Result<(), String> {
    let hidden_system = win::FILE_ATTRIBUTE_HIDDEN | win::FILE_ATTRIBUTE_SYSTEM;
    let keep = win::attrs(p).unwrap_or(0) & !(win::FILE_ATTRIBUTE_HIDDEN | win::FILE_ATTRIBUTE_SYSTEM);
    if !win::set_attrs(p, keep | hidden_system) {
        return Err(format!("改不了属性：{}（只读盘或没有权限）", p.display()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn add_system(_p: &Path) -> Result<(), String> {
    Ok(())
}
#[cfg(not(windows))]
fn clear_system(_p: &Path) -> Result<(), String> {
    Ok(())
}
#[cfg(not(windows))]
fn set_ini_attrs(_p: &Path) -> Result<(), String> {
    Ok(())
}

/* ---------------- ini 内容 ---------------- */

/// 我们写出去的 ini：UTF-16LE + BOM。**不要用 UTF-8** —— 那种 Explorer 不认。
fn ini_bytes() -> Vec<u8> {
    let text = format!("[.ShellClassInfo]\r\n{ICON_LINE}\r\n");
    let mut out = Vec::with_capacity(2 + text.len() * 2);
    out.extend_from_slice(&[0xFF, 0xFE]); // UTF-16LE BOM
    for u in text.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// 把 ini 的字节读成字符串（UTF-16LE/BE 带 BOM 或 UTF-8 都认）。
/// 只用于"这份是不是我们写的"判断，不要求还原得一字节不差。
fn decode_ini(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let words: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&words);
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let words: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&words);
    }
    String::from_utf8_lossy(bytes).to_string()
}

/// 这份 desktop.ini 是我们写的吗？（我们写的那行是固定的，逐字匹配即可）
fn is_ours(text: &str) -> bool {
    text.contains(ICON_LINE)
}

/// 图标文件的落点：`<库根>/.AssetManager/folder.ico`
pub fn ico_path(root: &Path) -> PathBuf {
    crate::libhome::dir_for(&root.to_string_lossy()).join(ICO_NAME)
}

/// 内容相同就不写盘（每次启动都会走一遍，别白白写 35 KB）。
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if let Ok(existing) = std::fs::read(path) {
        if existing == bytes {
            return Ok(false);
        }
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

/* ---------------- 对外：设置 / 摘除 ---------------- */

/// 给一个素材库根目录设上图标。**幂等**，失败只返回 `Failed`，不 panic、不阻断调用方。
pub fn ensure(root: &Path) -> IconState {
    match ensure_inner(root) {
        Ok(s) => s,
        Err(e) => IconState::Failed(e),
    }
}

fn ensure_inner(root: &Path) -> Result<IconState, String> {
    if !root.is_dir() {
        return Err(format!("库目录不存在：{}", root.display()));
    }
    let ini = root.join(INI_NAME);

    /* ① 用户自己那份 → 立刻收手（一个字节都不改，连图标也不写：写了也没人引用） */
    let mut had_ini = false;
    if ini.is_file() {
        had_ini = true;
        let bytes = std::fs::read(&ini)
            .map_err(|e| format!("读不了 {}（{e}）", ini.display()))?;
        if !is_ours(&decode_ini(&bytes)) {
            return Ok(IconState::Kept);
        }
    } else if ini.exists() {
        /* 同名但不是普通文件（文件夹 / 符号链接）：不是我们该动的东西 */
        return Ok(IconState::Kept);
    }

    /* ② 图标文件 */
    let ico = ico_path(root);
    let mut changed = false;
    if let Some(dir) = ico.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("建不了 {}（{e}）", dir.display()))?;
    }
    if write_if_changed(&ico, ICO_BYTES)
        .map_err(|e| format!("写不了图标文件 {}（{e}）", ico.display()))?
    {
        changed = true;
    }

    /* ③ desktop.ini：内容不同才写 */
    let want = ini_bytes();
    let same = std::fs::read(&ini).map(|b| b == want).unwrap_or(false);
    if !same {
        std::fs::write(&ini, &want)
            .map_err(|e| format!("写不了 {}（{e}）——库可能在只读盘上", ini.display()))?;
        changed = true;
    }

    /* ④ 属性：ini 必须隐藏+系统，库根必须带 SYSTEM（否则 Explorer 不读 ini） */
    set_ini_attrs(&ini)?;
    add_system(root)?;

    Ok(if !had_ini {
        IconState::Applied
    } else if changed {
        IconState::Updated
    } else {
        IconState::Kept
    })
}

/// 摘掉**我们自己设的**图标（移除库时调用）。
///
/// 只删"是我们的"那份 ini；用户自己改过的一律不碰。
pub fn remove(root: &Path) -> Result<(), String> {
    let ini = root.join(INI_NAME);
    let mut ours = false;
    if ini.is_file() {
        if let Ok(bytes) = std::fs::read(&ini) {
            ours = is_ours(&decode_ini(&bytes));
        }
    }
    if !ours {
        /* 不是我们写的：图标文件也不删（说不定用户自己引用了它），属性也不动 */
        return Ok(());
    }
    std::fs::remove_file(&ini).map_err(|e| format!("删不掉 {}（{e}）", ini.display()))?;
    let ico = ico_path(root);
    if ico.is_file() {
        std::fs::remove_file(&ico).map_err(|e| format!("删不掉 {}（{e}）", ico.display()))?;
    }
    /* 属性最后撤：ini 已经没了，目录再留着 SYSTEM 就成了"看不出原因的系统文件夹" */
    clear_system(root)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-icon-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn attrs_bits(p: &Path) -> u32 {
        #[cfg(windows)]
        {
            win::attrs(p).unwrap_or(0)
        }
        #[cfg(not(windows))]
        {
            let _ = p;
            0
        }
    }

    /// 新建库：图标落进 `.AssetManager/`、ini 内容正确、属性齐活。
    #[test]
    fn ensure_writes_ini_and_sets_attributes() {
        let d = tmp("new");
        let st = ensure(&d);
        assert_eq!(st, IconState::Applied, "第一次应当 Applied：{st:?}");

        let ini = d.join(INI_NAME);
        assert!(ini.is_file(), "desktop.ini 要写在库根");
        let raw = std::fs::read(&ini).unwrap();
        assert_eq!(&raw[0..2], &[0xFF, 0xFE], "必须是 UTF-16LE + BOM");
        let text = decode_ini(&raw);
        assert!(text.contains("[.ShellClassInfo]"), "{text}");
        assert!(text.contains(ICON_LINE), "{text}");

        let ico = d.join(crate::libhome::DIR).join(ICO_NAME);
        assert!(ico.is_file(), "图标要在 .AssetManager/ 里（扫描器会跳过它）");
        assert_eq!(std::fs::read(&ico).unwrap(), ICO_BYTES, "图标内容要原样写入");

        #[cfg(windows)]
        {
            assert_ne!(
                attrs_bits(&d) & win::FILE_ATTRIBUTE_SYSTEM,
                0,
                "库根必须带 SYSTEM，否则 Explorer 不读 desktop.ini"
            );
            let a = attrs_bits(&ini);
            assert_ne!(a & win::FILE_ATTRIBUTE_HIDDEN, 0, "ini 要隐藏");
            assert_ne!(a & win::FILE_ATTRIBUTE_SYSTEM, 0, "ini 要系统");
        }

        /* 幂等：再跑一次什么都不改 */
        assert_eq!(ensure(&d), IconState::Kept);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 用户自己写过一版 → 一个字节都不动。
    #[test]
    fn ensure_keeps_user_customized_ini() {
        let d = tmp("user");
        let ini = d.join(INI_NAME);
        std::fs::write(&ini, "[.ShellClassInfo]\r\nIconResource=shell32.dll,3\r\n").unwrap();
        let before = std::fs::read(&ini).unwrap();

        assert_eq!(ensure(&d), IconState::Kept);
        assert_eq!(std::fs::read(&ini).unwrap(), before, "用户那份不许改");
        assert!(
            !d.join(crate::libhome::DIR).join(ICO_NAME).exists(),
            "用户那份在用时，不该往库里塞图标文件"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 移除库：只摘我们自己的，用户那份原样留着。
    #[test]
    fn remove_restores_only_our_icon() {
        let d = tmp("remove");
        ensure(&d);
        assert!(d.join(INI_NAME).is_file());
        remove(&d).unwrap();
        assert!(!d.join(INI_NAME).exists(), "我们的 ini 要删掉");
        assert!(
            !d.join(crate::libhome::DIR).join(ICO_NAME).exists(),
            "图标文件也要删掉"
        );
        #[cfg(windows)]
        assert_eq!(
            attrs_bits(&d) & win::FILE_ATTRIBUTE_SYSTEM,
            0,
            "目录的 SYSTEM 要去掉"
        );

        let d2 = tmp("remove-user");
        let ini = d2.join(INI_NAME);
        std::fs::write(&ini, "[.ShellClassInfo]\r\nIconResource=shell32.dll,3\r\n").unwrap();
        remove(&d2).unwrap();
        assert!(ini.is_file(), "用户那份不许删");

        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    /// 加了库图标之后，`desktop.ini` 不能被当成一张素材扫进索引。
    #[test]
    fn scan_skips_desktop_ini() {
        let d = tmp("scan");
        let lib = d.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("a.png"), b"x").unwrap();
        /* 真的走一遍 ensure：ini 会带着 HIDDEN|SYSTEM 落地 */
        assert_eq!(ensure(&lib), IconState::Applied);

        let mut app = crate::App::open(d.join("data")).unwrap();
        let s = app.scan(&lib.to_string_lossy(), |_| {}).unwrap();
        assert!(s.skipped >= 1, "desktop.ini 应当被跳过：{s:?}");
        assert_eq!(app.total().unwrap(), 1, "素材数只能是那张 png");

        let _ = std::fs::remove_dir_all(&d);
    }
}
