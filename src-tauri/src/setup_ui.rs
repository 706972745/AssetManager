//! 路线 C：安装器 / 卸载器的界面由**主程序自己**画。
//!
//! # 为什么不是"再做一个安装器 exe"
//!
//! 独立的 Tauri 安装器要自带一套 WebView 资源与第二套窗口栈，实测/估算 +5 MB；而
//! NSIS 自绘又只能做到 75% 保真（照片级右图、墨黑按钮、1px 描边都画不出来）。
//! 主程序里已经有 WebView2、已经有图标、已经有前端栈 —— 借它画画，增量只有这
//! 几百行代码。用户的要求是「尽量 0 体积占用的 1:1 还原」，这条路是唯一同时满足的。
//!
//! # 谁在做什么
//!
//! ```text
//! 安装：setup.exe(GUI) ──.onInit 里只解主程序到 $PLUGINSDIR──▶ assetmanager.exe --installer-ui
//!                        ◀── UI 退出码（0=装好 / 10=用户取消）──┘
//!        UI 在"安装"那一下拉起：  setup.exe /S /D=<目标目录>      ← 仍然是那套调通了的 NSIS 逻辑
//!        UI 不重复实现解包/注册表/快捷方式，它**驱动**并**核对**结果
//!
//! 卸载：uninstall.exe ──先把主程序拷到 %TEMP%──▶ assetmanager.exe --uninstall-ui
//!        UI 在"卸载"那一下拉起：  uninstall.exe /S [/PURGE=1]
//! ```
//!
//! 这样"安装/卸载引擎"始终只有一份（NSIS 里那套，/S /P /UPDATE 语义全都在），
//! 界面才可能做到 1:1。
//!
//! # 三条不变式（改这个文件之前先认下来）
//!
//! 1. **没有 flag 时一个字节都不碰主程序启动路径**：`dispatch()` 只扫一遍 `argv`，
//!    没命中就返回 false，`main()` 继续走原来的流程（单实例 claim → Builder）。
//! 2. **setup 模式不加载任何用户态**：不打开索引、不起 watcher / 插件 / 全局快捷键，
//!    不抢单实例互斥体。安装时这台机器上可能还没有"用户数据"，也不该有。
//! 3. **失败要说人话并留日志**（`%TEMP%\AssetManager-setup.log`）。安装器出错时用户
//!    只会截一张图，日志就是我们唯一能拿到的现场。
//! 4. **装之前先把正在跑的自己关掉**：`assetmanager.exe` 还开着时 NSIS 的 `File`
//!    写不进被占用的文件，结果是「装完了版本没变」这种最难解释的现象。
//!    检测到就**停下来问用户**，绝不替他做决定（见 `setup_running` / `setup:blocked`）。

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};

/// 卸载注册表键 —— 与 installer.nsi 的 `!define UNINSTKEY` 必须逐字一致
/// （`${PRODUCTNAME}` 由 tauri 渲染成 `AssetManager`）。
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\AssetManager";

/// 安装形态：这几个名字与 installer.nsi / hooks.nsh / tauri.conf.json 是同一份事实。
const PRODUCT: &str = "AssetManager";
const MAIN_EXE: &str = "assetmanager.exe";
const UNINST_EXE: &str = "uninstall.exe";
/// Tauri 的 `app_local_data_dir()`（= 索引 / 缩略图缓存 / 设置）在 Windows 上是
/// `%LOCALAPPDATA%\<identifier>`；素材文件本身在用户自己的素材库里，卸载不碰。
const BUNDLE_ID: &str = "com.assetmanager.desktop";
const TEMP_UI_DIR: &str = "AssetManager-setup-ui";
const TEMP_WEBVIEW_DIR: &str = "AssetManager-setup-webview";
const LOG_NAME: &str = "AssetManager-setup.log";

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Install,
    Uninstall,
}

/// 安装 / 卸载界面的语言。**只服务这 7 屏**，与主界面的 9 语言 i18n 体系无关
/// （那套在 `app/src/i18n`，两边的键和加载方式都不一样，别互相引用）。
///
/// 判定优先级：`--setup-lang=` > `AM_SETUP_LANG` > 系统 UI 语言。
/// 需求口径是"中文系统给中文，其余一律英文"——所以系统判定只认中文，
/// 不做"英文系统给英文、法文系统给法文"那种泛化。
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    fn code(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    /// `zh` / `zh-CN` / `zh-Hant` → 中文；`en` / `en-US` → 英文；其余 None（= 不认识，继续往下一级找）。
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        if s.starts_with("zh") || s == "cn" || s == "chs" || s == "cht" {
            Some(Lang::Zh)
        } else if s.starts_with("en") {
            Some(Lang::En)
        } else {
            None
        }
    }
}

/// 当前界面语言。**必须在 `dispatch()` 里定下来**：进度事件的文案是 Rust 这边拼的，
/// 页面自己的文案表也靠 `setup_env().lang` 对齐，两边只能是同一个值。
fn lang() -> Lang {
    ctx().lang
}

/// 静态文案二选一。`zh` / `en` 都写全，别用"英文缺省回退中文"——
/// 那样漏翻的一行会在英文界面上突然变成中文，比少一句话更难解释。
fn pick(zh: &str, en: &str) -> String {
    if lang() == Lang::En {
        en.to_string()
    } else {
        zh.to_string()
    }
}

struct Ctx {
    mode: Mode,
    /// 界面语言（Rust 拼的进度文案与页面文案表共用这一个判定结果）
    lang: Lang,
    /// 驱动真装的安装包；`None` = 演练模式（只走界面，不写盘）
    setup_exe: Option<PathBuf>,
    /// UI 被拉起来时的默认安装位置（`/D=` 或注册表里的旧位置）
    install_dir: Option<PathBuf>,
    /// 界面关闭时给 NSIS 的退出码：0 装好/卸完，10 用户取消
    exit_code: AtomicI32,
    /// 真的装好 / 卸完了没有。**必须单独记**：`exit_code` 的初值就是 0，
    /// 光看它分不出"装成功"和"用户一进来就把窗口关了"——后者如果也回 0，
    /// NSIS 会把「用户取消」当成「装好了」，直接退出安装器。
    ok: AtomicBool,
    /// 正在跑的子进程句柄，`setup_cancel` 要 kill 它
    child: Mutex<Option<std::process::Child>>,
    cancelled: AtomicBool,
    /// 取消/失败后待回滚的目录（只装本次新建出来的那些）。
    /// 存起来而不是当场删，是因为引擎刚被 `TerminateProcess` 掉时目录句柄还没释放；
    /// 后台线程慢慢重试，退出前再兜一次 —— 见 `fail()`。
    rollback: Mutex<Option<PathBuf>>,
    /// 装完后要拉起来的主程序（用户勾了「立即运行」）。
    ///
    /// **不能当场 spawn**：「装完了」这一刻安装界面还开着，主程序紧接着弹自己的启动页，
    /// 用户就会同时看到两个窗口（一个安装器 + 一个软件）。所以这里只记路径，
    /// 真正拉起放在 `RunEvent::Exit` —— 那时界面已经关了，进程也正要退。
    launch: Mutex<Option<PathBuf>>,
}

static CTX: OnceLock<Ctx> = OnceLock::new();

fn ctx() -> &'static Ctx {
    CTX.get().expect("setup_ui::dispatch 没跑过就调了命令")
}

/* ══════════════════════════ 入口 ══════════════════════════ */

