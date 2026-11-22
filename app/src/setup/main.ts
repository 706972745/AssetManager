/**
 * 安装器 / 卸载器的前端逻辑（路线 C，照 FontManager 的同名实现移植）。
 *
 * 三条设计约束，改之前先读：
 *
 * 1. **不引 React、不引状态库**。这份界面只有 7 屏、状态是线性的，用原生 TS 直接改
 *    DOM 就够，产物 30KB 上下 —— 这就是"0 体积 1:1 还原"的前提。
 * 2. **界面上的每个数字都由 Rust 给**（版本、路径、字节数、剩余空间）。这里不猜、
 *    不拼常量：完成屏里的路径必须是真装上的路径，不能是 `%LOCALAPPDATA%` 这种模板。
 * 3. **进度只认 `setup:progress` 事件**，不自己拍脑袋跑动画。装到哪一步就是哪一步；
 *    演练模式（没有安装包可驱动）由 Rust 发同一套事件，UI 完全不知道区别。
 *
 * 【语言】只有中文 / 英文两套，只服务安装与卸载界面，**不是**主界面的 9 语言体系：
 * 判定在 Rust（`setup_ui.rs` 的 `Lang`），优先级 `--setup-lang=` > `AM_SETUP_LANG` >
 * 系统 UI 语言；结果通过两个渠道到达本文件 —— 建窗时的 `initialization_script`
 * （`window.__AM_SETUP_LANG__`）与 `setup_env` 返回的 `lang`。两者一致时以后者兜底；
 * 页面自己也认 `?lang=zh|en`（只给浏览器里调样式用）。
 */

import "./setup.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Mode = "install" | "uninstall";
type Lang = "zh" | "en";

interface SetupEnv {
  mode: Mode;
  version: string;
  /** 用户这台机器上已安装的版本（没装就是 null） */
  installedVersion: string | null;
  /** 默认安装位置（首次安装 = %LOCALAPPDATA%\AssetManager） */
  defaultDir: string;
  /** 已安装的话 = 注册表里的真实安装位置 */
  installDir: string | null;
  /** 安装包体积（字节）—— 欢迎屏"需要空间"用 */
  payloadBytes: number;
  /** 程序本体占用（字节）—— 卸载屏"占用空间"用 */
  appBytes: number;
  /** 数据目录（索引 / 缩略图缓存 / 设置；素材文件不在这里） */
  dataDir: string;
  dataBytes: number;
  freeBytes: number;
  /** 驱动真装的安装包路径；null = 演练模式（只走界面，不写盘） */
  setupExe: string | null;
  demo: boolean;
  elevated: boolean;
  /** Rust 侧判定的界面语言（与进度事件的文案同源） */
  lang: Lang;
}

interface Progress {
  percent: number;
  /** 状态行整行 */
  label: string;
  sub: string;
  /** 追加到日志区的一行；空字符串 = 这一拍不追加 */
  log: string;
  done: boolean;
  /** 非空 = 失败，值是给人看的原因 */
  failed: string | null;
  /** 失败/取消时的收尾动作：cancelled 表示用户自己取消的 */
  cancelled: boolean;
}

/** 一个正在运行的 assetmanager.exe（Rust 侧已排除安装器自己） */
interface RunningProc {
  pid: number;
  path: string;
}

/** `setup:blocked`：引擎还没启动就被"程序还开着"挡住，等用户决定 */
interface Blocked {
  kind: Mode;
  count: number;
  pids: number[];
}

const $ = <T extends HTMLElement = HTMLElement>(sel: string) => document.querySelector(sel) as T;

const app = $("#app");

let env: SetupEnv | null = null;
let mode: Mode = "install";
let targetDir = "";
let purgeData = false;
let wantRun = true;
let wantDesktop = true;
let busy = false;
/** 用户在"程序还在运行"对话框里选了关闭之后要重做的那一步 */
let pendingRetry: (() => Promise<void>) | null = null;

/* ───────────────── 文案 ─────────────────
 * 只有 zh / en 两套，紧贴安装器这一个场景。键名按屏分组（i=安装，u=卸载），
 * `{name}` 由 t() 的第二个参数替换。改中文时**必须**同时改英文：
 * `en` 的类型是 `Record<Key, string>`，少一个键 tsc 直接不过。
 */
