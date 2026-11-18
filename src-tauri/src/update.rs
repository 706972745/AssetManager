//! 检查更新 / 下载安装包 / 校验 / 静默安装。
//!
//! 这一份是照 FontManager 的 `update.rs` 搬过来的（用户 2026-11-18 要求
//! 「自动更新的页面和功能也加上，和 fontmanager 一样」），仓库名、资产名、
//! 安装目录、日志文件换成了 AssetManager 的，其余规矩一条没松：
//!
//!   ① **检查**只读版本号 + 发布说明 + 资产清单，不下载、不执行；
//!   ② **下载**只认 https + 白名单站点（github.com / objects / release-assets），
//!      重定向落点也要在白名单内；
//!   ③ 必须带 SHA-256（GitHub Asset 的 `digest`）。下完对不上 → 删掉，**绝不执行**；
//!   ④ 安装前再算一次哈希（防「下载完到点安装之间被换掉」）；
//!   ⑤ 只给「按用户安装」的 NSIS 版用。装成 MSI / 从文件夹直接跑的副本
//!      一律退回「打开发布页」，静默 `/S` 在那种机器上会装出第二份；
//!   ⑥ 安装交给一段临时 PowerShell：等本进程退出 → `/S` 静默装 → 拉起新版。
//!      Windows 不允许覆盖正在运行的 exe，只能这么绕。
//!
//! 失败一律返回**能看懂的中文原因**：更新这条链上用户最需要的不是错误码，
//! 而是「是网络断了、还是这一版没有安装包、还是我这台机器装法不支持」。

use serde::Serialize;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::AppHandle;

const UA: &str = concat!(
    "AssetManager/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/706972745/AssetManager)"
);

const API_URL: &str = "https://api.github.com/repos/706972745/AssetManager/releases/latest";
/// 兜底源：只读版本号（API 被墙时用）。frontend 在 app/ 子目录下，路径别漏。
const RAW_PKG_URL: &str = "https://raw.githubusercontent.com/706972745/AssetManager/main/app/package.json";
const CDN_PKG_URL: &str = "https://cdn.jsdelivr.net/gh/706972745/AssetManager@main/app/package.json";
const RELEASES_PAGE: &str = "https://github.com/706972745/AssetManager/releases/latest";

const TIMEOUT: Duration = Duration::from_secs(12);

/// 进程内缓存：反复点「检查更新」不重复打接口（GitHub 匿名接口 60 次/小时/IP）
const CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// 当前运行的版本（编译期写死，跟着安装包走）
    pub current: String,
    /// 远端最新版本（去掉前缀 `v`）
    pub latest: String,
    pub has_update: bool,
    /// 发布页地址（浏览器打开）
    pub url: String,
    /// 发布说明（截断到 4000 字符，界面按纯文本展示）
    pub notes: String,
    pub published_at: String,
    /// `github` = Releases API；`raw` / `cdn` = 只能读到 package.json 的兜底
    pub source: String,
    pub setup_url: String,
    pub setup_name: String,
    /// 安装包 SHA-256（小写十六进制；空串 = 发布里没给）
    pub sha256: String,
    /// 安装包 Asset 的数字 id（asset API 兜底通道用；0 = 没有）
    pub setup_asset_id: u64,
    pub can_auto_install: bool,
    /// `user` / `system` / `portable`
    pub install_kind: String,
    /// 装不了时**是哪一条**没过（空 = 没有阻塞）
    pub auto_install_blocked: String,
}

