/**
 * 窗口外壳与详情 —— 标题栏、顶栏（搜索 / 筛选 / 排序）、右侧详情面板。
 *
 * 文件内按 `---- 标题栏 ----`、`---- 顶栏 ----`、`---- 详情 ----` 三条横幅分节，
 * 新东西必须落进对应分节，不要在文件末尾追加。
 * 本文件不持全局状态（状态在 `App.tsx`），文件操作一律走 `source.xxx`，不直接 invoke。
 */
import { memo, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { SearchSuggest, useSuggest, type SuggestScope } from "./SearchSuggest";
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
import { useT, type TKey } from "../lib/i18n";
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
  const t = useT();
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
          <button className="tb-action" title={t("chrome.undo")} onClick={onUndo} disabled={busy}>
            <Icon name="undo" size={14} stroke={1.5} />
          </button>
          <button className="tb-action" title={t("chrome.add_folder")} onClick={onAddFolder} disabled={busy}>
            {/* 照 FontManager 的「打开文件夹 + 外挂加号」（owner 2026-11-18 指定），
                不是 lucide 的 FolderPlus —— 那个是闭合文件夹，认不出来。 */}
            <Icon name="folder-plus" size={14} />
          </button>
          <button className="tb-action" title={t("chrome.rescan")} onClick={onRescan} disabled={busy}>
            <Icon name="scan" size={14} stroke={1.5} />
          </button>
          <button className="tb-action" title={t("chrome.settings")} onClick={onSettings}>
            <Icon name="settings" size={14} stroke={1.5} />
          </button>
        </div>
        <span className="titlebar-sep" aria-hidden />
        <button className="tc-btn" title={t("chrome.minimize")} onClick={win("am_minimize")}>
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <path d="M2.5 6h7" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
          </svg>
        </button>
        <button className="tc-btn" title={t("chrome.maximize")} onClick={win("am_toggle_max")}>
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <rect x="2.5" y="2.5" width="7" height="7" fill="none" stroke="currentColor" strokeWidth="1.1" />
          </svg>
        </button>
        <button className="tc-close" title={t("chrome.close")} onClick={win("am_close")}>
          <Icon name="close" size={13} />
        </button>
      </div>
    </header>
  );
}

/* ---------------------------------------------------------------- 顶栏 */

/** 排序项 → 文案 key（`t` 只能在渲染期取，所以这里只存 key） */
const SORT_LABEL_KEY: Record<SortKey, TKey> = {
  mtime_desc: "topbar.sort_mtime_desc",
  name_asc: "topbar.sort_name",
  size_desc: "topbar.sort_size",
  rating_desc: "topbar.sort_rating",
};

/**
 * 类型段：**唯一**的 kindKey → 后端 kinds 映射（`App.tsx` 的查询直接从这张表派生，
 * 以前两边各写一份，加一种类型要改两处 —— 方案 §4.7）。
 *
 * `countTag` 是后端自动标签里对应的那个名字（`core/src/autotag.rs::kind_tag`）。
 * ⚠️ 3D 的后端标签叫 `kind:model`（KIND_MODEL），不是 `kind:3d` —— 照 UI 的 key 猜会一个都命中不了。
 * 计数来自 `source.autoTags()`（跨库汇总，和侧栏「自动标签」同一份数字），所以它是
 * **整个素材库**的条数，不是当前目录的条数；拿不到就不显示（别造 0）。
 */
export const KINDS: {
  key: string;
  labelKey: TKey;
  kinds: number[] | null;
  countTag?: string;
}[] = [
  { key: "all", labelKey: "topbar.kind_all", kinds: null },
  { key: "image", labelKey: "topbar.kind_image", kinds: [0], countTag: "kind:image" },
  { key: "3d", labelKey: "topbar.kind_3d", kinds: [3], countTag: "kind:model" },
  { key: "video", labelKey: "topbar.kind_video", kinds: [1], countTag: "kind:video" },
  { key: "doc", labelKey: "topbar.kind_doc", kinds: [4], countTag: "kind:doc" },
  { key: "audio", labelKey: "topbar.kind_audio", kinds: [2], countTag: "kind:audio" },
  { key: "other", labelKey: "topbar.kind_other", kinds: [5], countTag: "kind:other" },
];

const SIZE_LABEL_KEY: TKey[] = ["topbar.size_small", "topbar.size_medium", "topbar.size_large"];
const SIZE_PX = [120, 176, 260];

/** 色桶键 → 文案 key（`COLOR_LABEL` 是 `lib/tags` 里的中文表，这里只补翻译层） */
const COLOR_LABEL_KEY: Record<string, TKey> = {
  gray: "topbar.color_gray",
  red: "topbar.color_red",
  orange: "topbar.color_orange",
  amber: "topbar.color_amber",
  yellow: "topbar.color_yellow",
  green: "topbar.color_green",
  teal: "topbar.color_teal",
  blue: "topbar.color_blue",
  indigo: "topbar.color_indigo",
  violet: "topbar.color_violet",
  pink: "topbar.color_pink",
  brown: "topbar.color_brown",
  black: "topbar.color_black",
  white: "topbar.color_white",
};

