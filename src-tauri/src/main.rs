//! Tauri 壳：只做三件事 —— 暴露命令、注册 thumb:// 协议、管窗口。
//! 所有业务逻辑都在 `am-core`，这里不写算法。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use am_core::fsops::MovePolicy;
use am_core::model::{Asset, AssetQuery, DirNode, ScanStats};
use am_core::{db, fsops, scan, thumb, watch, App};
use serde::Serialize;
use tauri::http::{header, Response};
use tauri::{AppHandle, Emitter, Manager, State, Window};

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
const DECODE_BUDGET: usize = 10;
/// 一份对应的像素量（4 MP ≈ 解码后 12 MB RGB）
const DECODE_UNIT_PX: u64 = 4_000_000;

struct DecodeGate {
    free: Mutex<usize>,
    cv: Condvar,
}

fn gate() -> &'static DecodeGate {
    static G: OnceLock<DecodeGate> = OnceLock::new();
    G.get_or_init(|| DecodeGate { free: Mutex::new(DECODE_BUDGET), cv: Condvar::new() })
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
    ((est / DECODE_UNIT_PX) as usize + 1).clamp(1, DECODE_BUDGET - 1)
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

#[tauri::command]
fn am_children(parent: Option<i64>, state: State<AppState>) -> Result<Vec<DirNode>, String> {
    lock(&state).tree(parent)
}

/// 已加入的库根数量（前端加库前先问一次，最多 3 个）。
#[tauri::command]
fn am_lib_count(state: State<AppState>) -> Result<i64, String> {
    lock(&state).lib_count()
}