const zh = {
  "win.min": "最小化",
  "win.close": "关闭",

  "i0.lead1": "你的全部素材，一处管理，永不离开你的电脑。",
  "i0.lead2":
    "程序装进下面这个目录；素材库、索引与设置放在单独的数据目录里 —— 卸载时不会被删除。",
  "i0.sub": "预计 5 秒 · 无需管理员权限",
  "i0.metaDir": "安装位置",
  "i0.metaNeeded": "需要空间",
  "i0.metaContents": "包含内容",
  "i0.metaContentsVal": "程序本体 · 卸载程序",
  "i0.metaData": "数据目录",
  "i0.cancel": "取消",
  "i0.next": "继续",

  "i1.title": "选择安装位置",
  "i1.lead": "换一个盘，只影响程序文件；素材库与数据目录不在这里，也不会被搬动。",
  "i1.browse": "浏览…",
  "i1.metaNeeded": "需要空间",
  "i1.metaAfter": "完成后",
  "i1.metaAfterVal": "程序文件在此目录",
  "i1.metaUninstall": "卸载时",
  "i1.metaUninstallVal": "默认只删程序；可选清理索引与缓存",
  "i1.back": "返回",
  "i1.install": "安装",

  "i2.status": "正在安装…",
  "i2.cancel": "取消安装",

  "i3.title": "安装完成",
  "i3.run": "立即运行 AssetManager",
  "i3.desktop": "在桌面创建快捷方式",
  "i3.openDir": "打开安装目录",
  "i3.finish": "完成",

  "u0.title": "卸载 AssetManager",
  "u0.lead1": "将从这台电脑移除 AssetManager。程序文件、卸载程序和开始菜单项都会被删除。",
  "u0.lead2":
    "你的素材库、索引与设置默认保留 —— 重新安装会直接接上；素材文件本身永远不会被删除。",
  "u0.warn":
    "这台电脑上找不到 AssetManager 的卸载信息 —— 可能已经卸载过，或安装时没有写入注册表。",
  "u0.metaDir": "安装位置",
  "u0.metaUsed": "占用空间",
  "u0.metaData": "数据目录",
  "u0.purge": "同时删除程序的设置与缓存",
  "u0.purgeNote":
    "包含索引、缩略图缓存与全部设置 —— 素材文件本身不会被删除，删除后无法恢复。",
  "u0.cancel": "取消",
  "u0.uninstall": "卸载",

  "u1.status": "正在移除…",

  "u2.title": "卸载完成",
  "u2.close": "关闭",

  "run.title": "AssetManager 正在运行",
  "run.bodyInstall":
    "检测到 {n} 个 AssetManager 进程正在运行。安装需要替换程序文件，得先把它们关掉 —— 否则文件被占用，会出现「装完了版本还是旧的」。",
  "run.bodyUninstall":
    "检测到 {n} 个 AssetManager 进程正在运行。卸载需要删除程序文件，得先把它们关掉 —— 否则文件删不掉，只能留到下次开机再清理。",
  "run.noteInstall": "关闭时，正在进行的操作会丢失。",
  "run.noteUninstall": "关闭软件不会动你的素材库与设置。",
  "run.cancel": "取消",
  "run.close": "关闭软件并继续",
  "run.left":
    "还有 {n} 个 AssetManager 进程没能关闭（可能以管理员身份运行）。请手动退出 AssetManager 后再试。",
  "run.leftUnknown": "无法结束正在运行的 AssetManager，请手动退出后再试。",

  "cancel.title": "要取消安装吗？",
  "cancel.body":
    "已经写进安装目录的文件会被删除；如果这个目录是本次安装才新建的，目录本身也会一并清掉。",
  "cancel.note": "你的素材库、索引与已有设置不受影响。",
  "cancel.yes": "取消安装",
  "cancel.no": "继续安装",
  "cancel.busy": "正在取消…",

  "art.tag": "你的素材 · 你的电脑 · 你的决定",

  "env.ready": "准备就绪 · 即将开始安装",
  "env.elevated": "预计 5 秒 · 当前已是管理员权限",
  "env.installed": "检测到已安装 {v} · 将直接覆盖升级",
  "env.demoStatus": "演练模式 · 当前没有可驱动的安装包",
  "env.demoSub": "只走界面流程，不写注册表、不落文件",
  "env.runningStatus": "AssetManager 正在运行 · 安装前会请你先关闭",
  "env.runningSub": "检测到 {n} 个正在运行的实例（不影响继续）",
  "env.needed": "约 {size}",
  "env.dataWithSize": "{path}（{size}）",
  "env.notInstalledDir": "（未检测到安装）",
  "env.notInstalledBtn": "未安装",

  tick: "已用 {s} 秒",
  "tick.slow": "已用 {s} 秒 · 正在写入，请勿关闭窗口",

  checking: "正在检查这个位置…",

  "fail.install": "安装失败",
  "fail.uninstall": "卸载失败",
  "fail.logHint": "详细日志：%TEMP%\\AssetManager-setup.log",
  "fail.back": "返回",
  "fail.retry": "重试",

  "done.install": "AssetManager {v} 已安装。素材库数据在 {dir}，卸载时可以决定是否保留。",
  "done.demoTitle": "演练完成",
  "done.demoBody":
    "界面流程已经走完，但这次没有写任何文件、也没有改注册表 —— 因为没有可驱动的安装包（--setup-exe 未传入）。",
  "done.uninstallPurged":
    "AssetManager 已从这台电脑移除，程序的设置与缓存也已清理。你的素材文件仍在原处，没有被删除。",
  "done.uninstallKept":
    "AssetManager 已从这台电脑移除。索引与设置保留在 {dir}，重新安装会直接接上；素材文件没有被删除。",

  bootError: "安装界面启动失败：",
};

type Key = keyof typeof zh;

