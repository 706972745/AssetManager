/**
 * 窗口外壳与详情 —— 标题栏、顶栏（搜索 / 筛选 / 排序）、右侧详情面板。
 *
 * 文件内按 `---- 标题栏 ----`、`---- 顶栏 ----`、`---- 详情 ----` 三条横幅分节，
 * 新东西必须落进对应分节，不要在文件末尾追加。
 * 本文件不持全局状态（状态在 `App.tsx`），文件操作一律走 `source.xxx`，不直接 invoke。
 */
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, FolderOpen, GripVertical, Sparkles, Star, Tag as TagIcon } from "lucide-react";
import {
  badge,
  fmtNum,
  fmtSize,
  fmtTime,
  KIND_LABEL,
  PREVIEWABLE_KINDS,
  type Asset,
  type AutoTag,
  type ExifInfo,
  type SortKey,
  type Source,
} from "../types";
import { BrandMark, BlinkCaretInput, Icon, Menu, type MenuItem } from "./ui";
import { COLOR_LABEL, autoTagLabel, colorClass, isAutoTag } from "../lib/tags";
import type { ViewMode } from "./Views";
import type { DirNode, RefItem, TagCount } from "../types";

/* 窗口动作：桌面端走命令，浏览器预览下静默 */
function win(cmd: string) {
  return () => {
    const inv = (window as any).__TAURI_INTERNALS__?.invoke ?? (window as any).__TAURI__?.core?.invoke;
    if (inv) void inv(cmd);
    else window.close();
  };
}

/* ---------------------------------------------------------------- 标题栏 */

export function TitleBar({
  onRescan,
  onAddFolder,
  onSettings,
  onUndo,
  busy,
}: {
  onRescan: () => void;
  onAddFolder: () => void;
  onSettings: () => void;
  onUndo: () => void;
  busy: boolean;
}) {
  /* 标题栏只写应用名。以前这里还跟一段「— 快照：xxx / 标签：xxx」表示"我在看什么"
     （T44 的轻量版），owner 2026-11-18 明确不要：侧栏本来就高亮着当前项，
     标题栏再写一遍只是噪音，而且长名字会把标题挤走。 */
  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar-brand" data-tauri-drag-region>
        <span className="brand-mark">
          <BrandMark />
        </span>
        <span className="titlebar-title">AssetManager</span>
      </div>
      {/* 结构与 FontManager 逐层一致（TitleBar.tsx）：
         `.titlebar-controls`（gap 1px）里装三样 —— 动作组 `.titlebar-actions`、
         一条竖分隔线、三个窗口按钮。
         我上一版把动作组拎到 controls 外面当兄弟节点，父级 `justify-content: space-between`
         于是把三个子元素按「左 / 中 / 右」摊开，图标组被推到标题栏中间悬空，
         离右边的窗口按钮一大截 —— owner 截图指的就是这里。 */}
      <div className="titlebar-controls">
        <div className="titlebar-actions">
          <button className="tb-action" title="撤销上一步（Ctrl+Z）" onClick={onUndo} disabled={busy}>
            <Icon name="undo" size={14} stroke={1.5} />
          </button>
          <button className="tb-action" title="添加素材文件夹" onClick={onAddFolder} disabled={busy}>
            {/* 照 FontManager 的「打开文件夹 + 外挂加号」（owner 2026-11-18 指定），
                不是 lucide 的 FolderPlus —— 那个是闭合文件夹，认不出来。 */}
            <Icon name="folder-plus" size={14} />
          </button>
          <button className="tb-action" title="重新扫描" onClick={onRescan} disabled={busy}>
            <Icon name="scan" size={14} stroke={1.5} />
          </button>
          <button className="tb-action" title="设置" onClick={onSettings}>
            <Icon name="settings" size={14} stroke={1.5} />
          </button>
        </div>
        <span className="titlebar-sep" aria-hidden />
        <button className="tc-btn" title="最小化" onClick={win("am_minimize")}>
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <path d="M2.5 6h7" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
          </svg>
        </button>
        <button className="tc-btn" title="最大化" onClick={win("am_toggle_max")}>
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <rect x="2.5" y="2.5" width="7" height="7" fill="none" stroke="currentColor" strokeWidth="1.1" />
          </svg>
        </button>
        <button className="tc-close" title="关闭" onClick={win("am_close")}>
          <Icon name="close" size={13} />
        </button>
      </div>
    </header>
  );
}

/* ---------------------------------------------------------------- 顶栏 */

const SORT_LABEL: Record<SortKey, string> = {
  mtime_desc: "最近修改",
  name_asc: "名称",
  size_desc: "大小",
  rating_desc: "评分",
};

export const KINDS: { key: string; label: string; kinds: number[] | null }[] = [
  { key: "all", label: "全部类型", kinds: null },
  { key: "image", label: "图片", kinds: [0] },
  { key: "3d", label: "3D", kinds: [3] },
  { key: "video", label: "视频", kinds: [1] },
  { key: "doc", label: "文档", kinds: [4] },
];

const SIZE_LABEL = ["小", "中", "大"];
const SIZE_PX = [120, 176, 260];

