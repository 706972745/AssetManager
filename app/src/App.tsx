import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { onAppEvent, source } from "./data/source";
import { loadHotkeys } from "./lib/hotkeys";
import {
  fmtNum,
  fmtSize,
  loadPrefs,
  savePrefs,
  type Asset,
  type DirNode,
  type AutoTag,
  type MovePolicy,
  type MoveProgress,
  type MoveResult,
  type Prefs,
  type TagCount,
  type TagGroup,
  type SnapshotRow,
  type CollRow,
  type Query,
  type AutoRule,
  type ScanProgress,
  type ScanResult,
  type SortKey,
  type TrashEntry,
} from "./types";
import { AssetView, THUMB_PX, type ViewMode } from "./components/Views";
import { Detail, TitleBar, TopBar } from "./components/Chrome";
import { HoverHintHost } from "./components/HoverHint";
import { TrashView } from "./components/TrashView";
import { AboutView } from "./components/AboutView";
import { Settings, type SettingsKey } from "./components/Settings";
import { Shortcuts } from "./components/Shortcuts";
import { Tools, type ToolTab } from "./components/Tools";
import { CommandPalette, type Command } from "./components/CommandPalette";
import { Tree } from "./components/Tree";
import { Icon, Toaster, type ToastItem } from "./components/ui";
import { ContextMenu, type CtxItem, type CtxState } from "./components/ContextMenu";
import { TagPrompt } from "./components/TagPrompt";
import { TagDock } from "./components/TagDock";
import { CollDock } from "./components/CollDock";
import { SnapshotDock } from "./components/SnapshotDock";
import { SnapshotPrompt } from "./components/SnapshotPrompt";
import { TagEdit } from "./components/TagEdit";
import { COLOR_LABEL, autoTagLabel, isAutoTag, loadDenied, saveDenied } from "./lib/tags";
import { useDialog } from "./components/Dialog";

const KINDS_OF: Record<string, number[] | null> = {
  all: null,
  image: [0],
  "3d": [3],
  video: [1],
  doc: [4],
};

/** 阈值确认的口径（规格 §3.8）：移动 > 100 个或 > 2GB、删除 > 20 个，都要再问一次 */
const CONFIRM_MOVE_COUNT = 100;
const CONFIRM_MOVE_BYTES = 2 * 1024 ** 3;
const CONFIRM_DELETE_COUNT = 20;
/** 最多几个素材库（与 FontManager 的「多库最多 3 个」同一口径） */
const MAX_LIBS = 3;
/** 批量操作的每片大小：一次丢几千个路径给 Shell，回收站 API 会卡住好几十秒 */
const CHUNK = 200;

let toastSeq = 1;

/** 「把图片加入快照」弹层的四种来源（见 `snapPromptProps`） */
type SnapPromptState =
  | { mode: "tag"; tag: string; count?: number }
  | { mode: "ids"; ids: number[] }
  | { mode: "merge"; srcId: number }
  | { mode: "diff"; srcId: number };

/**
 * 四种入口 → 同一张卡片的文案。
 *
 * 合并 / 差集**不给新建**（名字由两份快照的名字拼出来）：否则用户会以为还得自己起名，
 * 而自己起名这件事在这两个动作里没有语义。
 */
function snapPromptProps(p: SnapPromptState, snaps: SnapshotRow[]) {
  const nameOf = (id: number) => snaps.find((s) => s.id === id)?.name ?? "这份快照";
  if (p.mode === "ids") {
    return {
      title: "把选中的图片加入快照",
      lead: (
        <>
          选中的 <b>{p.ids.length}</b> 张会记进快照：不移动、不复制任何文件，
          只是记下「这些图是哪几张」。
        </>
      ),
      picksHint: "已有的快照（点一下直接加进去）",
      excludeIds: undefined as number[] | undefined,
      canCreate: true,
    };
  }
  if (p.mode === "merge") {
    return {
      title: "与另一份快照合并",
      lead: (
        <>
          把「{nameOf(p.srcId)}」和<b>点的那一份</b>合成一份<b>新快照</b>（同一张图只算一次）。
          两份原件都不动。
        </>
      ),
      picksHint: "选另一份（会新建一份并集）",
      excludeIds: [p.srcId],
      canCreate: false,
    };
  }
  if (p.mode === "diff") {
    return {
      title: "与另一份快照做差集",
      lead: (
        <>
          新建一份 =「{nameOf(p.srcId)}」里<b>去掉</b>另一份也有的图。两份原件都不动。
        </>
      ),
      picksHint: "选要减掉的那一份",
      excludeIds: [p.srcId],
      canCreate: false,
    };
  }
  return {
    title: "把图片加入快照",
    lead: (
      <>
        标签「{p.tag}」下{typeof p.count === "number" ? ` ${p.count} 张图` : "的图"}
        会记进快照：不移动、不复制任何文件，只是记下「这些图是哪几张」。
      </>
    ),
    picksHint: "已有的快照（点一下直接加进去）",
    excludeIds: undefined as number[] | undefined,
    canCreate: true,
  };
}

