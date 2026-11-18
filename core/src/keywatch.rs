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
//! **装不上就放行**：绝不能因为钩子失败让复制/保存用不了。
//! 另外还记一个 `HOOK_SEEN`（回调一共收到过多少个 keydown，注入的也算）：
//! 这个数是 0 说明钩子**根本没工作**（不是「没人按键」），这时候也必须放行 ——
//! 否则「钩子坏了」会伪装成「你按的都是注入」，快捷键彻底失灵。

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
/// 回调累计收到的 keydown 条数（**含注入**）。>0 才说明钩子真的在工作。
static HOOK_SEEN: AtomicU64 = AtomicU64::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// 钩子是否确认在工作：装上了，并且至少见到过一条 keydown。
pub fn alive() -> bool {
    INSTALLED.load(Ordering::SeqCst) && HOOK_SEEN.load(Ordering::SeqCst) > 0
}

/// 诊断快照：`(装上了吗, 回调收到过多少条 keydown（含注入）, 最近一次真实按键)`。
pub fn state() -> (bool, u64, Option<(u32, u64)>) {
    (
        INSTALLED.load(Ordering::SeqCst),
        HOOK_SEEN.load(Ordering::SeqCst),
        last_real_key(),
    )
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
///
/// 放行的两种情况（都算「判不了」，宁可让功能可用）：
/// 1. 钩子没装上（非 Windows / SetWindowsHookEx 失败）；
/// 2. 钩子装上了但一条 keydown 都没收到 —— 它没在工作，这个判据不成立。
pub fn real_key_matches(vk: u32, window_ms: u64) -> bool {
    if !alive() {
        return true;
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
    use std::sync::atomic::AtomicU32;
    use std::sync::Mutex;
    use std::time::Duration;

    const WH_KEYBOARD_LL: i32 = 13;
    const WM_KEYDOWN: usize = 0x0100;
    const WM_SYSKEYDOWN: usize = 0x0104;
    const LLKHF_INJECTED: u32 = 0x10;
    const PM_REMOVE: u32 = 0x0001;
    const WM_QUIT: u32 = 0x0012;

    #[repr(C)]
    struct KbdLlHookStruct {
        vk_code: u32,
        scan_code: u32,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[repr(C)]
    struct Msg {
        hwnd: *mut c_void,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt_x: i32,
        pt_y: i32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
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
        fn PeekMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32, remove: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
        fn GetMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32) -> i32;
        fn PostThreadMessageW(tid: u32, msg: u32, w: usize, l: isize) -> i32;
        fn GetCurrentThreadId() -> u32;
    }

    /// 钩子回调：只做几件原子写，然后原样传下去。
    /// **绝对不要在里做耗时的事**，否则整机输入都会被拖慢。
    unsafe extern "system" fn proc(code: i32, w: usize, l: isize) -> isize {
        if code == 0 && (w == WM_KEYDOWN || w == WM_SYSKEYDOWN) && l != 0 {
            HOOK_SEEN.fetch_add(1, Ordering::Relaxed);
            let info = &*(l as *const KbdLlHookStruct);
            if info.flags & LLKHF_INJECTED == 0 {
                LAST_VK.store(info.vk_code, Ordering::SeqCst);
                LAST_REAL_KEY.store(now_ms(), Ordering::SeqCst);
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, w, l)
    }

    static HOOK: Mutex<usize> = Mutex::new(0);
    /// 钩子线程的 id（诊断用）
    static HOOK_TID: AtomicU32 = AtomicU32::new(0);
    /// 让钩子线程退出：置位 → 线程自己 `UnhookWindowsHookEx` 后结束
    static STOP: AtomicBool = AtomicBool::new(false);
    /// SetWindowsHookEx 失败时的 GetLastError（诊断用，0 = 没失败过）
    static LAST_ERR: AtomicU32 = AtomicU32::new(0);

    fn set_last_error(e: u32) {
        LAST_ERR.store(e, Ordering::SeqCst);
    }

    pub fn last_error() -> u32 {
        LAST_ERR.load(Ordering::SeqCst)
    }

    /// 装钩子。**在一条专用线程上装**，那条线程随后跑纯消息循环。
    ///
    /// 为什么不用 Tauri 主线程：实测（0.1.27）在主线程装完、`installed=true`，
    /// 但 `HOOK_SEEN` 一直是 0 —— 一条按键事件都收不到。低级键盘钩子的回调要求
    /// **安装线程处于消息等待状态**，主线程被 tao/WebView2 的各种同步等待占着，
    /// 轮不到它派发。专用线程 + `MsgWaitForMultipleObjectsEx` 是这套 API 的标准用法。
    pub fn install() -> bool {
        if INSTALLED.load(Ordering::SeqCst) {
            return true;
        }
        STOP.store(false, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        std::thread::Builder::new()
            .name("am-keywatch".into())
            .spawn(move || unsafe {
                /* 低级钩子要求安装线程**已经有消息队列**。新建的 std 线程默认没有，
                   它会等到第一次 PeekMessage 才创建 —— 所以先探一次（PM_NOREMOVE，
                   不取走任何消息），再装钩子。不这么做的话系统给你一个非空句柄，
                   回调却永远不触发（`HOOK_SEEN` 恒为 0）。 */
                let mut probe: Msg = std::mem::zeroed();
                PeekMessageW(&mut probe, std::ptr::null_mut(), 0, 0, 0 /* PM_NOREMOVE */);

                /* `hMod` 给当前模块句柄（传 NULL 在部分系统上同样是"装上但永不回调"）。 */
                let hmod = GetModuleHandleW(std::ptr::null());
                let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), hmod, 0);
                if h.is_null() {
                    set_last_error(GetLastError());
                    let _ = tx.send(false);
                    return;
                }
                *HOOK.lock().unwrap_or_else(|e| e.into_inner()) = h as usize;
                HOOK_TID.store(GetCurrentThreadId(), Ordering::SeqCst);
                INSTALLED.store(true, Ordering::SeqCst);
                let _ = tx.send(true);

                /* 纯消息泵：线程必须**阻塞在 GetMessage 上**（可被派遣的等待状态），
                   钩子回调才有机会被插进来。`MsgWaitForMultipleObjectsEx` 试过，
                   同样是"装上了但一次都不回调"，所以回到最经典的写法。 */
                let mut msg: Msg = std::mem::zeroed();
                loop {
                    let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
                    if r <= 0 {
                        break; // WM_QUIT 或出错
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                /* 卸载必须在装它的线程上做 */
                UnhookWindowsHookEx(h as *mut c_void);
                let mut g = HOOK.lock().unwrap_or_else(|e| e.into_inner());
                *g = 0;
                HOOK_TID.store(0, Ordering::SeqCst);
                INSTALLED.store(false, Ordering::SeqCst);
            })
            .ok();

        rx.recv_timeout(Duration::from_millis(800)).unwrap_or(false)
    }

    pub fn uninstall() {
        if !INSTALLED.load(Ordering::SeqCst) {
            return;
        }
        STOP.store(true, Ordering::SeqCst);
        /* 叫醒阻塞在 GetMessage 上的钩子线程 */
        let tid = HOOK_TID.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                PostThreadMessageW(tid, WM_QUIT, 0, 0);
            }
        }
        /* 等线程自己收尾（最多 1.2s）。等不到也不强杀：线程会在下一轮退出。 */
        for _ in 0..60 {
            if !INSTALLED.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn hook_tid() -> u32 {
        HOOK_TID.load(Ordering::SeqCst)
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

/// 钩子线程 id（诊断：`snip gate … tid=` 一眼看出钩子线程还活着没）
#[cfg(windows)]
pub fn hook_tid() -> u32 {
    imp::hook_tid()
}

#[cfg(not(windows))]
pub fn hook_tid() -> u32 {
    0
}

#[cfg(windows)]
pub fn last_error() -> u32 {
    imp::last_error()
}

#[cfg(not(windows))]
pub fn last_error() -> u32 {
    0
}

#[cfg(not(windows))]
pub fn install() -> bool {
    false
}

#[cfg(not(windows))]
pub fn uninstall() {}