export function TopBar({
  text,
  onText,
  kindKey,
  onKind,
  sort,
  onSort,
  sizeIndex,
  onSize,
  view,
  onView,
  color,
  onColor,
  colorCounts,
  tags,
  activeTag,
  onTag,
  source,
  dirNode,
  onDir,
  autoTags,
  rating,
  onRating,
}: {
  text: string;
  onText: (v: string) => void;
  kindKey: string;
  onKind: (k: string) => void;
  sort: SortKey;
  onSort: (s: SortKey) => void;
  sizeIndex: number;
  onSize: (i: number) => void;
  view: ViewMode;
  onView: (v: ViewMode) => void;
  /** 颜色筛选（T24）：选中的色桶键，null = 不筛 */
  color: string | null;
  onColor: (c: string | null) => void;
  /** 各色桶的素材数（只列有素材的，避免一屏空点） */
  colorCounts: [string, number][];
  /** 标签清单（含使用数）：筛菜单里那一段「标签」用它，照 FontManager 的筛选菜单结构 */
  tags: TagCount[];
  /** 当前正在筛的标签（null = 不筛） */
  activeTag: string | null;
  onTag: (name: string | null) => void;
  /** 文件夹段：选中的目录（null = 不按目录筛） */
  source: Source;
  dirNode: DirNode | null;
  onDir: (n: DirNode | null) => void;
  /** 形状 / 格式段用现成的自动标签（`orient:*` / `fmt:*`），不另造口径 */
  autoTags: TagCount[];
  /** 评分下限（1–5 星；null = 不筛） */
  rating: number | null;
  onRating: (n: number | null) => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number; kind: "sort" | "filter" | "color" } | null>(
    null
  );
  /** 打开筛选菜单时才拉目录：主界面初次渲染不该为了一个可能永远不打开的菜单扫两遍树 */
  const [folders, setFolders] = useState<DirNode[]>([]);
  useEffect(() => {
    if (!menu || menu.kind !== "filter") return;
    let live = true;
    void (async () => {
      const roots = await source.roots().catch(() => [] as DirNode[]);
      const children = await Promise.all(roots.map((r) => source.children(r.id).catch(() => [] as DirNode[])));
      const list = [...roots, ...children.flat()].slice(0, 30);
      if (live) setFolders(list);
    })();
    return () => {
      live = false;
    };
  }, [menu, source]);

  const ORIENT_LABEL: Record<string, string> = {
    landscape: "横向",
    portrait: "纵向",
    square: "方形",
  };
  const kind = KINDS.find((k) => k.key === kindKey) ?? KINDS[0];

  const openMenu = (e: React.MouseEvent<HTMLElement>, k: "sort" | "filter" | "color") => {
    const r = e.currentTarget.getBoundingClientRect();
    /* 传**右沿**：三个下拉都照 FontManager 贴按钮右侧展开（`.sort-menu { right: 0 }`），
       菜单比按钮宽，所以它是向左伸出去的。以前传左沿 + 左对齐，跟 FontManager 不是一个方向。 */
    setMenu({ x: r.right, y: r.bottom + 6, kind: k });
  };

  const autoOf = (ns: string) => autoTags.filter((t) => t.name.startsWith(`${ns}:`));
  const pickTag = (name: string) => onTag(activeTag === name ? null : name);

  /* 二级面板每行前面的图标必须**每一行都有**（照 FontManager 的 `.filter-sub-name`）：
     只有部分行带图标时，同一列里的文字会左右错开 —— FontManager 2026-11-14 踩过。
     自动标签 = 魔棒（品牌色），手工标签 / 文件夹 = 中性灰线性图标。 */
  const ICO_AUTO = <Sparkles size={11} strokeWidth={1.6} className="menu-ico-auto" />;
  const ICO_TAG = <TagIcon size={11} strokeWidth={1.5} className="menu-ico-manual" />;
  const ICO_DIR = <FolderOpen size={11} strokeWidth={1.5} className="menu-ico-manual" />;
  const ICO_STAR = <Star size={11} strokeWidth={1.5} className="menu-ico-manual" />;

  /* 筛选菜单（照 owner 给的 Eagle 那条：标签 / 文件夹 / 形状 / 评分 / 格式；
     **颜色不做** —— 顶栏那颗彩色按钮已经是颜色筛选，再放一遍就是两套入口）。
     Shape / 格式复用 `orient:*` / `fmt:*` 自动标签，评分走后端 `minRating`。 */
  const filterItems: MenuItem[] = [
    { label: "类型", heading: true },
    ...KINDS.map((k) => ({
      label: k.label,
      on: kindKey === k.key,
      check: kindKey === k.key,
      onPick: () => onKind(k.key),
    })),

    { sep: true, label: "" },
    /* 下面几段像 FontManager 那样收进**二级菜单**：
       主菜单只留入口行，内容在左边的浮层里选 —— 一屏就能看全所有维度，
       不必在一个长列表里滚半天（owner 2026-11-17 指定）。 */
    { label: "范围", heading: true },
    {
      label: "标签",
      meta: activeTag ? "1" : undefined,
      on: !!activeTag,
      sub: [
        { label: "全部标签", icon: ICO_TAG, on: !activeTag, check: !activeTag, onPick: () => onTag(null) },
        ...tags.map((t) => ({
          label: t.name,
          /* 标签行的图标口径和 FontManager 一致：手工标签 = 标签图标（不再是色点） */
          icon: isAutoTag(t.name) ? ICO_AUTO : ICO_TAG,
          meta: String(t.count),
          on: activeTag === t.name,
          check: activeTag === t.name,
          onPick: () => pickTag(t.name),
        })),
        /* 自动标签也要在这里出现（owner 2026-11-17：「筛选下、标签下自动标签没显示出来」）。
           FontManager 的筛选菜单是**手工 + 自动**混在同一列（按名字排序），
           因为「筛选菜单是用户唯一能勾标签的地方」；我们侧栏虽然单独分了「自动标签」一段，
           但顶栏这层菜单如果不给，用户在筛选里就找不到 fmt:/orient: 这些。
           排在手后面：手工是用户的，自动是软件推的，主次不能反。 */
        ...[...autoTags]
          .sort((a, b) => a.name.localeCompare(b.name))
          .map((t) => ({
            label: autoTagLabel(t.name),
            icon: ICO_AUTO,
            meta: String(t.count),
            on: activeTag === t.name,
            check: activeTag === t.name,
            onPick: () => pickTag(t.name),
          })),
      ],
    },
    {
      label: "文件夹",
      meta: dirNode ? "1" : undefined,
      on: !!dirNode,
      sub: [
        { label: "全部文件夹", icon: ICO_DIR, on: !dirNode, check: !dirNode, onPick: () => onDir(null) },
        ...folders.map((d) => ({
          label: d.name,
          icon: ICO_DIR,
          meta: d.assetCnt > 0 ? String(d.assetCnt) : undefined,
          on: dirNode?.id === d.id,
          check: dirNode?.id === d.id,
          onPick: () => onDir(d),
        })),
      ],
    },
    {
      label: "形状",
      meta: activeTag?.startsWith("orient:") ? "1" : undefined,
      on: !!activeTag?.startsWith("orient:"),
      sub: [
        { label: "全部形状", icon: ICO_AUTO, on: !activeTag?.startsWith("orient:"), check: !activeTag?.startsWith("orient:"), onPick: () => onTag(null) },
        ...autoOf("orient").map((t) => ({
          label: ORIENT_LABEL[t.name.slice("orient:".length)] ?? t.name,
          icon: ICO_AUTO,
          meta: String(t.count),
          on: activeTag === t.name,
          check: activeTag === t.name,
          onPick: () => pickTag(t.name),
        })),
      ],
    },
    {
      label: "评分",
      meta: rating !== null ? `${rating}★` : undefined,
      on: rating !== null,
      sub: [
        { label: "全部评分", icon: ICO_STAR, on: rating === null, check: rating === null, onPick: () => onRating(null) },
        ...[5, 4, 3, 2, 1].map((n) => ({
          label: `${n} 星及以上`,
          icon: ICO_STAR,
          on: rating === n,
          check: rating === n,
          onPick: () => onRating(rating === n ? null : n),
        })),
      ],
    },
    {
      label: "格式",
      meta: activeTag?.startsWith("fmt:") ? "1" : undefined,
      on: !!activeTag?.startsWith("fmt:"),
      sub: [
        { label: "全部格式", icon: ICO_AUTO, on: !activeTag?.startsWith("fmt:"), check: !activeTag?.startsWith("fmt:"), onPick: () => onTag(null) },
        ...autoOf("fmt").map((t) => ({
          label: t.name.slice("fmt:".length).toUpperCase(),
          icon: ICO_AUTO,
          meta: String(t.count),
          on: activeTag === t.name,
          check: activeTag === t.name,
          onPick: () => pickTag(t.name),
        })),
      ],
    },
  ];

  return (
    <div className="topbar">
      <div className="topbar-search">
        <Icon name="search" size={13} stroke={1.7} className="tf-icon" />
        <BlinkCaretInput
          value={text}
          onChange={onText}
          placeholder="搜索文件名 / 标签…"
          inputClassName="topbar-field"
          ariaLabel="搜索文件名或标签"
        />
      </div>

      <div className="topbar-size" title={`缩略图大小：${SIZE_LABEL[sizeIndex]}`}>
        <input
          type="range"
          min={0}
          max={2}
          step={1}
          value={sizeIndex}
          onChange={(e) => onSize(Number(e.target.value))}
          aria-label="缩略图大小"
        />
        <span className="size-value">{SIZE_PX[sizeIndex]}px</span>
      </div>

      <button
        className={`ctl ctl-icon ${color ? "on" : ""}`}
        onClick={(e) => openMenu(e, "color")}
        title={color ? `按颜色筛选：${COLOR_LABEL[color] ?? color}` : "按主色筛选"}
        aria-label="按主色筛选"
      >
        <span
          className="ctl-color-dot"
          style={color ? { background: `var(--c-${color}, #8b8b86)` } : undefined}
        />
      </button>

      <button
        className={`ctl ctl-icon ${kindKey !== "all" || dirNode || activeTag || rating !== null ? "on" : ""}`}
        onClick={(e) => openMenu(e, "filter")}
        title="筛选：类型 / 标签 / 文件夹 / 形状 / 评分 / 格式"
        aria-label={`按类型筛选：${kind.label}`}
        aria-pressed={kindKey !== "all"}
      >
        <Icon name="funnel" size={13} stroke={1.5} className="ctl-ico" />
      </button>

      <div className="topbar-right">
        <button
          className="ctl"
          onClick={(e) => openMenu(e, "sort")}
          title={`排序：${SORT_LABEL[sort]}`}
        >
          <Icon name="sort" size={13} stroke={1.5} className="ctl-ico" />
          {SORT_LABEL[sort]}
          <Icon name="chevron" size={10} stroke={2} className="rot90" />
        </button>

        <div className="view-toggle">
          <span
            className="view-pill"
            style={{ ["--i" as string]: view === "grid" ? 0 : 1 }}
            aria-hidden
          />
          <button
            className={`view-btn ${view === "grid" ? "on" : ""}`}
            title="常规网格"
            onClick={() => onView("grid")}
          >
            <Icon name="grid" size={13} stroke={1.5} />
          </button>
          <button
            className={`view-btn ${view === "waterfall" ? "on" : ""}`}
            title="瀑布流"
            onClick={() => onView("waterfall")}
          >
            <Icon name="waterfall" size={13} stroke={1.5} />
          </button>
        </div>
      </div>

      {menu && (
        <Menu
          x={menu.x}
          y={menu.y}
          variant="pop"
          align="right"
          onClose={() => setMenu(null)}
          items={
            menu.kind === "sort"
              ? (Object.keys(SORT_LABEL) as SortKey[]).map((k) => ({
                  label: SORT_LABEL[k],
                  on: sort === k,
                  check: sort === k,
                  onPick: () => onSort(k),
                }))
              : menu.kind === "color"
              ? [
                  {
                    label: "全部颜色",
                    on: color === null,
                    check: color === null,
                    onPick: () => onColor(null),
                    /* 全部颜色没有"自己的颜色" → 用一个七彩环点，和下面单色点区分开 */
                    dot: "conic-gradient(#d94a45,#d9a13a,#d9cf4a,#4f9d5d,#4a7fd9,#8c5ad9,#d94a45)",
                  },
                  ...(colorCounts.length
                    ? colorCounts.map(([k, n]) => ({
                        label: COLOR_LABEL[k] ?? k,
                        meta: String(n),
                        on: color === k,
                        check: color === k,
                        onPick: () => onColor(k),
                        /* 行首圆点 = 这个颜色本身（与顶栏那颗、缩略图上的色点同一套变量） */
                        dot: `var(--c-${k}, #8b8b86)`,
                      }))
                    : [{ label: "还没有主色数据，滚一遍缩略图就会算出来", on: false, onPick: () => {} }]),
                ]
              : filterItems
          }
        />
      )}
    </div>
  );
}