/// 扫描 `argv`：命中安装器/卸载器入口就接管进程并返回 true。
///
/// 认三种写法：
/// - `--installer-ui` / `--uninstall-ui`（installer.nsi 用的就是这两个）
/// - `--setup-ui=install` / `--setup-ui=uninstall`（内部短名，调试用）
pub fn dispatch() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = None;
    let mut setup_exe = None;
    let mut install_dir = None;
    let mut lang_override: Option<Lang> = None;

    for a in &args {
        match a.as_str() {
            "--installer-ui" | "--setup-ui=install" => mode = Some(Mode::Install),
            "--uninstall-ui" | "--setup-ui=uninstall" => mode = Some(Mode::Uninstall),
            _ => {
                if let Some(v) = a.strip_prefix("--setup-exe=") {
                    setup_exe = Some(PathBuf::from(v.trim_matches('"')));
                } else if let Some(v) = a.strip_prefix("--install-dir=") {
                    install_dir = Some(PathBuf::from(v.trim_matches('"')));
                } else if let Some(v) = a.strip_prefix("--setup-lang=") {
                    lang_override = Lang::parse(v);
                }
            }
        }
    }

    let Some(mode) = mode else {
        return false;
    };

    // 语言：参数 > 环境变量 > 系统 UI 语言。
    // 环境变量/参数是给验收用的（不改系统语言也能截英文图）；读完 `mode` 才读它们，
    // 所以主程序正常启动（没有任何 setup flag）时连环境变量都不会碰。
    // `/S` 静默安装根本不进这个函数 —— 语言逻辑不可能影响它。
    let lang = lang_override
        .or_else(|| std::env::var("AM_SETUP_LANG").ok().and_then(|v| Lang::parse(&v)))
        .unwrap_or_else(system_lang);

    // 卸载界面必须**从临时目录**跑：它一会儿要拉起 `uninstall.exe /S`，那个进程会删
    // `$INSTDIR\assetmanager.exe` —— 如果 UI 自己就是那个文件，删除会失败，用户看到的是
    // "卸完了但程序还在"。NSIS 侧已经会先拷贝；这里兜住"用户自己双击了主程序"的情况。
    if mode == Mode::Uninstall {
        if let Some(temp_self) = relocate_out_of_install_dir() {
            log_line(&format!("[setup] 卸载界面改从 {} 启动", temp_self.display()));
            let _ = spawn_detached(&temp_self, &args);
            std::process::exit(0);
        }
    }

    /* 同一模式只允许一个自绘界面（0.5.7）。**必须放在上面那段"搬家重启"之后**：
       搬家时父进程马上就要退出，它要是先占了互斥体，子进程会被自己人挡回去。

       为什么需要：NSIS 侧没有"安装器互斥体"（它只检查主程序在不在跑），用户连点
       两下 setup.exe 就会有两个 `--installer-ui` 进程、两块一模一样的界面；而
       `dispatch()` 又跑在单实例 claim **之前**（安装时那个 claim 恰恰要绕开）。
       第二个进程按"用户取消"退出（10）—— NSIS 收到 10 就安静收场。 */
    if !claim_setup_mutex(mode) {
        log_line(&format!(
            "[setup] 已有 {} 界面在跑（pid {}），本进程按取消退出",
            if mode == Mode::Install { "安装" } else { "卸载" },
            std::process::id()
        ));
        std::process::exit(10);
    }

    let _ = CTX.set(Ctx {
        mode,
        lang,
        setup_exe,
        install_dir,
        exit_code: AtomicI32::new(0),
        ok: AtomicBool::new(false),
        child: Mutex::new(None),
        cancelled: AtomicBool::new(false),
        rollback: Mutex::new(None),
        launch: Mutex::new(None),
    });

    // 别在这里用 `setup_exe.is_none()` 当"演练"的判据：卸载模式永远没有 setup_exe，
    // 会把每次正常卸载都打成 demo=true。
    log_line(&format!(
        "[setup] 进入 {} 界面（pid {}，lang={}，setup_exe={}，install_dir={}）",
        if mode == Mode::Install { "安装" } else { "卸载" },
        std::process::id(),
        lang.code(),
        ctx().setup_exe.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "无".into()),
        ctx().install_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "无".into())
    ));

    true
}

/// 命名互斥体：同一个安装 / 卸载界面只允许一枚（见 `dispatch` 里的调用点）。
///
/// 为什么用命名互斥体而不是锁文件：进程被杀、蓝屏、断电时内核会自动释放，
/// 不会留下一个"装到一半的锁"让用户永远装不了东西。`Local\` 前缀 = 只在本登录会话内
/// 生效，多用户同时装各自的副本互不干扰。
#[cfg(windows)]
fn claim_setup_mutex(mode: Mode) -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attrs: *const core::ffi::c_void, owner: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
    }

    let name = format!(
        "Local\\AssetManager-setup-ui-{}",
        if mode == Mode::Install { "install" } else { "uninstall" }
    );
    let wide: Vec<u16> = OsStr::new(&name).encode_wide().chain(std::iter::once(0)).collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if handle == 0 {
        // 建不出来（内存耗尽之类的极端情况）：宁可不挡，也不要让用户装不了
        log_line("[setup] 建互斥体失败，跳过重复实例检查");
        return true;
    }
    const ERROR_ALREADY_EXISTS: u32 = 183;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return false;
    }
    // 句柄必须活到进程结束：提前 drop 等于把锁放了
    let _ = SETUP_MUTEX.set(handle);
    true
}

#[cfg(not(windows))]
fn claim_setup_mutex(_mode: Mode) -> bool {
    true
}

/// 存住互斥体句柄（见 `claim_setup_mutex`）。值本身从不读，存在就是"锁还持着"。
static SETUP_MUTEX: OnceLock<isize> = OnceLock::new();

/// 主程序被直接从安装目录拉起来当卸载界面时，把自己搬去临时目录再跑。
fn relocate_out_of_install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    // 判据：同目录下有 uninstall.exe（那就是安装目录）且自己不在 %TEMP% 下
    if !dir.join(UNINST_EXE).exists() {
        return None;
    }
    let temp = std::env::temp_dir();
    if exe.starts_with(&temp) {
        return None;
    }
    let dst = temp.join(TEMP_UI_DIR).join(MAIN_EXE);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    std::fs::copy(&exe, &dst).ok()?;
    Some(dst)
}

/* ══════════════════════════ 窗口 ══════════════════════════ */

