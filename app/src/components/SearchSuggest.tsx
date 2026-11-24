import { useEffect, useRef, useState } from "react";
import { badge, type Source, type TagCount } from "../types";
import { Icon } from "./ui";

/**
 * 顶栏搜索的下拉建议（0.3.30，照 FontManager 的 `SearchSuggest`）。
 *
 * 为什么要做：几十万张图里，用户只记得"那张红色的封面"或"文件名里有 11-18"，
 * 敲两个字就得靠列表自己跳 —— 而列表是分页的（一屏 200 条），
 * **命中在第 3 页就等于"没搜到"**。下拉直接把最像的几条名字摆出来，
 * 点一下就把关键词填进去，列表立刻只剩那几条。
 *
 * 两类建议（排序上标签在前）：标签总共几十个、命中往往就是他要的；
 * 图片动辄几千条命中，只取前 6 条。
 *
 * 与 FontManager 的差异（"个别适配"）：
 * - 字体换**图片**：右侧小字写扩展名（JPG/PNG…），不是"字体"；
 * - 图片的候选**走后端** `am_list` 的 text 过滤（FTS trigram + LIKE 兜底），
 *   不在前端遍历 —— 前端手里没有全库文件名，只有当前这一页。
 * - 不做「插件 / 分组 / 快照」三类：这个软件里搜这几样各有专门入口，塞进下拉只会更长。
 */
export type Suggest = {
  kind: "asset" | "tag";
  key: string;
  /** 给用户看的文字 */
  label: string;
  /** 右侧灰色小字：图片是扩展名，标签是张数 */
  meta?: string;
  /** 点下去写进搜索框的值 */
  value: string;
};

const LIMIT_ASSET = 6;
const LIMIT_TAG = 3;
/** 防抖：一次 `am_list` 在几十万行的库上是 20–30ms，逐键发会明显卡顿 */
const DEBOUNCE_MS = 130;

/**
 * 建议查询的**范围**：与主列表 `buildQuery` 同口径（当前目录 / 类型 / 标签 / 颜色 /
 * 评分 / 分组 / 快照），只少了 `text / sort / cursor / limit`。
 *
 * 为什么选「跟随范围」而不是「点选时清范围」（方案 §4.3 的二选一）：
 * ① 这条只多传几个 IPC 参数，改的是本文件 + Chrome 的一行；② 要走「点选时清范围」
 * 就得从 TopBar 里回调 App 去 `setDir(null)` + 清 5 个筛选 + 弹 toast，再把
 * 「已在整个素材库中搜索」这件事讲清楚 —— 改动面大，还会让「点一下搜索框里的字」
 * 产生副作用（用户只是想补个词，却被搬到了全库）。① 的语义是 WYSIWYG：
 * 下拉里看到的，点下去一定在主列表里，因为它和主列表跑的是同一组 WHERE。
 *
 * 不做「范围命中 0 条就自动放宽到全库」：方案 §5 明确禁止自动放宽
 * （会让"筛选看起来没生效"），只保留用户显式操作这一条路。
 */
export type SuggestScope = {
  dirId: number | null;
  kinds: number[] | null;
  tag: string | null;
  color: string | null;
  minRating: number | null;
  collection: number | null;
  snapshot: number | null;
};

/** 取建议。`enabled = false`（下拉没打开）时一次 IPC 都不发。 */
export function useSuggest(
  source: Source,
  raw: string,
  tags: TagCount[],
  enabled: boolean,
  /** 当前主列表的范围（不传 = 全库全目录，浏览器预览等无范围场景） */
  scope?: SuggestScope,
): Suggest[] {
  const [items, setItems] = useState<Suggest[]>([]);
  /* 竞态：打字快的时候先发的请求可能后回来，把新结果盖成旧的 —— 序号对不上就丢掉 */
  const seq = useRef(0);

  useEffect(() => {
    const q = raw.trim();
    if (!enabled || !q) {
      setItems([]);
      return;
    }
    const my = ++seq.current;
    const lower = q.toLowerCase();
    const timer = setTimeout(() => {
      void (async () => {
        const out: Suggest[] = [];
        /* ① 标签：本地过滤（几十个，比 IPC 快） */
        let n = 0;
        for (const tg of tags) {
          if (n >= LIMIT_TAG) break;
          if (!tg.name.toLowerCase().includes(lower)) continue;
          out.push({
            kind: "tag",
            key: `tag:${tg.name}`,
            label: tg.name,
            meta: String(tg.count),
            value: tg.name,
          });
          n++;
        }
        /* ② 图片：后端模糊匹配（文件名 / 标签都参与），只要前几条。
              范围与主列表一致 —— 否则下拉里会出现别的目录 / 别的库的图，
              点一下在主列表里是 0 条（方案 P1-11）。 */
        try {
          const list = await source.list({
            dirId: scope?.dirId ?? null,
            recursive: true,
            kinds: scope?.kinds ?? null,
            text: q,
            sort: "name_asc",
            cursor: null,
            limit: LIMIT_ASSET,
            tag: scope?.tag ?? null,
            color: scope?.color ?? null,
            minRating: scope?.minRating ?? null,
            collection: scope?.collection ?? null,
            snapshot: scope?.snapshot ?? null,
          });
          if (seq.current !== my) return;
          for (const a of list) {
            out.push({
              kind: "asset",
              key: `asset:${a.id}`,
              label: a.name,
              meta: badge(a.ext),
              value: a.name,
            });
          }
        } catch {
          /* 建议失败不打扰用户：搜索框本身不依赖它，照常能搜 */
        }
        if (seq.current === my) setItems(out);
      })();
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
    /* scope 逐个字段进依赖：对象本身每次渲染可能是新引用，比对象会让建议在
       无关重渲时也重新发 IPC；只比字段则「筛选真的变了」才重查。 */
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    raw,
    enabled,
    source,
    tags,
    scope?.dirId,
    scope?.kinds,
    scope?.tag,
    scope?.color,
    scope?.minRating,
    scope?.collection,
    scope?.snapshot,
  ]);

  return items;
}

/**
 * 建议下拉。键盘（↑↓ / Enter / Esc）由父组件处理 —— 事件发生在输入框上，
 * 这里只负责"看起来能点、点得中"。
 */
export function SearchSuggest({
  items,
  active,
  onPick,
  onHover,
}: {
  items: Suggest[];
  /** 键盘当前选中的下标（-1 = 还没用方向键选过） */
  active: number;
  onPick: (s: Suggest) => void;
  onHover: (i: number) => void;
}) {
  if (!items.length) return null;
  return (
    <ul className="suggest-list" role="listbox" aria-label="搜索建议">
      {items.map((it, i) => (
        <li key={it.key}>
          <button
            role="option"
            aria-selected={i === active}
            className={`suggest-item ${i === active ? "on" : ""}`}
            data-kind={it.kind}
            /* 用 mousedown 而不是 click：输入框失焦会先把面板关掉，click 就点不到了 */
            onMouseDown={(e) => {
              if (e.button !== 0) return;
              /* 必须 preventDefault：不然输入框先失焦 → 父组件的 blur 收面板 → 这一下白点 */
              e.preventDefault();
              onPick(it);
            }}
            onMouseEnter={() => onHover(i)}
          >
            <span className="suggest-icon" aria-hidden="true">
              <Icon name={it.kind === "tag" ? "tag" : "image"} size={12} stroke={1.6} />
            </span>
            <span className="suggest-label">{it.label}</span>
            <span className="suggest-kind">{it.meta ?? ""}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}