/* ---------------------------------------------------------------- 详情 */

/** 面板默认宽度 / 可拖范围（FontManager 是 348，双击复位到 348） */
const DETAIL_W = 348;
const DETAIL_MIN = 300;
const DETAIL_MAX = 560;
const DETAIL_W_KEY = "am.detail.w.v1";

/** 复制文本：优先 Clipboard API，失败退 textarea + execCommand（WebView2 在自定义协议下偶尔拒绝） */
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    let ok = false;
    try {
      ok = document.execCommand("copy");
    } catch {
      ok = false;
    }
    document.body.removeChild(ta);
    return ok;
  }
}

/**
 * 详情预览：**固定 1:1 的方框**，图片宽度正好铺满方框（`2026-11-17` 改：纵向滚，永不横向溢出）。
 *
 * 为什么不 `object-fit: contain`：那样只会得到一张缩得很小的图 + 大片留白，
 * 想看细节只能开大图。做法：固定 1:1 方框 + 在框内滚动看其余部分，但**不裁切**。
 *
 * 为什么**只允许纵向滚**：横向多出来那段既没有滚动条也拖不动（面板宽度是固定的），
 * 图就被右边切掉一块，用户看到的是"图不完整"而不是"可以滚"。
 * 现在宽度恒等于方框宽 → 左右永远完整，高的部分往下滚。
 */
/** 一行摘要：机身 · f/1.8 · 1/250s · ISO 200 · 50mm（没展开折叠时显示） */
function exifSummary(e: ExifInfo): string {
  const v: string[] = [];
  if (e.model) v.push(e.model);
  else if (e.make) v.push(e.make);
  if (e.fNumber != null) v.push(`f/${e.fNumber}`);
  if (e.exposure) v.push(`${e.exposure}s`);
  if (e.iso != null) v.push(`ISO ${e.iso}`);
  if (e.focal != null) v.push(`${e.focal}mm`);
  return v.join(" · ");
}

/** EXIF 的时间是 `2026:11:17 10:30:00`，只把那两个冒号换成短横线 */
function fmtExifTime(s: string): string {
  return s.replace(/^(\d{4}):(\d{2}):(\d{2})/, "$1-$2-$3");
}

function SquarePreview({
  src,
  aspect,
  onPressStart,
}: {
  src: string;
  /** 原图宽高比（宽/高）。0 = 未知，按 1:1 处理 */
  aspect: number;
  onPressStart: (e: React.MouseEvent) => void;
}) {
  const box = useRef<HTMLDivElement>(null);

  /* 尺寸全交给 CSS（`width:100%; height:auto` + 方框 `aspect-ratio:1/1` + `overflow-y:auto`）。
     之前用 JS 算像素高度，超长图（例如 1080×6555）会把 flex 行撑坏、预览整块消失 ——
     宽度交给 CSS 反而是最稳的：**横向永远不会溢出**，纵向由方框自己滚。 */
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    el.scrollTop = 0;
  }, [src, aspect]);

  return (
    <div
      ref={box}
      className="detail-art"
      title="按住拖动 = 把原文件拖到 PS / 资源管理器；竖着的部分滚轮往下看"
      onMouseDown={onPressStart}
    >
      <img src={src} alt="" draggable={false} />
    </div>
  );
}