pub fn run_window(mut context: tauri::Context<tauri::Wry>) {
    // 配置里没有窗口（tauri.conf.json 的 app.windows 是空数组），这里再清一遍是
    // 给"将来加了默认窗口"留的保险：setup 模式绝不该顺手建出主界面。
    context.config_mut().app.windows.clear();

    let mode = ctx().mode;
    // 标题栏在无边框窗口里看不见，但任务栏 / Alt+Tab / 录屏工具都读它，
    // 而且验收脚本是按标题找窗口（"AssetManager" 两种语言都在）。
    let title = match (mode, lang()) {
        (Mode::Install, Lang::En) => "AssetManager Setup",
        (Mode::Install, Lang::Zh) => "AssetManager 安装程序",
        (Mode::Uninstall, Lang::En) => "AssetManager Uninstaller",
        (Mode::Uninstall, Lang::Zh) => "AssetManager 卸载程序",
    };
    // 页面在第一个脚本之前就能拿到语言：走 initialization_script，而不是 URL query。
    // （query 要改 `WebviewUrl::App` 的路径语义，资源定位会跟着带上 `?lang=`，不值得。）
    let init_script = format!("window.__AM_SETUP_LANG__=\"{}\";", lang().code());

    let app = tauri::Builder::default()
        /* ★ 这个 handler 是**安装/卸载界面能看到的全部 IPC** —— 一共 9 条 setup_*。
           主程序那 180 多条 `am_*`（删除 / 移动 / 重建索引 / 任意路径打开）在这里
           根本**没有注册**，所以「安装界面被注入也删不了用户的图」这条不是靠 ACL，
           是靠进程边界 + 这份列表。改这个列表等于改安装器的攻击面，加任何一条 `am_*`
           都要先想清楚：安装界面凭什么需要它？（守卫的另一半在主程序的
           `ipc_guard.rs`，负责管主进程内的 6 类窗口。） */
        .invoke_handler(tauri::generate_handler![
            setup_env,
            setup_check_dir,
            setup_pick_dir,
            setup_running,
            setup_close_running,
            setup_install,
            setup_uninstall,
            setup_cancel,
            setup_window,
        ])
        .setup(move |app| {
            // 独立的 WebView2 数据目录：不能被主程序的缓存/会话污染，也不能反过来在
            // 用户目录里留下垃圾（用完即弃）。放 %TEMP%。
            let data_dir = std::env::temp_dir().join(TEMP_WEBVIEW_DIR);
            let data_dir = if std::fs::create_dir_all(&data_dir).is_ok() {
                Some(data_dir)
            } else {
                None
            };

            let build = |dir: Option<PathBuf>| {
                let mut builder = tauri::WebviewWindowBuilder::new(
                    app,
                    // ★ 窗口 label 用 "main"（不是 "setup"）：仓库里只有
                    //   `capabilities/default.json` 一个前端权限文件，它的 windows
                    //   列表写的是 "main"。setup 窗口借用这个 label 才能拿到
                    //   core:default（事件监听）与 core:window:allow-start-dragging
                    //   （顶部拖动条）—— 没有它，进度事件订阅会被 ACL 拒掉，
                    //   界面只剩一条不动的进度条。这是**另一个进程**，与真正的主窗
                    //   不会同时存在（run_window 前已清空窗口表）。
                    "main",
                    tauri::WebviewUrl::App("setup.html".into()),
                )
                .title(title)
                .initialization_script(init_script.clone())
                .additional_browser_args(
                    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --force-renderer-accessibility",
                )
                .inner_size(1000.0, 620.0)
                .resizable(false)
                .maximizable(false)
                .minimizable(true)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .center()
                .visible(false)
                // 安装器不该吃拖放：拖进来的文件在"还没装好"的状态下没人处理
                .disable_drag_drop_handler();
                if let Some(d) = dir {
                    builder = builder.data_directory(d);
                }
                builder.build()
            };

            // 第一次带独立数据目录；失败就**换个姿势再试一次**（上次的 msedgewebview2
            // 还占着那个目录时会失败），再失败才认命退原生。安装器宁可多花 200ms，
            // 也不能白屏。
            let win = build(data_dir.clone()).or_else(|e| {
                log_line(&format!("[setup] 带独立数据目录建窗失败（{e}），改用默认数据目录重试"));
                if let Some(w) = {
                    use tauri::Manager;
                    app.get_webview_window("main")
                } {
                    let _ = w.close();
                }
                build(None)
            });

            match win {
                Ok(win) => {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
                Err(e) => {
                    // 画不出界面 = 这次安装没法继续。**绝不 panic**（发布版没控制台，
                    // panic 等于静默退出），写日志 + 用非 0 退出码告诉 NSIS "走原生回退"。
                    log_line(&format!("[setup] 界面窗口创建失败：{e}"));
                    ctx().exit_code.store(1, Ordering::SeqCst);
                    cleanup_webview_dir();
                    app.handle().exit(1);
                }
            }
            Ok(())
        })
        .build(context);

    match app {
        Ok(app) => {
            app.run(|_app, event| {
                if let tauri::RunEvent::Exit = event {
                    log_line("[setup] 界面退出");
                    // 退出码与清理都在这里做：这是 `run()` 内部最后一道回调，
                    // 早于它自己的 `process::exit(0)`，所以 ctx 里的码还来得及生效。
                    // 兜底路径（Alt+F4 / 系统关窗）也走这儿 —— 那时 ok 为假，回 10。
                    let code = ctx().exit_code.load(Ordering::SeqCst);
                    let code = if code == 0 && !ctx().ok.load(Ordering::SeqCst) { 10 } else { code };
                    // 取消回滚可能还在后台重试（引擎刚被杀时目录锁着）。退出前再兜一次，
                    // 错过这一步就是"用户点了取消，安装目录留个空壳"。
                    // 注意：不能先持着锁再申请同一把锁（std 的 Mutex 不可重入），先 take 出来。
                    let pending = ctx().rollback.lock().ok().and_then(|mut s| s.take());
                    if let Some(dir) = pending {
                        let _ = rollback_new_dir(&dir, 4_000, true);
                    }
                    cleanup_webview_dir();
                    /* 装完「立即运行」：**在这里**才拉起主程序。此时界面已经关了，
                       用户只会看到主程序自己的启动页，不会出现"安装器 + 软件"两个窗口。
                       `spawn_detached` 是独立进程，我们随后 exit 不影响它。 */
                    let launch = ctx().launch.lock().ok().and_then(|mut s| s.take());
                    if let Some(exe) = launch {
                        match spawn_detached(&exe, &[]) {
                            Ok(()) => log_line(&format!("[setup] 已启动主程序：{}", exe.display())),
                            Err(e) => log_line(&format!("[setup] 启动主程序失败（不影响安装）：{e}")),
                        }
                    }
                    std::process::exit(code);
                }
            });
            // `App::run` 不返回（见 `shutdown` 的注释），这里只是给读代码的人一个终点。
            #[allow(unreachable_code)]
            {
                log_line("[setup] run() 意外返回，按退出码兜底结束");
                std::process::exit(ctx().exit_code.load(Ordering::SeqCst));
            }
        }
        Err(e) => {
            log_line(&format!("[setup] 构建失败：{e}"));
            std::process::exit(1);
        }
    }
}

/* ══════════════════════════ 环境探测 ══════════════════════════ */

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Env {
    mode: Mode,
    /// 界面语言（页面拿它对齐静态文案；与进度事件里的文案同源）
    lang: Lang,
    version: String,
    installed_version: Option<String>,
    default_dir: String,
    install_dir: Option<String>,
    payload_bytes: u64,
    app_bytes: u64,
    data_dir: String,
    data_bytes: u64,
    free_bytes: u64,
    setup_exe: Option<String>,
    demo: bool,
    elevated: bool,
}

#[tauri::command]
fn setup_env() -> Env {
    let c = ctx();
    let installed = install_info();
    let default_dir = c
        .install_dir
        .clone()
        .or_else(|| installed.as_ref().map(|i| i.dir.clone()))
        .unwrap_or_else(default_install_dir);

    // 数据目录和安装目录在 0.5.7 起是「安装目录 / data」的关系，但仍然可能被用户
    // 指到别处（data-root.json）：界面上分开写 —— 用户以为"卸载会删掉我的素材"
    // 是最容易劝退的一种误解。
    let data_dir = resolved_data_dir(&default_dir);

    let payload = c
        .setup_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .or_else(|| std::env::current_exe().ok().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()))
        .unwrap_or(0);

    // "占用空间"只算程序本体：数据目录里是用户的索引与缩略图缓存，卸载时默认留着，
    // 把它算进"将释放的空间"是骗人。
    let app_bytes = if default_dir.exists() {
        dir_size_inner(&default_dir, Some("data"), usize::MAX)
    } else {
        payload
    };

    Env {
        mode: c.mode,
        lang: c.lang,
        version: env!("CARGO_PKG_VERSION").to_string(),
        installed_version: installed.as_ref().map(|i| i.version.clone()),
        install_dir: installed.as_ref().map(|i| i.dir.display().to_string()),
        default_dir: default_dir.display().to_string(),
        payload_bytes: payload,
        app_bytes,
        data_dir: data_dir.display().to_string(),
        // 缩略图缓存可能有几十万个小文件：设个上限，别让"打开卸载界面"变成一次磁盘扫描。
        // 超出上限时数字偏小，但界面只用它给个量级。
        data_bytes: if data_dir.exists() {
            dir_size_inner(&data_dir, None, 20_000)
        } else {
            0
        },
        free_bytes: free_space(&default_dir),
        setup_exe: c.setup_exe.as_ref().map(|p| p.display().to_string()),
        demo: if c.mode == Mode::Install {
            // 安装的引擎 = 安装包本身（NSIS 通过 --setup-exe 递进来）
            c.setup_exe.is_none()
        } else {
            // 卸载的引擎是安装目录里的 uninstall.exe，**不是** --setup-exe。
            !install_info()
                .map(|i| i.dir.join(UNINST_EXE).exists())
                .unwrap_or(false)
        },
        elevated: is_elevated(),
    }
}

#[tauri::command]
fn setup_check_dir(dir: String) -> serde_json::Value {
    let p = PathBuf::from(&dir);
    let mut ok = true;
    let hint: String;

    if dir.trim().is_empty() {
        return serde_json::json!({ "ok": false, "hint": pick("请选择一个目录", "Choose a folder") });
    }
    // 盘符存不存在（目标是 C:\ 这种根目录时 parent 为空，直接用自己）
    let probe = if p.parent().is_some() { p.parent().unwrap().to_path_buf() } else { p.clone() };
    let free = free_space(&probe);
    if free == 0 {
        ok = false;
        hint = pick("这个位置不存在或不可写", "This location doesn't exist or isn't writable");
    } else if free < 80 * 1024 * 1024 {
        ok = false;
        hint = pick(
            &format!("剩余空间只有 {}，不够安装", human(free)),
            &format!("Only {} free — not enough space to install", human(free)),
        );
    } else if p.exists() && p.join(UNINST_EXE).exists() {
        hint = pick(
            &format!("已存在一份 AssetManager · 将直接覆盖升级 · 剩余 {}", human(free)),
            &format!("AssetManager is already here · it will be upgraded in place · {} free", human(free)),
        );
    } else {
        hint = pick(
            &format!("这个位置当前用户可写 · 剩余 {}", human(free)),
            &format!("Writable by the current user · {} free", human(free)),
        );
    }
    serde_json::json!({ "ok": ok, "hint": hint })
}

/// 原生文件夹选择器。用 `rfd`（本仓库已有的依赖），不引 dialog 插件：
/// setup 模式的窗口不加载任何插件，少一条权限面。
#[tauri::command]
fn setup_pick_dir(current: String) -> Option<String> {
    let mut dlg = rfd::FileDialog::new();
    if !current.is_empty() {
        dlg = dlg.set_directory(&current);
    }
    dlg.pick_folder().map(|p| p.display().to_string())
}

/* ══════════════════════════ 进度事件 ══════════════════════════ */

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    percent: u8,
    label: String,
    sub: String,
    log: String,
    done: bool,
    failed: Option<String>,
    cancelled: bool,
}

fn emit(app: &tauri::AppHandle, p: Progress) {
    use tauri::Emitter;
    let _ = app.emit("setup:progress", p);
}

/* ══════════════════════════ 正在运行的实例 ══════════════════════════ */