const en: Record<Key, string> = {
  "win.min": "Minimize",
  "win.close": "Close",

  "i0.lead1": "All your assets in one place — they never leave your computer.",
  "i0.lead2":
    "The program goes into the folder below. Your library, index and settings live in a separate data folder — they are not deleted when you uninstall.",
  "i0.sub": "About 5 seconds · no admin rights needed",
  "i0.metaDir": "Install location",
  "i0.metaNeeded": "Space needed",
  "i0.metaContents": "Contents",
  "i0.metaContentsVal": "Program · uninstaller",
  "i0.metaData": "Data folder",
  "i0.cancel": "Cancel",
  "i0.next": "Continue",

  "i1.title": "Choose install location",
  "i1.lead":
    "Picking another drive only moves the program files. Your library and data folder stay where they are.",
  "i1.browse": "Browse…",
  "i1.metaNeeded": "Space needed",
  "i1.metaAfter": "After install",
  "i1.metaAfterVal": "Program files in this folder",
  "i1.metaUninstall": "On uninstall",
  "i1.metaUninstallVal": "Program only by default; index and cache optional",
  "i1.back": "Back",
  "i1.install": "Install",

  "i2.status": "Installing…",
  "i2.cancel": "Cancel install",

  "i3.title": "Installation complete",
  "i3.run": "Launch AssetManager now",
  "i3.desktop": "Create a desktop shortcut",
  "i3.openDir": "Open install folder",
  "i3.finish": "Finish",

  "u0.title": "Uninstall AssetManager",
  "u0.lead1":
    "AssetManager will be removed from this computer. Program files, the uninstaller and the Start menu entry will be deleted.",
  "u0.lead2":
    "Your library, index and settings are kept by default — reinstalling picks them right up. Your asset files are never touched.",
  "u0.warn":
    "No uninstall information found for AssetManager — it may already be removed, or it was installed without registry entries.",
  "u0.metaDir": "Install location",
  "u0.metaUsed": "Space used",
  "u0.metaData": "Data folder",
  "u0.purge": "Also delete the app's settings and cache",
  "u0.purgeNote":
    "Includes the index, thumbnail cache and all settings. Your asset files are not deleted; this cannot be undone.",
  "u0.cancel": "Cancel",
  "u0.uninstall": "Uninstall",

  "u1.status": "Removing…",

  "u2.title": "Uninstalled",
  "u2.close": "Close",

  "run.title": "AssetManager is running",
  "run.bodyInstall":
    "{n} AssetManager process(es) are running. Installing replaces program files, so they must be closed first — otherwise the files stay locked and the upgrade won't take effect.",
  "run.bodyUninstall":
    "{n} AssetManager process(es) are running. Uninstalling deletes program files, so they must be closed first — otherwise they can only be removed after a restart.",
  "run.noteInstall": "Anything in progress is lost when the app closes.",
  "run.noteUninstall": "Closing the app won't touch your library or settings.",
  "run.cancel": "Cancel",
  "run.close": "Close app and continue",
  "run.left":
    "{n} AssetManager process(es) could not be closed (they may be running as administrator). Quit AssetManager manually and try again.",
  "run.leftUnknown": "Could not close the running AssetManager. Quit it manually and try again.",

  "cancel.title": "Cancel the installation?",
  "cancel.body":
    "Files already written to the install folder will be removed; if this folder was created by this install, the folder itself is removed too.",
  "cancel.note": "Your library, index and existing settings are unaffected.",
  "cancel.yes": "Cancel install",
  "cancel.no": "Keep installing",
  "cancel.busy": "Cancelling…",

  "art.tag": "Your assets · Your computer · Your call",

  "env.ready": "Ready · about to start",
  "env.elevated": "About 5 seconds · already running as administrator",
  "env.installed": "Detected {v} · will upgrade in place",
  "env.demoStatus": "Demo mode · no installer package to drive",
  "env.demoSub": "UI walkthrough only — no registry changes, no files",
  "env.runningStatus": "AssetManager is running · you'll be asked to close it",
  "env.runningSub": "{n} running instance(s) detected (you can continue)",
  "env.needed": "About {size}",
  "env.dataWithSize": "{path} ({size})",
  "env.notInstalledDir": "(not installed)",
  "env.notInstalledBtn": "Not installed",

  tick: "{s}s elapsed",
  "tick.slow": "{s}s elapsed · writing files, keep this window open",

  checking: "Checking this folder…",

  "fail.install": "Installation failed",
  "fail.uninstall": "Uninstall failed",
  "fail.logHint": "Details: %TEMP%\\AssetManager-setup.log",
  "fail.back": "Back",
  "fail.retry": "Retry",

  "done.install":
    "AssetManager {v} is installed. Library data is in {dir}; you can choose to keep it when uninstalling.",
  "done.demoTitle": "Demo complete",
  "done.demoBody":
    "The UI flow finished, but nothing was written and no registry keys were touched — there was no installer package to drive (--setup-exe was not passed).",
  "done.uninstallPurged":
    "AssetManager has been removed and its settings and cache were cleaned up. Your asset files are untouched.",
  "done.uninstallKept":
    "AssetManager has been removed. The index and settings were kept in {dir}; reinstalling picks them right up. Your asset files are untouched.",

  bootError: "Setup UI failed to start:",
};

const dict: Record<Lang, Record<Key, string>> = { zh, en };