/* ---------------- 评分：星标 + 浮层（照 FontManager 的交互） ----------------
 *
 * 详情里不再摆一排星星。顶上只有**一颗星 + 角标**：角标显示已打几星（0 星不显示角标）。
 * 鼠标停一下（110ms）或点一下，浮层在星标下方展开：5 颗星 + 「N 星」+「清除」。
 * 浮层用 portal 挂到 body —— 详情面板有 overflow:hidden，就地绝对定位会被裁掉半截。
 */

const RATING_POP_W = 176;
const RATING_OPEN_DELAY = 110;
const RATING_CLOSE_DELAY = 220;

function RatingStars({
  rating,
  onChange,
}: {
  rating: number;
  /** 传 0 = 清除评分 */
  onChange: (n: number) => void;
}) {
  const [open, setOpen] = useState(false);
  const [hover, setHover] = useState(0);
  const [pos, setPos] = useState({ left: -999, top: -999 });
  const btn = useRef<HTMLButtonElement>(null);
  const pop = useRef<HTMLDivElement>(null);
  const openTimer = useRef<number | undefined>(undefined);
  const closeTimer = useRef<number | undefined>(undefined);

  useEffect(
    () => () => {
      window.clearTimeout(openTimer.current);
      window.clearTimeout(closeTimer.current);
    },
    []
  );

  const place = () => {
    const r = btn.current?.getBoundingClientRect();
    if (!r) return;
    /* 贴星标右下沿展开；面板靠左时往右收，别让最后两颗星跑到窗口外 */
    setPos({
      left: Math.max(8, Math.min(r.left - 4, window.innerWidth - RATING_POP_W - 8)),
      top: r.bottom + 7,
    });
  };

  const show = (delay: number) => {
    window.clearTimeout(closeTimer.current);
    window.clearTimeout(openTimer.current);
    const reveal = () => {
      place();
      setOpen(true);
    };
    if (delay === 0) reveal();
    else openTimer.current = window.setTimeout(reveal, delay);
  };

  const hide = () => {
    window.clearTimeout(openTimer.current);
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => {
      setOpen(false);
      setHover(0);
    }, RATING_CLOSE_DELAY);
  };

  const closeNow = () => {
    window.clearTimeout(openTimer.current);
    window.clearTimeout(closeTimer.current);
    setOpen(false);
    setHover(0);
  };

  const rate = (n: number) => {
    onChange(n);
    closeNow();
  };

  /* 浮层是 portal 到 body 的，鼠标一旦移出去就收不起来（`onMouseLeave` 只挂在
     星标和浮层自己身上）。补两条出口：点浮层外面、按 Esc —— 用户反馈
     "打开了没办法取消"。 */
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node | null;
      if (pop.current?.contains(t) || btn.current?.contains(t)) return;
      closeNow();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        closeNow();
      }
    };
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  return (
    <span className="rating-wrap">
      <button
        ref={btn}
        className={`rating-btn ${rating > 0 ? "on" : ""}`}
        title={rating > 0 ? `已评 ${rating} 星 · 点击修改` : "还没评分 · 点击打分"}
        aria-label="评分"
        aria-expanded={open}
        onMouseEnter={() => show(RATING_OPEN_DELAY)}
        onMouseLeave={hide}
        onFocus={() => show(0)}
        onClick={() => (open ? closeNow() : show(0))}
      >
        <Icon name="star" size={15} stroke={1.6} />
        {rating > 0 && <span className="rating-badge mono">{rating}</span>}
      </button>

      {createPortal(
        open ? (
          <div
            ref={pop}
            className="rating-pop"
            style={{ left: pos.left, top: pos.top }}
            role="group"
            aria-label="打分"
            onMouseEnter={() => show(0)}
            onMouseLeave={hide}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="rating-pop-stars" onMouseLeave={() => setHover(0)}>
              {[1, 2, 3, 4, 5].map((n) => (
                <button
                  key={n}
                  className={`rating-star ${n <= (hover || rating) ? "on" : ""}`}
                  title={`${n} 星`}
                  aria-label={`${n} 星`}
                  onMouseEnter={() => setHover(n)}
                  onFocus={() => setHover(n)}
                  onClick={() => rate(n)}
                >
                  <Icon name="star" size={16} stroke={1.5} />
                </button>
              ))}
            </div>
            <div className="rating-pop-foot">
              <span className="rating-pop-value">
                {hover > 0 ? `${hover} 星` : rating > 0 ? `${rating} 星` : "未评分"}
              </span>
              {rating > 0 && (
                <button className="rating-pop-clear" onClick={() => rate(0)}>
                  <Icon name="close" size={9} stroke={2} />
                  清除
                </button>
              )}
            </div>
          </div>
        ) : null,
        document.body
      )}
    </span>
  );
}

/** 详情顶栏的就地改名（owner 2026-11-17：全软件只此一处改名，不弹窗）。
 *  回车 / 失焦 = 提交，Esc = 放弃。进入时只选中主名，扩展名留着不选 ——
 *  改名十次有九次是换前半截。 */
function TitleEdit({
  name,
  onCommit,
  onCancel,
}: {
  name: string;
  onCommit: (v: string) => void;
  onCancel: () => void;
}) {
  const box = useRef<HTMLInputElement>(null);
  /* Enter 提交后 input 会被卸掉，blur 还会再追一次 —— 用一次性闸门挡掉重复提交 */
  const done = useRef(false);
  /* 不能用 autoFocus：双击的第二次 mouseup 落在**刚挂上的 input** 上，
     浏览器会把焦点丢回 body，input 立刻 blur → 当场提交 + 退出编辑（实测就是
     "双击没反应"）。改成事件跑完之后再聚焦，鼠标那一下早就结束了。 */
  useEffect(() => {
    const t = window.setTimeout(() => {
      const el = box.current;
      if (!el) return;
      el.focus();
      const dot = name.lastIndexOf(".");
      el.setSelectionRange(0, dot > 0 ? dot : name.length);
      /* 名字比框长时，input 会跟着光标滚到尾部（用户只能看到后半截）。
         全选之后把视口拉回开头，先看见的是文件名开头。 */
      el.scrollLeft = 0;
    }, 0);
    return () => window.clearTimeout(t);
  }, [name]);

  const finish = (v: string) => {
    if (done.current) return;
    done.current = true;
    onCommit(v);
  };
  const bail = () => {
    if (done.current) return;
    done.current = true;
    onCancel();
  };

  return (
    <input
      ref={box}
      className="detail-title detail-title-edit"
      defaultValue={name}
      spellCheck={false}
      aria-label="重命名文件"
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          finish(e.currentTarget.value);
        } else if (e.key === "Escape") {
          e.preventDefault();
          bail();
        }
      }}
      onBlur={(e) => finish(e.currentTarget.value)}
    />
  );
}

