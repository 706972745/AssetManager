//! 单实例守卫 + 「再点一次图标」唤起 + 「双击导出文件」把路径递给已开着的实例。
//!
//! 职责（三件事，同一套机制）：
//! 1. **守卫**：第二个进程不许开第二个索引连接（两个进程同时扫同一份 `index.db`，
//!    计数会互相覆盖 —— 用户看到的是"扫完侧栏数字忽多忽少"，查起来像索引坏了）；
//! 2. **唤起**：已经开着（多半收在托盘里）时再点一次图标 → 把主窗叫到前台；
//! 3. **转发**：双击 `.D/.L/.S/.P/.F-AssetManager` 时 Windows 会按文件关联再起一个进程并传
//!    `--open "<文件>"`；第二个进程把路径写进请求文件 + SetEvent，首实例收到后
//!    发 `am://open-file`，由前端统一走「确认 → 导入」。
//!
//! 边界：
//! - 不做窗口消息 / 管道：只用「命名互斥体 + 命名事件 + 一个临时小文本」，
//!   零新依赖（本地 registry 没有缓存，引 crate 等于打不出包）；
//! - 命名都带 `Local\` 前缀：作用域是当前登录会话，多用户同时登录时互不干扰；
//! - **不在这里碰数据库**：argv 里的路径原样转给前端，导入命令自己去嗅探。
//!
//! 硬约束：
//! - 请求文件必须能被**两个进程算出同一个路径**（第二个进程还没有 AppHandle），
//!   所以走 `am_core` 的数据目录口径，不在这里手拼 `%LOCALAPPDATA%`；
//! - 写文件在 `SetEvent` 之前，保证首实例醒来一定读得到。

use std::path::PathBuf;

/// 首实例收到「再点一次图标」时发给前端的应用内事件名
pub const ACTIVATE_EVENT: &str = "am://activate";
/// 首实例收到「有导出文件要打开」时发给前端的应用内事件名，payload 是文件路径
pub const OPEN_FILE_EVENT: &str = "am://open-file";

const REQUEST_NAME: &str = "AssetManager-open-request.txt";

/// 请求文件放在 **`%TEMP%`** 下（不是数据目录、更不是安装目录）。
///
/// 为什么是 temp：
/// - 安装目录（`%LOCALAPPDATA%\AssetManager`，机器级安装时是 Program Files）**可能只读**，
///   写不进去就等于"双击导入静默失效"，而这条链路没有任何界面能提示用户；
/// - `%TEMP%` 一定可写，且是**每用户每会话**的 —— 与 `Local\` 互斥体的作用域正好对应，
///   同一个人同一个会话里两个进程必然看到同一份文件；
/// - 第二个进程在抢互斥体时就退出了，那时还没有 AppHandle，**两个进程必须能不靠 App 算出同一个路径**。
pub fn request_path() -> PathBuf {
    std::env::temp_dir().join(REQUEST_NAME)
}

/// 从命令行里挑出「要打开的那个文件」。
///
/// 规则：跳过多余的旗标（`--open` 这类以 `-` 开头的），取**第一个真实存在**的路径。
/// 双击时资源管理器给的是 `exe --open "C:\x\a.S-AssetManager"`，但也可能只给
/// `exe "C:\x\a.S-AssetManager"`（用户自己拖到 exe 上 / 命令行手敲），两种都要认。
pub fn argv_file() -> Option<String> {
    for a in std::env::args_os().skip(1) {
        let s = a.to_string_lossy().to_string();
        if s.is_empty() || s.starts_with('-') {
            continue;
        }
        let p = std::path::Path::new(&s);
        if p.is_file() {
            return Some(s);
        }
    }
    None
}

#[cfg(windows)]
mod platform {
    use super::{request_path, REQUEST_NAME};
    use std::ffi::c_void;

    /* 自己声明 kernel32 入口（理由见文件头：不引 windows-sys）。句柄一律 isize。 */
    extern "system" {
        fn CreateMutexW(attrs: *const c_void, initial_owner: i32, name: *const u16) -> isize;
        fn CreateEventW(attrs: *const c_void, manual_reset: i32, initial: i32, name: *const u16)
            -> isize;
        fn SetEvent(handle: isize) -> i32;
        fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
        fn GetLastError() -> u32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const INFINITE: u32 = 0xFFFF_FFFF;
    /* 名字沿用 0.1.21 起的互斥体名：老版本升上来时"已经在跑"的判定不能断 */
    const MUTEX_NAME: &str = "Local\\AssetManager.single.instance";
    const EVENT_NAME: &str = "Local\\AssetManager.Activate";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 第二个实例把要打开的文件路径留给首实例（写失败就算了：至少还能唤起窗口）。
    fn write_request(path: &str) {
        let p = request_path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, path);
    }