let lang: Lang = "zh";

/** 取文案并替换 `{name}` 占位符；缺失时退回中文，绝不把键名漏到界面上 */
function t(key: Key, vars?: Record<string, string | number>): string {
  let s: string = dict[lang][key] ?? zh[key];
  if (vars) {
    for (const [k, v] of Object.entries(vars)) s = s.split(`{${k}}`).join(String(v));
  }
  return s;
}

function parseLang(v: string | null | undefined): Lang | null {
  const s = (v ?? "").trim().toLowerCase();
  if (s.startsWith("zh")) return "zh";
  if (s.startsWith("en")) return "en";
  return null;
}

/** 系统语言兜底：只在浏览器里调样式（没有 Rust 注入）时用得上 */
function systemLang(): Lang {
  return (navigator.language || "").toLowerCase().startsWith("zh") ? "zh" : "en";
}

/**
 * 语言来源优先级（与 Rust 侧一致）：
 * 页面 `?lang=` > 建窗注入 `__AM_SETUP_LANG__` > `setup_env().lang` > navigator。
 * 前两项同步可得，所以首帧就是对的；`setup_env` 那一项只是兜底。
 */
const langParam = parseLang(new URLSearchParams(location.search).get("lang"));
const langInjected = parseLang(
  (window as unknown as { __AM_SETUP_LANG__?: string }).__AM_SETUP_LANG__,
);
const langPinned: Lang | null = langParam ?? langInjected;

/** 切语言：改 `<html lang/data-lang>`，并把所有 `data-i18n` 静态文案铺一遍 */
function setLang(next: Lang) {
  lang = next;
  document.documentElement.lang = next === "zh" ? "zh-CN" : "en";
  document.documentElement.dataset.lang = next;
  document.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    const key = el.dataset.i18n as Key;
    if (key in zh) el.textContent = t(key);
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((el) => {
    const key = el.dataset.i18nTitle as Key;
    if (key in zh) el.title = t(key);
  });
}

/* ───────────────── 小工具 ───────────────── */

function mb(bytes: number): string {
  if (!bytes) return "0 MB";
  const m = bytes / 1024 / 1024;
  return m >= 100 ? `${Math.round(m)} MB` : `${m.toFixed(1)} MB`;
}

function gb(bytes: number): string {
  const g = bytes / 1024 / 1024 / 1024;
  if (g >= 10) return `${g.toFixed(0)} GB`;
  if (g >= 1) return `${g.toFixed(1)} GB`;
  return `${(bytes / 1024 / 1024).toFixed(0)} MB`;
}

/** 只显示尾部的路径：`C:\Users\x\AppData\Local\AssetManager` → `…\Local\AssetManager` */
function tail(path: string, keep = 3): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length <= keep + 1) return path;
  return "…\\" + parts.slice(-keep).join("\\");
}

/**
 * 用户选中的目录 → 真正的安装目录：一律落到 `<所选目录>\AssetManager`。
 *
 * 用户点「桌面\新建文件夹」是想装到那儿，不是把一堆文件摊在桌面根上；
 * 已经叫 AssetManager 的目录不再套一层（`D:\AssetManager\AssetManager` 很蠢）。
 */
function normalizeInstallDir(picked: string): string {
  const p = picked.replace(/[\\/]+$/, "");
  const name = p.split(/[\\/]/).pop() ?? "";
  if (name.toLowerCase() === "assetmanager") return p;
  return `${p}\\AssetManager`;
}

/* ───────────────── 屏切换 ───────────────── */

function stepOf(name: "install" | "uninstall", n: number) {
  const attr = name === "install" ? "data-istep" : "data-ustep";
  // 安装屏和卸载屏共用同一个 #app：切到哪一套，就把另一套的 .on 摘掉。
  // 否则两个 .state.on 同时存在，任何按 `.state.on` 找当前屏的代码都会摸错套。
  app.querySelectorAll<HTMLElement>(".state.on").forEach((el) => {
    if (!el.hasAttribute(attr)) el.classList.remove("on");
  });
  app.querySelectorAll<HTMLElement>(`.state[${attr}]`).forEach((el) => {
    el.classList.toggle("on", Number(el.getAttribute(attr)) === n);
  });
  // 焦点交给主按钮：Enter 直接往下走，不用先点一下窗口
  const cur = app.querySelector<HTMLElement>(`.state[${attr}].on`);
  const primary = cur?.querySelector<HTMLButtonElement>(".btn-primary");
  primary?.focus({ preventScroll: true });
}

function goInstall(n: number) {
  stepOf("install", n);
}
function goUninstall(n: number) {
  stepOf("uninstall", n);
}

function pushLog(el: HTMLElement, line: string) {
  if (!line) return;
  const div = document.createElement("div");
  div.textContent = line;
  el.insertBefore(div, el.firstChild);
  while (el.children.length > 5) el.lastElementChild?.remove();
}