export function Detail({
  asset,
  source,
  thumbSrc,
  thumbPx,
  onReveal,
  onRename,
  onPeek,
  onOpenDefault,
  onCopyFile,
  onDelete,
  onDragOut,
  onToast,
  onClose,
  tags,
  colors,
  autoTags,
  allTags,
  onAddTag,
  onRemoveTag,
  onPickTag,
  onDenyAutoTag,
  onEditTag,
  onSetRating,
  onSetNote,
  onSetSource,
  renaming,
  onStartRename,
  onCommitRename,
  onCancelRename,
}: {
  asset: Asset | null;
  source: Source;
  thumbSrc: (a: Asset, px: number) => string;
  /** 当前界面档位对应的缩略图像素（详情里如实写，不能一律写 512） */
  thumbPx: number;
  onDragOut: (a: Asset) => void;
  onReveal: (a: Asset) => void;
  /** 「重命名」入口：**不再弹窗**，改成让顶栏文件名进入就地编辑 */
  onRename: (a: Asset) => void;
  /** 「看图」= **贴图窗口**（Peek，窗口就是那张图），不是灯箱 */
  onPeek: (a: Asset) => void;
  /** 顶栏文件名是否处于改名状态（由 App 统一管：右键菜单 / 操作区的重命名都走这里） */
  renaming: boolean;
  onStartRename: () => void;
  onCommitRename: (name: string) => void;
  onCancelRename: () => void;
  onOpenDefault: (a: Asset) => void;
  onCopyFile: (a: Asset) => void;
  onDelete: (a: Asset) => void;
  onToast: (msg: string, err?: boolean) => void;
  /** 关掉详情（顶栏那个 ×） */
  onClose: () => void;
  /** 当前素材的手工标签 */
  tags: string[];
  /** 标签名 → 颜色键（详情 chip 上的小色点） */
  colors: Record<string, string>;
  /** 已按「手工优先 + 否决名单」过滤过的自动标签（带理由） */
  autoTags: AutoTag[];
  /** 已有手工标签（做候选） */
  allTags: string[];
  onAddTag: (tag: string) => void;
  onRemoveTag: (tag: string) => void;
  /** 点标签 = 按它筛列表 */
  onPickTag: (tag: string) => void;
  /** 自动标签「不再建议」（否决名单） */
  onDenyAutoTag: (tag: string) => void;
  /** 右键手动标签 → 编辑卡（改名 / 颜色 / 分组） */
  onEditTag: (tag: string) => void;
  /** 评分（0–5，0 = 清除） */
  onSetRating: (ids: number[], rating: number) => void;
  onSetNote: (id: number, note: string) => void;
  onSetSource: (id: number, url: string) => void;
}) {
  const [width, setWidth] = useState(() => {
    /* 宽度记在本地（照 FontManager：它把 panelWidth 存进 prefs）。
       默认 348、范围 300–560 与 FontManager 完全相同。 */
    try {
      const v = Number(localStorage.getItem(DETAIL_W_KEY));
      if (Number.isFinite(v) && v >= DETAIL_MIN && v <= DETAIL_MAX) return v;
    } catch {
      /* 读不到就用默认宽 */
    }
    return DETAIL_W;
  });
  const drag = useRef<{ x: number; w: number } | null>(null);
  /** 引用（显示层 / 谁在用 / 它引用了谁）默认收起 —— 排障时才展开 */
  const [refsOpen, setRefsOpen] = useState(false);
  /* 引用关系（T26）：这个文件引用了谁 / 谁在用它。只在选中变化时查一次。 */
  const [refsOut, setRefsOut] = useState<RefItem[]>([]);
  const [refsIn, setRefsIn] = useState<RefItem[]>([]);
  useEffect(() => {
    if (!asset || !source.desktop) {
      setRefsOut([]);
      setRefsIn([]);
      return;
    }
    let dead = false;
    void source
      .refsOf(asset.id)
      .then((r) => !dead && setRefsOut(r))
      .catch(() => {});
    void source
      .refsUsing(asset.id)
      .then((r) => !dead && setRefsIn(r))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [asset?.id, source]);

  /* 拍摄信息（T41）：只有 JPEG 之类真带 EXIF 的才有。
     换素材就重查一次；读不到就把这一段整个藏起来（不留空壳）。 */
  const [exif, setExif] = useState<ExifInfo | null>(null);
  const [exifOpen, setExifOpen] = useState(false);
  useEffect(() => {
    setExif(null);
    setExifOpen(false);
    if (!asset || !source.desktop || asset.kind !== 0) return;
    let dead = false;
    void source
      .exif(asset.path)
      .then((e) => !dead && setExif(e))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [asset?.id, asset?.path, asset?.kind, source]);

  /* 换素材 / 关掉详情时把折叠收回去：上一条的"展开"不该跟着带过来 */
  useEffect(() => {
    setRefsOpen(false);
  }, [asset?.id]);

  /* 顶栏文件名的单击复制延时器（见下面 onClick：双击时要把这一下取消掉） */
  const titleClick = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(titleClick.current), []);

  /* 详情里的预览图也能直接拖出去（与网格里的瓦片同一套手势）：
     按下记坐标 → 移动超过 6px 交给 Rust 起 OLE 拖拽，出去的是原文件。 */
  const press = useRef<{ x: number; y: number } | null>(null);
  const assetRef = useRef<Asset | null>(asset);
  assetRef.current = asset;
  useEffect(() => {
    // 与网格瓦片同一套：mouse 事件（WebView2 在原生拖拽介入时不送 pointermove，见 WORKLOG #21）
    const move = (e: MouseEvent) => {
      const p = press.current;
      const a = assetRef.current;
      if (!p || !a || e.buttons !== 1) return;
      if (Math.hypot(e.clientX - p.x, e.clientY - p.y) < 6) return;
      press.current = null;
      onDragOut(a);
    };
    const up = () => {
      press.current = null;
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    window.addEventListener("blur", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      window.removeEventListener("blur", up);
    };
  }, [onDragOut]);

  /* 左侧边缘拖动改宽度：抓在 10px 的握把上（hover 才显形），双击复位到 348 */
  const onDown = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, w: width };
  };
  const onMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    const next = drag.current.w - (e.clientX - drag.current.x);
    setWidth(Math.min(DETAIL_MAX, Math.max(DETAIL_MIN, next)));
  };
  const onUp = (e: React.PointerEvent<HTMLDivElement>) => {
    drag.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
    try {
      localStorage.setItem(DETAIL_W_KEY, String(width));
    } catch {
      /* 存不进就本次会话有效 */
    }
  };

  /** 主色板：后端给的是 `rrggbb,rrggbb,…` */
  const palette = asset && asset.palette ? asset.palette.split(",").filter(Boolean) : [];

  const copy = async (text: string, what: string) => {
    const ok = await copyText(text);
    onToast(ok ? `已复制${what}` : `复制失败，可以手动选中`, !ok);
  };

  return (
    <aside className="detail" style={{ width }} aria-label="详情">
      <div
        className="detail-resize"
        role="separator"
        aria-orientation="vertical"
        title="拖动调整宽度，双击复位"
        onPointerDown={onDown}
        onPointerMove={onMove}
        onPointerUp={onUp}
        onPointerCancel={onUp}
        onDoubleClick={() => setWidth(DETAIL_W)}
      >
        <GripVertical size={12} strokeWidth={1.5} />
      </div>

      <div className="detail-inner scroll-hot">
        {asset ? (
          <>
            {/* 顶栏：格式徽标 + 文件名 + 评分 + 关闭。**吸附在顶部**，往下滚也不走 */}
            <header className="detail-head">
              <span className="detail-badge">{badge(asset.ext)}</span>
              {renaming ? (
                <TitleEdit
                  name={asset.name}
                  onCommit={onCommitRename}
                  onCancel={onCancelRename}
                />
              ) : (
                <button
                  className="detail-title"
                  title="单击复制文件名 · 双击改名"
                  onClick={(e) => {
                    window.clearTimeout(titleClick.current);
                    /* 用 click 的 detail 判双击，而不是 onDoubleClick：
                       WebView2 在部分输入路径下不派发 dblclick（自测注入点击就是如此），
                       detail 则是「这一串里的第几次点击」，稳。
                       单击复制延后 220ms —— 双击时第一次 click 先到，
                       不延后就会「复制两次 + 弹两次 toast + 又进改名」。 */
                    if (e.detail >= 2) {
                      onStartRename();
                      return;
                    }
                    titleClick.current = window.setTimeout(
                      () => void copy(asset.name, "文件名"),
                      220
                    );
                  }}
                  /* detail 判双击之外再挂一条 dblclick：两条任意一条先到都进改名，
                     onStartRename 是幂等的（只设同一个 id）。 */
                  onDoubleClick={() => {
                    window.clearTimeout(titleClick.current);
                    onStartRename();
                  }}
                >
                  {asset.name}
                </button>
              )}
              <RatingStars
                rating={asset.rating}
                onChange={(n) => onSetRating([asset.id], n)}
              />
              <button className="detail-close" aria-label="关闭详情" title="关闭详情（Esc）" onClick={onClose}>
                <Icon name="close" size={15} stroke={1.5} />
              </button>
            </header>

            <div className="detail-body">
              {/* 预览 + 右侧竖排色板：色板贴在预览右边而不是另起一行，
                  扫一眼就知道这张图是什么调性，也不占纵向空间。 */}
              <div className="detail-hero">
                <SquarePreview
                  src={thumbSrc(asset, 512)}
                  aspect={asset.width && asset.height ? asset.width / asset.height : 0}
                  onPressStart={(e) => {
                    if (e.button === 0) press.current = { x: e.clientX, y: e.clientY };
                  }}
                />
                {palette.length > 0 && (
                  <div className="detail-palette" aria-label="主色板">
                    {palette.slice(0, 6).map((hex, i) => (
                      <button
                        key={`${hex}-${i}`}
                        style={{ background: `#${hex}` }}
                        title={`#${hex.toUpperCase()} · 点击复制`}
                        onClick={() => void copy(`#${hex.toUpperCase()}`, "色值")}
                      />
                    ))}
                  </div>
                )}
              </div>

              <div className="detail-artmeta mono">
                <span>{asset.width && asset.height ? `${asset.width} × ${asset.height}` : "—"}</span>
                <span>{fmtSize(asset.size)}</span>
                <span>{KIND_LABEL[asset.kind] ?? "其他"}</span>
              </div>

              <section>
                <div className="detail-heading">基本信息</div>
                <dl className="detail-meta">
                  <dt>格式</dt>
                  <dd>{asset.ext.toUpperCase()}</dd>
                  <dt>尺寸</dt>
                  <dd className="mono">
                    {asset.width && asset.height ? `${asset.width} × ${asset.height}` : "—"}
                  </dd>
                  <dt>大小</dt>
                  <dd className="mono">{fmtSize(asset.size)}</dd>
                  <dt>修改时间</dt>
                  <dd className="mono">{fmtTime(asset.mtime)}</dd>
                  <dt>添加日期</dt>
                  <dd
                    className="mono"
                    /* 0.1.16 及更早的库里没有这个值 —— 如实写「未知」，
                       不拿 mtime 顶上（那是文件的修改时间，不是入库时间） */
                    title={
                      asset.addedAt
                        ? "第一次被这个软件扫到的时间"
                        : "这张图入库时还没有记录添加时间（0.1.17 之前的版本）；重新扫描也不会补"
                    }
                  >
                    {asset.addedAt ? fmtTime(asset.addedAt) : "未知"}
                  </dd>
                  <dt>位置</dt>
                  <dd>
                    {/* 悬停 1 秒弹完整路径（图上只放得下一行，长路径必被截） */}
                    <button
                      className="path-link mono"
                      onClick={() => onReveal(asset)}
                      data-hint-text={asset.path}
                    >
                      <Icon name="reveal" size={11} stroke={1.5} />
                      <span>{asset.path}</span>
                    </button>
                  </dd>
                </dl>
              </section>

              {/* 拍摄信息（T41）：默认收起。相机/镜头/光圈/快门/ISO/焦距/时间。
                  没有 EXIF 的文件这一段不渲染 —— 空壳比没有更让人困惑。 */}
              {exif && (
                <section className="detail-fold-sec">
                  <button
                    className="detail-fold"
                    aria-expanded={exifOpen}
                    onClick={() => setExifOpen((v) => !v)}
                  >
                    <span className="detail-heading">拍摄信息</span>
                    {!exifOpen && exifSummary(exif) && (
                      <span className="detail-fold-hint mono">{exifSummary(exif)}</span>
                    )}
                    <Icon name="chevron" size={12} stroke={2} className={exifOpen ? "rot90" : ""} />
                  </button>
                  {exifOpen && (
                    <div className="detail-fold-body">
                      <dl className="detail-meta">
                        {exif.model && (
                          <>
                            <dt>机身</dt>
                            <dd>
                              {exif.make ? `${exif.make} ` : ""}
                              {exif.model}
                            </dd>
                          </>
                        )}
                        {exif.lens && (
                          <>
                            <dt>镜头</dt>
                            <dd className="wrap">{exif.lens}</dd>
                          </>
                        )}
                        {exif.focal != null && (
                          <>
                            <dt>焦距</dt>
                            <dd className="mono">{exif.focal}</dd>
                          </>
                        )}
                        {exif.fNumber != null && (
                          <>
                            <dt>光圈</dt>
                            <dd className="mono">f/{exif.fNumber}</dd>
                          </>
                        )}
                        {exif.exposure && (
                          <>
                            <dt>快门</dt>
                            <dd className="mono">{exif.exposure}s</dd>
                          </>
                        )}
                        {exif.iso != null && (
                          <>
                            <dt>ISO</dt>
                            <dd className="mono">{exif.iso}</dd>
                          </>
                        )}
                        {exif.taken && (
                          <>
                            <dt>拍摄时间</dt>
                            <dd className="mono">{fmtExifTime(exif.taken)}</dd>
                          </>
                        )}
                        {exif.software && (
                          <>
                            <dt>软件</dt>
                            <dd className="wrap">{exif.software}</dd>
                          </>
                        )}
                      </dl>
                    </div>
                  )}
                </section>
              )}

              {/* 引用（默认收起）：显示层 + 谁在用它 + 它引用了谁。
                  这些是"排障 / 想知道缩略图哪来的"才看的东西，不该占满屏。 */}
              <section className="detail-fold-sec">
                <button
                  className="detail-fold"
                  aria-expanded={refsOpen}
                  onClick={() => setRefsOpen((v) => !v)}
                >
                  <span className="detail-heading">引用</span>
                  <Icon name="chevron" size={12} stroke={2} className={refsOpen ? "rot90" : ""} />
                </button>

                {refsOpen && (
                  <div className="detail-fold-body">
                    {!PREVIEWABLE_KINDS.has(asset.kind) ? (
                      <dl className="detail-meta">
                        <dt>缩略图</dt>
                        <dd className="wrap">没有（这类文件没有预览图）</dd>
                        <dt>怎么看</dt>
                        <dd className="wrap">双击、或用下面的「用默认程序打开」</dd>
                      </dl>
                    ) : (
                      <dl className="detail-meta">
                        <dt>缩略图</dt>
                        <dd className="mono">
                          WebP · {thumbPx}px 档
                        </dd>
                        <dt>来源</dt>
                        <dd className="wrap">{thumbSourceLabel(asset)}</dd>
                        {asset.quality === 2 && (
                          <>
                            <dt>清晰度</dt>
                            <dd className="wrap">
                              受限于来源（
                              {EMBEDDED_EXT.has(asset.ext) ? "文件里的内嵌预览" : "系统的缩略图提供者"}
                              ），可能不如原图锐利
                            </dd>
                          </>
                        )}
                        <dt>用途</dt>
                        <dd className="wrap">只为显示快；上面所有操作都不碰它</dd>
                      </dl>
                    )}

                    {refsIn.length > 0 && (
                      <>
                        <div className="detail-sub">被这些工程文件用着（{refsIn.length}）</div>
                        <ul className="ref-list">
                          {refsIn.slice(0, 6).map((r) => (
                            <li key={`in-${r.srcId}`} title={r.srcPath}>
                              {r.srcName}
                            </li>
                          ))}
                        </ul>
                      </>
                    )}
                    {refsOut.length > 0 && (
                      <>
                        <div className="detail-sub">
                          这个工程引用的素材（{refsOut.length}，丢失{" "}
                          {refsOut.filter((r) => r.missing).length}）
                        </div>
                        <ul className="ref-list">
                          {refsOut.slice(0, 8).map((r) => (
                            <li
                              key={`out-${r.dstPath}`}
                              className={r.missing ? "miss" : ""}
                              title={r.dstPath}
                            >
                              {r.dstName || r.dstPath}
                              {r.missing && <span className="miss-tag">找不到</span>}
                            </li>
                          ))}
                        </ul>
                      </>
                    )}
                    {refsIn.length === 0 && refsOut.length === 0 && PREVIEWABLE_KINDS.has(asset.kind) && (
                      <p className="detail-note">没有工程文件引用它，它也没有引用别的素材。</p>
                    )}
                  </div>
                )}
              </section>

              <section>
                <div className="detail-heading">标签</div>
                <TagSection
                  manual={tags}
                  colors={colors}
                  auto={autoTags}
                  all={allTags}
                  onAdd={onAddTag}
                  onRemove={onRemoveTag}
                  onPick={onPickTag}
                  onDeny={onDenyAutoTag}
                  onEdit={onEditTag}
                />
              </section>

              <section>
                <div className="detail-heading">备注</div>
                {/* 备注 / 来源 URL 都是**人写的东西**：扫描与重建索引永远不碰它们 */}
                <textarea
                  key={`note-${asset.id}`}
                  className="mine-note"
                  rows={3}
                  placeholder="备注——例如：给客户 A 的那一版"
                  defaultValue={asset.note}
                  onBlur={(e) => {
                    const v = e.target.value;
                    if (v !== asset.note) onSetNote(asset.id, v);
                  }}
                />
              </section>

              <section>
                <div className="detail-heading">来源</div>
                <div className="mine-url">
                  <input
                    key={`url-${asset.id}`}
                    className="mine-url-input mono"
                    placeholder="来源 URL（选填）"
                    defaultValue={asset.sourceUrl}
                    onBlur={(e) => {
                      const v = e.target.value.trim();
                      if (v !== asset.sourceUrl) onSetSource(asset.id, v);
                    }}
                  />
                  {asset.sourceUrl && (
                    <button
                      className="mine-url-open"
                      title="用浏览器打开来源"
                      onClick={() => {
                        void source.openUrl(asset.sourceUrl).catch(() => {});
                      }}
                    >
                      <Icon name="reveal" size={12} stroke={1.5} />
                    </button>
                  )}
                </div>
              </section>

              {/* 操作放最后（owner 2026-11-17）：删改这类动作不该挡在查看的路径上 */}
              <section>
                <div className="detail-heading">操作（都作用在原文件上）</div>
                <button
                  className="act-primary"
                  onClick={() => onOpenDefault(asset)}
                  disabled={!source.desktop}
                >
                  <span>用默认程序打开</span>
                  <Icon name="image" size={13} stroke={1.5} />
                </button>
                <div className="act-row">
                  <button className="btn" onClick={() => onReveal(asset)}>
                    在资源管理器中显示
                  </button>
                  <button className="btn" onClick={() => onCopyFile(asset)} disabled={!source.desktop}>
                    复制文件
                  </button>
                </div>
                <div className="act-row">
                  <button className="btn" onClick={() => onPeek(asset)}>
                    贴图
                  </button>
                  <button
                    className="btn btn-danger"
                    onClick={() => onDelete(asset)}
                    disabled={!source.desktop}
                  >
                    删除
                  </button>
                </div>
                <p className="detail-note">
                  按住上面的预览图往外拖 = 把原文件拖到 Photoshop / 资源管理器；
                  拖到左侧目录树 = 移动到这个文件夹。出去/移走的都是磁盘上的原文件。
                </p>
              </section>
            </div>
          </>
        ) : (
          <p className="muted">选一个素材看详情。空格全屏预览，Esc 关闭。</p>
        )}
      </div>
    </aside>
  );
}

