import { t } from "./i18n";

/**
 * 标签的前端小工具：命名空间判定 + 「不再建议」否决名单。
 *
 * 与 Rust 侧 `core/src/autotag.rs` 的 NAMESPACES / EXCLUSIVE 保持一致：
 * 那边管**推导**，这边管**展示前过滤**（用户否决、手工优先）。
 * 之所以不把否决名单塞进 DB：它是纯偏好，换台机器重新选就好，不该污染素材库。
 */

export const AUTO_NAMESPACES = ["fmt", "kind", "place", "orient", "size"] as const;

export function namespaceOf(tag: string): string | null {
  const i = tag.indexOf(":");
  if (i <= 0 || i === tag.length - 1) return null;
  const ns = tag.slice(0, i);
  return (AUTO_NAMESPACES as readonly string[]).includes(ns) ? ns : null;
}

export function isAutoTag(tag: string): boolean {
  return namespaceOf(tag) !== null;
}

const DENY_KEY = "am.tags.denied.v1";

export function loadDenied(): Set<string> {
  try {
    const raw = localStorage.getItem(DENY_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(arr) ? arr.filter((x) => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

export function saveDenied(s: Set<string>) {
  try {
    localStorage.setItem(DENY_KEY, JSON.stringify([...s]));
  } catch {
    /* 写不进去就本次会话有效 */
  }
}

/** 自动标签的显示名：`fmt:jpg` → `JPG`，`place:4k` → `4K`，其它保持原样 */
export function autoTagLabel(tag: string): string {
  const ns = namespaceOf(tag);
  if (!ns) return tag;
  const v = tag.slice(ns.length + 1);
  return v.toUpperCase();
}

/** 维度名（侧栏分组用）。**取值时才翻译**：这些表是模块级常量，导入时语言还没定，
 *  用 getter 才能在切换语言后跟着变（消费方本来就会因 useT 重渲染）。 */
export const NS_LABEL: Record<string, string> = {
  get fmt() {
    return t("tag.ns_fmt");
  },
  get kind() {
    return t("tag.ns_kind");
  },
  get place() {
    return t("tag.ns_place");
  },
  get orient() {
    return t("tag.ns_orient");
  },
  get size() {
    return t("tag.ns_size");
  },
};

/* ------------------------------- 颜色 ------------------------------- */

/**
 * 标签颜色：**只认语义键，不认色值**。
 *
 * 与 Rust 侧 `core/src/db.rs::TAG_COLORS` 必须逐字一致（那边是白名单，传错会被拒）。
 * 键 → 实际颜色在 `styles/app.css` 的 `--tc-*` 里，浅色/深色各一套 ——
 * 所以这里的键在深色主题下会自动变亮，不需要（也不能）存十六进制。
 */
export const TAG_COLORS = [
  "gray",
  "red",
  "orange",
  "amber",
  "green",
  "teal",
  "blue",
  "indigo",
  "violet",
  "pink",
] as const;

export type TagColor = (typeof TAG_COLORS)[number];

/* 色名直接复用顶栏筛选菜单的翻译（同一批词，没必要各存一份） */
export const COLOR_LABEL: Record<string, string> = {
  get gray() {
    return t("topbar.color_gray");
  },
  get red() {
    return t("topbar.color_red");
  },
  get orange() {
    return t("topbar.color_orange");
  },
  get amber() {
    return t("topbar.color_amber");
  },
  get yellow() {
    return t("topbar.color_yellow");
  },
  get green() {
    return t("topbar.color_green");
  },
  get teal() {
    return t("topbar.color_teal");
  },
  get blue() {
    return t("topbar.color_blue");
  },
  get indigo() {
    return t("topbar.color_indigo");
  },
  get violet() {
    return t("topbar.color_violet");
  },
  get pink() {
    return t("topbar.color_pink");
  },
  get brown() {
    return t("topbar.color_brown");
  },
  get black() {
    return t("topbar.color_black");
  },
  get white() {
    return t("topbar.color_white");
  },
};

/** 颜色键 → class（`tc-blue` 这类，CSS 里负责把 `--tc` / `--tcs` 设好） */
export function colorClass(color?: string | null): string {
  const key = color && (TAG_COLORS as readonly string[]).includes(color) ? color : "gray";
  return `tc-${key}`;
}

/** 侧栏 / chip 上的小色点；灰就等于「没设颜色」，视觉上不要跳出来 */
export function hasCustomColor(color?: string | null): boolean {
  return !!color && color !== "gray";
}

/** 自动标签的分组标题（抽屉里按维度折叠） */
export function groupAutoTags<T extends { name: string; count: number }>(list: T[]) {
  const order = AUTO_NAMESPACES as readonly string[];
  const buckets = new Map<string, T[]>();
  for (const t of list) {
    const ns = namespaceOf(t.name);
    if (!ns) continue;
    if (!buckets.has(ns)) buckets.set(ns, []);
    buckets.get(ns)!.push(t);
  }
  return order
    .filter((ns) => (buckets.get(ns)?.length ?? 0) > 0)
    .map((ns) => ({ ns, label: NS_LABEL[ns] ?? ns, items: buckets.get(ns)! }));
}

/**
 * 色桶（T24）的名字 + 色值。**必须和 Rust `core/src/palette.rs` 的 BUCKETS 逐字一致**，
 * 顺序也一致（侧栏按这个顺序排，用户看到的顺序才是稳定的）。
 * 色值只用于侧栏那个小圆点与详情里的名字，不参与筛选。
 */
export const COLOR_BUCKETS: { key: string; label: string; hex: string }[] = [
  { key: "red", get label() { return t("topbar.color_red"); }, hex: "#d94a45" },
  { key: "orange", get label() { return t("topbar.color_orange"); }, hex: "#e08a3c" },
  { key: "amber", get label() { return t("topbar.color_amber"); }, hex: "#d9b23f" },
  { key: "yellow", get label() { return t("topbar.color_yellow"); }, hex: "#e3d94a" },
  { key: "green", get label() { return t("topbar.color_green"); }, hex: "#4f9d5d" },
  { key: "teal", get label() { return t("topbar.color_teal"); }, hex: "#3f9d94" },
  { key: "blue", get label() { return t("topbar.color_blue"); }, hex: "#4a7fd9" },
  { key: "indigo", get label() { return t("topbar.color_indigo"); }, hex: "#5b5fd0" },
  { key: "violet", get label() { return t("topbar.color_violet"); }, hex: "#8a5bc9" },
  { key: "pink", get label() { return t("topbar.color_pink"); }, hex: "#cf5b94" },
  { key: "brown", get label() { return t("topbar.color_brown"); }, hex: "#8a6647" },
  { key: "black", get label() { return t("topbar.color_black"); }, hex: "#1d1d1b" },
  { key: "gray", get label() { return t("topbar.color_gray"); }, hex: "#8b8b86" },
  { key: "white", get label() { return t("topbar.color_white"); }, hex: "#ececea" },
];
