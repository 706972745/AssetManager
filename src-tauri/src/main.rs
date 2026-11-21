//! Tauri 壳：只做三件事 —— 暴露命令、注册 thumb:// 协议、管窗口。
//! 所有业务逻辑都在 `am-core`，这里不写算法。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use am_core::fsops::MovePolicy;
use am_core::model::{Asset, AssetQuery, DirNode, ScanStats};
use am_core::{clipimg, db, fsops, keywatch, scan, screen, thumb, watch, App};
use serde::{Deserialize, Serialize};
use tauri::http::{header, Response};
use tauri::{AppHandle, Emitter, Manager, State, Window};

mod snip_native;

/// 检查更新 / 下载 / 校验 / 静默安装（照 FontManager 那一套搬过来，
/// 仓库名与安装形态判据换成本仓的）。
mod update;

/// 三种导出文件的双击关联（0.4.2）：只写 HKCU，免管理员，值没变就不写。
mod file_assoc;

/// 单实例守卫 + 唤起 + 「双击文件把路径递给已开着的实例」。
mod single_instance;

struct AppState(Mutex<App>);

/// 日志目录（默认数据目录）。setup 里设一次，panic hook 与退出回调都从这儿取。
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

fn lock<'a>(state: &'a State<'_, AppState>) -> std::sync::MutexGuard<'a, App> {
    state.0.lock().unwrap_or_else(|e| e.into_inner())
}

/* ---------------- 解码闸门：内存峰值的唯一刹车 ----------------
   **按像素估重，不按张数**（2026-11-16 改）。

   以前是「同时 4 张」——小截图和大合成图一视同仁。结果：一屏 40 张 2MP 的图要排 10 轮，
   用户滑一下等好几秒；而真有 36MP 大图时 4 张又已经吃掉 550 MB。

   现在是**份额制**：1 份 ≈ 4 MP 的解码内存（16 MB 量级）。
   · 2MP 的小图 → 1 份，最多 10 张同时跑；
   · 24MP 的照片 → 6 份，同时 1 张（JPEG 走 DCT 降采样，实际只解 1/64 的像素，估重按降采样后算）；
   · 36MP 的合成大图 → 9 份，独占闸门 —— 宁可它自己慢，也不能把进程顶死。
   内存峰值大致封在 160–200 MB，和以前一样安全，但小图吞吐翻一倍多。 */
/// 一份对应的像素量（4 MP ≈ 解码后 12 MB RGB）
const DECODE_UNIT_PX: u64 = 4_000_000;

/// 闸门份额按**机器档位**给（P2.5 T42）。
///
/// 以前写死 10 份：4 核 8G 的老机器上 10 路同时解码会被拖到卡顿，
/// 32G 的新机器上又白白浪费一半余量。现在按 核数 + 物理内存 分四档
/// （照果核看图的 low/mid/high/ultra）：
/// * 低档 ≤4 核或 ≤8G → 5 份；
/// * 中档 → 7 份；
/// * 高档（本机：16 核 / 32G 级）→ 10 份；
/// * 极高档 → 14 份。
/// 内存峰值仍封在 200 MB 上下（5×4MP×3B ≈ 60MB ~ 14×12MB ≈ 170MB）。
fn decode_budget() -> usize {
    static B: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *B.get_or_init(|| {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let gb = total_ram_gb().unwrap_or(16);
        let b = if cores <= 4 || gb <= 8 {
            5
        } else if cores <= 8 || gb <= 16 {
            7
        } else if cores <= 16 || gb <= 32 {
            10
        } else {
            14
        };
        if let Some(d) = LOG_DIR.get() {
            log_runtime(d, &format!("解码闸门：{cores} 核 / {gb} GB → {b} 份"));
        }
        b
    })
}

/// 物理内存（GB，四舍五入）。拿不到就返回 None，按 16G 算。
#[cfg(windows)]
fn total_ram_gb() -> Option<u64> {
    #[repr(C)]
    struct MemStatus {
        len: u32,
        load: u32,
        total: u64,
        avail: u64,
        total_page: u64,
        avail_page: u64,
        total_virt: u64,
        avail_virt: u64,
        avail_ext: u64,
    }
    extern "system" {
        fn GlobalMemoryStatusEx(m: *mut MemStatus) -> i32;
    }
    unsafe {
        let mut m: MemStatus = std::mem::zeroed();
        m.len = std::mem::size_of::<MemStatus>() as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return None;
        }
        Some((m.total as f64 / 1024.0 / 1024.0 / 1024.0).round() as u64)
    }
}

#[cfg(not(windows))]
fn total_ram_gb() -> Option<u64> {
    None
}

struct DecodeGate {
    free: Mutex<usize>,
    cv: Condvar,
}

fn gate() -> &'static DecodeGate {
    static G: OnceLock<DecodeGate> = OnceLock::new();
    G.get_or_init(|| DecodeGate { free: Mutex::new(decode_budget()), cv: Condvar::new() })
}

struct DecodeSlot {
    cost: usize,
}
impl Drop for DecodeSlot {
    fn drop(&mut self) {
        let g = gate();
        *g.free.lock().unwrap_or_else(|e| e.into_inner()) += self.cost;
        g.cv.notify_all();
    }
}

/// 估一张图解码时要占多少份。
///
/// JPEG 走 DCT 降采样（`thumb::decode_jpeg_scaled`），真正解出来的只有目标尺寸附近，
/// 所以按 `(px·2)²` 估，而不是原图像素 —— 否则 24MP 照片会被误判成"重活"，
/// 又回到「4 张一起卡」的老问题。
fn decode_cost(path: &PathBuf, px: u32, db_px: u64) -> usize {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let jpeg = matches!(ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif");
    let src = if db_px > 0 { db_px } else { 4_000_000 };
    let est = if jpeg {
        src.min(((px as u64) * 2).pow(2))
    } else {
        src
    };
    ((est / DECODE_UNIT_PX) as usize + 1).clamp(1, decode_budget() - 1)
}

fn acquire_decode(cost: usize) -> DecodeSlot {
    let g = gate();
    let mut n = g.free.lock().unwrap_or_else(|e| e.into_inner());
    while *n < cost {
        n = g.cv.wait(n).unwrap_or_else(|e| e.into_inner());
    }
    *n -= cost;
    DecodeSlot { cost }
}

/* ---------------- 扫描控制（T03）：一次只跑一个，可取消 ---------------- */

struct ScanCtl {
    cancel: Arc<AtomicBool>,
    busy: AtomicBool,
}

fn scan_ctl() -> &'static ScanCtl {
    static S: OnceLock<ScanCtl> = OnceLock::new();
    S.get_or_init(|| ScanCtl {
        cancel: Arc::new(AtomicBool::new(false)),
        busy: AtomicBool::new(false),
    })
}

/* ---------------- 目录监听（T06） ---------------- */

struct WatchCtl {
    stop: Arc<AtomicBool>,
}

struct WatchState(Mutex<Option<WatchCtl>>);

/* ------------------------------------------------------------------ 命令 */

#[tauri::command]
fn am_roots(state: State<AppState>) -> Result<Vec<DirNode>, String> {
    lock(&state).roots()
}

#[tauri::command]
fn am_set_rating(state: State<AppState>, ids: Vec<i64>, rating: i32) -> Result<usize, String> {
    lock(&state).set_rating(&ids, rating)
}

#[tauri::command]
fn am_set_note(state: State<AppState>, id: i64, note: String) -> Result<(), String> {
    lock(&state).set_note(id, &note)
}

#[tauri::command]
fn am_set_source(state: State<AppState>, id: i64, url: String) -> Result<(), String> {
    lock(&state).set_source_url(id, &url)
}

/// 用默认浏览器打开「来源 URL」。协议白名单在 fsops::open_url 里。
#[tauri::command]
fn am_open_url(url: String) -> Result<(), String> {
    fsops::open_url(&url)
}

/* ---------------- 检查更新（用户 2026-11-18：「自动更新的页面和功能也加上，
   和 fontmanager 一样」）----------------
   三个命令的边界写在 `update.rs` 顶部：检查只读、下载必须过哈希、安装只给
   按用户安装的 NSIS 版。这里只做「把阻塞活扔到线程池」这一件事 ——
   下载几 MB 跑在主线程上会把界面冻住。 */

#[tauri::command]
async fn am_update_check(force: bool) -> Result<update::UpdateInfo, String> {
    update::check(force).await
}

#[tauri::command]
async fn am_update_download(
    app: AppHandle,
    url: String,
    name: String,
    sha256: String,
    asset_id: u64,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        update::download_setup(&app, &url, &name, &sha256, asset_id)
    })
    .await
    .map_err(|e| format!("下载任务异常：{e}"))?
}

#[tauri::command]
fn am_update_install(
    app: AppHandle,
    path: String,
    sha256: String,
    version: Option<String>,
) -> Result<(), String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        update::install_setup(&app, &path, &sha256, version.as_deref().unwrap_or(""))
    }))
    .unwrap_or_else(|_| Err("安装出错（已拦下，软件继续运行）".into()))
}

#[tauri::command]
fn am_children(parent: Option<i64>, state: State<AppState>) -> Result<Vec<DirNode>, String> {
    lock(&state).tree(parent)
}

/// 已加入的**用户**库根数量（前端加库前先问一次，最多 3 个）。
///
/// 内置截图库不算名额：它一直存在，算进去等于用户只有 2 个位置。
/// **所有** builtin 都扣（不只是当前 exe 那一个）—— 同一个软件有 debug / release /
/// 已安装三种 exe，各自都会把自己的截图目录登记成库根；只扣一个的话，
/// 用户明明只加了 2 个库，侧栏却写着 3/3（owner 2026-11-17 截图就是这情况）。
#[tauri::command]
fn am_lib_count(state: State<AppState>) -> Result<i64, String> {
    let app = lock(&state);
    let roots = app.lib_roots()?;
    /* 只数**真实存在**的用户库根：路径已经不在的（用户手输错、盘拔了）不该占名额 ——
       它们连一行素材都索引不到，占着位置只会让"再加一个库"被莫名其妙顶回来。 */
    let user = roots
        .iter()
        .filter(|r| !is_builtin_root(r) && std::path::Path::new(r).is_dir())
        .count() as i64;
    Ok(user)
}

/// 内置库的路径（前端据此：不计入 3 个名额、右键不给「移除」、永远显示成一行「截图」）。
///
/// 返回的是**所有** builtin 根：`Tree.tsx` 把它当"这行不是用户库"的判据，
/// 谁被漏掉谁就会以 `screenshots` 的英文名混进素材库列表、还占一个名额。
#[tauri::command]
fn am_builtin_roots(state: State<AppState>) -> Result<Vec<String>, String> {
    let mut app = lock(&state);
    let roots = builtin_roots_of(&app)?;
    /* 顺手刷新 core 那份（用户可能刚把某个 screenshots 目录加进来） */
    app.set_builtin_roots(roots.clone());
    Ok(roots)
}

/// 从素材库移除一个库根：只清索引，磁盘上的文件一个都不动。
#[tauri::command]
fn am_remove_lib(path: String, state: State<AppState>) -> Result<usize, String> {
    if is_builtin_root(&path) {
        return Err("这是软件自己的截图目录（内置库），不能移除 —— 截图、贴图、灵感板粘贴都往这里存。".into());
    }
    lock(&state).remove_lib(&path)
}

/// 切库：把"当前库"切到这条路径所在的库。
///
/// 每个库**自带一套标签体系**（存在库自己的 `.AssetManager/lib.db` 里），
/// 所以切库之后侧栏要重新拉 tags / groups —— 不切的话 A 库的标签会挂在 B 库上，
/// 正是 owner 要避免的"数据混在一起"。
#[tauri::command]
fn am_set_current_lib(path: String, state: State<AppState>) -> Result<bool, String> {
    lock(&state).set_current_lib(&path)
}

#[tauri::command]
fn am_list(q: AssetQuery, state: State<AppState>) -> Result<Vec<Asset>, String> {
    lock(&state).list(&q)
}

#[tauri::command]
fn am_total(state: State<AppState>) -> Result<i64, String> {
    lock(&state).total()
}

/* ---------------- 标签（手动落库 / 自动运行时推导） ---------------- */

/// 侧栏用：手工标签 + 使用数
#[tauri::command]
fn am_tags(state: State<AppState>) -> Result<Vec<am_core::model::TagCount>, String> {
    lock(&state).tags()
}

/// 侧栏用：自动标签 + 使用数（扩展名 / 类型 / 像素档 / 比例 / 体积档，全在 SQL 里聚合）
#[tauri::command]
fn am_auto_tags(state: State<AppState>) -> Result<Vec<am_core::model::TagCount>, String> {
    lock(&state).auto_tags()
}

/// 当前页每个素材各自的手工标签（一次 IPC 拿一页，别一行一个请求）
#[tauri::command]
fn am_asset_tags(ids: Vec<i64>, state: State<AppState>) -> Result<Vec<(i64, Vec<String>)>, String> {
    lock(&state).tags_for(&ids)
}

/// 详情面板用：按**原始文件事实**推导这个素材的自动标签，并套上「手工优先」。
/// 推导在 core 里只有一份实现，界面不另写一套规则（否则两边口径迟早分叉）。
#[tauri::command]
fn am_auto_tags_for(
    asset: Asset,
    manual: Vec<String>,
) -> Vec<am_core::autotag::AutoTag> {
    am_core::autotag::apply_manual_priority(am_core::autotag::derive(&asset), &manual)
}

/// 批量打标签（单个素材也走这个）。返回真正新增的条数，已经打过的重复调用无害。
/// 建空标签（侧栏「+ 标签」）。已存在就静默成功，不报错。
#[tauri::command]
fn am_tag_create(name: String, state: State<AppState>) -> Result<(), String> {
    let mut a = lock(&state);
    a.create_tag(&name)
}

#[tauri::command]
fn am_tag_assets(ids: Vec<i64>, tag: String, state: State<AppState>) -> Result<i64, String> {
    lock(&state).tag_assets(&ids, &tag)
}

/// 批量去掉标签（标签本身保留，可能别的素材还在用）
#[tauri::command]
fn am_untag_assets(ids: Vec<i64>, tag: String, state: State<AppState>) -> Result<i64, String> {
    lock(&state).untag_assets(&ids, &tag)
}

/// 从所有素材上移除这个标签（侧栏右键）
#[tauri::command]
fn am_delete_tag(tag: String, state: State<AppState>) -> Result<i64, String> {
    lock(&state).delete_tag(&tag)
}

/* ---------------- 标签组 / 颜色（0.5.0） ---------------- */

/// 侧栏用：标签组 + 每组多少素材 / 多少个标签
#[tauri::command]
fn am_tag_groups(state: State<AppState>) -> Result<Vec<am_core::model::TagGroup>, String> {
    lock(&state).groups()
}

/// 改标签颜色（传 null = 恢复默认灰）
#[tauri::command]
fn am_tag_set_color(tag: String, color: Option<String>, state: State<AppState>) -> Result<(), String> {
    lock(&state).set_tag_color(&tag, color.as_deref())
}

/// 把标签放进分组（null = 移回未分组，分组不存在会自动建）
#[tauri::command]
fn am_tag_set_group(tag: String, group: Option<String>, state: State<AppState>) -> Result<(), String> {
    lock(&state).set_tag_group(&tag, group.as_deref())
}

/// 标签改名；新名已存在时**合并**（返回合并过去的素材数）
#[tauri::command]
fn am_tag_rename(old: String, new: String, state: State<AppState>) -> Result<i64, String> {
    lock(&state).rename_tag(&old, &new)
}

#[tauri::command]
fn am_group_new(name: String, state: State<AppState>) -> Result<(), String> {
    lock(&state).group_new(&name)
}

#[tauri::command]
fn am_group_rename(old: String, new: String, state: State<AppState>) -> Result<(), String> {
    lock(&state).group_rename(&old, &new)
}

#[tauri::command]
fn am_group_set_color(
    name: String,
    color: Option<String>,
    state: State<AppState>,
) -> Result<(), String> {
    lock(&state).group_set_color(&name, color.as_deref())
}

/// 解散分组：**标签不删**，只是回到「未分组」。返回被解散的标签数。
#[tauri::command]
fn am_group_delete(name: String, state: State<AppState>) -> Result<i64, String> {
    lock(&state).group_delete(&name)
}

/// 同步扫描（CLI / 冒烟用）。界面走 `am_scan_start`（带进度与取消）。
#[tauri::command]
fn am_scan(path: String, state: State<AppState>) -> Result<String, String> {
    let mut app = lock(&state);
    let t = std::time::Instant::now();
    let s = app.scan(&path, |_p| {})?;
    let mut msg = format!(
        "{} 个目录 · {} 个文件 · {} 张图有尺寸 · {} ms",
        s.dirs, s.files, s.with_dims, t.elapsed().as_millis()
    );
    if s.skipped > 0 {
        msg.push_str(&format!(" · 跳过 {} 个外部素材库目录", s.skipped));
    }
    Ok(msg)
}

/// 启动后台扫描：**立刻返回**，进度走 `scan://progress`，结束走 `scan://done`。
///
/// 为什么要单开线程 + 单开连接：
/// 1. 扫描要几分钟，占着全局 `Mutex<App>` 会让列表、缩略图全部卡死；
/// 2. 自己的连接 = 自己的事务，扫到一半被取消也不会把主库锁住。
#[tauri::command]
fn am_scan_start(app: AppHandle, path: String, state: State<AppState>) -> Result<(), String> {
    let ctl = scan_ctl();
    if ctl.busy.swap(true, Ordering::SeqCst) {
        return Err("已经在扫描了，等这次跑完（或点取消）".into());
    }
    ctl.cancel.store(false, Ordering::SeqCst);
    let data_dir = lock(&state).data_dir.clone();
    let cancel = ctl.cancel.clone();

    std::thread::spawn(move || {
        let mut failed: Option<String> = None;
        match open_app(&data_dir) {
            Ok(mut a) => {
                match a.scan_cancelable(&path, &cancel, |p| {
                    let _ = app.emit("scan://progress", p);
                }) {
                    Ok(s) => {
                        let mut msg = format!(
                            "{} 个目录 · {} 个文件 · {} 张图有尺寸 · {:.1} 秒",
                            s.dirs, s.files, s.with_dims, s.elapsed_ms as f64 / 1000.0
                        );
                        if s.skipped > 0 {
                            msg.push_str(&format!(" · 跳过 {} 个外部素材库目录", s.skipped));
                        }
                        let _ = app.emit("scan://done", ScanDone { ok: true, message: msg, stats: s });
                    }
                    Err(e) => failed = Some(e),
                }
            }
            Err(e) => failed = Some(e),
        }
        if let Some(e) = failed {
            let _ = app.emit(
                "scan://done",
                ScanDone { ok: false, message: e, stats: ScanStats::default() },
            );
        }
        scan_ctl().busy.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[derive(Clone, Serialize)]
struct ScanDone {
    ok: bool,
    message: String,
    stats: ScanStats,
}

/* ---------------- 全库缩略图预热（2026-11-16）----------------
   预取解决"下一屏"，但第一次浏览某个很久没打开的目录，图还是要现做。
   这里在后台把**全库**的缩略图按当前档位生成一遍（低优先、可停、有进度）：
   等它跑完，滚动就只剩读文件，任何位置都是"立刻出来"。

   四条纪律：
   1. **永远低优先**：只有 2 条 lane，且最近 0.8 秒内有可见请求就先睡 250ms 让路；
   2. **可停**：epoch 一改，下一张之前就退出（换档位、用户点停、关软件）；
   3. **不写库**：生成的图落在缓存里，真请求到来时 `ensure_path` 命中、再补写 thumb_key；
   4. **幂等**：已经在盘上的直接跳过，重复启动不会重算。 */
struct WarmState {
    running: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    epoch: AtomicU64,
}

fn warm() -> &'static WarmState {
    static W: OnceLock<WarmState> = OnceLock::new();
    W.get_or_init(|| WarmState {
        running: AtomicBool::new(false),
        done: AtomicU64::new(0),
        total: AtomicU64::new(0),
        epoch: AtomicU64::new(0),
    })
}

/// 最近一次「可见区请求缩略图」的时间（毫秒时间戳）。预热靠它避让。
static LAST_THUMB_REQ: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct WarmProgress {
    running: bool,
    done: u64,
    total: u64,
}

fn warm_payload() -> WarmProgress {
    let w = warm();
    WarmProgress {
        running: w.running.load(Ordering::Relaxed),
        done: w.done.load(Ordering::Relaxed),
        total: w.total.load(Ordering::Relaxed),
    }
}

#[tauri::command]
fn am_warm_status() -> WarmProgress {
    warm_payload()
}

#[tauri::command]
fn am_warm_stop() -> WarmProgress {
    let w = warm();
    w.epoch.fetch_add(1, Ordering::SeqCst); // 让正在跑的循环下一张就退
    w.running.store(false, Ordering::SeqCst);
    warm_payload()
}

#[tauri::command]
fn am_warm_start(app: AppHandle, state: State<AppState>, px: u32) -> Result<WarmProgress, String> {
    let w = warm();
    if w.running.load(Ordering::SeqCst) {
        return Ok(warm_payload());
    }
    // 待办清单：只要图片（视频/文档走系统或根本没有图）
    struct Job {
        path: PathBuf,
        src_px: u64,
    }
    let (store, jobs) = {
        let a = lock(&state);
        let store = thumb::ThumbStore::new(a.thumbs.root.clone());
        let mut jobs: Vec<Job> = Vec::new();
        for l in &a.libs {
            let mut st = l
                .conn
                .prepare(
                    "SELECT a.id, a.name, a.width, a.height, d.path AS dpath
                     FROM assets a JOIN dirs d ON d.id=a.dir_id
                     WHERE a.kind=0 ORDER BY a.id",
                )
                .map_err(|e| e.to_string())?;
            let rows = st
                .query_map([], |r| {
                    let id: i64 = r.get(0)?;
                    let name: String = r.get(1)?;
                    let w: Option<i64> = r.get(2)?;
                    let h: Option<i64> = r.get(3)?;
                    let dpath: String = r.get(4)?;
                    Ok((id, name, w.unwrap_or(0), h.unwrap_or(0), dpath))
                })
                .map_err(|e| e.to_string())?;
            for row in rows.flatten() {
                let (_id, name, wd, ht, dpath) = row;
                let path = if dpath.ends_with('\\') || dpath.ends_with('/') {
                    PathBuf::from(format!("{dpath}{name}"))
                } else {
                    PathBuf::from(format!("{dpath}\\{name}"))
                };
                jobs.push(Job { path, src_px: wd.max(0) as u64 * ht.max(0) as u64 });
            }
        }
        (store, jobs)
    };

    let epoch = w.epoch.fetch_add(1, Ordering::SeqCst) + 1;
    w.total.store(jobs.len() as u64, Ordering::SeqCst);
    w.done.store(0, Ordering::SeqCst);
    w.running.store(true, Ordering::SeqCst);
    let _ = app.emit("warm://progress", warm_payload());

    let app2 = app.clone();
    std::thread::spawn(move || {
        let mut last_emit = std::time::Instant::now();
        for (i, j) in jobs.iter().enumerate() {
            if w.epoch.load(Ordering::SeqCst) != epoch {
                break;
            }
            let key = thumb::ThumbStore::key_for(&j.path, px);
            if !store.exists(&key) {
                // 让路：最近有可见请求就先歇一会（用户正在滚动，别跟他抢 CPU）
                if now_ms().saturating_sub(LAST_THUMB_REQ.load(Ordering::Relaxed)) < 800 {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    if w.epoch.load(Ordering::SeqCst) != epoch {
                        break;
                    }
                }
                let cost = decode_cost(&j.path, px, j.src_px).min(2);
                let _slot = acquire_decode(cost);
                let _ = thumb::ensure_path(&j.path, &store, px);
            }
            w.done.store(i as u64 + 1, Ordering::Relaxed);
            if last_emit.elapsed().as_millis() >= 400 || i + 1 == jobs.len() {
                last_emit = std::time::Instant::now();
                let _ = app2.emit("warm://progress", warm_payload());
            }
        }
        w.running.store(false, Ordering::SeqCst);
        let _ = app2.emit("warm://progress", warm_payload());
    });
    Ok(warm_payload())
}

/* ---------------- 缩略图预取：把「下一屏」先做好 ----------------
   滑动的等待感来自「滚到哪才开始生成」。这里让界面在滚动停下后，提前把可视区
   上下各一屏的缩略图做掉 —— 用户滑过去时多半已经命中缓存，只剩读文件。

   三条纪律：
   1. **同时只有一个预取任务**：新调用把 epoch +1，老任务在下一张之前就退出，
      不会出现「滚到新目录了，后台还在给旧列表出图」；
   2. 用**同一道解码闸门**，所以预取永远抢不走全部能力，可见区照样优先；
   3. 命中缓存直接跳过，不写库（`ensure_path` 自己查盘，之后真请求会补写）。 */
static PREFETCH_EPOCH: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
fn am_prefetch_thumbs(state: State<AppState>, ids: Vec<i64>, px: u32) -> Result<(), String> {
    let epoch = PREFETCH_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    struct Job {
        path: PathBuf,
        src_px: u64,
    }
    let (store, jobs) = {
        let app = lock(&state);
        let store = thumb::ThumbStore::new(app.thumbs.root.clone());
        let mut jobs = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(Some(a)) = app.asset(id) {
                jobs.push(Job {
                    path: PathBuf::from(&a.path),
                    src_px: a.width.unwrap_or(0) as u64 * a.height.unwrap_or(0) as u64,
                });
            }
        }
        (store, jobs)
    };
    std::thread::spawn(move || {
        for j in jobs {
            if PREFETCH_EPOCH.load(Ordering::SeqCst) != epoch {
                break;
            }
            if !j.path.is_file() {
                continue;
            }
            let key = thumb::ThumbStore::key_for(&j.path, px);
            if store.exists(&key) {
                continue;
            }
            let _slot = acquire_decode(decode_cost(&j.path, px, j.src_px));
            let _ = thumb::ensure_path(&j.path, &store, px);
        }
    });
    Ok(())
}

#[tauri::command]
fn am_scan_cancel() -> bool {
    scan_ctl().cancel.store(true, Ordering::SeqCst);
    true
}

#[tauri::command]
fn am_scan_busy() -> bool {
    scan_ctl().busy.load(Ordering::SeqCst)
}

/// 启动增量扫描（T06）：后台 stat 一遍索引里记过的目录，只重扫真变了的。
#[tauri::command]
fn am_incremental(app: AppHandle, limit: Option<i64>, state: State<AppState>) -> Result<(), String> {
    let data_dir = lock(&state).data_dir.clone();
    let limit = limit.unwrap_or(4000);
    let cancel = scan_ctl().cancel.clone();
    std::thread::spawn(move || {
        let Ok(mut a) = open_app(&data_dir) else { return };
        if let Ok(s) = a.incremental(&cancel, limit, |p| {
            let _ = app.emit("scan://progress", p);
        }) {
            let changed = s.dirs + s.removed_dirs;
            if changed > 0 {
                let _ = app.emit(
                    "scan://done",
                    ScanDone {
                        ok: true,
                        message: format!(
                            "增量扫描：{} 个目录有变化 · {} 个文件 · {} ms",
                            changed, s.files, s.elapsed_ms
                        ),
                        stats: s,
                    },
                );
            }
        }
    });
    Ok(())
}

/// 监听一批目录（非递归）。**替换**上一批：界面换了目录就只盯新的。
#[tauri::command]
fn am_watch(app: AppHandle, dirs: Vec<String>, watch_state: State<WatchState>) -> Result<usize, String> {
    let mut guard = watch_state.0.lock().unwrap_or_else(|e| e.into_inner());

    // 先停旧的（线程里的 watcher 会被 drop，系统订阅随之取消）
    if let Some(old) = guard.take() {
        old.stop.store(true, Ordering::SeqCst);
    }

    let paths: Vec<PathBuf> = dirs.iter().map(PathBuf::from).filter(|p| p.is_dir()).collect();
    if paths.is_empty() {
        return Ok(0);
    }
    let n = paths.len();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();

    std::thread::spawn(move || {
        let Some(w) = watch::watch_dirs(&paths) else { return };
        let mut pending: HashSet<String> = HashSet::new();
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            match w.receiver().recv_timeout(Duration::from_millis(350)) {
                Ok(dir) => {
                    pending.insert(dir.to_string_lossy().to_string());
                }
                Err(RecvTimeoutError::Timeout) => {
                    if !pending.is_empty() {
                        let list: Vec<String> = pending.drain().collect();
                        let _ = app.emit("fs://changed", list);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    *guard = Some(WatchCtl { stop });
    Ok(n)
}

/* ---------------- 拖出窗口（T01）：CF_HDROP，出的是原文件 ---------------- */

/// 光标位置 → 窗口内 CSS 像素。拆成两半：
///
/// - `client_origin_scale` 在主线程算一次（客户区屏幕原点 + DPI 缩放）——
///   拖拽期间窗口不会动，所以整个拖拽只取一次就够；
/// - `cursor_pos` 是拖拽期间工作线程每 30ms 调的，只有这一个 Win32 调用，
///   不去碰 tao / Tauri 的窗口对象（那些方法在工作线程上没有保证）。
///
/// 为什么用 `ClientToScreen` 而不是 `outer_position()`：后者是窗口外框（含标题栏），
/// 换算出来的 y 会比真实位置高一个标题栏，落点判定整体偏上。
#[cfg(windows)]
fn client_origin_scale(win: &tauri::WebviewWindow) -> Option<(i32, i32, f64)> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn ClientToScreen(hwnd: isize, p: *mut Point) -> i32;
        fn GetDpiForWindow(hwnd: isize) -> u32;
    }
    let hwnd = win.hwnd().ok()?.0 as isize;
    let mut p = Point { x: 0, y: 0 };
    if unsafe { ClientToScreen(hwnd, &mut p) } == 0 {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
    Some((p.x, p.y, scale))
}

#[cfg(windows)]
fn cursor_pos() -> Option<(i32, i32)> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(p: *mut Point) -> i32;
    }
    let mut p = Point { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut p) } == 0 {
        None
    } else {
        Some((p.x, p.y))
    }
}

#[cfg(not(windows))]
fn cursor_pos() -> Option<(i32, i32)> {
    None
}

#[cfg(not(windows))]
fn client_origin_scale(_win: &tauri::WebviewWindow) -> Option<(i32, i32, f64)> {
    None
}

/// 屏幕坐标 → 窗口内 CSS 像素
fn to_css(origin: (i32, i32, f64), p: (i32, i32)) -> (f64, f64) {
    (
        (p.0 - origin.0) as f64 / origin.2,
        (p.1 - origin.1) as f64 / origin.2,
    )
}

/// 把素材拖出窗口 —— 拖到 PS / 资源管理器 / 浏览器地址栏。
///
/// 三条纪律（规格 §2.4）：
/// 1. **必须 `run_on_main_thread` 后立刻返回**：`DoDragDrop` 会开一个模态消息循环，
///    在命令线程里调它，拖完之前整个 IPC 都被它占着；
/// 2. 出去的是**磁盘原文件**（`DragItem::Files` → CF_HDROP），不是缩略图；
/// 3. COPY|MOVE 同时允许（vendor 补丁，0.4.12）—— 落点自己选：
///    资源管理器/桌面**同盘=移动**（不再复制出副本，owner 2026-11-21）、
///    跨盘=复制；PS / 浏览器只认 COPY，拖过去照旧是复制。
#[tauri::command]
fn am_drag_out(
    app: AppHandle,
    window: tauri::WebviewWindow,
    paths: Vec<String>,
    preview_id: Option<i64>,
    state: State<AppState>,
) -> Result<(), String> {
    let files: Vec<PathBuf> = paths
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect();
    if files.is_empty() {
        return Err("要拖的文件已经不在了（可能刚被移动或删除）".into());
    }
    // 一次最多 50 个：再多，很多目标程序会把拖拽当成误操作直接拒收
    let files: Vec<PathBuf> = files.into_iter().take(50).collect();

    // 拖拽时跟着鼠标的那张小图：拿缓存里的 WebP 缩成 96px 的 PNG（WIC 认 PNG，不认 WebP）
    let preview = preview_id.and_then(|id| {
        let a = lock(&state);
        let mut a = a;
        a.ensure_thumb(id, 128)
            .ok()
            .flatten()
            .and_then(|o| fsops::drag_preview_png(&o.path, 96))
    });

    let win = window.clone();
    window
        .run_on_main_thread(move || {
            let image = match preview {
                Some(png) => drag::Image::Raw(png),
                // 没有预览就用目标文件夹里那张图占位（drag crate 的兜底）
                None => drag::Image::File(files[0].clone()),
            };
            let app2 = app.clone();
            let win2 = win.clone();

            /* 拖放期间的**跟手浮层**靠这条线程推：OLE 模态循环占住主线程，
               WebView 收不到 mousemove、drag-over 事件也不持续（owner 2026-11-20
               实测拖回来浮层不跟手）。GetCursorPos 是纯 Win32 调用，任何线程都能调，
               16ms 一拍 push 给前端 —— 不依赖消息循环的活跃度。 */
            let origin = client_origin_scale(&win);
            let stop_poll = Arc::new(AtomicBool::new(false));
            let stop_poll2 = stop_poll.clone();
            let stop_poll3 = stop_poll.clone();
            let app_poll = app.clone();
            std::thread::spawn(move || {
                while !stop_poll2.load(Ordering::SeqCst) {
                    if let (Some(o), Some(p)) = (origin, cursor_pos()) {
                        let (lx, ly) = to_css(o, p);
                        let _ = app_poll.emit(
                            "am://drag-move",
                            serde_json::json!({ "x": lx, "y": ly }),
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
            });

            let r = drag::start_drag(
                &win,
                drag::DragItem::Files(files),
                image,
                move |result, pos| {
                    stop_poll3.store(true, Ordering::SeqCst);
                    // 松手位置换成本窗口的 CSS 像素，前端据此判断落在哪
                    let (lx, ly) = match (client_origin_scale(&win2), cursor_pos()) {
                        (Some(o), Some(p)) => to_css(o, p),
                        _ => {
                            let scale = win2.scale_factor().unwrap_or(1.0).max(0.5);
                            let origin = win2.inner_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));
                            ((pos.x - origin.0) as f64 / scale, (pos.y - origin.1) as f64 / scale)
                        }
                    };
                    let payload = serde_json::json!({
                        "x": lx,
                        "y": ly,
                        "dropped": matches!(result, drag::DragResult::Dropped),
                    });
                    if let Err(e) = app2.emit("am://drag-end", payload) {
                        eprintln!("拖拽结束事件没发出去：{e}");
                    }
                },
                drag::Options { mode: drag::DragMode::Copy, ..Default::default() },
            );
            if let Err(e) = r {
                eprintln!("拖拽启动失败：{e}");
                stop_poll.store(true, Ordering::SeqCst);
            }
        })
        .map_err(|e| format!("启动拖拽失败：{e}"))?;
    Ok(())
}

/// 重建索引（T13）：清索引 + 全量重扫所有库根。后台跑，进度复用 `scan://progress`。
#[tauri::command]
fn am_rebuild_index(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let ctl = scan_ctl();
    if ctl.busy.swap(true, Ordering::SeqCst) {
        return Err("已经在扫描了，等这次跑完（或点取消）".into());
    }
    ctl.cancel.store(false, Ordering::SeqCst);
    let data_dir = lock(&state).data_dir.clone();
    let cancel = ctl.cancel.clone();

    std::thread::spawn(move || {
        let mut failed: Option<String> = None;
        match open_app(&data_dir) {
            Ok(mut a) => match a.rebuild(&cancel, None, |p| {
                let _ = app.emit("scan://progress", p);
            }) {
                Ok(s) => {
                    let msg = format!(
                        "重建完成：{} 个目录 · {} 个文件 · {:.1} 秒",
                        s.dirs,
                        s.files,
                        s.elapsed_ms as f64 / 1000.0
                    );
                    let _ = app.emit("scan://done", ScanDone { ok: true, message: msg, stats: s });
                }
                Err(e) => failed = Some(e),
            },
            Err(e) => failed = Some(e),
        }
        if let Some(e) = failed {
            let _ = app.emit(
                "scan://done",
                ScanDone { ok: false, message: e, stats: ScanStats::default() },
            );
        }
        scan_ctl().busy.store(false, Ordering::SeqCst);
    });
    Ok(())
}

/// 本地时间戳 `YYYYMMDD-HHMM`：导出文件默认名里的那一刻。
///
/// 为什么不直接用 `db::now_ts()`（unix 秒）：文件名是给人看的，
/// `我的库-userdata-20261120-2143.D-AssetManager` 一眼就知道是哪个库、哪天的备份。
/// 取**本地时间**而不是 UTC：用户在上海，文件名写着 8 小时前的那一刻只会让人困惑。
/// 拿不到系统时间（理论上不会）就退回 unix 秒，别让导出失败。
#[cfg(windows)]
fn local_parts() -> Option<(u16, u16, u16, u16, u16)> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        millis: u16,
    }
    extern "system" {
        fn GetLocalTime(out: *mut SystemTime);
    }
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    if t.year == 0 {
        return None;
    }
    Some((t.year, t.month, t.day, t.hour, t.minute))
}

#[cfg(not(windows))]
fn local_parts() -> Option<(u16, u16, u16, u16, u16)> {
    None
}

/// `YYYYMMDD` —— 整库 zip 的名字只要"哪一天"，不要时分（一天导几次不该有歧义，
/// 重名时由系统文件对话框自己加 `(2)`）。
fn local_ymd() -> String {
    match local_parts() {
        Some((y, m, d, _, _)) => format!("{y:04}{m:02}{d:02}"),
        None => db::now_ts().to_string(),
    }
}

fn local_stamp() -> String {
    match local_parts() {
        Some((y, m, d, hh, mm)) => format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}"),
        None => db::now_ts().to_string(),
    }
}

