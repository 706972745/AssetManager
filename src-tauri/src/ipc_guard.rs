//! 逐命令的窗口守卫（SEC-01）。
//! 自定义命令不走 ACL，这里在 IPC 的唯一入口按「窗口 label」拦一道：
//! 主窗全放行；附属窗只放行它真正用到的命令；破坏性命令任何附属窗都不放行。
//! 白名单来源：`tools/ipc_allowlist.mjs`（扫前端调用点生成，可重跑对账）。
//! 关掉：环境变量 `AM_IPC_GUARD=off`；只记不拦：`AM_IPC_GUARD=log`；
//! 记录每个窗口调了什么（只记前 3 次/条）：`AM_IPC_AUDIT=1`。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use tauri::ipc::{Invoke, InvokeError};
use tauri::Wry;

/// 窗口种类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Win {
    Main,
    Peek,
    Board,
    Snip,
    Tray,
    Splash,
    Unknown,
}

pub fn win_of(label: &str) -> Win {
    match label {
        "main" => Win::Main,
        "peek" => Win::Peek,
        l if l.starts_with("peek-") => Win::Peek,
        "board" => Win::Board,
        l if l.starts_with("board-") => Win::Board,
        "snip" => Win::Snip,
        "tray-menu" => Win::Tray,
        "splash" => Win::Splash,
        _ => Win::Unknown,
    }
}

/// 守卫结论（测试要用，pub）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
}

/// 破坏性 / 有副作用，只准主窗调。
const MAIN_ONLY: &[&str] = &[
    "am_delete",
    "am_move",
    "am_move_start",
    "am_rename",
    "am_rename_apply",
    "am_mkdir",
    "am_trash_purge",
    "am_trash_empty",
    "am_trash_restore",
    "am_rebuild_index",
    "am_remove_lib",
    "am_set_current_lib",
    "am_set_data_root",
    "am_update_install",
    "am_relaunch",
    "am_export_userdata",
    "am_export_package",
    "am_export_library_zip",
    "am_import_files",
    "am_pick_and_import",
    "am_import_userdata",
    "am_import_package",
    "am_open_import_file",
    "am_snapshot_import",
    "am_snapshot_import_path",
    "am_snapshot_export",
    "am_snapshot_export_zip",
    "am_snapshot_delete",
    "am_snapshot_rename",
    "am_scan_start",
    "am_scan",
    "am_incremental",
    "am_cache_gc",
    "am_hotkeys_set",
    "am_set_autostart",
    "am_set_ui_scale",
];

/// 贴图窗（label `peek-N`）
const PEEK: &[&str] = &[
    "am_clip_text",
    "am_peek_boot",
    "am_peek_close",
    "am_peek_copy_file",
    "am_peek_opacity",
    "am_peek_perf",
    "am_peek_ratio",
    "am_peek_ready",
    "am_peek_save_lib",
    "am_peek_set_frame",
    "am_peek_top",
    "am_peek_track",
    "am_peek_zoom",
    "am_save_as",
    "am_snip_start",
];

/// 灵感板（label `board-N`）
const BOARD: &[&str] = &[
    "am_board_add_paths",
    "am_board_ann",
    "am_board_ann_add",
    "am_board_ann_del",
    "am_board_ann_update",
    "am_board_close",
    "am_board_group",
    "am_board_hide",
    "am_board_is_top",
    "am_board_layout",
    "am_board_note_del",
    "am_board_note_put",
    "am_board_notes",
    "am_board_paste",
    "am_board_place",
    "am_board_top",
    "am_coll_paste",
    "am_open_path",
    "am_peek_copy_file",
    "am_peek_open",
    "am_reveal",
    "am_save_as",
    "am_snapshot_paste",
    "am_system_fonts",
];

/// 截图覆盖层（label `snip`）
const SNIP: &[&str] = &[
    "am_cursor_step",
    "am_snip_cancel",
    "am_snip_finish",
    "am_snip_full",
    "am_snip_ready",
    "am_snip_window_at",
];

/// 托盘菜单（label `tray-menu`）
const TRAY: &[&str] = &[
    "tray_check_update",
    "tray_menu_data",
    "tray_menu_hide",
    "tray_menu_sub",
    "tray_open_settings",
    "tray_quit",
    "tray_set_theme",
    "tray_show_main",
    "tray_switch_lib",
];