/// 从素材库移除一个库根：只清索引，磁盘上的文件一个都不动。
#[tauri::command]
fn am_remove_lib(path: String, state: State<AppState>) -> Result<usize, String> {
    lock(&state).remove_lib(&path)
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
        match App::open(&data_dir) {
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
        let Ok(mut a) = App::open(&data_dir) else { return };
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

/// 把素材拖出窗口 —— 拖到 PS / 资源管理器 / 浏览器地址栏。
///
/// 三条纪律（规格 §2.4）：
/// 1. **必须 `run_on_main_thread` 后立刻返回**：`DoDragDrop` 会开一个模态消息循环，
///    在命令线程里调它，拖完之前整个 IPC 都被它占着；
/// 2. 出去的是**磁盘原文件**（`DragItem::Files` → CF_HDROP），不是缩略图；
/// 3. 一律 COPY 语义（`DragMode::Copy`）—— 从素材库拖出去永远不该「移走」源文件。
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
            let r = drag::start_drag(
                &win,
                drag::DragItem::Files(files),
                image,
                move |result, pos| {
                    // 松手位置换成本窗口的 CSS 像素，前端据此判断「是不是落在目录树上」
                    let scale = win2.scale_factor().unwrap_or(1.0).max(0.5);
                    let origin = win2.outer_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));
                    let lx = (pos.x - origin.0) as f64 / scale;
                    let ly = (pos.y - origin.1) as f64 / scale;
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
        match App::open(&data_dir) {
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

/// 导出用户层数据（T13）：`userdata-<ts>.json` 落在数据目录，返回完整路径。
#[tauri::command]
fn am_export_userdata(state: State<AppState>) -> Result<String, String> {
    let app = lock(&state);
    let json = app.export_userdata()?;
    let name = format!("userdata-{}.json", db::now_ts());
    let path = app.data_dir.join(name);
    std::fs::write(&path, json).map_err(|e| format!("写文件失败：{}", e))?;
    Ok(path.to_string_lossy().to_string())
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

#[tauri::command]
fn am_rename(path: String, name: String, state: State<AppState>) -> Result<String, String> {
    let app = lock(&state);
    let ops = fsops::FsOps::new(app.conn_for_path(&path)?);
    let out = ops.rename(std::path::Path::new(&path), &name)?;
    Ok(out.to_string_lossy().to_string())
}

#[tauri::command]
fn am_mkdir(parent: String, name: String, state: State<AppState>) -> Result<String, String> {
    let app = lock(&state);
    let ops = fsops::FsOps::new(app.conn_for_path(&parent)?);
    let out = ops.mkdir(std::path::Path::new(&parent), &name)?;
    Ok(out.to_string_lossy().to_string())
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

    Ok(MoveOutcome {
        moved: ext.report.moved,
        skipped: ext.report.skipped,
        renamed: ext.report.renamed.iter().map(|(o, n)| format!("{} → {}", o, n)).collect(),
        replaced: ext.report.replaced,
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

        match App::open(&data_dir) {
            Ok(mut a) => {
                let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
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
                        outcome = Some(MoveOutcome {
                            moved: ext.report.moved,
                            skipped: ext.report.skipped,
                            renamed: ext.report.renamed.iter().map(|(o, n)| format!("{} → {}", o, n)).collect(),
                            replaced: ext.report.replaced,
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
    let n = list.len();
    {
        // 一批文件可能跨库（虽然罕见）：取第一个文件的库写 ops 日志就够，
        // 索引刷新走 refresh_dirs（按目录路径，不受库边界影响）
        if let Some(first) = list.first() {
            let ops = fsops::FsOps::new(app.conn_for_path(&first.to_string_lossy())?);
            ops.delete(&list)?;
        }
    }
    let dirty: HashSet<PathBuf> = list
        .iter()
        .filter_map(|p| p.parent().map(|x| x.to_path_buf()))
        .collect();
    refresh_dirs(&mut app, &dirty);
    Ok(n)
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

/* ---------------- 无边框看图窗口（0.9.0，照果核看图的「无边框模式」） ----------------
   果核看图最值得抄的一点不是渲染，而是**窗口形态**：它能把窗口变成"就是那张图"——
   没有标题栏、没有边框、没有工具条，长边固定 342 DIP，窗口尺寸按图片比例算。
   拖到截图里、贴在素材旁边比对时，它不占地方也不穿帮。

   这里照它做一扇同样形态的窗口。差别在于我们用的是 WebView2（不是 D2D 直接画），
   所以必须：
   · `transparent(true)` —— 不然圆角外面是一块白/黑底；
   · `shadow(false)` —— 系统阴影只会画成方框，圆角就白做了；
   · 前端 body 透明（`body.peek-body`）；
   · 缩放**改窗口尺寸**而不是改 CSS transform —— 这才是"连窗口一起缩放"的手感，
     也是截图时最需要的行为（窗口边缘始终贴着图）。 */

/// 无边框窗口的默认长边（DIP）。和果核看图同一个数：342。
const PEEK_LONG_EDGE: f64 = 342.0;
/// 长边允许范围（逻辑像素）：再小点不到、再大就是放大糊图
const PEEK_MIN_EDGE: f64 = 96.0;
const PEEK_MAX_EDGE: f64 = 4096.0;

/// 图片比例 → 窗口逻辑尺寸。未知尺寸按 4:3 兜底（和瓦片骨架同一口径）。
fn peek_size_for(w: u32, h: u32, long_edge: f64) -> (f64, f64) {
    let (w, h) = if w > 0 && h > 0 { (w as f64, h as f64) } else { (4.0, 3.0) };
    let long = long_edge.clamp(PEEK_MIN_EDGE, PEEK_MAX_EDGE);
    if w >= h {
        (long, (long * h / w).max(24.0))
    } else {
        ((long * w / h).max(24.0), long)
    }
}

/// 把窗口居中到鼠标所在的那块屏（双屏用户常在副屏截图，居中到主屏就跑到另一边去了）
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
    let (cx, cy) = match m {
        Some(m) => {
            let p = m.position();
            let s = m.size();
            (
                p.x as f64 + (s.width as f64 - size.width as f64) / 2.0,
                p.y as f64 + (s.height as f64 - size.height as f64) / 2.0,
            )
        }
        None => (
            cur.x - size.width as f64 / 2.0,
            cur.y - size.height as f64 / 2.0,
        ),
    };
    let _ = win.set_position(tauri::PhysicalPosition::new(cx.round(), cy.round()));
}

/// 从 `ids` 里截出最多 `cap` 个，并让 `index` 落在窗口内。
fn trim_around(ids: &[i64], index: usize, cap: usize) -> (Vec<i64>, usize) {
    let i = index.min(ids.len().saturating_sub(1));
    if ids.len() <= cap {
        return (ids.to_vec(), i);
    }
    let half = cap / 2;
    let start = i.saturating_sub(half).min(ids.len() - cap);
    (ids[start..start + cap].to_vec(), i - start)
}

/// 开一扇无边框看图窗口。`ids` 是这次浏览的整个集合（方向键在其中切换），`index` 是当前那张。
///
/// **必须是 `async fn`**（六个 peek 命令都是）：同步命令跑在主线程上，
/// 而 `WebviewWindowBuilder::build()` / `set_size()` / `set_position()` 都要回主线程执行，
/// 于是它们等自己 —— 自死锁。症状很迷惑：窗口对象被建出来了（能枚举、能截图），
/// 但里面什么都不渲染、`on_page_load` 永不触发、命令也永不返回（日志停在调用前一行）。
#[tauri::command]
async fn am_peek_open(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
    index: usize,
    // 上次用的长边（DIP）；前端从 localStorage 里带过来，没有就用默认 342
    long_edge: Option<f64>,
) -> Result<(), String> {
    if ids.is_empty() {
        return Err("没有可看的素材".into());
    }
    /* 上限 600 个，且**围着当前这张截窗**：直接 `take(600)` 会把"第 800 个"截成第 599 个，
       用户按 B 看到的是一张完全不相干的图（而且不报错）。 */
    let (ids, i) = trim_around(&ids, index, 600);
    // 只在锁里取尺寸，拿到立刻放开 —— 建窗口要走主线程，别在持锁期间干慢事
    let (w, h) = {
        let a = lock(&state);
        match a.asset(ids[i])? {
            Some(x) => (x.width.unwrap_or(0).max(0) as u32, x.height.unwrap_or(0).max(0) as u32),
            None => return Err("这张素材已经不在库里了".into()),
        }
    };
    let (lw, lh) = peek_size_for(w, h, long_edge.unwrap_or(PEEK_LONG_EDGE));
    if let Some(old) = app.get_webview_window("peek") {
        let _ = old.close();
    }
    /* 启动数据用**初始化脚本**注入，不走 URL。
       踩过两次：`WebviewUrl::App` 收的是 PathBuf，`index.html#/peek?ids=…` 与
       `index.html?peek=1&ids=…` 都让新窗口全透明、连 on_page_load 都不触发
       （runtime.log 里只有 `peek open`，没有 `peek page`）。
       换成纯 `index.html` + `initialization_script` 之后导航正常，
       而且省掉了 URL 长度限制（一屏 200 个 id 就是 3 KB 的查询串）。 */
    let ids_js = ids.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",");
    let boot = format!("window.__AM_PEEK__ = {{ ids: [{ids_js}], i: {i} }};");
    if let Some(d) = LOG_DIR.get() {
        log_runtime(d, &format!("peek open n={} i={i}", ids.len()));
    }
    let win = tauri::WebviewWindowBuilder::new(&app, "peek", tauri::WebviewUrl::App("index.html".into()))
        .title("看图")
        .inner_size(lw, lh)
        .decorations(false)
        /* **不透明**：窗口尺寸就是图片尺寸、图正好铺满，没有任何地方需要透出桌面。
           反而 `transparent(true)` 在"运行中新建的窗口"上不工作（实测：窗口有了、
           全透明，webview 却连 on_page_load 都不触发）。果核看图的窗口也是方的。 */
        .transparent(false)
        .shadow(false)
        .resizable(true)
        .maximizable(false)
        .minimizable(false)
        .initialization_script(&boot)
        .build();
    // build() 失败**不能 `?` 提前返回**：窗口对象可能已经建了一半（实测能枚举到窗口，
    // 但里面什么都没有），错误又被前端 toast 吞掉，排查时完全不可见。先留证。
    let win = match win {
        Ok(w) => w,
        Err(e) => {
            if let Some(d) = LOG_DIR.get() {
                log_runtime(d, &format!("peek build FAIL {e}"));
            }
            return Err(format!("无边框窗口创建失败：{e}"));
        }
    };
    peek_center_on_cursor(&win);
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

/// 缩放窗口（**连窗口一起缩放**，锚定窗口中心）—— 滚轮走这里。
#[tauri::command]
async fn am_peek_zoom(win: tauri::Window, step: f64) -> Result<(), String> {
    if !(0.5..=2.0).contains(&step) {
        return Err("缩放步长不合法".into());
    }
    let size = win.outer_size().map_err(|e| e.to_string())?;
    let pos = win.outer_position().map_err(|e| e.to_string())?;
    let scale = win.scale_factor().map_err(|e| e.to_string())?;

    let mut w = size.width as f64 * step;
    let mut h = size.height as f64 * step;
    let long = w.max(h);
    let min = PEEK_MIN_EDGE * scale;
    let max = PEEK_MAX_EDGE * scale;
    // 撞到上下限时按同一比例回拉，形状不能变
    let k = if long < min {
        min / long
    } else if long > max {
        max / long
    } else {
        1.0
    };
    w = (w * k).round().max(24.0);
    h = (h * k).round().max(24.0);
    let nx = (pos.x as f64 + (size.width as f64 - w) / 2.0).round();
    let ny = (pos.y as f64 + (size.height as f64 - h) / 2.0).round();
    let r = win.set_size(tauri::PhysicalSize::new(w, h)).and_then(|_| {
        win.set_position(tauri::PhysicalPosition::new(nx, ny))
    });
    if let Err(e) = &r {
        if let Some(d) = LOG_DIR.get() {
            log_runtime(d, &format!("peek zoom FAIL {e}"));
        }
    }
    r.map_err(|e| e.to_string())
}

/// 换图时按新图比例改窗口尺寸（长边保持当前大小，中心不动）。
#[tauri::command]
async fn am_peek_ratio(win: tauri::Window, w: u32, h: u32) -> Result<(), String> {
    let size = win.outer_size().map_err(|e| e.to_string())?;
    let pos = win.outer_position().map_err(|e| e.to_string())?;
    let long = (size.width.max(size.height)) as f64;
    let (nw, nh) = peek_size_for(w, h, long);
    let nx = (pos.x as f64 + (size.width as f64 - nw) / 2.0).round();
    let ny = (pos.y as f64 + (size.height as f64 - nh) / 2.0).round();
    win.set_size(tauri::PhysicalSize::new(nw.round(), nh.round()))
        .map_err(|e| e.to_string())?;
    win.set_position(tauri::PhysicalPosition::new(nx, ny))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn am_peek_close(win: tauri::Window) -> Result<(), String> {
    win.close().map_err(|e| e.to_string())
}

/// 置顶（对着参考图改稿时，它必须压在 PS 上面）
#[tauri::command]
async fn am_peek_top(win: tauri::Window, on: bool) -> Result<bool, String> {
    win.set_always_on_top(on).map_err(|e| e.to_string())?;
    Ok(on)
}

/// 无边框窗口里双击 / 按 F11 的"铺满屏幕"（不抢任务栏，仍可 Esc 退出）
#[tauri::command]
async fn am_peek_fullscreen(win: tauri::Window, on: bool) -> Result<bool, String> {
    win.set_fullscreen(on).map_err(|e| e.to_string())?;
    Ok(on)
}

fn main() {
    tauri::Builder::default()
        .manage(WatchState(Mutex::new(None)))
        .setup(|app| {
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
            let st = App::open(&data).map_err(|e| format!("打开索引失败：{}", e))?;
            // 旧版 JPEG 缩略图清一遍（后台跑，别卡首屏；删不掉的留到下次）
            let store = thumb::ThumbStore::new(st.thumbs.root.clone());
            std::thread::spawn(move || {
                let (files, bytes) = thumb::purge_legacy(&store);
                if files > 0 {
                    eprintln!("清理旧 JPEG 缩略图：{} 个 / {:.1} MB", files, bytes as f64 / 1048576.0);
                }
            });
            app.manage(AppState(Mutex::new(st)));

            // 启动增量扫描（T06）：等界面先画出来，再拿索引里记的 mtime 比对磁盘。
            // 不递归、不全量，所以几万目录也就一两秒；有变化才推 fs://changed。
            let handle = app.handle().clone();
            let data2 = data.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(2500));
                let Ok(mut a) = App::open(&data2) else { return };
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
            });
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
            am_lib_count,
            am_remove_lib,
            am_children,
            am_list,
            am_total,
            am_tags,
            am_auto_tags,
            am_auto_tags_for,
            am_asset_tags,
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
            am_reveal,
            am_open_path,
            am_copy_files,
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
            am_peek_fullscreen,
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
                    tauri::RunEvent::ExitRequested { .. } => log_runtime(&dir, "exit-requested"),
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
}