function TopBarImpl({
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
  trashMode,
  trashFormats,
  trashFormat,
  onTrashFormat,
  suggestScope,
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
  /** 回收站模式（owner 2026-11-19）：只留**搜索 / 筛选（按格式）/ 排序**，
   *  缩略图大小与网格↔瀑布流切换对一张列表没有意义，整块不渲染。 */
  trashMode?: boolean;
  /** 回收站里出现过的扩展名 + 条数（筛选菜单用） */
  trashFormats?: [string, number][];
  trashFormat?: string | null;
  onTrashFormat?: (ext: string | null) => void;
  /** 搜索建议的范围 = 主列表当前的目录 / 筛选（方案 §4.3，见 `SearchSuggest` 里的说明） */
  suggestScope?: SuggestScope;
}) {
  const t = useT();
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
    landscape: t("topbar.orient_landscape"),
    portrait: t("topbar.orient_portrait"),
    square: t("topbar.orient_square"),
  };
  /* ---- 搜索联想（0.3.30，照 FontManager）----
     `sugOpen` 管面板开合，`sugActive` 是键盘选中项（-1 = 还没用方向键选过）。
     数据源见 `useSuggest`：图片走后端模糊查、标签本地过滤。 */
  const [sugOpen, setSugOpen] = useState(false);
  const [sugActive, setSugActive] = useState(-1);
  const sugItems = useSuggest(source, text, tags, sugOpen && !trashMode, suggestScope);

  const onSearchKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    /* Esc：**只在搜索框有内容且自身获得焦点时**清词（方案 §4.6）。
       这里必须拦住冒泡：App 的全局 Esc 会把详情浮层 / 大图 / 多选一起收掉，
       用户只是想清掉关键词，不该顺带把别的临时状态也关掉。
       有词时只清词、不关面板（清空后建议列表本来就是空的）；没词时退回原来的
       「关面板」，再按一次才轮到全局 Esc —— 全局那条的活一点没抢。 */
    if (e.key === "Escape" && text && !e.nativeEvent.isComposing) {
      e.preventDefault();
      e.stopPropagation();
      onText("");
      setSugOpen(false);
      setSugActive(-1);
      return;
    }
    /* 面板没内容时只处理 Esc（关面板），别把方向键吃掉 —— 输入框里左右移动光标是正常操作 */
    if (!sugItems.length) {
      if (e.key === "Escape") setSugOpen(false);
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSugActive((i) => Math.min(sugItems.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSugActive((i) => Math.max(-1, i - 1));
    } else if (e.key === "Enter") {
      /* 没选过就用第一条：用户敲完字直接回车，得到的是最像的那条 */
      const pick = sugItems[sugActive >= 0 ? sugActive : 0];
      if (pick) {
        e.preventDefault();
        onText(pick.value);
        setSugOpen(false);
        setSugActive(-1);
      }
    } else if (e.key === "Escape") {
      /* 必须 stopPropagation：App 的全局 Esc 会顺手把详情面板也收掉 */
      e.preventDefault();
      e.stopPropagation();
      setSugOpen(false);
    }
  };

  const kind = KINDS.find((k) => k.key === kindKey) ?? KINDS[0];
  /* 漏斗指示灯 = **真的有没有筛选**（方案 P1-4）：
     只认类型 / 标签（含形状、格式）/ 评分；目录是「我在哪」不算，颜色有自己那颗按钮。
     回收站里只认格式（那是它唯一的维度）。 */
  const filterOn = trashMode
    ? !!trashFormat
    : kindKey !== "all" || !!activeTag || rating !== null;
  /* 回收站的"最近修改"其实是"最近删除"（条目只有删除时间这一个时间戳），
     文案必须跟着换，否则用户以为在按原文件的修改时间排。 */
  const sortLabel =
    trashMode && sort === "mtime_desc" ? t("topbar.sort_deleted") : t(SORT_LABEL_KEY[sort]);
  const sortKeys: SortKey[] = trashMode
    ? ["mtime_desc", "name_asc", "size_desc"]
    : (Object.keys(SORT_LABEL_KEY) as SortKey[]);

  /** 颜色名（`COLOR_LABEL` 里没有的键原样显示，别让筛选按钮出现空白） */
  const colorLabel = (c: string) =>
    COLOR_LABEL_KEY[c] ? t(COLOR_LABEL_KEY[c]) : (COLOR_LABEL[c] ?? c);

  const openMenu = (e: React.MouseEvent<HTMLElement>, k: "sort" | "filter" | "color") => {
    const r = e.currentTarget.getBoundingClientRect();
    /* 传**右沿**：三个下拉都照 FontManager 贴按钮右侧展开（`.sort-menu { right: 0 }`），
       菜单比按钮宽，所以它是向左伸出去的。以前传左沿 + 左对齐，跟 FontManager 不是一个方向。 */
    setMenu({ x: r.right, y: r.bottom + 6, kind: k });
  };

  const autoOf = (ns: string) => autoTags.filter((t) => t.name.startsWith(`${ns}:`));
  const pickTag = (name: string) => onTag(activeTag === name ? null : name);

  /* ---- 形状 / 格式 / 标签 三段共用一个 `tagFilter`，必须按命名空间分开判定 ----
     （方案 §4.4：以前 `meta: activeTag ? "1"` 让只筛了 `fmt:jpg` 时「标签」行也显示 1，
     点「全部形状」还会把手工标签一起清掉。）
     「标签」行代表**除形状 / 格式之外**的所有标签（手工 + `place:`/`kind:`/`size:`）。
     只认手工标签会留一个洞：筛了 `place:4k` 时三行都不亮，用户看不出条件挂在哪。 */
  const isShapeTag = !!activeTag?.startsWith("orient:");
  const isFmtTag = !!activeTag?.startsWith("fmt:");
  const isPlainTag = !!activeTag && !isShapeTag && !isFmtTag;
  /** 行尾 meta：显示**当前值**而不是 "1"（菜单宽 150–260，超长要省略） */
  const short = (s: string, n = 12) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);
  const plainTagLabel = isPlainTag
    ? short(isAutoTag(activeTag!) ? autoTagLabel(activeTag!) : activeTag!)
    : undefined;
  const shapeLabel = isShapeTag
    ? short(ORIENT_LABEL[activeTag!.slice("orient:".length)] ?? activeTag!)
    : undefined;
  const fmtLabel = isFmtTag ? short(activeTag!.slice("fmt:".length).toUpperCase()) : undefined;
  /** 类型行计数：来自自动标签（跨库汇总，与侧栏 kind:* 同一份数字）；没有就返回 undefined */
  const kindCount = (tag?: string) => (tag ? autoTags.find((x) => x.name === tag)?.count : undefined);

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
    { label: t("topbar.type"), heading: true },
    ...KINDS.map((k) => ({
      label: t(k.labelKey),
      /* 计数只给具体类型；「全部类型」不给 —— 那是跨库合计，当前目录下对不上，
         宁可空着也不报一个假的"一共 N 条"（方案 §4.7 / §5） */
      meta: (() => {
        const n = kindCount(k.countTag);
        return n === undefined ? undefined : String(n);
      })(),
      on: kindKey === k.key,
      check: kindKey === k.key,
      onPick: () => onKind(k.key),
    })),

    { sep: true, label: "" },
    /* 下面几段像 FontManager 那样收进**二级菜单**：
       主菜单只留入口行，内容在左边的浮层里选 —— 一屏就能看全所有维度，
       不必在一个长列表里滚半天（owner 2026-11-17 指定）。 */
    { label: t("topbar.scope"), heading: true },
    {
      label: t("topbar.tags"),
      meta: plainTagLabel,
      on: isPlainTag,
      sub: [
        {
          label: t("topbar.all_tags"),
          icon: ICO_TAG,
          on: !isPlainTag,
          check: !isPlainTag,
          /* 只清自己命名空间：筛着 `fmt:png` 时点「全部标签」不该把格式也放掉 */
          onPick: () => {
            if (isPlainTag) onTag(null);
          },
        },
        ...tags.map((tag) => ({
          label: tag.name,
          /* 标签行的图标口径和 FontManager 一致：手工标签 = 标签图标（不再是色点） */
          icon: isAutoTag(tag.name) ? ICO_AUTO : ICO_TAG,
          meta: String(tag.count),
          on: activeTag === tag.name,
          check: activeTag === tag.name,
          onPick: () => pickTag(tag.name),
        })),
        /* 自动标签也要在这里出现（owner 2026-11-17：「筛选下、标签下自动标签没显示出来」）。
           FontManager 的筛选菜单是**手工 + 自动**混在同一列（按名字排序），
           因为「筛选菜单是用户唯一能勾标签的地方」；我们侧栏虽然单独分了「自动标签」一段，
           但顶栏这层菜单如果不给，用户在筛选里就找不到 fmt:/orient: 这些。
           排在手后面：手工是用户的，自动是软件推的，主次不能反。 */
        ...[...autoTags]
          .sort((a, b) => a.name.localeCompare(b.name))
          .map((tag) => ({
            label: autoTagLabel(tag.name),
            icon: ICO_AUTO,
            meta: String(tag.count),
            on: activeTag === tag.name,
            check: activeTag === tag.name,
            onPick: () => pickTag(tag.name),
          })),
      ],
    },
    {
      /* 「文件夹」不是筛选，是「我在哪」（方案 P1-4 / §4.2）：
         启动就会选中第一个库根，如果算作筛选，漏斗指示灯从第一秒起常亮，
         用户就再也没有"有没有筛选"的信号了。这里只如实显示当前目录名，不点亮。
         （选中某个目录仍然会换视图 —— 那是 `onDir` 的事，和数据条件无关。） */
      label: t("topbar.folders"),
      meta: dirNode ? short(dirNode.name) : undefined,
      sub: [
        { label: t("topbar.all_folders"), icon: ICO_DIR, on: !dirNode, check: !dirNode, onPick: () => onDir(null) },
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
      label: t("topbar.shape"),
      meta: shapeLabel,
      on: isShapeTag,
      sub: [
        {
          label: t("topbar.all_shapes"),
          icon: ICO_AUTO,
          on: !isShapeTag,
          check: !isShapeTag,
          /* 同样只清 `orient:*`：手工标签 / 格式必须留着（方案 §4.4 的验收点） */
          onPick: () => {
            if (isShapeTag) onTag(null);
          },
        },
        ...autoOf("orient").map((tag) => ({
          label: ORIENT_LABEL[tag.name.slice("orient:".length)] ?? tag.name,
          icon: ICO_AUTO,
          meta: String(tag.count),
          on: activeTag === tag.name,
          check: activeTag === tag.name,
          onPick: () => pickTag(tag.name),
        })),
      ],
    },
    {
      label: t("topbar.rating"),
      meta: rating !== null ? `${rating}★` : undefined,
      on: rating !== null,
      sub: [
        { label: t("topbar.all_ratings"), icon: ICO_STAR, on: rating === null, check: rating === null, onPick: () => onRating(null) },
        ...[5, 4, 3, 2, 1].map((n) => ({
          label: t("topbar.rating_at_least", { n }),
          icon: ICO_STAR,
          on: rating === n,
          check: rating === n,
          onPick: () => onRating(rating === n ? null : n),
        })),
      ],
    },
    {
      label: t("topbar.format"),
      meta: fmtLabel,
      on: isFmtTag,
      sub: [
        {
          label: t("topbar.all_formats"),
          icon: ICO_AUTO,
          on: !isFmtTag,
          check: !isFmtTag,
          onPick: () => {
            if (isFmtTag) onTag(null);
          },
        },
        ...autoOf("fmt").map((tag) => ({
          label: tag.name.slice("fmt:".length).toUpperCase(),
          icon: ICO_AUTO,
          meta: String(tag.count),
          on: activeTag === tag.name,
          check: activeTag === tag.name,
          onPick: () => pickTag(tag.name),
        })),
      ],
    },
  ];

  /* 回收站的筛选只有"格式"一个维度：标签 / 文件夹 / 评分 / 颜色都不属于已删除的文件
     （它们已经不在库里了），列出来只会点出一片空。 */
  const trashFilterItems: MenuItem[] = [
    {
      label: t("topbar.all_formats"),
      on: !trashFormat,
      check: !trashFormat,
      onPick: () => onTrashFormat?.(null),
    },
    ...(trashFormats ?? []).map(([ext, n]) => ({
      label: ext.toUpperCase(),
      meta: String(n),
      on: trashFormat === ext,
      check: trashFormat === ext,
      onPick: () => onTrashFormat?.(trashFormat === ext ? null : ext),
    })),
  ];

  return (
    <div className="topbar">
      <div className={`topbar-search ${text ? "has-clear" : ""}`}>
        <Icon name="search" size={13} stroke={1.7} className="tf-icon" />
        <BlinkCaretInput
          value={text}
          onChange={(v) => {
            onText(v);
            /* 一打字就开面板（和 FontManager 一致）；`useSuggest` 自己防抖 */
            setSugOpen(true);
            setSugActive(-1);
          }}
          placeholder={t("topbar.search_placeholder")}
          inputClassName="topbar-field"
          ariaLabel={t("topbar.search_aria")}
          onFocus={() => setSugOpen(true)}
          /* 点建议项那一下已经 `preventDefault`，不会走到这里；
             键盘 Tab 走开时靠它收面板 */
          onBlur={() => setSugOpen(false)}
          onKeyDown={onSearchKey}
        />
        {/* 有词才出现的 ×（方案 §4.6）：全选删除清空关键词的成本太高。
            mousedown 里 preventDefault —— 否则输入框先失焦，自绘块光标消失、
            建议面板也跟着收，看起来像"点了一下整块都关了"。 */}
        {text && (
          <button
            type="button"
            className="topbar-clear"
            title={t("topbar.search_clear")}
            aria-label={t("topbar.search_clear")}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              onText("");
              setSugOpen(false);
              setSugActive(-1);
            }}
          >
            <Icon name="close" size={10} stroke={2} />
          </button>
        )}
        {sugOpen && (
          <SearchSuggest
            items={sugItems}
            active={sugActive}
            onHover={setSugActive}
            onPick={(s) => {
              onText(s.value);
              setSugOpen(false);
              setSugActive(-1);
            }}
          />
        )}
      </div>

      {!trashMode && (
      <div className="topbar-size" title={t("topbar.thumb_size", { size: t(SIZE_LABEL_KEY[sizeIndex]) })}>
        <input
          type="range"
          min={0}
          max={2}
          step={1}
          value={sizeIndex}
          onChange={(e) => onSize(Number(e.target.value))}
          aria-label={t("topbar.thumb_size_label")}
        />
        <span className="size-value">{SIZE_PX[sizeIndex]}px</span>
      </div>
      )}

      {!trashMode && (
      <button
        className={`ctl ctl-icon ${color ? "on" : ""}`}
        onClick={(e) => openMenu(e, "color")}
        title={color ? t("topbar.filter_by_color", { color: colorLabel(color) }) : t("topbar.filter_by_dominant_color")}
        aria-label={t("topbar.filter_by_dominant_color")}
      >
        <span
          className="ctl-color-dot"
          style={color ? { background: `var(--c-${color}, #8b8b86)` } : undefined}
        />
      </button>
      )}

      <button
        className={`ctl ctl-icon ${filterOn ? "on" : ""}`}
        onClick={(e) => openMenu(e, "filter")}
        title={trashMode ? t("topbar.filter_trash") : t("topbar.filter_all")}
        aria-label={t("topbar.filter_by_type", { type: t(kind.labelKey) })}
        aria-pressed={filterOn}
      >
        <Icon name="funnel" size={13} stroke={1.5} className="ctl-ico" />
      </button>

      <div className="topbar-right">
        <button
          className="ctl"
          onClick={(e) => openMenu(e, "sort")}
          title={t("topbar.sort_title", { sort: sortLabel })}
        >
          <Icon name="sort" size={13} stroke={1.5} className="ctl-ico" />
          {sortLabel}
          <Icon name="chevron" size={10} stroke={2} className="rot90" />
        </button>

        {!trashMode && (
        <div className="view-toggle">
          <span
            className="view-pill"
            style={{ ["--i" as string]: view === "grid" ? 0 : 1 }}
            aria-hidden
          />
          <button
            className={`view-btn ${view === "grid" ? "on" : ""}`}
            title={t("topbar.view_grid")}
            onClick={() => onView("grid")}
          >
            <Icon name="grid" size={13} stroke={1.5} />
          </button>
          <button
            className={`view-btn ${view === "waterfall" ? "on" : ""}`}
            title={t("topbar.view_waterfall")}
            onClick={() => onView("waterfall")}
          >
            <Icon name="waterfall" size={13} stroke={1.5} />
          </button>
        </div>
        )}
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
              ? sortKeys.map((k) => ({
                  label: t(SORT_LABEL_KEY[k]),
                  on: sort === k,
                  check: sort === k,
                  onPick: () => onSort(k),
                }))
              : menu.kind === "color"
              ? [
                  {
                    label: t("topbar.all_colors"),
                    on: color === null,
                    check: color === null,
                    onPick: () => onColor(null),
                    /* 全部颜色没有"自己的颜色" → 用一个七彩环点，和下面单色点区分开 */
                    dot: "conic-gradient(#d94a45,#d9a13a,#d9cf4a,#4f9d5d,#4a7fd9,#8c5ad9,#d94a45)",
                  },
                  ...(colorCounts.length
                    ? colorCounts.map(([k, n]) => ({
                        label: colorLabel(k),
                        meta: String(n),
                        on: color === k,
                        check: color === k,
                        onPick: () => onColor(k),
                        /* 行首圆点 = 这个颜色本身（与顶栏那颗、缩略图上的色点同一套变量） */
                        dot: `var(--c-${k}, #8b8b86)`,
                      }))
                    : [{ label: t("topbar.no_color_data"), on: false, onPick: () => {} }]),
                ]
              : trashMode
                ? trashFilterItems
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
  const t = useT();

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
      title={t("chrome.preview_tip")}
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
  const t = useT();
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
        title={
          rating === 0
            ? t("chrome.rate_none_tip")
            : rating === 1
              ? t("chrome.rate_one_tip")
              : t("chrome.rate_many_tip", { n: rating })
        }
        aria-label={t("chrome.rating")}
        aria-expanded={open}
        onMouseEnter={() => show(RATING_OPEN_DELAY)}
        onMouseLeave={hide}
        onFocus={() => show(0)}
        onClick={() => {
          /* owner 2026-11-19：单击 = 直接给 1 星，再点一下取消（0）。
             精确的 2–5 星仍由悬浮面板给 —— 鼠标停上去就出来，所以单击不再只是"开门"。 */
          onChange(rating === 1 ? 0 : 1);
          closeNow();
        }}
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
            aria-label={t("chrome.rating_pop")}
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
                  title={t("chrome.star_n", { n })}
                  aria-label={t("chrome.star_n", { n })}
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
                {hover > 0
                  ? t("chrome.star_n", { n: hover })
                  : rating > 0
                    ? t("chrome.star_n", { n: rating })
                    : t("chrome.unrated")}
              </span>
              {rating > 0 && (
                <button className="rating-pop-clear" onClick={() => rate(0)}>
                  <Icon name="close" size={9} stroke={2} />
                  {t("chrome.clear")}
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
  const t = useT();
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
      aria-label={t("chrome.rename_file")}
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
  onBeginDrag,
  onToast,
  tags,
  colors,
  autoTags,
  allAutoTags,
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
  pop,
  onClosePop,
}: {
  asset: Asset | null;
  source: Source;
  thumbSrc: (a: Asset, px: number) => string;
  /** 当前界面档位对应的缩略图像素（详情里如实写，不能一律写 512） */
  thumbPx: number;
  onBeginDrag: (a: Asset, x: number, y: number) => void;
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
  /** 当前素材的手工标签 */
  tags: string[];
  /** 标签名 → 颜色键（详情 chip 上的小色点） */
  colors: Record<string, string>;
  /** 已按「手工优先 + 否决名单」过滤过的自动标签（带理由） */
  autoTags: AutoTag[];
  /** 已有手工标签（做候选） */
  allTags: string[];
  /** 全库的自动标签（带使用数）：选择面板要列出**所有**标签，不只是这张图推导出来的那几个 */
  allAutoTags: TagCount[];
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
  /** 右键浮层：非空 = 以「鼠标旁 400×(≤800)」的浮层形态出现（0.4.24） */
  pop?: { x: number; y: number } | null;
  onClosePop?: () => void;
}) {
  const t = useT();
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

  /* ---- 0.4.24：详情不再常驻、也不做右边缘抽屉 ----
     它只在「右键某张素材」时以浮层出现，**原地渐入**（不滑动）——
     owner 2026-11-21：详情是"顺手看一眼"的东西，跟着鼠标来、点外面就走。
     主界面因此把这 348px 还给了网格；左侧素材库另有自己的收起/滑出（见 Tree.tsx）。 */
  const isPop = !!pop && !!asset;

  /** 浮层落点：默认鼠标右下，右边/下边放不下就翻到另一侧，再不行夹在视口内 */
  const popPos = useMemo(() => {
    if (!pop) return null;
    const W = 400;
    const pad = 10;
    const H = Math.min(800, window.innerHeight - pad * 2);
    let left = pop.x + 14;
    if (left + W > window.innerWidth - pad) left = pop.x - W - 14;
    left = Math.min(Math.max(pad, left), Math.max(pad, window.innerWidth - W - pad));
    let top = pop.y - 36;
    top = Math.min(Math.max(pad, top), Math.max(pad, window.innerHeight - H - pad));
    return { left, top, width: W, height: H };
  }, [pop]);

  /* 浮层：点外部或 Esc 关掉（owner 选的「看完即走」） */
  useEffect(() => {
    if (!isPop || !onClosePop) return;
    const down = (e: PointerEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && t.closest(".detail")) return;
      onClosePop();
    };
    document.addEventListener("pointerdown", down, true);
    return () => document.removeEventListener("pointerdown", down, true);
  }, [isPop, onClosePop]);

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

  /* 0.4.11 复制文件名改成 Shift+双击：单击复制太容易误触（选了半天素材，
     手一抖点在文件名上就弹 toast），且单击路径不再需要延时器。 */
  const titleCopiedAt = useRef(0);
  const copyTitle = (name: string) => {
    const now = Date.now();
    if (now - titleCopiedAt.current < 350) return;
    titleCopiedAt.current = now;
    void copy(name, t("chrome.what_filename"));
  };

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
      onBeginDrag(a, e.clientX, e.clientY);
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
  }, [onBeginDrag]);

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
    onToast(ok ? t("chrome.copied", { what }) : t("chrome.copy_failed"), !ok);
  };

  /* 没选中素材时什么都不渲染；右键浮层关闭后也就整块卸掉（不留空壳、不占位）。
     形态只有一种：跟鼠标的浮层，渐入出现（CSS 里 `detail-pop-in`）。 */
  if (!asset || !isPop || !popPos) return null;
  return (
      <aside
        className="detail is-pop"
        style={{ left: popPos.left, top: popPos.top, width: popPos.width, height: popPos.height }}
        aria-label={t("chrome.detail")}
      >
      <div
        className="detail-resize"
        role="separator"
        aria-orientation="vertical"
        title={t("chrome.resize_tip")}
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
                  title={t("chrome.title_tip")}
                  onClick={(e) => {
                    /* 用 click 的 detail 判双击，而不是只靠 onDoubleClick：
                       WebView2 在部分输入路径下不派发 dblclick（自测注入点击就是如此），
                       detail 则是「这一串里的第几次点击」，稳。
                       0.4.11 起单击不做事（原来单击复制太容易误触），复制挪到 Shift+双击。 */
                    if (e.detail < 2) return;
                    if (e.shiftKey) copyTitle(asset.name);
                    else onStartRename();
                  }}
                  /* detail 判双击之外再挂一条 dblclick：两条任意一条先到都进改名，
                     onStartRename 是幂等的（只设同一个 id）。
                     Shift+双击走 copyTitle —— 复制不幂等（会弹两个 toast），
                     click(detail≥2) 与 dblclick 真鼠标下都会到，350ms 内只认第一次。 */
                  onDoubleClick={(e) => {
                    if (e.shiftKey) copyTitle(asset.name);
                    else onStartRename();
                  }}
                >
                  {asset.name}
                </button>
              )}
              <RatingStars
                rating={asset.rating}
                onChange={(n) => onSetRating([asset.id], n)}
              />
              {/* 0.4.9 删掉「关闭详情」按钮：面板是常驻的，这个 × 实际只是取消选中，
                  关不掉任何东西，反而让用户以为点它会收起右半边。Esc 的行为不变。 */}
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
                  <div className="detail-palette" aria-label={t("chrome.palette")}>
                    {palette.slice(0, 6).map((hex, i) => (
                      <button
                        key={`${hex}-${i}`}
                        style={{ background: `#${hex}` }}
                        title={t("chrome.hex_copy_tip", { hex: hex.toUpperCase() })}
                        onClick={() => void copy(`#${hex.toUpperCase()}`, t("chrome.what_color"))}
                      />
                    ))}
                  </div>
                )}
              </div>

              <div className="detail-artmeta mono">
                <span>{asset.width && asset.height ? `${asset.width} × ${asset.height}` : "—"}</span>
                <span>{fmtSize(asset.size)}</span>
                <span>{KIND_LABEL[asset.kind] ?? t("chrome.kind_other")}</span>
              </div>

              <section>
                <div className="detail-heading">{t("chrome.sec_basic")}</div>
                <dl className="detail-meta">
                  <dt>{t("chrome.fmt")}</dt>
                  <dd>{asset.ext.toUpperCase()}</dd>
                  <dt>{t("chrome.dimensions")}</dt>
                  <dd className="mono">
                    {asset.width && asset.height ? `${asset.width} × ${asset.height}` : "—"}
                  </dd>
                  <dt>{t("chrome.size")}</dt>
                  <dd className="mono">{fmtSize(asset.size)}</dd>
                  <dt>{t("chrome.mtime")}</dt>
                  <dd className="mono">{fmtTime(asset.mtime)}</dd>
                  <dt>{t("chrome.added")}</dt>
                  <dd
                    className="mono"
                    /* 0.1.16 及更早的库里没有这个值 —— 如实写「未知」，
                       不拿 mtime 顶上（那是文件的修改时间，不是入库时间） */
                    title={
                      asset.addedAt
                        ? t("chrome.added_tip")
                        : t("chrome.added_tip_unknown")
                    }
                  >
                    {asset.addedAt ? fmtTime(asset.addedAt) : t("chrome.unknown")}
                  </dd>
                  <dt>{t("chrome.location")}</dt>
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
                    <span className="detail-heading">{t("chrome.sec_exif")}</span>
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
                            <dt>{t("chrome.exif_model")}</dt>
                            <dd>
                              {exif.make ? `${exif.make} ` : ""}
                              {exif.model}
                            </dd>
                          </>
                        )}
                        {exif.lens && (
                          <>
                            <dt>{t("chrome.exif_lens")}</dt>
                            <dd className="wrap">{exif.lens}</dd>
                          </>
                        )}
                        {exif.focal != null && (
                          <>
                            <dt>{t("chrome.exif_focal")}</dt>
                            <dd className="mono">{exif.focal}</dd>
                          </>
                        )}
                        {exif.fNumber != null && (
                          <>
                            <dt>{t("chrome.exif_aperture")}</dt>
                            <dd className="mono">f/{exif.fNumber}</dd>
                          </>
                        )}
                        {exif.exposure && (
                          <>
                            <dt>{t("chrome.exif_shutter")}</dt>
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
                            <dt>{t("chrome.exif_taken")}</dt>
                            <dd className="mono">{fmtExifTime(exif.taken)}</dd>
                          </>
                        )}
                        {exif.software && (
                          <>
                            <dt>{t("chrome.exif_software")}</dt>
                            <dd className="wrap">{exif.software}</dd>
                          </>
                        )}
                      </dl>
                    </div>
                  )}
                </section>
              )}

              {/* 来源（人写的）排在「引用」上面：owner 2026-11-19 ——
                  引用是排障信息、默认收起，来源是可编辑的元数据，必须一眼能看到。 */}
              <section>
                <div className="detail-heading">{t("chrome.sec_source")}</div>
                <div className="mine-url">
                  <input
                    key={`url-${asset.id}`}
                    className="mine-url-input mono"
                    placeholder={t("chrome.source_placeholder")}
                    defaultValue={asset.sourceUrl}
                    onBlur={(e) => {
                      const v = e.target.value.trim();
                      if (v !== asset.sourceUrl) onSetSource(asset.id, v);
                    }}
                  />
                  {asset.sourceUrl && (
                    <button
                      className="mine-url-open"
                      title={t("chrome.open_source")}
                      onClick={() => {
                        void source.openUrl(asset.sourceUrl).catch(() => {});
                      }}
                    >
                      <Icon name="reveal" size={12} stroke={1.5} />
                    </button>
                  )}
                </div>
              </section>

              {/* 引用（默认收起）：显示层 + 谁在用它 + 它引用了谁。
                  这些是"排障 / 想知道缩略图哪来的"才看的东西，不该占满屏。 */}
              <section className="detail-fold-sec">
                <button
                  className="detail-fold"
                  aria-expanded={refsOpen}
                  onClick={() => setRefsOpen((v) => !v)}
                >
                  <span className="detail-heading">{t("chrome.sec_refs")}</span>
                  <Icon name="chevron" size={12} stroke={2} className={refsOpen ? "rot90" : ""} />
                </button>

                {refsOpen && (
                  <div className="detail-fold-body">
                    {!PREVIEWABLE_KINDS.has(asset.kind) ? (
                      <dl className="detail-meta">
                        <dt>{t("chrome.thumb")}</dt>
                        <dd className="wrap">{t("chrome.thumb_none")}</dd>
                        <dt>{t("chrome.thumb_how")}</dt>
                        <dd className="wrap">{t("chrome.thumb_how_val")}</dd>
                      </dl>
                    ) : (
                      <dl className="detail-meta">
                        <dt>{t("chrome.thumb")}</dt>
                        <dd className="mono">
                          {t("chrome.thumb_webp", { px: thumbPx })}
                        </dd>
                        <dt>{t("chrome.sec_source")}</dt>
                        <dd className="wrap">{thumbSourceLabel(asset, t)}</dd>
                        {asset.quality === 2 && (
                          <>
                            <dt>{t("chrome.thumb_quality")}</dt>
                            <dd className="wrap">
                              {t("chrome.thumb_limited", {
                                from: EMBEDDED_EXT.has(asset.ext)
                                  ? t("chrome.thumb_from_embedded")
                                  : t("chrome.thumb_from_system"),
                              })}
                            </dd>
                          </>
                        )}
                        <dt>{t("chrome.thumb_use")}</dt>
                        <dd className="wrap">{t("chrome.thumb_use_val")}</dd>
                      </dl>
                    )}

                    {refsIn.length > 0 && (
                      <>
                        <div className="detail-sub">{t("chrome.refs_in", { n: refsIn.length })}</div>
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
                          {t("chrome.refs_out", {
                            n: refsOut.length,
                            m: refsOut.filter((r) => r.missing).length,
                          })}
                        </div>
                        <ul className="ref-list">
                          {refsOut.slice(0, 8).map((r) => (
                            <li
                              key={`out-${r.dstPath}`}
                              className={r.missing ? "miss" : ""}
                              title={r.dstPath}
                            >
                              {r.dstName || r.dstPath}
                              {r.missing && <span className="miss-tag">{t("chrome.ref_missing")}</span>}
                            </li>
                          ))}
                        </ul>
                      </>
                    )}
                    {refsIn.length === 0 && refsOut.length === 0 && PREVIEWABLE_KINDS.has(asset.kind) && (
                      <p className="detail-note">{t("chrome.refs_none")}</p>
                    )}
                  </div>
                )}
              </section>

              <section>
                <div className="detail-heading">{t("chrome.sec_tags")}</div>
                <TagSection
                  manual={tags}
                  colors={colors}
                  auto={autoTags}
                  all={allTags}
                  allAuto={allAutoTags}
                  onAdd={onAddTag}
                  onRemove={onRemoveTag}
                  onPick={onPickTag}
                  onDeny={onDenyAutoTag}
                  onEdit={onEditTag}
                />
              </section>

              <section>
                <div className="detail-heading">{t("chrome.sec_note")}</div>
                {/* 备注 / 来源 URL 都是**人写的东西**：扫描与重建索引永远不碰它们 */}
                <textarea
                  key={`note-${asset.id}`}
                  className="mine-note"
                  rows={3}
                  placeholder={t("chrome.note_placeholder")}
                  defaultValue={asset.note}
                  onBlur={(e) => {
                    const v = e.target.value;
                    if (v !== asset.note) onSetNote(asset.id, v);
                  }}
                />
              </section>


              {/* 操作放最后（owner 2026-11-17）：删改这类动作不该挡在查看的路径上 */}
              <section>
                <div className="detail-heading">{t("chrome.sec_actions")}</div>
                <button
                  className="act-primary"
                  onClick={() => onOpenDefault(asset)}
                  disabled={!source.desktop}
                >
                  <span>{t("chrome.act_open")}</span>
                  <Icon name="image" size={13} stroke={1.5} />
                </button>
                <div className="act-row">
                  <button className="btn" onClick={() => onReveal(asset)}>
                    {t("chrome.act_reveal")}
                  </button>
                  <button className="btn" onClick={() => onCopyFile(asset)} disabled={!source.desktop}>
                    {t("chrome.act_copy")}
                  </button>
                </div>
                <div className="act-row">
                  <button className="btn" onClick={() => onPeek(asset)}>
                    {t("chrome.act_peek")}
                  </button>
                  <button
                    className="btn btn-danger"
                    onClick={() => onDelete(asset)}
                    disabled={!source.desktop}
                  >
                    {t("chrome.act_delete")}
                  </button>
                </div>
                <p className="detail-note">
                  {t("chrome.act_drag_note")}
                </p>
              </section>
            </div>
          </>
        ) : null}
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
 * 3. "从已有标签选"列出库里的**全部**标签（手工 + 自动），已在这张素材上的勾着不可点 ——
 *    过滤掉它们会让面板只剩「没有匹配的标签」（owner 2026-11-19 两轮反馈）。
 */
