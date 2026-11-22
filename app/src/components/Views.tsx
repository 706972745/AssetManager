/**
 * 主视图 —— 网格 / 瀑布流两种排布的渲染与交互（含 `Tile`）。
 *
 * 只负责"怎么摆、怎么点"：排布计算在 `lib/layout.ts`，滚动与滚动惯性在 `lib/momentumScroll.ts`，
 * 数据由 `App` 传下来。缩略图走 `thumb://` 协议，加载失败记在 `THUMB_FAILED` 里避免重试风暴。
 */
import { memo, useCallback, useLayoutEffect, useEffect, useMemo, useRef, useState } from "react";
import {
  badge,
  KIND_ICON,
  PREVIEWABLE_KINDS,
  THUMB_FAILED,
  type Asset,
} from "../types";
import {
  gridLayout,
  masonryLayout,
  MAX_COLS_GRID,
  MAX_COLS_WATERFALL,
  type Rect,
} from "../lib/layout";
import { Icon } from "./ui";

export type ViewMode = "grid" | "waterfall";

export const TILE_SIZES = [120, 176, 260];
/** 瀑布流的**列宽**（不裁切时列宽就是图片宽度，比网格格子大一号才不显得碎） */
export const COL_WIDTHS = [200, 300, 420];
/** 每个档位向后台要的缩略图像素（中间档 384 是「清晰度 / 体积」的平衡点） */
export const THUMB_PX = [256, 384, 512];

/* 判定与图标映射都在 types.ts（详情面板也要用同一份，别各写一遍） */

interface Props {
  items: Asset[];
  view: ViewMode;
  sizeIndex: number;
  /** 已选中的 id 集合（多选，T11） */
  selectedIds: Set<number>;
  /** 查询条件指纹：变了就回到顶部（换目录 / 换筛选 / 重新扫描） */
  resetKey: string;
  onPick: (a: Asset, mods: { ctrl: boolean; shift: boolean }) => void;
  /** 框选结果（T11） */
  onPickMany: (ids: number[], additive: boolean) => void;
  onOpen: (a: Asset) => void;
  /** 按住图拖动超过 6px → 开始一次**应用内拖拽**（浮层、落点、出窗交系统都在 App 里） */
  onBeginDrag: (a: Asset, x: number, y: number) => void;
  onNearBottom: () => void;
  thumbSrc: (a: Asset, px: number) => string;
  /** 预取可视区上下各一屏的缩略图（滚动停下后触发，不阻塞渲染） */
  onPrefetch?: (ids: number[], px: number) => void;
  /** 空状态里的「导入文件…」（0.4.2）：把外部文件复制进当前文件夹 */
  onImport?: () => void;
  onToast: (msg: string, err?: boolean) => void;
}