/// 导出用户层数据（T13）：**按库导**，文件名带库名，落在数据目录，返回完整路径。
///
/// `path` = 库根（或库内任意路径）；`None` = 当前库。0.4.5 起不再"一锅全导"：
/// 用户点的是"这个库的数据"，导出全库只会让人以为数据又混在一起了。
///
/// 后缀 0.4.2 从 `.json` → `.L-AssetManager`，0.4.5 再改成 `.D-AssetManager`（D = Data）。
/// **内容格式一个字都没改** —— 老的 `.L` 文件照样能导入（类型判定只看内容，不看扩展名）。
#[tauri::command]
fn am_export_userdata(path: Option<String>, state: State<AppState>) -> Result<String, String> {
    let app = lock(&state);
    let lib_id = match path.as_deref() {
        Some(p) => app
            .lib_for_path(p)
            .ok_or_else(|| format!("这个路径不在任何素材库里：{p}"))?,
        None => app
            .current_lib_id()
            .ok_or_else(|| "还没有选中素材库".to_string())?,
    };
    let json = app.export_userdata(Some(lib_id))?;
    let label = app.lib_label(lib_id).unwrap_or_else(|| "素材库".into());
    let name = format!(
        "{}-userdata-{}.D-AssetManager",
        am_core::snapshot::safe_file_stem(&label),
        local_stamp()
    );
    let target = app.data_dir.join(name);
    std::fs::write(&target, json).map_err(|e| format!("写文件失败：{}", e))?;
    Ok(target.to_string_lossy().to_string())
}

/// 原生文件夹选择器（T15）。`rfd` 走系统 IFileDialog，比让用户手打路径靠谱得多。
///
/// 返回 `None` = 用户点了取消（不是错误）。
#[tauri::command]
fn am_pick_folder(title: Option<String>) -> Option<String> {
    let mut d = rfd::FileDialog::new();
    if let Some(t) = title {
        d = d.set_title(&t);
    }
    d.pick_folder().map(|p| p.to_string_lossy().to_string())
}

/// 侧栏「回收站」：打开 Windows 回收站（不做自己的回收站面板 —— 系统那份已经够用，
/// 而且删除本来就是交给系统的，自己再维护一套只会两边不同步）
#[tauri::command]
fn am_open_recycle() -> Result<(), String> {
    fsops::open_recycle_bin()
}

#[tauri::command]
fn am_reveal(path: String) -> Result<(), String> {
    fsops::reveal_in_explorer(std::path::Path::new(&path))
}

/// 用系统默认程序打开**原文件**（缩略图只是显示层，这里绝不碰缓存里的 webp）
#[tauri::command]
fn am_open_path(path: String) -> Result<(), String> {
    fsops::open_with_default(Path::new(&path))
}

/// 把**原文件**放进剪贴板（CF_HDROP）—— 粘贴出去的是真文件，不是缩略图
#[tauri::command]
fn am_copy_files(paths: Vec<String>) -> Result<(), String> {
    let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    fsops::copy_files_to_clipboard(&list)
}

/* ---------------- 调整尺寸（P2.5 T38） ---------------- */
/* 「另存为」直接复用上面的 `am_save_as`（原样复制字节，任何格式都成立）。 */

/// 缩放另存：`max_edge` = 长边上限（0 = 只转格式不缩放）。
///
/// 目标扩展名由用户在对话框里选（.png / .jpg / .webp 过滤器），
/// 没写扩展名就按 `format` 补上。
#[tauri::command]
async fn am_resize_save(
    path: String,
    max_edge: u32,
    format: Option<String>,
) -> Result<Option<ResizeOut>, String> {
    let src = PathBuf::from(&path);
    if !am_core::imgops::resizable(&src) {
        return Err("这个格式没法重新编码（动图/RAW/PSD/PDF 只能「另存为」原样复制）".into());
    }
    let fmt = format.unwrap_or_else(|| "png".into()).to_lowercase();
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    let suggest = if max_edge > 0 {
        format!("{stem}_{max_edge}.{fmt}")
    } else {
        format!("{stem}.{fmt}")
    };
    let mut dlg = rfd::AsyncFileDialog::new()
        .set_file_name(&suggest)
        .add_filter("PNG", &["png"])
        .add_filter("JPEG", &["jpg", "jpeg"])
        .add_filter("WebP", &["webp"]);
    if let Some(d) = src.parent() {
        dlg = dlg.set_directory(d);
    }
    let Some(picked) = dlg.save_file().await else {
        return Ok(None);
    };
    let mut dst = picked.path().to_path_buf();
    if dst.extension().is_none() {
        dst.set_extension(&fmt);
    }
    let (w, h, bytes) = am_core::imgops::resized_to(&src, &dst, max_edge)?;
    Ok(Some(ResizeOut {
        path: dst.to_string_lossy().to_string(),
        width: w,
        height: h,
        bytes,
    }))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ResizeOut {
    path: String,
    width: u32,
    height: u32,
    bytes: u64,
}

/// 读 EXIF 拍摄信息（T41）。读不到就是 `None`，界面那一段直接不显示。
#[tauri::command]
async fn am_exif(path: String) -> Result<Option<am_core::exif::Exif>, String> {
    let p = PathBuf::from(path);
    Ok(tauri::async_runtime::spawn_blocking(move || am_core::exif::read(&p))
        .await
        .unwrap_or(None))
}

#[tauri::command]
fn am_rename(path: String, name: String, state: State<AppState>) -> Result<String, String> {
    let app = lock(&state);
    let ops = fsops::FsOps::new(app.conn_for_path(&path)?);
    let out = ops.rename(std::path::Path::new(&path), &name)?;
    Ok(out.to_string_lossy().to_string())
}

#[tauri::command]
fn am_mkdir(parent: String, name: String, state: State<AppState>) -> Result<String, String> {
    let mut app = lock(&state);
    let out = {
        let ops = fsops::FsOps::new(app.conn_for_path(&parent)?);
        ops.mkdir(std::path::Path::new(&parent), &name)?
    };
    let made = out.to_string_lossy().to_string();
    /* **空目录自己也要有目录行**（owner 2026-11-19）：`scan_one_dir` 只索引文件，
       不写子目录行 —— 只刷父目录的话，新建的文件夹在树里依然看不见。 */
    if let Err(e) = app.index_new_dir(&made, &parent) {
        if let Some(d) = LOG_DIR.get() {
            log_runtime(d, &format!("mkdir index FAIL {made}：{e}"));
        }
    }
    /* 父目录再扫一遍：目录 mtime 变了，增量扫描的判据要对齐（顺手索引里面的文件，通常是空的）。 */
    let mut dirty: HashSet<PathBuf> = HashSet::new();
    dirty.insert(PathBuf::from(&parent));
    refresh_dirs(&mut app, &dirty);
    Ok(made)
}

/// 落盘前的体检（T05）：给前端弹「>100 个 / >2GB」与「同名三选」用。
#[tauri::command]
fn am_move_plan(paths: Vec<String>, target: String) -> Result<am_core::fsops::MovePlan, String> {
    let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    fsops::plan_move(&list, Path::new(&target))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MoveOutcome {
    moved: i64,
    skipped: Vec<String>,
    renamed: Vec<String>,
    replaced: Vec<String>,
    /// 跟着文件走过去的用户数据（评分/备注/来源/标签/分组/快照条目）。
    /// 0.4.5 起跨库移动不再是"搬完变白纸"——搬了多少条，这里如实报。
    carried: am_core::CarryStat,
}

/// 真移动（T02）。`policy` 由「同名三选」决定；索引在同一个命令里刷新，
/// 所以网格不用等下次扫描就已经是对的。
#[tauri::command]
fn am_move(
    paths: Vec<String>,
    target: String,
    policy: Option<MovePolicy>,
    state: State<AppState>,
) -> Result<MoveOutcome, String> {
    let mut app = lock(&state);
    let first = paths
        .first()
        .ok_or_else(|| "没有要移动的文件".to_string())?;
    /* 抓数据必须在 mv 之前：重扫会 DELETE 源行，连带级联清掉标签关联 */
    let carries = app.capture_carry(&paths)?;
    /* 哪些是整目录移动，也得在 mv 之前记 —— 移动之后源路径就没了 */
    let dirs = dir_keys(&paths);
    let ops = fsops::FsOps::new(app.conn_for_path(first)?);
    let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let ext = ops.mv_with(&list, Path::new(&target), policy.unwrap_or_default())?;

    // 索引跟着动：老目录与新目录各重扫一层（只读元数据，几十毫秒）
    let mut dirty: HashSet<PathBuf> = HashSet::new();
    for (from, to) in &ext.pairs {
        if let Some(p) = Path::new(from).parent() {
            dirty.insert(p.to_path_buf());
        }
        if let Some(p) = Path::new(to).parent() {
            dirty.insert(p.to_path_buf());
        }
    }
    refresh_dirs(&mut app, &dirty);
    /* 整目录移动：源子树清空、目标子树递归索引（否则子目录里的图两边都不对） */
    resync_moved_dirs(&mut app, &dir_moves_of(&ext.pairs, &dirs));

    /* 重扫完再写回：目标库的新行这时才存在。
       搬运失败**不当作移动失败** —— 文件已经动了，报错只会让用户以为要重来。 */
    let pairs: Vec<(String, String)> = ext.pairs.clone();
    let carried = app.apply_carry(&carries, &pairs).unwrap_or_default();

    Ok(MoveOutcome {
        moved: ext.report.moved,
        skipped: ext.report.skipped,
        renamed: ext.report.renamed.iter().map(|(o, n)| format!("{} → {}", o, n)).collect(),
        replaced: ext.report.replaced,
        carried,
    })
}

/// 有移动在跑吗（后台线程正在复制的目录不允许并发再来一次，
/// 两条线程对着同一个目标目录会互相覆盖同名判定）。
fn move_busy() -> &'static AtomicBool {
    static B: AtomicBool = AtomicBool::new(false);
    &B
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MoveDone {
    ok: bool,
    message: String,
    outcome: Option<MoveOutcome>,
}

/// 后台移动（缺陷 #7 收口）：跨盘走「复制 → 删源」，几百 MB 的单文件能跑十几秒。
/// 以前同步执行时整条命令一直不返回，进度条一格不动，用户只能猜是不是卡死了。
/// 现在照 `am_scan_start` 那套：**独立线程 + 独立连接**，
/// 逐项进度发 `move://progress`，结束发 `move://done`；主连接不占锁，列表与缩略图照常能用。
#[tauri::command]
fn am_move_start(
    app: AppHandle,
    paths: Vec<String>,
    target: String,
    policy: Option<MovePolicy>,
    state: State<AppState>,
) -> Result<(), String> {
    if move_busy().swap(true, Ordering::SeqCst) {
        return Err("上一次移动还没结束，等它跑完".into());
    }
    let data_dir = lock(&state).data_dir.clone();
    std::thread::spawn(move || {
        let mut err: Option<String> = None;
        let mut outcome: Option<MoveOutcome> = None;

        match open_app(&data_dir) {
            Ok(mut a) => {
                let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                    /* 抓数据必须在 mv 之前（重扫会删源行）；抓不到就继续搬，只是没有可带的东西。
                       放在 `res` 块外：块里只做搬运，搬完还要用这批快照写回目标库。 */
                    let carries = a.capture_carry(&paths).unwrap_or_default();
                    let dirs = dir_keys(&paths);
                    let res = {
                    let Some(first) = list.first().cloned() else {
                        let _ = app.emit("move://done", MoveDone {
                            ok: false,
                            message: "没有要移动的文件".into(),
                            outcome: None,
                        });
                        move_busy().store(false, Ordering::SeqCst);
                        return;
                    };
                    let Ok(conn) = a.conn_for_path(&first.to_string_lossy()) else {
                        let _ = app.emit("move://done", MoveDone {
                            ok: false,
                            message: "这些文件不在任何素材库里".into(),
                            outcome: None,
                        });
                        move_busy().store(false, Ordering::SeqCst);
                        return;
                    };
                    let ops = fsops::FsOps::new(conn);
                    ops.mv_with_progress(
                        &list,
                        Path::new(&target),
                        policy.unwrap_or_default(),
                        |p| {
                            let _ = app.emit("move://progress", p);
                        },
                    )
                };
                match res {
                    Ok(ext) => {
                        // 索引跟着动：老目录与新目录各重扫一层（只读元数据，几十毫秒）
                        let mut dirty: HashSet<PathBuf> = HashSet::new();
                        for (from, to) in &ext.pairs {
                            if let Some(p) = Path::new(from).parent() {
                                dirty.insert(p.to_path_buf());
                            }
                            if let Some(p) = Path::new(to).parent() {
                                dirty.insert(p.to_path_buf());
                            }
                        }
                        refresh_dirs(&mut a, &dirty);
                        resync_moved_dirs(&mut a, &dir_moves_of(&ext.pairs, &dirs));
                        /* 重扫完再写回：目标库的新行这时才存在 */
                        let pairs: Vec<(String, String)> = ext.pairs.clone();
                        let carried = a.apply_carry(&carries, &pairs).unwrap_or_default();
                        outcome = Some(MoveOutcome {
                            moved: ext.report.moved,
                            skipped: ext.report.skipped,
                            renamed: ext.report.renamed.iter().map(|(o, n)| format!("{} → {}", o, n)).collect(),
                            replaced: ext.report.replaced,
                            carried,
                        });
                    }
                    Err(e) => err = Some(e),
                }
            }
            Err(e) => err = Some(e),
        }

        let message = match (&err, &outcome) {
            (Some(e), _) => e.clone(),
            (None, Some(o)) => {
                let mut bits: Vec<String> = Vec::new();
                if o.moved > 0 {
                    bits.push(format!("移动 {} 项", o.moved));
                }
                if !o.renamed.is_empty() {
                    bits.push(format!("改名 {} 项", o.renamed.len()));
                }
                if !o.replaced.is_empty() {
                    bits.push(format!("替换 {} 项（旧的在回收站）", o.replaced.len()));
                }
                if !o.skipped.is_empty() {
                    bits.push(format!("跳过 {} 项", o.skipped.len()));
                }
                /* 跨库/跨目录移动带走了什么，明说一句：
                   "标签 3 · 分组 1 · 快照 2" 比一句"移动成功"让人放心得多 */
                let c = &o.carried;
                if c.carried > 0 && (c.tags + c.colls + c.snaps) > 0 {
                    bits.push(format!(
                        "带走批注：标签 {} · 分组 {} · 快照 {}",
                        c.tags, c.colls, c.snaps
                    ));
                }
                if bits.is_empty() {
                    "没有移动任何文件".into()
                } else {
                    bits.join(" · ")
                }
            }
            _ => "移动结束（没有拿到结果）".into(),
        };
        let ok = err.is_none() && outcome.as_ref().map(|o| o.moved > 0).unwrap_or(false);
        let _ = app.emit("move://done", MoveDone { ok, message, outcome });
        move_busy().store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[tauri::command]
fn am_delete(paths: Vec<String>, state: State<AppState>) -> Result<usize, String> {
    let mut app = lock(&state);
    let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    /* 删除 = **搬进应用内回收站**（owner 2026-11-17，照 FontManager）：
       文件挪到 <数据目录>/trash/ + `.amtrash` 账本，回收站页面能还原 / 彻底删除 / 清空。 */
    let entries = am_core::trash::put(&app.data_dir, &list)?;
    let dirty: HashSet<PathBuf> = list
        .iter()
        .filter_map(|p| p.parent().map(|x| x.to_path_buf()))
        .collect();
    refresh_dirs(&mut app, &dirty);
    /* 删掉的图可能把标签/分组/快照腾空 → 清掉空壳（宽限保护刚建的那些） */
    let _ = app.prune_empty_shelves(am_core::EMPTY_SHELF_GRACE);
    Ok(entries.len())
}

/* ---------------- 应用内回收站（0.1.6） ---------------- */

#[tauri::command]
fn am_trash_list(state: State<AppState>) -> Vec<am_core::trash::TrashEntry> {
    let app = lock(&state);
    am_core::trash::list(&app.data_dir)
}

/// 回收站里那些文件的预览地址（**只认账本里的 id**，不接受任意路径）。
///
/// 回收站行左侧要缩略图、右侧详情要整张图，而 `thumb://` 只认素材 id ——
/// 进回收站的文件已经不是素材了。这里复用 `peekfile://` 的令牌表，按 id 现开一张，
/// 返回顺序与入参一致（`None` = 这条已经没了）。
#[tauri::command]
fn am_trash_media(ids: Vec<String>, state: State<AppState>) -> Vec<Option<String>> {
    let app = lock(&state);
    let list = am_core::trash::list(&app.data_dir);
    ids.iter()
        .map(|id| {
            list.iter()
                .find(|t| &t.id == id)
                .map(|t| PathBuf::from(&t.trashed_path))
                .filter(|p| p.is_file())
                .map(|p| media_url(&p))
        })
        .collect()
}

#[tauri::command]
fn am_trash_restore(
    ids: Vec<String>,
    /* 还原到哪儿：`None` = 回原位置（截图贴图就是回 screenshots）。
       传了目录就落到那里 —— 贴图默认进回收站，用户想留底时说一句"存到我的素材夹"就行。 */
    to: Option<String>,
    state: State<AppState>,
) -> Result<usize, String> {
    let mut app = lock(&state);
    let dest = to.map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
    let back = am_core::trash::restore_to(&app.data_dir, &ids, dest.as_deref())?;
    // 还原 = 磁盘上又出现了文件 → 把落点所在目录重新索引一遍
    let dirty: HashSet<PathBuf> = back
        .iter()
        .map(PathBuf::from)
        .filter_map(|p| p.parent().map(|x| x.to_path_buf()))
        .collect();
    refresh_dirs(&mut app, &dirty);
    Ok(back.len())
}

#[tauri::command]
fn am_trash_purge(ids: Vec<String>, state: State<AppState>) -> Result<usize, String> {
    let app = lock(&state);
    am_core::trash::purge(&app.data_dir, &ids)
}

#[tauri::command]
fn am_trash_empty(state: State<AppState>) -> Result<usize, String> {
    let app = lock(&state);
    am_core::trash::empty(&app.data_dir)
}

#[tauri::command]
fn am_undo(state: State<AppState>) -> Result<String, String> {
    let mut app = lock(&state);
    let msg = app.undo()?;
    // 撤销改了磁盘，索引也得跟上；全库一层目录重扫代价高，这里只刷新库根
    let roots: Vec<PathBuf> = app.lib_roots().unwrap_or_default().into_iter().map(PathBuf::from).collect();
    let mut dirty: HashSet<PathBuf> = HashSet::new();
    for r in roots {
        dirty.insert(r);
    }
    refresh_dirs(&mut app, &dirty);
    Ok(msg)
}

/// 把一批目录的一层内容重新对账进索引（文件操作之后调）。
/// 路径比较用的键（小写 + 统一反斜杠）。只在"这条 pair 原来是目录吗"这一处用。
fn path_key(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// 目录移动的索引收尾：源子树**整棵**清掉，目标子树**递归**建索引。
///
/// 只刷上一层（`refresh_dirs`）不够：拖走一个文件夹之后
/// · 源库里子目录的行没人删 → 变成指向空路径的幽灵（列表里还在、点了打不开）；
/// · 目标库里子目录中的图没人建 → `apply_carry` 按路径找不到行，批注只能记成跳过。
fn resync_moved_dirs(app: &mut App, moves: &[(String, String)]) {
    for (from, to) in moves {
        if let Some(lib_id) = app.lib_for_path(from) {
            if let Some(l) = app.libs.iter().find(|l| l.id == lib_id) {
                let _ = db::purge_subtree(&l.conn, from);
            }
        }
        let Some(lib_id) = app.lib_for_path(to) else { continue };
        let Some(l) = app.libs.iter_mut().find(|l| l.id == lib_id) else { continue };
        let cancel = AtomicBool::new(false);
        let opts = scan::ScanOptions::default();
        let _ = scan::scan_root(&mut l.conn, to, &opts, &cancel, |_p| {});
    }
}

/// 移动前记下哪些路径是**目录**：移动之后源路径就不存在了，再 `is_dir` 全是 false。
fn dir_keys(paths: &[String]) -> HashSet<String> {
    paths
        .iter()
        .filter(|p| Path::new(p).is_dir())
        .map(|p| path_key(p))
        .collect()
}

/// 从真实移动结果里挑出"整目录移动"的那几条。
fn dir_moves_of(pairs: &[(String, String)], dirs: &HashSet<String>) -> Vec<(String, String)> {
    pairs
        .iter()
        .filter(|(f, _)| dirs.contains(&path_key(f)))
        .cloned()
        .collect()
}

fn refresh_dirs(app: &mut App, dirs: &HashSet<PathBuf>) {
    for d in dirs {
        if !d.is_dir() {
            continue;
        }
        let p = d.to_string_lossy().to_string();
        // 目录在哪个库就只刷哪个库：刷错库不会报错，只会"这边少图、那边多图"
        let Some(lib_id) = app.lib_for_path(&p) else { continue };
        if let Some(l) = app.libs.iter_mut().find(|l| l.id == lib_id) {
            let _ = scan::scan_one_dir(&mut l.conn, &p);
        }
    }
}

/// 移动前的体量体检（T05 阈值确认用；单独一个命令，避免把 plan 塞进 list 的返回里）
#[tauri::command]
fn am_stat_paths(paths: Vec<String>) -> am_core::fsops::ItemStat {
    let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    fsops::stat_paths(&list)
}

/* ---------------- T24 主色板 / T25 查重 / T26 引用 / T33 批量重命名 ---------------- */

/// 按一批全局 id 取素材（查重/引用面板要显示缩略图）
#[tauri::command]
fn am_assets_by_ids(ids: Vec<i64>, state: State<AppState>) -> Result<Vec<Asset>, String> {
    lock(&state).assets_by_ids(&ids)
}

/// 还缺多少元数据：`(缺主色板, 缺全文件哈希)`
#[tauri::command]
fn am_meta_missing(state: State<AppState>) -> Result<(i64, i64), String> {
    lock(&state).meta_missing()
}

/// 补算主色板（读缓存里的缩略图，不重解原图）。返回 `(本次补了, 还剩)`。
#[tauri::command]
fn am_fill_palette(limit: Option<i64>, state: State<AppState>) -> Result<(i64, i64), String> {
    let mut app = lock(&state);
    app.fill_palette(limit.unwrap_or(800))
}

/// 各色桶的素材数（侧栏颜色筛选的口径）
#[tauri::command]
fn am_color_counts(state: State<AppState>) -> Result<Vec<(String, i64)>, String> {
    lock(&state).color_counts()
}

/// 精确重复：先按批算哈希，再返回重复组 + 还剩多少没算
#[tauri::command]
fn am_dupes_exact(
    limit: Option<i64>,
    state: State<AppState>,
) -> Result<(Vec<am_core::model::DupGroup>, i64), String> {
    let mut app = lock(&state);
    app.dupes_exact(limit.unwrap_or(2000))
}

#[tauri::command]
fn am_dupes_similar(
    max_dist: Option<u32>,
    limit: Option<usize>,
    state: State<AppState>,
) -> Result<Vec<am_core::model::SimilarPair>, String> {
    lock(&state).dupes_similar(max_dist.unwrap_or(6), limit.unwrap_or(200))
}

/// 扫一批工程文件的外部引用
#[tauri::command]
fn am_refs_scan(limit: Option<i64>, state: State<AppState>) -> Result<(i64, i64), String> {
    let mut app = lock(&state);
    app.refs_scan(limit.unwrap_or(120))
}

#[tauri::command]
fn am_refs_summary(state: State<AppState>) -> Result<(i64, i64), String> {
    lock(&state).refs_summary()
}

#[tauri::command]
fn am_broken_refs(limit: Option<i64>, state: State<AppState>) -> Result<Vec<am_core::model::RefItem>, String> {
    lock(&state).broken_refs(limit.unwrap_or(300))
}

#[tauri::command]
fn am_refs_of(id: i64, state: State<AppState>) -> Result<Vec<am_core::model::RefItem>, String> {
    lock(&state).refs_of(id, 200)
}

#[tauri::command]
fn am_refs_using(id: i64, state: State<AppState>) -> Result<Vec<am_core::model::RefItem>, String> {
    lock(&state).refs_using(id, 200)
}

/// 批量重命名：先出预览（不碰磁盘），用户确认后再 apply
#[tauri::command]
fn am_rename_plan(
    ids: Vec<i64>,
    rule: am_core::model::RenameRule,
    state: State<AppState>,
) -> Result<Vec<am_core::model::RenameItem>, String> {
    lock(&state).rename_plan(&ids, &rule)
}

#[tauri::command]
fn am_rename_apply(
    items: Vec<(i64, String)>,
    state: State<AppState>,
) -> Result<am_core::model::RenameResult, String> {
    let mut app = lock(&state);
    let out = app.rename_apply(&items)?;
    // 索引跟上：只重扫受影响的目录（改名不改目录内容，扫一层就够）
    let dirty: HashSet<PathBuf> = out.dirs.iter().map(PathBuf::from).collect();
    refresh_dirs(&mut app, &dirty);
    Ok(out)
}

/// T34：给标签设自动规则（传 null 就清掉规则）
#[tauri::command]
fn am_tag_set_rule(
    tag: String,
    rule: Option<am_core::reg::AutoRule>,
    state: State<AppState>,
) -> Result<(), String> {
    let mut app = lock(&state);
    app.set_tag_rule(&tag, rule.as_ref())
}

#[tauri::command]
fn am_tag_get_rule(tag: String, state: State<AppState>) -> Result<Option<am_core::reg::AutoRule>, String> {
    lock(&state).tag_rule(&tag)
}

/// 每个库的**独立 DB 文件**在哪、多大（T28：用户要能看见"一个库一个文件"）
#[tauri::command]
fn am_lib_files(state: State<AppState>) -> Result<Vec<(String, String, i64)>, String> {
    let app = lock(&state);
    let rows = am_core::reg::libs(&app.reg)?;
    let mut out = Vec::new();
    for r in rows {
        let rel = if r.file.is_empty() {
            format!("libs/lib-{}.db", r.id)
        } else {
            r.file.clone()
        };
        let p = app.data_dir.join(&rel);
        let bytes = std::fs::metadata(&p).map(|m| m.len() as i64).unwrap_or(0);
        out.push((r.root, p.to_string_lossy().to_string(), bytes));
    }
    Ok(out)
}

/* ---------------- 缓存（设置面板要用真实的数字，不能写死） ---------------- */
#[tauri::command]
fn am_cache_stat(state: State<AppState>) -> Result<(i64, i64), String> {
    let app = lock(&state);
    let mut bytes = 0i64;
    let mut files = 0i64;
    let root = app.thumbs.root.clone();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let p = ent.path();
            match ent.metadata() {
                Ok(m) if m.is_dir() => stack.push(p),
                Ok(m) => {
                    bytes += m.len() as i64;
                    files += 1;
                }
                Err(_) => {}
            }
        }
    }
    Ok((bytes, files))
}

/// `limit` = 当前上限；`target` = 要降到多少。清理缓存传 (0, 0) 就是全清。
#[tauri::command]
fn am_cache_gc(limit: i64, target: i64, state: State<AppState>) -> Result<(i64, i64), String> {
    let app = lock(&state);
    let mut app = app;
    app.gc_thumbs(limit, target)
}

/// 设置面板「关于」要显示数据目录与库根，避免用户去猜。
#[tauri::command]
fn am_data_dir(state: State<AppState>) -> Result<String, String> {
    let app = lock(&state);
    Ok(app.data_dir.to_string_lossy().to_string())
}

/* ------------------------------------------------------ 数据文件夹（可配置） */

/// 设置面板「素材库数据文件夹」要显示的全部信息。
///
/// `effective` 是**真正在用的**目录（自定义不可用时已经回退成默认），
/// 界面必须显示它而不是 `current` —— 用户看到的应该是事实，不是愿望。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DataRootInfo {
    default_dir: String,
    effective_dir: String,
    enabled: bool,
    current: Option<String>,
    dirs: Vec<String>,
    max: usize,
}

/// 看一眼目录里有什么：用来在用户选完「数据文件夹」后判断他是不是选错了地方
/// （选成了自己的素材目录）。只往下看两层、数到上限就停，不会卡住大库。
#[tauri::command]
fn am_probe_dir(dir: String) -> am_core::dataroot::DirProbe {
    am_core::dataroot::probe(Path::new(&dir))
}

#[tauri::command]
fn am_data_root(app: AppHandle, state: State<AppState>) -> Result<DataRootInfo, String> {
    let default_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("拿不到默认数据目录：{e}"))?;
    let cfg = am_core::dataroot::load(&default_dir);
    let effective = lock(&state).data_dir.to_string_lossy().to_string();
    Ok(DataRootInfo {
        default_dir: default_dir.to_string_lossy().to_string(),
        effective_dir: effective,
        enabled: cfg.enabled,
        current: cfg.current,
        dirs: cfg.dirs,
        max: am_core::dataroot::MAX_DIRS,
    })
}

/// 写入数据文件夹配置。**不立即切换**（连接已经绑在旧文件上），下次启动生效。
///
/// `copy_index` 为真时先把索引整份复制到新目录（`VACUUM INTO`），
/// 这样重启后打开就是同一套库，不用重扫 3 万个文件。缩略图缓存不复制。
#[tauri::command]
fn am_set_data_root(
    app: AppHandle,
    state: State<AppState>,
    enabled: bool,
    current: Option<String>,
    dirs: Vec<String>,
    copy_index: bool,
    // 用户已经看过「这看起来是素材目录」的提示、仍要用它当索引位置
    force: Option<bool>,
) -> Result<String, String> {
    let default_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("拿不到默认数据目录：{e}"))?;

    let mut cfg = am_core::dataroot::DataRootConfig {
        enabled,
        current: current.clone(),
        dirs,
    };
    cfg.dirs.truncate(am_core::dataroot::MAX_DIRS);

    let mut copied: u64 = 0;
    if enabled {
        let Some(cur) = current.as_deref().filter(|s| !s.trim().is_empty()) else {
            return Err("启用自定义文件夹时必须指定一个目录".into());
        };
        let cur_path = PathBuf::from(cur);
        if !am_core::dataroot::is_usable(&cur_path) {
            return Err(format!("这个目录建不出来或不可写：{}", cur_path.display()));
        }
        /* 目录里已经有素材 → 用户多半是「想把素材库设成它」，不是「想把索引放进来」。
           2026-11-16 就出过这事：用户把素材库根设成数据文件夹，界面上只多了一条 toast，
           他以为设好了，回主界面看到 0/3 空状态 —— 「设置了文件库但没有任何效果」。
           前端会先探一次并弹三选一（加入素材库 / 仍用作数据位置 / 取消），这里是最后一道。 */
        if !force.unwrap_or(false) {
            let p = am_core::dataroot::probe(&cur_path);
            if p.images > 0 || p.videos > 0 {
                return Err(format!(
                    "这个文件夹里有 {} 张图片、{} 个视频，看起来是素材目录。数据文件夹只放索引和缩略图缓存，不会让素材出现在软件里 —— 要加素材库，请用「素材库」页的「添加文件夹…」。",
                    p.images, p.videos
                ));
            }
        }
        /* 不许把数据文件夹设进素材库（2026-11-16 真出过一次：库根里冒出 index.db + cache/）。
           数据目录里放的是 index.db 和 cache/thumbs，混进素材目录后用户既看不懂也删不得。 */
        let roots = {
            let a = lock(&state);
            a.lib_roots().unwrap_or_default()
        };
        if let Some(hit) = am_core::dataroot::conflicts_with_libs(&cur_path, &roots) {
            return Err(format!(
                "不能把数据文件夹设进素材库「{}」里 —— 索引和缩略图缓存会混进你的素材目录。请换一个不在素材库内的位置（比如 D:\\AssetManagerData）。",
                hit
            ));
        }
        // 目标就是当前在用的目录 → 没什么可复制的（也别去 VACUUM 到自己头上）
        let same = {
            let a = lock(&state);
            a.data_dir == cur_path
        };
        if copy_index && !same && !db::db_path_for(&cur_path).exists() {
            let a = lock(&state);
            copied = a.copy_index_to(&cur_path)?;
        }
    }

    am_core::dataroot::save(&default_dir, &cfg)?;
    Ok(if copied > 0 {
        format!(
            "已保存，并复制了 {:.1} MB 的索引。重启软件后生效（缩略图会按需重建）",
            copied as f64 / 1048576.0
        )
    } else {
        "已保存，重启软件后生效".to_string()
    })
}

/// 重启自己：换数据文件夹、或者用户点「立即重启」都走这里。
#[tauri::command]
fn am_relaunch(app: AppHandle) {
    app.restart();
}

#[tauri::command]
fn am_close(window: Window) {
    let _ = window.close();
}

#[tauri::command]
fn am_minimize(window: Window) {
    let _ = window.minimize();
}

#[tauri::command]
fn am_toggle_max(window: Window) {
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    } else {
        let _ = window.maximize();
    }
}