/* ───────────────── 「已用 N 秒」自己走字 ─────────────────
 * 为什么需要：Rust 那边只在**跳步**时才发一次 `sub`，装到某一步之后长时间没有
 * 新事件（真机上解压 + 注册表 + 快捷方式合计几十秒），于是「已用 0 秒」
 * 冻在那儿一动不动，进度条也不动 —— 用户只会觉得卡死了。
 *
 * 这里只接管**计时**，不碰百分比：约束 3 说的是"进度不自己拍脑袋跑"，时钟不是
 * 进度，它是"我还在干活"的唯一证据。所以：
 *   · 每秒把 sub 改成真实秒数（和 Rust 发的 `已用 N 秒` 同口径，不会互相打脸）
 *   · 超过 20 秒补一句"正在写入，请勿关闭窗口" —— 沉默超过这个时长，任何界面
 *     都会被当成死机
 *   · 给填充条 `<i>` 挂 `running` 类，由 CSS 做呼吸动画（纯视觉，不改宽度、不改百分比）
 * 收尾必须 stopTicker()：失败/取消/完成三种出口都会走到 finish*()。
 */
let tickTimer = 0;
let tickFrom = 0;

function startTicker(subSel: string, progSel: string) {
  stopTicker();
  tickFrom = Date.now();
  const sub = $(subSel);
  const prog = $(progSel);
  prog.classList.add("running");
  tickTimer = window.setInterval(() => {
    if (!busy) {
      stopTicker();
      return;
    }
    const s = Math.round((Date.now() - tickFrom) / 1000);
    sub.textContent = s >= 20 ? t("tick.slow", { s }) : t("tick", { s });
  }, 1000);
}

function stopTicker() {
  if (tickTimer) window.clearInterval(tickTimer);
  tickTimer = 0;
  document.querySelectorAll(".prog i.running").forEach((e) => e.classList.remove("running"));
}

/* ───────────────── 首屏：把真实环境填进去 ───────────────── */

async function boot() {
  // 先定语言再取环境：body 在 .ready 之前是透明的，所以这里改文案不会闪
  setLang(langPinned ?? systemLang());

  env = await invoke<SetupEnv>("setup_env");
  mode = env.mode;
  // Rust 的是权威值（它同时也决定了进度事件的文案）；只有当页面没有 ?lang / 注入
  // 指定时才采纳，避免调试覆盖被反压回去。
  if (!langPinned && env.lang && env.lang !== lang) setLang(env.lang);
  document.body.classList.toggle("mode-install", mode === "install");
  document.body.classList.toggle("mode-uninstall", mode === "uninstall");
  // 品牌行徽标 = **安装器自身版本**（env.version 来自当前 exe 的
  // CARGO_PKG_VERSION），不是"检测到的已安装版本"。升级安装时后者是机器上的
  // 旧版本（例：装 0.5.4 时检测到 0.5.3），拿它当徽标会让头部显示成旧版，
  // 和同一屏的「检测到已安装 0.5.3 · 将直接覆盖升级」并列时像是装错了版本。
  // 检测值只用于正文那句话，见下方 iStatus0。
  $("#ver").textContent = env.version;

  if (mode === "install") {
    targetDir = env.defaultDir;
    $("#mInstallDir").textContent = tail(env.defaultDir, 2);
    $("#mNeeded").textContent = t("env.needed", { size: mb(env.payloadBytes) });
    // 数据目录**不在**安装目录里：写真实路径的尾部，别写成"<安装目录>\data"骗人
    $("#mDataDir").textContent = tail(env.dataDir, 2);
    $("#iSub0").textContent = env.elevated ? t("env.elevated") : t("i0.sub");
    $("#iStatus0").textContent = env.installedVersion
      ? t("env.installed", { v: env.installedVersion })
      : t("env.ready");
    if (env.demo) {
      // 演练模式：把话说在前面，绝不让用户以为真装了
      $("#iStatus0").textContent = t("env.demoStatus");
      $("#iSub0").textContent = t("env.demoSub");
    }
    // 首屏就把"有实例没关"说出来，比等到点"安装"才弹窗从容（弹窗仍然要弹，
    // 因为在开始装之前必须确认一次）
    void invoke<RunningProc[]>("setup_running")
      .then((list) => {
        if (!list.length) return;
        $("#iStatus0").textContent = t("env.runningStatus");
        $("#iSub0").textContent = t("env.runningSub", { n: list.length });
      })
      .catch(() => {});
    goInstall(0);
  } else {
    targetDir = env.installDir ?? "";
    $("#uInstallDir").textContent = tail(targetDir || t("env.notInstalledDir"), 2);
    $("#uUsed").textContent = mb(env.appBytes);
    $("#uDataDir").textContent = t("env.dataWithSize", {
      path: tail(env.dataDir, 2),
      size: gb(env.dataBytes),
    });
    if (!env.installDir) {
      // 没装 / 注册表里没有：别让用户点一个注定失败的"卸载"
      $("#uWarn").style.display = "";
      $("#uPurgeRow").style.display = "none";
      const btn = app.querySelector<HTMLButtonElement>("[data-act='uninstall']");
      if (btn) {
        btn.disabled = true;
        btn.textContent = t("env.notInstalledBtn");
      }
    }
    goUninstall(0);
  }

  document.body.classList.add("ready");
}

/* ───────────────── 进度事件 ───────────────── */

