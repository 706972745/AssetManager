import { invoke } from "@tauri-apps/api/core";

/**
 * 托盘自绘菜单的逻辑（tray.html 的脚本，独立 vite 入口）。
 *
 * 职责边界：这里**只渲染 + invoke**。窗口的创建 / 定位 / 显示 / 失焦隐藏、以及
 * **二级面板开合的几何**（加宽、主卡片锚定、方向判定）全在 Rust 侧
 * （main.rs 的 open_tray_menu / build_tray_menu_window / tray_menu_sub）；
 * 每次显示 Rust 直接 eval `window.__trayShow()`，本页收到后重置为干净的主菜单态、
 * 重新拉一次 `tray_menu_data` 并重放入场动画。
 *
 * 为什么不用 `emit_to("tray-menu", ...)` + listen：`tray-menu` 不在
 * capabilities/default.json 的 windows 白名单里（那个文件不在本任务的改动范围），
 * 事件监听会缺权限；自定义命令不走 ACL，所以显示信号改由 Rust eval 触发。
 *
 * 九语文案维护在下面的 L10N 常量（独立页不加载主应用 i18n，也不参与
 * `check-locales.mjs`）。其中 settings / screenshots 等译名照抄 locales JSON，
 * 其余按 FontManager tray.ts 的同一套译法。
 */

/** 后端 `tray_menu_data` 的载荷（camelCase） */
interface TrayMenuData {
  /** 全部可用库（用户库 + 内置截图库）；len <= 1 时不渲染切换项 */
  libs: TrayLib[];
  /** 后端镜像的最后主题偏好（localStorage 读不到时兜底） */
  theme: string;
  version: string;
}

interface TrayLib {
  path: string;
  label: string;
  current: boolean;
  builtin: boolean;
}

/** `tray_check_update` 返回里托盘用得到的字段 */
interface UpdateBrief {
  current: string;
  latest: string;
  hasUpdate: boolean;
}

/** 二级面板标识：与 Rust `tray_menu_sub` 的 panel 参数同一份字面 */
type SubPanel = "libs" | "theme";

type ThemePref = "system" | "light" | "dark";

const PREFS_KEY = "am.prefs.v1";
const LOCALE_KEY = "am.locale";
const ACTIVE_LIB_KEY = "am.lib.active.v1";

/** 九种界面语言（与 src/lib/i18n.ts 的 LOCALES 同一份口径、同一顺序） */
const LOCALE_CODES = [
  "en",
  "de",
  "es",
  "fr",
  "pt-BR",
  "ru",
  "ja",
  "zh-CN",
  "zh-TW",
] as const;

type LocaleCode = (typeof LOCALE_CODES)[number];

type TrayKey =
  | "openMain"
  | "settings"
  | "libs"
  | "theme"
  | "themeSystem"
  | "themeDark"
  | "themeLight"
  | "checkUpdate"
  | "checking"
  | "upToDate"
  | "found"
  | "failed"
  | "quit"
  | "screenshots";