export function AssetView({
  items,
  view,
  sizeIndex,
  selectedIds,
  resetKey,
  onPick,
  onPickMany,
  onOpen,
  onBeginDrag,
  onNearBottom,
  thumbSrc,
  onPrefetch,
  onImport,
  onToast,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [vh, setVh] = useState(720);
  const [ww, setWw] = useState(900);
  const [failed, setFailed] = useState<Set<number>>(new Set());
  const [hintAsset, setHintAsset] = useState<Asset | null>(null);
  const askedRef = useRef(false);
  const hintTimer = useRef<number | null>(null);
  const hintRef = useRef<HTMLDivElement>(null);
  const hintPos = useRef({ x: 0, y: 0 });
  const hintRaf = useRef<number | null>(null);

  /* 拖出窗口的起手式（T01）：
     按下左键记坐标 → 移动超过 6px 就交给 Rust 起 OLE 拖拽。
     为什么不用 HTML5 拖拽：`draggable` 一开，WebView 会自己接管拖拽（拖出去只剩一张透明图），
     而且和我们自己的 DoDragDrop 抢手势。这里用 mouse 事件只做「判定」，拖拽本身在原生侧。 */
  const press = useRef<{ a: Asset; x: number; y: number } | null>(null);
  const fired = useRef(false);
  const firedTimer = useRef<number | null>(null);

  /* 框选（T11）：从空白处按下拖出一个矩形，松开时选中所有相交的瓦片 */
  const marquee = useRef<{ x0: number; y0: number; x1: number; y1: number; additive: boolean } | null>(null);
  const [marqueeBox, setMarqueeBox] = useState<{ l: number; t: number; w: number; h: number } | null>(null);

  /* 回调稳定性（2026-11-16 性能）：`Tile` 是 `memo` 的，props 里只要有一个新函数引用，
     整屏瓦片就会跟着重渲 —— 而滚动时最贵的恰恰就是「40 个瓦片 × 每个 60 次/秒」。
     所有交给瓦片的回调都过一遍 ref + `useCallback([])`，引用永远不变；
     真正的父级回调从 `cbRef` 里现取，所以也不会拿到旧的闭合变量。 */
  const cbRef = useRef({ onPick, onOpen });
  cbRef.current = { onPick, onOpen };

  const pickTile = useCallback((a: Asset, mods: { ctrl: boolean; shift: boolean }) => {
    cbRef.current.onPick(a, mods);
  }, []);
  const openTile = useCallback((a: Asset) => {
    /* 拖拽刚结束的 400ms 内浏览器可能补一个 dblclick，框选过程同理：
       手还在拖动时不当成「打开大图」，避免拖完素材突然弹灯箱。 */
    if (fired.current || marquee.current) return;
    cbRef.current.onOpen(a);
  }, []);
  const failTile = useCallback((id: number) => {
    setFailed((s) => {
      if (s.has(id)) return s; // 同一个 id 重复失败不重建 Set
      const n = new Set(s);
      n.add(id);
      return n;
    });
  }, []);
  const pressTile = useCallback((a: Asset, e: React.MouseEvent) => {
    if (e.button !== 0) return;
    press.current = { a, x: e.clientX, y: e.clientY };
  }, []);

  useEffect(() => {
    const move = (e: MouseEvent) => {
      // 框选优先：它和拖出用的是同一个左键，但起点不同（空白 vs 瓦片）
      if (marquee.current) {
        marquee.current.x1 = e.clientX;
        marquee.current.y1 = e.clientY;
        const m = marquee.current;
        const host = innerRef.current?.getBoundingClientRect();
        if (host) {
          setMarqueeBox({
            l: Math.min(m.x0, m.x1) - host.left,
            t: Math.min(m.y0, m.y1) - host.top,
            w: Math.abs(m.x1 - m.x0),
            h: Math.abs(m.y1 - m.y0),
          });
        }
        return;
      }
      const p = press.current;
      if (!p || e.buttons !== 1) return;
      if (Math.hypot(e.clientX - p.x, e.clientY - p.y) < 6) return;
      press.current = null;
      fired.current = true;
      if (firedTimer.current) window.clearTimeout(firedTimer.current);
      // 拖拽结束后浏览器还会补一个 click，用一个短窗口把它吃掉
      firedTimer.current = window.setTimeout(() => {
        fired.current = false;
      }, 400);
      hideHint(); // 无条件清一次：hintAsset 是 state，在这个不重订阅的 effect 里会读到旧值
      onBeginDrag(p.a, e.clientX, e.clientY);
    };
    const up = () => {
      press.current = null;
      const m = marquee.current;
      marquee.current = null;
      setMarqueeBox(null);
      if (!m) return;
      const host = innerRef.current?.getBoundingClientRect();
      if (!host) return;
      // 拖动距离太短＝误触（只是想点空白处），不当框选
      if (Math.abs(m.x1 - m.x0) < 6 && Math.abs(m.y1 - m.y0) < 6) return;
      const l = Math.min(m.x0, m.x1) - host.left;
      const t = Math.min(m.y0, m.y1) - host.top;
      const r = Math.max(m.x0, m.x1) - host.left;
      const b = Math.max(m.y0, m.y1) - host.top;
      const ids: number[] = [];
      for (const rc of layoutRef.current.rects) {
        if (rc.x < r && rc.x + rc.w > l && rc.y < b && rc.y + rc.h > t) ids.push(rc.a.id);
      }
      onPickMany(ids, m.additive);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    window.addEventListener("blur", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      window.removeEventListener("blur", up);
      if (firedTimer.current) window.clearTimeout(firedTimer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onBeginDrag, onPickMany]);

  /* 滚动节流（2026-11-16 性能）：以前**每个 scroll 事件**都 `setScrollTop`，
     一帧里滚轮/触控板可能推 3–5 个事件，每个事件都把 40+ 个瓦片整体重渲一遍。
     现在两点收敛：
       ① 一帧只算一次（rAF 合帧）；
       ② 位移不足 100px **不改 state** —— 可见窗口上下各有 400px 预渲染余量，
          100px 抖动完全落在余量里，用户看不到任何差别，但渲染次数能降一个量级。 */
  const committedTop = useRef(0);
  const scrollRaf = useRef(0);

  /* 自绘 hint：原生 title 要等 1 秒、还把「文件名 + 路径」挤成两行系统白框。
     这里 420ms 后出、跟着鼠标走。
     ⚠️ 位置**不进 React state**：以前每次 mousemove 都 setState，一格移动要重渲全部可见瓦片，
     几十个节点的 diff 跑不赢指针，肉眼就是「提示框黏在原地、慢半拍」。现在只改 DOM transform，
     rAF 合帧，移动路径上零重渲染。 */
  const placeHint = useCallback(() => {
    const el = hintRef.current;
    if (!el) return;
    const pad = 14;
    const { x, y } = hintPos.current;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    let left = x + pad;
    let top = y + pad;
    if (left + w > window.innerWidth - 8) left = Math.max(8, x - w - pad);
    if (top + h > window.innerHeight - 8) top = Math.max(8, y - h - pad);
    left = Math.max(8, left);
    top = Math.max(8, top);
    el.style.transform = `translate3d(${Math.round(left)}px, ${Math.round(top)}px, 0)`;
  }, []);

  const moveHint = useCallback(
    (x: number, y: number) => {
      hintPos.current = { x, y };
      if (hintRaf.current !== null) return;
      hintRaf.current = window.requestAnimationFrame(() => {
        hintRaf.current = null;
        placeHint();
      });
    },
    [placeHint]
  );

  const showHint = useCallback((a: Asset, x: number, y: number) => {
    hintPos.current = { x, y };
    if (hintTimer.current) window.clearTimeout(hintTimer.current);
    hintTimer.current = window.setTimeout(() => setHintAsset(a), 420);
  }, []);

  const hideHint = useCallback(() => {
    if (hintTimer.current) window.clearTimeout(hintTimer.current);
    if (hintRaf.current !== null) {
      window.cancelAnimationFrame(hintRaf.current);
      hintRaf.current = null;
    }
    setHintAsset(null);
  }, []);

  // 提示框一出现就贴到当前指针位置（不等下一次 mousemove）
  useEffect(() => {
    if (!hintAsset) return;
    const id = window.requestAnimationFrame(placeHint);
    return () => window.cancelAnimationFrame(id);
  }, [hintAsset]);

  useEffect(() => () => {
    if (hintTimer.current) window.clearTimeout(hintTimer.current);
    if (hintRaf.current !== null) window.cancelAnimationFrame(hintRaf.current);
  }, []);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setVh(el.clientHeight);
      setWw(el.clientWidth);
    });
    ro.observe(el);
    setVh(el.clientHeight);
    setWw(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  // 换目录 / 换筛选 / 重新扫描 → 回到顶部（T08 的锚点恢复**不**在这条路径上）
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
    setScrollTop(0);
    committedTop.current = 0;
    anchor.current = null;
    askedRef.current = false;
  }, [resetKey]);

  const inner = Math.max(120, ww - 32);
  const tile = TILE_SIZES[sizeIndex] ?? 176;
  const colW = COL_WIDTHS[sizeIndex] ?? 300;

  const layout = useMemo(() => {
    return view === "grid"
      ? gridLayout(items, inner, tile, 8, MAX_COLS_GRID[sizeIndex] ?? 8)
      : masonryLayout(items, inner, colW, 8, MAX_COLS_WATERFALL);
  }, [items, inner, tile, colW, view, sizeIndex]);

  const layoutRef = useRef(layout);
  layoutRef.current = layout;

  /* 滚动锚点（T08）：切档位 / 切视图 / 拖窗口宽度会让所有格子重排。
     如果不记锚点，用户看着第 40 张、换一档就跳回第 1 张 —— 规格 §6.5 明确要求保住位置。
     锚点用「最上面那张完全可见的图 + 它相对视口顶部的偏移」，重排后把它放回同一个偏移。 */
  const anchor = useRef<{ id: number; dy: number } | null>(null);
  const layoutSig = `${view}|${sizeIndex}|${Math.round(inner)}`;

  const captureAnchor = () => {
    const el = scrollRef.current;
    if (!el) return;
    const top = el.scrollTop;
    let best: Rect | null = null;
    for (const r of layoutRef.current.rects) {
      if (r.y + r.h < top) continue;
      if (!best || r.y < best.y) best = r;
    }
    if (best) anchor.current = { id: best.a.id, dy: best.y - top };
  };

  /* 锚点每 120ms 采一次就够了：它是给"重排后回到原位"用的，
     拉到 60Hz 每次扫几千个 rect 纯属浪费（这是滚动期间另一笔固定开销）。 */
  const anchorAt = useRef(0);
  const maybeAnchor = () => {
    const now = performance.now();
    if (now - anchorAt.current < 120) return;
    anchorAt.current = now;
    captureAnchor();
  };

  const sigRef = useRef(layoutSig);
  useLayoutEffect(() => {
    if (sigRef.current === layoutSig) return;
    sigRef.current = layoutSig;
    const a = anchor.current;
    const el = scrollRef.current;
    if (!a || !el) return;
    const r = layout.rects.find((x) => x.a.id === a.id);
    if (!r) return;
    const next = Math.max(0, r.y - a.dy);
    el.scrollTop = next;
    committedTop.current = next;
    setScrollTop(next);
  }, [layoutSig, layout]);

  const visible = useMemo(() => {
    const top = scrollTop - 400;
    const bottom = scrollTop + vh + 400;
    return layout.rects.filter((r) => r.y + r.h >= top && r.y <= bottom);
  }, [layout, scrollTop, vh]);

  /* 预取：滚动停下 220 ms 后，把可视区**上下各一屏**的缩略图排进后台。
     为什么不是"渲染时就请求"：渲染只该关心看得见的东西；生成要几十到几百毫秒，
     所以提前一屏做，用户滑过去时就是命中缓存（只剩读文件，个位数毫秒）。
     用 ref 转存回调，避免 onPrefetch 每次重建都把定时器重置一遍。 */
  const prefetchRef = useRef(onPrefetch);
  prefetchRef.current = onPrefetch;
  /* 上一次预取请求的签名。小幅滚动会反复落在同一批图上，签名没变就别再发 ——
     后端收到后要为每个 id 抢一次全局锁、逐个查盘，还会起线程（旧线程靠 epoch 自杀）。 */
  const prefetchSig = useRef("");
  useEffect(() => {
    if (!prefetchRef.current) return;
    const top = scrollTop - vh * 1.2;
    const bottom = scrollTop + vh * 2.2;
    const px = THUMB_PX[sizeIndex] ?? 384;
    const t = window.setTimeout(() => {
      const ids = layout.rects
        .filter(
          (r) =>
            r.y + r.h >= top && r.y <= bottom && PREVIEWABLE_KINDS.has(r.a.kind)
        )
        .map((r) => r.a.id);
      if (!ids.length) return;
      const sig = `${px}|${ids.length}|${ids[0]}|${ids[ids.length - 1]}`;
      if (prefetchSig.current === sig) return;
      prefetchSig.current = sig;
      prefetchRef.current?.(ids, px);
    }, 220);
    return () => window.clearTimeout(t);
  }, [layout, scrollTop, vh, sizeIndex]);

  /* 滚动节流的 ref 声明在上面（`committedTop` / `scrollRaf`），这里只放提交逻辑。 */
  const commitScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const t = el.scrollTop;
    if (Math.abs(t - committedTop.current) < 100) return;
    committedTop.current = t;
    setScrollTop(t);
  };

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    if (hintAsset) hideHint();
    maybeAnchor();
    const rest = el.scrollHeight - el.scrollTop - el.clientHeight;
    if (rest < 900 && !askedRef.current) {
      askedRef.current = true;
      onNearBottom();
      window.setTimeout(() => {
        askedRef.current = false;
      }, 260);
    }
    if (scrollRaf.current) return;
    scrollRaf.current = window.requestAnimationFrame(() => {
      scrollRaf.current = 0;
      commitScroll();
    });
  };

  /* 空白处按下 = 开始框选；按住 Ctrl/Shift 则是「追加选择」 */
  const onBlankDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest("[data-asset-id]")) return;
    const host = innerRef.current?.getBoundingClientRect();
    if (!host) return;
    marquee.current = {
      x0: e.clientX,
      y0: e.clientY,
      x1: e.clientX,
      y1: e.clientY,
      additive: e.ctrlKey || e.metaKey || e.shiftKey,
    };
    setMarqueeBox({ l: e.clientX - host.left, t: e.clientY - host.top, w: 0, h: 0 });
  };

  /* 框选的按下监听挂在 .viewport 上而不是 .view-inner：
     内层四周有 14/16px 的内距，从那条缝起手是很自然的动作，
     挂在内层会让「从边距拖」完全没反应（2026-11-16 实测踩到）。 */
  return (
    <div className="viewport scroll-hot" ref={scrollRef} onScroll={onScroll} onMouseDown={onBlankDown}>
      {items.length === 0 ? (
        <Empty onImport={onImport} />
      ) : (
        <div
          className={`view-inner ${view === "waterfall" ? "wf" : ""}`}
          ref={innerRef}
          style={{ height: layout.height }}
        >
          {visible.map((r) => (
            <Tile
              key={r.a.id}
              rect={r}
              selected={selectedIds.has(r.a.id)}
              failed={failed.has(r.a.id) || r.a.thumbState === THUMB_FAILED}
              src={thumbSrc(r.a, THUMB_PX[sizeIndex] ?? 384)}
              onFail={failTile}
              onPick={pickTile}
              onOpen={openTile}
              onPress={pressTile}
              onHintEnter={showHint}
              onHintMove={moveHint}
              onHintLeave={hideHint}
              /* 瀑布流里超高图可以被"就地滚动"看到全貌（网格是正方形格子，没这个必要） */
              scrollInside={view === "waterfall"}
            />
          ))}
          {marqueeBox && marqueeBox.w > 2 && (
            <div
              className="marquee"
              style={{ left: marqueeBox.l, top: marqueeBox.t, width: marqueeBox.w, height: marqueeBox.h }}
              aria-hidden
            />
          )}
        </div>
      )}

      {hintAsset && (
        <div className="tile-hint" ref={hintRef} aria-hidden="true">
          <div className="tile-hint-name">{hintAsset.name}</div>
          <div className="tile-hint-path mono">{hintAsset.path}</div>
          <div className="tile-hint-meta mono">
            {hintAsset.width && hintAsset.height
              ? `${hintAsset.width} × ${hintAsset.height}`
              : "—"}
            <span>·</span>
            {badge(hintAsset.ext)}
            {hintAsset.quality === 2 && (
              <>
                <span>·</span>系统预览
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function Empty({ onImport }: { onImport?: () => void }) {
  return (
    <div className="empty-state">
      <div className="empty-inner">
        <span className="empty-tile">
          <Icon name="image" size={26} stroke={1.5} />
        </span>
        <h2>这里还没有素材</h2>
        <p>把文件夹加进库，或者换一个目录。AssetManager 只建索引，不会移动、复制你的原始文件。</p>
        {/* 空目录最常见的下一步就是"我有图要放进来"，就地给一个按钮，
            省得用户去翻右键菜单或命令面板 */}
        {onImport && (
          <button className="btn btn-accent" style={{ marginTop: 14 }} onClick={onImport}>
            导入文件…
          </button>
        )}
      </div>
    </div>
  );
}

/**
 * 一张瓦片。**三态加载（T07）**：
 *   1. 骨架 —— 这张图一张都没加载过，先用 `.tile-skel` 占位；
 *   2. 上一张 —— 已经从别的档位加载过（切档 / 拖窗口宽度），旧图留在底下加 6px 模糊，
 *      绝不闪白；
 *   3. 高清 —— 新图 `onLoad` 之后再淡入，然后撤掉底下的旧图。
 * 三态之间用 CSS opacity 过渡，不做任何布局跳变（瓦片尺寸本来就是算好的）。
 */
/* memo 的作用点：滚动时 `AssetView` 只多挂载新进窗口的几张，
   已经在屏上的瓦片因为 props 全等（rect 来自 memo 过的 layout、回调都是稳定引用）直接跳过重渲。 */
/**
 * 用主色板拼一张"占位色块"（零成本：色板本来就在 DTO 里）。
 *
 * 为什么值得做：滚到一屏新图时，最刺眼的不是"慢 200ms"，而是**一片灰方块**——
 * 眼睛会把它读成"这里什么都没有"。用这张图自己的主色做一个渐变底，
 * 在缩略图到位之前就已经"看起来像那张图"了，等待感消失（而且不增加一个字节的存储）。
 * 没有色板（老数据 / 灰图）就返回 undefined，退回原来的骨架底色。
 */
function paletteBg(palette: string): string | undefined {
  const cs = palette.split(",").filter(Boolean).slice(0, 3);
  if (!cs.length) return undefined;
  if (cs.length === 1) return `#${cs[0]}`;
  const stops = cs.map((c, i) => `#${c} ${Math.round((i / (cs.length - 1)) * 100)}%`);
  return `linear-gradient(135deg, ${stops.join(", ")})`;
}

const Tile = memo(function Tile({
  rect,
  selected,
  failed,
  src,
  onFail,
  onPick,
  onOpen,
  onPress,
  onHintEnter,
  onHintMove,
  onHintLeave,
  scrollInside,
}: {
  rect: Rect;
  selected: boolean;
  failed: boolean;
  src: string;
  onFail: (id: number) => void;
  onPick: (a: Asset, mods: { ctrl: boolean; shift: boolean }) => void;
  onOpen: (a: Asset) => void;
  onPress: (a: Asset, e: React.MouseEvent) => void;
  onHintEnter: (a: Asset, x: number, y: number) => void;
  onHintMove: (x: number, y: number) => void;
  onHintLeave: () => void;
  /** 允许"悬停后用滚轮滚动这张图"（瀑布流专用） */
  scrollInside?: boolean;
}) {
  /* `loadedSrc` 只记录「已经画出来的那张」，和 src 不一致时说明在换图 */
  const [loadedSrc, setLoadedSrc] = useState<string | null>(null);
  const a = rect.a;
  const ready = loadedSrc === src;
  /* 这类文件没有缩略图可出（json / pdf / 音频 / 模型…），直接画"代替缩略图"的卡片 */
  const noPreview = !PREVIEWABLE_KINDS.has(a.kind);

  /* ---- 瀑布流里"超高图就地滚动"（owner 2026-11-19）-------------------------
     布局把单格高度封顶在 `waterfallTileH`（2.4×列宽 与 720px 取小的那个），
     图片 `object-fit: cover` 铺满宽度、纵向溢出，改 `object-position` 的纵向百分比
     就等于在溢出的那一段里上下移动 —— 和详情面板顶部预览是同一套手感。

     **只有 Ctrl（或 ⌘）+ 滚轮才滚这张图**：普通滚轮一律还给列表。
     理由（owner 2026-11-19 明确要求）：鼠标偶然压在一张长图上时列表滚不动，
     用户会以为"滚动卡了"；把"看全图"绑到一个显式组合键上，两边都不打架。
     为什么手动 addEventListener：React 的 onWheel 挂在 root 上是 passive，preventDefault 无效。 */
  const rootRef = useRef<HTMLDivElement | null>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const barRef = useRef<HTMLSpanElement | null>(null);
  const scrollPos = useRef(0);

  const iw = a.width ?? 0;
  const ih = a.height ?? 0;
  const imgAspect = iw > 0 && ih > 0 ? iw / ih : rect.w / Math.max(1, rect.h);
  /* cover 下图片的实际显示高度（宽铺满 → 高按比例） */
  const shownH = rect.w / Math.max(0.01, imgAspect);
  const overflow = Math.max(0, shownH - rect.h);
  const scrollable = !!scrollInside && !noPreview && overflow > 6;

  useEffect(() => {
    const el = rootRef.current;
    const img = imgRef.current;
    const bar = barRef.current;
    if (!el || !img || !scrollable) return;

    scrollPos.current = 0;
    img.style.objectPosition = "50% 0%";
    const track = rect.h;
    const barH = Math.max(20, (track * rect.h) / shownH);
    if (bar) {
      bar.style.height = `${barH}px`;
      bar.style.transform = "translateY(0px)";
    }

    const onWheel = (e: WheelEvent) => {
      /* 没按 Ctrl = 用户在滚列表，这里一个字节都不拦 */
      if (!e.ctrlKey && !e.metaKey) return;
      /* 按了 Ctrl 就整段吃掉（连到顶/到底也吃）：否则界面会在边界处跟着抖一下。
         触控板捏合会送来很大的 deltaY，钳到 ±120 免得一下跳到图尾。 */
      e.preventDefault();
      const dy = Math.max(-120, Math.min(120, e.deltaY));
      const cur = scrollPos.current;
      const next = Math.min(overflow, Math.max(0, cur + dy));
      if (next === cur) return;
      scrollPos.current = next;
      img.style.objectPosition = `50% ${(next / overflow) * 100}%`;
      if (bar) bar.style.transform = `translateY(${(next / overflow) * (track - barH)}px)`;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [scrollable, overflow, shownH, rect.h]);

  return (
    <div
      ref={rootRef}
      className={`tile ${selected ? "sel" : ""} ${loadedSrc ? "loaded" : ""}`}
      data-asset-id={a.id}
      style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h, transform: "translateZ(0)" }}
      onClick={(e) => onPick(a, { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey })}
      onDoubleClick={(e) => {
        /* 多选手势（Ctrl / Shift + 双击）不打开大图：否则选着选着就弹灯箱 */
        if (e.ctrlKey || e.metaKey || e.shiftKey) return;
        onOpen(a);
      }}
      onMouseEnter={(e) => onHintEnter(a, e.clientX, e.clientY)}
      onMouseMove={(e) => onHintMove(e.clientX, e.clientY)}
      onMouseLeave={onHintLeave}
      onMouseDown={(e) => onPress(a, e)}
      draggable={false}
      onDragStart={(e) => e.preventDefault()}
    >
      {noPreview ? (
        <div className="tile-kind">
          <Icon name={KIND_ICON[a.kind] ?? "file"} size={22} stroke={1.3} />
          <span className="tile-kind-name">{a.name}</span>
        </div>
      ) : (
        <>
          {/* 主色板占位：缩略图到位前先"看起来像那张图" */}
          <div
            className="tile-pal"
            style={{ background: paletteBg(a.palette) }}
            aria-hidden="true"
          />
          {!failed && loadedSrc && !ready && (
            <img className="tile-img ghost" src={loadedSrc} alt="" draggable={false} aria-hidden />
          )}
          {!failed && (
            <img
              ref={imgRef}
              className={`tile-img ${ready ? "in" : "out"}`}
              src={src}
              alt=""
              decoding="async"
              draggable={false}
              onLoad={() => setLoadedSrc(src)}
              onError={() => {
                setLoadedSrc(src);
                onFail(a.id);
              }}
            />
          )}
          {/* 能滚的长图：一条细指示条 + 悬停时一句「Ctrl+滚轮」——
              不标出来的话，用户猜不到"这张能滚"，更猜不到要按 Ctrl */}
          {scrollable && (
            <>
              <span className="tile-scrollbar" ref={barRef} aria-hidden="true" />
              <span className="tile-scroll-hint">Ctrl + 滚轮</span>
            </>
          )}
          {!loadedSrc && !failed && <div className="tile-skel" />}
          {failed && (
            <div className="tile-fail">
              <Icon name="image" size={18} stroke={1.4} />
              <span>暂无预览</span>
            </div>
          )}
        </>
      )}
      <span className="tile-anchor" />
      {a.rating > 0 && (
        <span className="tile-rate" title={`${a.rating} 星`}>
          <Icon name="star" size={9} stroke={2.2} />
          {a.rating}
        </span>
      )}
      <span className="tile-badge mono">{badge(a.ext)}</span>
      {/* 文件名：普通情况靠悬停浮出；没有预览的卡片常显（否则一块灰方块什么信息都没有） */}
      {!noPreview && <span className="tile-name">{a.name}</span>}
    </div>
  );
}, (p, n) =>
  /* 值语义比较：`layout` 每次重算都会给**同一张图生成新的 Rect 对象**，默认浅比较会把
     整屏瓦片全判成"变了"（翻一页 / 点一颗星就重渲 ~40 个瓦片）。这里只比值：
     `rect.a` 的引用（`patchAsset` 对没改动的素材保持原对象）+ 四个坐标 + 其余已稳定 props。
     回调由上层 `cbRef`/`useCallback` 固定引用，是常量，所以不参与比较。 */
  p.rect.a === n.rect.a &&
  p.rect.x === n.rect.x &&
  p.rect.y === n.rect.y &&
  p.rect.w === n.rect.w &&
  p.rect.h === n.rect.h &&
  p.selected === n.selected &&
  p.failed === n.failed &&
  p.src === n.src &&
  p.scrollInside === n.scrollInside
);