async function wireProgress() {
  await listen<Progress>("setup:progress", (e) => {
    const p = e.payload;
    if (mode === "install") {
      if (!busy && !p.done) return; // 已经收尾的迟到事件不带节奏
      const status = $("#iStatus");
      const prog = $("#iProg");
      const sub = $("#iSub");
      status.textContent = p.label;
      prog.style.width = `${Math.max(0, Math.min(100, p.percent))}%`;
      sub.textContent = p.sub;
      pushLog($("#iLog"), p.log);
      if (p.done) finishInstall(p);
    } else {
      if (!busy && !p.done) return;
      $("#uStatus").textContent = p.label;
      $("#uProg").style.width = `${Math.max(0, Math.min(100, p.percent))}%`;
      $("#uSub").textContent = p.sub;
      pushLog($("#uLog"), p.log);
      if (p.done) finishUninstall(p);
    }
  });

  // 引擎被"程序还开着"挡住（`setup_install` 内部的兜底检测触发）。
  // 注意**不能**当成失败处理：没有 done 事件，进度屏停在原地，等用户决定。
  await listen<Blocked>("setup:blocked", (e) => {
    busy = false;
    const kind = e.payload.kind;
    pendingRetry = kind === "install" ? () => startInstall(true) : () => startUninstall(true);
    openRunModal(e.payload.count, kind);
  });
}

/* ───────────────── 程序还在运行 ───────────────── */

function openRunModal(count: number, kind: Mode) {
  $("#runTitle").textContent = t("run.title");
  $("#runBody").textContent =
    kind === "install"
      ? t("run.bodyInstall", { n: count })
      : t("run.bodyUninstall", { n: count });
  $("#runNote").textContent = kind === "install" ? t("run.noteInstall") : t("run.noteUninstall");
  $("#runModal").hidden = false;
  app
    .querySelector<HTMLButtonElement>(".modal-acts [data-act='run-close']")
    ?.focus({ preventScroll: true });
}

function closeRunModal() {
  $("#runModal").hidden = true;
}

/**
 * 动手前先看有没有别的实例在跑。有 → 弹窗并返回 true（调用方直接收工，等用户决定）。
 *
 * 这里查不到就当"没有"：真正的闸门在 Rust 侧（`setup_install` 会再查一遍并回
 * `setup:blocked`），前端这一查只是为了不把用户先扔到进度屏上再弹回来。
 */
async function blockedByRunning(next: () => Promise<void>): Promise<boolean> {
  let list: RunningProc[] = [];
  try {
    list = await invoke<RunningProc[]>("setup_running");
  } catch (err) {
    console.warn("[setup] 运行中检测失败，交给 Rust 侧兜底", err);
    return false;
  }
  if (!list.length) return false;
  pendingRetry = next;
  openRunModal(list.length, mode);
  return true;
}

/* ───────────────── 安装 ───────────────── */

function finishInstall(p: Progress) {
  busy = false;
  stopTicker();
  if (p.failed || p.cancelled) {
    // 取消：退回欢迎屏，不留半装目录（目录清理由 Rust 做）
    goInstall(p.cancelled ? 0 : 2);
    if (!p.cancelled) {
      $("#iStatus").textContent = p.failed ?? t("fail.install");
      $("#iProg").style.width = "100%";
      $("#iSub").textContent = t("fail.logHint");
      const acts = $("#iActs3");
      acts.innerHTML =
        `<button class="btn btn-ghost" data-act="to-welcome">${t("fail.back")}</button>` +
        `<button class="btn btn-primary" data-act="install">${t("fail.retry")}</button>`;
    }
    return;
  }
  const title = $("#iDoneTitle");
  const doneBody = $("#iDoneBody");
  if (env?.demo) {
    title.textContent = t("done.demoTitle");
    doneBody.textContent = t("done.demoBody");
    $("#iDoneChecks").style.display = "none";
  } else {
    title.textContent = t("i3.title");
    doneBody.textContent = t("done.install", {
      v: env?.version ?? "",
      dir: tail(env?.dataDir ?? "", 2),
    });
    $("#iDoneChecks").style.display = "";
  }
  goInstall(3);
}

async function startInstall(force = false) {
  if (busy) return;
  // 有人在跑就先问，别把用户扔进进度屏再弹回来
  if (!force && (await blockedByRunning(() => startInstall(true)))) return;
  busy = true;
  goInstall(2);
  $("#iLog").innerHTML = "";
  $("#iProg").style.width = "0%";
  $("#iStatus").textContent = t("i2.status");
  $("#iSub").textContent = t("tick", { s: 0 });
  startTicker("#iSub", "#iProg");
  try {
    await invoke("setup_install", {
      dir: targetDir,
      desktopShortcut: wantDesktop,
      runAfter: wantRun,
      forceCloseRunning: force,
    });
  } catch (err) {
    finishInstall({
      percent: 100,
      label: t("fail.install"),
      sub: String(err),
      log: "",
      done: true,
      failed: String(err),
      cancelled: false,
    });
  }
}

/* ───────────────── 卸载 ───────────────── */