/** 九语文案表（逐语言一列；译法照 FontManager 的 tray.ts，术语换成本项目） */
const L10N: Record<LocaleCode, Record<TrayKey, string>> = {
  en: {
    openMain: "Open main window",
    settings: "Open settings",
    libs: "Switch library",
    theme: "Switch theme",
    themeSystem: "System",
    themeDark: "Dark",
    themeLight: "Light",
    checkUpdate: "Check for updates",
    checking: "Checking…",
    upToDate: "You're up to date",
    found: "New version {v} available",
    failed: "Couldn't check, try again later",
    quit: "Quit",
    screenshots: "Screenshots",
  },
  de: {
    openMain: "Hauptfenster öffnen",
    settings: "Einstellungen öffnen",
    libs: "Bibliothek wechseln",
    theme: "Design wechseln",
    themeSystem: "System",
    themeDark: "Dunkel",
    themeLight: "Hell",
    checkUpdate: "Jetzt prüfen",
    checking: "Wird geprüft…",
    upToDate: "Alles aktuell",
    found: "Neue Version {v} verfügbar",
    failed: "Prüfung fehlgeschlagen, später erneut versuchen",
    quit: "Beenden",
    screenshots: "Screenshots",
  },
  es: {
    openMain: "Abrir ventana principal",
    settings: "Abrir ajustes",
    libs: "Cambiar biblioteca",
    theme: "Cambiar tema",
    themeSystem: "Sistema",
    themeDark: "Oscuro",
    themeLight: "Claro",
    checkUpdate: "Comprobar ahora",
    checking: "Comprobando…",
    upToDate: "Ya está actualizado",
    found: "Nueva versión {v} disponible",
    failed: "No se pudo comprobar, inténtalo más tarde",
    quit: "Salir",
    screenshots: "Capturas",
  },
  fr: {
    openMain: "Ouvrir la fenêtre principale",
    settings: "Ouvrir les réglages",
    libs: "Changer de bibliothèque",
    theme: "Changer de thème",
    themeSystem: "Système",
    themeDark: "Sombre",
    themeLight: "Clair",
    checkUpdate: "Vérifier maintenant",
    checking: "Vérification…",
    upToDate: "Déjà à jour",
    found: "Nouvelle version {v} disponible",
    failed: "Vérification impossible, réessayez plus tard",
    quit: "Quitter",
    screenshots: "Captures",
  },
  "pt-BR": {
    openMain: "Abrir janela principal",
    settings: "Abrir configurações",
    libs: "Trocar biblioteca",
    theme: "Trocar tema",
    themeSystem: "Sistema",
    themeDark: "Escuro",
    themeLight: "Claro",
    checkUpdate: "Verificar agora",
    checking: "Verificando…",
    upToDate: "Você está atualizado",
    found: "Nova versão {v} disponível",
    failed: "Não foi possível verificar, tente mais tarde",
    quit: "Sair",
    screenshots: "Capturas",
  },
  ru: {
    openMain: "Открыть главное окно",
    settings: "Открыть настройки",
    libs: "Сменить библиотеку",
    theme: "Сменить тему",
    themeSystem: "Системная",
    themeDark: "Тёмная",
    themeLight: "Светлая",
    checkUpdate: "Проверить сейчас",
    checking: "Проверка…",
    upToDate: "У вас последняя версия",
    found: "Доступна новая версия {v}",
    failed: "Не удалось проверить, попробуйте позже",
    quit: "Выход",
    screenshots: "Снимки",
  },
  ja: {
    openMain: "メインウィンドウを開く",
    settings: "設定を開く",
    libs: "ライブラリを切り替え",
    theme: "テーマを切り替え",
    themeSystem: "システム",
    themeDark: "ダーク",
    themeLight: "ライト",
    checkUpdate: "今すぐ確認",
    checking: "確認中…",
    upToDate: "最新です",
    found: "新しいバージョン {v} があります",
    failed: "確認できませんでした。後でもう一度",
    quit: "終了",
    screenshots: "スクリーンショット",
  },
  "zh-CN": {
    openMain: "打开主界面",
    settings: "打开设置",
    libs: "切换素材库",
    theme: "切换主题",
    themeSystem: "跟随系统",
    themeDark: "深色",
    themeLight: "浅色",
    checkUpdate: "检查更新",
    checking: "检查中…",
    upToDate: "已是最新",
    found: "发现新版 {v}",
    failed: "检查失败，稍后再试",
    quit: "退出软件",
    screenshots: "截图",
  },
  "zh-TW": {
    openMain: "開啟主介面",
    settings: "開啟設定",
    libs: "切換素材庫",
    theme: "切換主題",
    themeSystem: "跟隨系統",
    themeDark: "深色",
    themeLight: "淺色",
    checkUpdate: "檢查更新",
    checking: "正在檢查…",
    upToDate: "已是最新",
    found: "發現新版 {v}",
    failed: "檢查失敗，稍後再試",
    quit: "結束軟體",
    screenshots: "截圖",
  },
};