fn parse_query(q: Option<&str>) -> u32 {
    let Some(q) = q else { return 256 };
    for part in q.split('&') {
        if let Some(v) = part.strip_prefix("px=") {
            if let Ok(n) = v.parse::<u32>() {
                /* `0` 是**哨兵值**：要原图（长图在灯箱里铺满一屏时用）。
                   这里必须先放行 —— 夹到 64 就会出 64px 宽的缩略图，铺到 1000px 就是一团糊
                   （0.3.12 上线当天踩到：灯箱比上一版还糊）。 */
                if n == 0 {
                    return 0;
                }
                // 上限 2048：灯箱要 1600 档（1360 CSS × DPR1.25 ≈ 1700 设备像素），
                // 原来卡在 1024 会把大图看糊 —— 而且钳位是**静默**的，排查时只看得到「档位没生成」
                return n.clamp(64, 2048);
            }
        }
    }
    256
}

fn not_found() -> Response<Vec<u8>> {
    // 失败绝不能被缓存：早期没加 no-store，一次 404 让那张图永久空白
    Response::builder()
        .status(404)
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Vec::new())
        .unwrap()
}

/* ---------------- 无边框看图窗口（0.9.0 → 0.10.0） ----------------
   果核看图最值得抄的一点不是渲染，而是**窗口形态**：窗口变成"就是那张图"——
   没有标题栏、没有边框、没有工具条，长边 342 DIP，尺寸按图片比例算。

   0.10.0 起它是**贴图板**，不是"单张大图"：
   · 想贴几张就贴几张（Q 键位/右键/中键各开一扇，彼此独立）；
   · 默认置顶、Ctrl+滚轮调不透明度、双击撕掉；
   · **关掉软件 / 重启电脑后，上次没手动关的贴图会自己回来**（`peeks.json`）。

   三种来源（素材库 / 截图 / 剪贴板）在窗口看来完全一样：都是一条 `PeekItem`。 */

const PEEK_LONG_EDGE: f64 = 342.0;

/// 当前 UI 主题（0=未知 1=浅 2=深）。主窗在主题变化时推上来，
/// 建贴图窗口时注入 boot —— 贴图是独立 webview，读不到主窗那侧的偏好。
static UI_THEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// 贴图「蓝色边框」的全局开关，**默认开**（owner 2026-11-19：白底网页/文档上贴图
/// 需要一圈轮廓才看得出边界）。右键菜单里随时可关，关掉后连那 10px 发光 padding
/// 一起没有视觉痕迹。
///
/// 为什么必须放后端而不是 localStorage：每个贴图是一个独立 webview，
/// 前端各自记状态的话，关掉当前这张不影响已经贴着的其他张，新贴的还会按默认值再开 ——
/// 用户看到的就是"明明关过了，还有蓝边"（owner 2026-11-18 连报两次）。放这里 + 事件广播，一次点击对所有贴图生效。
///
/// 注意：这是**进程内**的全局值，重启软件回到"开"。owner 要的是出厂默认，不是记忆上次选择；
/// 真要记住用户选择，得进 `prefs` 落盘，别在前端另存一份。
static PEEK_FRAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
const PEEK_MIN_EDGE: f64 = 96.0;
const PEEK_MAX_EDGE: f64 = 4096.0;
/// 贴图窗口四周的透明边距（**CSS px**，逻辑单位；前端 `.peek-root` 的 `--peek-glow` 必须同值）。
///
/// 用途只有一个：「由边框向外的渐变蓝色窗口阴影」（owner 2026-11-18）。
/// 窗口外面没有像素可画，发光只能画在窗口里，所以窗口要比图片本身大一圈，
/// 图居中、光晕落在这一圈透明的 padding 里。
/// **必须和前端 `.peek-root { --peek-glow }` 的值一致**，不然图会偏、会缩。
/// 20 → 10（0.2.2）：owner 说蓝边"太宽、太明显"，图外面那圈透明边跟着收窄。
const PEEK_GLOW: f64 = 10.0;
const CLIP_HISTORY: usize = 10;

/// 一张"可以贴到屏幕上"的图
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeekItem {
    src: String,
    w: u32,
    h: u32,
    /// 磁盘上的真实文件（Ctrl+C 复制原图 / Ctrl+S 另存都靠它）
    path: String,
    name: String,
    /// 素材的全局 id（0 = 磁盘文件）。换图后要写回存档，靠它才能重启还原成"素材模式"
    gid: i64,
}

/// 一扇贴图的"存档"：重启后靠它原样恢复
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PeekRecall {
    label: String,
    /// `asset` = 素材库里的（重开走 thumb://，快）；`file` = 磁盘文件（截图/剪贴板）
    kind: String,
    path: String,
    /// 素材的全局 id（0 = 文件模式）
    gid: i64,
    x: i32,
    y: i32,
    /// 逻辑长边（DIP）
    long_edge: f64,
    /// 精确几何 `[x, y, w, h]`（**物理像素**）。
    ///
    /// 只有「截图 → 贴图」这条路会填：owner 要求**贴图和截图一样大、落在同一个位置**，
    /// 所以不能再按长边等比缩放、也不能居中到鼠标那里。填了它就以它为准
    /// （`spawn_peek` 里 `set_size(Physical)` + `set_position(Physical)`），
    /// 拖动 / 缩放窗口时同步更新，重启后也能原地原尺寸回来。
    place: Option<[f64; 4]>,
    top: bool,
    opacity: f64,
}

impl Default for PeekRecall {
    fn default() -> Self {
        Self {
            label: String::new(),
            kind: "file".into(),
            path: String::new(),
            gid: 0,
            x: 0,
            y: 0,
            long_edge: PEEK_LONG_EDGE,
            place: None,
            top: true,
            opacity: 1.0,
        }
    }
}

/* peekfile:// 的可信文件表。**只放我们自己刚写出来的文件**（剪贴板历史、截图临时文件），
   不接受任意路径：这个协议要是能读全盘，WebView 里一段脚本就能把用户文件发出去。 */
static MEDIA: OnceLock<Mutex<HashMap<u64, PathBuf>>> = OnceLock::new();
static MEDIA_SEQ: AtomicU64 = AtomicU64::new(1);

fn media_url(path: &Path) -> String {
    let t = MEDIA_SEQ.fetch_add(1, Ordering::Relaxed);
    let m = MEDIA.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 128 {
        g.clear();
    }
    g.insert(t, path.to_path_buf());
    format!("http://peekfile.localhost/{t}")
}

/// 程序所在目录（截图目录、剪贴板缓存都放它下面 —— owner 定的位置）
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

fn shot_dir() -> PathBuf {
    exe_dir().join("screenshots")
}

/// 路径比较口径（与 core 的 `norm_path` 一致）：反斜杠、去尾分隔符、小写。
fn same_path(a: &str, b: &str) -> bool {
    let f = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    f(a) == f(b)
}

/// **内置素材库**：软件自己的截图目录（`screenshots`，与 `assetmanager.exe` 同级）。
///
/// 它不是用户指定的素材库 —— owner 2026-11-17 定：**不占那 3 个名额、不能移除、
/// 侧栏永远留着**。截图（F1）、灵感板粘贴、剪贴板贴图都往这里落，
/// 把它当成"用户第 N 个库"来算名额，用户加第三个自己的库时就会被莫名其妙顶掉。
///
/// 判定为什么按「同名 + 同级有 exe」而不是只和当前 `shot_dir()` 比：
/// 同一个软件会有 debug / release / 已安装三种 exe，各自的截图目录都是它自己生成的。
/// 只认当前那条的话，换个构建跑一次，旧的那种就又变回"用户库"了（实测：release
/// 目录那行一直占着名额，用户加第三个库时被顶掉）。
fn is_builtin_root(path: &str) -> bool {
    if same_path(path, &shot_dir().to_string_lossy()) {
        return true;
    }
    let p = Path::new(path);
    let named = p
        .file_name()
        .map(|n| n.eq_ignore_ascii_case("screenshots"))
        .unwrap_or(false);
    if !named {
        return false;
    }
    p.parent()
        .map(|d| d.join("assetmanager.exe").exists())
        .unwrap_or(false)
}

/* 0.3.28 起没有 `clipboard-cache` / `peek-cache` 了：
   剪贴板图与截图贴图统一落 `screenshots` 再进软件回收站（见 `trash_png`）。
   老用户安装目录里那两个旧文件夹不会被删（里面可能有他要的图），只是不再被读。 */

fn mime_of(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "image/png",
    }
}

fn peek_size_for(w: u32, h: u32, long_edge: f64) -> (f64, f64) {
    let (w, h) = if w > 0 && h > 0 { (w as f64, h as f64) } else { (4.0, 3.0) };
    let long = long_edge.clamp(PEEK_MIN_EDGE, PEEK_MAX_EDGE);
    if w >= h {
        (long, (long * h / w).max(24.0))
    } else {
        ((long * w / h).max(24.0), long)
    }
}

fn peek_center_on_cursor(win: &tauri::WebviewWindow) {
    let Ok(cur) = win.cursor_position() else { return };
    let Ok(size) = win.outer_size() else { return };
    let monitors = win.available_monitors().unwrap_or_default();
    let m = monitors.iter().find(|m| {
        let p = m.position();
        let s = m.size();
        cur.x >= p.x as f64
            && cur.x < (p.x + s.width as i32) as f64
            && cur.y >= p.y as f64
            && cur.y < (p.y + s.height as i32) as f64
    });
    let (mut cx, mut cy) = match m {
        Some(m) => {
            let p = m.position();
            let s = m.size();
            (
                p.x as f64 + (s.width as f64 - size.width as f64) / 2.0,
                p.y as f64 + (s.height as f64 - size.height as f64) / 2.0,
            )
        }
        None => (cur.x - size.width as f64 / 2.0, cur.y - size.height as f64 / 2.0),
    };
    /* 级联错位：不这样的话连开三张会**严丝合缝叠在一起**，
       用户以为只开了一张（实测就是这样）。每张往右下挪 28px，绕 6 个循环回来。 */
    let n = peeks().lock().map(|g| g.len().saturating_sub(1)).unwrap_or(0) % 6;
    let step = 28.0 * n as f64;
    cx += step;
    cy += step;
    let _ = win.set_position(tauri::PhysicalPosition::new(cx.round(), cy.round()));
}

fn trim_around(ids: &[i64], index: usize, cap: usize) -> (Vec<i64>, usize) {
    let i = index.min(ids.len().saturating_sub(1));
    if ids.len() <= cap {
        return (ids.to_vec(), i);
    }
    let half = cap / 2;
    let start = i.saturating_sub(half).min(ids.len() - cap);
    (ids[start..start + cap].to_vec(), i - start)
}

fn item_of_file(p: &Path) -> Option<PeekItem> {
    let (w, h) = screen::dims(p);
    Some(PeekItem {
        src: media_url(p),
        w,
        h,
        path: p.to_string_lossy().to_string(),
        name: p.file_name()?.to_string_lossy().to_string(),
        gid: 0,
    })
}

fn items_of_assets(app: &App, ids: &[i64]) -> Vec<PeekItem> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Ok(Some(a)) = app.asset(*id) else { continue };
        let (w, h) = (a.width.unwrap_or(0).max(0) as u32, a.height.unwrap_or(0).max(0) as u32);
        out.push(PeekItem {
            src: format!("http://thumb.localhost/{}?px=1600&r={}", a.id, a.mtime),
            w,
            h,
            path: a.path.clone(),
            name: a.name.clone(),
            gid: *id,
        });
    }
    out
}

/* ---------------- 贴图存档（重启恢复） ---------------- */

static PEEKS: OnceLock<Mutex<Vec<PeekRecall>>> = OnceLock::new();
static PEEK_SEQ: AtomicU64 = AtomicU64::new(1);
static LAST_PERSIST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
/// 正在退出：这时候贴图窗口被销毁**不删存档**（下次开机要还原）
static QUITTING: AtomicBool = AtomicBool::new(false);

fn peeks() -> &'static Mutex<Vec<PeekRecall>> {
    PEEKS.get_or_init(|| Mutex::new(Vec::new()))
}

fn peeks_file() -> PathBuf {
    LOG_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| exe_dir())
        .join("peeks.json")
}

fn load_peeks() -> Vec<PeekRecall> {
    std::fs::read_to_string(peeks_file())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<PeekRecall>>(&s).ok())
        .unwrap_or_default()
}

/// 原子写（先写 .tmp 再改名）：写到一半掉电也不会留一个坏 JSON，
/// 坏掉的后果是"上次贴的图全没了"，用户没有任何办法找回来。
fn save_peeks() {
    let list = peeks().lock().unwrap_or_else(|e| e.into_inner()).clone();
    let path = peeks_file();
    let Ok(body) = serde_json::to_string_pretty(&list) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// 拖动/缩放时高频触发，节流到 350ms 一次；关闭窗口或退出时走 `save_peeks` 立即落盘
fn save_peeks_throttled() {
    let mut last = LAST_PERSIST.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    if let Some(t) = *last {
        if now.duration_since(t) < Duration::from_millis(350) {
            return;
        }
    }
    *last = Some(now);
    drop(last);
    save_peeks();
}

fn peek_touch<F: FnOnce(&mut PeekRecall)>(label: &str, f: F) {
    {
        let mut g = peeks().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = g.iter_mut().find(|r| r.label == label) {
            f(r);
        }
    }
    save_peeks_throttled();
}

fn next_label() -> String {
    let n = PEEK_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("peek-{n}")
}

/// 存档 → 可显示的条目。拿不到就返回 None（文件被删 / 换了盘），
/// 这条会被安静丢掉 —— 比留一扇空白窗口好。
fn recall_to_item(app: &AppHandle, rec: &PeekRecall) -> Option<PeekItem> {
    let p = Path::new(&rec.path);
    if !p.is_file() {
        return None;
    }
    if rec.kind == "asset" && rec.gid > 0 {
        let st = app.state::<AppState>();
        let a = lock(&st);
        if let Ok(Some(x)) = a.asset(rec.gid) {
            if x.path == rec.path {
                let (w, h) = (x.width.unwrap_or(0).max(0) as u32, x.height.unwrap_or(0).max(0) as u32);
                return Some(PeekItem {
                    src: format!("http://thumb.localhost/{}?px=1600&r={}", x.id, x.mtime),
                    w,
                    h,
                    path: x.path.clone(),
                    name: x.name.clone(),
                    gid: rec.gid,
                });
            }
        }
    }
    // 文件模式：WebView 能直接画的格式才贴（PSD/RAW 贴出来是空白）
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "avif") {
        return None;
    }
    item_of_file(p)
}

/// 贴图窗口要**直角 + 无系统描边**（owner 2026-11-19）。
///
/// 为什么不是 CSS 的事：Win11 会给每个顶层窗口自动加 8px 圆角和一圈系统描边，
/// 贴图窗 `decorations(false) + transparent(true) + shadow(false)` 也躲不掉 ——
/// 圆角把图的四个角切掉，描边在用户关掉「蓝色边框」之后仍然留着一条线，
/// 看着就像开关没生效。这两样只能在 DWM 层关。
/// 打开索引（所有入口共用一个）：**缓存放安装目录**（owner 2026-11-19：缓存别塞数据目录，
/// 这样装到哪个盘、删哪个目录都是自洽的）。写不进去时 core 自己退回 `<数据目录>/cache`。
///
/// 顺带把**内置库**（截图目录）的名单告诉 core：标签 / 分组 / 快照要把内置库合并进
/// 当前库的视图（owner 2026-11-20）。判定规则留在这里，core 不知道 exe 装在哪。
fn open_app(data: &Path) -> Result<App, String> {
    let mut app = App::open_with_cache(data, Some(exe_dir().join("cache")))?;
    app.set_builtin_roots(builtin_roots_of(&app)?);
    Ok(app)
}

/// 内置库根名单：当前截图目录 + 已登记里满足 `is_builtin_root` 的（换构建跑过会有好几个）。
fn builtin_roots_of(app: &App) -> Result<Vec<String>, String> {
    let mut out = vec![shot_dir().to_string_lossy().to_string()];
    for r in app.lib_roots()? {
        if is_builtin_root(&r) && !out.iter().any(|p| same_path(p, &r)) {
            out.push(r);
        }
    }
    Ok(out)
}

#[cfg(windows)]
fn peek_square_corners(win: &tauri::WebviewWindow) {
    if let Ok(h) = win.hwnd() {
        peek_square_corners_hwnd(h.0 as isize);
    }
}

/// 真正干活的那一半：只认 HWND，`WebviewWindow` / `Window` 都能喂进来。
///
/// 为什么要把 HWND 单独拆出来：**DWM 的属性会被窗口几何变化冲掉**。
/// 截图贴图那条路（`rec.place` 有值）在 `build()` 之后还要 `set_size` + `set_position`
/// 做"原位原尺寸"，系统在重排窗口时把 `BORDER_COLOR=NONE` 重置回默认描边 ——
/// 表现就是「右键菜单贴图没有蓝边，截图后中键贴图有蓝边」（owner 2026-11-19）。
/// 所以凡是会改几何 / show 的地方，都要再刷一次这个属性。
#[cfg(windows)]
fn peek_square_corners_hwnd(hwnd: isize) {
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attr: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWCP_DONOTROUND: i32 = 1;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    unsafe {
        let round = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        let none = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &none as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

#[cfg(not(windows))]
fn peek_square_corners(_win: &tauri::WebviewWindow) {}

#[cfg(not(windows))]
fn peek_square_corners_hwnd(_hwnd: isize) {}

/// 开一扇贴图窗口（三种来源 + 重启恢复共用这一条路）
async fn spawn_peek(
    app: &AppHandle,
    item: PeekItem,
    rec: PeekRecall,
    restore: bool,
) -> Result<(), String> {
    /* 新贴一张 = 回到"全部可见"：否则 Alt+F3 藏起来的那批会一直藏着，
       下一次按 Alt+F3 又走「隐藏」分支，用户唤不回来（见 release_peek_hide）。 */
    release_peek_hide(app);    /* 「截图 → 贴图」要 1:1 摆在原位（`place` 有值），这时**不许**按长边等比缩放：
       先按目标物理尺寸 / 屏幕 scale 给个接近的逻辑初值，`build()` 之后再用
       `set_size(Physical)` 精确对齐（多显示器 DPI 不同也不会偏）。 */
    let app_scale = app
        .get_webview_window("main")
        .and_then(|w| w.scale_factor().ok())
        .filter(|s| *s > 0.1)
        .unwrap_or(1.0);
    /* 逻辑尺寸 = 图的尺寸 + 两侧发光边。`inner_size` 收的是逻辑像素，
       所以这里用的是 CSS px 的 PEEK_GLOW（不乘 scale）。 */
    let pad = PEEK_GLOW * 2.0;
    let (lw, lh) = {
        let (iw, ih) = match rec.place {
            Some([_, _, w, h]) if w > 2.0 && h > 2.0 => (w / app_scale, h / app_scale),
            _ => peek_size_for(item.w, item.h, rec.long_edge),
        };
        (iw + pad, ih + pad)
    };
    if let Some(old) = app.get_webview_window(&rec.label) {
        let _ = old.close();
    }
    let boot = serde_json::json!({
        "id": rec.label,
        "items": [item],
        "i": 0,
        "opacity": rec.opacity,
        "theme": match UI_THEME.load(std::sync::atomic::Ordering::SeqCst) {
            1 => "light",
            _ => "dark",
        },
        "frame": PEEK_FRAME.load(std::sync::atomic::Ordering::SeqCst),
    });
    let script = format!("window.__AM_PEEK__ = {boot};");
    let b = tauri::WebviewWindowBuilder::new(
        app,
        &rec.label,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("看图")
    .inner_size(lw, lh)
    .decorations(false)
    // 透明：Ctrl+滚轮调不透明度才有意义（不透明窗只能调成"对着黑底变暗"）
    .transparent(true)
    .shadow(false)
    .resizable(true)
    .maximizable(false)
    .minimizable(false)
    .always_on_top(rec.top)
    .skip_taskbar(true)
    /* **先不显示**：WebView2 建窗后到首帧之间会闪一下默认底色（黑）。
       中键把截图变无边框看图时这一下特别明显（owner 2026-11-17 反馈"黑屏一下"）。
       现在等前端把图画上去再 `am_peek_ready` 显示；下面还有兜底定时器，前端崩了也不会留个隐形窗口。 */
    .visible(false)
    .initialization_script(&script);
    let win = b.build();
    // build() 失败**不能 `?` 提前返回**：窗口可能已经建了一半，错误又被前端 toast 吞掉
    let win = match win {
        Ok(w) => w,
        Err(e) => {
            if let Some(d) = LOG_DIR.get() {
                log_runtime(d, &format!("peek build FAIL {e}"));
            }
            return Err(format!("无边框窗口创建失败：{e}"));
        }
    };
    peek_square_corners(&win);
    if let Some([px, py, pw, ph]) = rec.place {
        /* 原位原尺寸：先把窗口量准（图 + 两侧发光边），再挪到"选区左上角减去发光边"那个点 ——
           图就正好落在选区原来的位置、原来的大小。
           顺序不能反 —— 先定位后改尺寸的话某些 DPI 下 set_size 会顺带挪一次。 */
        let sc = win.scale_factor().unwrap_or(1.0);
        let g = (PEEK_GLOW * sc).round();
        let ww = (pw + 2.0 * g).round().max(1.0);
        let wh = (ph + 2.0 * g).round().max(1.0);
        let wx = (px - g).round() as i32;
        let wy = (py - g).round() as i32;
        let _ = win.set_size(tauri::PhysicalSize::new(ww as u32, wh as u32));
        let _ = win.set_position(tauri::PhysicalPosition::new(wx, wy));
        /* 存档里的 `place` 语义统一成**窗口 rect**（含发光边）——
           拖动/缩放事件记的也是窗口 rect，恢复时直接照用，不会一次次把边距叠上去。 */
        peek_touch(&rec.label, |r| r.place = Some([wx as f64, wy as f64, ww, wh]));
    } else if restore {
        /* 存档里记的是**物理像素**（`Moved` 事件给的也是物理），
           而 `WindowBuilder::position()` 收的是**逻辑像素** —— 直接喂过去在高 DPI 上
           会偏移 25%（实测 983 变成 1229）。所以恢复用 set_position(Physical…)。 */
        let _ = win.set_position(tauri::PhysicalPosition::new(rec.x, rec.y));
    } else {
        peek_center_on_cursor(&win);
    }
    /* 三条分支都可能刚改过窗口几何（原位原尺寸 / 恢复位置 / 居中到光标），
       而 DWM 的系统描边会跟着重排重新出现 —— 所以几何定下来之后**再刷一次**。
       这一行就是「截图贴图有蓝边、素材贴图没有」的修复点（owner 2026-11-19）。 */
    peek_square_corners(&win);
    /* 拖动/缩放要记进存档，否则重启后全都回到屏幕正中（用户摆的位置白摆了）。
       拖动事件 ~60/s，所以走 `peek_touch` 的 350ms 节流。
       注意：这是 `WebviewWindow` 上的方法，`WebviewWindowBuilder` 没有 `on_window_event`。 */
    let lbl = rec.label.clone();
    let sc0 = win.scale_factor().unwrap_or(1.0);
    /* HWND 先取出来（`isize` 是 Copy），下面的事件闭包里还要用。
       DWM 的"不要圆角 / 不要描边"会被**任何一次几何变化**重置 ——
       拖动、缩放、摆回选区原位都会，所以事件里每次都要补一刀；
       否则用户拖一下贴图，蓝边就又回来了。 */
    #[cfg(windows)]
    let hwnd0 = win.hwnd().ok().map(|h| h.0 as isize);
    #[cfg(not(windows))]
    let hwnd0: Option<isize> = None;
    win.on_window_event(move |event| match event {
        tauri::WindowEvent::Moved(p) => {
            if let Some(h) = hwnd0 {
                peek_square_corners_hwnd(h);
            }
            peek_touch(&lbl, |r| {
                r.x = p.x;
                r.y = p.y;
                /* 精确几何也要跟着走：`place` 是"这次贴图摆在哪"的唯一事实，
                   重启还原靠它，别只在建窗那一次算。 */
                if let Some(q) = r.place.as_mut() {
                    q[0] = p.x as f64;
                    q[1] = p.y as f64;
                }
            });
        }
        tauri::WindowEvent::Resized(s) => {
            if let Some(h) = hwnd0 {
                peek_square_corners_hwnd(h);
            }
            if s.width > 0 && s.height > 0 {
                let long = (s.width.max(s.height) as f64 / sc0).round();
                peek_touch(&lbl, |r| {
                    r.long_edge = long;
                    if let Some(q) = r.place.as_mut() {
                        q[2] = s.width as f64;
                        q[3] = s.height as f64;
                    }
                });
            }
        }
        _ => {}
    });
    // 位置/尺寸变了就记下来（下次开机原地还原）
    let scale = win.scale_factor().unwrap_or(1.0);
    if let Ok(pos) = win.outer_position() {
        peek_touch(&rec.label, |r| {
            r.x = pos.x;
            r.y = pos.y;
        });
    }
    let _ = scale;
    /* 兜底：前端如果没能报到 ready（脚本报错 / 图片解码失败），3.5 秒后自己显示出来 ——
       否则用户按了键却"什么都没发生"，比闪一下黑更糟。 */
    let w2 = win.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(3500));
        if !w2.is_visible().unwrap_or(false) {
            let _ = w2.show();
            let _ = w2.set_focus();
        }
    });
    Ok(())
}

/// 开一扇无边框看图窗口（素材）。`ids` 是这次浏览的整个集合。
///
/// **必须是 `async fn`**：同步命令跑在主线程上，而 `WebviewWindowBuilder::build()` /
/// `set_size()` 又要回主线程执行 —— 于是它们等自己，自死锁（见 WORKLOG §14.3）。
#[tauri::command]
async fn am_peek_open(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
    index: usize,
    long_edge: Option<f64>,
) -> Result<(), String> {
    if ids.is_empty() {
        return Err("没有可看的素材".into());
    }
    let (ids, i) = trim_around(&ids, index, 600);
    let items = {
        let a = lock(&state);
        items_of_assets(&a, &ids)
    };
    let Some(item) = items.get(i).cloned() else {
        return Err("这张素材已经不在库里了".into());
    };
    let rec = PeekRecall {
        label: next_label(),
        kind: "asset".into(),
        path: item.path.clone(),
        gid: ids.get(i).copied().unwrap_or(0),
        long_edge: long_edge.unwrap_or(PEEK_LONG_EDGE),
        ..Default::default()
    };
    peeks().lock().unwrap_or_else(|e| e.into_inner()).push(rec.clone());
    save_peeks();
    spawn_peek(&app, item, rec, false).await
}

/// 前端把图画上去了 → 现在显示窗口（首帧已经就绪，不会闪黑）
#[tauri::command]
async fn am_peek_ready(win: Window) -> Result<(), String> {
    win.show().map_err(|e| e.to_string())?;
    let _ = win.set_focus();
    /* show / 拿焦点同样会让系统把窗口描边再画一遍，这里补最后一刀。 */
    #[cfg(windows)]
    if let Ok(h) = win.hwnd() {
        peek_square_corners_hwnd(h.0 as isize);
    }
    Ok(())
}

/// 缩放窗口（连窗口一起缩放，锚定中心）
#[tauri::command]
async fn am_peek_zoom(win: tauri::Window, step: f64) -> Result<(), String> {
    if !(0.5..=2.0).contains(&step) {
        return Err("缩放步长不合法".into());
    }
    /* **客户区**而不是外框：`outer_size` 含系统描边，DWM 描边哪怕只 1px，
       每次缩放都把它算进"图"的尺寸里，几次之后内容盒就比图宽/高几像素，
       `object-fit` 于是留出四条透明缝 —— 表现就是"放大后四个角又有边框"。
       `set_size` 设的本来就是客户区，这里配套用 `inner_*` 口径才对得上。 */
    let size = win.inner_size().map_err(|e| e.to_string())?;
    let pos = win.inner_position().map_err(|e| e.to_string())?;
    let scale = win.scale_factor().map_err(|e| e.to_string())?;

    /* 缩放的是「图」，不是整扇窗：先把四周的发光边减掉，缩完再加回去，
       这样滚轮缩放时那一圈渐变不会跟着变胖变瘦。 */
    let g = (PEEK_GLOW * scale).round();
    let mut w = (size.width as f64 - 2.0 * g).max(8.0) * step;
    let mut h = (size.height as f64 - 2.0 * g).max(8.0) * step;
    let long = w.max(h);
    let min = PEEK_MIN_EDGE * scale;
    let max = PEEK_MAX_EDGE * scale;
    let k = if long < min {
        min / long
    } else if long > max {
        max / long
    } else {
        1.0
    };
    w = (w * k).round().max(24.0);
    h = (h * k).round().max(24.0);
    let (ww, wh) = (w + 2.0 * g, h + 2.0 * g);
    let nx = (pos.x as f64 + (size.width as f64 - ww) / 2.0).round();
    let ny = (pos.y as f64 + (size.height as f64 - wh) / 2.0).round();
    win.set_size(tauri::PhysicalSize::new(ww, wh)).map_err(|e| e.to_string())?;
    win.set_position(tauri::PhysicalPosition::new(nx, ny)).map_err(|e| e.to_string())?;
    /* 缩放改了几何 → 系统描边会回来，补一刀（和 spawn_peek 里同一个理由）。 */
    #[cfg(windows)]
    if let Ok(h) = win.hwnd() {
        peek_square_corners_hwnd(h.0 as isize);
    }
    peek_touch(win.label(), |r| {
        r.long_edge = (w.max(h) / scale).round();
        r.x = nx as i32;
        r.y = ny as i32;
    });
    Ok(())
}