/// 一个正在运行的 `assetmanager.exe` —— **永远不含当前进程**（安装/卸载界面自己
/// 就叫这个名字，不排除自己的话每次都会"检测到自己"而卡住）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningProc {
    pid: u32,
    /// 完整路径；拿不到（权限/刚退出）时是空串
    path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Blocked {
    kind: String,
    count: usize,
    pids: Vec<u32>,
}

/// 有没有别的 AssetManager 开着（首屏与点"安装"前各查一次）。
#[tauri::command]
fn setup_running() -> Vec<RunningProc> {
    running_instances()
}

/// 结束所有正在运行的实例，返回真正剩下的（`remaining` 为空 = 都关掉了）。
///
/// 只在前端用户明确选了「关闭软件并继续」时才调 —— 安装器没有权力替用户
/// 静默杀掉一个正在整理素材的程序。
#[tauri::command]
fn setup_close_running() -> serde_json::Value {
    let before = running_instances();
    for p in &before {
        log_line(&format!(
            "[setup] 结束运行中的实例 pid={} path={}",
            p.pid, p.path
        ));
        let _ = terminate_pid(p.pid);
    }
    // TerminateProcess 成功 ≠ 进程已从进程表消失，复查几次再下结论：
    // 早退的话紧接着的 NSIS File 还是写不进去。
    let mut left = running_instances();
    for _ in 0..25 {
        if left.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
        left = running_instances();
    }
    log_line(&format!(
        "[setup] 关闭结果：请求 {} 个，仍剩 {} 个",
        before.len(),
        left.len()
    ));
    serde_json::json!({ "closed": before.len() - left.len(), "remaining": left })
}

fn emit_blocked(app: &tauri::AppHandle, procs: &[RunningProc], kind: &str) {
    use tauri::Emitter;
    log_line(&format!(
        "[setup] 检测到 {} 个正在运行的实例，停下来等用户决定（{kind}）",
        procs.len()
    ));
    let _ = app.emit(
        "setup:blocked",
        Blocked {
            kind: kind.to_string(),
            count: procs.len(),
            pids: procs.iter().map(|p| p.pid).collect(),
        },
    );
}

/* ══════════════════════════ 安装 ══════════════════════════ */

#[tauri::command]
fn setup_install(
    app: tauri::AppHandle,
    dir: String,
    desktop_shortcut: bool,
    run_after: bool,
    force_close_running: Option<bool>,
) {
    let dir = PathBuf::from(dir.trim());
    let handle = app.clone();
    let force = force_close_running.unwrap_or(false);
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let el = || {
            pick(
                &format!("已用 {} 秒", started.elapsed().as_secs()),
                &format!("{}s elapsed", started.elapsed().as_secs()),
            )
        };

        let existed = dir.exists();
        let setup_exe = ctx().setup_exe.clone();

        // ⓪ 先处理"自己还开着"：文件被占用时 NSIS 的 File 写不进去，最终表现是
        //    「装完了但版本没变」。这里**不替用户杀进程**——先停下来问，
        //    用户选「关闭软件并继续」时前端会先调 setup_close_running 再带 force 回来。
        let mut running = running_instances();
        if !running.is_empty() {
            if !force {
                emit_blocked(&handle, &running, "install");
                // 故意不发 done：前端收到 blocked 就弹对话框，进度屏保持"待定"状态
                return;
            }
            // force = 用户已经同意关闭。前端通常已经关过一轮，这里兜第二次，
            // 仍关不掉就如实失败 —— 带着占用去装只会得到"装完版本没变"。
            for p in &running {
                let _ = terminate_pid(p.pid);
            }
            std::thread::sleep(std::time::Duration::from_millis(350));
            running = running_instances();
            if !running.is_empty() {
                let pids: Vec<String> = running.iter().map(|p| p.pid.to_string()).collect();
                return fail(
                    &handle,
                    &pick(
                        &format!(
                            "还有 {} 个 AssetManager 进程没能关闭（pid {}），请手动退出后再试",
                            running.len(),
                            pids.join("、")
                        ),
                        &format!(
                            "{} AssetManager process(es) are still running (pid {}). Quit them and try again.",
                            running.len(),
                            pids.join(", ")
                        ),
                    ),
                    existed,
                    &dir,
                );
            }
        }

        // ① 目标目录
        emit(
            &handle,
            Progress {
                percent: 4,
                label: pick("正在准备安装目录", "Preparing install folder"),
                sub: el(),
                log: pick(
                    &format!("目标目录 {}", dir.display()),
                    &format!("Target folder {}", dir.display()),
                ),
                done: false,
                failed: None,
                cancelled: false,
            },
        );
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return fail(
                &handle,
                &pick(
                    &format!("无法创建 {}：{e}", dir.display()),
                    &format!("Cannot create {}: {e}", dir.display()),
                ),
                existed,
                &dir,
            );
        }

        // ② 前面没有安装包可驱动 → 演练模式：走完界面，一个字节都不写
        if setup_exe.is_none() {
            demo_timeline(&handle, "install", &el);
            emit(
                &handle,
                Progress {
                    percent: 100,
                    label: pick("演练完成", "Demo complete"),
                    sub: pick(
                        &format!("共 {} 秒 · 未写入任何文件", started.elapsed().as_secs()),
                        &format!("{}s total · nothing was written", started.elapsed().as_secs()),
                    ),
                    log: pick(
                        "演练模式：没有可驱动的安装包",
                        "Demo mode: no installer package to drive",
                    ),
                    done: true,
                    failed: None,
                    cancelled: false,
                },
            );
            return;
        }

        // ③ 真装：把界面让给 NSIS 的静默安装（它才是引擎）
        let exe = setup_exe.clone().unwrap();
        // `/DS` / `/NS`：**两条都显式给**。模板里 `/NS` 只是"禁止建"，静默安装
        // 默认会建，但覆盖升级（`$UpdateMode`）那条分支会直接 Return 跳过创建，
        // 于是"勾着却没建"。给一条正向的 `/DS` 让勾选变成明确指令。
        let mut args: Vec<&str> = vec!["/S"];
        args.push(if desktop_shortcut { "/DS" } else { "/NS" });
        log_line(&format!(
            "[setup] 启动静默安装：{} {} /D={}",
            exe.display(),
            args.join(" "),
            dir.display()
        ));
        let child = match spawn_silent(&exe, &args, Some(&format!("/D={}", dir.display()))) {
            Ok(c) => c,
            Err(e) => {
                return fail(
                    &handle,
                    &pick(
                        &format!("无法启动安装引擎：{e}"),
                        &format!("Could not start the install engine: {e}"),
                    ),
                    existed,
                    &dir,
                );
            }
        };
        if let Ok(mut slot) = ctx().child.lock() {
            *slot = Some(child);
        }

        emit(
            &handle,
            Progress {
                percent: 12,
                label: pick("正在解压程序文件", "Extracting program files"),
                sub: el(),
                log: pick("安装引擎已启动（静默）", "Install engine started (silent)"),
                done: false,
                failed: None,
                cancelled: false,
            },
        );

        // ④ 轮询真实落盘结果当进度：比"假装 30%"诚实，也不需要引擎回报
        let main_exe = dir.join(MAIN_EXE);
        let uninstaller = dir.join(UNINST_EXE);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        // **覆盖安装**：这两条"事实"在覆盖安装时开头就成立 —— 文件上一步还在。
        // 只按 `exists()` 判，界面会在第 0.1 秒直接跳到 88%、然后把「已用 0 秒」冻在
        // 那儿几十秒。所以先记下开局状态：
        //   · 主程序：开局就有 → 不再拿它当进度信号（NSIS 的 File 会保留源文件 mtime，
        //     mtime 比对也识别不出重写，只能放弃这条）
        //   · 卸载程序：WriteUninstaller 是"现写"，mtime 一定是新的 → 可以用 mtime 判
        let had_exe = main_exe.exists();
        let un_mtime0 = std::fs::metadata(&uninstaller).and_then(|m| m.modified()).ok();
        let un_fresh = || {
            let now = std::fs::metadata(&uninstaller).and_then(|m| m.modified()).ok();
            match (un_mtime0, now) {
                (Some(a), Some(b)) => b > a,
                (None, Some(_)) => true,
                _ => false,
            }
        };
        let mut step_exe = false;
        let mut step_un = false;

        loop {
            if ctx().cancelled.load(Ordering::SeqCst) {
                if let Ok(mut slot) = ctx().child.lock() {
                    if let Some(c) = slot.as_mut() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                    *slot = None;
                }
                log_line("[setup] 用户取消安装，开始回滚");
                return fail(&handle, &pick("已取消安装", "Installation cancelled"), existed, &dir);
            }

            // 子进程句柄只在 ctx 里存一份（Child 不能克隆）：每拍借出来 try_wait 一下
            let status = {
                let mut slot = match ctx().child.lock() {
                    Ok(s) => s,
                    Err(_) => {
                        return fail(&handle, &pick("内部锁损坏", "Internal lock failure"), existed, &dir)
                    }
                };
                match slot.as_mut() {
                    Some(c) => match c.try_wait() {
                        Ok(s) => s,
                        Err(e) => {
                            return fail(
                                &handle,
                                &pick(
                                    &format!("等待安装引擎失败：{e}"),
                                    &format!("Failed waiting for the install engine: {e}"),
                                ),
                                existed,
                                &dir,
                            )
                        }
                    },
                    None => Some(Default::default()),
                }
            };
            if let Some(status) = status {
                if let Ok(mut slot) = ctx().child.lock() {
                    *slot = None;
                }
                if !status.success() {
                    return fail(
                        &handle,
                        &pick(
                            &format!("安装引擎返回失败（退出码 {:?}）", status.code()),
                            &format!("Install engine failed (exit code {:?})", status.code()),
                        ),
                        existed,
                        &dir,
                    );
                }
                break;
            }

            // 首次安装：main_exe 从无到有 ≈ 解压刚结束。覆盖安装时它开局就在，
            // 这条信号无效，进度就停在 12% 等卸载程序现身 —— 那才是真的尾巴。
            if !step_exe && !had_exe && main_exe.exists() {
                step_exe = true;
                emit(
                    &handle,
                    Progress {
                        percent: 62,
                        label: pick(
                            "正在写入注册表与快捷方式",
                            "Writing registry entries and shortcuts",
                        ),
                        sub: el(),
                        log: pick("主程序已就位", "Main program in place"),
                        done: false,
                        failed: None,
                        cancelled: false,
                    },
                );
            }
            // 不再要求 step_exe：覆盖安装时上面那条永远不会亮，`uninstall.exe` 的
            // mtime 更新才是唯一可靠的"快装完了"。
            if !step_un && uninstaller.exists() && un_fresh() {
                step_un = true;
                emit(
                    &handle,
                    Progress {
                        percent: 88,
                        label: pick("正在登记卸载信息", "Registering uninstall information"),
                        sub: el(),
                        log: pick("卸载程序已生成", "Uninstaller created"),
                        done: false,
                        failed: None,
                        cancelled: false,
                    },
                );
            }

            if std::time::Instant::now() > deadline {
                if let Ok(mut slot) = ctx().child.lock() {
                    if let Some(c) = slot.as_mut() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                    *slot = None;
                }
                return fail(
                    &handle,
                    &pick("安装超时（180 秒）", "Install timed out (180s)"),
                    existed,
                    &dir,
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(120));
        }

        if let Ok(mut slot) = ctx().child.lock() {
            *slot = None;
        }

        // ⑤ 核对：文件 + 注册表版本。引擎说成功不算成功，装不上的情况真发生过。
        let missing: Vec<&str> = [(MAIN_EXE, main_exe.as_path()), (UNINST_EXE, uninstaller.as_path())]
            .iter()
            .filter(|(_, p)| !p.exists())
            .map(|(n, _)| *n)
            .collect();
        if !missing.is_empty() {
            return fail(
                &handle,
                &pick(
                    &format!("安装后缺少文件：{}", missing.join("、")),
                    &format!("Files missing after install: {}", missing.join(", ")),
                ),
                existed,
                &dir,
            );
        }
        let installed = install_info();
        let ver = installed
            .as_ref()
            .map(|i| i.version.clone())
            .unwrap_or_else(|| pick("未知", "unknown"));
        if installed.is_none() {
            return fail(
                &handle,
                &pick(
                    "注册表里没有卸载信息（Add/Remove Programs 里会找不到它）",
                    "No uninstall entry in the registry (it won't appear in Apps & features)",
                ),
                existed,
                &dir,
            );
        }

        // ⑤b 勾了"桌面快捷方式"就核对真建出来了没有。建不出来**不判失败**
        //     （组策略 / 只读桌面 / 企业把桌面重定向都可能拦），但必须留一条证据，
        //     不能让用户对着空桌面以为勾选生效了。
        let lnk_note = if desktop_shortcut {
            match desktop_shortcut_path() {
                Some(lnk) if lnk.exists() => {
                    log_line(&format!("[setup] 桌面快捷方式已就位：{}", lnk.display()));
                    String::new()
                }
                Some(lnk) => {
                    log_line(&format!("[setup] 桌面快捷方式未创建：{}", lnk.display()));
                    pick(" · 桌面快捷方式未能创建", " · desktop shortcut could not be created")
                }
                None => String::new(),
            }
        } else {
            String::new()
        };

        emit(
            &handle,
            Progress {
                percent: 100,
                label: pick("安装完成", "Installation complete"),
                sub: pick(
                    &format!("共 {} 秒", started.elapsed().as_secs()),
                    &format!("{}s total", started.elapsed().as_secs()),
                ),
                log: pick(
                    &format!("已登记 {} {ver}{lnk_note}", PRODUCT),
                    &format!("Registered {} {ver}{lnk_note}", PRODUCT),
                ),
                done: true,
                failed: None,
                cancelled: false,
            },
        );

        // ⑥ 收尾交给界面：「立即运行」等用户点"完成"时才做（他可能临时不想开），
        // 这里只把"装好了"记进退出码，NSIS 外层据此决定回 0 还是 10。
        let _ = run_after;
        ctx().ok.store(true, Ordering::SeqCst);
        ctx().exit_code.store(0, Ordering::SeqCst);
    });
}