export default function App() {
  /* 偏好先读出来：默认档位/视图在首屏就要用上，不能等设置面板打开过一次 */
  const [prefs, setPrefs] = useState<Prefs>(() => loadPrefs());
  const [view, setView] = useState<ViewMode>(prefs.view);
  const [sizeIndex, setSizeIndex] = useState(prefs.size);
  /* `?settings=cache|appearance|…` 直接打开设置并定位到某一页。
     只给 QA / 截图用（真机启动 URL 没有 query，所以不影响用户）；
     有了它就不用模拟点击去截设置面板 —— 模拟输入在这台机器上会撞中文输入法。 */
  const bootSection = useMemo(() => {
    const s = new URLSearchParams(location.search).get("settings");
    return s ? (s as SettingsKey) : undefined;
  }, []);
  const [settingsOpen, setSettingsOpen] = useState(() => !!bootSection);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  /** 命令面板（T27）：Ctrl/⌘ + K 唤起 */
  const [cmdkOpen, setCmdkOpen] = useState(false);
  const [sort, setSort] = useState<SortKey>("mtime_desc");
  const [kindKey, setKindKey] = useState("all");
  const [text, setText] = useState("");
  const [debounced, setDebounced] = useState("");
  const [dir, setDir] = useState<DirNode | null>(null);
  const [items, setItems] = useState<Asset[]>([]);
  const [cursor, setCursor] = useState<[number, number] | null>(null);
  const [done, setDone] = useState(false);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<Asset | null>(null);
  /** 正在改名的素材 id（owner 2026-11-17：改名**只**在详情顶栏就地做，不弹窗）。
   *  右键菜单 / 操作区的「重命名」都只是把它设上，然后聚焦到顶栏那个输入框。 */
  const [renameId, setRenameId] = useState<number | null>(null);
  /** 主内容区是哪个页面：素材列表 / 回收站 / 关于（侧栏底栏切换） */
  const [page, setPage] = useState<"assets" | "trash" | "about">("assets");
  const [trash, setTrash] = useState<TrashEntry[]>([]);
  /** 多选集合（T11）：详情面板盯 `selected`，批量操作作用于这个集合 */
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const [busy, setBusy] = useState(false);
  const [batch, setBatch] = useState<{ done: number; total: number; label?: string } | null>(null);
  const [lightbox, setLightbox] = useState<number | null>(null);
  /** 灯箱的换图入口（由 Lightbox 注册，带逐帧闸门）。全局方向键都从这里走 */
  const lightNav = useRef<((dir: number) => void) | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  /* ---------- 标签（2026-11-16）：手动落库、自动运行时推导 ---------- */
  /** 正在按哪个标签筛（null = 不筛）。设了标签就跨目录看，和 FontManager 的标签导航一致 */
  const [tagFilter, setTagFilter] = useState<string | null>(null);
  /* 颜色筛选（T24）：色桶键；colorCounts 是侧栏口径的"每种颜色多少张" */
  const [colorFilter, setColorFilter] = useState<string | null>(null);
  const [colorCounts, setColorCounts] = useState<[string, number][]>([]);
  /** 评分下限（筛选菜单里的「评分」段；1–5 星，null = 不筛） */
  const [ratingFilter, setRatingFilter] = useState<number | null>(null);
  /* 工具面板（重复 / 丢失引用 / 批量重命名） */
  const [tools, setTools] = useState<{ tab: ToolTab } | null>(null);
  const [tags, setTags] = useState<TagCount[]>([]);
  const [tagGroups, setTagGroups] = useState<TagGroup[]>([]);
  /* 分组（0.2.x）：侧栏里紧贴标签面板上方那一段（位置/样式照 FontManager）。
     筛选语义同标签：点了就**跨目录**看这一撮素材。 */
  const [colls, setColls] = useState<CollRow[]>([]);
  const [collFilter, setCollFilter] = useState<number | null>(null);
  /* 快照（0.12.0）：侧栏一块，右上的 + / 右键头部可以新建、导入 */
  const [snaps, setSnaps] = useState<SnapshotRow[]>([]);
  /** 正按哪份快照筛（null = 不筛）。与标签/分组同一口径：点了就跨目录看 */
  const [snapFilter, setSnapFilter] = useState<number | null>(null);
  /* 新建 / 改名都是**侧栏就地输入**（照 FontManager，不弹窗）：状态放这里，
     因为「Ctrl+K → 新建快照」和快照右键「改名」都要能把它打开。 */
  const [snapCreating, setSnapCreating] = useState(false);
  const [snapRenaming, setSnapRenaming] = useState<{ id: number; name: string } | null>(null);
  /**
   * 「把图片加入快照」那一层弹层的状态（要选目标快照，弹层比就地输入合适）。
   *
   * 四种来源共用同一张卡片（0.1.18 起）：
   * - `tag`   标签右键 → 把这个标签下的图加进去
   * - `ids`   选中素材 → 加入快照（批量条 / 右键菜单）
   * - `merge` 快照右键 → 并入另一份快照（并集）
   * - `diff`  快照右键 → 与另一份做差集（新建）
   */
  const [snapPrompt, setSnapPrompt] = useState<SnapPromptState | null>(null);
  /** 正在编辑的标签/分组（null = 没开编辑卡） */
  const [tagEdit, setTagEdit] = useState<{ kind: "tag" | "group"; name: string } | null>(null);
  /** 正在编辑的标签的自动规则（T34）：null = 没有规则 */
  const [tagRule, setTagRuleState] = useState<AutoRule | null>(null);
  const [autoTagCounts, setAutoTagCounts] = useState<TagCount[]>([]);
  /** 当前页素材的手工标签：id → 标签数组（一次 IPC 拿一页，不逐行查） */
  const [tagMap, setTagMap] = useState<Map<number, string[]>>(new Map());
  /** 批量打标签弹窗的目标（null = 关着） */
  const [tagPrompt, setTagPrompt] = useState<{ ids: number[] } | null>(null);
  /** 自动标签的否决名单（本地偏好） */
  const [denied, setDenied] = useState<Set<string>>(() => loadDenied());
  /** 选中素材的自动标签（后端推导 + 手工优先） */
  const [selectedAuto, setSelectedAuto] = useState<AutoTag[]>([]);
  const [ctx, setCtx] = useState<CtxState | null>(null);
  const [scan, setScan] = useState<{ active: boolean; dirs: number; files: number; current: string }>({
    active: false,
    dirs: 0,
    files: 0,
    current: "",
  });

  const reqRef = useRef(0);
  const dirRef = useRef<DirNode | null>(null);
  dirRef.current = dir;
  const dragPaths = useRef<string[] | null>(null);
  const dragPreview = useRef<number | null>(null);
  const dlg = useDialog();
  const askRef = useRef(dlg.ask);
  askRef.current = dlg.ask;
  /** 上一次点击的锚点（Shift 连滑要用）：id + 当时的完整列表快照 */
  const lastPick = useRef<number | null>(null);
  const itemsRef = useRef<Asset[]>([]);
  itemsRef.current = items;

  /* 提示条：内容镜像一份在 ref 里 —— 合并判断必须**同步**拿到"最后一条是谁"，
     放进 setState 的 updater 里读不到（updater 是渲染阶段才跑的，StrictMode 还会跑两遍）。 */
  const toastsRef = useRef<ToastItem[]>([]);
  const toastTimers = useRef(new Map<number, number>());

  const toast = useCallback((title: string, err = false, detail?: string) => {
    const commit = (list: ToastItem[]) => {
      toastsRef.current = list;
      setToasts(list);
    };
    /* 到期自动收（合并时要先 clear 再重设，否则第 3 次点击那条会突然整块消失） */
    const arm = (id: number) => {
      window.clearTimeout(toastTimers.current.get(id));
      toastTimers.current.set(
        id,
        window.setTimeout(() => {
          toastTimers.current.delete(id);
          commit(toastsRef.current.filter((x) => x.id !== id));
        }, 3600)
      );
    };

    /* 连续同一条提示合并成一条 + `×N` 角标（FontManager 同一套）：
       连点三次「复制文件名」不该在屏幕右边堆三块，看的人只想知道"复制了三次"。
       只跟**最后一条**比，不全表扫描 —— A→B→A 应该是三条，合成一条会让人以为中间那次没发生。 */
    const list = toastsRef.current;
    const last = list[list.length - 1];
    if (
      last &&
      last.title === title &&
      (last.detail ?? "") === (detail ?? "") &&
      !!last.error === !!err
    ) {
      commit(list.map((x) => (x.id === last.id ? { ...x, count: x.count + 1 } : x)));
      arm(last.id);
      return;
    }

    const id = toastSeq++;
    const next = [...list, { id, title, detail, error: err, count: 1 }];
    /* 超过 4 条从最旧那条收，顺手清掉它的计时器（不清就是泄漏） */
    for (const gone of next.slice(0, Math.max(0, next.length - 4))) {
      window.clearTimeout(toastTimers.current.get(gone.id));
      toastTimers.current.delete(gone.id);
    }
    commit(next.slice(-4));
    arm(id);
  }, []);

  useEffect(() => {
    let alive = true;
    void source.roots().then((rs) => {
      /* 只有在「没有任何跨目录筛选」时才自动落到第一个库。
         没有这半个条件就是一个真 bug（owner 2026-11-17 报「标签明明有、点了不出图」）：
         点标签会把目录清空（= 跨目录看这个标签），紧接着这里又按"当前没选目录"
         把第一个库选回来 —— 查询立刻变成「这个库 AND 这个标签」，
         而那个标签的 400 张图在**另一个库**里 → 永远空列表。
         截图当时的状态就是：筛选条写着标签、侧栏第一个库高亮、网格空白。 */
      if (!alive || dirRef.current || !rs[0]) return;
      if (tagFilter !== null || collFilter !== null || snapFilter !== null) return;
      setDir(rs[0]);
    });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey, tagFilter, collFilter, snapFilter]);

  /** 设置面板里的任何改动都走这里：落盘 + 立刻把默认档位与视图套到当前界面 */
  const applyPrefs = useCallback((p: Prefs) => {
    savePrefs(p);
    setPrefs(p);
    setSizeIndex(p.size);
    setView(p.view);
  }, []);

  /* 主题与动效落到 <html> 上（与 FontManager 同一套开关位置）：
     · data-theme="light" → 浅色；不设 / "dark" → 深色（tokens.css 的 :root 就是深色）
     · data-motion="reduced" → 全站时长令牌压到 1ms，惯性滚动也一起关（momentumScroll 读的就是这个属性）
     首次挂载不加过渡，否则启动瞬间能看到一次「从默认深色淡到浅色」。 */
  const themeMounted = useRef(false);
  useEffect(() => {
    const el = document.documentElement;
    if (themeMounted.current) {
      el.classList.add("theme-switching");
      window.setTimeout(() => el.classList.remove("theme-switching"), 300);
    }
    themeMounted.current = true;
    /* `?theme=light|dark` / `?motion=reduced` 是给 QA 与截图用的临时覆盖：
       不改偏好、不落盘，只影响本次页面。临时看另一个主题别再改 DEFAULT_PREFS
       （改完忘了改回来，出厂默认就错了）。 */
    const q = new URLSearchParams(location.search);
    const forcedTheme = q.get("theme");
    const forcedMotion = q.get("motion");
    el.dataset.theme = forcedTheme === "light" || forcedTheme === "dark" ? forcedTheme : prefs.theme;
    el.dataset.motion = forcedMotion === "reduced" ? "reduced" : prefs.motion;
  }, [prefs.theme, prefs.motion]);

  /* 后台预热全库缩略图：启动 6 秒后开始（避开首屏那一波请求）。
     为什么要它：预取只做「下一屏」，第一次打开很久没看的目录仍然要现做图；
     全库跑一遍之后，滑到任何位置都只是读文件。开关在「设置 → 缓存与索引」。 */
  const warmedRef = useRef(false);
  useEffect(() => {
    if (!source.desktop || !prefs.warmThumbs || warmedRef.current) return;
    warmedRef.current = true;
    const t = window.setTimeout(() => {
      void source.warmThumbs(THUMB_PX[prefs.size] ?? 384).catch(() => {});
    }, 6000);
    return () => window.clearTimeout(t);
  }, [prefs.warmThumbs, prefs.size]);

  // 搜索防抖：打字时不打数据库
  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(text.trim()), 160);
    return () => window.clearTimeout(t);
  }, [text]);

  const buildQuery = useCallback(
    (c: [number, number] | null): Query => ({
      dirId: dir?.id ?? null,
      recursive: true,
      kinds: KINDS_OF[kindKey] ?? null,
      text: debounced,
      sort,
      cursor: c,
      limit: 200,
      tag: tagFilter,
      color: colorFilter,
      minRating: ratingFilter,
      collection: collFilter,
      snapshot: snapFilter,
    }),
    [dir?.id, kindKey, debounced, sort, tagFilter, colorFilter, ratingFilter, collFilter, snapFilter]
  );

  const loadPage = useCallback(
    async (reset: boolean) => {
      /* 「没有当前目录就不查」只对**浏览目录**成立。
         标签 / 分组 / 颜色 / 评分 这些是**跨目录**筛选：点标签时目录被清空（这是对的），
         但如果这里仍然短路，就永远不会去查 —— 表现正是 owner 报的
         「标签明明有 400 张，点了以后网格空白」（2026-11-17，P0）。
         判定"有没有筛选"要看这几个维度，不能只看 dir。 */
      const scoped =
        tagFilter !== null ||
        collFilter !== null ||
        snapFilter !== null ||
        colorFilter !== null ||
        ratingFilter !== null ||
        !!debounced;
      if (reset && !dir && !scoped) {
        setItems([]);
        setDone(true);
        return;
      }
      const my = ++reqRef.current;
      setLoading(true);
      try {
        const rows = await source.list(buildQuery(reset ? null : cursor));
        if (my !== reqRef.current) return; // 丢弃过期响应
        setItems((prev) => (reset ? rows : [...prev, ...rows]));
        setDone(rows.length < 200);
        if (rows.length) {
          const last = rows[rows.length - 1];
          setCursor(
            sort === "name_asc"
              ? // [已取条数, 最后一行 id]：Rust 侧用 id 反查 name 做 (name,id) keyset，
                // 第一维只在极少数「那一行刚被删」的情况下当 OFFSET 兜底
                [(reset ? 0 : items.length) + rows.length, last.id]
              : sort === "size_desc"
                ? [last.size, last.id]
                : sort === "rating_desc"
                  ? [last.rating, last.id]
                  : [last.mtime, last.id]
          );
        }
        // 详情面板常驻：没选中任何东西时，默认盯住第一张，别让右半边空着
        if (reset) {
          setSelected((cur) => {
            const keep = cur && rows.some((r) => r.id === cur.id) ? cur : rows[0] ?? null;
            setSelectedIds(keep ? new Set([keep.id]) : new Set());
            lastPick.current = keep?.id ?? null;
            return keep;
          });
        }
      } catch (e) {
        toast(String(e), true);
      } finally {
        if (my === reqRef.current) setLoading(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [buildQuery, cursor, dir, sort, toast, tagFilter, collFilter, colorFilter, ratingFilter, snapFilter, debounced]
  );

  // 查询条件变化 → 重新拉第一页
  useEffect(() => {
    setCursor(null);
    setDone(false);
    void loadPage(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir?.id, kindKey, debounced, sort, refreshKey, tagFilter, colorFilter, ratingFilter, collFilter, snapFilter]);

  /* 颜色计数（顶栏色板菜单用）：只在"库里可能变了"的时候拉，不跟着滚动刷新 */
  useEffect(() => {
    let dead = false;
    void source
      .colorCounts()
      .then((rows) => {
        if (!dead) setColorCounts(rows);
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [refreshKey, source]);

  const loadMore = useCallback(() => {
    if (loading || done) return;
    void loadPage(false);
  }, [loading, done, loadPage]);

  const thumbSrc = useCallback((a: Asset, px: number) => source.thumb(a, px), []);

  /* 预取：后台把「下一屏」先做掉。失败不影响显示，所以一律吞掉错误 ——
     真要查问题看缩略图本身（详情面板的「来源」会说明它为什么没出来）。 */
  const onPrefetchThumbs = useCallback((ids: number[], px: number) => {
    void source.prefetchThumbs(ids, px).catch(() => {});
  }, []);

  /* ---------------- 选择（T11）：单击 / Ctrl 加减 / Shift 连滑 / 框选 ---------------- */
  const pickOne = useCallback((a: Asset, mods: { ctrl: boolean; shift: boolean }) => {
    setSelected(a);
    if (mods.shift && lastPick.current !== null) {
      // Shift：从上一个锚点滑到这里（按当前列表顺序取区间，两端都含）
      const list = itemsRef.current;
      const from = list.findIndex((x) => x.id === lastPick.current);
      const to = list.findIndex((x) => x.id === a.id);
      if (from >= 0 && to >= 0) {
        const [s, e] = from <= to ? [from, to] : [to, from];
        setSelectedIds(new Set(list.slice(s, e + 1).map((x) => x.id)));
        return;
      }
    }
    if (mods.ctrl) {
      setSelectedIds((cur) => {
        const n = new Set(cur);
        if (n.has(a.id)) n.delete(a.id);
        else n.add(a.id);
        // 取消到空的时候详情不能跟着空 —— 保留这一张作为"主选中"
        return n.size ? n : new Set([a.id]);
      });
      lastPick.current = a.id;
      return;
    }
    setSelectedIds(new Set([a.id]));
    lastPick.current = a.id;
  }, []);

  const pickMany = useCallback((ids: number[], additive: boolean) => {
    if (!ids.length) {
      if (!additive) {
        setSelectedIds(new Set());
      }
      return;
    }
    setSelectedIds((cur) => (additive ? new Set([...cur, ...ids]) : new Set(ids)));
    const first = itemsRef.current.find((x) => x.id === ids[0]);
    if (first) setSelected(first);
    lastPick.current = ids[ids.length - 1] ?? null;
  }, []);

  const clearSel = useCallback(() => {
    setSelectedIds(selected ? new Set([selected.id]) : new Set());
  }, [selected]);

  const selPaths = useMemo(
    () => items.filter((a) => selectedIds.has(a.id)).map((a) => a.path),
    [items, selectedIds]
  );

  /* ---------------- 扫描（T03）：后台跑 + 进度 + 取消 ---------------- */
  const startScan = useCallback(
    async (path: string, label = "开始扫描") => {
      try {
        await source.scanStart(path);
        setScan({ active: true, dirs: 0, files: 0, current: path });
        toast(label, false, "正在后台建立索引，可以继续浏览；随时能取消");
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const cancelScan = useCallback(async () => {
    await source.scanCancel().catch(() => false);
    toast("已请求取消", false, "已经扫进来的部分会保留");
  }, [toast]);

  /* 进度事件：只在扫描期间订阅，避免无谓的 JSON 解析 */
  useEffect(() => {
    const offP = onAppEvent<ScanProgress>("scan://progress", (p) => {
      if (p.done) return;
      setScan({ active: true, dirs: p.dirs, files: p.files, current: p.current });
    });
    const offD = onAppEvent<ScanResult>("scan://done", (r) => {
      setScan({ active: false, dirs: 0, files: 0, current: "" });
      toast(r.ok ? "扫描完成" : "扫描没跑完", !r.ok, r.message);
      setRefreshKey((k) => k + 1);
    });
    return () => {
      offP();
      offD();
    };
  }, [toast]);

  /* ---------------- 外部变更跟随（T06）：只盯当前目录 ---------------- */
  useEffect(() => {
    if (!dir?.path) return;
    void source.watch([dir.path]).catch(() => 0);
  }, [dir?.path]);

  /* 截图保存后刷新（截图目录可能已自动进库，列表要立刻看到这张新图） */
  useEffect(() => {
    const offSaved = onAppEvent<string | null>("shot://saved", (p) => {
      setRefreshKey((k) => k + 1);
      if (p) toast(`截图已保存：${p.split(/[\\/]/).pop() ?? p}`, false, "Ctrl+S 可以另选位置");
    });
    /* 复制这条要说一句：截图窗一关，用户看不到剪贴板里有没有东西 */
    const offCopied = onAppEvent<null>("shot://copied", () => {
      toast("截图已复制到剪贴板", false, "直接 Ctrl+V 就能贴");
    });
    return () => {
      offSaved();
      offCopied();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* 系统级快捷键（F1 截图 / F3 贴剪贴板）。
     启动时按用户存的组合注册一次；设置里改完由设置面板自己再调一次。 */
  useEffect(() => {
    const saved = loadHotkeys();
    const list: [string, string][] = [
      [saved.snip, "snip"],
      [saved.clip, "clip_peek"],
    ];
    void source
      .hotkeysSet(list)
      .then((failed) => {
        if (failed.length) toast(`这些快捷键被别的软件占了：${failed.join("、")}`, true);
      })
      .catch(() => {});
  }, []); // 只注册一次；改动走设置面板

  useEffect(() => {
    let timer: number | null = null;
    const off = onAppEvent<string[]>("fs://changed", () => {
      // 连发去抖：一次保存可能产生 3~5 个事件
      if (timer) window.clearTimeout(timer);
      timer = window.setTimeout(() => setRefreshKey((k) => k + 1), 400);
    });
    return () => {
      off();
      if (timer) window.clearTimeout(timer);
    };
  }, []);

  /* 后台移动的进度与收尾（缺陷 #7）：跨盘走「复制 → 删源」，几百 MB 要十几秒，
     以前同步执行时界面只能干等。现在 Rust 在独立线程里跑，这里只接事件。 */
  useEffect(() => {
    const offP = onAppEvent<MoveProgress>("move://progress", (p) => {
      setBatch({ done: p.done, total: p.total, label: `正在移动 · ${p.current}` });
    });
    const offD = onAppEvent<MoveResult>("move://done", (r) => {
      setBatch(null);
      toast(r.ok ? "已移动到目标文件夹" : "没有移动任何文件", !r.ok, r.message);
      setRefreshKey((k) => k + 1);
    });
    return () => {
      offP();
      offD();
    };
  }, [toast]);

  /* ---------------- 标签：清单 / 当前页标签 / 单张自动标签 ---------------- */

  // 侧栏要的两份清单（手工 + 自动），随刷新更新
  useEffect(() => {
    void source.tags().then(setTags).catch(() => {});
    void source.tagGroups().then(setTagGroups).catch(() => {});
    void source.autoTags().then(setAutoTagCounts).catch(() => {});
    void source.snapshotList().then(setSnaps).catch(() => {});
    void source.collList().then(setColls).catch(() => {});
  }, [refreshKey, tagPrompt, tagEdit, snapPrompt]);

  // 当前页的手工标签（详情面板 + 后面的批量操作都要用）
  useEffect(() => {
    const ids = items.map((a) => a.id);
    if (!ids.length) {
      setTagMap(new Map());
      return;
    }
    // 分片取：翻到几千项时一条 SQL 的占位符会顶到 SQLite 变量上限
    const chunks: number[][] = [];
    for (let i = 0; i < ids.length; i += 400) chunks.push(ids.slice(i, i + 400));
    void Promise.all(chunks.map((c) => source.assetTags(c).catch(() => [] as [number, string[]][])))
      .then((parts) => setTagMap(new Map(parts.flat())))
      .catch(() => {});
  }, [items, refreshKey]);

  // 选中素材的自动标签：推导在 Rust（唯一一份规则），这里只负责按否决名单隐藏
  useEffect(() => {
    const a = selected;
    if (!a) {
      setSelectedAuto([]);
      return;
    }
    const manual = tagMap.get(a.id) ?? [];
    void source
      .autoTagsFor(a, manual)
      .then(setSelectedAuto)
      .catch(() => setSelectedAuto([]));
  }, [selected, tagMap]);

  const applyTag = useCallback(
    async (tag: string, ids: number[]) => {
      if (!ids.length) return;
      try {
        const added = await source.tagAssets(ids, tag);
        toast(
          `已给 ${ids.length} 项打上「${tag}」`,
          false,
          added < ids.length ? `${ids.length - added} 项之前就有这个标签` : undefined
        );
        setRefreshKey((k) => k + 1);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const removeTag = useCallback(
    async (tag: string, ids: number[]) => {
      if (!ids.length) return;
      try {
        await source.untagAssets(ids, tag);
        setRefreshKey((k) => k + 1);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const deleteTagEverywhere = useCallback(
    async (tag: string) => {
      const v = await askRef.current({
        title: `从所有素材上移除「${tag}」？`,
        body: <p className="dlg-hint">只是不再有这个标签，素材本身和磁盘上的文件都不动。</p>,
        buttons: [
          { label: "移除标签", value: "go", tone: "danger" },
          { label: "取消", value: "no" },
        ],
      });
      if (v !== "go") return;
      try {
        const n = await source.deleteTag(tag);
        if (tagFilter === tag) setTagFilter(null);
        toast(`已移除「${tag}」`, false, `涉及 ${n} 项素材`);
        setRefreshKey((k) => k + 1);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [tagFilter, toast]
  );

  const denyAutoTag = useCallback(
    (tag: string) => {
      const next = new Set(denied);
      next.add(tag);
      setDenied(next);
      saveDenied(next);
      toast(`不再建议「${isAutoTag(tag) ? autoTagLabel(tag) : tag}」`, false, "改回来：设置 → 常用 → 恢复自动标签建议");
    },
    [denied, toast]
  );

  /* ---------------- 标签自己的右键菜单 ---------------- */

  const tagMenu = useCallback(
    (tag: string, x: number, y: number) => {
      const on = tagFilter === tag;
      const label = isAutoTag(tag) ? autoTagLabel(tag) : tag;
      setCtx({
        x,
        y,
        items: [
          {
            label: "打开灵感板（无限画布）",
            icon: "expand",
            action: () =>
              void source.boardOpen(tag).catch((e) => toast(String(e))),
          },
          {
            /* 快照：把这一整个标签下的图都记进快照（文件一个都不动） */
            label: "把标签下的图片加入快照…",
            icon: "copy",
            action: () =>
              setSnapPrompt({
                mode: "tag",
                tag,
                count:
                  tags.find((t) => t.name === tag)?.count ??
                  autoTagCounts.find((t) => t.name === tag)?.count,
              }),
          },
          { kind: "separator" },
          {
            label: on ? "取消这个筛选" : `只看「${label}」`,
            icon: "funnel",
            action: () => {
              setTagFilter(on ? null : tag);
              if (!on) {
                setDir(null);
                setSnapFilter(null);
              }
            },
          },
          { kind: "separator" },
          {
            label: isAutoTag(tag) ? "这个标签是自动生成的（只读）" : "编辑标签（颜色 / 分组 / 改名）",
            icon: "tag",
            disabled: isAutoTag(tag),
            action: () => setTagEdit({ kind: "tag", name: tag }),
          },
          {
            label: "从所有素材上移除",
            icon: "trash",
            danger: true,
            disabled: isAutoTag(tag),
            action: () => void deleteTagEverywhere(tag),
          },
        ],
      });
    },
    [tagFilter, deleteTagEverywhere, toast]
  );

  /* ---------------- 快照（0.12.0） ----------------
     快照记的是图片的**内容指纹**，所以改名、换目录、换库之后导回来照样对得上。
     界面上只有三件事：打开灵感板、对账、导出（清单 / 原图 zip）。 */

  const openSnapshot = useCallback(
    (row: SnapshotRow) => {
      void source.snapshotOpen(row.id).catch((e) => toast(String(e), true));
    },
    [toast]
  );

  const resolveSnapshot = useCallback(
    async (row: SnapshotRow) => {
      try {
        const r = await source.snapshotResolve(row.id);
        if (r.missing === 0) {
          toast(
            `「${row.name}」对上了：${r.found} 张全在`,
            false,
            "按图片内容匹配 —— 改过名字、换过库的也算同一张"
          );
        } else {
          toast(
            `「${row.name}」对上 ${r.found} 张，${r.missing} 张这次没找到`,
            true,
            "条目不会被删，硬盘插回来再点一次对账就行"
          );
        }
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const exportSnapshot = useCallback(
    async (row: SnapshotRow) => {
      try {
        const p = await source.snapshotExport(row.id);
        if (p) toast("快照已导出", false, p);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const exportSnapshotZip = useCallback(
    async (row: SnapshotRow) => {
      try {
        toast(`正在打包「${row.name}」的原图…`, false, "图多、图大的话要等一会儿");
        const r = await source.snapshotExportZip(row.id);
        if (!r) return;
        const notes: string[] = [];
        if (r.bad.length) notes.push(`${r.bad.length} 个失败`);
        if (r.missing) notes.push(`${r.missing} 张这次没找到，没进包`);
        const file = r.path.split(/[\\/]/).pop() ?? r.path;
        toast(`已导出 ${r.ok} 张 → ${file}`, notes.length > 0, notes.join(" · ") || undefined);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const importSnapshot = useCallback(async () => {
    try {
      const r = await source.snapshotImport();
      if (!r) return;
      const [, name, added, skipped] = r;
      setRefreshKey((k) => k + 1);
      toast(
        `已导入快照「${name}」：${added} 张`,
        skipped > 0,
        skipped ? `${skipped} 条坏数据已跳过` : undefined
      );
    } catch (e) {
      toast(String(e), true);
    }
  }, [toast]);

  const deleteSnapshot = useCallback(
    async (row: SnapshotRow) => {
      const v = await dlg.ask({
        title: `删除快照「${row.name}」？`,
        body: (
          <p className="dlg-hint">
            只删这份清单（{row.count} 条）。磁盘上的图片、素材库里的素材、标签都不动。
          </p>
        ),
        buttons: [
          { label: "删除快照", value: "go", tone: "danger" },
          { label: "取消", value: "no" },
        ],
      });
      if (v !== "go") return;
      try {
        await source.snapshotDelete(row.id);
        if (snapFilter === row.id) setSnapFilter(null);
        setRefreshKey((k) => k + 1);
        toast(`已删除快照「${row.name}」`);
      } catch (e) {
        toast(String(e), true);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [toast, snapFilter]
  );

  const addTagToSnapshot = useCallback(
    async (id: number, tag: string) => {
      try {
        const r = await source.snapshotAddTag(id, tag);
        setSnapPrompt(null);
        setRefreshKey((k) => k + 1);
        const notes: string[] = [];
        if (r.already) notes.push(`另外 ${r.already} 张本来就在`);
        if (r.failed.length) notes.push(`${r.failed.length} 张失败`);
        toast(`已加入 ${r.added} 张`, r.failed.length > 0, notes.join(" · ") || undefined);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  /* ---------------- 快照的「加入 / 合并 / 差集」（0.1.18，P2.9） ----------------
     三个动作都只改**清单**：一件素材、一个文件都不搬。 */

  /** 把一批选中的素材加进某份快照（弹层里点已有快照，或写新名字新建） */
  const addIdsToSnapshot = useCallback(
    async (id: number, ids: number[]) => {
      try {
        const r = await source.snapshotAdd(id, ids);
        setSnapPrompt(null);
        setRefreshKey((k) => k + 1);
        const notes: string[] = [];
        if (r.already) notes.push(`另外 ${r.already} 张本来就在`);
        if (r.failed.length) notes.push(`${r.failed.length} 张失败`);
        toast(`已加入 ${r.added} 张`, r.failed.length > 0, notes.join(" · ") || undefined);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  /** 合并（并集）/ 差集（第一份减第二份）：结果是一份**新快照**，两份原件都不动 */
  const combineSnapshots = useCallback(
    async (srcId: number, otherId: number, mode: "union" | "diff") => {
      const a = snaps.find((s) => s.id === srcId);
      const b = snaps.find((s) => s.id === otherId);
      if (!a || !b) return;
      setSnapPrompt(null);
      const name = mode === "diff" ? `${a.name} − ${b.name}` : `${a.name} ∪ ${b.name}`;
      try {
        const [, total, added] = await source.snapshotCombine([srcId, otherId], name, mode);
        setRefreshKey((k) => k + 1);
        toast(
          mode === "diff"
            ? `已新建「${name}」：${total} 张`
            : `已新建「${name}」：${total} 张（其中 ${added} 条新记）`,
          false,
          "原快照都没动 —— 只是照着算了一份新的"
        );
      } catch (e) {
        toast(String(e), true);
      }
    },
    [snaps, toast]
  );

  /* ---------------- 快照的新建 / 改名：都是侧栏里的一行输入（照 FontManager） ----------------
     空名字 = 用户什么都没写 → 只退出编辑态，不报错（与"添加素材库"一致）。 */

  const createSnapshotInline = useCallback(
    async (name: string) => {
      setSnapCreating(false);
      const n = name.trim();
      if (!n) return;
      try {
        await source.snapshotCreate(n);
        setRefreshKey((k) => k + 1);
        toast(
          `快照「${n}」已建立`,
          false,
          "右键任意标签 →「把标签下的图片加入快照」就能往里放图"
        );
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const renameSnapshotInline = useCallback(
    async (id: number, name: string) => {
      const n = name.trim();
      const cur = snaps.find((s) => s.id === id)?.name;
      setSnapRenaming(null);
      if (!n || n === cur) return;
      try {
        await source.snapshotRename(id, n);
        setRefreshKey((k) => k + 1);
        toast(`已改名为「${n}」`);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [snaps, toast]
  );

  /* ---------------- 分组（0.2.x） ----------------
     位置与样式照 FontManager：侧栏底栏上方、标签面板之上；新建和改名都是**就地输入**。
     分组只记「哪几张」（id 名单），不搬文件、不改素材 —— 删分组只删名单。 */

  const createCollInline = useCallback(
    async (name: string) => {
      const n = name.trim();
      if (!n) return;
      try {
        const id = await source.collCreate(n);
        setColls(await source.collList());
        setCollFilter(id);
        setDir(null);
        setTagFilter(null);
        setPage("assets");
        toast(`分组「${n}」已建立`, false, "选中素材右键 →「加入分组」就能往里放");
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const renameCollInline = useCallback(
    async (id: number, name: string) => {
      const n = name.trim();
      if (!n) return;
      try {
        await source.collRename(id, n);
        setColls(await source.collList());
        toast(`已改名为「${n}」`);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [toast]
  );

  const deleteColl = useCallback(
    async (row: CollRow) => {
      const v = await askRef.current({
        title: `删除分组「${row.name}」？`,
        body: (
          <p className="dlg-hint">
            只删这份名单（{row.count} 项）。素材本身和磁盘上的文件一个都不动。
          </p>
        ),
        buttons: [
          { label: "删除分组", value: "go", tone: "danger" },
          { label: "取消", value: "no" },
        ],
      });
      if (v !== "go") return;
      try {
        await source.collDelete(row.id);
        if (collFilter === row.id) setCollFilter(null);
        setColls(await source.collList());
        toast(`已删除分组「${row.name}」`);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [collFilter, toast]
  );

  const collMenu = useCallback(
    (row: CollRow, x: number, y: number) => {
      setCtx({
        x,
        y,
        items: [
          {
            label: `只看「${row.name}」`,
            icon: "funnel",
            action: () => {
              setCollFilter(collFilter === row.id ? null : row.id);
              setDir(null);
              setTagFilter(null);
              setSnapFilter(null);
              setPage("assets");
            },
          },
          { kind: "heading", label: "双击这一行可以改名" },
          { kind: "separator" },          { label: "删除分组（不删素材）", icon: "trash", danger: true, action: () => void deleteColl(row) },
        ],
      });
    },
    [collFilter, deleteColl]
  );

  /** 把一批素材加进某个分组（返回真正新增的条数，用于 toast） */
  const addToColl = useCallback(
    async (collId: number, ids: number[]) => {
      try {
        const n = await source.collAdd(collId, ids);
        const name = colls.find((c) => c.id === collId)?.name ?? "分组";
        setColls(await source.collList());
        toast(
          n > 0 ? `已加入「${name}」${n} 项` : `这些已经在「${name}」里了`,
          false,
          n > 0 ? undefined : "重复加入不会产生两份"
        );
      } catch (e) {
        toast(String(e), true);
      }
    },
    [colls, toast]
  );

  /** 从某个分组里移除一批素材（只断开关联，文件一个不动）。
      入口：正按这个分组筛选时的素材右键菜单 —— 以前只有后端 `am_coll_remove`，
      前端没有调用点，用户进了分组就出不去（测试文档 §9.2 记的已知缺口）。 */
  const removeFromColl = useCallback(
    async (collId: number, ids: number[]) => {
      try {
        const n = await source.collRemove(collId, ids);
        const name = colls.find((c) => c.id === collId)?.name ?? "分组";
        setColls(await source.collList());
        setRefreshKey((k) => k + 1);
        toast(
          n > 0 ? `已从「${name}」移除 ${n} 项` : `这些本来就不在「${name}」里`,
          false,
          "只断开关联，原文件不动"
        );
      } catch (e) {
        toast(String(e), true);
      }
    },
    [colls, toast]
  );

  /** 「加入分组…」的第二级菜单（右键 / 批量条共用） */
  const collPickMenu = useCallback(
    (ids: number[], x: number, y: number) => {
      if (!colls.length) {
        setCtx({
          x,
          y,
          items: [
            { kind: "heading", label: "还没有分组" },
            {
              label: "在侧栏「分组」里点 + 新建",
              icon: "plus",
              action: () => setCtx(null),
            },
          ],
        });
        return;
      }
      setCtx({
        x,
        y,
        items: [
          { kind: "heading", label: `加入分组（${ids.length} 项）` },
          ...colls.map((c) => ({
            label: c.name,
            icon: "folder-open",
            action: () => void addToColl(c.id, ids),
          })),
        ],
      });
    },
    [colls, addToColl]
  );

  /** 快照那层弹窗里"写个新名字直接建好并加入"（四种入口共用） */
  const createAndTagSnapshot = useCallback(
    async (name: string) => {
      const p = snapPrompt;
      if (!p) return;
      try {
        if (p.mode === "merge" || p.mode === "diff") {
          /* 合并/差集不需要新名字 —— 「新建并加入」在这里没有意义，直接忽略 */
          setSnapPrompt(null);
          return;
        }
        const id = await source.snapshotCreate(name);
        setSnapPrompt(null);
        setRefreshKey((k) => k + 1);
        if (p.mode === "tag") await addTagToSnapshot(id, p.tag);
        else await addIdsToSnapshot(id, p.ids);
      } catch (e) {
        toast(String(e), true);
      }
    },
    [snapPrompt, toast, addTagToSnapshot, addIdsToSnapshot]
  );

  const snapshotMenu = useCallback(
    (id: number, x: number, y: number) => {
      const row = snaps.find((s) => s.id === id);
      if (!row) return;
      const on = snapFilter === id;
      const others = snaps.filter((s) => s.id !== id);
      const selCount = selectedIds.size;
      setCtx({
        x,
        y,
        items: [
          {
            label: on ? "取消「只看这份快照」" : `只看「${row.name}」（在主列表里筛）`,
            icon: "funnel",
            action: () => {
              setSnapFilter(on ? null : id);
              if (!on) {
                /* 与标签/分组一致：跨目录看这一批，目录选择要清掉，
                   否则会变成"这个目录 AND 这份快照"，而快照里的图可能在别的目录 */
                setDir(null);
                setTagFilter(null);
                setCollFilter(null);
                setPage("assets");
              }
            },
          },
          {
            label: `把选中的 ${selCount} 张加进来`,
            icon: "plus",
            disabled: selCount === 0,
            action: () => void addIdsToSnapshot(id, [...selectedIds]),
          },
          { kind: "separator" },
          { label: "打开灵感板（无限画布）", icon: "expand", action: () => openSnapshot(row) },
          {
            label: "重新对账（按内容找回来）",
            icon: "search",
            action: () => void resolveSnapshot(row),
          },
          { kind: "separator" },
          { kind: "heading", label: "和另一份快照…" },
          ...(others.length
            ? [
                {
                  label: "合并（并集，新建一份）…",
                  icon: "copy",
                  action: () => setSnapPrompt({ mode: "merge", srcId: id }),
                },
                {
                  label: "做差集（这份减另一份，新建）…",
                  icon: "funnel",
                  action: () => setSnapPrompt({ mode: "diff", srcId: id }),
                },
              ]
            : [{ kind: "heading" as const, label: "只有一份快照，凑不齐" }]),
          { kind: "separator" },
          {
            label: "导出快照（.json，只含编号）",
            icon: "file",
            action: () => void exportSnapshot(row),
          },
          {
            label: "导出原始图片（.zip）",
            icon: "file",
            action: () => void exportSnapshotZip(row),
          },
          { kind: "separator" },
          {
            label: "改名",
            icon: "tag",
            action: () => setSnapRenaming({ id, name: row.name }),
          },
          {
            label: "删除快照（不删文件）",
            icon: "trash",
            danger: true,
            action: () => void deleteSnapshot(row),
          },
        ],
      });
    },
    [
      snaps,
      openSnapshot,
      resolveSnapshot,
      exportSnapshot,
      exportSnapshotZip,
      deleteSnapshot,
      snapFilter,
      selectedIds,
      addIdsToSnapshot,
    ]
  );


  const groupMenu = useCallback((name: string, x: number, y: number) => {
    setCtx({
      x,
      y,
      items: [
        { label: "编辑分组（颜色 / 改名 / 解散）", icon: "tag", action: () => setTagEdit({ kind: "group", name }) },
      ],
    });
  }, []);

  /* ---------------- 标签组 / 颜色（0.5.0） ---------------- */

  /** 详情面板 chip 的颜色查表（按标签名） */
  const tagColorMap = useMemo(() => {
    const m: Record<string, string> = {};
    for (const t of tags) if (t.color) m[t.name] = t.color;
    return m;
  }, [tags]);

  const editColor = useCallback(
    async (color: string | null) => {
      const e = tagEdit;
      if (!e) return;
      try {
        if (e.kind === "tag") await source.setTagColor(e.name, color);
        else await source.setGroupColor(e.name, color);
        setRefreshKey((k) => k + 1);
      } catch (err) {
        toast(String(err), true);
      }
    },
    [tagEdit, toast]
  );

  /** 改名：标签走「撞名即合并」，分组走唯一性校验。两边都把编辑卡切到新名字，否则下一次改色会打到旧名上 */
  const renameEntity = useCallback(
    async (next: string) => {
      const e = tagEdit;
      if (!e) return;
      try {
        if (e.kind === "tag") {
          const merged = await source.renameTag(e.name, next);
          if (tagFilter === e.name) setTagFilter(next);
          toast(
            merged > 0 ? `已并入「${next}」` : `已改名为「${next}」`,
            false,
            merged > 0 ? `${merged} 项素材的标签合并了过去` : undefined
          );
          setTagEdit({ kind: "tag", name: next });
        } else {
          await source.renameGroup(e.name, next);
          toast(`分组已改名为「${next}」`);
          setTagEdit({ kind: "group", name: next });
        }
        setRefreshKey((k) => k + 1);
      } catch (err) {
        toast(String(err), true);
      }
    },
    [tagEdit, tagFilter, toast]
  );

  const moveTagToGroup = useCallback(
    async (group: string | null) => {
      const e = tagEdit;
      if (!e || e.kind !== "tag") return;
      try {
        await source.setTagGroup(e.name, group);
        toast(group ? `「${e.name}」已放进「${group}」` : `「${e.name}」已移出分组`);
        setRefreshKey((k) => k + 1);
      } catch (err) {
        toast(String(err), true);
      }
    },
    [tagEdit, toast]
  );

  const deleteEntity = useCallback(async () => {
    const e = tagEdit;
    if (!e) return;
    setTagEdit(null);
    if (e.kind === "tag") {
      await deleteTagEverywhere(e.name);
      return;
    }
    const v = await askRef.current({
      title: `解散分组「${e.name}」？`,
      body: (
        <p className="dlg-hint">
          只解散分组，<b>不会删标签</b>：组里的标签回到「未分组」，素材上的标签一条都不会少。
        </p>
      ),
      buttons: [
        { label: "解散分组", value: "go", tone: "danger" },
        { label: "取消", value: "no" },
      ],
    });
    if (v !== "go") return;
    try {
      const n = await source.deleteGroup(e.name);
      toast(`已解散「${e.name}」`, false, `${n} 个标签回到未分组`);
      setRefreshKey((k) => k + 1);
    } catch (err) {
      toast(String(err), true);
    }
  }, [tagEdit, deleteTagEverywhere, toast]);

  const createGroup = useCallback(
    async (name: string) => {
      try {
        await source.newGroup(name);
        toast(`已建分组「${name}」`, false, "右键任意标签 → 分组，就能把它放进来");
        setRefreshKey((k) => k + 1);
      } catch (err) {
        toast(String(err), true);
      }
    },
    [toast]
  );

  /* ---------------- 移动（T01/T02/T11 共用）：体检 → 三选 → 阈值确认 → 落盘 ---------------- */
  const doMove = useCallback(
    async (paths: string[], target: string) => {
      if (!paths.length || !target) return;
      setBusy(true);
      try {
        const plan = await source.movePlan(paths, target);
        let policy: MovePolicy = "skip";
        if (plan.conflicts.length) {
          const v = await askRef.current({
            title: `目标文件夹里已经有 ${plan.conflicts.length} 个同名文件`,
            body: (
              <>
                <div className="dlg-list mono">
                  {plan.conflicts.slice(0, 6).map((n) => (
                    <div key={n}>{n}</div>
                  ))}
                  {plan.conflicts.length > 6 && <div>…还有 {plan.conflicts.length - 6} 个</div>}
                </div>
                <p className="dlg-hint">「替换」会把目标位置原有的那个先放进回收站，可以还原。</p>
              </>
            ),
            buttons: [
              { label: "保留两者（自动改名）", value: "keep_both", tone: "accent" },
              { label: "替换旧的", value: "replace", tone: "danger" },
              { label: "跳过同名的", value: "skip" },
            ],
          });
          if (!v) return;
          policy = v as MovePolicy;
        }

        if (plan.files > CONFIRM_MOVE_COUNT || plan.bytes > CONFIRM_MOVE_BYTES) {
          const v = await askRef.current({
            title: `要移动 ${fmtNum(plan.files)} 个文件（${fmtSize(plan.bytes)}）`,
            body: (
              <p className="dlg-hint">
                目标：<span className="mono">{target}</span>
                {plan.crossVolume && (
                  <>
                    <br />
                    跨盘移动会「先复制、再删源」，大文件会比较久，中途别关软件。
                  </>
                )}
              </p>
            ),
            buttons: [
              { label: "开始移动", value: "go", tone: "accent" },
              { label: "取消", value: "no" },
            ],
          });
          if (v !== "go") return;
        } else if (plan.crossVolume) {
          toast("跨盘移动", false, "会先复制再删源，稍慢一点");
        }

        /* 交给后台线程执行（缺陷 #7）：这里只等「已经起跑」，
           逐项进度与收尾走 move://progress / move://done 两个事件。
           跨盘复制几百 MB 时界面仍然能滚动、能看缩略图。 */
        await source.moveStart(paths, target, policy);
        setBatch({ done: 0, total: paths.length, label: "正在移动" });
      } catch (e) {
        toast(String(e), true);
      } finally {
        setBusy(false);
      }
    },
    [toast]
  );

  /* ---------------- 删除（单个 / 批量，T11 分片） ---------------- */
  const doDelete = useCallback(
    async (paths: string[], label: string) => {
      if (!paths.length) return;
      setBusy(true);
      try {
        const st = await source.statPaths(paths);
        const many = st.files > CONFIRM_DELETE_COUNT || paths.length > CONFIRM_DELETE_COUNT;
        const v = await askRef.current({
          title: `把${label}移到回收站？`,
          body: (
            <p className="dlg-hint">
              {paths.length > 1 ? `${paths.length} 项，` : ""}共 {fmtNum(st.files)} 个文件（
              {fmtSize(st.bytes)}）。
              <br />
              删除＝移进左侧「回收站」，随时可以还原；清空时才彻底删掉。
              {many && <><br />数量较多，会分批执行，中途可以看进度。</>}
            </p>
          ),
          buttons: [
            { label: "移到回收站", value: "go", tone: "danger" },
            { label: "取消", value: "no" },
          ],
        });
        if (v !== "go") return;

        let removed = 0;
        if (paths.length <= CHUNK) {
          removed = await source.remove(paths);
        } else {
          setBatch({ done: 0, total: paths.length });
          for (let i = 0; i < paths.length; i += CHUNK) {
            const part = paths.slice(i, i + CHUNK);
            try {
              removed += await source.remove(part);
            } catch (e) {
              toast(`第 ${i + 1}~${i + part.length} 个没删成`, true, String(e));
            }
            setBatch({ done: Math.min(paths.length, i + part.length), total: paths.length });
          }
          setBatch(null);
        }
        toast(`已移到回收站 ${removed} 项`, false, "在左侧「回收站」里可以还原");
        setSelected(null);
        setSelectedIds(new Set());
        setRefreshKey((k) => k + 1);
      } catch (e) {
        setBatch(null);
        toast(String(e), true);
      } finally {
        setBusy(false);
      }
    },
    [toast]
  );

  /* ---------------- 拖出窗口 / 拖到目录树（T01 / T02 / T11 批量） ---------------- */
  const dragMoveRef = useRef(doMove);
  dragMoveRef.current = doMove;

  const onDragOut = useCallback(
    async (a: Asset) => {
      // 拖的是「选中集合里的一员」就整批拖出去，否则只拖这一张
      const many = selectedIds.has(a.id) && selectedIds.size > 1;
      const list = many ? selPaths : [a.path];
      dragPaths.current = list;
      dragPreview.current = a.id;
      try {
        await source.dragOut(list, a.id);
      } catch (e) {
        dragPaths.current = null;
        toast(String(e), true);
      }
    },
    [selectedIds, selPaths, toast]
  );

  /* OLE 拖拽结束回调：如果松手位置压在本窗口的目录树上，就当「移到这里」 */
  useEffect(() => {
    const off = onAppEvent<{ x: number; y: number; dropped: boolean }>("am://drag-end", (p) => {
      const paths = dragPaths.current;
      dragPaths.current = null;
      if (!p.dropped || !paths?.length) return;
      // 松手点换算是 Rust 侧算好的 CSS 像素（已除过 DPR），这里直接命中测试
      const el = document.elementFromPoint(p.x, p.y) as HTMLElement | null;
      const row = el?.closest<HTMLElement>("[data-dir-path]");
      const target = row?.dataset.dirPath;
      if (!target) return;
      // 拖回自己所在的目录 = 无操作，别弹一堆提示
      const tl = target.toLowerCase().replace(/[\\/]+$/, "");
      if (paths.every((x) => x.slice(0, x.lastIndexOf("\\") + 1).toLowerCase().replace(/[\\/]+$/, "") === tl)) {
        return;
      }
      void dragMoveRef.current(paths, target);
    });
    return off;
  }, []);

  /* ---------------- 加库：原生文件夹选择器（T15） ---------------- */
  const onAddPath = async (path: string, label = "已加入库") => {
    if (!path) return;
    await startScan(path, label);
  };

  const onAddFolder = async () => {
    try {
      /* 最多 3 个库（与 FontManager 的多库上限一致）。
         到顶了必须**明确告诉用户为什么**：只弹一条 toast 的话，用户点完"添加文件夹"
         看到选择框没出来（或者选完列表没变），结论是"这个功能坏了"。
         2026-11-16 用户就报过一次「选了资产文件夹，并没有在软件里显示出来」——
         真实原因就是库已经 3/3。 */
      const n = await source.libCount();
      if (n >= MAX_LIBS) {
        const roots = await source.roots().catch(() => []);
        /* 内置截图库也在 roots 里，但它不占名额 —— 不列出来，否则用户会数出 4 个 */
        const bi = await source.builtinRoots().catch(() => [] as string[]);
        const norm2 = (s: string) => s.replace(/[\\/]+$/, "").toLowerCase();
        const userRoots = roots.filter((r) => !bi.some((b) => norm2(b) === norm2(r.path)));
        await dlg.ask({
          title: `已经有 ${MAX_LIBS} 个素材库了`,
          body: (
            <div className="grow">
              <p>最多同时挂 {MAX_LIBS} 个。想加新的，先从下面某一个移除：</p>
              <p className="mono" style={{ marginTop: 6, opacity: 0.75, whiteSpace: "pre-line" }}>
                {userRoots.map((r) => r.path).join("\n")}
              </p>
              <p style={{ marginTop: 8 }}>
                移除方法：在左侧列表里<b>右键那个库</b> → 「从素材库移除（不删文件）」。
                只是不再索引它，磁盘上的文件一个都不会动，以后还能再加回来。
              </p>
            </div>
          ),
          buttons: [{ label: "知道了", value: "ok", tone: "accent" }],
        });
        return;
      }
      const p = await source.pickFolder("选择要加入素材库的文件夹（只建索引，不移动文件）");
      if (!p) return;
      /* 已经在库里就别再扫一遍（用户的库可能只有两三个，但重复点 + 很常见） */
      const existing = await source.roots().catch(() => []);
      const norm = (s: string) => s.replace(/[\\/]+$/, "").toLowerCase();
      if (existing.some((r) => norm(r.path) === norm(p))) {
        toast("这个文件夹已经在素材库里了", true);
        return;
      }
      await onAddPath(p);
    } catch (e) {
      toast(String(e), true);
    }
  };

  /** 从素材库移除一个库根：只清索引。磁盘文件一个都不动（要删文件走回收站操作） */
  const onRemoveLib = async (path: string) => {
    const ok = await dlg.ask({
      title: "从素材库移除？",
      body: (
        <p>
          只会把这个文件夹从索引里去掉，<b>磁盘上的文件一个都不会动</b>。
          以后还可以再把它加回来，缩略图缓存会在淘汰时回收。
        </p>
      ),
      buttons: [
        { label: "取消", value: null as unknown as string },
        { label: "移除", value: "ok", tone: "danger" },
      ],
    });
    if (ok !== "ok") return;
    try {
      const n = await source.removeLib(path);
      toast("已从素材库移除", false, `清掉 ${fmtNum(n)} 个目录索引`);
      setDir(null);
      setSelected(null);
      setSelectedIds(new Set());
      setRefreshKey((k) => k + 1);
    } catch (e) {
      toast(String(e), true);
    }
  };

  /* ---------------- T23：评分 / 注释 / 来源 URL ----------------
     三样都是**人写的东西**（删了就找不回），所以：改完立刻回写列表与详情，
     不让界面显示旧值；写库失败就弹错误，绝不假装成功。 */
  const patchAsset = useCallback((ids: number[], patch: Partial<Asset>) => {
    const set = new Set(ids);
    setItems((prev) => prev.map((a) => (set.has(a.id) ? { ...a, ...patch } : a)));
    setSelected((cur) => (cur && set.has(cur.id) ? { ...cur, ...patch } : cur));
  }, []);

  const onSetRating = useCallback(
    async (ids: number[], rating: number) => {
      try {
        await source.setRating(ids, rating);
        patchAsset(ids, { rating });
      } catch (e) {
        toast(String(e), true);
      }
    },
    [patchAsset, toast]
  );

  const onSetNote = useCallback(
    async (id: number, note: string) => {
      try {
        await source.setNote(id, note);
        patchAsset([id], { note });
      } catch (e) {
        toast(String(e), true);
      }
    },
    [patchAsset, toast]
  );

  const onSetSource = useCallback(
    async (id: number, url: string) => {
      try {
        await source.setSource(id, url);
        patchAsset([id], { sourceUrl: url });
      } catch (e) {
        toast(String(e), true);
      }
    },
    [patchAsset, toast]
  );

  const onNewSubfolder = async (parent: string) => {
    const name = window.prompt("新文件夹名称（会真的在磁盘上创建）", "");
    if (!name) return;
    try {
      await source.mkdir(parent, name);
      toast("已创建文件夹");
      setRefreshKey((k) => k + 1);
    } catch (e) {
      toast(String(e), true);
    }
  };

  const onRescan = async () => {
    if (!dir) {
      toast("先选一个库目录", true);
      return;
    }
    await startScan(dir.path, "重新扫描");
  };

  const onReveal = async (a: Asset) => {
    try {
      await source.reveal(a.path);
    } catch (e) {
      toast(String(e), true);
    }
  };

  const onOpenDefault = async (a: Asset) => {
    try {
      await source.openDefault(a.path);
    } catch (e) {
      toast(String(e), true);
    }
  };

  /* 另存为：走系统保存对话框，原样复制字节（动图/RAW/PSD 都成立） */
  const onSaveAs = async (a: Asset) => {
    try {
      const dst = await source.saveAs(a.path, a.name);
      if (dst) toast("已另存为", false, dst);
    } catch (e) {
      toast(String(e), true);
    }
  };

  /* 调整尺寸（T38）：长边三档 → 选位置 → 重新编码另存。
     原图一个字节都不动（这是"另存一份"，不是编辑器）。 */
  const onResize = async (a: Asset) => {
    const pick = await dlg.ask({
      title: "调整尺寸另存为",
      body: (
        <div className="dlg-note">
          当前 {a.width || "?"} × {a.height || "?"}，选一个长边上限。
          <br />
          只缩不放；原图不会被改动，输出到你在下一步选的位置。
        </div>
      ),
      buttons: [
        { label: "长边 512", value: "512" },
        { label: "长边 1024", value: "1024" },
        { label: "长边 2048", value: "2048", tone: "accent" },
      ],
    });
    if (!pick) return;
    try {
      const out = await source.resizeSave(a.path, Number(pick), "png");
      if (out) {
        toast(
          `已另存 ${out.width} × ${out.height}`,
          false,
          `${(out.bytes / 1024).toFixed(0)} KB · ${out.path}`
        );
      }
    } catch (e) {
      toast(String(e), true);
    }
  };

  const onCopyFile = async (paths: string[]) => {
    try {
      await source.copyFile(paths);
      toast(paths.length > 1 ? `已复制 ${paths.length} 个原文件` : "已复制原文件", false, "现在可以直接粘到 PS / 资源管理器");
    } catch (e) {
      toast(String(e), true);
    }
  };

  /* 改名：入口只负责「把详情顶栏切到编辑态」，真正的落盘在 commitRename。
     以前这里弹 window.prompt（系统灰框，和整个界面不是一个东西），
     owner 2026-11-17 明确要求：所有改名都在顶栏文件名上就地改，不弹窗。 */
  const onRename = (a: Asset) => {
    setSelected(a);
    setSelectedIds(new Set([a.id]));
    setRenameId(a.id);
  };

  const commitRename = async (a: Asset, raw: string) => {
    setRenameId(null);
    const name = raw.trim();
    if (!name || name === a.name) return;
    try {
      await source.rename(a.path, name);
      toast("已重命名原文件");
      setRefreshKey((k) => k + 1);
    } catch (e) {
      toast(String(e), true);
    }
  };

  /* 回收站：进页面时拉一次；还原/彻底删除/清空后自己刷新一遍 */
  const loadTrash = useCallback(async () => {
    try {
      setTrash(await source.trashList());
    } catch (e) {
      toast(String(e), true);
    }
  }, [toast]);

  useEffect(() => {
    if (page === "trash") void loadTrash();
  }, [page, refreshKey, loadTrash]);

  const restoreTrash = async (ids: string[]) => {
    try {
      const n = await source.trashRestore(ids);
      toast(`已还原 ${n} 项`);
      await loadTrash();
      setRefreshKey((k) => k + 1);
    } catch (e) {
      toast(String(e), true);
    }
  };

  const purgeTrash = async (ids: string[]) => {
    try {
      const n = await source.trashPurge(ids);
      toast(`已彻底删除 ${n} 项`);
      await loadTrash();
    } catch (e) {
      toast(String(e), true);
    }
  };

  const emptyTrash = async () => {
    const ok = await dlg.ask({
      title: `清空回收站（${trash.length} 项）？`,
      body: <p>回收站里的文件会被彻底删除（先进系统回收站，还能再捞一次）。</p>,
      buttons: [
        { label: "取消", value: null as unknown as string },
        { label: "清空", value: "ok", tone: "danger" },
      ],
    });
    if (ok !== "ok") return;
    try {
      const n = await source.trashEmpty();
      toast(`回收站已清空（${n} 项）`);
      await loadTrash();
    } catch (e) {
      toast(String(e), true);
    }
  };

  /* 开大图：网格双击、详情里的「看图」都走这一处 */
  const openLightbox = (a: Asset) => {
    const i = items.findIndex((x) => x.id === a.id);
    setLightbox(i >= 0 ? i : 0);
  };

  const onUndo = async () => {
    try {
      const msg = await source.undo();
      toast(msg);
      setRefreshKey((k) => k + 1);
    } catch (e) {
      toast(String(e), true);
    }
  };

  const onOpenLibrary = async () => {
    const rs = await source.roots();
    const p = dir?.path ?? rs[0]?.path;
    if (!p) {
      toast("还没有库，先添加一个文件夹", true);
      return;
    }
    try {
      await source.reveal(p);
    } catch (e) {
      toast(String(e), true);
    }
  };

  /* 无边框看图：开一扇"就是那张图"的独立窗口（照果核看图的无边框模式）。
     为什么不在主窗里做：截图时主窗的侧栏/顶栏/详情栏都会进画面。
     `items` 整个带过去，方向键在里面直接翻。 */
  const openPeek = useCallback(
    (a: Asset) => {
      const list = items.length ? items : [a];
      const i = Math.max(0, list.findIndex((v) => v.id === a.id));
      // 记住上次缩放到的长边（同一个 origin，localStorage 两个窗口共用）
      const last = Number(localStorage.getItem("am.peek.longEdge") ?? "") || undefined;
      void source
        .peekOpen(
          list.map((v) => v.id),
          i,
          last
        )
        .catch((e) => toast(String(e), true));
    },
    [items, toast]
  );

  /* 中键（滚轮按下去）= 立刻贴图。
     素材格上的左键有选择语义、右键有菜单，中键在浏览器里是"自动滚动"，在这里正好空着。 */
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (e.button !== 1) return;
      const t = e.target as HTMLElement | null;
      const tile = t?.closest<HTMLElement>("[data-asset-id]");
      if (!tile) return;
      e.preventDefault();
      const id = Number(tile.dataset.assetId);
      const a = items.find((v) => v.id === id);
      if (a) openPeek(a);
    };
    document.addEventListener("mousedown", onDown, true);
    return () => document.removeEventListener("mousedown", onDown, true);
  }, [items, openPeek]);

  /* ---------------- 右键菜单 ----------------
     一处接管全窗口：
     · 页面任意处右键都 preventDefault，WebView2 那套「返回/刷新/另存为/打印」不再出现；
     · 输入框除外（系统自带的剪切/复制/粘贴是刚需，别一起砍掉）；
     · 素材格 / 目录树行 / 空白区各给一份真菜单，所有条目都接到已有的命令上。
     多选时菜单作用于**整个选中集合**（T11）。 */
  useEffect(() => {
    const onCtx = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (!t) return;
      // 弹层上不许冒出浏览器菜单，也不额外叠一层菜单
      if (t.closest(".set-scrim") || t.closest(".lightbox") || t.closest(".ctx-menu") || t.closest(".dlg-scrim")) {
        e.preventDefault();
        return;
      }
      if (t.closest('input, textarea, [contenteditable="true"]')) return;

      e.preventDefault();
      const x = e.clientX;
      const y = e.clientY;
      const show = (menu: CtxItem[]) => setCtx({ x, y, items: menu });

      const tile = t.closest<HTMLElement>("[data-asset-id]");
      if (tile) {
        const hitId = Number(tile.dataset.assetId);
        const a = items.find((v) => v.id === hitId);
        if (!a) return;
        // 右键没选中项上的一格 → 先把选择切到这一格；已经在选中的集合里 → 保留集合
        const inSel = selectedIds.has(hitId);
        if (!inSel) {
          setSelected(a);
          setSelectedIds(new Set([a.id]));
        }
        const paths = inSel ? selPaths : [a.path];
        const n = paths.length;
        show([
          { label: n > 1 ? `用默认程序打开（${n} 个）` : "用默认程序打开", icon: "image", action: () => void onOpenDefault(a) },
          {
            label: "无边框看图（可以拖到任意位置截图）",
            icon: "expand",
            shortcut: "B",
            action: () => openPeek(a),
          },
          {
            label: "预览大图",
            icon: "play",
            shortcut: "空格",
            action: () => {
              const i = items.findIndex((v) => v.id === a.id);
              setLightbox(i >= 0 ? i : 0);
            },
          },
          { kind: "separator" },
          {
            label: n > 1 ? `加入分组…（${n} 项）` : "加入分组…",
            icon: "folder-open",
            /* 第二级菜单：菜单自己点完会关掉，所以延一帧再开新的，
               否则刚设进去的 ctx 立刻被 onClose 清掉（表现为"点了没反应"） */
            action: () => {
              const ids2 = inSel ? [...selectedIds] : [a.id];
              window.setTimeout(() => collPickMenu(ids2, x, y), 0);
            },
          },
          /* 正在按某个分组筛选时，才给「从这个分组移除」—— 否则用户不知道往哪个组移除。
             多选时作用于整个选中集合。 */
          ...(collFilter != null && colls.some((c) => c.id === collFilter)
            ? [
                {
                  label: (() => {
                    const nm = colls.find((c) => c.id === collFilter)?.name ?? "分组";
                    return n > 1 ? `从「${nm}」移除（${n} 项）` : `从「${nm}」移除`;
                  })(),
                  icon: "close",
                  action: () => {
                    const ids2 = inSel ? [...selectedIds] : [a.id];
                    void removeFromColl(collFilter, ids2);
                  },
                } as CtxItem,
              ]
            : []),
          {
            /* 快照（0.1.18）：只记清单、不搬文件，所以放在「加入分组」旁边 */
            label: n > 1 ? `加入快照…（${n} 项）` : "加入快照…",
            icon: "camera",
            action: () => {
              const ids2 = inSel ? [...selectedIds] : [a.id];
              window.setTimeout(() => setSnapPrompt({ mode: "ids", ids: ids2 }), 0);
            },
          },
          { label: "在资源管理器中显示", icon: "reveal", action: () => void onReveal(a) },
          /* T38：单张才有"另存为 / 调整尺寸"（多选时"存到哪"没有意义） */
          {
            label: "另存为…",
            icon: "import",
            disabled: n > 1 || busy,
            action: () => void onSaveAs(a),
          },
          {
            label: "调整尺寸另存为…",
            icon: "expand",
            disabled: n > 1 || busy || !a.width || !a.height,
            action: () => void onResize(a),
          },
          {
            label: n > 1 ? `复制 ${n} 个原文件` : "复制原文件",
            icon: "copy",
            disabled: busy,
            action: () => void onCopyFile(paths),
          },
          {
            label: "复制完整路径",
            icon: "copy",
            action: () => {
              void navigator.clipboard
                .writeText(paths.join("\r\n"))
                .then(() => toast(n > 1 ? `已复制 ${n} 条路径` : "已复制完整路径"))
                .catch(() => toast("复制失败，可在详情面板里选中", true));
            },
          },
          /* 「复制更多」里的那几样（照果核看图）：文件名 / 分辨率。
             单张时才有分辨率可言，多选就只给文件名 */
          {
            label: n > 1 ? `复制 ${n} 个文件名` : "复制文件名",
            icon: "copy",
            action: () => {
              void navigator.clipboard
                .writeText(paths.map((pp) => pp.split(/[\\/]/).pop() ?? pp).join("\r\n"))
                .then(() => toast(n > 1 ? `已复制 ${n} 个文件名` : "已复制文件名"))
                .catch(() => toast("复制失败", true));
            },
          },
          {
            label: "复制分辨率",
            icon: "copy",
            disabled: n > 1 || !a.width || !a.height,
            action: () => {
              void navigator.clipboard
                .writeText(`${a.width}×${a.height}`)
                .then(() => toast(`已复制 ${a.width}×${a.height}`))
                .catch(() => toast("复制失败", true));
            },
          },
          { kind: "separator" },
          { label: "重命名", icon: "more", disabled: n > 1, action: () => void onRename(a) },
          {
            label: n > 1 ? `把 ${n} 项移到当前目录` : "移到当前目录…",
            icon: "folder",
            disabled: !dir || busy,
            action: () => dir && void doMove(paths, dir.path),
          },
          {
            label: n > 1 ? `把 ${n} 项移到回收站` : "移到回收站",
            icon: "trash",
            danger: true,
            action: () => void doDelete(paths, n > 1 ? `这 ${n} 项` : `「${a.name}」`),
          },
        ]);
        return;
      }

      /* 标签面板与弹层有自己的右键菜单，别再弹「重新扫描 / 全选 / 瀑布流」那一套 ——
         那是"库级"操作，落在一条标签上完全不对题（实测：右键标签会同时弹编辑卡和这个菜单）。 */
      if (t.closest(".tag-dock") || t.closest(".dlg-scrim") || t.closest(".dlg")) return;

      const dirRow = t.closest<HTMLElement>("[data-dir-path]");
      if (dirRow) {
        const p = dirRow.dataset.dirPath ?? "";
        const name = dirRow.dataset.dirName ?? "";
        show([
          {
            label: "在资源管理器中打开",
            icon: "reveal",
            action: () => void source.reveal(p).catch((err) => toast(String(err), true)),
          },
          {
            label: "重新扫描这个目录",
            icon: "scan",
            action: () => void startScan(p, `重新扫描 ${name}`),
          },
          {
            label: selPaths.length > 1 ? `把 ${selPaths.length} 项移到这里` : "把选中的素材移到这里",
            icon: "folder",
            disabled: !selected || busy,
            action: () => selected && void doMove(selPaths.length ? selPaths : [selected.path], p),
          },
          { kind: "separator" },
          {
            label: "在这里新建子文件夹",
            icon: "plus",
            action: () => void onNewSubfolder(p),
          },
          /* 只有库根才有「移除」：子目录用「移到回收站」处理，不该有库级动作。
             内置库（软件自己的截图目录）不给移除 —— 截图/贴图/灵感板粘贴都往那儿存，
             删了索引用户就"截了图找不着"。给一条禁用项写明原因，比菜单里干脆没有更好懂。 */
          ...(dirRow.dataset.dirRoot === "1"
            ? dirRow.dataset.dirBuiltin === "1"
              ? [
                  { kind: "separator" as const },
                  {
                    label: "软件截图目录（内置库，不能移除）",
                    icon: "database",
                    disabled: true,
                    action: () => {},
                  },
                ]
              : [
                  { kind: "separator" as const },
                  {
                    label: "从素材库移除（不删文件）",
                    icon: "database",
                    danger: true,
                    action: () => void onRemoveLib(p),
                  },
                ]
            : []),
        ]);
        return;
      }

      show([
        { label: "重新扫描当前库", icon: "scan", action: () => void onRescan() },
        { label: "添加文件夹到库", icon: "plus", action: () => void onAddFolder() },
        { label: "全选", icon: "image", shortcut: "Ctrl+A", action: () => pickMany(items.map((v) => v.id), false) },
        { label: "撤销上一步", icon: "undo", shortcut: "Ctrl+Z", action: () => void onUndo() },
        { kind: "separator" },
        /* 截图 / 剪贴板看图是系统级功能（默认 F1 / F3），这里再给一份鼠标入口：
           快捷键被别的软件占住时（本机 Snipaste 就占着 F1/F3），用户总得有地方点。 */
        {
          label: "截图（选完区存到截图目录）",
          icon: "image",
          shortcut: "F1",
          action: () => void source.snipStart().catch((e) => toast(String(e), true)),
        },
        {
          label: "把剪贴板里的图用无边框看图打开",
          icon: "copy",
          shortcut: "F3",
          action: () => void source.clipPeek().catch((e) => toast(String(e), true)),
        },
        { kind: "separator" },
        { label: "常规网格", icon: "grid", disabled: view === "grid", action: () => setView("grid") },
        {
          label: "瀑布流",
          icon: "waterfall",
          disabled: view === "waterfall",
          action: () => setView("waterfall"),
        },
        { kind: "separator" },
        { label: "偏好设置", icon: "settings", shortcut: "Ctrl+,", action: () => setSettingsOpen(true) },
        { label: "快捷键一览", icon: "keyboard", action: () => setShortcutsOpen(true) },
        { label: "查找重复素材", icon: "copy", action: () => setTools({ tab: "dupes" }) },
        { label: "检查丢失的引用", icon: "funnel", action: () => setTools({ tab: "refs" }) },
        { label: "批量重命名", icon: "tag", action: () => setTools({ tab: "rename" }) },
      ]);
    };
    document.addEventListener("contextmenu", onCtx, true);
    return () => document.removeEventListener("contextmenu", onCtx, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    items,
    view,
    settingsOpen,
    dir,
    selected,
    busy,
    selectedIds,
    selPaths,
    openPeek,
    /* 「从分组移除」那一项要用到这三个（分组筛选态 + 分组清单 + 移除动作），
       漏了会点出旧闭包：筛选已经切了，移除却仍作用在上一个分组上 */
    collFilter,
    colls,
    removeFromColl,
  ]);

  // 键盘：空格预览、方向键切换、Esc、Ctrl+Z、Ctrl+,
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      // 设置面板 / 右键菜单开着时，全局快捷键让位给浮层
      if (settingsOpen || shortcutsOpen || tools || ctx) return;
      // 命令面板：连输入框里也响应（它就是"全局"入口，别让焦点位置决定能不能用）
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setCmdkOpen(true);
        return;
      }
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") {
        if (e.key === "Escape") (e.target as HTMLElement).blur();
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key === ",") {
        e.preventDefault();
        setSettingsOpen(true);
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
        e.preventDefault();
        pickMany(items.map((v) => v.id), false);
        return;
      }
      if (e.key === " " && selected) {
        e.preventDefault();
        const i = items.findIndex((x) => x.id === selected.id);
        setLightbox(i >= 0 ? i : 0);
      } else if (e.key === "Escape") {
        // 详情面板常驻，Esc 只关大图（owner 2026-11-16：详情不再收起）
        if (lightbox !== null) setLightbox(null);
        else if (selectedIds.size > 1) clearSel();
      } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
        if (lightbox !== null) {
          e.preventDefault();
          /* 换图走灯箱里的**逐帧闸门**（T37）：按住不放时，慢图也会先出一帧再切，
             而不是被一长串按键整批跳过（那是"按了半天不知道最后停在哪张"）。 */
          lightNav.current?.(e.key === "ArrowRight" ? 1 : -1);
        } else if (selected) {
          // 没开大图时方向键在图之间移动选中，详情面板跟着换
          const i = items.findIndex((x) => x.id === selected.id);
          const next = e.key === "ArrowRight" ? i + 1 : i - 1;
          if (next >= 0 && next < items.length) {
            e.preventDefault();
            const a = items[next];
            setSelected(a);
            setSelectedIds(new Set([a.id]));
            lastPick.current = a.id;
          }
        }
      } else if (e.key.toLowerCase() === "b" && !e.ctrlKey && !e.metaKey && !e.altKey) {
        // B = 无边框看图（Borderless）。对着参考图改稿/截图时最顺手的入口
        if (selected) {
          e.preventDefault();
          openPeek(selected);
        }
      } else if (e.key.toLowerCase() === "t" && !e.ctrlKey && !e.metaKey && !e.altKey) {
        // T = 给当前选中（多选就整批）打标签，和 FontManager 同一个键
        const ids = selectedIds.size ? [...selectedIds] : selected ? [selected.id] : [];
        if (ids.length) {
          e.preventDefault();
          setTagPrompt({ ids });
        }
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "c") {
        // Ctrl+C = 复制**原文件**（CF_HDROP）。粘到 PS/资源管理器得到的是文件，不是位图 ——
        // 和右键「复制原文件」完全同一条路，别让两处行为不一样
        const ids = selectedIds.size ? [...selectedIds] : selected ? [selected.id] : [];
        if (ids.length) {
          const paths = ids
            .map((id) => items.find((x) => x.id === id)?.path)
            .filter((p): p is string => !!p);
          if (paths.length) {
            e.preventDefault();
            void onCopyFile(paths);
          }
        }
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
        e.preventDefault();
        void onUndo();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, items, lightbox, settingsOpen, shortcutsOpen, tools, ctx, selectedIds, openPeek]);

  /* 打开标签编辑卡时把它的自动规则一起取回来（T34） */
  useEffect(() => {
    if (!tagEdit || tagEdit.kind !== "tag") {
      setTagRuleState(null);
      return;
    }
    let dead = false;
    void source
      .tagGetRule(tagEdit.name)
      .then((r) => !dead && setTagRuleState(r))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [tagEdit, source]);

  /* ---------------- 命令面板的清单（T27）----------------
     每条都指向下面已有的 handler，**不新造功能** —— 面板只是把散落在顶栏/右键/设置里的
     动作汇总成一张可搜索的清单。只列"现在真能做"的：没选中素材就不出现"删除选中项"。 */
  const cmds = useMemo<Command[]>(() => {
    const sel = selectedIds.size ? [...selectedIds] : selected ? [selected.id] : [];
    const out: Command[] = [
      {
        id: "settings",
        label: "打开偏好设置",
        hint: "Ctrl+,",
        icon: "settings",
        keywords: "设置 偏好 preference",
        run: () => setSettingsOpen(true),
      },
      {
        id: "add-lib",
        label: "添加素材库…",
        icon: "plus",
        keywords: "库 文件夹 添加 library folder",
        run: () => void onAddFolder(),
      },
      {
        id: "snap-new",
        label: "新建快照",
        icon: "expand",
        keywords: "快照 snapshot 收藏 书签",
        run: () => setSnapCreating(true),
      },
      {
        id: "snap-import",
        label: "导入快照（.json）",
        icon: "file",
        keywords: "快照 snapshot 导入 import",
        run: () => void importSnapshot(),
      },
    ];

    if (dir) {
      out.push(
        {
          id: "reveal-dir",
          label: "在资源管理器中打开当前目录",
          icon: "reveal",
          keywords: "定位 打开 文件夹 explorer",
          run: () => void source.reveal(dir.path).catch((e) => toast(String(e), true)),
        },
        {
          id: "new-sub",
          label: "在这里新建子文件夹…",
          icon: "folder",
          keywords: "新建 文件夹 mkdir 创建",
          run: () => void onNewSubfolder(dir.path),
        }
      );
    }

    out.push(
      {
        id: "view-grid",
        label: "视图：网格",
        icon: "grid",
        keywords: "视图 网格 grid",
        run: () => setView("grid"),
      },
      {
        id: "view-waterfall",
        label: "视图：瀑布流",
        icon: "waterfall",
        keywords: "视图 瀑布流 waterfall 列",
        run: () => setView("waterfall"),
      },
      {
        id: "size-s",
        label: "缩略图：小",
        icon: "image",
        keywords: "缩略图 尺寸 size 小",
        run: () => setSizeIndex(0),
      },
      {
        id: "size-m",
        label: "缩略图：中",
        icon: "image",
        keywords: "缩略图 尺寸 size 中",
        run: () => setSizeIndex(1),
      },
      {
        id: "size-l",
        label: "缩略图：大",
        icon: "image",
        keywords: "缩略图 尺寸 size 大",
        run: () => setSizeIndex(2),
      },
      {
        id: "sort-mtime",
        label: "排序：最近修改",
        icon: "sort",
        keywords: "排序 时间 最新 sort",
        run: () => setSort("mtime_desc"),
      },
      {
        id: "sort-name",
        label: "排序：名称",
        icon: "sort",
        keywords: "排序 名称 字母 sort",
        run: () => setSort("name_asc"),
      },
      {
        id: "sort-size",
        label: "排序：大小",
        icon: "sort",
        keywords: "排序 大小 体积 sort",
        run: () => setSort("size_desc"),
      },
      {
        id: "sort-rating",
        label: "排序：评分",
        icon: "star",
        keywords: "排序 评分 星级 sort rating",
        run: () => setSort("rating_desc"),
      },
      {
        id: "kind-all",
        label: "类型：全部",
        icon: "funnel",
        keywords: "类型 筛选 全部",
        run: () => setKindKey("all"),
      },
      {
        id: "kind-image",
        label: "类型：只看图片",
        icon: "funnel",
        keywords: "类型 筛选 图片 only image",
        run: () => setKindKey("image"),
      }
    );

    if (sel.length) {
      out.push(
        {
          id: "rate-1",
          label: `给选中项打 1 星（${sel.length} 项）`,
          icon: "star",
          keywords: "评分 星级 rate",
          run: () => void onSetRating(sel, 1),
        },
        {
          id: "rate-5",
          label: `给选中项打 5 星（${sel.length} 项）`,
          icon: "star",
          keywords: "评分 星级 rate 收藏",
          run: () => void onSetRating(sel, 5),
        },
        {
          id: "rate-0",
          label: `清除评分（${sel.length} 项）`,
          icon: "star",
          keywords: "评分 清除 取消 rate",
          run: () => void onSetRating(sel, 0),
        },
        {
          id: "tag",
          label: `打标签（${sel.length} 项）`,
          hint: "T",
          icon: "tag",
          keywords: "标签 tag 标记",
          run: () => setTagPrompt({ ids: sel }),
        },
        {
          id: "delete",
          label: `删除选中项（${sel.length} 项 → 回收站）`,
          icon: "trash",
          keywords: "删除 回收站 delete trash",
          run: () => void doDelete(selPaths, `${sel.length} 个素材`),
        }
      );
    }

    if (selected) {
      out.push({
        id: "peek",
        label: "无边框看图（可拖到任意位置截图）",
        hint: "B",
        icon: "expand",
        keywords: "无边框 看图 截图 borderless peek 参考图",
        run: () => openPeek(selected),
      });
      out.push({
        id: "preview",
        label: "全屏预览选中的素材",
        hint: "Space",
        icon: "play",
        keywords: "预览 全屏 灯箱 preview",
        run: () => {
          const i = items.findIndex((x) => x.id === selected.id);
          setLightbox(i >= 0 ? i : 0);
        },
      });
    }

    out.push(
      {
        id: "undo",
        label: "撤销上一步文件操作",
        hint: "Ctrl+Z",
        icon: "undo",
        keywords: "撤销 回退 undo",
        run: () => void onUndo(),
      },
      {
        id: "reindex",
        label: "重建索引（会清空后重扫）",
        icon: "scan",
        keywords: "重建 索引 重扫 rebuild",
        run: async () => {
          const ok = await dlg.ask({
            title: "重建索引？",
            body: <p>会清空索引并全量重扫所有库根。原文件不动，评分/注释/标签都保留。</p>,
            buttons: [
              { label: "取消", value: null as unknown as string },
              { label: "重建", value: "ok", tone: "danger" },
            ],
          });
          if (ok !== "ok") return;
          try {
            await source.rebuildIndex();
            toast("已开始重建", false, "后台在跑，扫完自动刷新");
          } catch (e) {
            toast(String(e), true);
          }
        },
      },
      {
        id: "gc",
        label: "清理缩略图缓存",
        icon: "database",
        keywords: "缓存 清理 释放 cache gc",
        run: async () => {
          try {
            const [before, freed] = await source.cacheGc(0, 0);
            toast(
              freed > 0 ? "缓存已清理" : "缓存已经是空的",
              false,
              freed > 0 ? `释放 ${fmtSize(freed)}（清理前 ${fmtSize(before)}）` : undefined
            );
          } catch (e) {
            toast(String(e), true);
          }
        },
      },
      {
        id: "warm",
        label: "后台预生成全部缩略图",
        icon: "image",
        keywords: "预热 预生成 warm 缩略图",
        run: () => void source.warmThumbs(THUMB_PX[prefs.size] ?? 384).catch((e) => toast(String(e), true)),
      },
      {
        id: "export",
        label: "导出用户数据（JSON）",
        icon: "copy",
        keywords: "导出 备份 export 用户数据",
        run: async () => {
          try {
            const p = await source.exportUserdata();
            toast("已导出用户数据", false, p);
          } catch (e) {
            toast(String(e), true);
          }
        },
      },
      {
        id: "recycle",
        label: "打开系统回收站",
        icon: "trash",
        keywords: "回收站 recycle 删除",
        run: () => void source.openRecycle().catch((e) => toast(String(e), true)),
      }
    );
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir?.id, selected?.id, selectedIds, items, selPaths, view, sort, kindKey, prefs.size]);

  const current = lightbox !== null ? items[lightbox] ?? null : null;

  return (
    <div className="app-frame">
      <TitleBar
        onRescan={onRescan}
        onAddFolder={onAddFolder}
        onSettings={() => setSettingsOpen(true)}
        onUndo={onUndo}
        busy={busy}
        where={dir?.name}
      />

      {/* 布局骨架照 FontManager：标题栏通栏 → 侧栏通栏 → 右侧「顶栏 + 内容」两行。
          ⚠️ 顶栏必须在 `.workspace` 里（= 侧栏右侧），不能放在 `.main` 外面：
          放外面就变成通栏横跨侧栏宽度，搜索框会压在侧栏列上，整条工具栏左移 248px，
          侧栏也被顶下去 57px —— 2026-11-15 owner 截图指出的就是这一条。 */}
      <div className="main">
        <Tree
          source={source}
          selected={dir?.id ?? null}
          onSelect={(n) => {
            // 点目录 = 离开标签 / 分组视图，否则两个筛选叠在一起会莫名其妙 0 结果
            setDir(n);
            setTagFilter(null);
            setCollFilter(null);
            setPage("assets");
          }}
          onDropTo={(n) => {
            if (selPaths.length) void doMove(selPaths, n.path);
          }}
          onToast={toast}
          onOpenLibrary={onOpenLibrary}
          onTrash={() => {
            setSelectedIds(new Set());
            setPage("trash");
          }}
          onAbout={() => {
            setSelectedIds(new Set());
            setPage("about");
          }}
          onShortcuts={() => setShortcutsOpen(true)}
          refreshKey={refreshKey}
          snapshotDock={
            <SnapshotDock
              items={snaps}
              creating={snapCreating}
              renamingId={snapRenaming?.id ?? null}
              onOpen={openSnapshot}
              onMenu={snapshotMenu}
              onStartCreate={() => {
                if (!snapCreating) setSnapCreating(true);
              }}
              onCreate={(name) => void createSnapshotInline(name)}
              onCancelCreate={() => setSnapCreating(false)}
              onRename={(id, name) => void renameSnapshotInline(id, name)}
              onCancelRename={() => setSnapRenaming(null)}
              onImport={() => void importSnapshot()}
            />
          }
          /* 分组面板：位置照 FontManager —— 底栏上方、**标签面板之上** */
          collDock={
            <CollDock
              items={colls}
              activeId={collFilter}
              onPick={(id) => {
                /* 点分组 = 跨目录看这一撮素材；再点一次取消。
                   和标签同一个口径：必须清目录，否则"目录 AND 分组"会空。 */
                setCollFilter(id);
                if (id !== null) {
                  setDir(null);
                  setTagFilter(null);
                  setColorFilter(null);
                  setRatingFilter(null);
                }
              }}
              onCreate={(name) => void createCollInline(name)}
              onRename={(id, name) => void renameCollInline(id, name)}
              onMenu={(row, x, y) => collMenu(row, x, y)}
              hold={!!ctx}
              half={false}
            />
          }
          tagDock={
            <TagDock              tags={tags}
              groups={tagGroups}
              autoTags={autoTagCounts}
              activeTag={tagFilter}
              onPickTag={(t) => {
                // 点标签 = 跨目录看这个标签；再点一次取消
                setTagFilter(t);
                if (t) {
                  setDir(null);
                  setCollFilter(null);
                }
              }}
              onEditTag={(name) => setTagEdit({ kind: "tag", name })}
              onEditGroup={(name) => setTagEdit({ kind: "group", name })}
              onNewGroup={(name) => void createGroup(name)}
              onTagMenu={(tag, x, y) => tagMenu(tag, x, y)}
              onGroupMenu={(name, x, y) => groupMenu(name, x, y)}
              /* 弹层开着时不收起：右键标签弹出编辑卡的瞬间，指针已经不在面板上了 */
              /* hold 是「标签面板保持展开」的开关（TagDock: open = pinned || hold）。
                 快照的新建/改名**不能**算进来 —— 就地输入行在侧栏上半部分，
                 把它算进来会让"点新建快照 → 下面的标签面板自己弹开"（owner 2026-11-17 报的 bug）。 */
              hold={!!tagEdit || !!tagPrompt || !!ctx || !!snapPrompt}
            />
          }
        />

        <div className="workspace">
          <TopBar
            text={text}
            onText={setText}
            kindKey={kindKey}
            onKind={setKindKey}
            sort={sort}
            onSort={setSort}
            sizeIndex={sizeIndex}
            onSize={setSizeIndex}
            view={view}
            onView={setView}
            color={colorFilter}
            onColor={(c) => {
              setColorFilter(c);
              if (c) {
                setTagFilter(null);
                setCollFilter(null);
              }
            }}
            colorCounts={colorCounts}
            tags={tags}
            activeTag={tagFilter}
            onTag={(n) => {
              setTagFilter(n);
              /* 颜色和标签都作用于同一批素材：选标签就把颜色放开，
                 否则"筛了红 + 筛了客户稿"会得到一个莫名其妙的空列表 */
              if (n) {
                setColorFilter(null);
                setRatingFilter(null);
                setCollFilter(null);
                /* 目录范围也要放开（2026-11-17 owner 报的 P0）：
                   侧栏点标签走的是「跨目录看这个标签」，顶栏筛选菜单里点标签
                   以前**没清目录** → 查询变成「当前目录 AND 这个标签」，
                   标签明明有 400 张、里面一张都不在当前目录，于是空态。
                   两个入口必须是同一种语义，否则同一个标签点出两种结果。 */
                setDir(null);
                setPage("assets");
              }
            }}
            source={source}
            dirNode={dir}
            onDir={(n) => {
              setDir(n);
              setTagFilter(null);
              setCollFilter(null);
              setPage("assets");
            }}
            autoTags={autoTagCounts}
            rating={ratingFilter}
            onRating={(n) => {
              setRatingFilter(n);
              if (n !== null) {
                setColorFilter(null);
                setTagFilter(null);
                setCollFilter(null);
              }
            }}
          />

          {colorFilter && (
            <div className="filter-strip">
              <span className="ctl-color-dot" style={{ background: `var(--c-${colorFilter}, #8b8b86)` }} />
              <span className="filter-strip-label">主色：{COLOR_LABEL[colorFilter] ?? colorFilter}</span>
              <button className="btn btn-tiny" onClick={() => setColorFilter(null)}>
                清除筛选
              </button>
            </div>
          )}

          {tagFilter && (
            <div className="filter-strip">
              <Icon name="tag" size={12} stroke={1.5} />
              <span className="filter-strip-label">
                标签：{isAutoTag(tagFilter) ? autoTagLabel(tagFilter) : tagFilter}
              </span>
              {isAutoTag(tagFilter) && <span className="filter-strip-kind">自动标签</span>}
              <button className="btn btn-tiny" onClick={() => setTagFilter(null)}>
                清除筛选
              </button>
            </div>
          )}

          {ratingFilter !== null && (
            <div className="filter-strip">
              <Icon name="star" size={12} stroke={1.5} />
              <span className="filter-strip-label">评分：{ratingFilter} 星及以上</span>
              <button className="btn btn-tiny" onClick={() => setRatingFilter(null)}>
                清除筛选
              </button>
            </div>
          )}

          {collFilter !== null && (
            <div className="filter-strip">
              <Icon name="folder-open" size={12} stroke={1.5} />
              <span className="filter-strip-label">
                分组：{colls.find((c) => c.id === collFilter)?.name ?? "分组"}
              </span>
              <button className="btn btn-tiny" onClick={() => setCollFilter(null)}>
                清除筛选
              </button>
            </div>
          )}

          {snapFilter !== null && (
            <div className="filter-strip">
              <Icon name="camera" size={12} stroke={1.5} />
              <span className="filter-strip-label">
                快照：{snaps.find((s) => s.id === snapFilter)?.name ?? "快照"}
              </span>
              <span className="filter-strip-kind">按图片内容匹配</span>
              <button className="btn btn-tiny" onClick={() => setSnapFilter(null)}>
                清除筛选
              </button>
            </div>
          )}

          <div className={`content ${prefs.showTileName ? "" : "no-tile-name"}`}>
            {/* 回收站 / 关于 是整页替换：铺在内容区上层，切回来时列表状态还在（不重挂载） */}
            {page !== "assets" && (
              <div className="page-view">
                {page === "trash" ? (
                  <TrashView
                    items={trash}
                    onRestore={(ids) => void restoreTrash(ids)}
                    onPurge={(ids) => void purgeTrash(ids)}
                    onEmpty={() => void emptyTrash()}
                    onClose={() => setPage("assets")}
                  />
                ) : (
                  <AboutView
                    onOpenUrl={(u) =>
                      void source.openUrl(u).catch((e) => toast(String(e), true))
                    }
                  />
                )}
              </div>
            )}
            <AssetView
              items={items}
              view={view}
              sizeIndex={sizeIndex}
              selectedIds={selectedIds}
              resetKey={`${dir?.id ?? "none"}|${kindKey}|${debounced}|${sort}|${refreshKey}|${tagFilter ?? ""}|${colorFilter ?? ""}|${collFilter ?? ""}|${snapFilter ?? ""}`}
              onPick={pickOne}
              onPickMany={pickMany}
              onOpen={(a) => openLightbox(a)}
              onDragOut={(a) => void onDragOut(a)}
              onNearBottom={loadMore}
              thumbSrc={thumbSrc}
              onPrefetch={onPrefetchThumbs}
              onToast={(m, e) => toast(m, e)}
            />

            {/* 批量操作条（T11）：只在多选时出现，压在内容区底部中间 */}
            {selectedIds.size > 1 && (
              <div className="batch-bar" role="toolbar" aria-label="批量操作">
                <span className="batch-count mono">已选 {selectedIds.size} 项</span>
                <button
                  className="btn btn-tiny"
                  disabled={busy}
                  onClick={() => setTagPrompt({ ids: [...selectedIds] })}
                  title="给选中的素材打标签（T）"
                >
                  打标签
                </button>
                <button
                  className="btn btn-tiny"
                  disabled={busy}
                  onClick={(e) => {
                    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                    collPickMenu([...selectedIds], r.left, r.bottom + 6);
                  }}
                  title="把选中的素材加进某个分组（不搬文件）"
                >
                  加入分组
                </button>
                <button
                  className="btn btn-tiny"
                  disabled={busy}
                  onClick={() => setSnapPrompt({ mode: "ids", ids: [...selectedIds] })}
                  title="把选中的素材记进某份快照（不搬文件）"
                >
                  加入快照
                </button>
                <button
                  className="btn btn-tiny"
                  disabled={busy || !dir}
                  onClick={() => dir && void doMove(selPaths, dir.path)}
                  title="移动到当前浏览的文件夹"
                >
                  移到当前目录
                </button>
                <button
                  className="btn btn-tiny"
                  disabled={busy}
                  onClick={() => void onCopyFile(selPaths)}
                >
                  复制原文件
                </button>
                <button
                  className="btn btn-tiny btn-danger"
                  disabled={busy}
                  onClick={() => void doDelete(selPaths, `这 ${selectedIds.size} 项`)}
                >
                  删除
                </button>
                <button className="btn btn-tiny" onClick={clearSel} title="取消多选（Esc）">
                  取消选择
                </button>
              </div>
            )}

            {batch ? (
              <div className="progress-bar" role="status" aria-live="polite">
                <div className="progress-track">
                  <div
                    className="progress-fill"
                    style={{ width: `${Math.round((batch.done / Math.max(1, batch.total)) * 100)}%` }}
                  />
                </div>
                <span className="progress-text">
                  {batch.label ?? "批量处理"} · {fmtNum(batch.done)} / {fmtNum(batch.total)}
                </span>
              </div>
            ) : scan.active ? (
              <div className="progress-bar scan-strip" role="status" aria-live="polite">
                <div className="progress-track">
                  <div className="progress-fill indet" />
                </div>
                <span className="progress-text">
                  正在扫描 · {fmtNum(scan.dirs)} 目录 / {fmtNum(scan.files)} 文件
                </span>
                <button className="btn btn-tiny" onClick={() => void cancelScan()}>
                  取消
                </button>
              </div>
            ) : (
              loading && (
                <div className="progress-bar" role="status" aria-live="polite">
                  <div className="progress-track">
                    <div className="progress-fill indet" />
                  </div>
                  <span className="progress-text">载入中 · 已取 {items.length} 项</span>
                </div>
              )
            )}
          </div>
        </div>

        {/* 详情面板常驻（owner 2026-11-16）：不做收起，也不再绑定 showDetail 状态 */}
        <Detail
          asset={selected}
          source={source}
          thumbSrc={thumbSrc}
          thumbPx={THUMB_PX[sizeIndex] ?? 384}
          onReveal={onReveal}
          onRename={onRename}
          onPeek={openPeek}
          renaming={!!selected && renameId === selected.id}
          onStartRename={() => {
            if (selected) setRenameId(selected.id);
          }}
          onCommitRename={(name) => {
            if (selected) void commitRename(selected, name);
          }}
          onCancelRename={() => setRenameId(null)}
          onOpenDefault={onOpenDefault}
          onCopyFile={(a) => void onCopyFile([a.path])}
          onDelete={(a) => void doDelete([a.path], `「${a.name}」`)}
          onDragOut={onDragOut}
          onToast={(m, e) => toast(m, e)}
          tags={selected ? tagMap.get(selected.id) ?? [] : []}
          colors={tagColorMap}
          autoTags={prefs.autoTags ? selectedAuto.filter((a) => !denied.has(a.tag)) : []}
          allTags={tags.map((t) => t.name)}
          onAddTag={(t) => {
            if (selected) void applyTag(t, [selected.id]);
          }}
          onRemoveTag={(t) => {
            if (selected) void removeTag(t, [selected.id]);
          }}
          onPickTag={(t) => {
            setTagFilter(t);
            setDir(null);
          }}
          onDenyAutoTag={denyAutoTag}
          onEditTag={(name) => setTagEdit({ kind: "tag", name })}
          onSetRating={onSetRating}
          onSetNote={onSetNote}
          onSetSource={onSetSource}
          onClose={() => {
            setRenameId(null);
            setSelected(null);
          }}
        />
      </div>

      {tagEdit && (
        <TagEdit
          kind={tagEdit.kind}
          name={tagEdit.name}
          color={
            tagEdit.kind === "tag"
              ? tags.find((t) => t.name === tagEdit.name)?.color ?? null
              : tagGroups.find((g) => g.name === tagEdit.name)?.color ?? null
          }
          group={tagEdit.kind === "tag" ? tags.find((t) => t.name === tagEdit.name)?.group ?? null : null}
          groups={tagGroups}
          onRename={(next) => void renameEntity(next)}
          onColor={(c) => void editColor(c)}
          onGroup={(g) => void moveTagToGroup(g)}
          onDelete={() => void deleteEntity()}
          onClose={() => setTagEdit(null)}
          rule={tagRule}
          onRule={(r) => {
            const name = tagEdit.name;
            void source
              .tagSetRule(name, r)
              .then(async () => {
                setTagRuleState(r);
                setRefreshKey((k) => k + 1);
                toast(r ? "规则已保存（计数即时生效）" : "规则已清除");
              })
              .catch((e) => toast(String(e), true));
          }}
        />
      )}

      {tagPrompt && (
        <TagPrompt
          targets={tagPrompt.ids.length}
          existing={tags.map((t) => t.name)}
          onApply={(t) => {
            const ids = tagPrompt.ids;
            setTagPrompt(null);
            void applyTag(t, ids);
          }}
          onClose={() => setTagPrompt(null)}
        />
      )}

      {snapPrompt && (
        <SnapshotPrompt
          snapshots={snaps}
          {...snapPromptProps(snapPrompt, snaps)}
          onPick={(id) => {
            const p = snapPrompt;
            if (p.mode === "tag") void addTagToSnapshot(id, p.tag);
            else if (p.mode === "ids") void addIdsToSnapshot(id, p.ids);
            else void combineSnapshots(p.srcId, id, p.mode === "diff" ? "diff" : "union");
          }}
          onName={(name) => void createAndTagSnapshot(name)}
          onClose={() => setSnapPrompt(null)}
        />
      )}

      {current && (
        <Lightbox
          asset={current}
          index={lightbox ?? 0}
          total={items.length}
          thumbSrc={thumbSrc}
          navRef={lightNav}
          onClose={() => setLightbox(null)}
          onPrev={() => setLightbox((i) => (i !== null && i > 0 ? i - 1 : i))}
          onNext={() => setLightbox((i) => (i !== null && i + 1 < items.length ? i + 1 : i))}
        />
      )}

      {ctx && <ContextMenu state={ctx} onClose={() => setCtx(null)} />}

      <CommandPalette open={cmdkOpen} onClose={() => setCmdkOpen(false)} commands={cmds} />

      <Shortcuts open={shortcutsOpen} onClose={() => setShortcutsOpen(false)} />

      <Tools
        open={!!tools}
        tab={tools?.tab ?? "dupes"}
        ids={[...selectedIds]}
        source={source}
        thumbSrc={source.thumb}
        onClose={() => setTools(null)}
        onToast={toast}
        onPick={(ids) => {
          setTools(null);
          pickMany(ids, false);
        }}
      />

      <Settings
        open={settingsOpen}
        initialSection={bootSection}
        prefs={prefs}
        source={source}
        onApply={applyPrefs}
        onClose={() => setSettingsOpen(false)}
        onToast={(m, e) => toast(m, e)}
        onResetDenied={() => {
          setDenied(new Set());
          saveDenied(new Set());
          toast("已恢复全部自动标签建议");
        }}
        onAddFolder={onAddFolder}
        onAddPath={(p) => onAddPath(p)}
        onRemoveLib={onRemoveLib}
      />

      {dlg.node}
      <Toaster items={toasts} />
      <HoverHintHost />
    </div>
  );
}

/* ---------------------------------------------------------------- 大图预览 */

/**
 * 灯箱（T16）：滚轮缩放 1×–8×、放大后拖动平移、双击复位。
 *
 * 两层图（T07）：底下先铺**已经缓存好的 512 档**（详情面板里那张，已在内存/磁盘缓存里），
 * 上面再加载 1600 档，`onLoad` 后淡入。这样打开大图不会先白一片再突然出现 ——
 * 1600 档对百兆 PNG 要 0.5 秒，等这 0.5 秒的时间用户看的是清晰的 512，不是空白。
 */
function Lightbox({
  asset,
  index,
  total,
  thumbSrc,
  navRef,
  onClose,
  onPrev,
  onNext,
}: {
  asset: Asset;
  index: number;
  total: number;
  thumbSrc: (a: Asset, px: number) => string;
  /** 注册换图入口：App 的方向键走这里，带逐帧闸门（T37） */
  navRef?: React.MutableRefObject<((dir: number) => void) | null>;
  onClose: () => void;
  onPrev: () => void;
  onNext: () => void;
}) {
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [loaded, setLoaded] = useState<string | null>(null);
  const drag = useRef<{ x: number; y: number; px: number; py: number } | null>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);

  const hi = thumbSrc(asset, 1600);
  const lo = thumbSrc(asset, 512);

  // 换图 → 缩放与位移复位（否则下一张会以 4 倍放大出现在角落里）
  useEffect(() => {
    setZoom(1);
    setPan({ x: 0, y: 0 });
  }, [asset.id]);

  const clampZoom = (z: number) => Math.min(8, Math.max(1, z));

  /* ---------------- T37 连续切图：逐帧闸门 ----------------
     按住 ←/→ 时浏览器会连发 keydown（每秒 20~30 次），旧实现是"每来一次就换一张"，
     于是慢图（大 PNG）一张都出不来，最后停在随机某张上 —— 用户看到的是"连按几下就没反应"。
     现在照果核的 `nav_frame_gate`：方向先攒进队列（最多领先 2 张），
     **当前这张真的出过一帧**才放下一张。缓存里已有的图直接放行，不会变慢。 */
  const painted = useRef<Set<number>>(new Set());
  const [frameReady, setFrameReady] = useState(false);
  const readyRef = useRef(false);
  readyRef.current = frameReady;
  const queue = useRef(0);
  const nextRef = useRef(onNext);
  nextRef.current = onNext;
  const prevRef = useRef(onPrev);
  prevRef.current = onPrev;

  const pump = useCallback(() => {
    if (!queue.current || !readyRef.current) return;
    const d = queue.current;
    queue.current = 0;
    if (d > 0) nextRef.current();
    else prevRef.current();
  }, []);

  useEffect(() => {
    if (!navRef) return;
    navRef.current = (dir: number) => {
      queue.current = Math.max(-2, Math.min(2, queue.current + dir));
      pump();
    };
    return () => {
      if (navRef) navRef.current = null;
    };
  }, [navRef, pump]);

  useEffect(() => {
    const known = painted.current.has(asset.id);
    setFrameReady(known);
    if (known) return;
    /* 兜底：坏图 / 极慢的图 500ms 后也放行，否则闸门等于把键盘锁死 */
    const t = window.setTimeout(() => setFrameReady(true), 500);
    return () => window.clearTimeout(t);
  }, [asset.id]);

  useEffect(() => {
    pump();
  }, [frameReady, pump]);

  /* ---------------- T40 鸟瞰图（长图 / 放大后定位） ----------------
     用**实际布局矩形**算可见比例，不自己推 pan/zoom 的数学：
     缩放锚定、contain 适配、窗口 resize 全都自动对上。 */
  const [nav, setNav] = useState<{ x: number; y: number; w: number; h: number } | null>(null);
  const [overflow, setOverflow] = useState(false);

  /* 图片**内容**在屏幕上的矩形。
     注意不能直接用 `<img>` 的 getBoundingClientRect —— 它是 width/height 100% 铺满舞台、
     靠 `object-fit: contain` 缩内容，元素框永远等于舞台，量出来"永远不溢出"。
     这里按 naturalWidth/Height 自己算 contain 后的尺寸，再叠 zoom/pan。 */
  const contentBox = useCallback(() => {
    const stage = stageRef.current;
    const im = imgRef.current;
    if (!stage || !im) return null;
    const sr = stage.getBoundingClientRect();
    const nw = im.naturalWidth || asset.width || 0;
    const nh = im.naturalHeight || asset.height || 0;
    if (!nw || !nh || !sr.width || !sr.height) return null;
    const fit = Math.min(sr.width / nw, sr.height / nh);
    const cw = nw * fit * zoom;
    const ch = nh * fit * zoom;
    return {
      sr,
      cw,
      ch,
      left: sr.left + sr.width / 2 + pan.x - cw / 2,
      top: sr.top + sr.height / 2 + pan.y - ch / 2,
    };
  }, [zoom, pan.x, pan.y, asset.width, asset.height]);

  const measure = useCallback(() => {
    const b = contentBox();
    if (!b) return;
    const { sr, cw, ch, left, top } = b;
    setOverflow(cw > sr.width + 1 || ch > sr.height + 1);
    const fx = Math.max(0, Math.min(1, (sr.left - left) / cw));
    const fy = Math.max(0, Math.min(1, (sr.top - top) / ch));
    setNav({
      x: fx,
      y: fy,
      w: Math.min(1 - fx, sr.width / cw),
      h: Math.min(1 - fy, sr.height / ch),
    });
  }, [contentBox]);

  useEffect(() => {
    measure();
  }, [measure, zoom, pan, loaded, asset.id]);

  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => measure());
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  /* 拖鸟瞰图 = 直接定位：把"想看的那个点"移到舞台正中 */
  const miniDrag = useRef(false);
  const jumpTo = (e: React.PointerEvent) => {
    const b = contentBox();
    const mini = miniRef.current;
    if (!b || !mini) return;
    const mr = mini.getBoundingClientRect();
    if (!mr.width || !mr.height) return;
    const fx = Math.max(0, Math.min(1, (e.clientX - mr.left) / mr.width));
    const fy = Math.max(0, Math.min(1, (e.clientY - mr.top) / mr.height));
    /* 目标：图像上的 (fx,fy) 这一点落在舞台中心 → 反推需要补的位移 */
    const dx = b.sr.left + b.sr.width / 2 - fx * b.cw - b.left;
    const dy = b.sr.top + b.sr.height / 2 - fy * b.ch - b.top;
    setPan((p) => ({ x: p.x + dx, y: p.y + dy }));
  };
  const miniRef = useRef<HTMLDivElement>(null);

  /* 滚轮缩放必须用**原生非 passive** 监听。
     React 的 onWheel 走的是根节点委托，Chrome 默认 passive:true，
     里面调 preventDefault 只会得到一句 "Unable to preventDefault inside passive event
     listener invocation" 然后整页跟着滚 —— 灯箱里滚一下，背后网格也滚走了。
     用 zoomRef 拿最新值，监听只装一次。 */
  const zoomRef = useRef(1);
  zoomRef.current = zoom;
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const h = (e: WheelEvent) => {
      e.preventDefault();
      const z = zoomRef.current;
      const next = clampZoom(z * (e.deltaY < 0 ? 1.15 : 1 / 1.15));
      if (next === z) return;
      const host = el.getBoundingClientRect();
      if (next > 1) {
        // 以指针为中心缩放：不然放大后想看的地方会跑出视口
        const cx = e.clientX - host.left - host.width / 2;
        const cy = e.clientY - host.top - host.height / 2;
        const k = next / z;
        setPan((p) => ({ x: cx - (cx - p.x) * k, y: cy - (cy - p.y) * k }));
      } else {
        setPan({ x: 0, y: 0 });
      }
      setZoom(next);
    };
    el.addEventListener("wheel", h, { passive: false });
    return () => el.removeEventListener("wheel", h);
  }, []);

  const onDown = (e: React.MouseEvent) => {
    if (zoom <= 1) return;
    drag.current = { x: e.clientX, y: e.clientY, px: pan.x, py: pan.y };
  };
  const onMove = (e: React.MouseEvent) => {
    const d = drag.current;
    if (!d) return;
    setPan({ x: d.px + (e.clientX - d.x), y: d.py + (e.clientY - d.y) });
  };
  const endDrag = () => {
    drag.current = null;
  };

  const reset = () => {
    setZoom(1);
    setPan({ x: 0, y: 0 });
  };

  /* 鸟瞰图尺寸：按原图比例，长边最多 132 CSS px（够看清，又不挡图） */
  const miniW = asset.width && asset.height
    ? Math.round((asset.width >= asset.height ? 132 : (132 * asset.width) / asset.height))
    : 96;
  const miniH = asset.width && asset.height
    ? Math.round((asset.height >= asset.width ? 132 : (132 * asset.height) / asset.width))
    : 72;
  const showMini = overflow && !!nav;

  return (
    <div className="lightbox" onClick={onClose}>
      <div className="lightbox-panel" onClick={(e) => e.stopPropagation()}>
        <div className="lightbox-bar">
          <Icon name="image" size={13} />
          <span className="lightbox-name">{asset.name}</span>
          <span className="mono lightbox-meta">
            {asset.width && asset.height ? `${asset.width} × ${asset.height}` : ""}
            {zoom > 1 && <> · {zoom.toFixed(1)}×</>}
            {" · "}
            {index + 1}/{total}
          </span>
          <button className="tb-action" onClick={onPrev} title="上一张（←）" disabled={index <= 0}>
            <Icon name="chevron" size={13} className="rot180" />
          </button>
          <button
            className="tb-action"
            onClick={onNext}
            title="下一张（→）"
            disabled={index >= total - 1}
          >
            <Icon name="chevron" size={13} />
          </button>
          <button className="tb-action" onClick={onClose} title="关闭（Esc）">
            <Icon name="close" size={13} />
          </button>
        </div>
        <div
          className={`lightbox-stage ${zoom > 1 ? "zoomed" : ""}`}
          ref={stageRef}
          onMouseDown={onDown}
          onMouseMove={onMove}
          onMouseUp={endDrag}
          onMouseLeave={endDrag}
          onDoubleClick={reset}
          title={zoom > 1 ? "拖动平移 · 双击复位 · 右侧鸟瞰图可拖动定位" : "滚轮放大 · 双击复位"}
        >
          {loaded !== hi && <img className="lightbox-img ghost" src={lo} alt="" draggable={false} aria-hidden />}
          <img
            ref={imgRef}
            className={`lightbox-img ${loaded === hi ? "in" : "out"}`}
            src={hi}
            alt=""
            draggable={false}
            style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }}
            onLoad={() => {
              painted.current.add(asset.id);
              setLoaded(hi);
              setFrameReady(true);
              measure();
            }}
            onDoubleClick={reset}
          />
        </div>

        {/* T43 边缘导航：光标到左右边缘才浮出来（无边框看图时最需要它） */}
        <div className="lb-edge l" onClick={(e) => e.stopPropagation()}>
          <button
            className="lb-edge-btn"
            title="上一张（←）"
            disabled={index <= 0}
            onClick={onPrev}
          >
            <Icon name="chevron" size={20} className="rot180" />
          </button>
        </div>
        <div className="lb-edge r" onClick={(e) => e.stopPropagation()}>
          <button
            className="lb-edge-btn"
            title="下一张（→）"
            disabled={index >= total - 1}
            onClick={onNext}
          >
            <Icon name="chevron" size={20} />
          </button>
        </div>

        {/* T40 鸟瞰图：只在图片比舞台大的时候出现（否则没有"定位"可言） */}
        {showMini && (
          <div
            className="lb-mini scroll-hot"
            ref={miniRef}
            style={{ width: miniW, height: miniH }}
            onClick={(e) => e.stopPropagation()}
            onPointerDown={(e) => {
              e.stopPropagation();
              miniDrag.current = true;
              (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
              jumpTo(e);
            }}
            onPointerMove={(e) => {
              if (!miniDrag.current) return;
              e.stopPropagation();
              jumpTo(e);
            }}
            onPointerUp={(e) => {
              miniDrag.current = false;
              (e.target as HTMLElement).releasePointerCapture?.(e.pointerId);
            }}
            title="拖动定位"
          >
            <img className="lb-mini-img" src={lo} alt="" draggable={false} />
            <div
              className="lb-mini-view"
              style={{
                left: `${nav!.x * 100}%`,
                top: `${nav!.y * 100}%`,
                width: `${Math.max(4, nav!.w * 100)}%`,
                height: `${Math.max(4, nav!.h * 100)}%`,
              }}
            />
          </div>
        )}
      </div>
    </div>
  );
}