/// 版本号解析：`v1.8.10` / `1.8.10-beta.1` / `1.8` 都接受，只取主.次.修订三段。
///
/// 解析不出来返回 None —— 调用方据此**不提示更新**（宁可少报，不可乱报）。
pub fn parse_version(raw: &str) -> Option<(u64, u64, u64)> {
    let s = raw.trim();
    let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    let core = s.split(['-', '+']).next()?.trim();
    if core.is_empty() {
        return None;
    }
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// 远端是否比本机新。任一侧解析失败一律 false。
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// 检查更新时允许落地的站点（跟随有限次重定向，落点仍要过白名单）
const ALLOWED_HOSTS: [&str; 5] = [
    "api.github.com",
    "raw.githubusercontent.com",
    "cdn.jsdelivr.net",
    "github.com",
    "objects.githubusercontent.com",
];

const DOWNLOAD_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// 国内网络下 github.com 常被干扰而 api.github.com 可用，所以留两条镜像退路。
/// 镜像只是**换条路拿同一个文件**，SHA-256 取自 GitHub 官方 digest、贯穿所有通道 ——
/// 通道被投毒也过不了校验。
const MIRROR_HOSTS: [&str; 2] = ["gh-proxy.com", "ghfast.top"];
const MIRROR_PREFIXES: [&str; 2] = ["https://gh-proxy.com/", "https://ghfast.top/"];
const ASSET_API_PREFIX: &str = "https://api.github.com/repos/706972745/AssetManager/releases/assets/";

fn host_of(url: &str) -> &str {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or("")
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .redirect(reqwest::redirect::Policy::limited(3))
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("网络客户端初始化失败：{e}"))
}

fn explain(url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!("{url} 连接超时（当前网络可能访问不了 GitHub）")
    } else if e.is_connect() {
        format!("{url} 连不上（当前网络可能访问不了 GitHub）")
    } else {
        format!("{url} 请求失败：{e}")
    }
}

async fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let res = client()?
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| explain(url, &e))?;
    let host = res.url().host_str().unwrap_or_default().to_string();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err(format!("{url} 被重定向到非预期站点：{host}"));
    }
    let status = res.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            403 | 429 => format!("GitHub 接口限流了（HTTP {}），过一会儿再试", status.as_u16()),
            404 => format!("{url} 返回 404：仓库还没有正式发布"),
            code => format!("{url} 返回 HTTP {code}"),
        });
    }
    res.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("{url} 返回的不是有效 JSON：{e}"))
}

/// 发布说明只留前 4000 字符：几万字全塞进界面只会卡。
fn trim_notes(text: &str) -> String {
    const MAX: usize = 4000;
    let mut out = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i >= MAX {
            out.push_str("\n…");
            break;
        }
        out.push(ch);
    }
    out
}

/// 从资产名里抠版本：`AssetManager_0.1.35_x64-setup.exe` → `(0, 1, 35)`。
///
/// 只做「把版本串切出来」，切出来的片段一律交给 [`parse_version`] ——
/// 版本口径全项目只有那一份，这里再写一套就会慢慢漂移。
fn version_from_asset(name: &str) -> (u64, u64, u64) {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix("assetmanager_").unwrap_or(&lower);
    let head: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    // ⚠️ 这里**不能**像 FontManager 那样要求 `major != 0`：本软件的版本号是 0.1.x，
    // major 恒为 0，加了那个判断所有资产都会被判成「没有版本号」而排到末尾。
    parse_version(&head).unwrap_or((0, 0, 0))
}

/// 从 release 的 assets 里挑出「能静默装的安装包」。
///
/// 只认 `*-setup.exe`（NSIS，currentUser 安装、不需要提权）。**不碰 MSI**：
/// perMachine 安装要 UAC，静默跑会直接失败或弹一个没人点的授权框。
/// 同一个 release 里若挂了两个 `-setup.exe`，按「版本高优先、其次 x64」挑，
/// 不比版本号就可能下发旧包。
fn pick_setup(assets: &serde_json::Value) -> (String, String, String, u64) {
    let Some(list) = assets.as_array() else {
        return (String::new(), String::new(), String::new(), 0);
    };
    let mut best: Option<((u64, u64, u64, u8), String, String, String, u64)> = None;
    for a in list {
        let name = a.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if !name.to_ascii_lowercase().ends_with("-setup.exe") {
            continue;
        }
        let url = a
            .get("browser_download_url")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if url.is_empty() {
            continue;
        }
        // GitHub 从 2024 起给 asset 带 digest（"sha256:…"）；没有校验值的版本
        // 一律不提供一键安装（见 can_auto_install）
        let digest = a
            .get("digest")
            .and_then(|x| x.as_str())
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(|h| h.to_ascii_lowercase())
            .unwrap_or_default();
        let id = a.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
        let (vmaj, vmin, vpat) = version_from_asset(name);
        let rank = (vmaj, vmin, vpat, u8::from(name.contains("x64")));
        let cand = (rank, name.to_string(), url.to_string(), digest, id);
        if best.as_ref().map(|b| cand.0 > b.0).unwrap_or(true) {
            best = Some(cand);
        }
    }
    match best {
        Some((_, name, url, digest, id)) => (url, name, digest, id),
        None => (String::new(), String::new(), String::new(), 0),
    }
}