    /// 读一次就删：否则下次只是「点图标」，上回的文件又会被导一遍。
    fn take_request() -> Option<String> {
        let p = request_path();
        let text = std::fs::read_to_string(&p).ok()?.trim().to_string();
        let _ = std::fs::remove_file(&p);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// 抢守卫。`Some(事件句柄)` = 自己是首实例；`None` = 已有实例（并且已经通知它）。
    pub fn acquire(open_file: Option<&str>) -> Option<isize> {
        let name = wide(MUTEX_NAME);
        // SAFETY: name 以 NUL 结尾且活得比调用久；属性指针传空 = 默认安全描述符。
        unsafe {
            let mutex = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
            if mutex == 0 {
                /* 连互斥体都建不出来（极少见）：当作首实例放行 —— 宁可多开一个，也不能起不来 */
                return Some(0);
            }
            /* 句柄故意不关：进程退出时系统回收。提前关会让「已存在」判定失效。 */
            if GetLastError() == ERROR_ALREADY_EXISTS {
                // 先写文件再发信号
                if let Some(path) = open_file {
                    write_request(path);
                }
                let ev = wide(EVENT_NAME);
                let handle = CreateEventW(std::ptr::null(), 0, 0, ev.as_ptr());
                if handle != 0 {
                    SetEvent(handle);
                }
                return None;
            }
            let ev = wide(EVENT_NAME);
            let handle = CreateEventW(std::ptr::null(), 0, 0, ev.as_ptr());
            if handle == 0 {
                return Some(0);
            }
            Some(handle)
        }
    }

    /// 起线程等信号：先唤起界面，再把待打开的文件（如果有）发给前端。
    pub fn wait_activate(app: tauri::AppHandle, handle: isize) {
        if handle == 0 {
            return;
        }
        std::thread::spawn(move || loop {
            // SAFETY: handle 来自 CreateEventW，进程活着期间一直有效
            let r = unsafe { WaitForSingleObject(handle, INFINITE) };
            if r != 0 {
                /* WAIT_FAILED / 句柄失效：没什么可等的了 */
                return;
            }
            use tauri::Emitter;
            let _ = app.emit(crate::single_instance::ACTIVATE_EVENT, ());
            if let Some(path) = take_request() {
                /* 前端切回完整界面是异步的，稍等一拍再投递导入请求 */
                std::thread::sleep(std::time::Duration::from_millis(250));
                let _ = app.emit(crate::single_instance::OPEN_FILE_EVENT, path);
            }
        });
    }

    /// 首实例启动时命令行里就带着文件（用户关着软件双击导出文件）：
    /// 等界面订阅上事件再投递，否则这条消息会掉在空里。
    pub fn emit_initial_file(app: tauri::AppHandle, path: String) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            use tauri::Emitter;
            let _ = app.emit(crate::single_instance::OPEN_FILE_EVENT, path);
        });
    }

    /// 请求文件名（单测断言用）
    pub fn request_file_name() -> &'static str {
        REQUEST_NAME
    }
}

/// 抢守卫：`true` = 首实例可以继续启动；`false` = 已有实例在跑（已通知它），调用方退出。
pub fn claim(open_file: Option<&str>) -> bool {
    #[cfg(windows)]
    {
        match platform::acquire(open_file) {
            Some(handle) => {
                let _ = HANDLE.set(handle);
                true
            }
            None => false,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = open_file;
        true
    }
}

/// 首实例等「再点一次图标 / 双击导出文件」。非首实例不需要调用。
pub fn watch(app: tauri::AppHandle) {
    #[cfg(windows)]
    {
        let handle = HANDLE.get().copied().unwrap_or(0);
        platform::wait_activate(app, handle);
    }
    #[cfg(not(windows))]
    {
        let _ = app;
    }
}

/// 首实例启动参数里带了文件时，把这个路径投给前端（统一走确认 → 导入）。
pub fn open_initial_file(app: tauri::AppHandle, path: String) {
    #[cfg(windows)]
    {
        platform::emit_initial_file(app, path);
    }
    #[cfg(not(windows))]
    {
        let _ = (app, path);
    }
}

#[cfg(windows)]
static HANDLE: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_path_is_in_temp() {
        let p = request_path();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some(REQUEST_NAME));
        assert_eq!(p.parent(), Some(std::env::temp_dir().as_path()), "必须落在 %TEMP%，安装目录可能只读");
    }

    #[test]
    fn event_names_are_the_ones_the_frontend_listens_to() {
        /* 前端 App.tsx 里是字面量 "am://activate" / "am://open-file"，改这里必须同时改那里 */
        assert_eq!(ACTIVATE_EVENT, "am://activate");
        assert_eq!(OPEN_FILE_EVENT, "am://open-file");
    }
}