function finishUninstall(p: Progress) {
  busy = false;
  stopTicker();
  if (p.failed) {
    $("#uStatus").textContent = p.failed;
    $("#uProg").style.width = "100%";
    $("#uSub").textContent = t("fail.logHint");
    return;
  }
  if (p.cancelled) {
    goUninstall(0);
    return;
  }
  $("#uDoneBody").textContent = purgeData
    ? t("done.uninstallPurged")
    : t("done.uninstallKept", { dir: tail(env?.dataDir ?? "", 2) });
  goUninstall(2);
}

async function startUninstall(force = false) {
  if (busy) return;
  if (!force && (await blockedByRunning(() => startUninstall(true)))) return;
  busy = true;
  goUninstall(1);
  $("#uLog").innerHTML = "";
  $("#uProg").style.width = "0%";
  startTicker("#uSub", "#uProg");
  $("#uStatus").textContent = t("u1.status");
  $("#uSub").textContent = t("tick", { s: 0 });
  try {
    await invoke("setup_uninstall", {
      dir: targetDir,
      purgeData,
      forceCloseRunning: force,
    });
  } catch (err) {
    finishUninstall({
      percent: 100,
      label: t("fail.uninstall"),
      sub: String(err),
      log: "",
      done: true,
      failed: String(err),
      cancelled: false,
    });
  }
}

/* ───────────────── 交互 ───────────────── */

async function pickDir() {
  const picked = await invoke<string | null>("setup_pick_dir", { current: targetDir });
  if (!picked) return;
  const dir = normalizeInstallDir(picked);
  targetDir = dir;
  $("#pathBox").textContent = dir;
  window.setTimeout(() => {
    const hint = $("#pathHint");
    hint.textContent = t("checking");
    invoke<{ ok: boolean; hint: string }>("setup_check_dir", { dir })
      .then((r) => (hint.textContent = r.hint))
      .catch(() => (hint.textContent = ""));
  }, 30);
}

function bind() {
  app.addEventListener("click", async (ev) => {
    const el = ev.target as HTMLElement;

    const winBtn = el.closest<HTMLElement>("[data-win]");
    if (winBtn) {
      const action = winBtn.dataset.win === "min" ? "minimize" : "close";
      await invoke("setup_window", { action });
      return;
    }

    const chk = el.closest<HTMLElement>("[data-chk]");
    if (chk) {
      const on = chk.querySelector(".box")!.classList.toggle("on");
      if (chk.dataset.chk === "run") wantRun = on;
      else wantDesktop = on;
      return;
    }
    if (el.closest("#uPurgeRow")) {
      purgeData = $("#uPurgeBox").classList.toggle("on");
      return;
    }

    const btn = el.closest<HTMLButtonElement>("[data-act]");
    if (!btn) return;

    switch (btn.dataset.act) {
      case "close":
        await invoke("setup_window", { action: "close" });
        break;
      case "to-location":
        $("#pathBox").textContent = targetDir;
        $("#mNeeded2").textContent = t("env.needed", { size: mb(env?.payloadBytes ?? 0) });
        goInstall(1);
        invoke<{ ok: boolean; hint: string }>("setup_check_dir", { dir: targetDir })
          .then((r) => ($("#pathHint").textContent = r.hint))
          .catch(() => ($("#pathHint").textContent = ""));
        break;
      case "browse":
        await pickDir();
        break;
      case "to-welcome":
        goInstall(0);
        break;
      case "install":
        await startInstall();
        break;
      case "run-cancel":
        // 取消 = 回到能再按一次的那一屏，什么都不改（不杀进程、不写盘）
        closeRunModal();
        pendingRetry = null;
        busy = false;
        if (mode === "install") {
          goInstall(1);
          $("#pathBox").textContent = targetDir;
        } else {
          goUninstall(0);
        }
        break;
      case "run-close": {
        // 用户明确同意关闭：这一步才允许结束别人的进程
        const btns = Array.from(app.querySelectorAll<HTMLButtonElement>(".modal-acts .btn"));
        btns.forEach((b) => (b.disabled = true));
        let left = -2;
        try {
          const r = await invoke<{ closed: number; remaining: RunningProc[] }>(
            "setup_close_running",
          );
          left = r.remaining.length;
        } catch (err) {
          console.warn("[setup] 结束进程失败", err);
        }
        btns.forEach((b) => (b.disabled = false));
        if (left !== 0) {
          $("#runBody").textContent =
            left > 0 ? t("run.left", { n: left }) : t("run.leftUnknown");
          return;
        }
        const retry = pendingRetry;
        closeRunModal();
        pendingRetry = null;
        if (retry) await retry();
        break;
      }
      case "cancel-install":
        // 不直接取消：先弹确认。屏 3 的按钮挨着「返回」，
        // 没有这一层就等于允许手滑废掉一次安装。
        $("#cancelModal").hidden = false;
        break;
      case "cancel-yes":
        $("#cancelModal").hidden = true;
        {
          const c = app.querySelector<HTMLButtonElement>("[data-act='cancel-install']");
          if (c) {
            c.textContent = t("cancel.busy");
            c.disabled = true;
          }
        }
        await invoke("setup_cancel");
        break;
      case "cancel-no":
        $("#cancelModal").hidden = true;
        break;
      case "uninstall":
        await startUninstall();
        break;
      case "open-dir":
        await invoke("setup_window", { action: "open-install-dir" });
        break;
      case "finish":
        await invoke("setup_window", {
          action: "finish",
          launch: mode === "install" ? wantRun : false,
        });
        break;
    }
  });

  document.addEventListener("keydown", (ev) => {
    if (ev.key !== "Enter" && ev.key !== "Escape") return;
    // 遮罩开着 = 必须先在"关闭软件并继续 / 取消"里选一个，别让 Enter 穿透到底下
    if (!$("#runModal").hidden) {
      ev.preventDefault();
      const act = ev.key === "Enter" ? "run-close" : "run-cancel";
      app.querySelector<HTMLButtonElement>(`[data-act='${act}']`)?.click();
      return;
    }
    // 「取消安装」的确认：Enter 与 Esc 都回「继续安装」（安全默认），
    // 想真取消必须正面点一次那个按钮。
    if (!$("#cancelModal").hidden) {
      ev.preventDefault();
      app.querySelector<HTMLButtonElement>("[data-act='cancel-no']")?.click();
      return;
    }
    // 屏号**必须**限定在当前模式里取：安装屏与卸载屏都在同一个 DOM 里，
    // 只写 `.state.on` 会摸到另一套屏（卸载时取到安装屏 1 的 data-istep，
    // 屏号算成 0，Enter 又把"卸载"按钮点了一次）。
    const cur = (mode === "install" ? "data-istep" : "data-ustep") as string;
    const n = Number(app.querySelector(`.state[${cur}].on`)?.getAttribute(cur) ?? 0);
    // 屏 3（安装中）期间所有按键都不该直接生效 —— 但 Esc 可以**开确认框**，
    // 别让只能鼠标点。
    if (busy) {
      if (ev.key === "Escape" && mode === "install" && n === 2) {
        ev.preventDefault();
        $("#cancelModal").hidden = false;
      }
      return;
    }
    // **必须吃掉默认行为**：stepOf 会把焦点交给主按钮，浏览器自己也会拿 Enter
    // 去点那个按钮 —— 否则一次按键 = 两次 click（实测：卸载被启动了两次，
    // 第二次在 uninstall.exe 已被删掉之后跑，界面报"找不到卸载程序"）。
    ev.preventDefault();
    if (ev.key === "Escape") {
      if (mode === "install" && n === 0) invoke("setup_window", { action: "close" });
      if (mode === "uninstall" && n === 0) invoke("setup_window", { action: "close" });
      if (n === 2 || n === 3) invoke("setup_window", { action: "finish", launch: false });
      return;
    }
    if (mode === "install") {
      if (n === 0) $("[data-act='to-location']").click();
      else if (n === 1) $("[data-act='install']").click();
      else if (n === 3) $("[data-act='finish']").click();
    } else {
      if (n === 0) $("[data-act='uninstall']").click();
      else if (n === 2) $("[data-act='finish']").click();
    }
  });
}

