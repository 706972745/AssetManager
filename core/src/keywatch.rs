//! 截图会话期间的「真实按键」监视（Windows 低级键盘钩子，只记时间戳）。
//!
//! 为什么需要它：2026-11-18 实测**有程序会向当前前台窗口注入 Ctrl+C**
//! （同机 LL 钩子抓到 `0xa2`(左 Ctrl) 与 `C` 在同一毫秒、两条都带 `INJECTED` 标记，
//! 前台正是截图覆盖层）。覆盖层把这当成「用户要复制」，表现就是
//! **「框完 / 画完一条线，它自己复制并关窗了」**。
//!
//! 前端拿不到「这条按键是注入的还是人按的」这个信息（`isTrusted` 对系统级注入也是 true），
//! 键盘消息到了 WebView 只剩 key/ctrlKey。所以判据必须放在这一层：
//! **只记 `LLKHF_INJECTED` 未置位的 keydown**，也就是真人按的键。
//!
//! 用法：截图会话开始时装、结束/取消时卸（`UnhookWindowsHookEx` 必须在装它的同一线程，
//! 也就是 Tauri 主线程 —— 那里有 winit 的消息泵，LL 钩子靠它回调）。
//!
//! **装不上就放行**（`real_key_within` 返回 true）：绝不能因为钩子失败让复制/保存用不了。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static BASE: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    BASE.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// 最近一次「真实（非注入）」keydown 的时刻（ms，进程启动基准）。0 = 还没见过。
static LAST_REAL_KEY: AtomicU64 = AtomicU64::new(0);
/// 最近一次真实 keydown 的虚拟键码（用来确认「按的确实是你说的那个键」）。
static LAST_VK: AtomicU32 = AtomicU32::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// 最近 `window_ms` 毫秒内有没有**真人**按过键。
///
/// 没装钩子（非 Windows / 安装失败）时返回 true —— 宁可放行，也不能让功能不可用。
pub fn real_key_within(window_ms: u64) -> bool {
    if !INSTALLED.load(Ordering::SeqCst) {
        return true;
    }
    let last = LAST_REAL_KEY.load(Ordering::SeqCst);
    last != 0 && now_ms().saturating_sub(last) <= window_ms
}

/// 最近一次真实按键（vk, 距今多少 ms）。没装钩子 / 没见过 → None。
pub fn last_real_key() -> Option<(u32, u64)> {
    let last = LAST_REAL_KEY.load(Ordering::SeqCst);
    if last == 0 {
        return None;
    }
    Some((LAST_VK.load(Ordering::SeqCst), now_ms().saturating_sub(last)))
}

/// 最近 `window_ms` 内，真人按下的键**是不是这一颗**。
///
/// 用它当交付闸门：注入的 Ctrl+C 不会进这个记录（带 INJECTED 标记被跳过），
/// 所以「最近真的有人按了 C」是注入伪造不了的事实。
pub fn real_key_matches(vk: u32, window_ms: u64) -> bool {
    if !INSTALLED.load(Ordering::SeqCst) {
        return true; // 钩子没装上：降级放行，不能让功能不可用
    }
    match last_real_key() {
        Some((last_vk, ago)) => last_vk == vk && ago <= window_ms,
        None => false,
    }
}

/// 清掉记录（每次开新截图会话时调，别把上一次的按键当成本次的）。
pub fn reset() {
    LAST_REAL_KEY.store(0, Ordering::SeqCst);
    LAST_VK.store(0, Ordering::SeqCst);
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::sync::Mutex;

    const WH_KEYBOARD_LL: i32 = 13;
    const WM_KEYDOWN: usize = 0x0100;
    const WM_SYSKEYDOWN: usize = 0x0104;
    const LLKHF_INJECTED: u32 = 0x10;

    #[repr(C)]
    struct KbdLlHookStruct {
        vk_code: u32,
        scan_code: u32,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[link(name = "user32")]
    extern "system" {
        fn SetWindowsHookExW(
            id_hook: i32,
            lpfn: Option<unsafe extern "system" fn(i32, usize, isize) -> isize>,
            hmod: *mut c_void,
            thread_id: u32,
        ) -> *mut c_void;
        fn UnhookWindowsHookEx(hhk: *mut c_void) -> i32;
        fn CallNextHookEx(hhk: *mut c_void, code: i32, w: usize, l: isize) -> isize;
    }

    /// 钩子回调：只做一件事 —— 真人按键就写一个时间戳，然后原样传下去。
    /// **绝对不要在里做耗时的事**，否则整机输入都会被拖慢。
    unsafe extern "system" fn proc(code: i32, w: usize, l: isize) -> isize {
        if code == 0 && (w == WM_KEYDOWN || w == WM_SYSKEYDOWN) && l != 0 {
            let info = &*(l as *const KbdLlHookStruct);
            if info.flags & LLKHF_INJECTED == 0 {
                LAST_VK.store(info.vk_code, Ordering::SeqCst);
                LAST_REAL_KEY.store(now_ms(), Ordering::SeqCst);
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, w, l)
    }

    static HOOK: Mutex<usize> = Mutex::new(0);

    pub fn install() -> bool {
        if INSTALLED.load(Ordering::SeqCst) {
            return true;
        }
        // hMod 传 NULL、thread_id 传 0 = 全局低级钩子，回调在**本进程**（装钩子的线程）上跑。
        let h = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), std::ptr::null_mut(), 0) };
        if h.is_null() {
            return false;
        }
        *HOOK.lock().unwrap_or_else(|e| e.into_inner()) = h as usize;
        INSTALLED.store(true, Ordering::SeqCst);
        true
    }

    pub fn uninstall() {
        let raw = {
            let mut g = HOOK.lock().unwrap_or_else(|e| e.into_inner());
            let v = *g;
            *g = 0;
            v
        };
        if raw != 0 {
            unsafe {
                UnhookWindowsHookEx(raw as *mut c_void);
            }
        }
        INSTALLED.store(false, Ordering::SeqCst);
    }
}

#[cfg(windows)]
pub fn install() -> bool {
    imp::install()
}

#[cfg(windows)]
pub fn uninstall() {
    imp::uninstall()
}

#[cfg(not(windows))]
pub fn install() -> bool {
    false
}

#[cfg(not(windows))]
pub fn uninstall() {}