/**
 * 标签编辑器：形态与 FontManager 详情面板里的那一块一致
 *
 * ```
 * 标签
 * [● 品牌A ×] [✦ 4K ×]              ← chip：图标 + 名字 + 删除；自动标签虚线 + 魔棒
 * [ 添加标签…                  ✓ ]   ← 一行输入，回车或点 ✓ 打上
 * [ + 从已有标签选 45 ]              ← 展开选择面板：搜索 + 多选 + 一次加进去
 * ```
 *
 * 三处**不能改**的约定（用户明确"和 FontManager 一模一样"）：
 * 1. 手工标签点名字 = 按它筛列表，右键 = 改颜色/分组，× = 从这张素材上移除；
 * 2. 自动标签是虚线 chip + 魔棒图标，点也可以筛，右键 = 不再建议；
 * 3. "从已有标签选"的候选里**排除已在这张素材上的**，别让用户看到自己已经有的。
 */
function TagSection({
  manual,
  colors,
  auto,
  all,
  onAdd,
  onRemove,
  onPick,
  onDeny,
  onEdit,
}: {
  manual: string[];
  colors: Record<string, string>;
  auto: AutoTag[];
  all: string[];
  onAdd: (tag: string) => void;
  onRemove: (tag: string) => void;
  onPick: (tag: string) => void;
  onDeny: (tag: string) => void;
  onEdit: (tag: string) => void;
}) {
  const [q, setQ] = useState("");
  const [picker, setPicker] = useState(false);
  const [pq, setPq] = useState("");
  const [picked, setPicked] = useState<string[]>([]);

  /** 候选 = 已有手工标签里，这张素材还没有的那些 */
  const candidates = all.filter((t) => !manual.includes(t));
  const list = candidates.filter((t) => !pq.trim() || t.toLowerCase().includes(pq.trim().toLowerCase()));

  /* 换素材就把草稿收掉：上一条打的半截字不该跟着到下一张 */
  const firstId = manual.join("\u0000");
  useEffect(() => {
    setQ("");
    setPicked([]);
    setPq("");
    setPicker(false);
  }, [firstId]);

  const commit = (v: string) => {
    const t = v.trim();
    if (!t) return;
    onAdd(t);
    setQ("");
  };

  const togglePick = (t: string) => {
    setPicked((cur) => (cur.includes(t) ? cur.filter((x) => x !== t) : [...cur, t]));
  };

  const commitPicked = () => {
    for (const t of picked) onAdd(t);
    setPicked([]);
    setPicker(false);
    setPq("");
  };

  return (
    <div className="tag-editor">
      {manual.map((t) => (
        <span
          key={t}
          className={`tag-chip tag-chip-iconed ${colorClass(colors[t])}`}
          title="点名字按它筛选；右键改颜色 / 分组"
          onContextMenu={(e) => {
            e.preventDefault();
            onEdit(t);
          }}
        >
          <span className="tag-dot tag-dot-chip" aria-hidden />
          <Icon name="tag" size={10} stroke={1.5} className="tag-manual-icon" />
          <button className="tag-chip-text" onClick={() => onPick(t)}>
            {t}
          </button>
          <button
            className="tag-chip-x"
            title="从这张素材上移除此标签"
            aria-label={`移除标签 ${t}`}
            onClick={() => onRemove(t)}
          >
            <Icon name="close" size={9} stroke={2} />
          </button>
        </span>
      ))}

      {auto.length > 0 && (
        <div className="tag-auto-row">
        {auto.map((a) => (
        <span
          key={a.tag}
          className="tag-chip tag-chip-auto"
          title={`${a.reason}｜右键可「不再建议」`}
          onContextMenu={(e) => {
            e.preventDefault();
            onDeny(a.tag);
          }}
        >
          <Sparkles size={10} strokeWidth={1.6} className="tag-auto-spark" />
          <button className="tag-chip-text" onClick={() => onPick(a.tag)}>
            {isAutoTag(a.tag) ? autoTagLabel(a.tag) : a.tag}
          </button>
        </span>
        ))}
        </div>
      )}

      {/* 一行输入 + ✓：与 FontManager 同一个手感（回车也提交，Esc 清空收起） */}
      <div className="tag-input">
        <BlinkCaretInput
          value={q}
          onChange={setQ}
          placeholder="添加标签…"
          fieldClassName="tag-field"
          inputClassName="tag-input-field"
          ariaLabel="标签名"
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit(q);
            } else if (e.key === "Escape") {
              e.stopPropagation();
              setQ("");
              setPicker(false);
            }
          }}
        />
        <button
          className="tag-input-ok"
          title="打上这个标签"
          aria-label="打上这个标签"
          disabled={!q.trim()}
          onClick={() => commit(q)}
        >
          <Check size={12} strokeWidth={2} />
        </button>
      </div>

      {/* 照 FontManager：虚线小按钮 + 候选数徽标，点开后在**文档流里**展开列表
         （面板本身就是滚动容器，浮层会被裁在底边上，也不用为它算高度）。 */}
      <div className="tag-pick-wrap">
        <button
          className={`tag-pick-btn ${picker ? "open" : ""}`}
          aria-expanded={picker}
          onClick={() => setPicker((v) => !v)}
        >
          <Icon name="plus" size={11} stroke={1.6} />
          从已有标签选
          {candidates.length > 0 && (
            <span className="tag-pick-badge tabular">{candidates.length}</span>
          )}
        </button>

      {picker && (
        <div className="tag-picker" role="dialog" aria-label="选标签">
          <div className="tag-picker-head">
            <input
              autoFocus
              value={pq}
              onChange={(e) => setPq(e.target.value)}
              placeholder="搜索标签…"
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.stopPropagation();
                  setPicker(false);
                  setPq("");
                }
              }}
            />
            <button
              className="tag-picker-close"
              aria-label="关闭选择面板"
              onClick={() => {
                setPicker(false);
                setPq("");
              }}
            >
              <Icon name="close" size={11} stroke={2} />
            </button>
          </div>
          <ul className="tag-picker-list">
            {list.length === 0 ? (
              <li className="tag-picker-empty">没有匹配的标签</li>
            ) : (
              list.map((t) => {
                const on = picked.includes(t);
                return (
                  <li key={t}>
                    <button className={`tag-picker-item ${on ? "on" : ""}`} onClick={() => togglePick(t)}>
                      <span className="tag-picker-mark">{on && <Check size={11} strokeWidth={2} />}</span>
                      <span className="tag-picker-name">{t}</span>
                    </button>
                  </li>
                );
              })
            )}
          </ul>
          <div className="tag-picker-foot">
            <button className="tag-picker-add" disabled={picked.length === 0} onClick={commitPicked}>
              {picked.length > 0 ? `添加 ${picked.length} 个标签` : "添加标签"}
            </button>
          </div>
        </div>
      )}
      </div>

      {manual.length === 0 && auto.length === 0 && (
        <p className="detail-note">还没有标签。在上面输入名字回车就能打上，也可以从已有标签里选。</p>
      )}
    </div>
  );
}

/** 走"内嵌预览"路线的格式：RAW / PSD。它们的 quality=2 但来源是文件自带的 JPEG。 */
const EMBEDDED_EXT = new Set([
  "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "rwl",
  "srw", "pef", "raw", "3fr", "fff", "iiq", "mos", "mrw", "mef", "erf", "kdc", "dcr", "x3f",
  "psd", "psb",
]);

/** 缩略图到底从哪来（T12）：详情面板必须如实说，不能一律写"本地解码"。 */
function thumbSourceLabel(a: Asset): string {
  if (a.quality === 2) {
    return EMBEDDED_EXT.has(a.ext) ? "文件内嵌预览（未解原图）" : "系统缩略图（Windows 提供者）";
  }
  if (a.quality === 1) return "本地解码原图";
  return "还没生成（滚到它时才出图）";
}

/* 数量格式化给标题栏状态用（暂未使用，保留导出避免 tree-shake 报错） */
export const fmtCount = fmtNum;