/// 换图时按新图比例改窗口尺寸（长边保持当前大小，中心不动）
#[tauri::command]
async fn am_peek_ratio(win: tauri::Window, w: u32, h: u32) -> Result<(), String> {
    let size = win.outer_size().map_err(|e| e.to_string())?;
    let pos = win.outer_position().map_err(|e| e.to_string())?;
    let scale = win.scale_factor().map_err(|e| e.to_string())?;
    let g = (PEEK_GLOW * scale).round();
    /* `peek_size_for` 收的是**逻辑**长边，这里减掉发光边、再按 scale 折回逻辑 */
    let long = ((size.width.max(size.height) as f64 - 2.0 * g) / scale).max(24.0);
    let (nw, nh) = peek_size_for(w, h, long);
    let (nw, nh) = (nw * scale + 2.0 * g, nh * scale + 2.0 * g);
    let nx = (pos.x as f64 + (size.width as f64 - nw) / 2.0).round();
    let ny = (pos.y as f64 + (size.height as f64 - nh) / 2.0).round();
    win.set_size(tauri::PhysicalSize::new(nw.round(), nh.round())).map_err(|e| e.to_string())?;
    win.set_position(tauri::PhysicalPosition::new(nx, ny)).map_err(|e| e.to_string())?;
    Ok(())
}

/// 关掉这扇贴图（用户主动关 = 从存档里删掉，下次开机不再回来）
#[tauri::command]
async fn am_peek_close(win: tauri::Window) -> Result<(), String> {
    let label = win.label().to_string();
    peek_forget(&label);
    win.close().map_err(|e| e.to_string())
}

fn peek_forget(label: &str) {
    {
        let mut g = peeks().lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|r| r.label != label);
    }
    save_peeks();
}

#[tauri::command]
async fn am_peek_top(win: tauri::Window, on: bool) -> Result<bool, String> {
    win.set_always_on_top(on).map_err(|e| e.to_string())?;
    peek_touch(win.label(), |r| r.top = on);
    Ok(on)
}

/// 不透明度（Ctrl+滚轮）。窗口是真透明的，所以这里只是让前端把画布调淡，
/// 存档里记一份，下次恢复还是这个浓度。
#[tauri::command]
fn am_peek_opacity(win: tauri::Window, v: f64) -> Result<(), String> {
    let v = v.clamp(0.15, 1.0);
    peek_touch(win.label(), |r| r.opacity = v);
    Ok(())
}

/// 窗口里换了图（←/→）：把存档指向新文件，重启后还原的是最后看到的那张
#[tauri::command]
fn am_peek_track(win: tauri::Window, path: String, gid: i64) {
    peek_touch(win.label(), |r| {
        r.path = path;
        r.gid = gid;
        r.kind = if gid > 0 { "asset".into() } else { "file".into() };
    });
}

/// 开机/启动恢复上次没关的贴图
async fn restore_peeks(app: AppHandle) {
    let list = load_peeks();
    if list.is_empty() {
        return;
    }
    {
        let mut g = peeks().lock().unwrap_or_else(|e| e.into_inner());
        g.clear();
    }
    // 标签序号要从已有存档之后接着走，否则新开的窗口会和恢复的撞 label
    let mut max_seq = 0u64;
    for rec in &list {
        if let Some(n) = rec.label.strip_prefix("peek-").and_then(|x| x.parse::<u64>().ok()) {
            max_seq = max_seq.max(n);
        }
    }
    PEEK_SEQ.store(max_seq + 1, Ordering::Relaxed);

    /* 还原失败的记录**留在 peeks.json 里**，不删除。
       为什么：原来这里只把成功建窗的记录写回去，失败的（缓存 png 被清理、盘没插、
       文件被移走）就永久消失了 —— 用户看到的是"昨天贴着的参考图今天开机就没了"，
       连解释都没有。留着它下次开机再试一次，代价只是 json 里多一行。
       真要丢掉，只能由用户在界面上关掉那扇窗（那是 window-destroyed 那条路）。 */
    let mut alive: Vec<PeekRecall> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for rec in list {
        let Some(item) = recall_to_item(&app, &rec) else {
            let why = if !Path::new(&rec.path).is_file() {
                "找不到文件（被移动/删除，或盘没插）"
            } else {
                "格式不能直接贴（非位图）"
            };
            failed.push((rec.path.clone(), why.to_string()));
            alive.push(rec);
            continue;
        };
        match spawn_peek(&app, item, rec.clone(), true).await {
            Ok(_) => alive.push(rec),
            Err(e) => {
                failed.push((rec.path.clone(), format!("建窗失败：{e}")));
                alive.push(rec);
            }
        }
    }
    {
        let mut g = peeks().lock().unwrap_or_else(|e| e.into_inner());
        *g = alive;
    }
    save_peeks();
    if let Some(d) = LOG_DIR.get() {
        for (p, why) in &failed {
            log_runtime(d, &format!("peek restore failed: {p} —— {why}"));
        }
        log_runtime(
            d,
            &format!(
                "peeks restored: {} failed: {}",
                peeks().lock().map(|g| g.len()).unwrap_or(0) - failed.len(),
                failed.len()
            ),
        );
    }
}

/* ---------------- 灵感板（标签 → 无限画布，照 PureRef） ----------------

   一个标签 = 一块板。板是**无边界画布**：图可以随便摆，画布跟着拖，滚轮缩放，
   不存位置（这是临时视图，不是文档 —— 重启不开，和 PureRef 的 .pur 文件不是一回事）。
   要做成"存下来 + 重启还原"就得先有 board 表，等 owner 明确要再加。 */

/// 一块板最多放多少张。2000 张 1024 档缩略图在 WebView2 里还能拖动得动，
/// 再多就开始掉帧；超出的部分不是悄悄丢掉，窗口里会写明"只放了前 N 张"。
const BOARD_MAX: usize = 2000;
static BOARD_SEQ: AtomicU64 = AtomicU64::new(1);
/* CDP 调试参数（`AM_CDP=1` 时才挂）。**凡是多开的窗口都要带同一串**：
   WebView2 的 additional browser args 是 **environment 级**的，主窗带了、新窗不带，
   创建直接失败（实测灵感板报 `0x8007139F 组或资源的状态不是执行请求操作的正确状态`）。
   发布路径不设这个环境变量，行为不变。 */
const CDP_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port=9222";
/// label → 标签名。同一个标签重复点「打开灵感板」不该开第二块，聚焦已有那块。
static BOARD_TAGS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn board_tags() -> &'static Mutex<HashMap<String, String>> {
    BOARD_TAGS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone, Serialize)]
struct BoardItem {
    id: i64,
    src: String,
    w: u32,
    h: u32,
    name: String,
    /// 复制原文件用（右键菜单）
    path: String,
    /// 动图（gif / apng / webp）：板上**默认静止**，指针停上去才换原图播
    anim: bool,
    /// 原文件地址（`px=0` 直吐原始字节，不进解码管线）：动图靠它才动得起来
    raw: String,
}

/// 动图候选：只看扩展名。真正的帧数要解码才知，这里只决定"值不值得挂原图" ——
/// 静态 webp 多请求一次原图也没什么代价（还是本地文件，不进缓存）。
fn anim_ext(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "gif" | "apng" | "webp")
}

/// 上板条目统一在这里拼 URL：`src` 是 1024 档缩略图（静止用），
/// `raw` 是原文件（动图悬停用）。以前四处各写一份，改档位就会漏一处。
fn board_item(id: i64, mtime: i64, w: u32, h: u32, name: String, path: String) -> BoardItem {
    BoardItem {
        id,
        src: format!("http://thumb.localhost/{id}?px=1024&r={mtime}"),
        raw: format!("http://thumb.localhost/{id}?px=0&r={mtime}"),
        anim: anim_ext(&path),
        w,
        h,
        name,
        path,
    }
}

/// 取全量素材上板（keyset 分页，单页上限 1000）。返回 `(条目, 命中总数)`。
///
/// `base` 只决定"是哪个集合"（标签 / 分组 / 快照），分页与上限统一在这里 ——
/// 以前只有标签一条路，分组想开板就得复制一整份实现。
fn board_items_q(app: &App, base: AssetQuery) -> Result<(Vec<BoardItem>, usize), String> {
    let mut out: Vec<BoardItem> = Vec::new();
    let mut cursor: Option<(i64, i64)> = None;
    let mut total = 0usize;
    loop {
        let q = AssetQuery {
            limit: Some(1000),
            cursor,
            sort: Some("mtime_desc".into()),
            ..base.clone()
        };
        let rows = app.list(&q)?;
        if rows.is_empty() {
            break;
        }
        total += rows.len();
        for r in &rows {
            let (w, h) = (
                r.width.unwrap_or(0).max(0) as u32,
                r.height.unwrap_or(0).max(0) as u32,
            );
            /* 板上按 1024 档取缩略图：放到两三倍还看得清，又不至于把内存吃光。
               `r` 带上 mtime，文件改了 URL 就变，WebView 缓存自动失效。 */
            out.push(board_item(r.id, r.mtime, w, h, r.name.clone(), r.path.clone()));
        }
        if out.len() >= BOARD_MAX {
            break;
        }
        match rows.last() {
            Some(last) => cursor = Some((last.mtime, last.id)),
            None => break,
        }
    }
    Ok((out, total))
}

/// 按标签取全量素材（灵感板 / 标签→快照都用它）
fn board_items(app: &App, tag: &str) -> Result<(Vec<BoardItem>, usize), String> {
    board_items_q(
        app,
        AssetQuery {
            tag: Some(tag.to_string()),
            ..Default::default()
        },
    )
}

/// 按分组取全量素材
fn board_items_coll(app: &App, coll: i64) -> Result<(Vec<BoardItem>, usize), String> {
    board_items_q(
        app,
        AssetQuery {
            collection: Some(coll),
            ..Default::default()
        },
    )
}

/// 开一块灵感板。同一个标签已经开着就直接聚焦，不重复开。
/// 开一块灵感板窗口（**标签板 / 快照板共用**）。
/// `key` 是去重键（`tag:参考` / `snap:12`）：同一个键已经开着就聚焦，不再开第二块。
/// `boot` 原样塞进新窗口的 `window.__AM_BOARD__`，前端 `Board.tsx` 只从那里读一切。
fn spawn_board(
    app: &AppHandle,
    key: &str,
    title: &str,
    boot: serde_json::Value,
) -> Result<(), String> {
    {
        let map = board_tags().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((label, _)) = map.iter().find(|(_, k)| k.as_str() == key) {
            if let Some(w) = app.get_webview_window(label) {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
                return Ok(());
            }
        }
    }
    let label = format!("board-{}", BOARD_SEQ.fetch_add(1, Ordering::Relaxed));
    let script = format!("window.__AM_BOARD__ = {boot};");
    let mut bb = tauri::WebviewWindowBuilder::new(
        app,
        &label,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title(title)
    /* 默认 900×650（owner 2026-11-18）；最小 100×100 —— 板子常被当成"随手看一眼"的
       临时窗口，1280 太占屏、720 的下限又不让它缩成角落小图。 */
    .inner_size(900.0, 650.0)
    .min_inner_size(100.0, 100.0)
    .decorations(false)
    .transparent(false)
    .resizable(true)
    .center()
    .initialization_script(&script);
    /* 见 CDP_ARGS 的注释：主窗带了调试参数，这块板也必须带同一串，否则窗口建不出来 */
    if std::env::var("AM_CDP").as_deref() == Ok("1") {
        bb = bb.additional_browser_args(CDP_ARGS);
    }
    let win = bb
        .build()
    .map_err(|e| {
        if let Some(d) = LOG_DIR.get() {
            log_runtime(d, &format!("board build FAIL {e}"));
        }
        format!("灵感板窗口创建失败：{e}")
    })?;
    board_tags()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(label.clone(), key.to_string());
    // 窗口没了就把映射清掉，否则下次点同一个键会去聚焦一个不存在的窗口
    let lbl = label.clone();
    win.on_window_event(move |ev| {
        if matches!(ev, tauri::WindowEvent::Destroyed) {
            board_tags().lock().unwrap_or_else(|e| e.into_inner()).remove(&lbl);
        }
    });
    let _ = win.set_focus();
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("board {label} key={key}"));
    }
    Ok(())
}

/// 开一块标签板。同一个标签已经开着就直接聚焦，不重复开。
#[tauri::command]
async fn am_board_open(
    app: AppHandle,
    state: State<'_, AppState>,
    tag: String,
    title: Option<String>,
) -> Result<usize, String> {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return Err("没有指定标签".into());
    }
    let (items, total) = {
        let a = lock(&state);
        board_items(&a, &tag)?
    };
    /* 空标签也开板：板子的第二个用法就是"往空画布上粘图"（Ctrl+V 会存进截图目录
       并打上这个标签）。原来这里直接报「下面还没有素材」，等于把这条路堵死了。 */
    let shown = items.len();
    let title = title.unwrap_or_else(|| format!("灵感板 · {tag}"));
    let boot = serde_json::json!({
        "board": format!("tag:{tag}"),
        "tag": tag,
        "title": title,
        "items": items,
        "more": total > shown,
        "total": total,
    });
    spawn_board(&app, &format!("tag:{tag}"), &title, boot)?;
    Ok(shown)
}

#[tauri::command]
async fn am_board_close(win: Window) -> Result<(), String> {
    win.close().map_err(|e| e.to_string())
}

/// 为**分组**开一块灵感板（结构照快照板：摆位键 `coll:<id>`，粘贴回填到分组）。
///
/// 为什么分组也要有板：分组和快照是同一层概念（一批图），
/// 用户在图库里挑素材时先在分组里摆，再决定要不要固化成快照。
#[tauri::command]
async fn am_coll_open(app: AppHandle, state: State<'_, AppState>, coll: i64) -> Result<usize, String> {
    let (name, items, total) = {
        let a = lock(&state);
        let name = a
            .coll_list()?
            .into_iter()
            .find(|c| c.id == coll)
            .map(|c| c.name)
            .ok_or_else(|| "这个分组已经不在了".to_string())?;
        let (items, total) = board_items_coll(&a, coll)?;
        (name, items, total)
    };
    let shown = items.len();
    let title = format!("灵感板 · 分组 {name}");
    let boot = serde_json::json!({
        /* 摆位的键：与 spawn_board 的 key 同源，改一处必须改两处 */
        "board": format!("coll:{coll}"),
        "coll": { "id": coll, "name": name },
        "title": title,
        "items": items,
        "more": total > shown,
        "total": total,
    });
    spawn_board(&app, &format!("coll:{coll}"), &title, boot)?;
    Ok(shown)
}

/// 灵感板摆位：读上次摆的位置/尺寸/组（`[[gid,x,y,w,h,grp], …]`）。读不到就空 —— 板按瀑布流重排
#[tauri::command]
fn am_board_layout(
    state: State<'_, AppState>,
    board: String,
) -> Result<Vec<(i64, f64, f64, f64, f64, i64)>, String> {
    lock(&state).board_layout(&board)
}

/// 灵感板摆位：记住这几张的位置/尺寸（松手时逐张写，不整板重写）
#[tauri::command]
fn am_board_place(
    state: State<'_, AppState>,
    board: String,
    rows: Vec<(i64, f64, f64, f64, f64)>,
) -> Result<usize, String> {
    lock(&state).board_place(&board, &rows)
}

/// 灵感板便签：读这块板上所有文字注释（含字体外观与组号，按创建顺序）
#[tauri::command]
fn am_board_notes(
    state: State<'_, AppState>,
    board: String,
) -> Result<Vec<am_core::reg::BoardNoteRow>, String> {
    lock(&state).board_notes(&board)
}

/// 灵感板便签：新建（`nid = null`，后端发号）或更新一张，返回 nid。
///
/// 文字为空也算一次正常写入 —— **清空文字的便签由前端决定删不删**，
/// 后端不替它判断，否则"先清空再打字"会在中途被删掉。
///
/// `font / fs / color / bold` 传 `null` 表示**保留原样**（只挪位置时用）：
/// 否则拖动一下就把用户设的字体冲回默认值。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn am_board_note_put(
    state: State<'_, AppState>,
    board: String,
    nid: Option<i64>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    text: String,
    font: Option<String>,
    fs: Option<f64>,
    color: Option<String>,
    bold: Option<bool>,
) -> Result<i64, String> {
    /* 四个外观字段要么一起给、要么一起不给：拆开传会让"改了字号但字体被冲掉"
       这种半截状态有机会出现。任何一个缺了就当没给。 */
    let look = match (font, fs, color, bold) {
        (Some(f), Some(s), Some(c), Some(b)) => Some((f, s, c, b)),
        _ => None,
    };
    match look {
        Some((f, s, c, b)) => {
            lock(&state).board_note_put(&board, nid, x, y, w, h, &text, Some((&f, s, &c, b)))
        }
        None => lock(&state).board_note_put(&board, nid, x, y, w, h, &text, None),
    }
}

/// 灵感板便签：删一张
#[tauri::command]
fn am_board_note_del(state: State<'_, AppState>, board: String, nid: i64) -> Result<usize, String> {
    lock(&state).board_note_del(&board, nid)
}

/// 灵感板打组 / 解组：`grp = -1` 新建一组（后端发号并返回），`grp = 0` 解组。
/// key 正数 = 图片 gid、负数 = 便签 nid 取负（和前端 `pos` 的 key 一致）。
#[tauri::command]
fn am_board_group(
    state: State<'_, AppState>,
    board: String,
    keys: Vec<i64>,
    grp: i64,
) -> Result<i64, String> {
    lock(&state).board_group_set(&board, &keys, grp)
}

/// 往灵感板上**拖文件进来**（P2.8）：只收已经入库的那几张。
///
/// 为什么不顺便把文件复制/导入进来：板子是"看图的地方"，不是第二个入库入口。
/// 拖进来的文件不在素材库里时**如实告诉用户**（而不是静默丢掉），
/// 让他把那个目录加进素材库再拖 —— 这样索引、缩略图、标签全都一致。
///
/// 返回 `(能上板的条目, 不能上板的原因)`
#[tauri::command]
fn am_board_add_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<(Vec<BoardItem>, Vec<String>), String> {
    let app = lock(&state);
    let mut items: Vec<BoardItem> = Vec::new();
    let mut bad: Vec<String> = Vec::new();
    for p in paths {
        let path = Path::new(&p);
        if !path.is_file() {
            bad.push(format!("{}（不是文件）", p));
            continue;
        }
        let Some(lib_id) = app.lib_for_path(&p) else {
            bad.push(format!("{}（不在任何素材库里）", p));
            continue;
        };
        let Ok(conn) = app.conn_for_path(&p) else {
            bad.push(format!("{}（打不开它所在的库）", p));
            continue;
        };
        let local = match db::asset_id_by_path(conn, &p) {
            Ok(Some(id)) => id,
            _ => {
                bad.push(format!(
                    "{}（还没入库：把它所在目录加进素材库后就会出现在这里）",
                    p
                ));
                continue;
            }
        };
        let Some(a) = db::asset_by_id(conn, local)? else {
            bad.push(format!("{p}（索引里没有这一行）"));
            continue;
        };
        let g = am_core::ids::gid(lib_id, local);
        items.push(board_item(
            g,
            a.mtime,
            a.width.unwrap_or(0).max(0) as u32,
            a.height.unwrap_or(0).max(0) as u32,
            a.name.clone(),
            a.path.clone(),
        ));
    }
    Ok((items, bad))
}

/// 同一秒里连粘两张会撞名（时间戳精确到秒）：`粘贴 2026-11-17 12-42-11.png`
/// → `… (2).png`。**绝不覆盖已有文件**（截图目录里都是用户的东西）。
fn free_path(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let p = dir.join(format!("{stem} ({n}).png"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem}-{}.png", std::process::id()))
}

/// 截图文件名：`截图 2026-11-21.png`（owner 2026-11-21：**只要年月日**，不要时分秒）。
/// 同一天第 2、3 张自动 `(2)(3)` —— 截图目录里都是用户的东西，绝不覆盖。
fn shot_target(dir: &Path, day: &str) -> PathBuf {
    let first = dir.join(format!("截图 {day}.png"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let p = dir.join(format!("截图 {day} ({n}).png"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("截图 {day} {}.png", std::process::id()))
}

/// 今天的日期串 `2026-11-21`（`local_stamp` 是"日期 时-分-秒"，这里只取前段）
fn today_day() -> String {
    let (stamp, _) = screen::local_stamp();
    stamp.split(' ').next().unwrap_or("").to_string()
}

/// 灵感板里 Ctrl+V：剪贴板图片 → **截图目录** → 入库 → 打上这块板的标签 → 上板。
///
/// 顺序不能换：先落盘、再入库、最后打标签。
/// - 落盘失败 = 什么都没发生（用户还能再按一次 Ctrl+V，不会留下半截文件）；
/// - 入库失败时文件**已经在盘上了**，必须把路径说出来，别装没发生；
/// - 标签是这块板的标签（一块板 = 一个标签），所以粘进来的图下次打开同一块板还在。
///
/// 返回的是**新入库那张素材**的信息，前端拿它直接贴到视口中心。
#[tauri::command]
/// 剪贴板图片 → `程序目录\screenshots\粘贴 <时间戳>.png`。
/// 标签板和快照板的 Ctrl+V 共用这一条（先落盘，再入库，最后挂到板上）。
fn save_clip_png() -> Result<(PathBuf, u32, u32), String> {
    let (cw, ch, png) = clipimg::get_png()
        .map_err(|e| format!("{e}。Ctrl+V 只认剪贴板里的图片（复制文件请用右键菜单）"))?;
    let dir = shot_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("建不了截图目录 {}：{e}", dir.display()))?;
    let (stamp, _) = screen::local_stamp();
    let target = free_path(&dir, &format!("粘贴 {stamp}"));
    std::fs::write(&target, &png).map_err(|e| format!("写图片失败 {}：{e}", target.display()))?;
    Ok((target, cw, ch))
}

/// 板上 Ctrl+V 的落点：剪贴板图片存盘入库之后，要**挂到哪个集合**上。
///
/// 三种板（标签板 / 快照板 / 分组板）只有这一步不同，其余（存盘、入库、刷新、回传条目）
/// 完全一样 —— 所以合成一条路，别再复制第三份。
#[derive(Clone)]
enum PasteSink {
    Tag(String),
    Snapshot(i64),
    Coll(i64),
}

impl PasteSink {
    /// 失败文案里"这次没做成什么"那一句：必须说清落在哪里，只说"失败"用户没法查
    fn describe(&self) -> String {
        match self {
            PasteSink::Tag(t) => format!("没打上「{t}」标签"),
            PasteSink::Snapshot(_) => "没进快照".to_string(),
            PasteSink::Coll(_) => "没进分组".to_string(),
        }
    }

    fn log_key(&self) -> String {
        match self {
            PasteSink::Tag(t) => format!("tag={t}"),
            PasteSink::Snapshot(id) => format!("snap={id}"),
            PasteSink::Coll(id) => format!("coll={id}"),
        }
    }
}

/// 板上 Ctrl+V 的共用实现：存进截图目录 → 入库 → 挂到 `sink` → 回传一条能上板的条目。
async fn paste_into(
    app: &AppHandle,
    state: &State<'_, AppState>,
    sink: PasteSink,
) -> Result<BoardItem, String> {
    let (target, cw, ch) = save_clip_png()?;
    let saved = target.to_string_lossy().to_string();

    // 截图目录自动进库（库满 3 个就只存盘、不进库 —— 下面会明确告出来，不静默）
    remember_shot_dir(app);

    let (lib_id, local) = {
        let a = lock(state);
        let lib_id = a.lib_for_path(&saved).ok_or_else(|| {
            format!(
                "图片已存到 {saved}，但截图目录没能加入素材库（多半是素材库已满 3 个），所以这次{}",
                sink.describe()
            )
        })?;
        let local = db::asset_id_by_path(a.conn_for_path(&saved)?, &saved)?
            .ok_or_else(|| format!("图片已存到 {saved}，但索引里还没找到它 —— 稍等一下再按一次 Ctrl+V"))?;
        (lib_id, local)
    };
    let g = am_core::gid(lib_id, local);
    {
        let mut a = lock(state);
        match &sink {
            PasteSink::Tag(t) => {
                a.tag_assets(&[g], t)?;
            }
            PasteSink::Snapshot(id) => {
                a.snapshot_add_assets(*id, &[g])?;
            }
            PasteSink::Coll(id) => {
                a.coll_add(*id, &[g])?;
            }
        }
    }
    let asset = {
        let a = lock(state);
        a.asset(g)?
    }
    .ok_or_else(|| "素材刚入库就又找不到了，请重试".to_string())?;

    // 主窗的列表要立刻能看到这张新图（走的是「外部变更」那条刷新链路，400ms 去抖）
    if let Some(parent) = target.parent() {
        let _ = app.emit("fs://changed", vec![parent.to_string_lossy().to_string()]);
    }
    log_runtime_opt(&format!("board paste {} -> {saved}", sink.log_key()));

    Ok(board_item(
        g,
        asset.mtime,
        // 扫描没填上宽高时用剪贴板自己的尺寸，别让板上出现一张 0×0 的方块
        asset.width.unwrap_or(cw as i64).max(0) as u32,
        asset.height.unwrap_or(ch as i64).max(0) as u32,
        asset.name.clone(),
        asset.path.clone(),
    ))
}

/// 标签板里的 Ctrl+V：存盘入库后**打上这个标签**，再贴到板上。
#[tauri::command]
async fn am_board_paste(
    app: AppHandle,
    state: State<'_, AppState>,
    tag: String,
) -> Result<BoardItem, String> {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return Err("这块灵感板没有对应的标签".into());
    }
    paste_into(&app, &state, PasteSink::Tag(tag)).await
}

/* ---------------- 快照（0.12.0） ----------------

   一个快照 = 一批图片的**内容指纹清单**。导出的是一份几 KB 的 json（不含图片字节），
   导回来时按哈希在**当前所有库里重新找一遍** —— 改名字、换目录、换库都还认得出。
   取舍与匹配算法见 `core/src/snapshot.rs` 的头注释。 */

#[derive(Serialize)]
struct SnapshotZip {
    path: String,
    ok: usize,
    /// 打不进去的文件（读失败等）
    bad: Vec<String>,
    /// 快照里有、但这次没找到文件的条数（没进 zip）
    missing: usize,
}

/* ---------------- 分组（0.2.x） ----------------
   分组的成员是**全局 asset id**，注册库里存一份；素材本身一张都不动。
   界面上的计数由 `App::coll_list` 按"素材现在还在不在"重算。 */

#[tauri::command]
fn am_coll_list(state: State<'_, AppState>) -> Result<Vec<am_core::reg::CollRow>, String> {
    lock(&state).coll_list()
}

#[tauri::command]
fn am_coll_create(name: String, state: State<'_, AppState>) -> Result<i64, String> {
    lock(&state).coll_create(&name)
}

#[tauri::command]
fn am_coll_rename(id: i64, name: String, state: State<'_, AppState>) -> Result<(), String> {
    lock(&state).coll_rename(id, &name)
}

#[tauri::command]
fn am_coll_delete(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    lock(&state).coll_delete(id)
}

#[tauri::command]
fn am_coll_add(id: i64, ids: Vec<i64>, state: State<'_, AppState>) -> Result<usize, String> {
    lock(&state).coll_add(id, &ids)
}

/// 把**整个标签**下的素材加进分组（只建关联，原文件一个字节都不动）。
///
/// 为什么要后端做而不是前端先查一遍 id：一个标签可能有几万项，
/// 前端要分页取完再回传，中间还要序列化两次；这里直接走同一套 keyset 分页取 id。
#[tauri::command]
fn am_coll_add_tag(id: i64, tag: String, state: State<'_, AppState>) -> Result<usize, String> {
    let mut a = lock(&state);
    let ids = a.ids_by_tag(&tag)?;
    if ids.is_empty() {
        return Ok(0);
    }
    a.coll_add(id, &ids)
}

#[tauri::command]
fn am_coll_remove(id: i64, ids: Vec<i64>, state: State<'_, AppState>) -> Result<usize, String> {
    lock(&state).coll_remove(id, &ids)
}

/// 这张素材在哪些分组里（右键菜单打勾用）
#[tauri::command]
fn am_coll_of(gid: i64, state: State<'_, AppState>) -> Result<Vec<i64>, String> {
    lock(&state).coll_of(gid)
}

#[tauri::command]
fn am_snapshot_list(state: State<'_, AppState>) -> Result<Vec<am_core::reg::SnapshotRow>, String> {
    lock(&state).snapshot_list()
}
#[tauri::command]
fn am_snapshot_create(name: String, state: State<'_, AppState>) -> Result<i64, String> {
    lock(&state).snapshot_create(&name)
}

#[tauri::command]
fn am_snapshot_rename(id: i64, name: String, state: State<'_, AppState>) -> Result<(), String> {
    lock(&state).snapshot_rename(id, &name)
}

#[tauri::command]
fn am_snapshot_delete(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    lock(&state).snapshot_delete(id)
}

/// 把一批素材加进快照（界面上是"选中 → 加入快照"）
#[tauri::command]
fn am_snapshot_add(
    snapshot: i64,
    ids: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<am_core::SnapshotAdd, String> {
    lock(&state).snapshot_add_assets(snapshot, &ids)
}

/// 把某个标签下的全部图片加进快照（不移动文件，语义等价于"给这批图打书签"）
#[tauri::command]
fn am_snapshot_add_tag(
    snapshot: i64,
    tag: String,
    state: State<'_, AppState>,
) -> Result<am_core::SnapshotAdd, String> {
    lock(&state).snapshot_add_tag(snapshot, &tag)
}

/// 合并 / 差集（P2.9）：几份快照 → 一份新快照。返回 `[新 id, 条数, 新写入行数]`
#[tauri::command]
fn am_snapshot_combine(
    ids: Vec<i64>,
    name: String,
    mode: String,
    state: State<'_, AppState>,
) -> Result<(i64, usize, usize), String> {
    lock(&state).snapshot_combine(&ids, &name, &mode)
}

/// 对账：把快照里的每一条按内容哈希在当前库里找回来（missing 不为 0 只是"这次没找到"）
#[tauri::command]
fn am_snapshot_resolve(
    snapshot: i64,
    state: State<'_, AppState>,
) -> Result<am_core::SnapshotResolve, String> {
    lock(&state).snapshot_resolve(snapshot)
}

#[tauri::command]
fn am_snapshot_remove(
    snapshot: i64,
    hashes: Vec<String>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    lock(&state).snapshot_remove(snapshot, &hashes)
}

/// 导出快照（json 清单）。返回最终路径，取消 = None
#[tauri::command]
async fn am_snapshot_export(
    state: State<'_, AppState>,
    snapshot: i64,
) -> Result<Option<String>, String> {
    let file = {
        let a = lock(&state);
        a.snapshot_export(snapshot)?
    };
    let stem = am_core::snapshot::safe_file_stem(&file.name);
    let picked = rfd::AsyncFileDialog::new()
        .set_title("导出快照（只含图片编号，不含原图）")
        .set_file_name(format!("{stem}.S-AssetManager"))
        .add_filter("AssetManager 快照", &["S-AssetManager"])
        .save_file()
        .await;
    let Some(h) = picked else { return Ok(None) };
    let path = h.path().to_path_buf();
    let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("写不了 {}：{e}", path.display()))?;
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("snapshot export id={snapshot} -> {}", path.display()));
    }
    Ok(Some(path.to_string_lossy().to_string()))
}

/// 导入快照。返回 `(id, 最终名字, 新增条数, 跳过的坏条目)`，取消 = None
#[tauri::command]
async fn am_snapshot_import(
    state: State<'_, AppState>,
) -> Result<Option<(i64, String, usize, usize)>, String> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("导入快照")
        /* 新后缀优先，老文件（.json）继续认：用户手里还留着一堆旧备份 */
        .add_filter("AssetManager 快照", &["S-AssetManager", "json"])
        .pick_file()
        .await;
    let Some(h) = picked else { return Ok(None) };
    let text = std::fs::read_to_string(h.path())
        .map_err(|e| format!("读不了 {}：{e}", h.path().display()))?;
    let mut a = lock(&state);
    let r = a.snapshot_import(&text)?;
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("snapshot import {} -> id={} 新增 {} 条", r.1, r.0, r.2));
    }
    Ok(Some(r))
}