/// 闪屏（label `splash`）
const SPLASH: &[&str] = &["splash_state"];

fn allowed(win: Win, cmd: &str) -> bool {
    let list = match win {
        Win::Peek => PEEK,
        Win::Board => BOARD,
        Win::Snip => SNIP,
        Win::Tray => TRAY,
        Win::Splash => SPLASH,
        Win::Main | Win::Unknown => return true,
    };
    list.contains(&cmd)
}

/// 纯函数：不碰 IO、不读环境变量，方便单测
pub fn verdict(label: &str, cmd: &str) -> Verdict {
    match win_of(label) {
        // 1. 主窗全放行
        Win::Main => Verdict::Allow,
        // 5. 未知窗口刻意 fail-open：宁可漏防新窗口，也不要它一上来全黑（启动会记一行）
        Win::Unknown => Verdict::Allow,
        win => {
            // 2. 破坏性命令任何附属窗都不放行（哪怕白名单误加了）
            if MAIN_ONLY.contains(&cmd) {
                return Verdict::Deny;
            }
            // 3. 该窗口白名单里有就放行；4. 其余一律拒绝
            if allowed(win, cmd) {
                Verdict::Allow
            } else {
                Verdict::Deny
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Enforce,
    Log,
    Off,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let raw = std::env::var("AM_IPC_GUARD")
            .unwrap_or_default()
            .to_lowercase();
        match raw.as_str() {
            "off" | "false" | "0" => {
                crate::log_runtime_opt("[ipc-guard] 守卫已关闭（AM_IPC_GUARD=off）");
                Mode::Off
            }
            "log" | "audit" | "dry" => Mode::Log,
            _ => Mode::Enforce,
        }
    })
}

fn audit_on() -> bool {
    static AUDIT: OnceLock<bool> = OnceLock::new();
    *AUDIT.get_or_init(|| {
        matches!(
            std::env::var("AM_IPC_AUDIT").unwrap_or_default().as_str(),
            "1" | "true" | "on"
        )
    })
}

/// 同一个 (label, cmd) 只记前 3 次，之后每 100 次记一次，避免刷屏。
fn should_log(label: &str, cmd: &str) -> bool {
    static COUNTS: OnceLock<Mutex<HashMap<(String, String), u32>>> = OnceLock::new();
    let m = COUNTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    let n = g
        .entry((label.to_string(), cmd.to_string()))
        .or_insert(0);
    *n += 1;
    *n <= 3 || *n % 100 == 0
}

fn log_deny(label: &str, cmd: &str) {
    if should_log(label, cmd) {
        crate::log_runtime_opt(&format!("[ipc-guard] 拒绝 label={label} cmd={cmd}"));
    }
}

/// 未知窗口每个 label 只警告一行。
fn warn_unknown_once(label: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let m = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    let first = m
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(label.to_string());
    if first {
        crate::log_runtime_opt(&format!(
            "[ipc-guard] 未知窗口 label={label}，未加限制；补进 ipc_guard.rs"
        ));
    }
}

/// 把 generate_handler 的产物包一层
pub fn wrap<F>(inner: F) -> impl Fn(Invoke<Wry>) -> bool + Send + Sync + 'static
where
    F: Fn(Invoke<Wry>) -> bool + Send + Sync + 'static,
{
    move |invoke: Invoke<Wry>| {
        let cmd = invoke.message.command().to_string();
        let label = invoke.message.webview().label().to_string();
        let win = win_of(&label);

        if win == Win::Unknown {
            warn_unknown_once(&label);
        }

        let m = mode();
        if m == Mode::Off {
            return inner(invoke);
        }

        let v = verdict(&label, &cmd);
        /* 审计要**限流**：贴图窗拖一下窗口就是几十上百次 `am_peek_track`，
           不限流的话 `AM_IPC_AUDIT=1` 一开，runtime.log 一分钟几 MB。 */
        if win != Win::Main && audit_on() && should_log(&label, &format!("audit:{cmd}")) {
            crate::log_runtime_opt(&format!(
                "[ipc-guard] 审计 label={label} cmd={cmd} => {v:?}"
            ));
        }

        match (m, v) {
            // log 模式：只记不拦
            (Mode::Log, Verdict::Deny) => {
                log_deny(&label, &cmd);
                inner(invoke)
            }
            (_, Verdict::Allow) => inner(invoke),
            (Mode::Enforce, Verdict::Deny) => {
                log_deny(&label, &cmd);
                // 必须回包：否则前端 invoke 的 Promise 永远 pending
                invoke.resolver.respond::<serde_json::Value>(Err(InvokeError::from(
                    format!("该窗口（{label}）无权调用 {cmd}"),
                )));
                // true = 已处理，Tauri 不会再报 "Command not found"
                true
            }
            // Mode::Off 已在上面提前返回
            (Mode::Off, _) => inner(invoke),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_label_mapping() {
        assert_eq!(win_of("main"), Win::Main);
        assert_eq!(win_of("peek"), Win::Peek);
        assert_eq!(win_of("peek-12"), Win::Peek);
        assert_eq!(win_of("board-3"), Win::Board);
        assert_eq!(win_of("snip"), Win::Snip);
        assert_eq!(win_of("tray-menu"), Win::Tray);
        assert_eq!(win_of("splash"), Win::Splash);
        assert_eq!(win_of("whatever"), Win::Unknown);
    }

    #[test]
    fn main_allows_destructive() {
        for cmd in MAIN_ONLY {
            assert_eq!(verdict("main", cmd), Verdict::Allow, "main 应放行 {cmd}");
        }
    }

    #[test]
    fn attach_windows_allow_own_lists() {
        for cmd in PEEK {
            assert_eq!(verdict("peek-3", cmd), Verdict::Allow, "peek 应放行 {cmd}");
        }
        for cmd in BOARD {
            assert_eq!(
                verdict("board-1", cmd),
                Verdict::Allow,
                "board 应放行 {cmd}"
            );
        }
        for cmd in SNIP {
            assert_eq!(verdict("snip", cmd), Verdict::Allow, "snip 应放行 {cmd}");
        }
        for cmd in TRAY {
            assert_eq!(
                verdict("tray-menu", cmd),
                Verdict::Allow,
                "tray 应放行 {cmd}"
            );
        }
        for cmd in SPLASH {
            assert_eq!(verdict("splash", cmd), Verdict::Allow, "splash 应放行 {cmd}");
        }
    }

    #[test]
    fn main_only_never_overlaps_attach_lists() {
        let attach: [(&str, &[&str]); 4] = [
            ("PEEK", PEEK),
            ("BOARD", BOARD),
            ("SNIP", SNIP),
            ("TRAY", TRAY),
        ];
        for (name, list) in attach {
            for cmd in list {
                assert!(
                    !MAIN_ONLY.contains(cmd),
                    "{name} 白名单里的 {cmd} 同时出现在 MAIN_ONLY（设计冲突）"
                );
            }
        }
        for cmd in SPLASH {
            assert!(!MAIN_ONLY.contains(cmd), "SPLASH 白名单里的 {cmd} 与 MAIN_ONLY 冲突");
        }
    }

    #[test]
    fn attach_windows_deny_destructive_and_foreign() {
        assert_eq!(verdict("peek-3", "am_delete"), Verdict::Deny);
        assert_eq!(verdict("snip", "am_delete"), Verdict::Deny);
        assert_eq!(verdict("tray-menu", "am_delete"), Verdict::Deny);
        assert_eq!(verdict("splash", "am_delete"), Verdict::Deny);

        assert_eq!(verdict("peek-3", "am_open_path"), Verdict::Deny);
        assert_eq!(verdict("snip", "am_open_path"), Verdict::Deny);
        assert_eq!(verdict("tray-menu", "am_open_path"), Verdict::Deny);
        assert_eq!(verdict("board-1", "am_open_path"), Verdict::Allow);

        assert_eq!(verdict("peek-3", "am_rebuild_index"), Verdict::Deny);
        assert_eq!(verdict("peek-3", "am_board_layout"), Verdict::Deny);
    }

    #[test]
    fn unknown_label_fails_open() {
        assert_eq!(verdict("whatever", "am_delete"), Verdict::Allow);
    }
}