function TagSection({
  manual,
  colors,
  auto,
  all,
  allAuto,
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
  /** 全库自动标签（带使用数）：面板要列全，不能只列这张图推导出来的那几条 */
  allAuto: TagCount[];
  onAdd: (tag: string) => void;
  onRemove: (tag: string) => void;
  onPick: (tag: string) => void;
  onDeny: (tag: string) => void;
  onEdit: (tag: string) => void;
}) {
  const t = useT();
  const [q, setQ] = useState("");
  const [picker, setPicker] = useState(false);
  const [pq, setPq] = useState("");
  const [picked, setPicked] = useState<string[]>([]);

  /**
   * 候选 = 库里**全部**手工标签 + 库里**全部**自动标签。
   *
   * 两条教训（owner 2026-11-19，两轮反馈）：
   * 1. 以前把「已经在这张素材上的」过滤掉了 —— 库里的标签全在这一张上时，面板只剩
   *    「没有匹配的标签」。现在照样列出来、勾着、不可点。
   * 2. 自动标签原来只列**这张图**推导出来的那几条，用户看到的还是"不全"；
   *    现在列全库的（与侧栏「自动标签」同一份口径，带使用数）。
   * 点自动标签 = 把软件推的这条**固定成自己的标签**（名字用显示名，如 PNG）。
   */
  const needle = pq.trim().toLowerCase();
  const hitName = (...names: string[]) => !needle || names.some((n) => n.toLowerCase().includes(needle));
  const manualRows = all.filter((t) => hitName(t));
  /** 这张图**本来就有**的自动标签名：列出来但不能再"固定"一次 */
  const onAssetAuto = new Set(auto.map((a) => (isAutoTag(a.tag) ? autoTagLabel(a.tag) : a.tag)));
  const autoRows = allAuto
    .map((t) => ({ tag: t.name, label: isAutoTag(t.name) ? autoTagLabel(t.name) : t.name, count: t.count }))
    .filter((a) => hitName(a.tag, a.label));
  /** 按钮角标只数「还能加的」：已在素材上的、本来就是自动标签的不算 */
  const addable =
    manualRows.filter((t) => !manual.includes(t)).length +
    autoRows.filter((a) => !manual.includes(a.label) && !onAssetAuto.has(a.label)).length;

  /* 换素材就把草稿收掉：上一条打的半截字不该跟着到下一张 */
  const firstId = manual.join("\u0000");
  useEffect(() => {
    setQ("");
    setPicked([]);
    setPq("");
    setPicker(false);
  }, [firstId]);

  const commit = (v: string) => {
    const tag = v.trim();
    if (!tag) return;
    onAdd(tag);
    setQ("");
  };

  const togglePick = (name: string) => {
    setPicked((cur) => (cur.includes(name) ? cur.filter((x) => x !== name) : [...cur, name]));
  };

  const commitPicked = () => {
    for (const t of picked) onAdd(t);
    setPicked([]);
    setPicker(false);
    setPq("");
  };

  return (
    <div className="tag-editor">
      {manual.map((name) => (
        <span
          key={name}
          className={`tag-chip tag-chip-iconed ${colorClass(colors[name])}`}
          title={t("chrome.tag_chip_tip")}
          onContextMenu={(e) => {
            e.preventDefault();
            onEdit(name);
          }}
        >
          <span className="tag-dot tag-dot-chip" aria-hidden />
          <Icon name="tag" size={10} stroke={1.5} className="tag-manual-icon" />
          <button className="tag-chip-text" onClick={() => onPick(name)}>
            {name}
          </button>
          <button
            className="tag-chip-x"
            title={t("chrome.tag_remove")}
            aria-label={t("chrome.tag_remove_aria", { name })}
            onClick={() => onRemove(name)}
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
          title={t("chrome.auto_tag_tip", { reason: a.reason })}
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
          placeholder={t("chrome.tag_add_placeholder")}
          fieldClassName="tag-field"
          inputClassName="tag-input-field"
          ariaLabel={t("chrome.tag_name_aria")}
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
          title={t("chrome.tag_apply")}
          aria-label={t("chrome.tag_apply")}
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
          {t("chrome.pick_existing")}
          {addable > 0 && <span className="tag-pick-badge tabular">{addable}</span>}
        </button>

      {picker && (
        <div className="tag-picker" role="dialog" aria-label={t("chrome.pick_aria")}>
          <div className="tag-picker-head">
            <input
              autoFocus
              value={pq}
              onChange={(e) => setPq(e.target.value)}
              placeholder={t("chrome.pick_search")}
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
              aria-label={t("chrome.pick_close")}
              onClick={() => {
                setPicker(false);
                setPq("");
              }}
            >
              <Icon name="close" size={11} stroke={2} />
            </button>
          </div>
          <ul className="tag-picker-list">
            {manualRows.length === 0 && autoRows.length === 0 ? (
              <li className="tag-picker-empty">{t("chrome.pick_empty")}</li>
            ) : (
              <>
                {manualRows.map((name) => {
                  const on = manual.includes(name);
                  const pickedNow = picked.includes(name);
                  return (
                    <li key={name}>
                      <button
                        className={`tag-picker-item ${pickedNow ? "on" : ""}`}
                        disabled={on}
                        title={on ? t("chrome.tag_already") : t("chrome.tag_apply_one", { name })}
                        onClick={() => togglePick(name)}
                      >
                        <span className="tag-picker-mark">
                          {(pickedNow || on) && <Check size={11} strokeWidth={2} />}
                        </span>
                        <span className="tag-picker-name">{name}</span>
                      </button>
                    </li>
                  );
                })}
                {autoRows.length > 0 && (
                  <li className="tag-picker-sec">{t("chrome.pick_auto_sec")}</li>
                )}
                {autoRows.map((a) => {
                  const on = manual.includes(a.label) || onAssetAuto.has(a.label);
                  const pickedNow = picked.includes(a.label);
                  return (
                    <li key={`auto-${a.tag}`}>
                      <button
                        className={`tag-picker-item ${pickedNow ? "on" : ""}`}
                        disabled={on}
                        title={
                          on
                            ? t("chrome.auto_already")
                            : t("chrome.auto_apply", { n: a.count, label: a.label })
                        }
                        onClick={() => togglePick(a.label)}
                      >
                        <span className="tag-picker-mark">
                          {(pickedNow || on) && <Check size={11} strokeWidth={2} />}
                        </span>
                        <Sparkles size={10} strokeWidth={1.6} className="tag-auto-spark" />
                        <span className="tag-picker-name">{a.label}</span>
                        <span className="tag-picker-count tabular">{a.count}</span>
                      </button>
                    </li>
                  );
                })}
              </>
            )}
          </ul>
          <div className="tag-picker-foot">
            <button className="tag-picker-add" disabled={picked.length === 0} onClick={commitPicked}>
              {picked.length > 0 ? t("chrome.add_n_tags", { n: picked.length }) : t("chrome.add_tag")}
            </button>
          </div>
        </div>
      )}
      </div>

      {manual.length === 0 && auto.length === 0 && (
        <p className="detail-note">{t("chrome.tags_empty")}</p>
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
function thumbSourceLabel(a: Asset, tr: (k: TKey) => string): string {
  if (a.quality === 2) {
    return EMBEDDED_EXT.has(a.ext) ? tr("chrome.thumb_src_embedded") : tr("chrome.thumb_src_system");
  }
  if (a.quality === 1) return tr("chrome.thumb_src_local");
  return tr("chrome.thumb_src_pending");
}

/* 数量格式化给标题栏状态用（暂未使用，保留导出避免 tree-shake 报错） */
export const fmtCount = fmtNum;

/* memo 边界：顶栏只跟着「搜索词 / 排序 / 视图 / 各筛选项 / 库目录」重渲。
   回调在 App 侧已 `useEvent` 稳定化（含 onColor / onTag / onDir / onRating 那四段
   带"互斥清筛选"逻辑的），否则这里每次都会判 props 变了。 */
export const TopBar = memo(TopBarImpl);