/// 导出快照里的**原始图片**，打成 zip 到用户选的位置。
/// 只有这次真找到的图才进包；找不到的条数照实报出来（不假装成功）。
#[tauri::command]
async fn am_snapshot_export_zip(
    state: State<'_, AppState>,
    snapshot: i64,
) -> Result<Option<SnapshotZip>, String> {
    let (name, files, missing) = {
        let a = lock(&state);
        let row = a.snapshot_get(snapshot)?.ok_or_else(|| "这个快照已经不在了".to_string())?;
        let res = a.snapshot_resolve(snapshot)?;
        let mut files: Vec<PathBuf> = Vec::new();
        let mut missing = 0usize;
        for it in res.items {
            if it.gid.is_none() {
                missing += 1;
                continue;
            }
            let p = PathBuf::from(&it.path);
            if p.is_file() {
                files.push(p);
            } else {
                missing += 1;
            }
        }
        (row.name, files, missing)
    };
    if files.is_empty() {
        return Err("这个快照里一张能导出的图都没有（文件都不在原位）".into());
    }
    let stem = am_core::snapshot::safe_file_stem(&name);
    let picked = rfd::AsyncFileDialog::new()
        /* 0.4.12：快照的原始图片包**就是 .zip**（owner 2026-11-21 最终口径，
           0.4.11 改成 .P 又被否了）。散图给人解压，不是资产包；.P 留给设置里的「导出资产包」。 */
        .set_title("导出原始图片（zip）")
        .set_file_name(format!("{stem}.zip"))
        .add_filter("压缩包", &["zip"])
        .save_file()
        .await;
    let Some(h) = picked else { return Ok(None) };
    let target = h.path().to_path_buf();
    let t2 = target.clone();
    let (ok, bad) = tauri::async_runtime::spawn_blocking(move || {
        am_core::snapshot::zip_files(&t2, &files)
    })
    .await
    .map_err(|e| format!("打包线程挂了：{e}"))??;
    if let Some(d) = LOG_DIR.get() {
        log_runtime(
            d,
            &format!("snapshot zip id={snapshot} ok={ok} bad={} -> {}", bad.len(), target.display()),
        );
    }
    Ok(Some(SnapshotZip { path: target.to_string_lossy().to_string(), ok, bad, missing }))
}

/* ---------------- 双击导入（0.4.2） ----------------

   三条导出（`.D` 用户数据 / `.S` 快照清单 / `.P` 资源包）都注册了文件关联（`.L` 旧格式仍兼容）：
   资源管理器双击 → `AssetManager.exe --open "<文件>"` → 若已有实例在跑，路径经
   `single_instance` 转给首实例 → 首实例 emit `am://open-file` → 前端弹确认框 →
   调 `am_open_import_file`。**类型由内容嗅探决定**（`am_core::file_kind`），
   所以用户把后缀改坏了也认得出来。

   为什么分成四个命令：前端只需要认识 `am_open_import_file` 一个入口，
   另外三个是它的分派目标（CLI 与自测也直接调，省得每次都要弹对话框）。 */

/// 导入结果（前端拿它拼提示）。字段名转 camelCase 给 TS。
#[derive(Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportReport {
    /// `userdata` / `snapshot` / `package` —— 前端据此决定要不要提示"会解压"
    kind: String,
    /// 一句话结论（已经带数量，前端直接 toast）
    message: String,
    libs_added: usize,
    assets_updated: usize,
    assets_unknown: usize,
    /// 资源包：解出来几个文件
    files: usize,
    /// 快照：新增几条 / 跳过几条坏数据
    added: usize,
    skipped: usize,
    /// 快照：落库后的 id 与最终名字（重名会自动加 (2)）
    snapshot_id: i64,
    snapshot_name: String,
    /// 资产包清单回填：还原了几张的评分 / 标签
    rated: usize,
    tagged: usize,
}

/// 读一个文本类导出文件。错误说人话（"为什么 + 怎么办"）。
fn read_import_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| {
        format!(
            "读不了「{}」（{e}）。它可能被移动、删除了，或者正被别的程序占用。",
            path.display()
        )
    })
}

/// 导入用户数据（`.D`，旧 `.L` 同样认）：只合并不删 —— 回填评分 / 备注 / 来源 / 标签。
#[tauri::command]
fn am_import_userdata(app: AppHandle, path: String) -> Result<ImportReport, String> {
    let text = read_import_text(Path::new(&path))?;
    let st = app.state::<AppState>();
    let mut a = lock(&st);
    let r = a.import_userdata(&text)?;
    let mut parts: Vec<String> = Vec::new();
    if r.libs_added > 0 {
        parts.push(format!("新增 {} 个库", r.libs_added));
    }
    parts.push(format!("回填 {} 张素材", r.assets_updated));
    if r.assets_unknown > 0 {
        parts.push(format!("{} 条在索引里找不到", r.assets_unknown));
    }
    Ok(ImportReport {
        kind: "userdata".into(),
        message: format!("已导入用户数据：{}", parts.join("，")),
        libs_added: r.libs_added,
        assets_updated: r.assets_updated,
        assets_unknown: r.assets_unknown,
        ..Default::default()
    })
}

/// 导入快照清单（`.S`）：落在**当前库**，重名自动加 `(2)`。
#[tauri::command]
fn am_snapshot_import_path(app: AppHandle, path: String) -> Result<ImportReport, String> {
    let text = read_import_text(Path::new(&path))?;
    let st = app.state::<AppState>();
    let mut a = lock(&st);
    let (id, name, added, skipped) = a.snapshot_import(&text)?;
    Ok(ImportReport {
        kind: "snapshot".into(),
        message: format!("已导入快照「{name}」：{added} 张"),
        added,
        skipped,
        snapshot_id: id,
        snapshot_name: name,
        ..Default::default()
    })
}

/// 导入资产包（`.P`）：**先问用户解压到哪**，再解包 + 扫一遍那个目录入库 + 按清单还原。
///
/// 为什么一定要问：包里的文件名是导出时的名字，直接铺到库根会跟现有文件混在一起，
/// 用户事后根本分不清哪些是这次导进来的。让它落在一个空目录里，进不进库由用户决定。
///
/// 没有清单的包（比如用户把旧的"快照原始图片 .zip"改名过来）也能导：纯解压 + 扫描。
#[tauri::command]
async fn am_import_package(app: AppHandle, path: String) -> Result<Option<ImportReport>, String> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("把资产包解压到这个文件夹（建议新建一个空文件夹）")
        .pick_folder()
        .await;
    let Some(dir) = picked else { return Ok(None) };
    let dest = dir.path().to_path_buf();
    let src = PathBuf::from(&path);
    let want = dest.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<ImportReport, String> {
        /* 清单读进内存，**不落盘** —— 落下去会被当成一张素材入索引 */
        let (files, manifest) = am_core::snapshot::unzip_package(&src, &want)?;
        if files == 0 && manifest.is_none() {
            return Err("这个包里一个文件都没有，没什么可导入的。".into());
        }
        /* 解到哪儿就扫到哪儿：不自动设成"当前库"，用户想加库自己加。
           **路径先统一成 Windows 口径**：索引里的 `dirs.path` 是原样存的，
           这里要是写成 `C:/x/y`，待会儿按清单回填时用 `C:\x\y` 就一行也查不到。 */
        let dest_str = want.to_string_lossy().replace('/', "\\");
        let st = app.state::<AppState>();
        let mut a = lock(&st);
        let stats = a.scan(&dest_str, |_| {})?;
        let restore = match manifest.as_deref() {
            Some(m) => a.import_package_manifest(m, &want)?,
            None => am_core::PackageRestore::default(),
        };
        let mut msg = format!(
            "已解压 {} 个文件到「{}」，{} 张图片已入库",
            files,
            want.display(),
            stats.images
        );
        if restore.rated > 0 || restore.tagged > 0 {
            msg.push_str(&format!(
                "；还原评分 {} 张、标签 {} 张",
                restore.rated, restore.tagged
            ));
        }
        Ok(ImportReport {
            kind: "package".into(),
            message: msg,
            files,
            added: stats.images.max(0) as usize,
            rated: restore.rated,
            tagged: restore.tagged,
            ..Default::default()
        })
    })
    .await
    .map_err(|e| format!("解包线程没能跑完（{e}）。请重试一次。"))??;
    Ok(Some(r))
}

/// 双击导入的统一入口：**先嗅探内容**，再分派到上面三条之一。
#[tauri::command]
async fn am_open_import_file(app: AppHandle, path: String) -> Result<Option<ImportReport>, String> {
    let p = PathBuf::from(&path);
    let kind = am_core::file_kind::sniff(&p)?;
    match kind {
        am_core::file_kind::ImportKind::UserData => am_import_userdata(app, path).map(Some),
        am_core::file_kind::ImportKind::Snapshot => am_snapshot_import_path(app, path).map(Some),
        am_core::file_kind::ImportKind::Package => am_import_package(app, path).await,
    }
}

/* ---------------- 界面里的「导入文件到此处」（0.4.2） ----------------

   与双击导入的区别：**不认导出文件**，认的是用户随手拿来的任意素材（png/jpg/psd/txt…），
   语义是"复制一份进当前这个文件夹"。所以它不嗅探、不弹确认框，只有一条路：选文件 → 复制 → 入库。

   三个入口（侧栏右键 / 空文件夹 / 命令面板）最后都落到 `am_pick_and_import`。 */

/// 把一批任意格式的文件**复制**进目标文件夹并入库。
///
/// `target` 由前端给（当前打开的文件夹），但**后端必须再校验一次**：
/// 界面可能拿着上一次的旧状态（刚删库、刚移库），复制到库外等于往用户磁盘上乱写。
#[tauri::command]
fn am_import_files(
    state: State<AppState>,
    paths: Vec<String>,
    target: String,
) -> Result<am_core::ImportFilesReport, String> {
    let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let mut a = lock(&state);
    a.import_files(&list, Path::new(&target))
}

/// 选文件（**不加扩展名过滤器**：任何格式都能导入）→ 走同一条 `import_files`。
#[tauri::command]
async fn am_pick_and_import(
    state: State<'_, AppState>,
    target: String,
) -> Result<Option<am_core::ImportFilesReport>, String> {
    /* 先校验目标再弹框：让用户挑完一堆文件才告诉他"这个文件夹不在库里"最气人 */
    {
        let a = lock(&state);
        let _ = a.conn_for_path(&target)?;
        if !Path::new(&target).is_dir() {
            return Err(format!("目标文件夹不存在：{target}"));
        }
    }
    let picked = rfd::AsyncFileDialog::new()
        .set_title("导入文件到当前文件夹（任意格式）")
        .pick_files()
        .await;
    let Some(handles) = picked else { return Ok(None) };
    let list: Vec<PathBuf> = handles.iter().map(|h| h.path().to_path_buf()).collect();
    if list.is_empty() {
        return Ok(None);
    }
    let mut a = lock(&state);
    a.import_files(&list, Path::new(&target)).map(Some)
}

/// 资产包导出的返回（前端拼 toast 用）
#[derive(Clone, Default, serde::Serialize)]
struct PackageOut {
    path: String,
    ok: usize,
    skipped: usize,
}

/// 导出资产包（`.P-AssetManager`）：**整个当前素材库**（不是当前文件夹、不是选中项）。
///
/// `path` = 前端给的"当前打开的目录"，用它定位是哪个库；传 None 就退到当前库 / 第一个库。
#[tauri::command]
async fn am_export_package(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<Option<PackageOut>, String> {
    /* ① 先把库根定下来（锁用完就放，绝不能带进 await） */
    let root = {
        let a = lock(&state);
        let by_path = path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .and_then(|p| a.lib_for_path(p))
            .and_then(|id| a.libs.iter().find(|l| l.id == id).map(|l| l.root.clone()));
        let cur = a
            .current
            .and_then(|id| a.libs.iter().find(|l| l.id == id))
            .map(|l| l.root.clone());
        by_path
            .or(cur)
            .or_else(|| a.libs.first().map(|l| l.root.clone()))
            .ok_or_else(|| "还没有素材库：先在设置里加一个文件夹".to_string())?
    };
    let lib_name = Path::new(&root)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "素材库".into());
    let picked = rfd::AsyncFileDialog::new()
        .set_title("导出资产包（整个素材库）")
        .set_file_name(format!(
            "{}.P-AssetManager",
            am_core::snapshot::safe_file_stem(&lib_name)
        ))
        .add_filter("AssetManager 资产包", &["P-AssetManager"])
        .save_file()
        .await;
    let Some(h) = picked else { return Ok(None) };
    let target = h.path().to_path_buf();
    let t2 = target.clone();
    let (ok, skipped) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(usize, usize), String> {
            let st = app.state::<AppState>();
            let a = lock(&st);
            a.export_package(&root, &t2)
        })
        .await
        .map_err(|e| format!("打包线程没能跑完（{e}）。请重试一次。"))??;
    if let Some(d) = LOG_DIR.get() {
        log_runtime(
            d,
            &format!(
                "export package ok={ok} skipped={skipped} -> {}",
                target.display()
            ),
        );
    }
    Ok(Some(PackageOut {
        path: target.to_string_lossy().to_string(),
        ok,
        skipped,
    }))
}

/// 单包上限：**3.6 GB**，不是 4 GB。
///
/// zip32 的偏移量是 u32（4 GiB），贴着上限装的话，最后一条加上中心目录就顶出去了 ——
/// 而 `ZipWriter` 越界是**报错**（不会写坏包），所以宁可早一点换包。
/// 条目数同理：6.5 万是硬上限，留 5 千给"每片都带的那份清单"。
const LIB_ZIP_MAX_BYTES: u64 = 3_600_000_000;
const LIB_ZIP_MAX_ENTRIES: u64 = 60_000;

/// 整库导出成**普通 zip**：原图（保留文件夹层级）+ `_AssetManager/` 里的库数据。
///
/// 和 `am_export_package`（`.P-AssetManager`）的区别：那个是给本软件双击导回的，
/// 这个是**给人用的** —— 换机备份、发给同事，任何解压软件都能打开，文件名带年月日。
/// 超过 4 GB / 6.5 万条自动分片（`part1`、`part2`…），单片时不带 `-part1` 尾巴。
#[tauri::command]
async fn am_export_library_zip(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<Option<am_core::LibraryZip>, String> {
    /* ① 先定库根 + 这个库自己的索引文件（锁用完就放，绝不带进 await） */
    let (root, index_db) = {
        let a = lock(&state);
        let by_path = path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .and_then(|p| a.lib_for_path(p));
        let id = by_path
            .or(a.current)
            .or_else(|| a.libs.first().map(|l| l.id))
            .ok_or_else(|| "还没有素材库：先在设置里加一个文件夹".to_string())?;
        let l = a
            .libs
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| "这个素材库已经不在了".to_string())?;
        (l.root.clone(), am_core::reg::lib_file(&a.data_dir, id))
    };
    let lib_name = Path::new(&root)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "素材库".into());
    let stem = format!(
        "{}-{}",
        am_core::snapshot::safe_file_stem(&lib_name),
        local_ymd()
    );

    /* ② 选落地的文件夹。分片数量要到装填时才知道，所以让用户选目录而不是选文件名 */
    let picked = rfd::AsyncFileDialog::new()
        .set_title("导出整个库（ZIP）：选一个文件夹放导出的包")
        .pick_folder()
        .await;
    let Some(dir) = picked else { return Ok(None) };
    let out_dir = dir.path().to_path_buf();

    /* ③ 重活搬出 async：整库读盘几分钟，不能占着运行时和锁 */
    let out = tauri::async_runtime::spawn_blocking(move || -> Result<am_core::LibraryZip, String> {
        let st = app.state::<AppState>();
        let a = lock(&st);
        a.export_library_zip(
            &root,
            &out_dir,
            &stem,
            LIB_ZIP_MAX_BYTES,
            LIB_ZIP_MAX_ENTRIES,
            Some(&index_db),
        )
    })
    .await
    .map_err(|e| format!("打包线程没能跑完（{e}）。请重试一次。"))??;

    if let Some(d) = LOG_DIR.get() {
        let names: Vec<&str> = out.parts.iter().map(|p| p.name.as_str()).collect();
        log_runtime(
            d,
            &format!(
                "export library zip files={} parts={} skipped={} oversized={} -> {}",
                out.files,
                out.parts.len(),
                out.skipped,
                out.oversized,
                names.join(",")
            ),
        );
    }
    Ok(Some(out))
}

/// 快照的灵感板：先把条目对成现在的素材，再把它们铺到无限画布上。
/// 找不到的条目**不静默丢** —— 窗口标题里写明「N 张这次没找到」。
#[tauri::command]
async fn am_snapshot_open(
    app: AppHandle,
    state: State<'_, AppState>,
    snapshot: i64,
) -> Result<usize, String> {
    let (row, res) = {
        let a = lock(&state);
        (
            a.snapshot_get(snapshot)?,
            a.snapshot_resolve(snapshot)?,
        )
    };
    let row = row.ok_or_else(|| "这个快照已经不在了".to_string())?;
    let mut items: Vec<BoardItem> = Vec::with_capacity(res.items.len());
    for it in &res.items {
        let Some(g) = it.gid else { continue };
        items.push(board_item(
            g,
            it.mtime,
            it.w.unwrap_or(0).max(0) as u32,
            it.h.unwrap_or(0).max(0) as u32,
            it.name.clone(),
            it.path.clone(),
        ));
    }
    let shown = items.len();
    let title = format!("灵感板 · 快照 {}", row.name);
    let boot = serde_json::json!({
        /* 摆位的键：与 spawn_board 的 key 同源，改一处必须改两处 */
        "board": format!("snap:{snapshot}"),
        "snapshot": { "id": snapshot, "name": row.name },
        "title": title,
        "items": items,
        "more": false,
        "total": res.items.len(),
        "missing": res.missing,
    });
    spawn_board(&app, &format!("snap:{snapshot}"), &title, boot)?;
    log_runtime_opt(&format!("snapshot board id={snapshot} items={shown} missing={}", res.missing));
    Ok(shown)
}

/// 快照板里的 Ctrl+V：剪贴板图片存进截图目录、入库、**加进这个快照**，再贴到板上。
#[tauri::command]
async fn am_snapshot_paste(
    app: AppHandle,
    state: State<'_, AppState>,
    snapshot: i64,
) -> Result<BoardItem, String> {
    paste_into(&app, &state, PasteSink::Snapshot(snapshot)).await
}

/// 分组板里的 Ctrl+V：同样存盘入库，然后**加进这个分组**（只建关联，文件不动）。
#[tauri::command]
async fn am_coll_paste(
    app: AppHandle,
    state: State<'_, AppState>,
    coll: i64,
) -> Result<BoardItem, String> {
    paste_into(&app, &state, PasteSink::Coll(coll)).await
}

/// 记一行运行时日志（没有日志目录时什么都不做）
fn log_runtime_opt(msg: &str) {
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, msg);
    }
}

/* ---------------- 复制 / 另存 ---------------- */

/// 复制**原文件**（CF_HDROP：粘到 PS / 资源管理器是文件，不是位图）
/// 贴图右键「以原始尺寸存到截图库」：把这张图**原样**复制进
/// `screenshots`（与 assetmanager.exe 同级，也就是软件自己的截图库），
/// **不缩放、不重编码**——owner 要的是原始尺寸的原件，重新编码会掉画质。
/// 重名就加 ` (2)`，和资源管理器一个习惯。
/// 主窗把「设置 → 外观 → 主题」推过来。贴图窗口建窗时读它，
/// 这样浅色主题下贴图的右键菜单才是浅色的（owner 2026-11-18 第 6 条）。
#[tauri::command]
fn am_ui_theme(theme: String) {
    let v = if theme == "light" { 1 } else { 2 };
    UI_THEME.store(v, std::sync::atomic::Ordering::SeqCst);
}

/// 贴图「蓝色边框」开关：写全局 + 广播给**所有**贴图窗口（包括发起调用的这一张）。
///
/// 为什么要广播而不是只改自己：每张贴图是独立 webview，各改各的话，
/// 用户关掉手上这张，屏幕上其他张还亮着蓝边 —— 看到的就是"开关坏了"。
#[tauri::command]
fn am_peek_set_frame(app: tauri::AppHandle, on: bool) {
    PEEK_FRAME.store(on, std::sync::atomic::Ordering::SeqCst);
    let _ = app.emit("peek:frame", on);
}

#[tauri::command]
fn am_peek_save_lib(path: String) -> Result<String, String> {
    let from = PathBuf::from(&path);
    if !from.is_file() {
        return Err("原文件已经不在了".into());
    }
    let dir = shot_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("建不了截图目录 {}：{e}", dir.display()))?;
    let stem = from
        .file_stem()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| "图片".into());
    let ext = from
        .extension()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| "png".into());
    let mut target = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    std::fs::copy(&from, &target).map_err(|e| format!("写入截图库失败：{e}"))?;
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("peek save-lib {path} -> {}", target.display()));
    }
    Ok(target.to_string_lossy().to_string())
}

#[tauri::command]
fn am_peek_copy_file(paths: Vec<String>) -> Result<(), String> {
    let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    fsops::copy_files_to_clipboard(&list)
}

#[tauri::command]
fn am_clip_text(text: String) -> Result<(), String> {
    clipimg::set_text(&text)
}

/// 另存为：把 `src` 拷到用户选的位置。返回最终路径（取消 = None）
///
/// **弹框前必须先取消本窗置顶**：无边框看图默认置顶，文件对话框没有 owner，
/// 会被稳稳压在下面 —— 用户看到的就是"按了 Ctrl+S 什么都没发生"
/// （owner 2026-11-17 反馈）。存完再恢复置顶。
#[tauri::command]
async fn am_save_as(
    win: Window,
    src: String,
    suggest: Option<String>,
) -> Result<Option<String>, String> {
    let from = PathBuf::from(&src);
    if !from.is_file() {
        return Err("原文件已经不在了".into());
    }
    let name = suggest
        .filter(|s| !s.trim().is_empty())
        .or_else(|| from.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "image.png".to_string());
    let was_top = win.is_always_on_top().unwrap_or(false);
    if was_top {
        let _ = win.set_always_on_top(false);
    }
    let picked = rfd::AsyncFileDialog::new()
        .set_file_name(&name)
        .set_directory(from.parent().unwrap_or(Path::new(".")))
        .save_file()
        .await;
    if was_top {
        let _ = win.set_always_on_top(true);
    }
    let Some(handle) = picked else { return Ok(None) };
    let to = handle.path().to_path_buf();
    if to != from {
        std::fs::copy(&from, &to).map_err(|e| format!("写入失败：{e}"))?;
    }
    Ok(Some(to.to_string_lossy().to_string()))
}

/* ---------------- 剪贴板图片（F3，带 10 张历史） ----------------

   历史**不再**是"剪贴板缓存里最近 10 个 PNG"，而是 `peek_history()`：
   截图目录 + 软件回收站里最新的 10 张图。理由见 `peek_history` 的注释。 */

/// 从 AppHandle 取数据目录（贴图落盘 / 回收站都要用）。**不持锁外传**：

/// 从 AppHandle 取数据目录（贴图落盘 / 回收站都要用）。**不持锁外传**：
/// 调用方拿到的是一个 PathBuf，锁在花括号里就释放了。
fn data_dir_of(app: &AppHandle) -> PathBuf {
    use tauri::Manager;
    let st = app.state::<AppState>();
    let g = lock(&st);
    g.data_dir.clone()
}

/* ---------------- 界面偏好：缩放 / 关闭到托盘 / 启动时最小化（0.3.30） ----------------
   这几个键为什么要在后端单独存一份：它们必须**在窗口显示之前**就被后端读到
   （「启动时最小化」要决定建不建可见窗口，「关闭到托盘」要在 CloseRequested 里拦），
   而前端 prefs 存在 webview 的 localStorage 里 —— 那时候 webview 还没跑起来。
   所以这里存一份"后端看得见的副本"，前端每次改动都同步过来（`am_ui_prefs_set`）。
   其余偏好仍只在前端，不重复存（一份数据一个地方，避免两边打架）。 */

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct UiPrefs {
    /// 关闭窗口 = 收进托盘（不退出程序）
    #[serde(default, rename = "closeToTray")]
    close_to_tray: bool,
    /// 启动后直接收进托盘（不弹主窗口）
    #[serde(default, rename = "startMinimized")]
    start_minimized: bool,
    /// 界面缩放：`None` = 自动（按工作区尺寸挑一档）
    #[serde(default, rename = "uiScale")]
    ui_scale: Option<f64>,
}

fn ui_prefs_path(app: &AppHandle) -> PathBuf {
    data_dir_of(app).join("ui-prefs.json")
}

fn read_ui_prefs(app: &AppHandle) -> UiPrefs {
    std::fs::read_to_string(ui_prefs_path(app))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn am_ui_prefs_get(app: AppHandle) -> UiPrefs {
    read_ui_prefs(&app)
}

#[tauri::command]
fn am_ui_prefs_set(app: AppHandle, prefs: UiPrefs) -> Result<(), String> {
    let p = ui_prefs_path(&app);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = serde_json::to_string_pretty(&prefs).map_err(|e| e.to_string())?;
    std::fs::write(&p, body).map_err(|e| format!("写界面偏好失败：{e}"))
}

/* 界面缩放的自动档：工作区 / 设计尺寸，取不超过它的最大档。
   和 FontManager 同一套档位（0.8 / 0.9 / 1.0），再往上 1.1 / 1.25 由用户手选 ——
   自动只是"这台机器不挤"，不代表用户不想放大。 */
const DESIGN_W: f64 = 1440.0;
const DESIGN_H: f64 = 900.0;
const SCALE_STEPS: [f64; 3] = [0.8, 0.9, 1.0];

fn auto_ui_scale(avail_w: f64, avail_h: f64) -> f64 {
    let raw = (avail_w / DESIGN_W).min(avail_h / DESIGN_H);
    let mut chosen = SCALE_STEPS[0];
    for step in SCALE_STEPS {
        if step <= raw + 0.0001 {
            chosen = step;
        }
    }
    chosen
}

/// 设界面缩放。`scale = None` → 按当前显示器算自动档，并把**算出来的值**回给前端
/// （前端要显示"当前 100%"这种文案，不能自己再算一遍，否则和后端不一致）。
#[tauri::command]
fn am_set_ui_scale(window: tauri::WebviewWindow, scale: Option<f64>) -> Result<f64, String> {
    let resolved = match scale {
        Some(s) => s.clamp(0.5, 2.0),
        None => {
            let m = window
                .current_monitor()
                .map_err(|e| e.to_string())?
                .ok_or("拿不到显示器信息")?;
            let dpi = m.scale_factor().max(0.1);
            let a = m.work_area();
            auto_ui_scale(a.size.width as f64 / dpi, a.size.height as f64 / dpi)
        }
    };
    window.set_zoom(resolved).map_err(|e| e.to_string())?;
    Ok(resolved)
}

/* ---------------- 托盘（0.3.30） ---------------- */

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 建托盘。失败只记日志、不影响启动 —— 托盘是个便利设施，
/// 它挂了也不该让软件打不开（「关闭到托盘」那边会检查 `tray_ok`）。
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let show = MenuItem::with_id(app, "show", "打开 AssetManager", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::InvalidIcon(std::io::Error::other("没有内置窗口图标")))?;

    let builder = TrayIconBuilder::with_id("am-tray")
        .icon(icon)
        .tooltip("AssetManager")
        .menu(&menu);

    /* Windows 上左键直接开主窗口（菜单只走右键），符合资源管理器托盘图标的习惯；
       其他平台保留左键弹菜单。 */
    let builder = if cfg!(target_os = "windows") {
        builder.show_menu_on_left_click(false)
    } else {
        builder
    };

    builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/* ---------------- 开机自动启动（0.3.30，官方插件写 HKCU Run） ---------------- */

#[tauri::command]
fn am_set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| e.to_string())
    } else {
        mgr.disable().map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn am_get_autostart(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// 把一个刚写出来的 PNG 收进**软件回收站**，返回它现在的位置。
///
/// 为什么默认进回收站（owner 2026-11-19）：贴图是"临时摆到屏幕上"的动作，
/// 用户并不一定想留底；但完全不存又没法"再贴一次"。放回收站正好——
/// 不占截图目录、回收站里看得见、想留就还原（还能还原到指定目录）。
/// 收不进去（磁盘/权限问题）就留在原处：**图不能丢**。
fn trash_png(app: &AppHandle, path: &Path) -> PathBuf {
    let data = data_dir_of(app);
    match am_core::trash::put(&data, &[path.to_path_buf()]) {
        Ok(v) if !v.is_empty() => PathBuf::from(&v[0].trashed_path),
        Ok(_) => path.to_path_buf(),
        Err(e) => {
            if let Some(d) = LOG_DIR.get() {
                log_runtime(d, &format!("trash put FAIL {}：{e}", path.display()));
            }
            path.to_path_buf()
        }
    }
}

/// 能拿来"再贴一次"的历史：**截图目录 + 软件回收站**里的图片，最新在前，最多 10 张。
///
/// owner 2026-11-19 明确范围：只认这两个地方。以前是 `clipboard-cache` 里按 10 张
/// 轮转，和用户的"我截的那张图"完全脱节 —— 他在回收站里看得见，F3 却翻不到。
///
/// **按（文件名 + 字节数）去重**：贴图窗口右键「以原始尺寸存到截图库」是**复制**，
/// 同一张图会同时存在于 screenshots 和回收站里，不去重的话连按 F3 会看到同一张图两次。
fn peek_history(app: &AppHandle) -> Vec<PathBuf> {
    const IMG: [&str; 8] = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "jfif", "jpe"];
    let is_img = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| IMG.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false)
    };
    /* (时间, 路径, 文件名, 字节数) —— 后两项只用来去重 */
    let mut out: Vec<(std::time::SystemTime, PathBuf, String, u64)> = Vec::new();
    let name_of = |p: &Path| p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    // ① 安装目录下的 screenshots
    if let Ok(rd) = std::fs::read_dir(shot_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if !is_img(&p) {
                continue;
            }
            if let Ok(m) = e.metadata() {
                if let Ok(t) = m.modified() {
                    out.push((t, p.clone(), name_of(&p), m.len()));
                }
            }
        }
    }
    // ② 软件回收站（贴图默认就落这儿）
    let data = data_dir_of(app);
    for t in am_core::trash::list(&data) {
        let p = PathBuf::from(&t.trashed_path);
        if !is_img(&p) {
            continue;
        }
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(t.deleted_at);
        out.push((when, p.clone(), t.name.clone(), t.size));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    out.into_iter()
        .filter(|(_, _, n, sz)| seen.insert((n.clone(), *sz)))
        .map(|(_, p, _, _)| p)
        .take(CLIP_HISTORY)
        .collect()
}

/// 把一张磁盘上的图贴到屏幕上（历史一起带过去，窗内 ←/→ 能翻）。
async fn peek_from_file(app: &AppHandle, path: PathBuf) -> Result<(), String> {
    let mut items: Vec<PeekItem> = Vec::new();
    let mut idx = 0usize;
    let want = path.to_string_lossy().to_string();
    for p in peek_history(app) {
        if let Some(it) = item_of_file(&p) {
            if it.path == want {
                idx = items.len();
            }
            items.push(it);
        }
    }
    if items.is_empty() {
        return Err("这张图已经不在了".into());
    }
    let rec = PeekRecall {
        label: next_label(),
        kind: "file".into(),
        path: items[idx].path.clone(),
        long_edge: PEEK_LONG_EDGE,
        ..Default::default()
    };
    peeks().lock().unwrap_or_else(|e| e.into_inner()).push(rec.clone());
    save_peeks();
    spawn_peek(app, items[idx].clone(), rec, false).await
}

/* 剪贴板连按翻历史：`CLIP_SIG` 记住上一次贴的剪贴板内容指纹，`CLIP_CURSOR` 是翻到第几张。
   指纹一样 = 用户没换剪贴板、只是想再贴一张 → 往后翻；不一样 = 新图，存下来贴它。 */
static CLIP_SIG: AtomicU64 = AtomicU64::new(0);
static CLIP_CURSOR: AtomicUsize = AtomicUsize::new(0);

fn sig_of(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 连着按 F3 时"往后翻一张"。
///
/// `skip_newest`：刚从剪贴板贴出来那一张就是历史里的第 0 条（它现在在屏幕上），
/// 再翻就不该把它又贴一遍 —— 那是"没反应"。剪贴板里没有图时没有这张，从最新开始翻。
async fn paste_from_history(app: &AppHandle, skip_newest: bool) -> Result<(), String> {
    let hist = peek_history(app);
    if hist.is_empty() {
        return Err("还没有可以再贴的图片（截图目录与回收站都是空的）".into());
    }
    let step = CLIP_CURSOR.fetch_add(1, Ordering::SeqCst);
    let mut i = step % hist.len();
    if skip_newest {
        i = (i + 1) % hist.len();
    }
    log_runtime_opt(&format!(
        "clip peek history {}/{} {}",
        i + 1,
        hist.len(),
        hist[i].display()
    ));
    peek_from_file(app, hist[i].clone()).await
}

async fn clip_peek_begin(app: AppHandle) -> Result<(), String> {
    let png = match clipimg::get_png() {
        Ok((_w, _h, png)) => png,
        Err(e) => {
            /* 剪贴板里不是图（文字 / 文件 / 空）：**仍然翻历史**。
               owner 要的是"连按 F3 依次把历史贴出来"，不该因为剪贴板恰好是文字就整条路死掉。 */
            log_runtime_opt(&format!("clip peek：剪贴板无图（{e}），改为翻历史"));
            return paste_from_history(&app, false).await;
        }
    };
    let sig = sig_of(&png);
    let prev = CLIP_SIG.swap(sig, Ordering::SeqCst);

    /* 连按：剪贴板内容没变 = 用户想再看一张 → 沿历史往后翻。
       owner 2026-11-19 要的"多次点击可以把历史图片贴出来"
       （以前每按一次都是把**同一张**剪贴板图再贴一遍，等于没反应）。 */
    if prev == sig && sig != 0 {
        return paste_from_history(&app, true).await;
    }

    /* 新图：落盘到截图目录 → 收进软件回收站 → 贴。
       回收站是"临时区"：不占截图目录、看得见、想留就还原（可还原到指定目录）。 */
    CLIP_CURSOR.store(0, Ordering::SeqCst);
    let dir = shot_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("建不了截图目录：{e}"))?;
    let (stamp, _) = screen::local_stamp();
    let path = dir.join(format!("剪贴板 {}.png", stamp.replace(':', "-")));
    std::fs::write(&path, &png).map_err(|e| format!("写剪贴板图片失败：{e}"))?;
    let trashed = trash_png(&app, &path);
    peek_from_file(&app, trashed).await
}