/// 当前这份程序是哪种装法。静默升级只对 `PerUser` 放行。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallKind {
    /// NSIS 按用户安装（默认落在 `%LOCALAPPDATA%\AssetManager`）
    PerUser,
    /// 装在 `Program Files` 下（MSI / 手动装成系统级）
    PerMachine,
    /// 从文件夹里直接跑的副本（开发构建、绿色版）
    Portable,
}

impl InstallKind {
    fn as_str(self) -> &'static str {
        match self {
            InstallKind::PerUser => "user",
            InstallKind::PerMachine => "system",
            InstallKind::Portable => "portable",
        }
    }
}

fn norm_dir(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    s.trim_end_matches('\\').to_lowercase()
}

fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    !a.as_os_str().is_empty() && norm_dir(a) == norm_dir(b)
}

fn looks_system_wide(dir: &std::path::Path) -> bool {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(|k| std::env::var_os(k))
        .any(|base| {
            let s = norm_dir(dir);
            let b = norm_dir(std::path::Path::new(&base));
            s == b || s.starts_with(&(b + "\\"))
        })
}

/// 判定当前安装形态。
///
/// 判据是**当前 exe 所在目录**：Tauri 的 NSIS 安装器默认把 currentUser 版装到
/// `%LOCALAPPDATA%\AssetManager`，静默 `/S` 会装回同一处；在那里就是可静默升级的。
/// 开发构建（`src-tauri\target\...`）落在别处 → 判成 `Portable`，按钮退回「打开发布页」，
/// 不会把开发机上的 exe 换成正式版。
pub fn install_kind() -> InstallKind {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()))
    else {
        return InstallKind::Portable;
    };
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        if same_dir(&std::path::Path::new(&base).join("AssetManager"), &dir) {
            return InstallKind::PerUser;
        }
    }
    if looks_system_wide(&dir) {
        return InstallKind::PerMachine;
    }
    InstallKind::Portable
}

/// 「装不了」的原因码（界面据此给不同文案）：`system` / `portable` / `no-asset` / `no-digest`
fn blocker(kind: InstallKind, setup_url: &str, sha: &str) -> &'static str {
    if kind != InstallKind::PerUser {
        return match kind {
            InstallKind::PerMachine => "system",
            _ => "portable",
        };
    }
    if setup_url.is_empty() {
        return "no-asset";
    }
    if sha.is_empty() {
        return "no-digest";
    }
    ""
}