/* ══════════════════════════ 卸载 ══════════════════════════ */

#[tauri::command]
fn setup_uninstall(
    app: tauri::AppHandle,
    dir: String,
    purge_data: bool,
    force_close_running: Option<bool>,
) {
    let dir = PathBuf::from(dir.trim());
    let handle = app.clone();
    let force = force_close_running.unwrap_or(false);
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let el = || {
            pick(
                &format!("已用 {} 秒", started.elapsed().as_secs()),
                &format!("{}s elapsed", started.elapsed().as_secs()),
            )
        };
        let uninstaller = dir.join(UNINST_EXE);
        log_line(&format!(
            "[setup] setup_uninstall 被调用 dir={} uninstaller={} purge={} force={}",
            dir.display(),
            uninstaller.exists(),
            purge_data,
            force
        ));

        // ⓪ 跟安装同一条规矩：删不掉正在运行的 exe，NSIS 只能 `RMDir /REBOOTOK`
        //    挂到重启后，用户看到的是「卸完了文件还在」。先问，别替他杀。
        let mut running = running_instances();
        if !running.is_empty() {
            if !force {
                emit_blocked(&handle, &running, "uninstall");
                return;
            }
            for p in &running {
                let _ = terminate_pid(p.pid);
            }
            std::thread::sleep(std::time::Duration::from_millis(350));
            running = running_instances();
            if !running.is_empty() {
                return fail_uninstall(
                    &handle,
                    &pick(
                        &format!(
                            "还有 {} 个 AssetManager 进程没能关闭，请手动退出后再试",
                            running.len()
                        ),
                        &format!(
                            "{} AssetManager process(es) could not be closed. Quit them and try again.",
                            running.len()
                        ),
                    ),
                    false,
                );
            }
        }

        if !uninstaller.exists() {
            return fail_uninstall(
                &handle,
                &pick(
                    &format!("找不到卸载程序：{}", uninstaller.display()),
                    &format!("Uninstaller not found: {}", uninstaller.display()),
                ),
                false,
            );
        }

        let mut uargs: Vec<&str> = vec!["/S"];
        // 模板里默认不删数据（`$DeleteAppDataCheckboxState` 只有 /PURGE 会置 1）。
        // `/PURGE=1` 是我们在 un.onInit 里留的口子，语义只有"勾了那个框"：
        // 删的是**数据目录**（0.5.7 起默认 = `<安装目录>\data`：索引 / 回收站 / 设置 /
        // WebView2 用户数据；老版本在 %APPDATA%/%LOCALAPPDATA% 下按 bundle id 存的
        // 那两处也一并清）。**素材文件本身在用户自己的素材库里，一个都不动**。
        if purge_data {
            uargs.push("/PURGE=1");
        }
        log_line(&format!(
            "[setup] 启动静默卸载：{} {}",
            uninstaller.display(),
            uargs.join(" ")
        ));
        let mut child = match spawn_silent(&uninstaller, &uargs, None) {
            Ok(c) => c,
            Err(e) => {
                return fail_uninstall(
                    &handle,
                    &pick(
                        &format!("无法启动卸载引擎：{e}"),
                        &format!("Could not start the uninstall engine: {e}"),
                    ),
                    false,
                )
            }
        };

        emit(
            &handle,
            Progress {
                percent: 20,
                label: pick("正在移除程序文件", "Removing program files"),
                sub: el(),
                log: pick("卸载引擎已启动（静默）", "Uninstall engine started (silent)"),
                done: false,
                failed: None,
                cancelled: false,
            },
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return fail_uninstall(
                            &handle,
                            &pick(
                                &format!("卸载引擎返回失败（退出码 {:?}）", status.code()),
                                &format!("Uninstall engine failed (exit code {:?})", status.code()),
                            ),
                            false,
                        );
                    }
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    return fail_uninstall(
                        &handle,
                        &pick(
                            &format!("等待卸载引擎失败：{e}"),
                            &format!("Failed waiting for the uninstall engine: {e}"),
                        ),
                        false,
                    )
                }
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                return fail_uninstall(
                    &handle,
                    &pick("卸载超时（180 秒）", "Uninstall timed out (180s)"),
                    false,
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(120));
        }

        log_line(&format!(
            "[setup] 卸载引擎父进程已退出（{:.1}s），开始等真实结果",
            started.elapsed().as_secs_f64()
        ));

        // **不能拿"引擎退出"当卸载完成**：NSIS 的 uninstall.exe 会把自己拷到
        // `%TEMP%\~nsu*.tmp\Au_.exe` 再重启一个子进程，父进程 1 秒内就退 —— 那时
        // 文件一个都没少、注册表也还在。所以盯**事实**：注册表项消失 + 主程序文件消失。
        let main_exe = dir.join(MAIN_EXE);
        let wait_until = std::time::Instant::now() + std::time::Duration::from_secs(90);
        let mut settled = false;
        let mut ticks = 0u32;
        while std::time::Instant::now() < wait_until {
            let reg_gone = install_info().is_none();
            let exe_gone = !main_exe.exists();
            if reg_gone && exe_gone {
                settled = true;
                break;
            }
            // 每 1.5 秒推一次进度，别让用户以为卡住
            if ticks % 12 == 0 {
                emit(
                    &handle,
                    Progress {
                        percent: 60 + (ticks / 12).min(35) as u8,
                        label: pick("正在移除文件", "Removing files"),
                        sub: el(),
                        log: String::new(),
                        done: false,
                        failed: None,
                        cancelled: false,
                    },
                );
            }
            ticks += 1;
            std::thread::sleep(std::time::Duration::from_millis(125));
        }
        log_line(&format!(
            "[setup] 卸载结果：注册表={} 主程序={} 用时 {:.1}s",
            if install_info().is_none() { "已清" } else { "仍在" },
            if main_exe.exists() { "仍在" } else { "已删" },
            started.elapsed().as_secs_f64()
        ));
        if !settled {
            // 没等到干净结果：如实报，别假装成功
            let leftover = std::fs::read_dir(&dir)
                .map(|rd| rd.filter_map(|e| e.ok()).count())
                .unwrap_or(0);
            return fail_uninstall(
                &handle,
                &pick(
                    &format!("卸载未完成（90 秒后仍有残留：{leftover} 项）"),
                    &format!("Uninstall didn't finish ({leftover} items left after 90s)"),
                ),
                false,
            );
        }

        // 部分文件被占用是**正常**结果，不是失败：说清剩几个，别假装干净
        let leftover = std::fs::read_dir(&dir)
            .map(|rd| rd.filter_map(|e| e.ok()).count())
            .unwrap_or(0);

        emit(
            &handle,
            Progress {
                percent: 100,
                label: if leftover > 0 {
                    pick("卸载完成（有残留）", "Uninstalled (files left behind)")
                } else {
                    pick("卸载完成", "Uninstalled")
                },
                sub: pick(
                    &format!("共 {} 秒", started.elapsed().as_secs()),
                    &format!("{}s total", started.elapsed().as_secs()),
                ),
                log: if leftover > 0 {
                    pick(
                        &format!("还有 {leftover} 个文件未能删除（可能被占用）"),
                        &format!("{leftover} file(s) could not be deleted (possibly in use)"),
                    )
                } else {
                    pick("程序文件已全部移除", "All program files removed")
                },
                done: true,
                failed: None,
                cancelled: false,
            },
        );
        ctx().ok.store(true, Ordering::SeqCst);
    });
}