/// F3：把剪贴板里的图片贴到屏幕上（并进历史）
#[tauri::command]
async fn am_clip_peek(app: AppHandle) -> Result<(), String> {
    clip_peek_begin(app).await
}

/// 剪贴板历史：`(路径, 宽, 高)`，最新在前
#[tauri::command]
fn am_clip_history(app: AppHandle) -> Result<Vec<(String, u32, u32)>, String> {
    Ok(peek_history(&app)
        .into_iter()
        .map(|p| {
            let (w, h) = screen::dims(&p);
            (p.to_string_lossy().to_string(), w, h)
        })
        .collect())
}

/* ---------------- 截图（F1 / Alt+F1） ----------------

   两段式：`am_snip_crop` 把选区从冻住的整屏里裁出来（base64 回前端），
   前端在 canvas 上画标注，再把**合成好的整张**交给 `am_snip_finish` 保存 / 复制 / 贴图。

   为什么裁剪和落盘要拆开：标注（箭头 / 文字 / 马赛克）在 canvas 上做最省事也最准，
   但 canvas 只能用同源图片当底 —— 所以底图走 base64（`data:` URL 不污染画布），
   而不是 `peekfile://`。拆开之后前端也不必为了"先画一笔"就跑一次落盘。 */

struct SnipState {
    shot: screen::Shot,
    /// 截图临时文件（裁剪完就没人要了，取消/完成时删掉）
    tmp: PathBuf,
}

static SNIP: OnceLock<Mutex<Option<SnipState>>> = OnceLock::new();

fn snip_slot() -> &'static Mutex<Option<SnipState>> {
    SNIP.get_or_init(|| Mutex::new(None))
}

/* 截图**不再隐藏贴图**（owner 2026-11-19：「截图的时候原本贴图的不该消失」）。
   0.10.x ~ 0.3.27 的做法是 hide → 截冻结图 → 截完 show：用户在按下 F1 的瞬间看到
   贴图闪一下不见、画面塌一块。现在贴图全程不透明度 100% 留在屏幕上，
   覆盖层显示的就是**含贴图**的那一帧 —— 代价是框选范围里有贴图时会被截进去，
   这是 owner 明确要的取舍（他要"看到什么截什么"）。
   所以这里不再有 hide_peeks / show_peeks，也没有 `SnipState.hidden` 要恢复。 */

/* ---------------- 原生底图层（splash） ----------------

   **为什么需要这一层**：WebView2 只在窗口被真正合成到屏幕时才开始出帧，窗口显示到
   第一帧之间有 200~400ms 是 WebView 的深色默认底 —— 用户看到就是"截图开始黑一下"。
   Snipaste / ShareX / 微信截图没有这个问题，因为它们整块底图是 **GDI 直接画**的，
   第一帧就是截图本身。

   所以这里照它们做：截屏后立刻用一层 `WS_EX_LAYERED` 的原生窗口把整张图
   `UpdateLayeredWindow` 上去 —— 零延迟、不抢焦点、不要消息循环。WebView 覆盖层随后
   叠上来画压暗 / 选区 / 工具栏，工具栏就绪后把这层销毁。
   用户看到的是"按下 F1 屏幕立刻冻住"，没有任何黑屏。 */

#[cfg(windows)]
mod splash {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    type Hwnd = isize;
    type Hdc = isize;
    type Hgdiobj = isize;

    const WS_POPUP: u32 = 0x8000_0000;
    const WS_EX_LAYERED: u32 = 0x0008_0000;
    const WS_EX_TOPMOST: u32 = 0x0000_0008;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const ULW_ALPHA: u32 = 0x0000_0002;
    const SW_SHOW: i32 = 5;
    const AC_SRC_OVER: u8 = 0x00;
    const AC_SRC_ALPHA: u8 = 0x01;

    /* ---------------- 交互层（0.2.4「C 方案」） ----------------

       背景：WebView2 新建窗口要 ~600ms 才交出第一帧，这段时间谁都没法让它出图
       （alpha 隐身、disable-gpu、关遮挡计算全试过，无效）。所以**选区这一步不再等网页**：
       冻结图铺上去之后，鼠标拖框、压暗、边框、角点全部由这块原生 GDI 窗口画完，
       用户按下 F1 的下一帧就能框选。等网页真的画好了，再把当前选区交给它接手
       （标注、放大镜、工具栏那些仍然是网页那套，一个功能都没少）。 */

    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_RBUTTONDOWN: u32 = 0x0204;
    const WM_KEYDOWN: u32 = 0x0100;

    struct Sel {
        /// 冻结后的整屏，**BGRA**（和 DIB 同格式，重绘时直接 memcpy）
        base: Vec<u8>,
        /// 整屏压暗 55% 之后的版本（开机算一次，拖框时只做 memcpy）
        dim: Vec<u8>,
        w: i32,
        h: i32,
        mem: Hdc,
        bmp: Hgdiobj,
        bits: *mut u8,
        /// 选区（物理像素，窗口客户区坐标）
        rect: Option<[i32; 4]>,
        start: (i32, i32),
        dragging: bool,
        /// 用户按了 Esc / 右键
        cancel: bool,
        /// 用户松手了（选区已定）
        done: bool,
    }
    unsafe impl Send for Sel {}

    static SEL: std::sync::Mutex<Option<Sel>> = std::sync::Mutex::new(None);

    fn lo(v: isize) -> i32 {
        (v & 0xFFFF) as i16 as i32
    }
    fn hi(v: isize) -> i32 {
        ((v >> 16) & 0xFFFF) as i16 as i32
    }

    /// 把"起点 + 当前点"整理成 x,y,w,h（支持反向拖）
    fn norm(a: (i32, i32), b: (i32, i32)) -> [i32; 4] {
        let x = a.0.min(b.0);
        let y = a.1.min(b.1);
        let w = (a.0 - b.0).abs();
        let h = (a.1 - b.1).abs();
        [x, y, w, h]
    }

    impl Sel {
        /// 重绘：整屏 memcpy（~4ms），选区内的行从原图取、其余从压暗版取。
        /// 这样拖框时每帧只是搬内存，不会因为"逐像素压暗"掉帧。
        fn redraw(&mut self) {
            let n = (self.w as usize) * (self.h as usize) * 4;
            if n == 0 || self.bits.is_null() {
                return;
            }
            let r = self.rect;
            match r {
                Some([rx, ry, rw, rh]) if rw > 0 && rh > 0 => {
                    let row = self.w as usize * 4;
                    for y in 0..self.h {
                        let inside = y >= ry && y < ry + rh;
                        let src = if inside { &self.base } else { &self.dim };
                        let off = y as usize * row;
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                src.as_ptr().add(off),
                                self.bits.add(off),
                                row,
                            );
                        }
                        if inside {
                            let l = rx.max(0) as usize * 4;
                            let rr = ((rx + rw).min(self.w) as usize) * 4;
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    self.dim.as_ptr().add(off),
                                    self.bits.add(off),
                                    l,
                                );
                                if rr < row {
                                    std::ptr::copy_nonoverlapping(
                                        self.dim.as_ptr().add(off + rr),
                                        self.bits.add(off + rr),
                                        row - rr,
                                    );
                                }
                            }
                        }
                    }
                    self.stroke_rect(rx, ry, rw, rh);
                    self.handles(rx, ry, rw, rh);
                }
                _ => unsafe {
                    std::ptr::copy_nonoverlapping(self.dim.as_ptr(), self.bits, n);
                },
            }
            self.present();
        }

        /// 整屏压暗 55%（只在建窗时算一次）
        fn make_dim(base: &[u8]) -> Vec<u8> {
            let mut v = base.to_vec();
            for p in v.chunks_exact_mut(4) {
                p[0] = ((p[0] as u32 * 45) / 100) as u8;
                p[1] = ((p[1] as u32 * 45) / 100) as u8;
                p[2] = ((p[2] as u32 * 45) / 100) as u8;
            }
            v
        }

        fn px(&mut self, x: i32, y: i32) -> Option<*mut u8> {
            if x < 0 || y < 0 || x >= self.w || y >= self.h {
                return None;
            }
            let o = ((y as usize) * (self.w as usize) + x as usize) * 4;
            Some(unsafe { self.bits.add(o) })
        }

        /// 1px 蓝色边框（#1B4FE0）
        fn stroke_rect(&mut self, rx: i32, ry: i32, rw: i32, rh: i32) {
            let (b, g, r) = (0xE0u8, 0x4Fu8, 0x1Bu8);
            let put = |s: &mut Self, x: i32, y: i32| {
                if let Some(p) = s.px(x, y) {
                    unsafe {
                        *p = b;
                        *p.add(1) = g;
                        *p.add(2) = r;
                    }
                }
            };
            for x in rx..=rx + rw {
                put(self, x, ry);
                put(self, x, ry + rh);
            }
            for y in ry..=ry + rh {
                put(self, rx, y);
                put(self, rx + rw, y);
            }
        }

        /// 四个角的抓取点：白底 + 蓝边（8×8）
        fn handles(&mut self, rx: i32, ry: i32, rw: i32, rh: i32) {
            for (cx, cy) in [
                (rx, ry),
                (rx + rw, ry),
                (rx, ry + rh),
                (rx + rw, ry + rh),
            ] {
                for dy in -4..4 {
                    for dx in -4..4 {
                        let edge = dx == -4 || dx == 3 || dy == -4 || dy == 3;
                        let (b, g, r) = if edge { (0xE0, 0x4F, 0x1B) } else { (0xFF, 0xFF, 0xFF) };
                        if let Some(p) = self.px(cx + dx, cy + dy) {
                            unsafe {
                                *p = b as u8;
                                *p.add(1) = g as u8;
                                *p.add(2) = r as u8;
                            }
                        }
                    }
                }
            }
        }

        fn present(&mut self) {
            unsafe {
                let size = Size { cx: self.w, cy: self.h };
                let src = Point { x: 0, y: 0 };
                let blend = BlendFunction {
                    blend_op: AC_SRC_OVER,
                    blend_flags: 0,
                    source_constant_alpha: 255,
                    alpha_format: AC_SRC_ALPHA,
                };
                /* 位置用 UpdateLayeredWindow 的 dst_pos 保持原样：这里传当前窗口位置，
                   避免每次重绘把窗口挪到 (0,0)。 */
                let mut r = RectPos::default();
                GetWindowRect(self_hwnd(), &mut r);
                let dst = Point { x: r.left, y: r.top };
                let screen_dc = GetDC(0);
                UpdateLayeredWindow(
                    self_hwnd(),
                    screen_dc,
                    &dst,
                    &size,
                    self.mem,
                    &src,
                    0,
                    &blend,
                    ULW_ALPHA,
                );
                ReleaseDC(0, screen_dc);
            }
        }
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct RectPos {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    extern "system" {
        fn GetWindowRect(h: Hwnd, r: *mut RectPos) -> i32;
        fn SetCapture(h: Hwnd) -> Hwnd;
        fn ReleaseCapture() -> i32;
        fn SetForegroundWindow(h: Hwnd) -> i32;
    }

    static CUR_HWND: std::sync::atomic::AtomicIsize =
        std::sync::atomic::AtomicIsize::new(0);

    fn self_hwnd() -> isize {
        CUR_HWND.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 重绘一次（自己拿锁，别在持锁时调用会发消息的 API）
    fn repaint() {
        let mut g = SEL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_mut() {
            s.redraw();
        }
    }

    /// 注意：**持锁期间绝不能调用 SetCapture / ReleaseCapture**。
    /// 它们会同步给窗口发 WM_CAPTURECHANGED，又回到这个 wnd_proc 里再拿一次锁 ——
    /// std 的 Mutex 不可重入，直接死锁（0.2.4 第一次实测就卡在这里：
    /// 松手后协调线程再也拿不到锁，选区永远交不出去）。
    unsafe extern "system" fn wnd_proc(h: Hwnd, msg: u32, w: usize, l: isize) -> isize {
        match msg {
            WM_LBUTTONDOWN => {
                {
                    let mut g = SEL.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(s) = g.as_mut() {
                        s.start = (lo(l), hi(l));
                        s.rect = Some([s.start.0, s.start.1, 0, 0]);
                        s.dragging = true;
                        s.done = false;
                    }
                }
                SetCapture(h);
                repaint();
                0
            }
            WM_MOUSEMOVE => {
                let dragging = {
                    let mut g = SEL.lock().unwrap_or_else(|e| e.into_inner());
                    match g.as_mut() {
                        Some(s) if s.dragging => {
                            s.rect = Some(norm(s.start, (lo(l), hi(l))));
                            true
                        }
                        _ => false,
                    }
                };
                if dragging {
                    repaint();
                    0
                } else {
                    DefWindowProcW(h, msg, w, l)
                }
            }
            WM_LBUTTONUP => {
                let was = {
                    let mut g = SEL.lock().unwrap_or_else(|e| e.into_inner());
                    match g.as_mut() {
                        Some(s) if s.dragging => {
                            s.dragging = false;
                            s.done = true;
                            true
                        }
                        _ => false,
                    }
                };
                if was {
                    ReleaseCapture();
                    repaint();
                    0
                } else {
                    DefWindowProcW(h, msg, w, l)
                }
            }
            WM_RBUTTONDOWN => {
                if let Ok(mut g) = SEL.lock() {
                    if let Some(s) = g.as_mut() {
                        s.cancel = true;
                    }
                }
                0
            }
            WM_KEYDOWN => {
                if w == 0x1B {
                    if let Ok(mut g) = SEL.lock() {
                        if let Some(s) = g.as_mut() {
                            s.cancel = true;
                        }
                    }
                    return 0;
                }
                DefWindowProcW(h, msg, w, l)
            }
            _ => DefWindowProcW(h, msg, w, l),
        }
    }

    #[repr(C)]
    #[derive(Default)]
    struct BitmapInfoHeader {
        bi_size: u32,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: u32,
        bi_size_image: u32,
        bi_xppm: i32,
        bi_yppm: i32,
        bi_clr_used: u32,
        bi_clr_important: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 3],
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct BlendFunction {
        blend_op: u8,
        blend_flags: u8,
        source_constant_alpha: u8,
        alpha_format: u8,
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Size {
        cx: i32,
        cy: i32,
    }

    type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

    #[repr(C)]
    struct WndClassW {
        style: u32,
        wnd_proc: Option<WndProc>,
        cls_extra: i32,
        wnd_extra: i32,
        hinstance: isize,
        hicon: isize,
        hcursor: isize,
        hbr_background: isize,
        menu_name: *const u16,
        class_name: *const u16,
    }

    extern "system" {
        fn RegisterClassW(c: *const WndClassW) -> u16;
        fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: isize,
            menu: isize,
            inst: isize,
            param: *mut c_void,
        ) -> Hwnd;
        fn DefWindowProcW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
        fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
        fn DestroyWindow(h: Hwnd) -> i32;
        fn GetDC(h: Hwnd) -> Hdc;
        fn ReleaseDC(h: Hwnd, dc: Hdc) -> i32;
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn UpdateLayeredWindow(
            h: Hwnd,
            dst: Hdc,
            dst_pos: *const Point,
            size: *const Size,
            src: Hdc,
            src_pos: *const Point,
            color_key: u32,
            blend: *const BlendFunction,
            flags: u32,
        ) -> i32;
    }

    extern "system" {
        fn CreateCompatibleDC(dc: Hdc) -> Hdc;
        fn CreateDIBSection(
            dc: Hdc,
            bi: *const BitmapInfo,
            usage: u32,
            bits: *mut *mut c_void,
            section: isize,
            offset: u32,
        ) -> isize;
        fn SelectObject(dc: Hdc, obj: Hgdiobj) -> Hgdiobj;
        fn DeleteObject(obj: Hgdiobj) -> i32;
        fn DeleteDC(dc: Hdc) -> i32;
    }

    fn class_name() -> &'static [u16] {
        static N: OnceLock<Vec<u16>> = OnceLock::new();
        N.get_or_init(|| "AMSnipSplash\0".encode_utf16().collect())
    }

    fn register_class() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            let wc = WndClassW {
                style: 0,
                wnd_proc: Some(wnd_proc),
                cls_extra: 0,
                wnd_extra: 0,
                hinstance: GetModuleHandleW(std::ptr::null()),
                hicon: 0,
                hcursor: 0,
                hbr_background: 0,
                menu_name: std::ptr::null(),
                class_name: class_name().as_ptr(),
            };
            RegisterClassW(&wc);
        });
    }

    /// 把整张截图铺到屏幕上（返回窗口句柄；失败返回 Err，调用方降级即可）
    pub fn show(x: i32, y: i32, w: u32, h: u32, rgba: &[u8]) -> Result<isize, String> {
        let n = w as usize * h as usize;
        if w == 0 || h == 0 || rgba.len() < n * 4 {
            return Err("位图尺寸对不上".into());
        }
        unsafe {
            register_class();
            /* 可激活（去掉 WS_EX_NOACTIVATE）：这块窗口现在要接鼠标拖框和 Esc，
               不激活就拿不到键盘。 */
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class_name().as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                x,
                y,
                w as i32,
                h as i32,
                0,
                0,
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            );
            if hwnd == 0 {
                return Err("建原生底图窗口失败".into());
            }
            CUR_HWND.store(hwnd, std::sync::atomic::Ordering::SeqCst);

            let mut bi = BitmapInfo::default();
            bi.header.bi_size = std::mem::size_of::<BitmapInfoHeader>() as u32;
            bi.header.bi_width = w as i32;
            bi.header.bi_height = -(h as i32); // top-down
            bi.header.bi_planes = 1;
            bi.header.bi_bit_count = 32;
            bi.header.bi_compression = 0; // BI_RGB

            let screen_dc = GetDC(0);
            let mem_dc = CreateCompatibleDC(screen_dc);
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(mem_dc, &bi, 0, &mut bits, 0, 0);
            if bmp == 0 || bits.is_null() {
                ReleaseDC(0, screen_dc);
                DeleteDC(mem_dc);
                DestroyWindow(hwnd);
                return Err("建 DIB 失败".into());
            }
            SelectObject(mem_dc, bmp);

            /* RGBA → BGRA（UpdateLayeredWindow 要的是 0xAARRGGBB，内存序 B,G,R,A）。
               顺便把"整屏压暗版"也备好，拖框时每帧只要 memcpy。 */
            let mut base = vec![0u8; n * 4];
            for i in 0..n {
                let o = i * 4;
                base[o] = *rgba.as_ptr().add(o + 2);
                base[o + 1] = *rgba.as_ptr().add(o + 1);
                base[o + 2] = *rgba.as_ptr().add(o);
                base[o + 3] = 255;
            }
            let dim = Sel::make_dim(&base);
            {
                let mut g = SEL.lock().unwrap_or_else(|e| e.into_inner());
                *g = Some(Sel {
                    base,
                    dim,
                    w: w as i32,
                    h: h as i32,
                    mem: mem_dc,
                    bmp,
                    bits: bits as *mut u8,
                    rect: None,
                    start: (0, 0),
                    dragging: false,
                    cancel: false,
                    done: false,
                });
                if let Some(s) = g.as_mut() {
                    s.redraw();
                }
            }
            ReleaseDC(0, screen_dc);
            /* 激活 + 显示：要接键盘（Esc），所以用 SW_SHOW 而不是 SW_SHOWNOACTIVATE */
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
            Ok(hwnd)
        }
    }

    /// 用户按了 Esc / 右键
    pub fn take_cancel() -> bool {
        let mut g = SEL.lock().unwrap_or_else(|e| e.into_inner());
        match g.as_mut() {
            Some(s) if s.cancel => {
                s.cancel = false;
                true
            }
            _ => false,
        }
    }

    /// 当前选区（物理像素，客户区坐标）
    pub fn current_rect() -> Option<[i32; 4]> {
        let g = SEL.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().and_then(|s| s.rect)
    }

    pub fn dragging() -> bool {
        let g = SEL.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().map(|s| s.dragging).unwrap_or(false)
    }

    /// 把选区交给网页接手之后，原生层停止画选区（网页那套更完整）
    pub fn freeze() {
        let mut g = SEL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_mut() {
            s.dragging = false;
            s.done = true;
        }
    }

    pub fn close(hwnd: isize) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            DestroyWindow(hwnd);
            let mut g = SEL.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(mut s) = g.take() {
                SelectObject(s.mem, 0);
                DeleteObject(s.bmp);
                DeleteDC(s.mem);
                s.bits = std::ptr::null_mut();
            }
            CUR_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// 抽干本线程的消息队列。窗口是在这个线程里建的，就必须有人给它派消息，
    /// 否则 `DestroyWindow` 之后那些 `WM_DESTROY` 永远排在队列里没人处理。
    pub fn pump() {
        #[repr(C)]
        struct Pt {
            x: i32,
            y: i32,
        }
        #[repr(C)]
        struct Msg {
            hwnd: isize,
            message: u32,
            w: usize,
            l: isize,
            time: u32,
            pt: Pt,
        }
        extern "system" {
            fn PeekMessageW(m: *mut Msg, hwnd: isize, min: u32, max: u32, remove: u32) -> i32;
            fn TranslateMessage(m: *const Msg) -> i32;
            fn DispatchMessageW(m: *const Msg) -> isize;
        }
        const PM_REMOVE: u32 = 1;
        unsafe {
            let mut m: Msg = std::mem::zeroed();
            while PeekMessageW(&mut m, 0, 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    }
}

#[cfg(not(windows))]
mod splash {
    pub fn show(_x: i32, _y: i32, _w: u32, _h: u32, _rgba: &[u8]) -> Result<isize, String> {
        Err("unsupported".into())
    }
    pub fn close(_hwnd: isize) {}
    pub fn pump() {}
    pub fn take_cancel() -> bool {
        false
    }
    pub fn current_rect() -> Option<[i32; 4]> {
        None
    }
    pub fn dragging() -> bool {
        false
    }
    pub fn freeze() {}
}

/* 0.2.3 起**不再**做「截一张窗口画面来判断它画没画好」的探针。
   原因记在这里，免得以后有人又想加：`PrintWindow` 会逼 WebView2 的合成窗口重绘一次，
   实测连续调用几次之后渲染进程直接退出（tools/gui.py 里记着这条，
   FontManager 的 WORKLOG 也有同样记录）。截图这种高频路径上拿它当探针
   等于每次按 F1 都在赌渲染进程会不会死。 */

/// 原生底图窗口的命令。
enum SplashCmd {
    Show {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        rgba: Vec<u8>,
        ack: std::sync::mpsc::Sender<bool>,
    },
    Close {
        ack: std::sync::mpsc::Sender<()>,
    },
}

static SPLASH_TX: OnceLock<std::sync::mpsc::Sender<SplashCmd>> = OnceLock::new();

/// 底图窗口句柄的**只读镜像**（真句柄归 `am-splash` 线程所有）。
/// 只给窗口枚举用：识别窗口时要跳过自己这块全屏底图，不然选中的永远是它。
static SPLASH_HWND_VIEW: AtomicIsize = AtomicIsize::new(0);

/// 原生底图窗口的**专属线程**（建窗 / 贴图 / 销毁全在这里做）。
///
/// 为什么要单开一个线程：`snip_begin` 跑在 tokio 的工作线程上（异步命令），
/// 而 **Win32 窗口只能由创建它的线程销毁**。跨线程 `DestroyWindow` 直接失败，
/// 于是那块全屏 topmost 的底图窗口永远留在桌面上 —— 用户看到的就是
/// 「每次截图后取消会卡死」：屏幕冻在截图上、点什么都没反应（2026-11-17 owner 报）。
///
/// 现在这个线程独占窗口句柄，带消息泵，任何线程调用 `splash_close()` 都能可靠撤掉。
fn splash_tx() -> &'static std::sync::mpsc::Sender<SplashCmd> {
    SPLASH_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<SplashCmd>();
        let spawned = std::thread::Builder::new()
            .name("am-splash".into())
            .spawn(move || {
                let mut hwnd: isize = 0;
                loop {
                    match rx.recv_timeout(Duration::from_millis(8)) {
                        Ok(SplashCmd::Show { x, y, w, h, rgba, ack }) => {
                            if hwnd != 0 {
                                splash::close(hwnd);
                                hwnd = 0;
                                SPLASH_HWND_VIEW.store(0, Ordering::SeqCst);
                            }
                            match splash::show(x, y, w, h, &rgba) {
                                Ok(h) => {
                                    hwnd = h;
                                    SPLASH_HWND_VIEW.store(h, Ordering::SeqCst);
                                    let _ = ack.send(true);
                                }
                                Err(e) => {
                                    if let Some(d) = LOG_DIR.get() {
                                        log_runtime(d, &format!("snip splash FAIL {e}"));
                                    }
                                    let _ = ack.send(false);
                                }
                            }
                        }
                        Ok(SplashCmd::Close { ack }) => {
                            if hwnd != 0 {
                                splash::close(hwnd);
                                hwnd = 0;
                                SPLASH_HWND_VIEW.store(0, Ordering::SeqCst);
                            }
                            let _ = ack.send(());
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        /* 发送端被丢了（进程收尾）：把窗口收干净再退 */
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            if hwnd != 0 {
                                splash::close(hwnd);
                                SPLASH_HWND_VIEW.store(0, Ordering::SeqCst);
                            }
                            break;
                        }
                    }
                    splash::pump();
                }
            });
        if spawned.is_err() {
            if let Some(d) = LOG_DIR.get() {
                log_runtime(d, "snip splash 线程起不来（降级：没有底图层）");
            }
        }
        tx
    })
}

/// 把截图立刻铺到屏幕上（零延迟）。失败只是没有这一层，不影响截图本身。
/// **会等底图上屏再返回**（最多 600ms）：紧跟着建 WebView 覆盖层，
/// 底图没上就已经是"黑一下"了。
fn splash_show(x: i32, y: i32, w: u32, h: u32, rgba: &[u8]) {
    let (ack_tx, ack_rx) = std::sync::mpsc::channel();
    let cmd = SplashCmd::Show {
        x,
        y,
        w,
        h,
        rgba: rgba.to_vec(),
        ack: ack_tx,
    };
    if splash_tx().send(cmd).is_ok() {
        let _ = ack_rx.recv_timeout(Duration::from_millis(600));
    }
}

/// 撤掉原生底图（WebView 覆盖层已经画好了，或者截图被取消）。
/// 阻塞到真的撤掉为止 —— 不然取消之后底图还挂在最上面，等于把桌面锁死。
fn splash_close() {
    let (ack_tx, ack_rx) = std::sync::mpsc::channel();
    if splash_tx().send(SplashCmd::Close { ack: ack_tx }).is_ok() {
        let _ = ack_rx.recv_timeout(Duration::from_millis(800));
    }
}

/// 整扇窗的全局 alpha（Win32 layered，0–255）。
///
/// 覆盖层的出场靠它：建完窗先设成 **1**（肉眼看不见，但窗口是**可见且在 z 序最前**的），
/// 这样 WebView2 既不会因为"被底图盖住"而判定自己被遮挡（Chromium 的遮挡检测会给
/// 被遮窗口降频/停帧 —— 这正是"一直黑到有人把它提到最前才画"的真因），
/// 又不会让用户看到还没画好的窗口。首帧探针确认画好了再淡到 255。
#[allow(dead_code)] // 0.2.3 起不再用它（WebView2 合成内容不吃 layered alpha），留作排查工具
#[cfg(windows)]
fn set_window_alpha(win: &tauri::WebviewWindow, alpha: u8) -> Result<(), String> {
    type Hwnd = isize;
    extern "system" {
        fn GetWindowLongPtrW(h: Hwnd, idx: i32) -> isize;
        fn SetWindowLongPtrW(h: Hwnd, idx: i32, v: isize) -> isize;
        fn SetLayeredWindowAttributes(h: Hwnd, key: u32, alpha: u8, flags: u32) -> i32;
    }
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const LWA_ALPHA: u32 = 0x0000_0002;
    let raw = win.hwnd().map_err(|e| format!("拿不到窗口句柄：{e}"))?.0 as Hwnd;
    if raw == 0 || raw == -1 {
        return Err("窗口句柄无效".into());
    }
    unsafe {
        let ex = GetWindowLongPtrW(raw, GWL_EXSTYLE);
        SetWindowLongPtrW(raw, GWL_EXSTYLE, ex | WS_EX_LAYERED);
        if SetLayeredWindowAttributes(raw, 0, alpha, LWA_ALPHA) == 0 {
            return Err("设置窗口透明度失败".into());
        }
    }
    Ok(())
}

#[allow(dead_code)]
#[cfg(not(windows))]
fn set_window_alpha(_win: &tauri::WebviewWindow, _alpha: u8) -> Result<(), String> {
    Ok(())
}