async fn from_release_api() -> Result<UpdateInfo, String> {
    let v = get_json(API_URL).await?;
    let tag = v
        .get("tag_name")
        .or_else(|| v.get("name"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let latest = parse_version(tag)
        .map(|(a, b, c)| format!("{a}.{b}.{c}"))
        .ok_or_else(|| format!("读不懂远端版本号：{tag}"))?;
    let url = v
        .get("html_url")
        .and_then(|x| x.as_str())
        .unwrap_or(RELEASES_PAGE)
        .to_string();
    let notes = v.get("body").and_then(|x| x.as_str()).unwrap_or("");
    let published_at = v.get("published_at").and_then(|x| x.as_str()).unwrap_or("");
    let empty = serde_json::Value::Null;
    let assets = v.get("assets").unwrap_or(&empty);
    let (setup_url, setup_name, sha256, setup_asset_id) = pick_setup(assets);

    let kind = install_kind();
    let blocked = blocker(kind, &setup_url, &sha256);
    Ok(UpdateInfo {
        current: env!("CARGO_PKG_VERSION").to_string(),
        has_update: is_newer(&latest, env!("CARGO_PKG_VERSION")),
        latest,
        url,
        notes: trim_notes(notes),
        published_at: published_at.to_string(),
        source: "github".into(),
        can_auto_install: blocked.is_empty(),
        install_kind: kind.as_str().into(),
        auto_install_blocked: blocked.into(),
        setup_url,
        setup_name,
        sha256,
        setup_asset_id,
    })
}

/// 兜底：API 被墙时只读 package.json 里的版本号（没有发布说明、没有安装包直链）。
async fn from_package_json(url: &str) -> Result<UpdateInfo, String> {
    let v = get_json(url).await?;
    let raw = v
        .get("version")
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("{url} 里没有 version"))?;
    let latest = parse_version(raw)
        .map(|(a, b, c)| format!("{a}.{b}.{c}"))
        .ok_or_else(|| format!("读不懂远端版本号：{raw}"))?;
    let kind = install_kind();
    Ok(UpdateInfo {
        current: env!("CARGO_PKG_VERSION").to_string(),
        has_update: is_newer(&latest, env!("CARGO_PKG_VERSION")),
        latest,
        url: RELEASES_PAGE.to_string(),
        notes: String::new(),
        published_at: String::new(),
        source: if url.contains("jsdelivr") { "cdn" } else { "raw" }.into(),
        setup_url: String::new(),
        setup_name: String::new(),
        sha256: String::new(),
        setup_asset_id: 0,
        can_auto_install: false,
        install_kind: kind.as_str().into(),
        auto_install_blocked: "no-asset".into(),
    })
}

fn cache() -> &'static Mutex<Option<(Instant, UpdateInfo)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, UpdateInfo)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 检查更新。`force = false` 时命中 10 分钟缓存直接返回。
pub async fn check(force: bool) -> Result<UpdateInfo, String> {
    if !force {
        if let Ok(guard) = cache().lock() {
            if let Some((at, info)) = guard.as_ref() {
                if at.elapsed() < CACHE_TTL {
                    return Ok(info.clone());
                }
            }
        }
    }

    // 三级回退：Releases API（带说明 + 资产）→ raw 原始文件 → jsDelivr 镜像。
    // 三条都断才报错，错误信息里把每一步的原因串起来，用户截图就能定位。
    let mut errs: Vec<String> = Vec::new();
    match from_release_api().await {
        Ok(info) => return Ok(remember(info)),
        Err(e) => errs.push(e),
    }
    for url in [RAW_PKG_URL, CDN_PKG_URL] {
        match from_package_json(url).await {
            Ok(info) => return Ok(remember(info)),
            Err(e) => errs.push(e),
        }
    }
    Err(errs.join("；备用源也不行："))
}

fn remember(info: UpdateInfo) -> UpdateInfo {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some((Instant::now(), info.clone()));
    }
    info
}

/// 安装包下载进度事件
pub const PROGRESS_EVENT: &str = "update:progress";

fn download_url_ok(url: &str) -> bool {
    DOWNLOAD_HOSTS.contains(&host_of(url))
}

/// 重定向**落点**的白名单：比入口宽一档 —— 镜像通道的落点可以是镜像域自身
/// （代理流），asset API 的落点是 objects / release-assets。
fn download_resp_host_ok(url: &str) -> bool {
    let host = host_of(url);
    DOWNLOAD_HOSTS.contains(&host) || MIRROR_HOSTS.contains(&host) || host == "api.github.com"
}

fn is_loopback_url(url: &str) -> bool {
    url.contains("://127.0.0.1:")
        || url.contains("://localhost:")
        || url.contains("://[::1]:")
}

/// 一条下载通道。`octet = true` 时请求要带 `Accept: application/octet-stream`
///（asset API 靠这个头返回文件内容而不是 JSON 元数据）。
struct DlChannel {
    url: String,
    octet: bool,
    label: &'static str,
}

/// 通道接力：直连 → gh-proxy.com → ghfast.top → asset API。
fn download_channels(url: &str, asset_id: u64) -> Vec<DlChannel> {
    let mut v = vec![DlChannel {
        url: url.to_string(),
        octet: false,
        label: "direct",
    }];
    if download_url_ok(url) {
        for m in MIRROR_PREFIXES {
            v.push(DlChannel {
                url: format!("{m}{url}"),
                octet: false,
                label: "mirror",
            });
        }
        if asset_id != 0 {
            v.push(DlChannel {
                url: format!("{ASSET_API_PREFIX}{asset_id}"),
                octet: true,
                label: "asset-api",
            });
        }
    }
    v
}

/// 文件名只允许 `…-setup.exe`，且不含路径分隔符 / 通配符（防目录穿越与参数注入）
fn setup_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && name.to_ascii_lowercase().ends_with("-setup.exe")
        && !name.contains(['/', '\\', ':', '"', '<', '>', '|', '?', '*'])
}

