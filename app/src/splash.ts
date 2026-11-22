import { invoke } from "@tauri-apps/api/core";

/**
 * 启动页（照 FontManager 的 `src/splash.ts` 移植）。
 *
 * 这一页只反映**当前素材库**的启动增量扫描进度，不代表全部库：
 * 当前库扫完主界面就接管，其余库由主界面底部的细进度条继续报（`scan://progress`）。
 *
 * 刻意不引 React、也不引 `src/lib/i18n.ts`（它会把 React 一起拖进来）：
 * 这一页的全部价值是「100ms 内出首帧」。所以文案在下面自带一张 9 语言小表，
 * 语言与主题直接从 localStorage 读（与主界面同一份 key）。
 *
 * 轮询而不是监听事件：进度是**可重复读的状态**，轮询天然幂等；
 * 启动路径上「事件在监听注册之前就发完了」几乎必然出现（splash 窗口比主窗先建）。
 */

interface SplashProgress {
  /** 正在扫的库名；空串 = 还没开始（准备中） */
  lib: string;
  /** 本轮已检查的目录数 */
  done: number;
  /** 本轮待检查的目录数；0 = 分母还没出来（不确定态） */
  total: number;
  /** 本轮重扫到的文件数 */
  files: number;
  /** 后面还有几个库要在主界面后台继续扫 */
  rest: number;
  /** 主界面已经接手，可以淡出了 */
  finished: boolean;
}

const LOCALES = ["en", "de", "es", "fr", "pt-BR", "ru", "ja", "zh-CN", "zh-TW"] as const;
type LocaleCode = (typeof LOCALES)[number];

interface CopySet {
  boot: string;
  scanning: string;
  indet: string;
  files: string;
  rest: string;
  opening: string;
}

/** 一套文案 = 一种语言。`{lib}` / `{i}` / `{n}` 由下面自己替换。 */
const COPY: Record<LocaleCode, CopySet> = {
  "zh-CN": {
    boot: "正在启动 AssetManager…",
    scanning: "正在扫描「{lib}」· {i} / {n} 个目录",
    indet: "正在扫描「{lib}」…",
    files: "已发现 {n} 个文件",
    rest: "{n} 个素材库将在主界面后台继续",
    opening: "正在打开主界面…",
  },
  "zh-TW": {
    boot: "正在啟動 AssetManager…",
    scanning: "正在掃描「{lib}」· {i} / {n} 個目錄",
    indet: "正在掃描「{lib}」…",
    files: "已發現 {n} 個檔案",
    rest: "{n} 個素材庫將在主介面後台繼續",
    opening: "正在開啟主介面…",
  },
  en: {
    boot: "Starting AssetManager…",
    scanning: "Scanning “{lib}” · {i} / {n} folders",
    indet: "Scanning “{lib}”…",
    files: "{n} files so far",
    rest: "{n} more libraries keep scanning in the background",
    opening: "Opening AssetManager…",
  },
  de: {
    boot: "AssetManager wird gestartet…",
    scanning: "„{lib}“ wird gescannt · {i} / {n} Ordner",
    indet: "„{lib}“ wird gescannt…",
    files: "Bisher {n} Dateien",
    rest: "{n} weitere Bibliotheken werden im Hintergrund gescannt",
    opening: "AssetManager wird geöffnet…",
  },
  es: {
    boot: "Iniciando AssetManager…",
    scanning: "Analizando «{lib}» · {i} / {n} carpetas",
    indet: "Analizando «{lib}»…",
    files: "{n} archivos hasta ahora",
    rest: "Otras {n} bibliotecas siguen en segundo plano",
    opening: "Abriendo AssetManager…",
  },
  fr: {
    boot: "Démarrage d’AssetManager…",
    scanning: "Analyse de « {lib} » · {i} / {n} dossiers",
    indet: "Analyse de « {lib} »…",
    files: "{n} fichiers pour l’instant",
    rest: "{n} autres bibliothèques continuent en arrière-plan",
    opening: "Ouverture d’AssetManager…",
  },
  ja: {
    boot: "AssetManager を起動しています…",
    scanning: "「{lib}」をスキャン中 · {i} / {n} フォルダー",
    indet: "「{lib}」をスキャン中…",
    files: "これまで {n} ファイル",
    rest: "残り {n} 個のライブラリはバックグラウンドで続行します",
    opening: "AssetManager を開いています…",
  },
  "pt-BR": {
    boot: "Iniciando o AssetManager…",
    scanning: "Escaneando “{lib}” · {i} / {n} pastas",
    indet: "Escaneando “{lib}”…",
    files: "{n} arquivos até agora",
    rest: "Mais {n} bibliotecas continuam em segundo plano",
    opening: "Abrindo o AssetManager…",
  },
  ru: {
    boot: "Запуск AssetManager…",
    scanning: "Сканирование «{lib}» · {i} / {n} папок",
    indet: "Сканирование «{lib}»…",
    files: "Найдено файлов: {n}",
    rest: "Ещё {n} библиотек продолжают сканироваться в фоне",
    opening: "Открываем AssetManager…",
  },
};

/** 与 `src/lib/i18n.ts` 同一个 key；主界面把用户选择写在这里。 */
const LOCALE_KEY = "am.locale";
/** 与 `src/types.ts` 的 PREFS_KEY 同一个 key（主题在那份 JSON 里）。 */
const PREFS_KEY = "am.prefs.v1";
/** 轮询间隔（与 FontManager 一致）。 */
const POLL_MS = 100;

