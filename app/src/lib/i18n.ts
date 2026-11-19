import { useSyncExternalStore } from "react";
import de from "../locales/de.json";
import en from "../locales/en.json";
import es from "../locales/es.json";
import fr from "../locales/fr.json";
import ja from "../locales/ja.json";
import ptBR from "../locales/pt-BR.json";
import ru from "../locales/ru.json";
import zhCN from "../locales/zh-CN.json";
import zhTW from "../locales/zh-TW.json";

/**
 * 轻量 i18n —— **和 FontManager 同一套做法**（owner 2026-11-19：「可以用 fontmanager 的方法」）。
 *
 * 为什么不引 i18next：这里只需要 ① 点号无关的扁平 key ② `{name}` 插值 ③ 复数，
 * 一个 `useSyncExternalStore` 就够，打包体积还能省 40KB 左右。
 *
 * 三条硬约定：
 * 1. **`en.json` 是源**：任何 key 以它为准，其它语言缺了自动回落英文（不会出现空字符串）。
 * 2. **缺 key 不报错**：`t()` 找不到就原样返回 key —— 界面上看到 `settings.language`
 *    这种字样说明这里漏翻了，比显示空白好定位（`check-locales.mjs` 会把它列出来）。
 * 3. **复数用 `Intl.PluralRules`**：`count` 传进 `t()` 时按目标语言的规则选
 *    `key_one` / `key_other` / `key_few` / `key_many`，俄语和日语因此能各用各的。
 */
export type Dict = typeof en;
type Vars = Record<string, string | number>;

type PluralBase<K extends string> = K extends `${infer B}_one`
  ? B
  : K extends `${infer B}_other`
    ? B
    : never;

export type TKey = keyof Dict | PluralBase<keyof Dict & string>;

export type LocaleCode =
  | "en"
  | "de"
  | "es"
  | "fr"
  | "pt-BR"
  | "ru"
  | "ja"
  | "zh-CN"
  | "zh-TW";
export type LocalePref = LocaleCode | "system";

export interface LocaleInfo {
  code: LocaleCode;
  /** 该语言自己的写法（列表里给人看的） */
  name: string;
  /** 英文名（搜索/客服定位用） */
  english: string;
}

export const LOCALES: readonly LocaleInfo[] = [
  { code: "en", name: "English", english: "English" },
  { code: "de", name: "Deutsch", english: "German" },
  { code: "es", name: "Español", english: "Spanish" },
  { code: "fr", name: "Français", english: "French" },
  { code: "pt-BR", name: "Português (Brasil)", english: "Portuguese (Brazil)" },
  { code: "ru", name: "Русский", english: "Russian" },
  { code: "ja", name: "日本語", english: "Japanese" },
  { code: "zh-CN", name: "简体中文", english: "Chinese (Simplified)" },
  { code: "zh-TW", name: "繁體中文", english: "Chinese (Traditional)" },
];

const DICTS: Record<LocaleCode, Partial<Dict>> = {
  en,
  de,
  es,
  fr,
  "pt-BR": ptBR,
  ru,
  ja,
  "zh-CN": zhCN,
  "zh-TW": zhTW,
};

const FALLBACK: LocaleCode = "en";
const STORAGE_KEY = "am.locale";

/** 系统语言 → 我们支持的九种。`zh-HK`/`zh-MO` 归繁体，`pt-PT` 归巴西葡语 */
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

function isLocaleCode(v: unknown): v is LocaleCode {
  return typeof v === "string" && LOCALES.some((l) => l.code === v);
}

export function isLocalePref(v: unknown): v is LocalePref {
  return v === "system" || isLocaleCode(v);
}