fn sha256_hex_ok(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("读不到安装包：{e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn emit_progress(app: &AppHandle, phase: &str, received: u64, total: u64, error: Option<&str>) {
    use tauri::Emitter as _;
    let _ = app.emit(
        PROGRESS_EVENT,
        serde_json::json!({
            "phase": phase,
            "received": received,
            "total": total,
            "percent": if total > 0 { (received.saturating_mul(100) / total).min(100) } else { 0 },
            "error": error,
        }),
    );
}

/// 安装包落地的固定目录（`install_setup` 只认这里的文件）
fn update_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("AssetManager-update")
}

fn log_line(msg: &str) {
    use std::io::Write as _;
    let dir = update_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("update.log"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// 清掉临时目录里**以前版本**留下的安装包（升级包 4~8MB，每升一次留一份没必要）。
/// 只保留下载目标这一个（「下到一半断了再点一次」要复用它，靠 SHA-256 决定能不能用）。
fn prune_stale_setups(dir: &std::path::Path, keep: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep || !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("AssetManager_") && name.ends_with("-setup.exe") {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 下载安装包到临时目录并校验 SHA-256，返回落地路径。
///
/// 已经下过且哈希对的文件直接复用：用户网络差、第一次下到一半断了再点一次，
/// 不该从头再来一遍。
pub fn download_setup(
    app: &AppHandle,
    url: &str,
    name: &str,
    want_sha: &str,
    asset_id: u64,
) -> Result<String, String> {
    log_line(&format!("[update] download start {name} <- {url}"));
    if !setup_name_ok(name) {
        return Err(format!("安装包文件名不合法：{name}"));
    }
    let want = want_sha.trim().to_ascii_lowercase();
    if !sha256_hex_ok(&want) {
        return Err("这一版没有提供 SHA-256 校验值，不能自动安装（请到发布页手动下载）".into());
    }

    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("建临时目录失败：{e}"))?;
    let dest = dir.join(name);
    prune_stale_setups(&dir, &dest);

    if dest.is_file() {
        if let Ok(h) = sha256_file(&dest) {
            if h == want {
                emit_progress(app, "ready", 1, 1, None);
                return Ok(dest.to_string_lossy().into_owned());
            }
        }
        let _ = std::fs::remove_file(&dest);
    }

    let channels = download_channels(url, asset_id);
    let mut errs: Vec<String> = Vec::new();
    for ch in &channels {
        // 直连通道的地址来自外部（Releases API 的 browser_download_url），必须过白名单；
        // 镜像 / asset API 是内部拼的，格式自证。
        if ch.label == "direct" && !download_url_ok(&ch.url) {
            errs.push(format!("{}：下载地址不在允许的站点内", ch.label));
            continue;
        }
        emit_progress(app, "download", 0, 0, None);
        match try_channel(app, ch, name, &want, &dir, &dest) {
            Ok(()) => {
                log_line(&format!("[update] download done {name} (sha ok, via {})", ch.label));
                emit_progress(app, "ready", 1, 1, None);
                return Ok(dest.to_string_lossy().into_owned());
            }
            Err(e) => {
                log_line(&format!("[update] channel {} failed: {e}", ch.label));
                errs.push(format!("{}：{e}", ch.label));
            }
        }
    }
    emit_progress(app, "error", 0, 0, Some("下载失败"));
    Err(format!(
        "直连与备用通道都下载失败（{}）；可以到发布页手动下载",
        errs.join("；")
    ))
}

/// 走单条通道把文件下到 `.part`，SHA-256 校验通过后挪到 `dest`。
fn try_channel(
    app: &AppHandle,
    ch: &DlChannel,
    name: &str,
    want: &str,
    dir: &std::path::Path,
    dest: &std::path::Path,
) -> Result<(), String> {
    use std::io::{Read as _, Write as _};

    let partial = dir.join(format!("{name}.part"));
    let client = {
        let mut b = reqwest::blocking::Client::builder()
            .user_agent(UA)
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(Duration::from_secs(600));
        if is_loopback_url(&ch.url) {
            b = b.no_proxy();
        }
        b.build()
            .map_err(|e| format!("网络客户端初始化失败：{e}"))?
    };
    let mut req = client.get(&ch.url);
    if ch.octet {
        req = req.header(reqwest::header::ACCEPT, "application/octet-stream");
    }
    let mut resp = req
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("{e}"))?;
    // 重定向后的落点也要在白名单里（GitHub 下载会 302 到 objects.githubusercontent.com）
    if !download_resp_host_ok(resp.url().as_str()) {
        return Err(format!(
            "下载被重定向到非预期站点：{}",
            resp.url().host_str().unwrap_or("?")
        ));
    }
    let total = resp.content_length().unwrap_or(0);

    {
        let mut f = std::fs::File::create(&partial).map_err(|e| format!("写临时文件失败：{e}"))?;
        let mut received: u64 = 0;
        let mut last_emit = Instant::now();
        let mut buf = [0u8; 65536];
        loop {
            let n = resp.read(&mut buf).map_err(|e| format!("下载中断：{e}"))?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| format!("写临时文件失败：{e}"))?;
            received += n as u64;
            if last_emit.elapsed() >= Duration::from_millis(200) {
                emit_progress(app, "download", received, total, None);
                last_emit = Instant::now();
            }
        }
        f.flush().ok();
    }

    emit_progress(app, "verify", total, total, None);
    let got = sha256_file(&partial)?;
    if got != want {
        let _ = std::fs::remove_file(&partial);
        emit_progress(app, "error", 0, 0, Some("校验失败"));
        return Err("安装包校验失败（内容与发布页的 SHA-256 不一致），已删除，没有安装".into());
    }
    std::fs::rename(&partial, dest).map_err(|e| format!("保存安装包失败：{e}"))?;
    Ok(())
}

/// `canonicalize()` / 某些启动方式会给回 `\\?\C:\...`（扩展长度路径）。
/// `Start-Process` **不认**这种前缀，会「找不到路径」；而脚本顶部是
/// `SilentlyContinue`，于是 NSIS 静默没跑、应用照常重启 —— 看起来像升级成功，
/// 实际什么都没换。
fn strip_verbatim(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
    }
}