#[tauri::command]
fn setup_cancel() {
    ctx().cancelled.store(true, Ordering::SeqCst);
    if let Ok(mut slot) = ctx().child.lock() {
        if let Some(child) = slot.as_mut() {
            let _ = child.kill();
        }
    }
}

/* ══════════════════════════ 窗口动作 ══════════════════════════ */

/// 关界面并把退出码记下来 —— **真正的退出发生在 `RunEvent::Exit` 回调里**。
///
/// 两条不能踩的坑：
/// 1. `AppHandle::exit(code)` 等价于 `std::process::exit`，`run()` 之后那句清理
///    永远不执行（`%TEMP%\AssetManager-setup-webview` 每次留 1.3 MB，
///    日志里连"清理失败"都没有，因为代码根本没跑到）。
/// 2. `App::run()` 在处理完事件循环后**自己** `process::exit(0)`，也不返回 ——
///    所以"关窗口 → run 返回 → 用 ctx 的码退出"这条路不存在，退出码会被钉死成 0，
///    用户点「取消」（应为 10）时 NSIS 会当成"装好了"。
fn shutdown(app: tauri::AppHandle, code: i32) {
    use tauri::Manager;
    ctx().exit_code.store(code, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.close();
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(8000));
        log_line("[setup] 窗口迟迟未关闭，按退出码强制结束");
        cleanup_webview_dir();
        std::process::exit(code);
    });
}

#[tauri::command]
fn setup_window(app: tauri::AppHandle, action: String, launch: Option<bool>) {
    use tauri::Manager;
    match action.as_str() {
        "minimize" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.minimize();
            }
        }
        "open-install-dir" => {
            let env = setup_env();
            let dir = env.install_dir.unwrap_or(env.default_dir);
            let _ = open_path(&dir);
        }
        "finish" => {
            if launch.unwrap_or(false) {
                let env = setup_env();
                let dir = env.install_dir.unwrap_or(env.default_dir);
                let exe = PathBuf::from(dir).join(MAIN_EXE);
                if exe.exists() {
                    /* 只记下来，不在这里 spawn：这一刻安装界面还开着，
                       立刻拉起主程序会出现"两个窗口"（见 Ctx::launch 的注释）。
                       真正的 spawn 在 `RunEvent::Exit`。 */
                    if let Ok(mut slot) = ctx().launch.lock() {
                        *slot = Some(exe.clone());
                    }
                    log_line(&format!(
                        "[setup] 安装完成，界面退出后启动主程序：{}",
                        exe.display()
                    ));
                } else {
                    log_line(&format!("[setup] 勾了「立即运行」但找不到 {}", exe.display()));
                }
            }
            let code = ctx().exit_code.load(Ordering::SeqCst);
            shutdown(app, code);
        }
        // 直接关窗口（点"取消"、点标题条关闭、还没开始就退出）：**没真装好就回 10**。
        // 不能用 `exit_code` 判断——它的初值就是 0，会把"一进来就关掉"当成装好了，
        // NSIS 那边收到 0 就认为成功、直接退出安装器（用户以为装了，其实什么都没发生）。
        _ => {
            let ok = ctx().ok.load(Ordering::SeqCst);
            let code = if ok { 0 } else { 10 };
            log_line(&format!("[setup] 界面被关闭（已完成={ok}，退出码 {code}）"));
            shutdown(app, code);
        }
    }
}

/* ══════════════════════════ 小工具 ══════════════════════════ */

