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

/** 维度中文名（侧栏分组用） */
export const NS_LABEL: Record<string, string> = {
  fmt: "格式",
  kind: "类型",
  place: "像素",
  orient: "比例",
  size: "体积",
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

export const COLOR_LABEL: Record<string, string> = {
  gray: "灰",
  red: "红",
  orange: "橙",
  amber: "琥珀",
  yellow: "黄",
  green: "绿",
  teal: "青",
  blue: "蓝",
  indigo: "靛",
  violet: "紫",
  pink: "粉",
  brown: "棕",
  black: "黑",
  white: "白",
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
 * 色桶（T24）的中文名 + 色值。**必须和 Rust `core/src/palette.rs` 的 BUCKETS 逐字一致**，
 * 顺序也一致（侧栏按这个顺序排，用户看到的顺序才是稳定的）。
 * 色值只用于侧栏那个小圆点与详情里的名字，不参与筛选。
 */
export const COLOR_BUCKETS: { key: string; label: string; hex: string }[] = [
  { key: "red", label: "红", hex: "#d94a45" },
  { key: "orange", label: "橙", hex: "#e08a3c" },
  { key: "amber", label: "琥珀", hex: "#d9b23f" },
  { key: "yellow", label: "黄", hex: "#e3d94a" },
  { key: "green", label: "绿", hex: "#4f9d5d" },
  { key: "teal", label: "青", hex: "#3f9d94" },
  { key: "blue", label: "蓝", hex: "#4a7fd9" },
  { key: "indigo", label: "靛", hex: "#5b5fd0" },
  { key: "violet", label: "紫", hex: "#8a5bc9" },
  { key: "pink", label: "粉", hex: "#cf5b94" },
  { key: "brown", label: "棕", hex: "#8a6647" },
  { key: "black", label: "黑", hex: "#1d1d1b" },
  { key: "gray", label: "灰", hex: "#8b8b86" },
  { key: "white", label: "白", hex: "#ececea" },
];
