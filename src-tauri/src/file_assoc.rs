#![cfg(target_os = "windows")]
//! 四种导出文件的 Windows 文件关联 + 各自的文件图标。
//!
//! 职责：把 `.D-AssetManager`（用户数据，0.4.5 起；`.L` 作为旧格式兼容保留）/
//! `.S-AssetManager`（快照清单）/ `.F-AssetManager`（资产包，0.4.24 起；`.P` 作为旧格式兼容保留）
//! 注册到 HKCU，让资源管理器双击时以 `--open "%1"` 启动本程序，
//! 并且**列表里显示各自的图标**（data / snapshot / pack）。
//!
//! 边界：
//! - **只写 HKCU**（`Software\Classes\...`），不需要管理员权限，也不动系统级 HKCR；
//! - 图标不生成、不重绘：三份 ico 就是 `src-tauri/icons/file-types/` 里那三份，
//!   原样 `include_bytes!` 嵌进 exe，运行期写到磁盘（安装目录随卸载一起消失）；
//! - 值没变就不写、也不通知资源管理器刷新 —— 这段代码每次启动都跑，不能每次都
//!   逼 Explorer 重建图标缓存（FontManager 1.7.3 踩过：每次启动图标闪一片白）。
//!
//! 硬约束：
//! - **不引 `windows-sys` / `windows` 这类新 crate**（本地仓库没有缓存，引了就打不出包）：
//!   这里用到的 6 个入口全部手写 `extern "system"` 声明，句柄一律 `isize`；
//! - 图标落点必须**先试 exe 同目录、不可写才退 `%LOCALAPPDATA%\AssetManager\icons`**：
//!   装到 Program Files（机器级安装）时 exe 目录只读，写不进去会让图标整块失效；
//! - 失败只返回 Err 由调用方记日志、不阻断启动（关联写不进去，软件照样能用）。

use std::path::{Path, PathBuf};

/// 一个扩展名的全部注册信息
struct Assoc {
    /// 不带点（连字符在 Windows 扩展名里是合法字符）
    ext: &'static str,
    /// ProgID：`AssetManager.<L|S|P>`
    prog_id: &'static str,
    /// 写到磁盘上的 ico 文件名（**改名不改内容**：同名文件会被内容比较命中，省一次写盘）
    icon_name: &'static str,
    icon_bytes: &'static [u8],
}

/// 导出文件各自的图标（owner 指定：D→data / S→snapshot / F,P→pack，原样使用）。
///
/// **`.L` / `.P` 为什么还留着**：0.4.5 起导出改用 `.D`（`.L` 是旧名），
/// 0.4.24 起资产包改用 `.F`（`.P` 是旧名）。用户手上已有的旧备份
/// 不该因为升级就双击失效，所以这两个旧后缀只作为**只读兼容**保留一条关联；
/// 程序里没有任何地方再产出它们（`file_kind` 判定本来就不看扩展名）。
const ASSOCS: [Assoc; 5] = [
    Assoc {
        ext: "D-AssetManager",
        prog_id: "AssetManager.D",
        icon_name: "assetmanager-data.ico",
        icon_bytes: include_bytes!("../icons/file-types/data.ico"),
    },
    Assoc {
        ext: "L-AssetManager",
        prog_id: "AssetManager.L",
        icon_name: "assetmanager-data.ico",
        icon_bytes: include_bytes!("../icons/file-types/data.ico"),
    },
    Assoc {
        ext: "S-AssetManager",
        prog_id: "AssetManager.S",
        icon_name: "assetmanager-snapshot.ico",
        icon_bytes: include_bytes!("../icons/file-types/snapshot.ico"),
    },
    Assoc {
        ext: "P-AssetManager",
        prog_id: "AssetManager.P",
        icon_name: "assetmanager-pack.ico",
        icon_bytes: include_bytes!("../icons/file-types/pack.ico"),
    },
    /* 0.4.24：资产包后缀 `.P` → `.F`（owner 2026-11-21，「F = Files」）。
       和 `.L` 同样的处理：新后缀是新出口，旧后缀只留关联 */
    Assoc {
        ext: "F-AssetManager",
        prog_id: "AssetManager.F",
        icon_name: "assetmanager-pack.ico",
        icon_bytes: include_bytes!("../icons/file-types/pack.ico"),
    },
];