/// 删掉"本次安装才新建"的目录，带重试。返回 true = 目录已经不在。
///
/// 为什么要重试而不是删一次了事：取消时引擎是被 `TerminateProcess` 干掉的，
/// 它写一半的 `assetmanager.exe` 句柄要等内核回收，这时 `remove_dir_all` 会在
/// 那个文件上失败；文件删掉后目录本身也可能还被当作某进程的 CWD 锁一小会儿。
fn rollback_new_dir(dir: &Path, budget_ms: u64, final_attempt: bool) -> bool {
    let t0 = std::time::Instant::now();
    let budget = std::time::Duration::from_millis(budget_ms);
    let mut tries = 0u32;
    loop {
        if !dir.exists() {
            log_line(&format!("[setup] 已回滚新建目录 {}", dir.display()));
            return true;
        }
        if std::fs::remove_dir_all(dir).is_ok() {
            log_line(&format!("[setup] 已回滚新建目录 {}", dir.display()));
            return true;
        }
        tries += 1;
        if t0.elapsed() >= budget {
            if final_attempt {
                log_line(&format!(
                    "[setup] 回滚未完成：{} 仍在（被刚结束的引擎占着，试了 {} 次 / {} ms）。\
                     里面是什么都不影响下次安装，重装会覆盖",
                    dir.display(),
                    tries,
                    t0.elapsed().as_millis()
                ));
            } else {
                log_line(&format!(
                    "[setup] 后台重试 {} 次 / {} ms 还没删掉 {}，界面退出前会再兜一次",
                    tries,
                    t0.elapsed().as_millis(),
                    dir.display()
                ));
            }
            return false;
        }
        if tries == 1 {
            log_line("[setup] 回滚时目录被占用，等引擎释放句柄后重试");
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

fn fail(app: &tauri::AppHandle, why: &str, existed: bool, dir: &Path) {
    log_line(&format!("[setup] 安装失败：{why}"));
    // 回滚：只删**我们自己建的**目标目录（升级安装时那目录里是用户已装好的程序，
    // 不能因为这次失败就把它清空 —— 那等于把可用状态也毁掉）
    //
    // 两个坑：
    // 1. 取消的那一刻我们是 `TerminateProcess` 掉引擎的，它手里的 `assetmanager.exe`
    //    句柄要过一会儿才释放。只删一次的话 `remove_dir_all` 会在锁住的那个文件上
    //    失败，**留下一个空目录**，而日志照样打"已回滚"——谎报。
    // 2. 重试不能同步做：界面这会儿正在等 `setup:progress` 事件去切屏，卡住就是
    //    "点了取消，窗口僵住好几秒"。所以先回报，再把目录交给后台线程慢慢删；
    //    退出前（`RunEvent::Exit`）还会再兜一次。
    if !existed {
        if let Ok(mut slot) = ctx().rollback.lock() {
            *slot = Some(dir.to_path_buf());
        }
        let d = dir.to_path_buf();
        std::thread::spawn(move || {
            let done = rollback_new_dir(&d, 12_000, false);
            if let Ok(mut slot) = ctx().rollback.lock() {
                // 删掉了就销账，别让退出时再白试一次
                if done && slot.as_deref() == Some(d.as_path()) {
                    *slot = None;
                }
            }
        });
    }
    let cancelled = ctx().cancelled.load(Ordering::SeqCst);
    emit(
        app,
        Progress {
            percent: 100,
            label: if cancelled {
                pick("已取消安装", "Installation cancelled")
            } else {
                pick("安装未完成", "Installation didn't finish")
            },
            sub: if cancelled {
                String::new()
            } else {
                pick(
                    &format!("详细日志：%TEMP%\\{}", LOG_NAME),
                    &format!("Details: %TEMP%\\{}", LOG_NAME),
                )
            },
            log: why.to_string(),
            done: true,
            failed: if cancelled { None } else { Some(why.to_string()) },
            cancelled,
        },
    );
    ctx().exit_code.store(if cancelled { 10 } else { 1 }, Ordering::SeqCst);
}

fn fail_uninstall(app: &tauri::AppHandle, why: &str, cancelled: bool) {
    log_line(&format!("[setup] 卸载失败：{why}"));
    emit(
        app,
        Progress {
            percent: 100,
            label: pick("卸载未完成", "Uninstall didn't finish"),
            sub: pick(
                &format!("详细日志：%TEMP%\\{}", LOG_NAME),
                &format!("Details: %TEMP%\\{}", LOG_NAME),
            ),
            log: why.to_string(),
            done: true,
            failed: Some(why.to_string()),
            cancelled,
        },
    );
}

/// 演练模式的时间线：所有文案与真实安装一致，唯独没有副作用。
/// 这是唯一允许"编"进度的地方 —— 也必须编，否则用户看不到界面长什么样。
fn demo_timeline(app: &tauri::AppHandle, kind: &str, el: &dyn Fn() -> String) {
    // 中英各一份，不能只翻 label：日志区那几行也在界面上，漏了就露馅。
    let steps: [(u8, &str, &str); 4] = match (kind, lang()) {
        ("install", Lang::En) => [
            (14, "Extracting program files", "Writing assetmanager.exe"),
            (46, "Writing program files", "Writing uninstall.exe"),
            (72, "Writing registry entries and shortcuts", "Registering file associations"),
            (92, "Verifying installation", "Registering uninstall information"),
        ],
        ("install", Lang::Zh) => [
            (14, "正在解压程序文件", "正在写入 assetmanager.exe"),
            (46, "正在写入程序文件", "正在写入 uninstall.exe"),
            (72, "正在写注册表与快捷方式", "正在注册文件关联"),
            (92, "正在校验安装完整性", "正在登记卸载信息"),
        ],
        (_, Lang::En) => [
            (18, "Removing program files", "assetmanager.exe removed"),
            (46, "Cleaning up shortcuts", "Start menu shortcut removed"),
            (72, "Cleaning up the registry", "Uninstall registry key removed"),
            (92, "Finishing up", "File associations removed"),
        ],
        (_, Lang::Zh) => [
            (18, "正在移除程序文件", "已移除 assetmanager.exe"),
            (46, "正在清理快捷方式", "已删除开始菜单快捷方式"),
            (72, "正在清理注册表", "已清理卸载信息注册表项"),
            (92, "正在完成清理", "已移除文件关联"),
        ],
    };
    for (pct, label, log) in steps {
        std::thread::sleep(std::time::Duration::from_millis(420));
        emit(
            app,
            Progress {
                percent: pct,
                label: label.to_string(),
                sub: el(),
                log: log.to_string(),
                done: false,
                failed: None,
                cancelled: false,
            },
        );
    }
}

/* ── 系统语言 ──
 *
 * 需求口径只有一条：**中文系统给中文，其余一律英文**。所以这里只判"是不是中文"，
 * 不做 en-US / fr-FR / ja-JP 的细分。
 *
 * 用 `GetUserDefaultUILanguage()`（安装器所在系统的 UI 语言）而不是 `LANG`：
 * GUI 进程从资源管理器启动时根本没有 `LANG`。LANGID 的低 10 位是 primary
 * language id，中文 = 0x04（zh-CN 0x0804 / zh-TW 0x0404 / zh-HK 0x0C04 都命中）。
 * 再用 `LCIDToLocaleName` 把同一个 LANGID 摊成 BCP47（`zh-Hans-CN` 等）复核一次，
 * 覆盖"UI 语言是中文但语言包给的是别的变体"这种边角。
 */
#[cfg(windows)]
fn system_lang() -> Lang {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
        fn LCIDToLocaleName(locale: u32, name: *mut u16, len: i32, flags: u32) -> i32;
    }

    const LANG_CHINESE: u16 = 0x04;
    const LOCALE_NAME_MAX_LENGTH: usize = 85;
    unsafe {
        let langid = GetUserDefaultUILanguage();
        if langid & 0x3ff == LANG_CHINESE {
            return Lang::Zh;
        }
        // LCID = MAKELCID(langid, SORT_DEFAULT)；SORT_DEFAULT 是 0，所以直接等于 langid
        let mut buf = [0u16; LOCALE_NAME_MAX_LENGTH];
        let n = LCIDToLocaleName(langid as u32, buf.as_mut_ptr(), buf.len() as i32, 0);
        if n > 1 {
            let name = String::from_utf16_lossy(&buf[..(n as usize - 1)]);
            if Lang::parse(&name) == Some(Lang::Zh) {
                return Lang::Zh;
            }
        }
        Lang::En
    }
}

#[cfg(not(windows))]
fn system_lang() -> Lang {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            if let Some(l) = Lang::parse(&v) {
                return l;
            }
        }
    }
    Lang::En
}

/* ── 平台相关：注册表 / 空间 / 提权 / 进程 ──
 *
 * 全部走手写 FFI，不引 `windows-sys`：本仓库的 Cargo.toml 里没有它，而且
 * `core/src/shellthumb.rs` 已经证明这条路线够用（打开 Win32_UI_Shell 的
 * `windows` crate 曾把 rustc 工作集顶到 37 GB）。 */

/// 扫一遍进程表，找出**除自己以外**的 `assetmanager.exe`。
///
/// 为什么不用 `taskkill /IM`：它会把界面自己（名字也叫 assetmanager.exe）一起杀掉，
/// 而且拿不到 pid / 路径，没法在对话框里告诉用户"关的是哪一个"。
#[cfg(windows)]
fn running_instances() -> Vec<RunningProc> {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(snap: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snap: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: isize = -1;

    let me = std::process::id();
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == INVALID_HANDLE_VALUE {
            log_line("[setup] 无法创建进程快照，跳过运行中检测");
            return out;
        }
        let mut entry: ProcessEntry32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            // szExeFile 是定长缓冲，\0 之后是垃圾数据，必须截断
            let raw = String::from_utf16_lossy(&entry.sz_exe_file);
            let name = raw.split('\0').next().unwrap_or("");
            if entry.th32_process_id != me && name.eq_ignore_ascii_case(MAIN_EXE) {
                out.push(RunningProc {
                    pid: entry.th32_process_id,
                    path: process_image_path(entry.th32_process_id).unwrap_or_default(),
                });
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(windows)]
fn process_image_path(pid: u32) -> Option<String> {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn QueryFullProcessImageNameW(h: *mut c_void, flags: u32, buf: *mut u16, len: *mut u32) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(windows)]
fn terminate_pid(pid: u32) -> bool {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn TerminateProcess(h: *mut c_void, code: u32) -> i32;
        fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }

    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        // 等它真的退出：不等的话紧接着的 File 覆盖仍然会撞上"文件被占用"
        if ok {
            WaitForSingleObject(h, 3000);
        }
        CloseHandle(h);
        ok
    }
}