/// 生成安装脚本。
///
/// 为什么不是「直接 spawn 安装器」：Windows 不允许覆盖正在运行的 exe，
/// 而我们要退出的这个进程正是它。所以脚本先等我们的 pid 消失，再 `/S` 静默装，
/// 最后把新版启动起来。`-Wait` 保证「装完才启动」。
///
/// 全链写日志（`apply-update.log`）：NSIS `/S` 失败是**完全静默**的 ——
/// 没有日志的话，「装完自动重启」到底成没成只能靠用户看版本号猜。
fn ps1_script(
    pid: u32,
    setup: &std::path::Path,
    exe: &std::path::Path,
    want_version: &str,
    log: &std::path::Path,
) -> String {
    let q = |p: &std::path::Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
    let mut s = String::from("\u{feff}"); // UTF-8 BOM：PowerShell 5.1 才认得出中文路径
    s.push_str("$ErrorActionPreference = 'SilentlyContinue'\r\n");
    s.push_str(&format!("$log = {}\r\n", q(log)));
    s.push_str(&format!("$want = '{}'\r\n", want_version.replace('\'', "''")));
    s.push_str(
        "function Note($m) { Add-Content -LiteralPath $log -Value ((Get-Date -Format 'yyyy-MM-dd HH:mm:ss') + ' ' + $m) -Encoding UTF8 }\r\n",
    );
    s.push_str(&format!(
        "Note ('start: host pid={pid}, setup=' + {} )\r\n",
        q(setup)
    ));
    s.push_str(&format!("$target = {pid}\r\n"));
    s.push_str("$i = 0\r\n");
    s.push_str("for ($i = 0; $i -lt 240; $i++) {\r\n");
    s.push_str("  if (-not (Get-Process -Id $target -ErrorAction SilentlyContinue)) { break }\r\n");
    s.push_str("  Start-Sleep -Milliseconds 250\r\n");
    s.push_str("}\r\n");
    s.push_str("Note ('host gone after ' + ($i * 250) + ' ms')\r\n");
    s.push_str("Start-Sleep -Milliseconds 800\r\n");
    // WebView2 子进程（msedgewebview2）可能比宿主多活几秒，还握着 exe 的句柄；
    // 不等它们退出，NSIS /S 会「覆盖失败、静默中止」，看起来像装好了其实没装。
    s.push_str(&format!("$exeName = (Split-Path -Leaf {})\r\n", q(exe)));
    s.push_str("$j = 0\r\n");
    s.push_str("for ($j = 0; $j -lt 120; $j++) {\r\n");
    s.push_str("  $left = Get-CimInstance Win32_Process -Filter \"Name = 'msedgewebview2.exe'\" |\r\n");
    s.push_str("    Where-Object { $_.CommandLine -and $_.CommandLine.Contains($exeName) }\r\n");
    s.push_str("  if (-not $left) { break }\r\n");
    s.push_str("  Start-Sleep -Milliseconds 250\r\n");
    s.push_str("}\r\n");
    s.push_str("Note ('webview gone after ' + ($j * 250) + ' ms')\r\n");
    s.push_str("Start-Sleep -Milliseconds 500\r\n");
    s.push_str(&format!(
        "$p = Start-Process -FilePath {} -ArgumentList '/S' -Wait -PassThru\r\n",
        q(setup)
    ));
    s.push_str("Note ('install exit code = ' + $(if ($p) { $p.ExitCode } else { 'unknown' }))\r\n");
    s.push_str("Start-Sleep -Milliseconds 500\r\n");
    // 装完核对版本：/S 失败时旧 exe 还在原位，光看「文件存在」会误判成功
    s.push_str(&format!(
        "$v = (Get-Item -LiteralPath {} -ErrorAction SilentlyContinue).VersionInfo.ProductVersion\r\n",
        q(exe)
    ));
    s.push_str("if ($v) {\r\n");
    s.push_str("  if ($want -and ($v -ne $want)) { Note ('WARN version after install = ' + $v + ', want ' + $want + ' (安装可能没生效)') }\r\n");
    s.push_str("  else { Note ('version after install = ' + $v) }\r\n");
    s.push_str("} else { Note 'WARN exe not found after install' }\r\n");
    s.push_str(&format!(
        "if (Test-Path -LiteralPath {p}) {{ Start-Process -FilePath {p}; Note 'relaunched' }}\r\n",
        p = q(exe)
    ));
    s.push_str("else { Note 'WARN exe missing, cannot relaunch' }\r\n");
    // 装成功才删安装包：注销码非 0 时留着，用户重试还能复用、也方便排查。
    // NSIS 退出后句柄通常立刻释放，但杀软扫描偶尔会晚一两秒，所以带重试。
    s.push_str(&format!("$setupPath = {}\r\n", q(setup)));
    s.push_str("if ($p -and $p.ExitCode -eq 0) {\r\n");
    s.push_str("  for ($k = 0; $k -lt 20; $k++) {\r\n");
    s.push_str("    Remove-Item -LiteralPath $setupPath -Force -ErrorAction SilentlyContinue\r\n");
    s.push_str("    if (-not (Test-Path -LiteralPath $setupPath)) { break }\r\n");
    s.push_str("    Start-Sleep -Milliseconds 250\r\n");
    s.push_str("  }\r\n");
    s.push_str("  if (Test-Path -LiteralPath $setupPath) { Note ('WARN setup still present: ' + $setupPath) }\r\n");
    s.push_str("  else { Note 'setup removed' }\r\n");
    s.push_str("} else { Note 'setup kept (installer exit code not 0)' }\r\n");
    s.push_str("Remove-Item -LiteralPath $MyInvocation.MyCommand.Path -Force\r\n");
    s
}