/* ---------------- Win32（手写声明，零新依赖） ---------------- */

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const KEY_READ: u32 = 0x2001_9;
const REG_SZ: u32 = 1;
const ERROR_SUCCESS: i32 = 0;
const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
const SHCNF_IDLIST: u32 = 0x0000;

#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyW(hkey: isize, sub: *const u16, out: *mut isize) -> i32;
    fn RegOpenKeyExW(hkey: isize, sub: *const u16, opt: u32, sam: u32, out: *mut isize) -> i32;
    fn RegQueryValueExW(
        hkey: isize,
        name: *const u16,
        reserved: *mut u32,
        ty: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        hkey: isize,
        name: *const u16,
        reserved: u32,
        ty: u32,
        data: *const u8,
        len: u32,
    ) -> i32;
    fn RegCloseKey(hkey: isize) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn SHChangeNotify(event: i32, flags: u32, item1: *const u8, item2: *const u8);
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 读某个键的默认值（REG_SZ）。读不到（键/值不存在）返回 None。
fn current_value(key: &str) -> Option<String> {
    let sub = wide(key);
    let mut hkey: isize = 0;
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut hkey) };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let mut buf = vec![0u16; 4096];
    let mut len = (buf.len() * 2) as u32;
    let mut kind = 0u32;
    let name = wide("");
    let rc = unsafe {
        RegQueryValueExW(
            hkey,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            buf.as_mut_ptr() as *mut u8,
            &mut len,
        )
    };
    unsafe { RegCloseKey(hkey) };
    if rc != ERROR_SUCCESS || kind != REG_SZ || len < 2 {
        return None;
    }
    /* REG_SZ 带结尾的 0，去掉再比 */
    let words = &buf[..(len as usize / 2).saturating_sub(1)];
    Some(String::from_utf16_lossy(words))
}

fn write_value(key: &str, value: &str) -> Result<(), String> {
    let sub = wide(key);
    let mut hkey: isize = 0;
    let rc = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, sub.as_ptr(), &mut hkey) };
    if rc != ERROR_SUCCESS {
        return Err(format!("注册表键建不出来（错误码 {rc}）"));
    }
    let name = wide("");
    let data = wide(value);
    let rc = unsafe {
        RegSetValueExW(
            hkey,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            (data.len() * 2) as u32,
        )
    };
    unsafe { RegCloseKey(hkey) };
    if rc != ERROR_SUCCESS {
        return Err(format!("注册表值写不进去（错误码 {rc}）"));
    }
    Ok(())
}

/// 值不同才写；返回"是否真的改了"
fn set_if_changed(key: &str, wanted: &str, changed: &mut bool) -> Result<(), String> {
    if current_value(key).as_deref() == Some(wanted) {
        return Ok(());
    }
    write_value(key, wanted)?;
    *changed = true;
    Ok(())
}

/* ---------------- 图标落点 ---------------- */

/// 目录能建出来、也能真的写进去才算可写：只读目录常常 `create_dir_all` 成功而写文件失败。
fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".am-icon-probe");
    match std::fs::write(&probe, b"1") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// ico 放哪：**先试 exe 同目录**（卸载时随安装目录一起消失），
/// 写不进去（机器级安装到 Program Files）再退 `%LOCALAPPDATA%\AssetManager\icons`。
fn icon_dir() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let icons = dir.join("icons");
            if writable(&icons) {
                return Some(icons);
            }
        }
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("AssetManager").join("icons");
    if writable(&dir) {
        Some(dir)
    } else {
        None
    }
}