/* ───────────────── 启动 ───────────────── */

(async () => {
  bind();
  try {
    await wireProgress();
  } catch (err) {
    // 进度事件只是"锦上添花"：订阅不到就退回静态进度条，**绝不能因此白屏**。
    // （实际踩到过：listen 抛错把整个启动链拒了，.ready 永远加不上，
    //   用户看到的就是一个纯白圆角框，连报障都无从说起。）
    console.warn("[setup] 订阅进度事件失败，改用静态进度", err);
  }
  try {
    await boot();
  } catch (err) {
    showBootError(err);
  }
})();

/** 启动失败时的最后一道防线：把错误与 Tauri 内部状态摊在页面上（见 setup.html 的注释）。
 *
 * **只在界面还没就绪时接管**。界面一旦 `ready`，任何运行期错误都不许盖住界面 ——
 * 实测踩过：安装已经装完，一个 `start_dragging not allowed by ACL` 就把整屏糊成红字，
 * 用户会以为装坏了。
 */
function showBootError(err: unknown) {
  if (document.body.classList.contains("ready")) {
    console.warn("[setup] 运行期错误（界面已就绪，不覆盖）", err);
    return;
  }
  document.body.classList.add("ready");
  if (document.getElementById("boot-err")) return;
  const w = window as unknown as { __TAURI_INTERNALS__?: Record<string, unknown> };
  const internals = w.__TAURI_INTERNALS__;
  const diag = [
    `__TAURI_INTERNALS__ = ${typeof internals}`,
    `transformCallback = ${typeof internals?.transformCallback}`,
    `invoke = ${typeof internals?.invoke}`,
  ].join("\n");
  const d = document.createElement("pre");
  d.id = "boot-err";
  d.style.cssText =
    "position:fixed;inset:0;z-index:99999;margin:0;padding:18px;overflow:auto;" +
    "background:#fff;color:#b3261e;font:12px/1.55 ui-monospace,Consolas,monospace;" +
    "white-space:pre-wrap";
  d.textContent = `${t("bootError")}\n\n${String((err as { stack?: string })?.stack ?? err)}\n\n${diag}`;
  document.body.appendChild(d);
}