/** 系统语言 → 九种之一。`zh-HK/MO/TW` 归繁体，`pt-PT` 归巴西葡语。 */
function matchLocale(tag: string): LocaleCode | null {
  const l = tag.toLowerCase();
  if (l.startsWith("zh")) return /hant|tw|hk|mo/.test(l) ? "zh-TW" : "zh-CN";
  if (l.startsWith("pt")) return "pt-BR";
  if (l.startsWith("ja")) return "ja";
  if (l.startsWith("ru")) return "ru";
  if (l.startsWith("de")) return "de";
  if (l.startsWith("es")) return "es";
  if (l.startsWith("fr")) return "fr";
  if (l.startsWith("en")) return "en";
  return null;
}

function resolveLocale(): LocaleCode {
  try {
    const pref = localStorage.getItem(LOCALE_KEY);
    if (pref && pref !== "system") {
      const hit = matchLocale(pref);
      if (hit) return hit;
    }
  } catch {
    /* 隐私模式读不到就当跟随系统 */
  }
  const tags =
    typeof navigator !== "undefined"
      ? navigator.languages?.length
        ? navigator.languages
        : [navigator.language]
      : [];
  for (const tag of tags) {
    if (!tag) continue;
    const hit = matchLocale(tag);
    if (hit) return hit;
  }
  return "en";
}

/**
 * 用户强制过的主题（`am.prefs.v1` 里的 `theme`）。
 * `system` / 缺失 / 读失败一律不写属性 —— `splash.html` 的
 * `@media (prefers-color-scheme: light)` 自己会给出与主界面一致的结果。
 */
function applyTheme(): void {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return;
    const pref = (JSON.parse(raw) as { theme?: unknown }).theme;
    if (pref === "light" || pref === "dark") {
      document.documentElement.dataset.theme = pref;
    }
  } catch {
    /* 读不到就跟随系统 */
  }
}

function main(): void {
  const root = document.getElementById("splash");
  const bar = document.getElementById("bar");
  const fill = document.getElementById("fill");
  const status = document.getElementById("status");
  const note = document.getElementById("note");
  const version = document.getElementById("version");
  // 结构对不上就什么都不做：宁可留一屏背景，也不要一个半挂的界面
  if (!root || !bar || !fill || !status || !note || !version) return;

  // 主题是同步的：首帧之前就把用户强制的那套落上去（系统主题由 @media 负责）
  applyTheme();

  const locale = resolveLocale();
  let copy: CopySet = COPY[locale];
  const zh = locale === "zh-CN" || locale === "zh-TW";
  let maxPercent = 0;
  let stopped = false;
  let timer = 0;

  if (typeof __APP_VERSION__ === "string" && __APP_VERSION__) {
    version.textContent = `v ${__APP_VERSION__}`;
  }

  /** 千分位：中文用空格（1 028），其余用逗号（1,028）。 */
  const group = (n: number): string => {
    const digits = String(Math.max(0, Math.round(n)));
    return digits.replace(/\B(?=(\d{3})+(?!\d))/g, zh ? " " : ",");
  };

  const interpolate = (template: string, vars: Record<string, string>): string =>
    template.replace(/\{(\w+)\}/g, (whole, key: string) => vars[key] ?? whole);

  const finish = (): void => {
    if (stopped) return;
    stopped = true;
    window.clearTimeout(timer);
    status.textContent = copy.opening;
    root.classList.add("out");
    // 不在这里关窗口：关窗由 Rust 侧负责（前端 close 要额外权限，
    // 而且窗口一关 JS 上下文就没了，120ms 的淡出根本放不出来）。
  };

  const render = (state: SplashProgress): void => {
    const total = Number.isFinite(state.total) ? Math.max(0, state.total) : 0;
    const done = Number.isFinite(state.done) ? Math.max(0, state.done) : 0;
    const rest = Number.isFinite(state.rest) ? Math.max(0, state.rest) : 0;
    const files = Number.isFinite(state.files) ? Math.max(0, state.files) : 0;
    const lib = typeof state.lib === "string" ? state.lib : "";

    if (total > 0) {
      bar.classList.remove("indet");
      const percent = Math.min(100, (Math.min(done, total) / total) * 100);
      // 进度只许往前走：两次轮询之间分母变化会让算出来的百分比回退
      maxPercent = Math.max(maxPercent, percent);
      fill.style.width = `${maxPercent}%`;
      status.textContent = interpolate(copy.scanning, {
        lib,
        i: group(done),
        n: group(total),
      });
    } else {
      // 分母未知（本轮候选目录清单还没拿到）：不确定态，不显示百分比
      bar.classList.add("indet");
      status.textContent = lib ? interpolate(copy.indet, { lib }) : copy.boot;
    }

    // 副状态：文件数常驻；「其余库」只在真有剩余时出现（避免「其余 0 个」）
    const parts: string[] = [];
    if (total > 0 || files > 0) parts.push(interpolate(copy.files, { n: group(files) }));
    if (rest > 0) parts.push(interpolate(copy.rest, { n: group(rest) }));
    note.textContent = parts.join(" · ");
  };

  const poll = async (): Promise<void> => {
    if (stopped) return;
    let state: SplashProgress | null = null;
    try {
      state = await invoke<SplashProgress>("splash_state");
    } catch {
      // 命令还没注册 / 主线程正忙：下一拍再问，启动页不写任何错误
    }
    if (stopped) return;
    if (state) render(state);
    if (state?.finished) {
      finish();
      return;
    }
    timer = window.setTimeout(() => void poll(), POLL_MS);
  };

  // 第一帧：不确定态 + 本地文案，不等任何 IPC
  bar.classList.add("indet");
  render({ lib: "", done: 0, total: 0, files: 0, rest: 0, finished: false });

  void poll();
}

main();
