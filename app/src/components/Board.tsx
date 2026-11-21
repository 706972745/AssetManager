import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ClipboardPaste, LayoutGrid, Layers, Maximize2, Minus, Plus, Type, X } from "lucide-react";
import { source } from "../data/source";
import type { BoardNote } from "../types";
import { ContextMenu, type CtxItem } from "./ContextMenu";
import { createShiftWheelPan } from "../lib/windowShiftPan";
import { isOwnDrag } from "../lib/ownDrag";

/**
 * 灵感板（照 PureRef）：把一个标签下的所有素材铺在一块**无边界画布**上。
 *
 * 三条手感约定（和 PureRef 一致）：
 * - 空白处左键拖 / 中键拖 = 平移画布；滚轮 = 缩放（锚定鼠标，指针下的那张图不动）
 * - 左键拖某张图 = 移动它；拖右下角手柄 = **改它的大小**（保持比例）；双击 = 贴图
 * - 画布没有边界，坐标可以是任意大/小（不夹紧，也不回弹）
 *
 * 0.4.14 起多三件事：
 * - **文字便签**：`T` 或顶栏「文字」在视口中心放一张，双击改、Esc 存、清空即删；
 *   位置和图片一样可拖，落在 `board_notes`（按板的键存，换机器就没了，和摆位同性质）。
 * - **一键整理**：按当前视觉顺序（从上到下、同一行从左到右）重新瀑布流排一遍并落库。
 * - **动图悬停才动**：板上永远是 1024 档静态缩略图（GIF 的首帧），
 *   指针停上去才把 **原文件**（`px=0`）叠上去播；离开立刻拆掉 —— 不悬停就不耗 CPU。
 *
 * 「什么能上板」：**不看格式，只看软件能不能把它变成图**。JPG/PNG/WebP/AVIF/HEIC 自不必说，
 * PSD/PSB/AI/EPS/SVG/WMF/EMF、全部 RAW、PDF 都走同一条 `thumb://` 出图管线
 * （内嵌预览或系统缩略图），能出图就上板。真出不了（.txt / .json / 损坏文件）
 * 也不丢：留一张占位卡写着文件名，位置和手感跟真图一模一样。
 *
 * 性能上只有一条铁律：**画布变换绝不走 React state**。
 * 2000 张图每帧 re-render 一次必卡；所以平移/缩放都是直接改 `transform`（ref + DOM），
 * 只有"哪几张在视口里"会 setState（rAF 节流），其余的连 `<img>` 都不挂 —— 不挂就不解码。
 */
interface BoardItem {
  id: number;
  src: string;
  w: number;
  h: number;
  name: string;
  path: string;
  /** 动图：默认静止，悬停才播 `raw` */
  anim?: boolean;
  /** 原文件地址（`px=0`）：动图悬停时换成它 */
  raw?: string;
}

interface BoardBoot {
  label?: string;
  /** 摆位的键（`tag:<标签>` / `snap:<id>` / `coll:<id>`）：同一个集合下次开板还摆回原样 */
  board?: string;
  /** 标签板：这块板对应哪个标签（Ctrl+V 打的就是这个标签） */
  tag?: string;
  /** 快照板：这块板对应哪份快照（Ctrl+V 加进这个快照） */
  snapshot?: { id: number; name: string };
  /** 分组板：这块板对应哪个分组（Ctrl+V 加进这个分组；只建关联，文件不动） */
  coll?: { id: number; name: string };
  title: string;
  items: BoardItem[];
  more?: boolean;
  total?: number;
  /** 快照板专用：快照里有、但这次没找到的条数（标题上要说出来） */
  missing?: number;
}

function bootData(): BoardBoot | null {
  const b = (window as unknown as { __AM_BOARD__?: BoardBoot }).__AM_BOARD__;
  if (!b) return null;
  // 快照板可能**一张都没对上**（硬盘没插）：那也要开板并说明，不能当成"数据丢失"
  if (!b.tag && !b.snapshot && !b.coll) return null;
  return b;
}

/* ---------------- 初始排布（瀑布流） ---------------- */

const CW = 240; // 列宽（画布单位）
const GAP = 18;
/** 视口外多留一圈：滚过去时图已经在解码了，不会"一块一块跳出来" */
const MARGIN = 600;
const MIN_W = 40;
const MAX_W = 2400;
/** 便签默认尺寸（画布单位）。宽度固定，高度由内容撑开，只作为落库/可见性判断的估计值 */
const NOTE_W = 240;
const NOTE_H = 120;
/** 左侧图层面板宽度（画布单位 = CSS px）。**改这里要同步 `.board-layers` 的 width**：
 *  fit / 居中要靠它把"看得见的那块"往右偏，否则内容会被面板压住一截。 */
const LAYERS_W = 208;

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

function layout(items: BoardItem[]): { boxes: Box[]; width: number; height: number } {
  // 空板（新建的标签板）没有可摆的东西：包围盒给 0，交给 fit() 居中
  if (!items.length) return { boxes: [], width: 0, height: 0 };
  const cols = Math.max(3, Math.min(10, Math.ceil(Math.sqrt(items.length))));
  const heights = new Array(cols).fill(0);
  const boxes: Box[] = [];
  for (const it of items) {
    // 比例未知（没读到宽高）按 4:3 摆，比正方形更接近真实图片
    const a = it.w > 0 && it.h > 0 ? it.w / it.h : 4 / 3;
    const w = CW;
    const h = Math.max(60, Math.min(CW * 4, Math.round(CW / a)));
    // 塞进当前最矮的那一列 —— 标准瀑布流，视觉上最"随便摆"
    let c = 0;
    for (let i = 1; i < cols; i++) if (heights[i] < heights[c]) c = i;
    boxes.push({ x: c * (CW + GAP), y: heights[c], w, h });
    heights[c] += h + GAP;
  }
  return {
    boxes,
    width: cols * (CW + GAP) - GAP,
    height: Math.max(...heights) - GAP,
  };
}