async fn snip_begin(app: AppHandle) -> Result<(), String> {
    /* 新会话：让上一次迟到的收尾线程（`am_snip_ready` 里那条）失效 */
    SNIP_GEN.fetch_add(1, Ordering::SeqCst);
    /* 0.1.27：`Ctrl+C` / `Ctrl+S` 恢复（owner 要的），所以又要判「这颗键是真人按的还是
       外部程序注入的」—— 见 core/src/keywatch.rs。钩子必须装在有消息泵的**主线程**上，
       走 run_on_main_thread；装不上/没收到任何事件时判据自动放行，不会让快捷键失效。 */
    keywatch::reset();
    /* 钩子**不占关键路径**：它要等系统调度、还要建消息泵，慢的时候几十毫秒；
       而用户至少要几百毫秒之后才会按 Ctrl+C/S。扔后台装，F1 → 可框选这段更快。 */
    std::thread::spawn(|| {
        let ok = keywatch::install();
        if let Some(d) = LOG_DIR.get() {
            log_runtime(
                d,
                &format!(
                    "keywatch install ok={ok} tid={} err={}",
                    keywatch::hook_tid(),
                    keywatch::last_error()
                ),
            );
        }
    });
    /* 残留状态清理。**这个不能省**：覆盖层被外部关掉（Alt+F4 / 崩溃 / 用户点了别处）时
       `am_snip_cancel` 不会执行，slot 一直占着 → 之后每次按 F1 都回"上一次截图还没结束"，
       永久失效，用户只会说"F1 坏了"。有窗口才算真在进行中。 */
    {
        let mut g = snip_slot().lock().unwrap_or_else(|e| e.into_inner());
        if g.is_some() {
            if app.get_webview_window("snip").is_some() {
                return Err("上一次截图还没结束".into());
            }
            if let Some(s) = g.take() {
                let _ = std::fs::remove_file(&s.tmp);
            }
        }
    }
    /* 上次的底图先撤掉（正常会随 cancel/finish 撤，这里兜残留） */
    splash_close();
    /* 残留窗口（状态空了但窗口还没销毁完）：**必须等它真的消失**。
       `close()` 是排进消息队列的，紧接着 `build()` 同一个 label 会撞
       "a webview with label `snip` already exists"（2026-11-17 实测日志）。 */
    if let Some(old) = app.get_webview_window("snip") {
        let _ = old.close();
        for _ in 0..60 {
            if app.get_webview_window("snip").is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /* 计时（0.1.29 提速用）：从进 `snip_begin` 到窗口建好，各阶段各占多少毫秒。
       只在 rust 端口径里量，不含前端 thumb 解码 —— 那部分看 `snip ready`。 */
    let t_all = std::time::Instant::now();
    *SNIP_T0.lock().unwrap_or_else(|e| e.into_inner()) = Some(t_all);
    /* 不再有"藏贴图 → 等 DWM 交一帧"那 50ms：贴图本来就要留在画面里
       （见上方 3640 那段说明），没有窗口要从屏幕上撤下来，也就没有残影要等。
       顺带把 F1 → 能框选的路径又省掉 50ms。 */

    // 截屏要在开覆盖窗**之前**：覆盖窗一开，截到的就是它自己
    let shot = match screen::capture_virtual() {
        Ok(s) => s,
        Err(e) => return Err(e),
    };
    /* 0.3.0「A 方案」：默认不再走 WebView，整块覆盖层由 `snip_native` 用 GDI 画
       （冻结图 / 拖框 / 工具栏 / 标注 / 撤销全在里面），交付仍复用 `am_snip_finish`。
       老路径留在后面，`AM_SNIP_WEBVIEW=1` 可以切回去对照。 */
    if std::env::var("AM_SNIP_WEBVIEW").map(|v| v == "1").unwrap_or(false) == false {
        /* 屏幕上的贴图窗口：原生层压暗时要**把它们跳过去**，保持 100% 亮度
           （owner 2026-11-19：「截图时贴图不该慢慢消失」）。
           矩形用物理像素、换算到虚拟桌面左上角 —— 和 `shot` 的坐标系一致。 */
        let keep: Vec<[i32; 4]> = {
            use tauri::Manager;
            let mut v = Vec::new();
            for (l, w) in app.webview_windows() {
                if !l.starts_with("peek") {
                    continue;
                }
                let (Ok(p), Ok(s)) = (w.outer_position(), w.outer_size()) else {
                    continue;
                };
                v.push([p.x - shot.x, p.y - shot.y, s.width as i32, s.height as i32]);
            }
            v
        };
        let app2 = app.clone();
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let theme = UI_THEME.load(std::sync::atomic::Ordering::SeqCst);
            let out = snip_native::run(&shot, theme, &keep);
            match out {
                Some(d) => {
                    use base64::{engine::general_purpose::STANDARD, Engine as _};
                    let b64 = STANDARD.encode(&d.png);
                    let place = vec![d.place[0], d.place[1], d.place[2], d.place[3]];
                    let r = tauri::async_runtime::block_on(am_snip_finish(
                        app2.clone(),
                        b64,
                        d.action.clone(),
                        Some("native".to_string()),
                        Some(place),
                    ));
                    if let Err(e) = r {
                        if let Some(dir) = LOG_DIR.get() {
                            log_runtime(dir, &format!("snip native deliver FAIL {e}"));
                        }
                    }
                }
                None => {
                    if let Some(dir) = LOG_DIR.get() {
                        log_runtime(dir, "snip native cancel");
                    }
                }
            }
            std::thread::spawn(keywatch::uninstall);
            if let Some(dir) = LOG_DIR.get() {
                log_runtime(
                    dir,
                    &format!("snip native total={}ms", t0.elapsed().as_millis()),
                );
            }
        });
        return Ok(());
    }

    /* **先铺原生底图**（GDI 直接 blit，零延迟）：用户按下 F1 的下一帧就能看到
       冻住的桌面。WebView 覆盖层随后叠上来画压暗 / 选区 / 工具栏，
       工具栏就绪后（`am_snip_ready`）再把这一层撤掉。
       PNG 编码 + WebView2 起来要几百毫秒，这段时间屏幕上就是这张图 —— 不会黑。 */
    splash_show(shot.x, shot.y, shot.w, shot.h, &shot.rgba);

    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("snip t capture+hide={}ms", t_all.elapsed().as_millis()));
    }
    let png = match shot.png() {
        Ok(p) => p,
        Err(e) => return Err(e),
    };
    let tmp = std::env::temp_dir().join(format!(
        "am-snip-{}.png",
        std::process::id() as u64 * 1000 + (db::now_ts() as u64 % 1000)
    ));
    if let Err(e) = std::fs::write(&tmp, &png) {
        return Err(format!("写截图临时文件失败：{e}"));
    }

    if let Some(d) = LOG_DIR.get() {
        log_runtime(
            d,
            &format!("snip t png+write={}ms (png {}KB)", t_all.elapsed().as_millis(), png.len() / 1024),
        );
    }
    let scale = app
        .get_webview_window("main")
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let (vx, vy, vw, vh) = (shot.x, shot.y, shot.w, shot.h);
    /* 低清底图（480 宽）：窗口一显示就有内容，治"截图开始黑一下"。
       详见 `screen::Shot::thumb_png` —— 高清图上传纹理要 300~400ms，
       这段时间里窗口只能显示自己的背景色。 */
    let thumb = {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        shot.thumb_png(480)
            .ok()
            .map(|b| format!("data:image/png;base64,{}", STANDARD.encode(&b)))
    };
    /* 平均色：覆盖层根节点先铺它。WebView 在隐藏窗口里不预合成，
       窗口显示到第一帧之间那 200~400ms 只能看到 WebView 背景（近黑）——
       铺成截图的平均色之后，那一段看起来就是"屏幕压暗了一下"。 */
    let avg = shot.avg_rgb();
    *snip_slot().lock().unwrap_or_else(|e| e.into_inner()) =
        Some(SnipState { shot, tmp: tmp.clone() });

    if let Some(old) = app.get_webview_window("snip") {
        let _ = old.close();
    }
    let boot = serde_json::json!({
        "src": media_url(&tmp),
        "thumb": thumb,
        "avg": avg,
        "scale": scale,
        "x": vx, "y": vy, "w": vw, "h": vh,
    });
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("snip t thumb={}ms", t_all.elapsed().as_millis()));
    }
    let script = format!("window.__AM_SNIP__ = {boot};");
    let win = tauri::WebviewWindowBuilder::new(&app, "snip", tauri::WebviewUrl::App("index.html".into()))
        .title("截图")
        .inner_size(vw as f64 / scale, vh as f64 / scale)
        .position(vx as f64 / scale, vy as f64 / scale)
        .decorations(false)
        /* **不透明 + 先隐形**（实测：透明窗口里 WebView2 只画得出文字，位图完全不渲染，
           所以不能靠"透出桌面"来躲黑屏）。
           现在的顺序：建不可见窗 → 设 layered alpha=1 → show（窗口可见，WebView2 才开始
           出帧，但人眼看不见）→ 低清底图解码完 → `am_snip_ready` 打回不透明。
           配合 120ms 淡入，用户看到的是覆盖层柔和浮现，不是"闪一下黑"。 */
        .transparent(false)
        /* 窗口底色 = 这张截图的平均色（不是全局深色底）：新开的 WebView2 窗口在
           交出第一帧之前露出来的就是它。哪怕淡入比内容快，露出来的也是图像主色调，
           而不是一块纯黑（`body.snip-body` 是 transparent，所以看得到窗口底色）。 */
        .background_color(tauri::window::Color(avg[0], avg[1], avg[2], 255))
        .shadow(false)
        .resizable(false)
        /* **不置顶**：原生底图（splash）在上面盖着，它才能"先悄悄渲染、后揭幕"。
           0.2.3 实测：WebView2 的合成内容**不吃 layered alpha**（设了 alpha=1
           窗口照样是不透明的黑），所以"半透明隐身"这条路走不通，只能靠底图盖。 */
        .always_on_top(false)
        .skip_taskbar(true)
        /* **先不显示**（owner 2026-11-17 报「截图开始先黑屏一下」）：
           以前这里一建窗就是可见的，而整屏底图要等 WebView2 起来、React 挂上、
           `<img>` 解码完才画得出来 —— 中间那 100~300ms 用户看到的是窗口自己的
           背景色（深色主题下就是一片黑），所以每次截图都"黑一下"。
           现在的顺序：建不可见窗 → 前端底图 onLoad 且画完第一帧 → `am_snip_ready`
           把它显示出来。人眼看到的第一帧就是冻结的桌面，不再是黑屏。 */
        /* 排查开关：AM_SNIP_VISIBLE=1 时建出来就 visible（用来验证"先 show 再画"
           和"建窗即 visible" 对首帧时间有没有区别）。默认仍是先隐形、随后 show。 */
        .visible(std::env::var("AM_SNIP_VISIBLE").map(|v| v == "1").unwrap_or(false))
        .initialization_script(&script)
        .build()
        .map_err(|e| {
            if let Some(d) = LOG_DIR.get() {
                log_runtime(d, &format!("snip build FAIL {e}"));
            }
            let mut g = snip_slot().lock().unwrap_or_else(|x| x.into_inner());
            if let Some(s) = g.take() {
                let _ = std::fs::remove_file(&s.tmp);
            }
            format!("截图窗口创建失败：{e}")
        })?;
    /* 先隐形再 show：窗口必须是"可见 + 置顶"的（WebView2 才肯出帧、Chromium 才不判遮挡），
       但此刻它还是一块没画好的底色 —— alpha=1 让用户看到的是下面那块冻结的桌面。
       首帧探针确认内容真的画出来之后，`am_snip_ready` 把它淡到 255 并撤底图。 */
    let vis_now = std::env::var("AM_SNIP_VISIBLE").map(|v| v == "1").unwrap_or(false);
    if !vis_now {
        let _ = win.show();
    }
    /* **不要抢焦点**：这一刻前台是那块原生层（它要接鼠标拖框和 Esc）。
       焦点在原生层 → 用户立刻能框选；网页画好了由协调线程再交还焦点。 */
    if let Some(d) = LOG_DIR.get() {
        log_runtime(
            d,
            &format!("snip t window-shown={}ms", t_all.elapsed().as_millis()),
        );
    }

    /* 协调线程（0.2.4「C 方案」）：
       此刻屏幕上/前台都是那块原生层，用户已经能拖框了。这里只做两件事：
         ① 用户按 Esc / 右键 → 整场取消（不用等网页）；
         ② 网页该画好了（实测 F1+1050ms）且手没在拖 → 把当前选区推给网页，
            再把网页提到最前、撤掉原生层。之后标注、工具栏、放大镜全归网页，
            功能和以前一模一样。 */
    {
        let app2 = app.clone();
        let win2 = win.clone();
        let gen = SNIP_GEN.load(Ordering::SeqCst);
        std::thread::spawn(move || {
            let t0 = SNIP_T0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .map(|t| t)
                .unwrap_or_else(std::time::Instant::now);
            let reveal = std::env::var("AM_SNIP_REVEAL_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1050);
            if let Some(d) = LOG_DIR.get() {
                log_runtime(d, &format!("snip coord start gen={gen} reveal={reveal}"));
            }
            let mut ticks = 0u32;
            loop {
                ticks += 1;
                if ticks % 20 == 0 {
                    if let Some(d) = LOG_DIR.get() {
                        log_runtime(
                            d,
                            &format!(
                                "snip coord tick {}ms dragging={} rect={:?}",
                                t0.elapsed().as_millis(),
                                splash::dragging(),
                                splash::current_rect()
                            ),
                        );
                    }
                }
                if SNIP_GEN.load(Ordering::SeqCst) != gen {
                    return;
                }
                if splash::take_cancel() {
                    if let Some(d) = LOG_DIR.get() {
                        log_runtime(d, "snip native cancel（原生层 Esc/右键）");
                    }
                    snip_abort(&app2);
                    return;
                }
                /* 兜底：如果因为丢鼠标捕获等原因一直显示"还在拖"，最多再等 3 秒就强行交接，
                   否则整场会卡死（0.2.4 实测过一次）。 */
                let overdue = t0.elapsed() >= Duration::from_millis(reveal + 3000);
                if t0.elapsed() >= Duration::from_millis(reveal)
                    && (!splash::dragging() || overdue)
                {
                    if let Some(r) = splash::current_rect() {
                        if r[2] >= 6 && r[3] >= 6 {
                            let payload = serde_json::json!({
                                "x": r[0], "y": r[1], "w": r[2], "h": r[3]
                            });
                            let _ = app2.emit_to("snip", "snip:sel", payload);
                            /* 给网页一帧把选区画上去，再接手 */
                            std::thread::sleep(Duration::from_millis(45));
                        }
                    }
                    if SNIP_GEN.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    let _ = win2.set_always_on_top(true);
                    let _ = win2.set_focus();
                    splash::freeze();
                    splash_close();
                    if let Some(d) = LOG_DIR.get() {
                        log_runtime(
                            d,
                            &format!("snip t handoff={}ms", t0.elapsed().as_millis()),
                        );
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
    }
    Ok(())
}

fn remember_shot_dir(app: &AppHandle) {
    let dir = shot_dir().to_string_lossy().to_string();
    let st = app.state::<AppState>();
    let mut a = lock(&st);
    if a.lib_for_path(&dir).is_some() {
        let mut set = HashSet::new();
        set.insert(PathBuf::from(&dir));
        refresh_dirs(&mut a, &set);
        return;
    }
    /* 内置库**不占用户那 3 个名额**（owner 2026-11-17）：以前这里判 `lib_count >= 3`
       就 return，于是用户挂满 3 个自己的库之后，截图目录就再也进不来了 ——
       剪了图、贴了图，软件里找不到（"截图去哪了"）。现在永远登记。 */
    if let Err(e) = a.scan(&dir, |_p| {}) {
        if let Some(d) = LOG_DIR.get() {
            log_runtime(d, &format!("截图目录加入素材库失败：{e}"));
        }
    }
}

/// 解 base64 并确认它真是 PNG（前端 canvas 的 `toDataURL` 一定给 PNG）。
fn png_from_b64(data: &str) -> Result<Vec<u8>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let body = data.split_once(',').map(|(_, b)| b).unwrap_or(data);
    let bytes = STANDARD.decode(body.trim()).map_err(|e| format!("图片数据坏了：{e}"))?;
    if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("拿到的不是 PNG".into());
    }
    Ok(bytes)
}

/// 裁出选区（**只裁不存**）。`x/y/w/h` 是截图窗口里的 CSS 像素，
/// `scale` 是覆盖窗的缩放比（前端从 boot 里带过来，比事后查窗口稳）。
///
/// 返回 `data:image/png;base64,…` + 物理像素宽高：前端拿它当 canvas 底图。
/// 用 data URL 而不是 `peekfile://`，是因为**跨域图片会把 canvas 污染掉**，
/// 一污染 `toDataURL` 就抛错，标注就导不出来了。
#[tauri::command]
fn am_snip_crop(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    scale: f64,
) -> Result<(String, u32, u32), String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let g = snip_slot().lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.as_ref() else {
        return Err("截图数据已经过期，请重新按一次快捷键".into());
    };
    let sc = if scale > 0.0 { scale } else { 1.0 };
    let img = s
        .shot
        .crop(
            (x * sc).round() as i32,
            (y * sc).round() as i32,
            (w * sc).round() as i32,
            (h * sc).round() as i32,
        )
        .ok_or_else(|| "选区太小了".to_string())?;
    let png = screen::png_of(&img)?;
    Ok((
        format!("data:image/png;base64,{}", STANDARD.encode(&png)),
        img.width(),
        img.height(),
    ))
}

/// 整张截图（**data URL**）。前端拿它建一张离屏 canvas 当"像素源"：
/// 放大镜要看像素、马赛克要采样、导出要合成 —— 都需要能 `getImageData`，
/// 而 `boot.src` 是 `http://peekfile.localhost/…`（跨源），画进 canvas 会污染，
/// `toDataURL` 直接抛错。所以这里给一份同源的 data URL。
#[tauri::command]
fn am_snip_full() -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let t = std::time::Instant::now();
    let g = snip_slot().lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.as_ref() else {
        return Err("截图数据已经过期，请重新按一次快捷键".into());
    };
    let png = s.shot.png()?;
    let out = format!("data:image/png;base64,{}", STANDARD.encode(&png));
    if let Some(d) = LOG_DIR.get() {
        let total = SNIP_T0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|s| s.elapsed().as_millis())
            .unwrap_or(0);
        log_runtime(
            d,
            &format!(
                "snip t full={}ms ({}KB, from F1 {total}ms)",
                t.elapsed().as_millis(),
                out.len() / 1024
            ),
        );
    }
    Ok(out)
}

/// 光标底下那一扇顶层窗口的矩形（**照 Snipaste 的"自动识别窗口"**）。
///
/// 入参 `x/y` 是覆盖层里的 CSS 像素；返回值也用 CSS 像素（相对覆盖层左上角）。
/// 找不到（桌面 / 全透明窗 / 只剩自己）就返回 `None`，前端就只显示十字线。
///
/// 为什么不用 `WindowFromPoint`：那个函数返回的一定是**最上面**那扇 —— 也就是我们自己的
/// 截图覆盖窗。所以这里按 z-order 从上往下走，跳过自己（覆盖层 + 原生底图），
/// 取第一扇"可见 + 没被 DWM 隐藏 + 矩形包含该点"的顶层窗口。
#[derive(serde::Serialize)]
struct WinBox {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[cfg(windows)]
fn window_rect_at(px: i32, py: i32, skip: &[isize]) -> Option<(i32, i32, i32, i32)> {
    type Hwnd = isize;
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Rc {
        l: i32,
        t: i32,
        r: i32,
        b: i32,
    }
    extern "system" {
        fn GetTopWindow(parent: Hwnd) -> Hwnd;
        fn GetWindow(h: Hwnd, cmd: u32) -> Hwnd;
        fn GetWindowRect(h: Hwnd, r: *mut Rc) -> i32;
        fn IsWindowVisible(h: Hwnd) -> i32;
        fn IsIconic(h: Hwnd) -> i32;
        fn GetWindowLongPtrW(h: Hwnd, idx: i32) -> isize;
        fn DwmGetWindowAttribute(h: Hwnd, attr: u32, pv: *mut core::ffi::c_void, cb: u32) -> i32;
        fn GetWindowThreadProcessId(h: Hwnd, pid: *mut u32) -> u32;
    }
    const GW_HWNDNEXT: u32 = 2;
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
    const DWMWA_CLOAKED: u32 = 14;

    let me = std::process::id();
    unsafe {
        let mut h = GetTopWindow(0);
        let mut guard = 0;
        while h != 0 && guard < 4000 {
            guard += 1;
            /* 本进程的窗口一律不算"底下的窗口"：覆盖层、原生底图、贴图窗都是我们自己的。
               只按句柄排除不够 —— 底图窗口销毁是异步的，句柄早清空了窗口还在。 */
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(h, &mut pid);
            if !skip.contains(&h) && pid != me && IsWindowVisible(h) != 0 && IsIconic(h) == 0 {
                let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
                /* 全透明穿透窗（输入法候选、覆盖层之类）不算"窗口" */
                if ex & WS_EX_TRANSPARENT == 0 {
                    let mut cloaked: u32 = 0;
                    let _ = DwmGetWindowAttribute(
                        h,
                        DWMWA_CLOAKED,
                        &mut cloaked as *mut u32 as *mut core::ffi::c_void,
                        4,
                    );
                    if cloaked == 0 {
                        /* 优先用 DWM 的"可见边框"：GetWindowRect 含一圈看不见的阴影边，
                           自动识别出来的框会比眼睛看到的窗口大 7~8px */
                        let mut rc = Rc::default();
                        let ok = DwmGetWindowAttribute(
                            h,
                            DWMWA_EXTENDED_FRAME_BOUNDS,
                            &mut rc as *mut Rc as *mut core::ffi::c_void,
                            std::mem::size_of::<Rc>() as u32,
                        ) == 0
                            || GetWindowRect(h, &mut rc) != 0;
                        if ok
                            && rc.r > rc.l
                            && rc.b > rc.t
                            && px >= rc.l
                            && px < rc.r
                            && py >= rc.t
                            && py < rc.b
                        {
                            /* 工具窗口（浮动面板）也让用户能选中，只是别把它当成桌面的替代 */
                            let _ = WS_EX_TOOLWINDOW;
                            return Some((rc.l, rc.t, rc.r - rc.l, rc.b - rc.t));
                        }
                    }
                }
            }
            h = GetWindow(h, GW_HWNDNEXT);
        }
    }
    None
}

#[cfg(not(windows))]
fn window_rect_at(_px: i32, _py: i32, _skip: &[isize]) -> Option<(i32, i32, i32, i32)> {
    None
}

/// 光标按像素微调（Snipaste 的 W/A/S/D）。
#[tauri::command]
fn am_cursor_step(dx: i32, dy: i32) -> Result<(), String> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Pt {
            x: i32,
            y: i32,
        }
        extern "system" {
            fn GetCursorPos(p: *mut Pt) -> i32;
            fn SetCursorPos(x: i32, y: i32) -> i32;
        }
        let mut p = Pt { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut p);
            SetCursorPos(p.x + dx, p.y + dy);
        }
    }
    let _ = (dx, dy);
    Ok(())
}

#[tauri::command]
async fn am_snip_window_at(
    app: AppHandle,
    x: f64,
    y: f64,
    scale: f64,
) -> Result<Option<WinBox>, String> {
    let (sx, sy, sw, sh) = {
        let g = snip_slot().lock().unwrap_or_else(|e| e.into_inner());
        match g.as_ref() {
            Some(s) => (s.shot.x, s.shot.y, s.shot.w as i32, s.shot.h as i32),
            None => return Ok(None),
        }
    };
    let sc = if scale > 0.0 { scale } else { 1.0 };
    let px = sx + (x * sc).round() as i32;
    let py = sy + (y * sc).round() as i32;
    let mut skip: Vec<isize> = Vec::new();
    if let Some(w) = app.get_webview_window("snip") {
        if let Ok(h) = w.hwnd() {
            skip.push(h.0 as isize);
        }
    }
    let splash = SPLASH_HWND_VIEW.load(Ordering::SeqCst);
    if splash != 0 {
        skip.push(splash);
    }
    let Some((rx, ry, rw, rh)) = window_rect_at(px, py, &skip) else {
        return Ok(None);
    };
    /* 夹到截图范围内（窗口可能有一半在屏幕外） */
    let l = rx.max(sx);
    let t = ry.max(sy);
    let r = (rx + rw).min(sx + sw);
    let b = (ry + rh).min(sy + sh);
    if r <= l || b <= t {
        return Ok(None);
    }
    Ok(Some(WinBox {
        x: (l - sx) as f64 / sc,
        y: (t - sy) as f64 / sc,
        w: (r - l) as f64 / sc,
        h: (b - t) as f64 / sc,
    }))
}

/// 收下前端合成好的整张图（含标注），按 `action` 处理：
/// `copy` 进剪贴板 / `save` 存到截图目录 / `saveas` 让用户选位置 / `peek` 直接贴到屏幕上。
///
/// 顺序很重要：**先落盘/进剪贴板，成功了才关窗**。反过来的话任何一步失败都表现为
/// "窗口一闪就没了、什么都没留下"，用户完全无从判断（0.9.x 就是这么被坑的）。
#[tauri::command]
async fn am_snip_finish(
    app: AppHandle,
    png: String,
    action: String,
    from: Option<String>,
    place: Option<Vec<f64>>,
) -> Result<Option<String>, String> {
    /* 闸门（只拦 `ctrl+c` / `ctrl+s` 这两条快捷键）：
       本机有程序会向前台窗口注入 Ctrl+C（`core/src/keywatch.rs` 抓到同毫秒的
       `0xa2` + `C`，两条都带 INJECTED）。判据是「最近 1.2s 内真人按的是不是这颗键」。

       **挡下来必须静默**：不关窗、不写剪贴板、不落盘，也**不往界面弹任何东西**
       （0.1.25 那次弹了个红条、整块覆盖层变红，owner 明确说"这种是什么情况，从源头解决"）。
       用户看到的应该是"这次 Ctrl+C 没有任何反应"，而不是一个报错。
       按钮 / 中键是鼠标动作，不经过这里，不受影响。 */
    let shortcut_vk = match from.as_deref() {
        Some("ctrl+c") => Some(0x43u32), // 'C'
        Some("ctrl+s") => Some(0x53u32),           // 'S'
        Some("ctrl+shift+s") => Some(0x53u32),     // 'S'（另存为）
        _ => None,
    };
    if let Some(vk) = shortcut_vk {
        let (inst, seen, last) = keywatch::state();
        let pass = keywatch::real_key_matches(vk, 1200);
        if let Some(d) = LOG_DIR.get() {
            log_runtime(
                d,
                &format!(
                    "snip gate vk={vk:#x} pass={pass} installed={inst} seen={seen} tid={} last={last:?}",
                    keywatch::hook_tid()
                ),
            );
        }
        if !pass {
            if let Some(d) = LOG_DIR.get() {
                log_runtime(
                    d,
                    &format!(
                        "snip ignore injected action={action} from={}（不是真人按键，静默丢弃）",
                        from.as_deref().unwrap_or("?")
                    ),
                );
            }
            return Ok(None);
        }
    }
    /* 一行审计：谁触发的交付。这个软件出过两次「截完自动复制」，两次都是靠回看
       某一个入口才定位到的 —— 所以每个交付都记，别只在 copy 记。
       0.1.27 起 `from` 可能是 `ctrl+c` / `ctrl+s` / `btn` / `middle-click`。 */
    if let Some(d) = LOG_DIR.get() {
        log_runtime(
            d,
            &format!(
                "snip finish action={action} from={}",
                from.as_deref().unwrap_or("?")
            ),
        );
    }
    /* 贴图要"摆回原处"：`[x, y, w, h]` 全是**物理像素**（虚拟桌面坐标）。
       前端从 boot 的窗口原点 + 选区 CSS 坐标 × scale 算出来，形状不对就当没有。 */
    let place_rect = place.and_then(|p| {
        if p.len() == 4 && p[2] > 2.0 && p[3] > 2.0 {
            Some([p[0], p[1], p[2], p[3]])
        } else {
            None
        }
    });
    let bytes = png_from_b64(&png)?;
    /* 这条会话到此结束：正在淡入的收尾线程不许再动窗口 */
    SNIP_GEN.fetch_add(1, Ordering::SeqCst);

    let win = app.get_webview_window("snip");
    let close_snip = || {
        /* 卸掉「真人按键」监视钩子。卸载动作由钩子线程自己做（它收到 STOP 就
           Unhook 并退出），这里只是叫它停 —— 扔后台，别让收尾命令等它。 */
        std::thread::spawn(keywatch::uninstall);
        /* 底图必须一起撤：它是全屏 topmost 的原生窗口，忘了会一直挡着桌面 */
        splash_close();
        if let Some(w) = app.get_webview_window("snip") {
            let _ = w.close();
        }
        let tmp = {
            let mut g = snip_slot().lock().unwrap_or_else(|e| e.into_inner());
            g.take().map(|s| s.tmp).unwrap_or_default()
        };
        if !tmp.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&tmp);
        }
    };

    match action.as_str() {
        "copy" => {
            clipimg::set_png(&bytes).map_err(|e| {
                if let Some(d) = LOG_DIR.get() {
                    log_runtime(d, &format!("snip copy FAIL {e}"));
                }
                e
            })?;
            close_snip();
            /* 复制是**没有痕迹**的动作：截完窗一关、剪贴板里有什么用户看不见，
               很容易以为"什么都没发生"。给主窗一个事件，让它弹一句「已复制」。 */
            let _ = app.emit("shot://copied", ());
            Ok(None)
        }
        "peek" => {
            /* 截图贴图：先落到**截图目录**，再收进软件回收站（owner 2026-11-19）。
               "贴图"是临时摆上屏幕的动作，不该在截图目录里堆一堆；
               想留底就从回收站还原（默认回 screenshots，也可以还到指定目录）。 */
            let dir = shot_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("建不了截图目录：{e}"))?;
            let raw = shot_target(&dir, &today_day());
            std::fs::write(&raw, &bytes).map_err(|e| format!("写贴图失败：{e}"))?;
            let path = trash_png(&app, &raw);
            let Some(item) = item_of_file(&path) else {
                return Err("贴图生成失败".into());
            };
            let rec = PeekRecall {
                label: next_label(),
                kind: "file".into(),
                path: path.to_string_lossy().to_string(),
                long_edge: PEEK_LONG_EDGE,
                /* 截图贴图：**与选区同尺寸、同位置**（owner 2026-11-18）。
                   有它就绕开"按长边等比缩放 + 居中到鼠标"那套（那是给素材浏览用的）。 */
                place: place_rect,
                ..Default::default()
            };
            peeks().lock().unwrap_or_else(|e| e.into_inner()).push(rec.clone());
            save_peeks();
            // 先关覆盖层再开贴图：两个都是置顶窗，顺序反了贴图会被压在截图底下
            close_snip();
            /* 这里**也不碰剪贴板**：贴图是"把它放到屏幕上"，不是"把它复制下来"。
               想复制有明确的「复制」按钮 / Enter / C。 */
            spawn_peek(&app, item, rec, false).await?;
            Ok(None)
        }
        _ => {
            let dir = shot_dir();
            std::fs::create_dir_all(&dir).map_err(|e| {
                if let Some(d) = LOG_DIR.get() {
                    log_runtime(d, &format!("snip mkdir FAIL {dir:?} {e}"));
                }
                format!("建不了截图目录 {}：{e}", dir.display())
            })?;
            let day = today_day();
            let default_name = format!("截图 {day}.png");
            let target = if action == "saveas" {
                /* 覆盖层是置顶窗：弹对话框前必须**先取消置顶**，
                   否则文件对话框被压在它下面 —— 用户看到的就是"点了保存什么都没发生"
                   （owner 2026-11-17 反馈）。 */
                if let Some(w) = &win {
                    let _ = w.set_always_on_top(false);
                }
                let picked = rfd::AsyncFileDialog::new()
                    .set_directory(&dir)
                    .set_file_name(&default_name)
                    .save_file()
                    .await;
                if let Some(w) = &win {
                    let _ = w.set_always_on_top(true);
                }
                match picked {
                    Some(h) => h.path().to_path_buf(),
                    None => return Ok(None), // 用户取消：留在覆盖层上继续编辑
                }
            } else {
                /* 同一天多张不覆盖：`截图 2026-11-21 (2).png` */
                shot_target(&dir, &day)
            };
            std::fs::write(&target, &bytes).map_err(|e| {
                if let Some(d) = LOG_DIR.get() {
                    log_runtime(d, &format!("snip write FAIL {target:?} {e}"));
                }
                format!("写截图失败 {}：{e}", target.display())
            })?;
            let saved = target.to_string_lossy().to_string();
            close_snip();
            /* 保存**不动**剪贴板（owner 2026-11-18 报的 bug：「截图完成后老是会直接自动复制」）。
               0.10.0 曾经照 Snipaste 顺手塞一份 PNG —— 后果是用户保存一张截图，
               剪贴板里原来的东西就被静默换掉了，等他要粘别的内容才发现。
               「复制」是一个有明确后果的动作，只能由用户按 C / Enter / 点复制按钮显式触发。 */
            remember_shot_dir(&app);
            let _ = app.emit("shot://saved", saved.clone());
            Ok(Some(saved))
        }
    }
}

/// 前端把底图解码完、画好第一帧之后调这个 —— 覆盖窗这时才真正显示。
///
/// 为什么要这一来一回：`snip` 窗建的时候是 `visible(false)`（见 `snip_begin`），
/// 早显示 200ms 就是一片黑（用户报的「先黑屏一下」）。窗口一直不显示更糟：
/// 用户按了 F1 像没反应。所以显示时机交给前端：图上屏了才 show。
/// 截图会话代次。每开始 / 取消一次就 +1。
///
/// 用来让"迟到的异步收尾"失效：`am_snip_ready` 里那条线程会睡 200ms 再淡入、
/// 再撤底图。如果这期间用户已经取消了（甚至又按了一次 F1 开了新的），
/// 旧的收尾线程会把**新**的那块底图撤掉 / 把新窗口设成半透明 —— 表现就是
/// "刚按 F1 画面一闪就没了"。所有跨线程收尾都先比一次代次。
static SNIP_GEN: AtomicU64 = AtomicU64::new(0);

/// 本次截图按 F1 的时刻（只给日志计时用，不参与任何判断）。
static SNIP_T0: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

#[tauri::command]
async fn am_snip_ready(app: AppHandle) -> Result<(), String> {
    /* 0.2.4 起这里**不再负责揭幕**：揭幕由 `snip_begin` 里那条协调线程统一管
       （它还要兼顾"用户正在拖框就别揭"、"Esc 要能取消"）。这条命令留着是因为
       前端首帧就绪时仍会调一次，用来在日志里对时间。 */
    let _ = app;
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, "snip ready（前端首帧就绪，等协调线程交接）");
    }
    Ok(())
}

#[tauri::command]
async fn am_snip_cancel(app: AppHandle) -> Result<(), String> {
    snip_abort(&app);
    Ok(())
}

/// 整场取消（命令和原生层的 Esc/右键共用一份逻辑）。
fn snip_abort(app: &AppHandle) {
    /* 取消也算一次会话结束：协调线程必须立刻停手 */
    SNIP_GEN.fetch_add(1, Ordering::SeqCst);
    /* 会话结束：卸掉「真人按键」监视钩子（扔后台，不挡取消） */
    std::thread::spawn(keywatch::uninstall);
    let tmp = {
        let mut g = snip_slot().lock().unwrap_or_else(|e| e.into_inner());
        g.take().map(|s| s.tmp).unwrap_or_default()
    };
    /* 顺序：先撤原生底图 → 再把覆盖层藏起来（用户这一刻就回到桌面了）→
       最后才真关窗。关窗放到独立线程里：这条命令本身还在这个 webview 的 IPC 里跑，
       就地 close 会让 webview 在等响应的时候被拆掉（偶发卡死）。 */
    splash_close();
    if let Some(win) = app.get_webview_window("snip") {
        let _ = win.hide();
    }
    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        if let Some(w) = app2.get_webview_window("snip") {
            let _ = w.close();
        }
        if !tmp.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&tmp);
        }
        /* 焦点还给主窗：取消完还停在别处的话，用户得先点一下窗口才能继续用 */
        if let Some(m) = app2.get_webview_window("main") {
            let _ = m.set_focus();
        }
    });
}

/// 开一次截图。**没有 mode**：截图界面永远是同一个，交付（复制 / 保存 / 贴图）
/// 只能由用户在界面里显式触发 —— 见 `am_snip_finish` 的注释。
#[tauri::command]
async fn am_snip_start(app: AppHandle) -> Result<(), String> {
    snip_begin(app).await
}

/* ---------------- 系统级快捷键（F1 / F3 / Alt+F1） ---------------- */

static HOTKEYS: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();

fn hotkey_slot() -> &'static Mutex<Vec<(String, String)>> {
    HOTKEYS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 贴图是否被"一键隐起来"了（Alt+F3 的开关状态）
