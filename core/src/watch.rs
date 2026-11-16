//! 外部变更跟随（T06）：只挂「库根 + 当前展开的目录」，**绝不递归全库**。
//!
//! 为什么不能递归：百万级库一递归就把 Windows 的 watch 句柄和内存吃光，
//! 而且 99% 的事件都在用户根本没看的目录里 —— 白烧 CPU。
//! 口径（MVP 方案 §4.5）：界面看到哪，才盯到哪。
//!
//! 事件在这里只做「合并 + 交出去」：真正的去抖在 Tauri 壳的消费线程里做
//! （400ms 窗口内同一目录只报一次），因为缩略图落地、杀毒软件回写都会连发。

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use notify::{Event, RecursiveMode, Watcher as _};

/// 一个「目录级」watcher。丢掉它 = 停止监听。
pub struct DirWatcher {
    // 必须保活：RecommendedWatcher 一 drop，系统那边就取消订阅了
    _inner: notify::RecommendedWatcher,
    rx: Receiver<PathBuf>,
}

impl DirWatcher {
    /// 拿变化目录的接收端（阻塞式，配合 `recv_timeout` 做去抖）。
    pub fn receiver(&self) -> &Receiver<PathBuf> {
        &self.rx
    }
}

/// 监听一批目录（每个都是非递归）。目录不存在就跳过，不报错。
pub fn watch_dirs(dirs: &[PathBuf]) -> Option<DirWatcher> {
    let (tx, rx) = channel();
    let mut inner = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(ev) = res else { return };
        for p in ev.paths {
            // 事件给的是「被改的那个路径」，我们要的是它所在的目录
            let dir = if p.is_dir() {
                p
            } else {
                p.parent().map(|x| x.to_path_buf()).unwrap_or(p)
            };
            let _ = tx.send(dir);
        }
    })
    .ok()?;

    for d in dirs {
        if d.is_dir() {
            let _ = inner.watch(d, RecursiveMode::NonRecursive);
        }
    }
    Some(DirWatcher { _inner: inner, rx })
}

/// 目录的 mtime（秒）。启动增量扫描靠它判断「这个目录有没有被动过」。
pub fn dir_mtime(p: &std::path::Path) -> i64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