export function Board() {
  const boot = useMemo(bootData, []);
  const [items, setItems] = useState<BoardItem[]>(() => boot?.items ?? []);
  /* 文字便签：坐标同样住在 `pos`（key = `-nid`），这里的 state 只负责文字与渲染 */
  const [notes, setNotes] = useState<BoardNote[]>([]);
  const notesRef = useRef<BoardNote[]>([]);
  /** 正在编辑的那张便签（nid）；null = 没有 */
  const [editId, setEditId] = useState<number | null>(null);
  /* 左侧图层面板（0.4.15）：列出板上**所有**图片，点一行 = 置顶 + 视口居中。
     面板默认展开，但只在展开时才渲染列表（2000 张的板不打开就不付这份 DOM 成本）。 */
  const [layersOpen, setLayersOpen] = useState(true);
  /** 当前**最顶层**的那张。只记一张 —— 需求是"最近点哪张哪张在上面"，不是 PS 那种层叠序 */
  const [topId, setTopId] = useState<number | null>(null);
  const { boxes, width: cw, height: ch } = useMemo(() => layout(items), [items]);
  const boxById = useMemo(() => {
    const m = new Map<number, Box>();
    items.forEach((it, i) => m.set(it.id, boxes[i]));
    return m;
  }, [items, boxes]);

  const rootRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  /** 每张图当前的位置与尺寸（画布坐标）。拖动时只改这张的 DOM，松手才写回这里 */
  const pos = useRef<Map<number, Box>>(new Map());
  const vp = useRef({ x: 0, y: 0, k: 1 });
  const [zoom, setZoom] = useState(1);
  const [vis, setVis] = useState<Set<number>>(new Set());
  const [broken, setBroken] = useState<Set<number>>(new Set());
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    id: number | null;
    /** 右键落在便签上：nid */
    note?: number;
  } | null>(null);
  const [active, setActive] = useState<number | null>(null); // 悬停/拖动中的那张：显示缩放手柄
  /* 操作反馈（粘贴成功/失败）：画布上没有 toast，横幅也不该常驻，就一条两秒半的提示 */
  const [hint, setHint] = useState<{ text: string; bad: boolean } | null>(null);

  useEffect(() => {
    notesRef.current = notes;
  }, [notes]);

  /* Shift + 滚轮 = 挪**窗口**在屏幕上的位置（owner 2026-11-19）。
     它自己管 rAF 合帧与窗口几何缓存，所以整块板只建一次；挂载/卸载跟着组件走。 */
  const winPan = useMemo(
    () =>
      createShiftWheelPan({
        onUnavailable: () =>
          setHint({ text: "Shift+滚轮 移动窗口不可用（当前窗口缺少窗口权限）", bad: true }),
      }),
    []
  );
  /* StrictMode 会「挂载→清理→再挂载」：这里只取消挂起的一帧，控制器本身仍然可用
     （详见 windowShiftPan 里 `cancel` 的注释）。 */
  useEffect(() => () => winPan.cancel(), [winPan]);

  const at = (id: number): Box => pos.current.get(id) ?? boxById.get(id) ?? { x: 0, y: 0, w: CW, h: CW };

  /* ---------------- 画布变换（不走 React） ---------------- */

  const applyVp = useCallback(() => {
    const c = canvasRef.current;
    if (!c) return;
    const { x, y, k } = vp.current;
    c.style.transform = `translate3d(${x}px, ${y}px, 0) scale(${k})`;
  }, []);

  /** 视口可见集合：只让视口内的图挂 `<img>`（不挂就不解码） */
  const recompute = useCallback(() => {
    const root = rootRef.current;
    if (!root) return;
    const { x, y, k } = vp.current;
    const x0 = -x / k - MARGIN;
    const y0 = -y / k - MARGIN;
    const x1 = (root.clientWidth - x) / k + MARGIN;
    const y1 = (root.clientHeight - y) / k + MARGIN;
    const out = new Set<number>();
    for (const it of items) {
      const b = at(it.id);
      if (b.x + b.w >= x0 && b.x <= x1 && b.y + b.h >= y0 && b.y <= y1) out.add(it.id);
    }
    setVis((old) => (old.size === out.size && [...out].every((v) => old.has(v)) ? old : out));
  }, [items, boxById]);

  /** 按给定包围盒 fit 进窗口并居中（`fit` 和「一键整理」共用：整理时新包围盒还没进 state） */
  const fitTo = useCallback(
    (w: number, h: number) => {
      const root = rootRef.current;
      if (!root) return;
      /* 图层面板占了左边一条：可见区从 `pad` 开始，否则 fit 之后内容被面板压住一截 */
      const pad = layersOpen ? LAYERS_W : 0;
      const W = (root.clientWidth || 1200) - pad;
      const H = root.clientHeight || 800;
      // 空板：包围盒是 0，直接让画布原点落在可见区中心（之后粘进来的图就落在视口正中）
      if (!(w > 0) || !(h > 0)) {
        vp.current = { k: 1, x: pad + W / 2, y: H / 2 };
        applyVp();
        setZoom(1);
        recompute();
        return;
      }
      const k = Math.min(1, (W * 0.92) / w, (H * 0.92) / h);
      vp.current = { k, x: pad + (W - w * k) / 2, y: (H - h * k) / 2 };
      applyVp();
      setZoom(k);
      recompute();
    },
    [applyVp, recompute, layersOpen]
  );

  /** 按内容包围盒 fit 进窗口并居中 */
  const fit = useCallback(() => fitTo(cw, ch), [fitTo, cw, ch]);

  /** 把某张图挪到视口正中（装不下才缩）。图层面板点一行走这里 */
  const focusItem = useCallback(
    (id: number) => {
      const root = rootRef.current;
      if (!root) return;
      const pad = layersOpen ? LAYERS_W : 0;
      const W = (root.clientWidth || 1200) - pad;
      const H = root.clientHeight || 800;
      const b = pos.current.get(id) ?? boxById.get(id) ?? { x: 0, y: 0, w: CW, h: CW };
      const v = vp.current;
      /* 只在"这张装不下"时才改 k：用户当前缩放手感不该被一次点击重置 */
      const need = Math.min(1, (W * 0.86) / Math.max(1, b.w), (H * 0.86) / Math.max(1, b.h));
      const k = need < v.k ? Math.max(0.04, need) : v.k;
      v.k = k;
      v.x = pad + W / 2 - (b.x + b.w / 2) * k;
      v.y = H / 2 - (b.y + b.h / 2) * k;
      applyVp();
      setZoom(k);
      /* 居中后这张可能刚进视口 —— 立刻重算可见集，它才会被挂成 `<img>` */
      recompute();
    },
    [applyVp, recompute, boxById, layersOpen]
  );

  const reset100 = useCallback(() => {
    vp.current = { x: 0, y: 0, k: 1 };
    applyVp();
    setZoom(1);
    recompute();
  }, [applyVp, recompute]);

  /** 以窗口中心为锚点缩一档（顶栏 +/- 用） */
  const zoomBy = useCallback(
    (factor: number) => {
      const v = vp.current;
      const k2 = Math.min(4, Math.max(0.04, v.k * factor));
      const r = rootRef.current;
      const mx = (r?.clientWidth ?? 800) / 2;
      const my = (r?.clientHeight ?? 600) / 2;
      v.x = mx - (mx - v.x) * (k2 / v.k);
      v.y = my - (my - v.y) * (k2 / v.k);
      v.k = k2;
      applyVp();
      setZoom(k2);
      recompute();
    },
    [applyVp, recompute]
  );

  useEffect(() => {
    document.body.classList.add("board-body");
    document.documentElement.classList.add("board-body");
    return () => {
      document.body.classList.remove("board-body");
      document.documentElement.classList.remove("board-body");
    };
  }, []);

  /* 第一次摆：**先读上次的摆位**，没记过的才用瀑布流；然后 fit 一次。
     **之后再增删条目都不许 fit** —— 粘一张图就把整块画布重缩一遍，等于把用户手摆的位置全打乱。 */
  const fitted = useRef(false);
  /** 摆位读回来没有：没读回来之前不 fit，否则会先按瀑布流 fit 一次、再跳一下 */
  const layoutReady = useRef(false);
  useEffect(() => {
    const board = boot?.board;
    if (!board) {
      layoutReady.current = true;
      return;
    }
    let alive = true;
    void Promise.all([
      source.boardLayout(board).catch(() => [] as [number, number, number, number, number][]),
      source
        .boardNotes(board)
        .catch(() => [] as [number, number, number, number, number, string][]),
    ]).then(([rows, ns]) => {
      if (!alive) return;
      for (const [g, x, y, w, h] of rows) pos.current.set(g, { x, y, w, h });
      /* 便签的坐标进同一个 `pos`，key 用 `-nid`：图片 id 是正数，两边永不相撞 */
      const list: BoardNote[] = ns.map(([nid, x, y, w, h, text]) => ({ nid, x, y, w, h, text }));
      for (const n of list) pos.current.set(-n.nid, { x: n.x, y: n.y, w: n.w, h: n.h });
      notesRef.current = list;
      setNotes(list);
      layoutReady.current = true;
      /* 读完之后重排一次：这一帧 items 已经从 boot 来了，直接把 DOM 位置改对 */
      setItems((old) => [...old]);
    });
    return () => {
      alive = false;
    };
  }, [boot?.board]);

  /** 把一张图的位置/尺寸记回库（松手、缩放单张时调） */
  const remember = useCallback(
    (id: number) => {
      const board = boot?.board;
      const b = pos.current.get(id);
      if (!board || !b) return;
      void source
        .boardPlace(board, [[id, Math.round(b.x), Math.round(b.y), Math.round(b.w), Math.round(b.h)]])
        .catch(() => {});
    },
    [boot?.board]
  );

  /** 便签的位置记回库。文字从 `notesRef` 取 —— settle 里那个闭包不跟着 notes 重挂 */
  const noteRemember = useCallback(
    (nid: number) => {
      const board = boot?.board;
      const b = pos.current.get(-nid);
      if (!board || !b) return;
      const text = notesRef.current.find((n) => n.nid === nid)?.text ?? "";
      void source
        .boardNotePut(
          board,
          nid,
          Math.round(b.x),
          Math.round(b.y),
          Math.round(b.w),
          Math.round(b.h),
          text
        )
        .catch(() => {});
    },
    [boot?.board]
  );

  /* ---------------- 文字便签（0.4.14） ---------------- */

  /** 在视口正中放一张空便签，并立刻进入编辑 */
  const addNote = useCallback(async () => {
    const board = boot?.board;
    if (!board) return;
    const root = rootRef.current;
    const W = root?.clientWidth ?? 1200;
    const H = root?.clientHeight ?? 800;
    const v = vp.current;
    const cx = (W / 2 - v.x) / v.k;
    const cy = (H / 2 - v.y) / v.k;
    const x = Math.round(cx - NOTE_W / 2);
    const y = Math.round(cy - NOTE_H / 2);
    try {
      const nid = await source.boardNotePut(board, null, x, y, NOTE_W, NOTE_H, "");
      pos.current.set(-nid, { x, y, w: NOTE_W, h: NOTE_H });
      notesRef.current = [...notesRef.current, { nid, x, y, w: NOTE_W, h: NOTE_H, text: "" }];
      setNotes(notesRef.current);
      setEditId(nid);
    } catch (e) {
      setHint({ text: String(e), bad: true });
    }
  }, [boot?.board]);

  /** 结束编辑：有字就存，**一个字都没有就删**（空便签留在板上没有意义） */
  const commitNote = useCallback(
    (nid: number, raw: string) => {
      setEditId((cur) => (cur === nid ? null : cur));
      const board = boot?.board;
      const b = pos.current.get(-nid) ?? { x: 0, y: 0, w: NOTE_W, h: NOTE_H };
      const text = raw.replace(/\s+$/, "");
      if (!text.trim()) {
        notesRef.current = notesRef.current.filter((n) => n.nid !== nid);
        setNotes(notesRef.current);
        pos.current.delete(-nid);
        if (board) void source.boardNoteDel(board, nid).catch(() => {});
        return;
      }
      notesRef.current = notesRef.current.map((n) => (n.nid === nid ? { ...n, text } : n));
      setNotes(notesRef.current);
      if (board)
        void source
          .boardNotePut(
            board,
            nid,
            Math.round(b.x),
            Math.round(b.y),
            Math.round(b.w),
            Math.round(b.h),
            text
          )
          .catch(() => {});
    },
    [boot?.board]
  );

  const delNote = useCallback(
    (nid: number) => {
      notesRef.current = notesRef.current.filter((n) => n.nid !== nid);
      setNotes(notesRef.current);
      pos.current.delete(-nid);
      setEditId((cur) => (cur === nid ? null : cur));
      const board = boot?.board;
      if (board) void source.boardNoteDel(board, nid).catch(() => {});
    },
    [boot?.board]
  );

  /* ---------------- 一键整理 ---------------- */

  /** 按**当前视觉顺序**重排成瀑布流并落库。便签不参与 —— 它们通常是贴在某张图旁边的标注 */
  const tidy = useCallback(() => {
    if (!items.length) return;
    const box = (id: number): Box => pos.current.get(id) ?? boxById.get(id) ?? { x: 0, y: 0, w: CW, h: CW };
    const order = [...items].sort((a, b) => {
      const A = box(a.id);
      const B = box(b.id);
      // 先按行（80 画布单位一带），行内再按 x —— 近似"从上到下、从左到右"的阅读序
      const ra = Math.round(A.y / 80);
      const rb = Math.round(B.y / 80);
      return ra - rb || A.x - B.x;
    });
    const { boxes: bx, width, height } = layout(order);
    const rows: [number, number, number, number, number][] = [];
    order.forEach((it, i) => {
      const b = { ...bx[i] };
      pos.current.set(it.id, b);
      rows.push([it.id, Math.round(b.x), Math.round(b.y), Math.round(b.w), Math.round(b.h)]);
    });
    // 顺序写回 state：再点一次「整理」时起点就是这次的阅读序
    setItems(order);
    if (boot?.board) void source.boardPlace(boot.board, rows).catch(() => {});
    fitTo(width, height);
    setHint({ text: `已整理 ${order.length} 张`, bad: false });
  }, [items, boxById, boot?.board, fitTo]);

  useEffect(() => {
    items.forEach((it, i) => {
      if (!pos.current.has(it.id)) pos.current.set(it.id, { ...boxes[i] });
    });
    if (!fitted.current && layoutReady.current) {
      fitted.current = true;
      fit();
    } else {
      recompute();
    }
  }, [items, boxes, fit, recompute]);

  useEffect(() => {
    const ro = new ResizeObserver(() => recompute());
    if (rootRef.current) ro.observe(rootRef.current);
    return () => ro.disconnect();
  }, [recompute]);

  useEffect(() => {
    if (!hint) return;
    const t = window.setTimeout(() => setHint(null), 2600);
    return () => window.clearTimeout(t);
  }, [hint]);

  /* ---------------- 平移 / 拖图 / 缩放单张 ---------------- */

  /**
   * 平移画布的手势。`mask` 是**按下时那个键的 `MouseEvent.buttons` 位**（左键 1、中键 4）：
   * 中键拖也是平移（见文件头的三条手感约定），所以不能一律按"左键还在不在"判断。
   */
  const pan = useRef<{
    sx: number;
    sy: number;
    ox: number;
    oy: number;
    mask: number;
  } | null>(null);
  const drag = useRef<{
    kind: "move" | "size";
    id: number;
    sx: number;
    sy: number;
    box: Box;
    /** 拖的是便签（`id` 是 `-nid`）：收尾要写 board_notes，不写 board_items */
    note?: boolean;
    /** 拖图只认左键（下面的 `e.button === 0` 已限定），所以恒为 1 —— 见 onDown 里的说明 */
    mask: number;
  } | null>(null);
  const tick = useRef(0);
  /** Alt+滚轮改大小的落库定时器（停止滚动 400ms 后写一次） */
  const sizeSave = useRef(0);

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;

    const onWheel = (e: WheelEvent) => {
      /* 左侧图层面板里的滚轮 = 滚那个列表，不是缩放画布。
         必须在 `preventDefault` **之前**放行，否则列表滚不动。 */
      const tgt = e.target instanceof HTMLElement ? e.target : null;
      if (tgt?.closest(".board-layers")) return;
      e.preventDefault();
      /* Shift + 滚轮 = **把这个窗口挪到屏幕上别的位置**（owner 原话："按住 shift 滚轮
         不松，可以随意拖动窗口到任意位置"）。它既不是缩放画布、也不是平移画布内容，
         所以必须在最前面截住 —— 不按 Shift 的滚轮继续走下面的缩放，两个行为不许混。
         一次滚一格窗口移动约 60px，连续滚就连续移；边界由 `windowShiftPan` 夹在
         当前显示器工作区内（不会推到任务栏底下抓不回来）。 */
      if (e.shiftKey) {
        winPan.handle(e);
        return;
      }

      const rect = root.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;
      const v = vp.current;

      /* Alt+滚轮 = 只缩放**指针下那张**（PureRef 的同款操作，改大小不用去抓手柄）。
         锚点是这张图自己的左上角，所以图不会"跑"。 */
      if (e.altKey) {
        const el = (e.target as HTMLElement).closest<HTMLElement>("[data-img]");
        if (el) {
          const id = Number(el.dataset.img);
          const b = at(id);
          const w = Math.min(MAX_W, Math.max(MIN_W, b.w * (e.deltaY < 0 ? 1.1 : 1 / 1.1)));
          const h = (w * b.h) / b.w;
          el.style.width = `${w}px`;
          el.style.height = `${h}px`;
          pos.current.set(id, { ...b, w, h });
          setActive(id);
          recompute();
          /* 滚轮会连发：停 400ms 再落库，别每帧一次 IPC */
          window.clearTimeout(sizeSave.current);
          sizeSave.current = window.setTimeout(() => remember(id), 400);
          return;
        }
      }

      // 指数步长：一格 ≈ ×1.12，和 PureRef 的手感接近
      const k2 = Math.min(4, Math.max(0.04, v.k * (e.deltaY < 0 ? 1.12 : 1 / 1.12)));
      v.x = mx - (mx - v.x) * (k2 / v.k);
      v.y = my - (my - v.y) * (k2 / v.k);
      v.k = k2;
      applyVp();
      if (tick.current) cancelAnimationFrame(tick.current);
      tick.current = requestAnimationFrame(() => {
        tick.current = 0;
        setZoom(vp.current.k);
        recompute();
      });
    };
    root.addEventListener("wheel", onWheel, { passive: false });
    return () => root.removeEventListener("wheel", onWheel);
  }, [applyVp, recompute, remember, winPan]);

  useEffect(() => {
    /* 一次指针交互的**收尾**（松手 / 事件丢了 / 窗口失焦都走这里）。
       拖图收尾要把最终位置写回 `pos` 并落库；平移收尾只要清状态重算可见集。 */
    const settle = () => {
      if (pan.current) {
        pan.current = null;
        rootRef.current?.classList.remove("panning");
        recompute();
      }
      const d = drag.current;
      if (!d) return;
      const el = canvasRef.current?.querySelector<HTMLElement>(
        d.note ? `[data-note="${-d.id}"]` : `[data-img="${d.id}"]`
      );
      if (el) {
        pos.current.set(d.id, {
          x: Math.round(parseFloat(el.style.left) || d.box.x),
          y: Math.round(parseFloat(el.style.top) || d.box.y),
          w: Math.round(parseFloat(el.style.width) || d.box.w),
          h: Math.round(parseFloat(el.style.height) || d.box.h),
        });
      }
      /* 便签的高度是 auto（由文字撑开），`style.height` 是空串 —— parseFloat 会给 NaN，
         上面那行自动退回 d.box.h，够用。 */
      if (d.note) noteRemember(-d.id);
      else remember(d.id);
      drag.current = null;
      rootRef.current?.classList.remove("dragging-img");
      recompute();
    };

    const onMove = (e: MouseEvent) => {
      /* **按下的那个键已经不在了 = 这次交互其实早结束了。**
         mouseup 不一定落到我们手里：拖标题栏移动窗口时，Tauri v2 内置的 drag-region 会把
         mousedown 交给系统走标题栏拖动（Windows 上是 caption 的模态循环，结尾的
         WM_LBUTTONUP 归标题栏，不进 WebView），WebView 收不到那个 mouseup；
         拖到窗口外松手同理。以前这里不看 `e.buttons`，于是 pan/drag 永远留着 ——
         松手后指针一动，画布就被"多出来的这段位移"推着继续跑。
         其余拖拽（Tree / Views / Chrome / App 的拖放）全都守了这一条，只有这里漏了。
         比的是**按下时那个键的位**（中键拖也在其中），不是硬写 `!== 1`。 */
      const pend = pan.current;
      const dend = drag.current;
      if ((pend && (e.buttons & pend.mask) === 0) || (dend && (e.buttons & dend.mask) === 0)) {
        settle();
        return;
      }
      const p = pan.current;
      if (p) {
        vp.current.x = p.ox + (e.clientX - p.sx);
        vp.current.y = p.oy + (e.clientY - p.sy);
        applyVp();
        return;
      }
      const d = drag.current;
      if (!d) return;
      const el = canvasRef.current?.querySelector<HTMLElement>(
        d.note ? `[data-note="${-d.id}"]` : `[data-img="${d.id}"]`
      );
      if (!el) return;
      if (d.kind === "move") {
        el.style.left = `${d.box.x + (e.clientX - d.sx) / vp.current.k}px`;
        el.style.top = `${d.box.y + (e.clientY - d.sy) / vp.current.k}px`;
      } else {
        /* 改大小：**保持比例**，取位移大的那个轴当主导，另一个按比例跟。
           这样拖角往哪边推都自然，也不会把图拉变形。 */
        const dx = (e.clientX - d.sx) / vp.current.k;
        const dy = (e.clientY - d.sy) / vp.current.k;
        const byW = d.box.w + dx;
        const byH = (d.box.h + dy) * (d.box.w / d.box.h);
        const w = Math.min(MAX_W, Math.max(MIN_W, Math.abs(dx) > Math.abs(dy) ? byW : byH));
        const h = (w * d.box.h) / d.box.w;
        el.style.width = `${w}px`;
        el.style.height = `${h}px`;
      }
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", settle);
    /* 窗口失焦（Alt+Tab、点别的窗口、系统接管鼠标）也当作松手：
       否则回来之后那次"按下的状态"还挂着。 */
    window.addEventListener("blur", settle);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", settle);
      window.removeEventListener("blur", settle);
    };
  }, [applyVp, recompute, boxById, remember, noteRemember]);

  const onDown = (e: React.MouseEvent) => {
    if (e.button === 2) return; // 右键交给菜单
    const t = e.target as HTMLElement;
    /* **拖窗口的把手不参与画布平移**：顶栏是 `data-tauri-drag-region`，按下去由 Tauri
       内置的 drag-region 处理（`start_dragging`，Windows 上走标题栏 caption 循环）整个挪窗口。
       这里如果再起一个 pan，同一份位移会被用两次 —— 窗口在动、画布也在动
       （owner 看到的"图片跟着窗口一起滑"），而且松手时 mouseup 被系统吃掉
       （见下面 onMove 的注释），pan 清不掉，指针一动画面就继续跑。
       换句话说：两个 handler 都想要这次 mousedown，谁也别指望另一个"会拦下来"，
       各让一步最省事 —— 让窗口拖动拿到这次按下，画布不参与。 */
    if (t.closest("[data-tauri-drag-region]")) return;
    // 顶栏/缩放条上的按钮同理：按下按钮不该顺手把画布平移起来
    if (t.closest("button")) return;
    /* 左侧图层面板：点列表、拖滚动条都不该把画布拖走 */
    if (t.closest(".board-layers")) return;
    /* 正在编辑的便签：这次按下归 textarea（选字 / 摆光标），**不要**起拖动。
       否则往便签里点一下，整张条就被挪走了。 */
    if (t.closest("textarea")) return;
    const noteEl = t.closest<HTMLElement>("[data-note]");
    if (noteEl && e.button === 0) {
      const nid = Number(noteEl.dataset.note);
      drag.current = {
        kind: "move",
        id: -nid,
        sx: e.clientX,
        sy: e.clientY,
        box: at(-nid),
        note: true,
        mask: 1,
      };
      rootRef.current?.classList.add("dragging-img");
      e.preventDefault();
      return;
    }
    const handle = t.closest<HTMLElement>("[data-handle]");
    const cell = t.closest<HTMLElement>("[data-img]");
    if (cell && e.button === 0) {
      const id = Number(cell.dataset.img);
      /* 最近点的那张放到最顶层（需求原话：不需要 PS 那种层叠序，点谁谁在上面） */
      setTopId(id);
      const b = at(id);
      drag.current = {
        kind: handle ? "size" : "move",
        id,
        sx: e.clientX,
        sy: e.clientY,
        box: b,
        /* 恒为 1：拖图只认左键（上面的 `e.button === 0` 已经限定），收尾判断只需问
           "左键还在不在"。这里**不能**照 pan 那样写 `e.buttons` —— 中键+左键和弦时
           会记成 5，之后只松开左键（buttons=4）时 `4 & 5 = 4 ≠ 0`，"还在按着"，
           拖图状态就结束不了。pan 那处要覆盖中键/侧键，所以必须用 `e.buttons`。 */
        mask: 1,
      };
      rootRef.current?.classList.add("dragging-img");
      e.preventDefault();
      return;
    }
    /* 中键按下去 WebView2 会在指针处插一个"自动滚动"锚点（那个四向箭头），
       画布跟着它滚会和我们的平移打架 —— 主流做法是直接吃掉默认行为
       （仓库里 `Snip.tsx:537` 就是同一条惯例）。只对中键做，左键不动。 */
    if (e.button === 1) e.preventDefault();
    pan.current = {
      sx: e.clientX,
      sy: e.clientY,
      ox: vp.current.x,
      oy: vp.current.y,
      /* 按下那一刻的位掩码原样记下来：左键=1、中键=4，鼠标侧键是 8/16。
         以前写死 `e.button === 1 ? 4 : 1`，侧键起手会拿到 1 → 下一帧
         `buttons & 1` 为 0 就被判成"已松手"，光标闪一下。 */
      mask: e.buttons || (e.button === 1 ? 4 : 1),
    };
    rootRef.current?.classList.add("panning");
    /* `.panning` 的清除统一交给上面那个 effect 的 `settle`（mouseup / 丢事件 / 失焦
       三种收尾都经它），这里不再另挂一个 once 监听 —— 两条路清同一个状态容易不同步。 */
  };

  /* ---------------- 粘贴（Ctrl+V）：落盘截图目录 + 入库 + 打上这块板的标签 ---------------- */

  const pasting = useRef(false);

  const paste = useCallback(async () => {
    if (!boot) return;
    const snap = boot.snapshot;
    const coll = boot.coll;
    const tag = boot.tag;
    /* 按住 Ctrl+V 会连发 keydown：粘贴要读剪贴板 + 可能扫一遍截图目录，
       不加这道闸，一次长按就能给你粘出二十张一模一样的图。 */
    if (pasting.current) return;
    pasting.current = true;
    try {
      const it = snap
        ? await source.snapshotPaste(snap.id)
        : coll
          ? await source.collPaste(coll.id)
          : await source.boardPaste(tag!);
      const root = rootRef.current;
      const W = root?.clientWidth ?? 1200;
      const H = root?.clientHeight ?? 800;
      const v = vp.current;
      /* 落在视口正中、屏幕上约占视口宽度三分之一 —— 粘完不用满画布找它。
         不放大超过原图像素（it.w 是原图宽），缩略图再放就糊。 */
      const aspect = it.w > 0 && it.h > 0 ? it.w / it.h : 4 / 3;
      const want = (W * 0.34) / v.k;
      const cap = it.w > 0 ? it.w : MAX_W;
      const w = Math.round(Math.min(MAX_W, cap, Math.max(MIN_W, want)));
      const h = Math.max(24, Math.round(w / aspect));
      const cx = (W / 2 - v.x) / v.k;
      const cy = (H / 2 - v.y) / v.k;
      pos.current.set(it.id, { x: Math.round(cx - w / 2), y: Math.round(cy - h / 2), w, h });
      remember(it.id);
      setItems((old) => (old.some((x) => x.id === it.id) ? old : [...old, it]));
      setHint({
        text: snap
          ? `已存进程序目录的 screenshots，并加进快照「${snap.name}」`
          : coll
            ? `已存进程序目录的 screenshots，并加进分组「${coll.name}」`
            : `已存进程序目录的 screenshots，并打上「${tag}」`,
        bad: false,
      });
    } catch (e) {
      setHint({ text: String(e), bad: true });
    } finally {
      pasting.current = false;
    }
  }, [boot, remember]);

  /* ---------------- 从外面拖文件进来（P2.8） ----------------
     走 WebView 的原生拖放事件（HTML5 drop 在 Tauri 里拿不到真实路径）。
     只收**已经在素材库里**的文件；不在库里的如实报出来（不静默丢）。 */
  const addPaths = useCallback(
    async (paths: string[]) => {
      if (!paths.length) return;
      try {
        const [items, bad] = await source.boardAddPaths(paths);
        if (items.length) {
          const root = rootRef.current;
          const W = root?.clientWidth ?? 1200;
          const H = root?.clientHeight ?? 800;
          const v = vp.current;
          const cx = (W / 2 - v.x) / v.k;
          const cy = (H / 2 - v.y) / v.k;
          /* 落到视口中心，一列往下排（多张时不会叠成一坨） */
          items.forEach((it, i) => {
            const aspect = it.w > 0 && it.h > 0 ? it.w / it.h : 4 / 3;
            const w = Math.round(Math.min(MAX_W, Math.max(MIN_W, CW)));
            const h = Math.max(24, Math.round(w / aspect));
            pos.current.set(it.id, {
              x: Math.round(cx - w / 2),
              y: Math.round(cy - h / 2 + i * (h + GAP)),
              w,
              h,
            });
          });
          setItems((old) => {
            const have = new Set(old.map((x) => x.id));
            return [...old, ...items.filter((x) => !have.has(x.id))];
          });
          items.forEach((it) => remember(it.id));
          setHint({ text: `已放上 ${items.length} 张`, bad: false });
        }
        if (bad.length) {
          setHint({
            text:
              bad.length === 1
                ? bad[0]
                : `${bad.length} 个文件没放上来（不在素材库里）：${bad[0]}`,
            bad: true,
          });
        }
      } catch (e) {
        setHint({ text: String(e), bad: true });
      }
    },
    [remember]
  );

  useEffect(() => {
    /* 浏览器预览没有原生拖放：没有 Tauri 运行时就直接跳过 */
    if (!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) return;
    let dead = false;
    let un: (() => void) | null = null;
    void import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview()
          .onDragDropEvent((e) => {
            const p = e.payload;
            if (p.type === "over") rootRef.current?.classList.add("drop-hot");
            else if (p.type === "leave") rootRef.current?.classList.remove("drop-hot");
            else if (p.type === "drop") {
              rootRef.current?.classList.remove("drop-hot");
              /* 自己刚拖出去又拖回来的那趟不算「导入」：拖回来可能只是想放到
                 标签/分组上（App 的 am://drag-end 负责投放），不拦的话素材库
                 会凭空多一份导入记录。 */
              if (isOwnDrag(p.paths)) return;
              void addPaths(p.paths);
            }
          })
          .then((u) => {
            if (dead) u();
            else un = u;
          })
          .catch(() => {})
      )
      .catch(() => {});
    return () => {
      dead = true;
      un?.();
    };
  }, [addPaths]);

  /* ---------------- 键盘 ---------------- */

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      /* 正在便签里打字：全局快捷键一律让路。
         否则 Ctrl+V 会被当成"粘贴剪贴板图片"，一按就往板里塞一张截图。
         注意 target 不一定是元素（事件可能落在 window 上，比如程序化派发），
         所以先判类型再用 `closest` —— 直接 `e.target.closest` 会 TypeError。 */
      const tgt = e.target instanceof HTMLElement ? e.target : null;
      if (tgt && tgt.closest("textarea, input")) return;
      const mod = e.ctrlKey || e.metaKey;
      /* Ctrl+V = 把剪贴板里的图**变成这个标签下的一张素材**：
         存进 `程序目录\screenshots` → 入库 → 打上这块板的标签 → 贴到视口正中。
         复制走的是原文件（下面 Ctrl+C），所以粘进来的永远是源文件，不是缩略图。 */
      if (mod && e.key.toLowerCase() === "v") {
        e.preventDefault();
        if (!e.repeat) void paste();
        return;
      }
      /* Ctrl+C = 复制**指针下那张的原文件**（CF_HDROP）。
         板上看到的永远是缩略图，但所有"复制 / 打开 / 定位"一律落到源文件 ——
         缩略图只是用来看的，不许当成结果交出去。 */
      if (mod && e.key.toLowerCase() === "c") {
        const it = active != null ? items.find((x) => x.id === active) : null;
        if (it) {
          e.preventDefault();
          void source.peekCopyFile([it.path]).catch(() => {});
        }
        return;
      }
      if (e.key === "Escape") {
        if (menu) {
          setMenu(null);
          return;
        }
        void source.boardClose();
        return;
      }
      if (e.key === "f" || e.key === "F") fit();
      if (e.key === "0") reset100();
      /* L = 开合左侧图层面板 */
      if ((e.key === "l" || e.key === "L") && !mod && !e.altKey) {
        e.preventDefault();
        setLayersOpen((v) => !v);
      }
      /* T = 在视口正中放一张文字便签（和顶栏那枚「文字」按钮同一条路） */
      if ((e.key === "t" || e.key === "T") && !mod && !e.altKey) {
        e.preventDefault();
        void addNote();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fit, menu, reset100, active, items, paste, addNote]);

  /* ---------------- 菜单 ---------------- */

  const menuItems: CtxItem[] = useMemo(() => {
    /* 便签的菜单只有两件事：改字、删掉 */
    if (menu?.note != null) {
      const nid = menu.note;
      return [
        {
          label: "编辑文字",
          icon: "file",
          action: () => setEditId(nid),
        },
        { kind: "separator" },
        {
          label: "删除这张便签",
          icon: "trash",
          danger: true,
          action: () => delNote(nid),
        },
      ];
    }
    const id = menu?.id ?? null;
    const it = id != null ? items.find((x) => x.id === id) : null;
    if (!it) {
      return [
        {
          label: "添加文字便签",
          icon: "file",
          shortcut: "T",
          action: () => void addNote(),
        },
        {
          label: "一键整理图片",
          icon: "grid",
          action: tidy,
        },
        { kind: "separator" },
        {
          label: "从资源管理器拖文件进来",
          icon: "import",
          action: () => setHint({ text: "直接把图片拖进画布即可（只收已入库的图）", bad: false }),
        },
        {
          label: "粘贴剪贴板图片",
          icon: "copy",
          shortcut: "Ctrl+V",
          action: () => void paste(),
        },
        { kind: "separator" },
        { label: "全部适应窗口", icon: "expand", shortcut: "F", action: fit },
        { label: "回到 100%", icon: "search", shortcut: "0", action: reset100 },
        { kind: "separator" },
        { label: "关闭这块灵感板", icon: "close", shortcut: "Esc", danger: true, action: () => void source.boardClose() },
      ];
    }
    return [
      {
        label: "用默认程序打开原文件",
        icon: "file",
        action: () => void source.openDefault(it.path).catch(() => {}),
      },
      {
        label: "用贴图打开",
        icon: "expand",
        action: () => void source.peekOpen([it.id], 0).catch(() => {}),
      },
      {
        label: "复制原图文件",
        icon: "copy",
        shortcut: "Ctrl+C",
        action: () => void source.peekCopyFile([it.path]).catch(() => {}),
      },
      {
        label: "另存原文件为…",
        icon: "file",
        action: () => void source.saveAs(it.path, it.name).catch((e) => setHint({ text: String(e), bad: true })),
      },
      {
        label: "在资源管理器中显示",
        icon: "reveal",
        action: () => void source.reveal(it.path).catch(() => {}),
      },
      { kind: "separator" },
      { label: "全部适应窗口", icon: "expand", shortcut: "F", action: fit },
      { label: "回到 100%", icon: "search", shortcut: "0", action: reset100 },
      { kind: "separator" },
      { label: "关闭这块灵感板", icon: "close", shortcut: "Esc", danger: true, action: () => void source.boardClose() },
    ];
  }, [menu, items, notes, fit, reset100, paste, addNote, tidy, delNote]);

  if (!boot) {
    return <div className="board-miss">灵感板数据丢失，请回到主窗口重新打开</div>;
  }

  const shown = items.filter((it) => vis.has(it.id));

  return (
    <div
      ref={rootRef}
      className="board-root"
      onMouseDown={onDown}
      onDoubleClick={(e) => {
        const t = e.target as HTMLElement;
        /* 双击顶栏是「最大化/还原窗口」（Tauri 内置的 drag-region 行为），
           不该顺带把画布 fit 一遍 —— 和 onMouseDown 用同一条排除规则。 */
        if (t.closest("[data-tauri-drag-region]")) return;
        const cell = t.closest<HTMLElement>("[data-img]");
        if (cell) {
          void source.peekOpen([Number(cell.dataset.img)], 0).catch(() => {});
        } else {
          fit();
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        const t = e.target as HTMLElement;
        const cell = t.closest<HTMLElement>("[data-img]");
        const note = t.closest<HTMLElement>("[data-note]");
        /* 图层面板里的行也能右键：菜单用的是同一份"这张图"的菜单（打开/另存/定位） */
        const layer = t.closest<HTMLElement>("[data-layer]");
        if (!cell && !note && t.closest(".board-layers") && !layer) {
          // 面板空白处：没有可作用于的图，直接不弹（否则会弹出"画布"菜单，语义是错的）
          setMenu(null);
          return;
        }
        setMenu({
          x: e.clientX,
          y: e.clientY,
          id: cell ? Number(cell.dataset.img) : layer ? Number(layer.dataset.layer) : null,
          note: note ? Number(note.dataset.note) : undefined,
        });
      }}
    >
      <div ref={canvasRef} className="board-canvas">
        {shown.map((it) => {
          const b = pos.current.get(it.id) ?? boxById.get(it.id)!;
          return (
            <div
              key={it.id}
              data-img={it.id}
              className={`board-cell ${active === it.id ? "on" : ""} ${topId === it.id ? "top" : ""} ${broken.has(it.id) ? "broken" : ""}`}
              style={{ left: b.x, top: b.y, width: b.w, height: b.h }}
              title={it.name}
              onMouseEnter={() => setActive(it.id)}
              onMouseLeave={() => setActive((v) => (v === it.id ? null : v))}
            >
              {broken.has(it.id) ? (
                /* 出不了缩略图的（.txt / 损坏文件 / 未知格式）也留在板上：
                   位置、拖动、改大小和真图一样，只是内容换成"文件名 + 类型" */
                <div className="board-fallback">
                  <span className="board-fallback-ext mono">
                    {(it.name.split(".").pop() || "?").toUpperCase().slice(0, 5)}
                  </span>
                  <span className="board-fallback-name">{it.name}</span>
                </div>
              ) : (
                <>
                  {/* 静态层：永远是 1024 档缩略图（动图就是它的首帧）。板上不动 = 不解码整段动画 */}
                  <img
                    src={it.src}
                    alt={it.name}
                    draggable={false}
                    decoding="async"
                    loading="lazy"
                    onError={() =>
                      setBroken((old) => {
                        if (old.has(it.id)) return old;
                        const n = new Set(old);
                        n.add(it.id);
                        return n;
                      })
                    }
                  />
                  {/* 动图：**只有指针停在这张上**才挂原文件那一层，离开立刻拆掉（卸载即停止解码）。
                      原图解码比缩略图慢，所以先透明、加载完再淡入，避免切换瞬间闪一下白。 */}
                  {it.anim && it.raw && active === it.id && (
                    <img
                      key="anim"
                      className="board-anim"
                      src={it.raw}
                      alt=""
                      draggable={false}
                      decoding="async"
                      onLoad={(e) => e.currentTarget.classList.add("ready")}
                      onError={(e) => {
                        /* 原图读不出来（超 48MB 退回缩略图 / 文件被移走）：只把这一层藏掉，
                           不删 DOM —— 从 React 手里抢节点删会让下一次渲染找不到它。 */
                        e.currentTarget.style.display = "none";
                      }}
                    />
                  )}
                </>
              )}
              <span className="board-handle" data-handle title="拖动改大小（Alt+滚轮也行）" />
            </div>
          );
        })}

        {/* 文字便签：和图片同住一块画布，跟着平移缩放一起走。
            高度不写死（由文字撑开），落库的 h 只是个估计值 —— 只用于可见性判断。 */}
        {notes.map((n) => {
          const b = pos.current.get(-n.nid) ?? { x: n.x, y: n.y, w: n.w, h: n.h };
          const editing = editId === n.nid;
          return (
            <div
              key={`n${n.nid}`}
              data-note={n.nid}
              className={`board-note ${editing ? "editing" : ""}`}
              style={{ left: b.x, top: b.y, width: b.w }}
              title={editing ? undefined : "双击改文字 · 拖动挪位置"}
              onDoubleClick={(e) => {
                /* 必须拦住：外层双击是「打开贴图 / fit」，会把便签一起带走 */
                e.stopPropagation();
                setEditId(n.nid);
              }}
            >
              {editing ? (
                <textarea
                  className="board-note-input"
                  defaultValue={n.text}
                  autoFocus
                  spellCheck={false}
                  placeholder="写点什么…（Esc 保存，清空即删）"
                  onFocus={(e) => {
                    const el = e.currentTarget;
                    el.style.height = "auto";
                    el.style.height = `${el.scrollHeight}px`;
                  }}
                  onInput={(e) => {
                    /* 随字数长高：便签本来是"贴一张纸条"，不该出现内部滚动条 */
                    const el = e.currentTarget;
                    el.style.height = "auto";
                    el.style.height = `${el.scrollHeight}px`;
                  }}
                  onBlur={(e) => commitNote(n.nid, e.currentTarget.value)}
                  onKeyDown={(e) => {
                    /* 打字期间不许触发全局快捷键（T 新便签 / Ctrl+V 粘图 / F 适应窗口） */
                    e.stopPropagation();
                    if (e.key === "Escape" || (e.key === "Enter" && (e.ctrlKey || e.metaKey))) {
                      e.preventDefault();
                      e.currentTarget.blur();
                    }
                  }}
                />
              ) : (
                <div className={`board-note-text ${n.text ? "" : "empty"}`}>
                  {n.text || "双击写字"}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* 左侧图层面板（0.4.15）：板上所有图片按板内顺序列出来，点哪行就把哪张
          置顶并平移到视口正中。**不做 PS 那种"列表上方压下方"的层叠语义** ——
          列表顺序只表示"板上有哪些图"，层级只认"最近点了谁"。 */}
      {layersOpen && (
        <div className="board-layers" onMouseDown={(e) => e.stopPropagation()}>
          <div className="board-layers-head">
            <Layers size={13} strokeWidth={1.8} className="board-layers-icon" />
            <span className="board-layers-title">图层</span>
            <span className="mono board-layers-count">{items.length}</span>
            <button title="收起图层面板（L）" onClick={() => setLayersOpen(false)}>
              <X size={12} strokeWidth={1.9} />
            </button>
          </div>
          <div className="board-layers-list">
            {items.map((it) => (
              <div
                key={it.id}
                data-layer={it.id}
                className={`board-layer ${topId === it.id ? "on" : ""}`}
                title={`${it.name}\n点一下 = 置顶并居中`}
                onClick={() => {
                  setTopId(it.id);
                  focusItem(it.id);
                }}
              >
                {broken.has(it.id) ? (
                  <span className="board-layer-ph mono">
                    {(it.name.split(".").pop() || "?").toUpperCase().slice(0, 4)}
                  </span>
                ) : (
                  <img src={it.src} alt="" draggable={false} loading="lazy" decoding="async" />
                )}
                <span className="board-layer-name">{it.name}</span>
                {topId === it.id && <span className="board-layer-top">顶</span>}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 空板：唯一的正经动作就是「粘一张进来」，所以直接把这句话写在画布中央 */}
      {!items.length && !notes.length && (
        <div className="board-empty">
          <span className="board-empty-title">
            {boot.snapshot ? "这份快照里的图这次一张都没找到" : "这块板还是空的"}
          </span>
          <span className="board-empty-hint">
            {boot.snapshot ? (
              <>
                条目不会被删 —— 移动硬盘插回来、或者把目录重新加进素材库之后，
                在侧栏右键这份快照点「重新对账」就行。
                <br />
              </>
            ) : null}
            <kbd className="mono">Ctrl+V</kbd> 粘贴剪贴板里的图片 —— 会存到程序目录的{" "}
            <span className="mono">screenshots</span>，
            {boot.snapshot
              ? `并加进快照「${boot.snapshot.name}」`
              : boot.coll
                ? `并加进分组「${boot.coll.name}」`
                : `并打上「${boot.tag}」标签`}
            <br />
            也可以按 <kbd className="mono">T</kbd> 先写一段文字注释。
          </span>
        </div>
      )}

      {hint && <div className={`board-hint ${hint.bad ? "bad" : ""}`}>{hint.text}</div>}

      {/* 左下角缩放条：固定在这一角（画布怎么平移缩放都不动它）。
          图层面板展开时往右让出它的宽度，否则会浮在列表上面。 */}
      <div
        className="board-zoombar"
        style={{ left: layersOpen ? LAYERS_W + 14 : 14 }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <button title="缩小" onClick={() => zoomBy(1 / 1.2)}>
          <Minus size={12} strokeWidth={1.9} />
        </button>
        <span className="mono board-zoom" onClick={reset100} title="点一下回到 100%">
          {Math.round(zoom * 100)}%
        </span>
        <button title="放大" onClick={() => zoomBy(1.2)}>
          <Plus size={12} strokeWidth={1.9} />
        </button>
        <button title="全部适应窗口（F）" onClick={fit}>
          <Maximize2 size={12} strokeWidth={1.9} />
        </button>
      </div>

      {/* 顶栏：整条是拖窗口的把手（画布本身也要能拖，所以只在顶部让出这一条） */}
      <div className="board-top" data-tauri-drag-region>
        <div className="board-title" data-tauri-drag-region>
          <span className="board-name">{boot.title}</span>
          <span className="board-count mono">
            {items.length}
            {boot.more ? `/${boot.total}` : ""} 张
            {!!boot.missing && ` · ${boot.missing} 张这次没找到`}
          </span>
        </div>
        <div className="board-actions">
          <button
            title="粘贴剪贴板图片（Ctrl+V）：存进程序目录的 screenshots，并打上这个标签"
            onClick={() => void paste()}
          >
            <ClipboardPaste size={13} strokeWidth={1.8} />
          </button>
          <button title="添加文字便签（T）" onClick={() => void addNote()}>
            <Type size={13} strokeWidth={1.8} />
          </button>
          <button title="一键整理图片：按当前顺序重排成整齐的瀑布流" onClick={tidy}>
            <LayoutGrid size={13} strokeWidth={1.8} />
          </button>
          <button
            title="图层面板（L）：所有图片一览，点谁谁置顶"
            className={layersOpen ? "on" : ""}
            onClick={() => setLayersOpen((v) => !v)}
          >
            <Layers size={13} strokeWidth={1.8} />
          </button>
          <span className="board-sep" />
          <button title="缩小" onClick={() => zoomBy(1 / 1.2)}>
            <Minus size={13} strokeWidth={1.8} />
          </button>
          <span className="mono board-zoom">{Math.round(zoom * 100)}%</span>
          <button title="放大" onClick={() => zoomBy(1.2)}>
            <Plus size={13} strokeWidth={1.8} />
          </button>
          <button title="全部适应窗口（F）" onClick={fit}>
            <Maximize2 size={13} strokeWidth={1.8} />
          </button>
          <span className="board-sep" />
          <button title="关闭（Esc）" onClick={() => void source.boardClose()}>
            <X size={13} strokeWidth={1.8} />
          </button>
        </div>
      </div>

      {menu && (
        <ContextMenu
          state={{ x: menu.x, y: menu.y, items: menuItems }}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}