static PEEK_HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Alt+F3：把屏幕上所有贴图**整体**隐起来 / 放出来。
///
/// 为什么是"全局一键"而不是逐张：贴图常常是临时挡在文档上的，需要对一下底下的东西时
/// 一下全收走、看完再一下全放回 —— 逐张点太慢，而且中途忘了哪张没收。
fn toggle_peeks(app: &AppHandle) {
    let hide = !PEEK_HIDDEN.load(Ordering::SeqCst);
    let mut n = 0usize;
    for (l, w) in app.webview_windows() {
        if !l.starts_with("peek") {
            continue;
        }
        let ok = if hide { w.hide().is_ok() } else { w.show().is_ok() };
        if ok {
            n += 1;
        }
    }
    PEEK_HIDDEN.store(hide, Ordering::SeqCst);
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("peek toggle hide={hide} n={n}"));
    }
}

/// 新贴一张之前，先解除"整批隐藏"状态。
///
/// 场景（owner 2026-11-19）：用 Alt+F3 把贴图全收走了，接着按 F3 又贴一张新的。
/// 新窗照常出现（用户刚要求看到的），但旧的那批还藏着 —— 此时 `PEEK_HIDDEN` 仍是 true，
/// 下一次 Alt+F3 会走「隐藏」分支，用户按半天也唤不回旧的（看着就是"键坏了"）。
/// 所以贴新图 = 回到"全部可见"，把状态清掉；调用点拿到的是**创建新窗之前**的窗口集合。
fn release_peek_hide(app: &AppHandle) {
    if !PEEK_HIDDEN.swap(false, Ordering::SeqCst) {
        return;
    }
    for (l, w) in app.webview_windows() {
        if l.starts_with("peek") {
            let _ = w.show();
        }
    }
}

fn dispatch_hotkey(app: &AppHandle, accel: &str) {
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("hotkey fired {accel}"));
    }
    let action = hotkey_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(a, _)| a.eq_ignore_ascii_case(accel))
        .map(|(_, b)| b.clone());
    let Some(action) = action else {
        if let Some(d) = LOG_DIR.get() {
            log_runtime(d, &format!("hotkey {accel} -> 没有对应动作（表里没有）"));
        }
        return;
    };
    let app = app.clone();
    // **必须 spawn**：快捷键回调在主线程上，而开窗口 / 弹对话框都要回主线程，直接做就是自死锁
    tauri::async_runtime::spawn(async move {
        let r = match action.as_str() {
            "snip" => snip_begin(app).await,
            "clip_peek" => clip_peek_begin(app).await,
            "peek_toggle" => {
                toggle_peeks(&app);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = r {
            if let Some(d) = LOG_DIR.get() {
                log_runtime(d, &format!("hotkey {action} FAIL {e}"));
            }
        }
    });
}

/// 注册串行化。前端一次启动会调两遍（StrictMode 双跑），两个命令交错执行时
/// 会出现「A unregister → B unregister → A register ok → B register 被自己占掉」，
/// 于是表被清空、按 F1 变成 `没有对应动作`（2026-11-17 实测日志里就是这个）。
/// 用原子标志而不是 Mutex：**不能持 std 锁跨 await**，而这条命令必须留在 async 里跑
/// （同步命令会占住 IPC 线程，热键注册就收不到消息了 —— 实测过，见 WORKLOG）。
static REG_BUSY: AtomicBool = AtomicBool::new(false);

/// 设置面板改键后调这个：先全注销再按新表注册。返回注册失败的 accelerators。
#[tauri::command]
async fn am_hotkeys_set(app: AppHandle, list: Vec<(String, String)>) -> Result<Vec<String>, String> {
    while REG_BUSY.swap(true, Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(4));
    }
    let out = register_hotkeys(&app, list);
    REG_BUSY.store(false, Ordering::SeqCst);
    out
}

fn register_hotkeys(app: &AppHandle, list: Vec<(String, String)>) -> Result<Vec<String>, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    // 幂等：表没变就别折腾。反复 unregister/register 在 Windows 上会偶发
    // "刚注销的键还被认为占用"，直接把 F1 弄成永久失效。
    let want: Vec<(String, String)> = list
        .iter()
        .map(|(a, b)| (a.trim().to_string(), b.clone()))
        .filter(|(a, _)| !a.is_empty())
        .collect();
    {
        let cur = hotkey_slot().lock().unwrap_or_else(|e| e.into_inner());
        if !cur.is_empty() && *cur == want {
            return Ok(Vec::new());
        }
    }
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let mut ok_list: Vec<(String, String)> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for (a, action) in want {
        match a.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            Ok(sc) => match gs.register(sc) {
                Ok(_) => ok_list.push((a, action)),
                Err(_) => failed.push(a),
            },
            Err(_) => failed.push(a),
        }
    }
    if let Some(d) = LOG_DIR.get() {
        let ok: Vec<String> = ok_list.iter().map(|(a, act)| format!("{a}={act}")).collect();
        log_runtime(d, &format!("hotkeys ok=[{}] failed=[{}]", ok.join(", "), failed.join(", ")));
    }
    *hotkey_slot().lock().unwrap_or_else(|e| e.into_inner()) = ok_list;
    Ok(failed)
}

/* ---------------- peekfile:// 协议 ---------------- */

fn serve_media(req_path: &str) -> Response<Vec<u8>> {
    let token: u64 = req_path.trim_start_matches('/').parse().unwrap_or(0);
    let path = {
        let m = MEDIA.get_or_init(|| Mutex::new(HashMap::new()));
        let g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.get(&token).cloned()
    };
    let Some(path) = path else { return not_found() };
    match std::fs::read(&path) {
        Ok(bytes) => Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, mime_of(&path))
            .header(header::CACHE_CONTROL, "no-store")
            .body(bytes)
            .unwrap(),
        Err(_) => not_found(),
    }
}

/* ---------------- 单实例守卫 ----------------

   **为什么必须有**：0.1.21 及以前没有这道闸。二次启动会起第二个进程，两个进程各自
   `App::open` 同一个 `index.db` / `libs\lib-*.db`。SQLite 的 WAL + `busy_timeout` 能扛住
   单点写竞争，但**两边同时跑扫描 / 重算 `dirs.cnt_all`** 时会互相覆盖 —— 表现是"扫完
   侧栏计数忽多忽少"，查起来像索引坏了。用户场景很普通：双击两次图标、或点完没反应再点一次。

   做法：命名互斥体（`Local\` = 每个登录会话一把，不跨用户串味）。已存在 → 把那个主窗
   拉到前台，本进程直接退出；找不到窗（崩在启动早期）→ 弹一句人话。

   0.4.2 起同一套机制**多担一件事**：双击 `.L/.S/.P-AssetManager` 时 Windows 会按文件
   关联再起一个进程并传 `--open "<文件>"`。这个进程把路径写进请求文件 + `SetEvent`，
   首实例收到后 emit `am://open-file` 交给前端（细节见 `single_instance.rs`）。

   为什么不用 `tauri-plugin-single-instance`：它要多引一个 crate（本地 registry 没有缓存，
   得联网拉），而这里要的只是「别起第二个」+「递个路径」。十几个 syscall，零依赖。 */

/// 把已经在跑的那个主窗拉到前台。返回 false 表示没找到窗（已弹提示）。
#[cfg(windows)]
fn focus_running_window() -> bool {
    type Hwnd = isize;
    extern "system" {
        fn EnumWindows(cb: extern "system" fn(Hwnd, isize) -> i32, l: isize) -> i32;
        fn IsWindowVisible(h: Hwnd) -> i32;
        fn GetWindowTextW(h: Hwnd, buf: *mut u16, max: i32) -> i32;
        fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
        fn SetForegroundWindow(h: Hwnd) -> i32;
        fn MessageBoxW(h: Hwnd, text: *const u16, cap: *const u16, ty: u32) -> i32;
    }
    extern "system" fn pick(hwnd: Hwnd, lparam: isize) -> i32 {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if n <= 0 {
                return 1;
            }
            // 主窗标题是「AssetManager — <当前目录>」；贴图窗没有标题，板窗是「灵感板…」，
            // 所以按前缀认主窗是安全的
            let title = String::from_utf16_lossy(&buf[..n as usize]);
            if title.starts_with("AssetManager") {
                *(lparam as *mut Hwnd) = hwnd;
                return 0; // 找到即停止枚举
            }
            1
        }
    }
    unsafe {
        let mut found: Hwnd = 0;
        EnumWindows(pick, &mut found as *mut Hwnd as isize);
        if found == 0 {
            let text: Vec<u16> =
                "AssetManager 已经在运行。\r\n\r\n请从任务栏切换过去 —— 同一份索引不能两个进程同时写。\0"
                    .encode_utf16()
                    .collect();
            let cap: Vec<u16> = "AssetManager\0".encode_utf16().collect();
            const MB_ICONINFORMATION: u32 = 0x40;
            MessageBoxW(0, text.as_ptr(), cap.as_ptr(), MB_ICONINFORMATION);
            return false;
        }
        const SW_RESTORE: i32 = 9;
        ShowWindow(found, SW_RESTORE);
        SetForegroundWindow(found);
        true
    }
}

#[cfg(not(windows))]
fn focus_running_window() -> bool {
    false
}

fn main() {
    /* 单实例：必须在 `tauri::Builder` 之前 —— Builder 一跑就会 open 索引。
       命令行里可能带着要打开的导出文件（双击文件 = Windows 给个 `--open "<路径>"`）。 */
    let initial_file = single_instance::argv_file();
    if !single_instance::claim(initial_file.as_deref()) {
        #[cfg(windows)]
        {
            focus_running_window();
        }
        return;
    }
    tauri::Builder::default()
        .manage(WatchState(Mutex::new(None)))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        dispatch_hotkey(app, &shortcut.to_string());
                    }
                })
                .build(),
        )
        /* 开机自启（写 HKCU\...\Run）。参数为空：不需要额外的启动参数，
           正常启动就是正常启动 —— 要不要最小化由 `ui-prefs.json` 决定，不靠命令行。 */
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None::<Vec<&str>>,
        ))
        .setup(move |app| {
            /* 数据目录可配置（照 FontManager 的「字体库文件夹」）：
               `%LOCALAPPDATA%\AssetManager\data-root.json` 记着自定义位置；
               自定义目录不可用（盘拔了）时 resolve 会回退默认目录，不让软件打不开。 */
            let default_dir = app
                .path()
                .app_local_data_dir()
                .map_err(|e| format!("拿不到数据目录：{}", e))?;
            let data = am_core::dataroot::resolve(&default_dir);
            /* 运行日志：这个 exe 没有 stdout，出问题只能靠它（见 log_runtime 的注释）。
               panic hook 只记「哪一行 panic 了」，不吞掉 panic —— 该崩还是崩，
               但我们事后能知道崩在哪。 */
            let _ = LOG_DIR.set(default_dir.clone());
            log_runtime(&default_dir, &format!("start v{} data={}", env!("CARGO_PKG_VERSION"), data.display()));

            /* ---- 主窗口（0.4.13 起从 tauri.conf.json 搬到 Rust）----
               唯一目的是给 AM_CDP=1 一个开关：设了就给 WebView2 挂
               --remote-debugging-port=9222，供 CDP 真鼠标注入复现拖拽问题。
               不设则完全等同原来的配置窗口，发布行为不变。 */
            let cdp = std::env::var("AM_CDP").as_deref() == Ok("1");
            let mut main_b = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("AssetManager")
            .inner_size(1280.0, 820.0)
            .min_inner_size(900.0, 600.0)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .center()
            .resizable(true);
            if cdp {
                main_b = main_b.additional_browser_args(CDP_ARGS);
            }
            if let Err(e) = main_b.build() {
                log_runtime(&default_dir, &format!("main window build FAIL (cdp={cdp}): {e}"));
                return Err(Box::new(e));
            }

            std::panic::set_hook(Box::new(|info| {
                let dir = LOG_DIR.get().cloned().unwrap_or_else(|| PathBuf::from("."));
                let loc = info
                    .location()
                    .map(|l| format!("{}:{}", l.file(), l.line()))
                    .unwrap_or_else(|| "?".into());
                let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| {
                    info.payload().downcast_ref::<String>().cloned()
                });
                log_runtime(&dir, &format!("PANIC at {loc}: {}", msg.unwrap_or_default()));
            }));
            let st = open_app(&data).map_err(|e| format!("打开索引失败：{}", e))?;
            // 旧版 JPEG 缩略图清一遍（后台跑，别卡首屏；删不掉的留到下次）
            let store = thumb::ThumbStore::new(st.thumbs.root.clone());
            std::thread::spawn(move || {
                let (files, bytes) = thumb::purge_legacy(&store);
                if files > 0 {
                    eprintln!("清理旧 JPEG 缩略图：{} 个 / {:.1} MB", files, bytes as f64 / 1048576.0);
                }
            });
            app.manage(AppState(Mutex::new(st)));

            /* ---- 双击导入的三块地基（0.4.2） ----
               ① 文件关联：每次都对齐（覆盖安装后 exe 路径变了，靠它自动修好）；
               ② 单实例转发：首实例起线程等「再点一次图标 / 双击文件」的信号；
               ③ 启动参数：软件关着时双击文件，Windows 把路径当 argv 给了我们。
               三件都不该阻断启动：写不进注册表也只是"双击不好使"，软件照样能用。 */
            {
                #[cfg(windows)]
                match file_assoc::ensure_associations() {
                    Ok(true) => log_runtime(&default_dir, "文件关联：已更新"),
                    Ok(false) => {}
                    Err(e) => log_runtime(&default_dir, &format!("文件关联失败（不影响使用）：{e}")),
                }
                single_instance::watch(app.handle().clone());
                if let Some(f) = initial_file.clone() {
                    single_instance::open_initial_file(app.handle().clone(), f);
                }
            }

            /* ---- 托盘 / 关闭到托盘 / 启动时最小化 / 界面缩放（0.3.30） ----
               这一块必须紧跟 `manage(AppState)`：`read_ui_prefs` 要靠 AppState 里的 data_dir。 */
            {
                let h = app.handle().clone();
                let ui = read_ui_prefs(&h);
                let tray_ok = match build_tray(&h) {
                    Ok(()) => true,
                    Err(e) => {
                        log_runtime(&default_dir, &format!("tray build FAIL: {e}"));
                        false
                    }
                };
                /* 启动时缩放：`uiScale = None` 走自动档。放在窗口显示之前，
                   否则用户会看到"先是 100%，随后跳成 80%"那一下。 */
                if let Some(w) = app.get_webview_window("main") {
                    let _ = am_set_ui_scale(w.clone(), ui.ui_scale);
                }
                /* 启动时最小化：直接不显示主窗口（托盘里点一下再出来） */
                if ui.start_minimized && tray_ok {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                }
                /* 关闭窗口 → 收进托盘。`tray_ok` 是前提：托盘没建成就藏起来，
                   用户既看不到窗口也找不到托盘图标，只能去任务管理器杀进程 —— 比"关不掉"更糟。 */
                if let Some(w) = app.get_webview_window("main") {
                    let h2 = h.clone();
                    w.on_window_event(move |event| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                            if tray_ok && read_ui_prefs(&h2).close_to_tray {
                                api.prevent_close();
                                if let Some(w) = h2.get_webview_window("main") {
                                    let _ = w.hide();
                                }
                            }
                        }
                    });
                }
            }

            /* 内置截图库：**启动就登记**。owner 2026-11-17 的原话是「要随时可以显示的」——
               它不能等用户截过一张图才出现在侧栏。放后台线程：目录里截图攒到几千张时
               扫描要几百毫秒，别去拖首屏。登记完发一次 fs://changed，侧栏自己会重读库列表。 */
            {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(600));
                    remember_shot_dir(&h);
                    let _ = h.emit("fs://changed", Vec::<String>::new());
                });
            }

            // 启动增量扫描（T06）：等界面先画出来，再拿索引里记的 mtime 比对磁盘。
            // 不递归、不全量，所以几万目录也就一两秒；有变化才推 fs://changed。
            let handle = app.handle().clone();
            let data2 = data.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(2500));
                let Ok(mut a) = open_app(&data2) else { return };
                let cancel = AtomicBool::new(false);
                if let Ok(s) = a.incremental(&cancel, 4000, |_p| {}) {
                    if s.dirs + s.removed_dirs > 0 {
                        eprintln!(
                            "启动增量扫描：{} 个目录有变化，{} 个文件，{} ms",
                            s.dirs + s.removed_dirs,
                            s.files,
                            s.elapsed_ms
                        );
                        let _ = handle.emit("fs://changed", Vec::<String>::new());
                    }
                }
                /* 顺手补一批主色板。为什么放这儿：0.8.0 的色板只写进了预览那条路径，
                   65k 张图的 palette 全是 NULL（顶栏颜色筛选因此一直是空的）。
                   现在每次启动补 1500 张（读的是已缓存的 256 档缩略图，1–3ms/张），
                   几十次启动就补齐了，也不占用户时间。 */
                if let Ok((done, _left)) = a.fill_palette(1500) {
                    if done > 0 {
                        let _ = handle.emit("fs://changed", Vec::<String>::new());
                    }
                }
            });

            /* 恢复上次没关的贴图（F3 贴的参考图、截图……）。
               等主窗先画出来再开：一上来糊十几扇窗，用户会以为软件出问题了。
               必须走 async runtime（建窗口不能在主线程上等自己）。 */
            let h = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(900));
                tauri::async_runtime::spawn(async move { restore_peeks(h).await });
            });
            /* 自测入口（只 debug）：`AM_AUTO_SNIP=1` 启动 4 秒后自动开一次截图。
               为什么要这个：F1 是系统级热键，被 Snipaste 之类用低级钩子的软件抢走后
               自动化脚本按不动（RegisterHotKey 成功但收不到 WM_HOTKEY），
               截图覆盖层就永远没法自动验证。**release 包里没有这段**。 */
            #[cfg(debug_assertions)]
            if std::env::var("AM_AUTO_SNIP").is_ok() {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(4000));
                    tauri::async_runtime::spawn(async move {
                        let _ = snip_begin(h).await;
                    });
                });
            }
            /* 自测入口（只 debug）：`AM_AUTO_BOARD=<标签名>` 启动 4 秒后自动开一块灵感板。
               标签面板的右键菜单在自动化里很难点准（要先展开侧栏再找准行），
               所以给板窗口留一个直接入口。 */
            #[cfg(debug_assertions)]
            if let Ok(tag) = std::env::var("AM_AUTO_BOARD") {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(4000));
                    tauri::async_runtime::spawn(async move {
                        // 走和菜单完全一样的那条命令（不另写一份逻辑）
                        let st = h.state::<AppState>();
                        let _ = am_board_open(h.clone(), st, tag, None).await;
                    });
                });
            }
            Ok(())
        })
        // 缩略图走自定义协议：字节永不经过 IPC（IPC 会序列化成 JSON 数组）
        //
        // ⚠️ 锁的粒度：以前整个请求（含解码 + 读文件 + 写库）都握着全局 `Mutex<App>`，
        // 一张 36MP 的图解码 0.5 秒，这 0.5 秒里 am_list / am_children / 详情全部排在后面 ——
        // 滚到一屏新图就"卡住"。现在拆成三段：
        //   ① 短锁：只取「路径 + 要不要回写」，微秒级；
        //   ② 不持锁：查缓存 / 解码 / 编码（几十到几百毫秒，互不阻塞）；
        //   ③ 短锁：仅在 key 或尺寸真的变了时写一行 —— 缓存命中是**零写入**。
        .register_uri_scheme_protocol("peekfile", |_ctx, req| serve_media(req.uri().path()))
        .register_uri_scheme_protocol("thumb", |ctx, req| {
            let state = ctx.app_handle().state::<AppState>();
            let id: i64 = req
                .uri()
                .path()
                .trim_start_matches('/')
                .trim_end_matches(".jpg")
                .parse()
                .unwrap_or(0);
            if id <= 0 {
                return not_found();
            }
            let px = parse_query(req.uri().query());
            // 给预热线程一个"有人在等我"的信号（见 am_warm_start 的让路逻辑）
            LAST_THUMB_REQ.store(now_ms(), Ordering::Relaxed);

            // ① 短锁
            let (path, store, need_dims, known_key, known_state, db_px) = {
                let app = lock(&state);
                let Ok(Some(a)) = app.asset(id) else {
                    return not_found();
                };
                (
                    PathBuf::from(&a.path),
                    thumb::ThumbStore::new(app.thumbs.root.clone()),
                    a.width.unwrap_or(0) == 0 || a.height.unwrap_or(0) == 0,
                    a.thumb_key.clone(),
                    a.thumb_state,
                    a.width.unwrap_or(0) as u64 * a.height.unwrap_or(0) as u64,
                )
            };

            if !path.is_file() {
                // 只标一次：已经是 FAILED 的不再重复写库（缺文件时每滚一次写一次纯属浪费）
                if known_state != am_core::model::THUMB_FAILED {
                    let mut app = lock(&state);
                    let _ = app.mark_thumb_state(id, am_core::model::THUMB_FAILED);
                }
                return not_found();
            }

            /* `px=0` = 要**原图**（不是缩略图）。
               长截图 / 全景图铺满一屏时，1600 档的缩略图也才一百多像素宽，放大就是一坨；
               原文件本来就是 1080×11942，直接给原图更小更快。这条不占解码配额。
               超过 48 MB 的（巨型 PNG / PSD 导出）退回 1600 档 —— 一次读几十上百兆不值当。 */
            if px == 0 {
                let big = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 48 * 1024 * 1024;
                if !big {
                    return match std::fs::read(&path) {
                        Ok(bytes) => Response::builder()
                            .status(200)
                            .header(header::CONTENT_TYPE, mime_of(&path))
                            .header(header::CACHE_CONTROL, "no-store")
                            .body(bytes)
                            .unwrap(),
                        Err(_) => not_found(),
                    };
                }
            }
            let px = if px == 0 { 1600 } else { px };

            let _slot = acquire_decode(decode_cost(&path, px, db_px));

            // ② 不持锁：这里可能是几十到几百毫秒
            match thumb::ensure_path(&path, &store, px) {
                Ok(o) => {
                    // ③ 短锁回写：命中缓存时 known_key 早就等于 o.key，一行都不写
                    if known_key.as_deref() != Some(o.key.as_str()) || need_dims {
                        let mut app = lock(&state);
                        let _ = app.set_thumb(
                            id,
                            &o.key,
                            am_core::model::THUMB_OK,
                            o.quality,
                            o.bytes,
                        );
                        if need_dims {
                            let _ = app.set_dims_if_missing(id, o.width, o.height);
                        }
                        /* **主色板 / 指纹必须在这里写**：缩略图主路径是
                           `ensure_path`（只出文件、不碰库），写库由这一层做。
                           0.8.0 只把 palette 写进了 `ensure_for_asset`（那条路只有预览/预热走），
                           结果 65k 张图 palette 全是 NULL —— 详情里的色板一栏永远不出现，
                           顶栏按颜色筛也永远是空的。教训：**"算出来了"和"存进去了"是两件事**，
                           两条写库路径都得写。 */
                        if !o.palette.is_empty() {
                            let _ = app.set_palette(id, &o.palette, &o.color);
                        }
                        if o.phash != 0 {
                            let _ = app.set_phash(id, o.phash);
                        }
                    }
                    match std::fs::read(&o.path) {
                        Ok(bytes) => Response::builder()
                            .status(200)
                            .header(header::CONTENT_TYPE, "image/webp")
                            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                            .body(bytes)
                            .unwrap(),
                        Err(_) => not_found(),
                    }
                }
                Err(_) => {
                    // 「不支持」只在两条路都走不通时才成立：自己解不了 **且** 系统也没给缩略图
                    let ext = path
                        .extension()
                        .map(|e| e.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    let unsupported = !am_core::formats::builtin_thumbable(&ext)
                        && !am_core::formats::shell_thumbable(&ext);
                    let mut app = lock(&state);
                    let _ = app.mark_thumb_state(
                        id,
                        if unsupported {
                            am_core::model::THUMB_UNSUPPORTED
                        } else {
                            am_core::model::THUMB_FAILED
                        },
                    );
                    not_found()
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            am_roots,
            am_set_rating,
            am_set_note,
            am_set_source,
            am_open_url,
            am_update_check,
            am_update_download,
            am_update_install,
            am_lib_count,
            am_builtin_roots,
            am_remove_lib,
            am_set_current_lib,
            am_children,
            am_list,
            am_total,
            am_tags,
            am_auto_tags,
            am_auto_tags_for,
            am_asset_tags,
            am_tag_create,
            am_tag_assets,
            am_untag_assets,
            am_delete_tag,
            am_tag_groups,
            am_tag_set_color,
            am_tag_set_group,
            am_tag_rename,
            am_group_new,
            am_group_rename,
            am_group_set_color,
            am_group_delete,
            am_scan,
            am_scan_start,
            am_scan_cancel,
            am_scan_busy,
            am_incremental,
            am_watch,
            am_drag_out,
            am_rebuild_index,
            am_export_userdata,
            am_pick_folder,
            am_open_recycle,
            am_trash_list,
            am_trash_media,
            /* 界面偏好 / 托盘 / 自启 / 缩放（0.3.30） */
            am_ui_prefs_get,
            am_ui_prefs_set,
            am_set_ui_scale,
            am_set_autostart,
            am_get_autostart,
            am_trash_restore,
            am_trash_purge,
            am_trash_empty,
            am_reveal,
            am_open_path,
            am_copy_files,
            am_resize_save,
            am_exif,
            am_rename,
            am_mkdir,
            am_move,
            am_move_start,
            am_move_plan,
            am_stat_paths,
            am_delete,
            am_undo,
            am_cache_stat,
            am_cache_gc,
            am_assets_by_ids,
            am_peek_open,
            am_peek_zoom,
            am_peek_ratio,
            am_peek_close,
            am_peek_top,
            am_peek_opacity,
            am_peek_set_frame,
            am_peek_track,
            am_peek_copy_file,
            am_peek_save_lib,
            am_ui_theme,
            am_clip_text,
            am_clip_peek,
            am_clip_history,
            am_snip_start,
            am_snip_crop,
            am_snip_full,
            am_snip_window_at,
            am_cursor_step,
            am_snip_finish,
            am_snip_cancel,
            am_snip_ready,
            am_peek_ready,
            am_board_open,
            am_board_close,
            am_board_layout,
            am_board_place,
            am_board_notes,
            am_board_note_put,
            am_board_note_del,
            am_board_group,
            am_board_add_paths,
            am_board_paste,
            am_coll_list,
            am_coll_create,
            am_coll_rename,
            am_coll_delete,
            am_coll_add,
            am_coll_add_tag,
            am_coll_remove,
            am_coll_of,
            am_coll_open,
            am_coll_paste,
            am_snapshot_list,
            am_snapshot_create,
            am_snapshot_rename,
            am_snapshot_delete,
            am_snapshot_add,
            am_snapshot_add_tag,
            am_snapshot_combine,
            am_snapshot_resolve,
            am_snapshot_remove,
            am_snapshot_export,
            am_snapshot_import,
            am_snapshot_export_zip,
            /* 双击导入（0.4.2）：前端只认识第一个，其余几个是它的分派目标 */
            am_open_import_file,
            am_import_userdata,
            am_import_package,
            am_snapshot_import_path,
            /* 界面导入任意格式 + 导出资产包（0.4.2） */
            am_import_files,
            am_pick_and_import,
            am_export_package,
            am_export_library_zip,
            am_snapshot_open,
            am_snapshot_paste,
            am_save_as,
            am_hotkeys_set,
            am_meta_missing,
            am_fill_palette,
            am_color_counts,
            am_dupes_exact,
            am_dupes_similar,
            am_refs_scan,
            am_refs_summary,
            am_broken_refs,
            am_refs_of,
            am_refs_using,
            am_rename_plan,
            am_rename_apply,
            am_tag_set_rule,
            am_tag_get_rule,
            am_lib_files,
            am_data_dir,
            am_data_root,
            am_probe_dir,
            am_prefetch_thumbs,
            am_warm_start,
            am_warm_stop,
            am_warm_status,
            am_set_data_root,
            am_relaunch,
            am_close,
            am_minimize,
            am_toggle_max
        ])
        .build(tauri::generate_context!())
        .expect("AssetManager 启动失败")
        .run(|_app, event| {
            // 退出时记一笔原因：区分「用户关的」「窗口被销毁」「正常退出」
            let dir = LOG_DIR.get().cloned();
            if let Some(dir) = dir {
                match &event {
                    tauri::RunEvent::ExitRequested { .. } => {
                        // 先立旗：接下来贴图窗口被销毁是"关机/关软件"，**不能**从存档里删掉，
                        // 否则重启后一扇都不会回来
                        QUITTING.store(true, Ordering::SeqCst);
                        log_runtime(&dir, "exit-requested")
                    }
                    tauri::RunEvent::Exit => log_runtime(&dir, "exit"),
                    tauri::RunEvent::WindowEvent { event, .. } => {
                        if matches!(event, tauri::WindowEvent::Destroyed) {
                            log_runtime(&dir, "window-destroyed");
                        }
                    }
                    _ => {}
                }
            }
        });
}

/// 把「出了什么事」写到数据目录下的 `runtime.log`。
///
/// 为什么需要：这个软件是 `windows_subsystem = "windows"`，**没有 stdout**；
/// 一旦在没有调试器的机器上出问题（比如「点一下设置窗口就没了」），
/// 用户能提供的只有一句「它自己关了」，我们这边什么都拿不到。
/// 有了这份日志，至少能区分「Rust panic」「WebView 崩溃」「被要求退出」这三种完全不同的原因。
pub fn log_runtime(data_dir: &Path, line: &str) {
    let _ = std::fs::create_dir_all(data_dir);
    let path = data_dir.join("runtime.log");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "[{stamp}] {line}")
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn log_runtime_creates_file() {
        let d = std::env::temp_dir().join(format!("am-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        super::log_runtime(&d, "hello");
        let body = std::fs::read_to_string(d.join("runtime.log")).unwrap();
        assert!(body.contains("hello"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `px=0` 是「要原图」的哨兵值，**不许**被档位钳位吃掉。
    /// 0.3.12 上线当天就栽在这：0 被夹成 64，长图在灯箱里反而比改之前更糊。
    #[test]
    fn px_zero_means_original() {
        assert_eq!(super::parse_query(Some("px=0&r=1&v=2")), 0);
        assert_eq!(super::parse_query(Some("px=384&r=1")), 384);
        assert_eq!(super::parse_query(Some("px=9999")), 2048, "上限仍是 2048");
        assert_eq!(super::parse_query(Some("px=8")), 64, "小档位仍按 64 兜底");
        assert_eq!(super::parse_query(None), 256, "没有参数时的默认档");
    }

    /// 贴图**内容盒**的比例必须等于图片比例：窗口 = 图 + 2×发光边（`spawn_peek` 里的
    /// `(iw + pad, ih + pad)`），去掉边之后的内容盒就是 `(iw, ih)`。
    ///
    /// 为什么值得一条测试：owner 2026-11-19 报「长图转贴图被压扁」。根因是 `.peek-img`
    /// 用了 `object-fit: fill` —— 窗口比例只要和图差一点点，图就被拉伸。CSS 已改成
    /// `contain`，但 contain 只有在「内容盒比例 == 图比例」时才**正好铺满**（否则四周留透明缝）。
    /// 这条测试钉的就是那个前提：别有人在 `spawn_peek` 里把 pad 只加一边、或者给内容盒
    /// 单独加最小宽 —— 那样长图立刻回到"要么变形要么留缝"的老路上。
    #[test]
    fn peek_box_keeps_image_aspect() {
        let pad = super::PEEK_GLOW * 2.0;
        for (w, h) in [
            (200u32, 1600u32), // 验证用的探针：1:8 竖长图
            (1844, 9797),      // owner 实测报"压扁"的那张
            (1920, 1080),      // 普通横图
            (1080, 1920),      // 普通竖图
            (800, 800),        // 方图
        ] {
            let (iw, ih) = super::peek_size_for(w, h, 342.0);
            let want = w as f64 / h as f64;
            let got = iw / ih;
            assert!(
                (got - want).abs() / want < 1e-9,
                "{w}×{h} 的内容盒 {iw:.2}×{ih:.2} 比例 {got:.6} ≠ 图 {want:.6}"
            );
            // 长边落在默认 342 上（没被 min/max 夹走），窗口正向尺寸也不为 0
            assert!(
                (iw.max(ih) - 342.0).abs() < 1e-9,
                "{w}×{h} 长边不是 342，实际 {iw:.2}×{ih:.2}"
            );
            assert!(iw + pad > 0.0 && ih + pad > 0.0);
        }
    }

    /// 极窄长图（短边会被 `_ .max(24)` 抬起来）：**只允许留透明缝，不允许变形**。
    /// 尺寸层已经尽力等比，剩下由 CSS `contain` 保证图本身不被拉伸。
    #[test]
    fn extreme_tall_peek_pads_not_stretches() {
        let (iw, ih) = super::peek_size_for(100, 20000, 342.0);
        assert_eq!(ih, 342.0, "长边仍是 342");
        assert_eq!(iw, 24.0, "短边按 24 兜底（再窄就没法拖了）");
        assert!(
            iw / ih > 100.0 / 20000.0,
            "兜底只会让盒子更宽（左右留透明缝），绝不会比图更窄"
        );
    }
}
