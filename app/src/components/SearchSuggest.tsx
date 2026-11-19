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

/** 取建议。`enabled = false`（下拉没打开）时一次 IPC 都不发。 */
export function useSuggest(
  source: Source,
  raw: string,
  tags: TagCount[],
  enabled: boolean,
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
        /* ② 图片：后端模糊匹配（文件名 / 标签都参与），只要前几条 */
        try {
          const list = await source.list({
            dirId: null,
            recursive: true,
            kinds: null,
            text: q,
            sort: "name_asc",
            cursor: null,
            limit: LIMIT_ASSET,
            tag: null,
            color: null,
            minRating: null,
            collection: null,
            snapshot: null,
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
  }, [raw, enabled, source, tags]);

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