/// 当前用户的桌面目录 + `AssetManager.lnk`（OneDrive / 企业重定向也能拿到真路径）。
#[cfg(windows)]
fn desktop_shortcut_path() -> Option<PathBuf> {
    use std::ffi::c_void;

    #[link(name = "shell32")]
    extern "system" {
        fn SHGetFolderPathW(
            hwnd: *mut c_void,
            csidl: i32,
            token: *mut c_void,
            flags: u32,
            path: *mut u16,
        ) -> i32;
    }

    const CSIDL_DESKTOPDIRECTORY: i32 = 0x0010;
    let mut buf = [0u16; 260];
    let hr = unsafe {
        SHGetFolderPathW(
            std::ptr::null_mut(),
            CSIDL_DESKTOPDIRECTORY,
            std::ptr::null_mut(),
            0,
            buf.as_mut_ptr(),
        )
    };
    if hr < 0 {
        return None;
    }
    let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])).join(format!("{}.lnk", PRODUCT)))
}

#[cfg(not(windows))]
fn running_instances() -> Vec<RunningProc> {
    Vec::new()
}

#[cfg(not(windows))]
fn terminate_pid(_pid: u32) -> bool {
    false
}

#[cfg(not(windows))]
fn desktop_shortcut_path() -> Option<PathBuf> {
    None
}

/// 删掉界面自己用的 WebView2 数据目录（`%TEMP%\AssetManager-setup-webview`）。
///
/// NSIS 侧的 `RMDir /r` 跑在界面刚退出那一刻，WebView2 的子进程往往还没放开句柄，
/// 结果每次装卸都在 `%TEMP%` 里留 1.3 MB。改成**由界面自己在退出后删**
/// （进程已经走到 `app.run` 之后，句柄基本都放开了），删不动就重试 3 秒。
fn cleanup_webview_dir() {
    let dir = std::env::temp_dir().join(TEMP_WEBVIEW_DIR);
    if !dir.exists() {
        return;
    }
    for _ in 0..30 {
        if std::fs::remove_dir_all(&dir).is_ok() {
            log_line("[setup] 已清理临时 WebView2 数据目录");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    log_line(&format!(
        "[setup] 临时 WebView2 数据目录未能删除：{}",
        dir.display()
    ));
}

struct InstallInfo {
    dir: PathBuf,
    version: String,
}

#[cfg(windows)]
fn install_info() -> Option<InstallInfo> {
    use std::ffi::c_void;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: *mut c_void,
            sub: *const u16,
            opts: u32,
            sam: u32,
            out: *mut *mut c_void,
        ) -> i32;
        fn RegQueryValueExW(
            key: *mut c_void,
            name: *const u16,
            reserved: *mut u32,
            ty: *mut u32,
            data: *mut u8,
            len: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: *mut c_void) -> i32;
    }

    // 预定义 HKEY：Windows 头文件里是 `(HKEY)(ULONG_PTR)((LONG)0x80000001)` ——
    // 先按 32 位有符号取负，再符号扩展成指针宽度。直接写 0x80000001 在 64 位下
    // 少了高 32 位的 F，RegOpenKeyExW 会当成一个非法句柄。
    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    const KEY_READ: u32 = 0x0002_0019;
    const ERROR_SUCCESS: i32 = 0;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    fn read(root: isize, name: &str) -> Option<String> {
        let sub = wide(UNINSTALL_KEY);
        let val = wide(name);
        let mut key: *mut c_void = std::ptr::null_mut();
        let rc = unsafe {
            RegOpenKeyExW(
                root as *mut c_void,
                sub.as_ptr(),
                0,
                KEY_READ,
                &mut key,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegQueryValueExW(
                key,
                val.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut u8,
                &mut len,
            )
        };
        unsafe { RegCloseKey(key) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let n = (len as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]).trim_matches('"').to_string())
    }

    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        if let Some(ver) = read(root, "DisplayVersion") {
            let dir = read(root, "InstallLocation")
                .map(PathBuf::from)
                .unwrap_or_else(default_install_dir);
            return Some(InstallInfo { dir, version: ver });
        }
    }
    None
}

#[cfg(not(windows))]
fn install_info() -> Option<InstallInfo> {
    None
}

/// NSIS 的 per-user 默认安装位置（`$LOCALAPPDATA\${PRODUCTNAME}`）。
fn default_install_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        PathBuf::from(local).join(PRODUCT)
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// 默认数据目录：0.5.7 起是**安装目录下的 `data\`**（主程序用的是同一个规则）；
/// 升级安装时新目录还没生成，而老数据还在 `%LOCALAPPDATA%\<identifier>` ——
/// 那时按老的显示，用户看到的是"我现在数据在哪"，首次启动后主程序会把它们复制过来。
fn default_data_dir(install_dir: &Path) -> PathBuf {
    let beside = install_dir.join("data");
    if beside.is_dir() {
        return beside;
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let old = PathBuf::from(local).join(BUNDLE_ID);
        if old.join("index.db").is_file() {
            return old;
        }
    }
    beside
}

/// 用户设过「数据文件夹」就显示真地址（`data-root.json`），否则默认目录。
///
/// 只读那个 JSON，**不**走 `am_core::dataroot::resolve`（它会写一个探针文件）；
/// setup 模式连一个字节的用户数据都不该碰。
fn resolved_data_dir(install_dir: &Path) -> PathBuf {
    let default = default_data_dir(install_dir);
    let cfg = am_core::dataroot::load(&default);
    if !cfg.enabled {
        return default;
    }
    let Some(cur) = cfg.current.as_deref().filter(|s| !s.trim().is_empty()) else {
        return default;
    };
    let p = PathBuf::from(cur);
    if p.is_dir() {
        p
    } else {
        // 移动硬盘拔了 / 网络盘断线：和主程序一样回退默认目录，不报错
        default
    }
}

fn dir_size_inner(root: &Path, skip: Option<&str>, max_entries: usize) -> u64 {
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            if seen >= max_entries {
                return total;
            }
            seen += 1;
            if skip.is_some_and(|s| e.file_name().to_str() == Some(s)) {
                continue;
            }
            let Ok(ft) = e.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(e.path());
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

fn human(bytes: u64) -> String {
    let gb = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1024.0 / 1024.0)
    }
}

#[cfg(windows)]
fn free_space(path: &Path) -> u64 {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            free: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }

    // 取盘根：`C:\a\b` → `C:\`；目录可能还不存在，所以按字符串取第一个分量
    let s = path.display().to_string();
    let root = if s.len() >= 2 && s.as_bytes()[1] == b':' {
        format!("{}\\", &s[..2])
    } else {
        s.clone()
    };
    let w: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    let mut total = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    if ok == 0 {
        0
    } else {
        free
    }
}

#[cfg(not(windows))]
fn free_space(_path: &Path) -> u64 {
    0
}

#[cfg(windows)]
fn is_elevated() -> bool {
    #[link(name = "shell32")]
    extern "system" {
        fn IsUserAnAdmin() -> i32;
    }
    unsafe { IsUserAnAdmin() != 0 }
}

#[cfg(not(windows))]
fn is_elevated() -> bool {
    false
}

fn open_path(p: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(p)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = p;
        Err("暂不支持".into())
    }
}

fn spawn_detached(exe: &Path, args: &[String]) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().map(|_| ())
}

/// 静默拉起安装/卸载引擎。`tail` 是必须在**最后**、且不加引号的参数（NSIS 的 `/D=`）。
#[cfg(windows)]
fn spawn_silent(
    exe: &Path,
    args: &[&str],
    tail: Option<&str>,
) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    if let Some(t) = tail {
        cmd.raw_arg(t);
    }
    cmd.creation_flags(0x0800_0000);
    cmd.spawn()
}

#[cfg(not(windows))]
fn spawn_silent(
    exe: &Path,
    args: &[&str],
    tail: Option<&str>,
) -> std::io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    if let Some(t) = tail {
        cmd.arg(t);
    }
    cmd.spawn()
}

/// 安装器日志：`%TEMP%\AssetManager-setup.log`。
///
/// 追加写、不轮转 —— 单次安装最多几十行，出问题时用户把这个文件发过来就够定位。
fn log_line(msg: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join(LOG_NAME);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{stamp} {msg}");
    }
}