/// 校验通过后启动安装脚本，并在约 1.2 秒后退出本进程。
///
/// `want_version` 只用于**装完核对**（写进 apply-update.log），不参与任何安全判定。
pub fn install_setup(app: &AppHandle, path: &str, want_sha: &str, want_version: &str) -> Result<(), String> {
    log_line(&format!("[update] install start {path}"));
    let dir = update_dir();
    let canon_dir = dir
        .canonicalize()
        .map_err(|_| "临时目录里没有待安装的包（可能被杀毒软件清掉了）".to_string())?;
    let canon = std::path::Path::new(path)
        .canonicalize()
        .map_err(|e| format!("找不到安装包：{e}"))?;
    if !canon.starts_with(&canon_dir) {
        return Err("安装包不在允许的位置，拒绝执行".into());
    }
    let name = canon.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if !setup_name_ok(name) {
        return Err("安装包文件名不合法，拒绝执行".into());
    }
    let want = want_sha.trim().to_ascii_lowercase();
    if !sha256_hex_ok(&want) {
        return Err("缺少校验值，拒绝执行安装包".into());
    }
    // 前端已经拦过一道，这里再拦一次：IPC 可以被脚本直接调用
    let kind = install_kind();
    if kind != InstallKind::PerUser {
        return Err(match kind {
            InstallKind::PerMachine => {
                "当前是装在 Program Files 下的系统级版本，静默升级会装出第二份，请到发布页手动下载".to_string()
            }
            _ => "当前这份是从文件夹直接运行的副本，没有安装记录，请到发布页手动下载安装包".to_string(),
        });
    }
    // 执行前再算一次：下载完到点安装之间，文件可能被替换掉
    if sha256_file(&canon)? != want {
        let _ = std::fs::remove_file(&canon);
        return Err("安装包校验失败（内容已被改动），已删除，没有安装".into());
    }

    let exe = std::env::current_exe().map_err(|e| format!("找不到当前程序路径：{e}"))?;
    let setup_for_script = strip_verbatim(&canon);
    let exe_for_script = strip_verbatim(&exe);
    let script = dir.join("apply-update.ps1");
    let log = dir.join("apply-update.log");
    std::fs::write(
        &script,
        ps1_script(
            std::process::id(),
            std::path::Path::new(&setup_for_script),
            std::path::Path::new(&exe_for_script),
            want_version.trim(),
            &log,
        ),
    )
    .map_err(|e| format!("写安装脚本失败：{e}"))?;

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // 只加 CREATE_NO_WINDOW：powershell.exe 是控制台程序，再叠 DETACHED_PROCESS
    // 会「启动即退」，NSIS 根本不会跑。
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| {
            log_line(&format!("[update] ps1 spawn failed: {e}"));
            format!("启动安装脚本失败：{e}")
        })?;

    log_line("[update] ps1 spawned, exiting in 1.2s");
    // 留一点时间让前端把「正在安装」画完；脚本会等本进程真的退出
    std::thread::sleep(Duration::from_millis(1200));
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.2.0", "0.1.35"));
        assert!(!is_newer("0.1.35", "0.1.35"));
        assert!(!is_newer("0.1.9", "0.1.35"));
        assert!(!is_newer("nonsense", "0.1.35"));
    }

    #[test]
    fn asset_version_parse() {
        assert_eq!(version_from_asset("AssetManager_0.1.35_x64-setup.exe"), (0, 1, 35));
        assert_eq!(version_from_asset("assetmanager_1.2.3_x64-setup.exe"), (1, 2, 3));
        assert_eq!(version_from_asset("weird-setup.exe"), (0, 0, 0));
    }

    #[test]
    fn setup_name_rules() {
        assert!(setup_name_ok("AssetManager_0.1.35_x64-setup.exe"));
        assert!(!setup_name_ok("../evil-setup.exe"));
        assert!(!setup_name_ok("AssetManager_0.1.35_x64-setup.msi"));
        assert!(!setup_name_ok(""));
    }

    #[test]
    fn download_hosts_are_strict() {
        assert!(download_url_ok("https://github.com/a/b/releases/download/v1/x-setup.exe"));
        assert!(download_url_ok("https://objects.githubusercontent.com/x"));
        assert!(!download_url_ok("http://github.com/x"));
        assert!(!download_url_ok("https://evil.example/x-setup.exe"));
        assert!(download_resp_host_ok("https://gh-proxy.com/x"));
    }

    #[test]
    fn blocker_codes() {
        assert_eq!(blocker(InstallKind::PerMachine, "u", "h"), "system");
        assert_eq!(blocker(InstallKind::Portable, "u", "h"), "portable");
        assert_eq!(blocker(InstallKind::PerUser, "", "h"), "no-asset");
        assert_eq!(blocker(InstallKind::PerUser, "u", ""), "no-digest");
        assert_eq!(blocker(InstallKind::PerUser, "u", "h"), "");
    }
}