/// 内容一致就不写盘：启动路径每次都跑，别白白写三份 50 KB。
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if let Ok(existing) = std::fs::read(path) {
        if existing == bytes {
            return Ok(false);
        }
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

/* ---------------- 对外：注册 ---------------- */

/// 双击命令行：`"<exe>" --open "%1"`。`--open` 让 argv 解析知道下一个是文件。
pub fn open_command(exe: &Path) -> String {
    format!("\"{}\" --open \"%1\"", exe.display())
}

/// `DefaultIcon` 的值：**ico 的绝对路径**（带引号，路径里有空格也不会被拆开）。
/// 不带 `,0`：整个 ico 就是我们的图标集，索引 0 是默认帧。
pub fn icon_value(ico: &Path) -> String {
    format!("\"{}\"", ico.display())
}

/// 幂等注册三个扩展名（含图标）。返回 `Ok(true)` = 真的改了（调用方可以打日志）。
pub fn ensure_associations() -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|e| format!("拿不到程序自身路径（{e}）"))?;
    let cmd = open_command(&exe);
    let dir = icon_dir()
        .ok_or_else(|| "图标放不下：程序目录和 %LOCALAPPDATA% 都写不进去".to_string())?;
    let mut changed = false;
    for a in &ASSOCS {
        let icon_path = dir.join(a.icon_name);
        /* 图标内容换了（注册表里的路径没变）也要让 Explorer 重读一遍，
           否则它会继续拿缓存里的旧图标画 .D/.S/.P */
        if write_if_changed(&icon_path, a.icon_bytes)
            .map_err(|e| format!("写不了 {}（{e}）", icon_path.display()))?
        {
            changed = true;
        }
        /* ① .<ext> 的默认值指向 ProgID —— 这一步才是"双击路由"的入口 */
        set_if_changed(&format!(r"Software\Classes\.{}", a.ext), a.prog_id, &mut changed)?;
        /* ② 打开命令 */
        set_if_changed(
            &format!(r"Software\Classes\{}\shell\open\command", a.prog_id),
            &cmd,
            &mut changed,
        )?;
        /* ③ 图标：指向刚写出去的那个 ico */
        set_if_changed(
            &format!(r"Software\Classes\{}\DefaultIcon", a.prog_id),
            &icon_value(&icon_path),
            &mut changed,
        )?;
    }
    if !changed {
        return Ok(false);
    }
    /* 不通知的话，资源管理器会继续按缓存画旧关联（双击还是"选择程序"） */
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn open_command_quotes_path_and_keeps_placeholder() {
        let exe = PathBuf::from(r"C:\Program Files\AssetManager\AssetManager.exe");
        assert_eq!(
            open_command(&exe),
            "\"C:\\Program Files\\AssetManager\\AssetManager.exe\" --open \"%1\""
        );
    }

    #[test]
    fn default_icon_is_the_ico_path_not_the_exe() {
        let ico = PathBuf::from(r"C:\Program Files\AssetManager\icons\assetmanager-data.ico");
        assert_eq!(
            icon_value(&ico),
            "\"C:\\Program Files\\AssetManager\\icons\\assetmanager-data.ico\""
        );
    }

    #[test]
    fn extensions_are_the_agreed_suffixes() {
        let exts: Vec<&str> = ASSOCS.iter().map(|a| a.ext).collect();
        assert_eq!(
            exts,
            vec![
                "D-AssetManager",
                "L-AssetManager",
                "S-AssetManager",
                "P-AssetManager",
                "F-AssetManager"
            ]
        );
        let ids: Vec<&str> = ASSOCS.iter().map(|a| a.prog_id).collect();
        assert_eq!(
            ids,
            vec![
                "AssetManager.D",
                "AssetManager.L",
                "AssetManager.S",
                "AssetManager.P",
                "AssetManager.F"
            ]
        );
    }

    /// 项目规定「ico 必须七帧」（16/24/32/48/64/128/256）——
    /// 这三份是 owner 给的成品，这里只做"没被换成单帧文件"的守门。
    #[test]
    fn embedded_icons_are_seven_frame_ico() {
        for a in &ASSOCS {
            let b = a.icon_bytes;
            assert!(b.len() > 6, "{} 太短", a.icon_name);
            assert_eq!(&b[0..2], &[0, 0], "{} ICO 头", a.icon_name);
            assert_eq!(&b[2..4], &[1, 0], "{} ICO 类型", a.icon_name);
            let count = u16::from_le_bytes([b[4], b[5]]);
            assert_eq!(count, 7, "{} 应有 16/24/32/48/64/128/256 七帧", a.icon_name);
            let last = 6 + (count as usize - 1) * 16;
            assert!(last < b.len(), "{} 目录项越界", a.icon_name);
            assert_eq!(b[last], 0, "{} 最后一帧应是 256×256", a.icon_name);
        }
    }
}