/** 与 i18n.ts 的 REGION_HINTS 同一小份（托盘解析 system 只用得上这几条） */
const REGION_HINTS: Record<string, LocaleCode> = {
  zh: "zh-CN",
  "zh-hans": "zh-CN",
  "zh-cn": "zh-CN",
  "zh-sg": "zh-CN",
  "zh-my": "zh-CN",
  "zh-hant": "zh-TW",
  "zh-tw": "zh-TW",
  "zh-hk": "zh-TW",
  "zh-mo": "zh-TW",
  pt: "pt-BR",
  "pt-pt": "pt-BR",
};

/** 读 `am.prefs.v1`（两处窗口共用同一个 origin 的 localStorage，splash 同法） */
function readPrefs(): Record<string, unknown> {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return {};
    const p: unknown = JSON.parse(raw);
    return p && typeof p === "object" ? (p as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** 主题偏好：localStorage 是主界面的唯一事实来源；读不到返回 null（走托盘镜像） */
function readThemePref(): ThemePref | null {
  const t = readPrefs().theme;
  return t === "light" || t === "dark" ? t : t === "system" ? "system" : null;
}

/** 只改 theme 一个键，别把用户其它偏好冲掉（与 App.tsx 的 savePrefs 同一份 JSON） */
function writeThemePref(mode: ThemePref): void {
  try {
    const p = readPrefs();
    p.theme = mode;
    localStorage.setItem(PREFS_KEY, JSON.stringify(p));
  } catch {
    /* 隐私模式写不进去就本次会话有效 */
  }
}

function writeActiveLib(path: string): void {
  try {
    localStorage.setItem(ACTIVE_LIB_KEY, path);
  } catch {
    /* 同上 */
  }
}

/** 语言偏好：`am.locale` 是 i18n 的缓存；再退 `am.prefs.v1` 的 locale */
function readLocalePref(): string {
  try {
    const v = localStorage.getItem(LOCALE_KEY);
    if (v) return v;
  } catch {
    /* 读不到就继续往下退 */
  }
  const v = readPrefs().locale;
  return typeof v === "string" && v ? v : "system";
}

/** "system"（或坏值）→ 按 navigator.languages 逐级匹配；与 i18n.ts 同判据 */
function resolveLocale(): LocaleCode {
  const pref = readLocalePref();
  if ((LOCALE_CODES as readonly string[]).includes(pref)) return pref as LocaleCode;
  const tags =
    typeof navigator !== "undefined" && navigator.languages?.length
      ? navigator.languages
      : [navigator.language];
  for (const tag of tags) {
    if (!tag) continue;
    const lower = tag.toLowerCase();
    if ((LOCALE_CODES as readonly string[]).includes(lower)) return lower as LocaleCode;
    if (REGION_HINTS[lower]) return REGION_HINTS[lower];
    const primary = lower.split("-")[0];
    if (REGION_HINTS[primary]) return REGION_HINTS[primary];
    const byPrimary = (LOCALE_CODES as readonly string[]).find(
      (c) => c.toLowerCase().split("-")[0] === primary,
    );
    if (byPrimary) return byPrimary as LocaleCode;
  }
  return "en";
}

/** 文案插值：{v} → 版本号 */
function fill(template: string, vars: Record<string, string>): string {
  return template.replace(/\{(\w+)\}/g, (whole, key: string) => vars[key] ?? whole);
}

// ───────────────── 渲染 ─────────────────

let shell: HTMLElement | null = null;
let mainCard: HTMLElement | null = null;
let subCard: HTMLElement | null = null;
/** 检查更新进行中：防重入，且 __trayShow 的重渲染要避开这一项 */
let updateBusy = false;
/** 最近一次渲染的数据（重渲染时复用） */
let lastData: TrayMenuData | null = null;
/** 当前展开的二级面板（null = 收起） */
let openSub: SubPanel | null = null;
/** 当前主题偏好（localStorage 优先，镜像兜底） */
let currentTheme: ThemePref = "system";
/** 父项悬停计时器（200ms 开面板） */
let hoverTimer = 0;
/** 指针离开壳后的收面板计时器（250ms） */
let leaveTimer = 0;
const SUB_HOVER_OPEN_MS = 200;
const SUB_LEAVE_CLOSE_MS = 250;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** 菜单行：mousedown preventDefault 防止点按时窗口先失焦（失焦即藏）。 */
function guardFocus(row: HTMLElement): void {
  row.addEventListener("mousedown", (e) => e.preventDefault());
}

/** 一行普通菜单项（纯文字，动作由 Rust 命令完成） */
function actionRow(label: string, onClick: () => void): HTMLButtonElement {
  const row = el("button", "trm-item");
  row.type = "button";
  row.appendChild(el("span", "trm-label", label));
  row.addEventListener("pointerenter", () => {
    if (openSub) void closeSubPanel();
  });
  guardFocus(row);
  row.addEventListener("click", onClick);
  return row;
}

/** 二级父项：文字右侧画淡色 ▸；悬停 200ms 或点击开/关面板 */
function parentRow(label: string, panel: SubPanel): HTMLButtonElement {
  const row = el("button", "trm-item");
  row.type = "button";
  row.appendChild(el("span", "trm-label", label));
  row.appendChild(el("span", "trm-arrow", "▸"));
  row.addEventListener("mouseenter", () => {
    window.clearTimeout(leaveTimer);
    if (openSub === panel) return;
    window.clearTimeout(hoverTimer);
    hoverTimer = window.setTimeout(() => void openSubPanel(panel), SUB_HOVER_OPEN_MS);
  });
  row.addEventListener("mouseleave", () => window.clearTimeout(hoverTimer));
  guardFocus(row);
  row.addEventListener("click", () => {
    window.clearTimeout(hoverTimer);
    if (openSub === panel) {
      void closeSubPanel();
    } else {
      void openSubPanel(panel);
    }
  });
  return row;
}

/** 二级面板里的一行（纯文字 + 行首 ✓/空位，与主菜单同一套对齐口径） */
function subRow(
  label: string,
  hint: string | null,
  selected: boolean,
  onClick: () => void,
): HTMLButtonElement {
  const row = el("button", "trm-opt");
  row.type = "button";
  row.appendChild(el("span", "trm-check", selected ? "✓" : ""));
  row.appendChild(el("span", "trm-name", label));
  if (hint) row.appendChild(el("span", "trm-dir", hint));
  guardFocus(row);
  row.addEventListener("click", () => {
    // 点任何子项 = 执行动作并整菜单收起（Rust 命令内部 hide 菜单并复位几何状态）
    resetSubDom();
    onClick();
  });
  return row;
}

/** 检查更新项：点击后文字在原地变状态，3 秒后恢复（不弹设置页、不下载安装） */
function updateRow(label: string): HTMLButtonElement {
  const row = actionRow(label, () => void runUpdateCheck(row));
  row.dataset.label = label;
  return row;
}

async function runUpdateCheck(row: HTMLButtonElement): Promise<void> {
  if (updateBusy) return;
  const copy = L10N[resolveLocale()];
  updateBusy = true;
  row.disabled = true;
  row.classList.add("trm-update-busy");
  row.querySelector(".trm-label")!.textContent = copy.checking;
  const restore = (): void => {
    row.classList.remove("trm-update-busy", "trm-update-found", "trm-update-failed");
    row.querySelector(".trm-label")!.textContent = row.dataset.label ?? "";
    row.disabled = false;
    updateBusy = false;
  };
  try {
    const info = await invoke<UpdateBrief>("tray_check_update", { force: true });
    row.classList.remove("trm-update-busy");
    if (info?.hasUpdate) {
      row.classList.add("trm-update-found");
      row.querySelector(".trm-label")!.textContent = fill(copy.found, { v: info.latest });
    } else {
      row.classList.add("trm-update-failed");
      row.querySelector(".trm-label")!.textContent = copy.upToDate;
    }
    window.setTimeout(restore, 3000);
  } catch {
    // 无网络 / 接口失败：说人话，绝不静默
    row.classList.remove("trm-update-busy");
    row.classList.add("trm-update-failed");
    row.querySelector(".trm-label")!.textContent = copy.failed;
    window.setTimeout(restore, 3000);
  }
}

/** 「退出软件」行：右侧带版本号副标题 */
function quitRow(label: string, version: string): HTMLButtonElement {
  const row = el("button", "trm-item");
  row.type = "button";
  row.appendChild(el("span", "trm-label", label));
  if (version) row.appendChild(el("span", "trm-ver", `v${version}`));
  guardFocus(row);
  row.addEventListener("click", () => void invoke("tray_quit"));
  return row;
}

/** 按数据整棵重建主菜单（每次显示都重建一次，几十个节点的开销可以忽略） */
function render(data: TrayMenuData): void {
  if (!mainCard) return;
  lastData = data;
  const copy = L10N[resolveLocale()];
  mainCard.replaceChildren();

  // 排序：打开主界面 → 打开设置 → [切换素材库 ▸] → 切换主题 ▸ →
  // 检查更新 → 分隔线 → 退出软件。分隔线只有一条。
  mainCard.appendChild(
    actionRow(copy.openMain, () => void invoke("tray_show_main")),
  );
  mainCard.appendChild(
    actionRow(copy.settings, () => void invoke("tray_open_settings")),
  );
  if (data.libs.length > 1) {
    mainCard.appendChild(parentRow(copy.libs, "libs"));
  }
  mainCard.appendChild(parentRow(copy.theme, "theme"));
  mainCard.appendChild(updateRow(copy.checkUpdate));
  mainCard.appendChild(el("div", "trm-sep"));
  mainCard.appendChild(quitRow(copy.quit, data.version));
}

/** 重建二级面板内容 */
function renderSub(panel: SubPanel): void {
  if (!subCard || !lastData) return;
  const copy = L10N[resolveLocale()];
  subCard.replaceChildren();
  if (panel === "libs") {
    for (const lib of lastData.libs) {
      subCard.appendChild(
        subRow(
          lib.builtin ? copy.screenshots : lib.label,
          lib.builtin || lib.label === tailOf(lib.path) ? null : tailOf(lib.path),
          lib.current,
          () => {
            // 主窗的 activeLib 是 localStorage 里的路径；两个 webview 同源共享
            writeActiveLib(lib.path);
            void invoke("tray_switch_lib", { path: lib.path });
          },
        ),
      );
    }
  } else {
    const rows: Array<[ThemePref, string]> = [
      ["system", copy.themeSystem],
      ["dark", copy.themeDark],
      ["light", copy.themeLight],
    ];
    for (const [mode, label] of rows) {
      subCard.appendChild(
        subRow(label, null, currentTheme === mode, () => {
          writeThemePref(mode);
          currentTheme = mode;
          void invoke("tray_set_theme", { mode });
        }),
      );
    }
  }
}

/** 目录末段（弱化提示），路径分隔符兼容两种写法 */
function tailOf(dir: string): string {
  const norm = dir.replace(/[\\/]+$/, "");
  const idx = Math.max(norm.lastIndexOf("\\"), norm.lastIndexOf("/"));
  return idx >= 0 ? norm.slice(idx + 1) : norm;
}

/**
 * 打开二级面板：Rust 先改几何（加宽窗口、主卡片锚定、判定方向并返回），
 * JS 再挂方向类 + 填内容 —— 顺序反了会闪一帧错位。
 */
async function openSubPanel(panel: SubPanel): Promise<void> {
  if (!shell || openSub === panel) return;
  try {
    const side = await invoke<string>("tray_menu_sub", { panel, open: true });
    openSub = panel;
    shell.classList.remove("sub-left", "sub-right");
    shell.classList.add(side === "right" ? "sub-right" : "sub-left");
    renderSub(panel);
  } catch {
    // 几何调不动（极端时序）：面板不出，主菜单保持可用
  }
}

/** 收起二级面板：几何复原（Rust）+ DOM 复位（本页） */
async function closeSubPanel(): Promise<void> {
  if (!openSub) return;
  openSub = null;
  shell?.classList.remove("sub-left", "sub-right");
  if (subCard) subCard.replaceChildren();
  try {
    await invoke("tray_menu_sub", { panel: "", open: false });
  } catch {
    // 几何复原失败不致命：下次 __trayShow 时 Rust 会按主菜单尺寸重设
  }
}

/** 只复位 DOM 状态（点子项后 Rust 会整菜单收起，几何走 hide 路径，不必再调命令） */
function resetSubDom(): void {
  openSub = null;
  shell?.classList.remove("sub-left", "sub-right");
  if (subCard) subCard.replaceChildren();
}

/** 重放入场动画（窗口复用，页面不重载，动画要手动放；两张卡一起放） */
function replayEntrance(): void {
  for (const card of [mainCard, subCard]) card?.classList.remove("in");
  // 强制回流才能让同一个动画重放
  if (mainCard) void mainCard.offsetWidth;
  if (subCard) void subCard.offsetWidth;
  for (const card of [mainCard, subCard]) card?.classList.add("in");
}

/** 主题落到 <html> 上；system 不写属性，交给 @media */
function applyTheme(pref: ThemePref): void {
  if (pref === "light" || pref === "dark") {
    document.documentElement.dataset.theme = pref;
  } else {
    delete document.documentElement.dataset.theme;
  }
}

/** 建壳与两张卡（只做一次；后续 render / renderSub 只换内容） */
function buildShell(): void {
  if (!shell) return;
  shell.replaceChildren();
  mainCard = el("div", "trm-card trm-main");
  subCard = el("div", "trm-card trm-sub");
  shell.appendChild(subCard);
  shell.appendChild(mainCard);
}

/** 拉一次菜单数据并整棵重建（显示时 / 首帧各一次） */
async function refresh(): Promise<void> {
  // localStorage 是主题偏好的主来源；镜像只有读不到时才用
  const localTheme = readThemePref();
  try {
    const data = await invoke<TrayMenuData>("tray_menu_data");
    const mirror: ThemePref = data.theme === "light" || data.theme === "dark" ? data.theme : "system";
    currentTheme = localTheme ?? mirror;
    applyTheme(currentTheme);
    render(data);
  } catch {
    // 拉不到也用一份最小数据把菜单画出来（打开主界面 / 设置 / 主题 / 退出都还能用），
    // 下次显示 __trayShow 会再拉一次 —— 空白菜单比没有菜单更难用。
    currentTheme = localTheme ?? "system";
    applyTheme(currentTheme);
    render(lastData ?? { libs: [], theme: "system", version: "" });
  }
}

async function main(): Promise<void> {
  shell = document.getElementById("trm");
  if (!shell) return;
  buildShell();
  await refresh();
  replayEntrance();

  // Rust 每次 show 都会 eval 这个入口：复位二级态 + 重拉数据 + 重放动画。
  // 检查更新进行中不重建主菜单（会把结果态冲掉），只重放动画。
  window.__trayShow = () => {
    resetSubDom();
    if (!updateBusy) void refresh();
    replayEntrance();
  };

  // 指针离开主卡与二级面板两者 ≥250ms → 收面板（主菜单保持）。
  shell.addEventListener("mouseleave", () => {
    if (!openSub) return;
    window.clearTimeout(leaveTimer);
    leaveTimer = window.setTimeout(() => {
      window.clearTimeout(hoverTimer);
      if (openSub) void closeSubPanel();
    }, SUB_LEAVE_CLOSE_MS);
  });
  shell.addEventListener("mouseenter", () => {
    window.clearTimeout(leaveTimer);
    window.clearTimeout(hoverTimer);
  });

  // Esc 收起整个菜单（点别处由 Rust 侧失焦 hide + 前台轮询兜底）
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") void invoke("tray_menu_hide");
  });
}

declare global {
  interface Window {
    /** Rust 每次显示菜单时 eval 的入口 */
    __trayShow?: () => void;
  }
}

void main();