function matchTag(tag: string): LocaleCode | null {
  const lower = tag.toLowerCase();
  const exact = LOCALES.find((l) => l.code.toLowerCase() === lower);
  if (exact) return exact.code;
  if (REGION_HINTS[lower]) return REGION_HINTS[lower];

  const parts = lower.split("-");
  for (let i = parts.length - 1; i >= 2; i--) {
    const shorter = parts.slice(0, i).join("-");
    if (REGION_HINTS[shorter]) return REGION_HINTS[shorter];
    const hit = LOCALES.find((l) => l.code.toLowerCase() === shorter);
    if (hit) return hit.code;
  }
  const primary = parts[0];
  if (REGION_HINTS[primary]) return REGION_HINTS[primary];
  const byPrimary = LOCALES.find((l) => l.code.toLowerCase().split("-")[0] === primary);
  return byPrimary ? byPrimary.code : null;
}

/** 跟随系统：按 `navigator.languages` 逐条匹配，一条都不中就用英文 */
export function detectLocale(): LocaleCode {
  const tags =
    typeof navigator !== "undefined"
      ? navigator.languages?.length
        ? navigator.languages
        : [navigator.language]
      : [];
  for (const tag of tags) {
    if (!tag) continue;
    const hit = matchTag(tag);
    if (hit) return hit;
  }
  return FALLBACK;
}

function readStoredPref(): LocalePref {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (isLocalePref(raw)) return raw;
  } catch {
    /* 隐私模式读不到就当跟随系统 */
  }
  return "system";
}

let pref: LocalePref = readStoredPref();
let resolved: LocaleCode = pref === "system" ? detectLocale() : pref;
let dict: Partial<Dict> = DICTS[resolved];
let plurals = new Intl.PluralRules(resolved);

let version = 0;
const listeners = new Set<() => void>();

function stampDocument() {
  if (typeof document !== "undefined") {
    document.documentElement.lang = resolved;
  }
}

function apply(next: LocalePref) {
  pref = next;
  resolved = next === "system" ? detectLocale() : next;
  dict = DICTS[resolved];
  plurals = new Intl.PluralRules(resolved);
  stampDocument();
  version++;
  for (const listen of listeners) listen();
}

stampDocument();

export function getLocalePref(): LocalePref {
  return pref;
}

export function activeLocale(): LocaleCode {
  return resolved;
}

export function setLocalePref(next: LocalePref) {
  if (next === pref) return;
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    /* 存不进去就本次会话有效 */
  }
  apply(next);
}

/** 启动时用 prefs 里的值对齐（prefs 是用户偏好的唯一事实来源之一，
 *  但 i18n 自己也存一份 —— 两边以 prefs 为准，避免"设置里显示英文、界面还是中文"）。 */
export function hydrateLocalePref(next: LocalePref) {
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    /* 同上 */
  }
  if (next !== pref) apply(next);
}

function interpolate(template: string, vars: Vars): string {
  return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in vars ? String(vars[name]) : whole,
  );
}

function lookup(key: string): string | undefined {
  const local = (dict as Record<string, string | undefined>)[key];
  if (local !== undefined) return local;
  /* 缺翻译回落 en：宁可给英文，也不要给 key 或空白 */
  return (en as Record<string, string | undefined>)[key];
}

export function t(key: TKey, vars?: Vars): string {
  let raw: string | undefined;

  if (vars && typeof vars.count === "number") {
    const category = plurals.select(vars.count);
    raw =
      (dict as Record<string, string | undefined>)[`${key}_${category}`] ??
      (dict as Record<string, string | undefined>)[`${key}_other`] ??
      lookup(`${key}_${category}`) ??
      lookup(`${key}_other`);
  }

  raw ??= lookup(key);
  if (raw === undefined) return key;
  return vars ? interpolate(raw, vars) : raw;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getVersion() {
  return version;
}

/** 组件里用这个：语言一变整棵子树重渲染 */
export function useT(): typeof t {
  useSyncExternalStore(subscribe, getVersion, getVersion);
  return t;
}

export function useLocalePref(): LocalePref {
  useSyncExternalStore(subscribe, getVersion, getVersion);
  return pref;
}

/** 当前语言在九种语言里的显示名（设置页那行「简体中文」用它） */
export function localeName(code: LocaleCode): string {
  return LOCALES.find((l) => l.code === code)?.name ?? code;
}
