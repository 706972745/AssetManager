import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Bold as BoldIcon,
  ChevronDown,
  ChevronUp,
  ClipboardPaste,
  LayoutGrid,
  Layers,
  Maximize2,
  Minimize2,
  Minus,
  Pin,
  PinOff,
  Plus,
  Trash2,
  Type,
  X,
} from "lucide-react";
import { boardAnnApi, source } from "../data/source";
import type { BoardAnnRow, BoardNote } from "../types";
import { ContextMenu, type CtxItem } from "./ContextMenu";
import { createShiftWheelPan } from "../lib/windowShiftPan";
import { isOwnDrag } from "../lib/ownDrag";
import { useT, type TKey } from "../lib/i18n";
import {
  ANN_COLORS,
  ANN_FONT_MAX,
  ANN_FONT_MIN,
  ANN_ICONS,
  ANN_TOOLS,
  ANN_W_MAX,
  ANN_W_MIN,
  AnnWidthGlyph,
  annBmpScale,
  annBox,
  annDecode,
  annDraw,
  annEncode,
  annHit,
  annKindKey,
  annNewOp,
  annToolKey,
  annTooSmall,
  type AnnLayer,
  type AnnOp,
  type AnnPt,
  type AnnTool,
} from "../lib/boardAnn";

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
 * 0.4.17 起多两件事：
 * - **打组**：Shift/⌘ 点选多个（或点已组里的一员，整组跟着选），`Ctrl+G` 打组、
 *   `Ctrl+Shift+G` 解组。组内任一成员被拖动 = 整组一起走；拖组框右下角 = 整组等比缩放。
 *   组号落在 `board_items.grp` / `board_notes.grp` 上（同一板内同值即同组），**不单开一张表**。
 * - **文本框**：文字便签不再是一块黄纸片 —— 只剩文字本身，可以换字体/字号/颜色/加粗，
 *   拖右下角整体缩放时**字号跟着框一起变**。
 *
 * 性能上只有一条铁律：**画布变换绝不走 React state**。
 * 2000 张图每帧 re-render 一次必卡；所以平移/缩放都是直接改 `transform`（ref + DOM），
 * 只有"哪几张在视口里"会 setState（rAF 节流），其余的连 `<img>` 都不挂 —— 不挂就不解码。
 * 拖动同理：只改被拖元素（和同组成员）的 style，松手才写回 `pos` 并落库。
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
  /** 主窗当前主题（设计稿 v1.3：灵感板跟随软件主题，浅色白面板深字） */
  theme?: string;
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
/** 缩略图取不到时的重试次数与间隔：`thumb://` 是现场生成缩略图的（刚粘贴入库的素材
 *  第一次请求完全可能赶在后台扫描/解码配额还没腾出来），失败一次就永久画成占位卡
 *  等于"这张图永远没有缩略图了"。重试 3 次（300/600/900ms）仍失败才认。 */
const THUMB_RETRY = 3;
const THUMB_RETRY_MS = 300;
/** 新便签的初始落点尺寸（画布单位）。**渲染时不设宽高** —— 实框由文字撑开，
    这两个数只用来"放哪儿"，量一次就被真实尺寸覆盖 */
const NOTE_W = 240;
const NOTE_H = 120;
/** 新便签的默认字号；`fs = 0` 的老数据用 `NOTE_FS_DEF`，重开板不会突然变大 */
const NOTE_FS = 16;
const NOTE_FS_DEF = 12.5;

/** 字体下拉：系统里常见的几家，选不到的会自动回退到系统默认（CSS 就是这么工作的）。
 *  第二列是文案 key（拉丁字体名不用翻译，给 null 直接用字体名本身）。 */
const FONTS: [string, TKey | null][] = [
  ["", "board.font_default"],
  ["Microsoft YaHei", "board.font_yahei"],
  ["PingFang SC", "board.font_pingfang"],
  ["Source Han Sans SC", "board.font_src_han_sans"],
  ["Source Han Serif SC", "board.font_src_han_serif"],
  ["SimHei", "board.font_simhei"],
  ["SimSun", "board.font_simsun"],
  ["KaiTi", "board.font_kaiti"],
  ["FangSong", "board.font_fangsong"],
  ["DengXian", "board.font_dengxian"],
  ["Arial", null],
  ["Georgia", null],
  ["Times New Roman", null],
  ["Consolas", null],
  ["Impact", null],
];
/** 文字颜色的几个预设（空 = 默认浅色） */
const NOTE_COLORS: [string, TKey][] = [
  ["", "board.color_default"],
  ["#ffffff", "board.color_white"],
  ["#ffd45e", "board.color_yellow"],
  ["#ff7b72", "board.color_red"],
  ["#7ee787", "board.color_green"],
  ["#C4B5FD", "board.color_purple"],
  ["#9e9e9e", "board.color_gray"],
];
/** 常用字体（排在下拉最上面）；本机装了的字体另外从后端枚举，见 `am_system_fonts` */
const COMMON_FONT_VALS = new Set(FONTS.map(([v]) => v).filter(Boolean));

/** 字体族名 → 带引号的 CSS 值（中文名/带空格的都要引号，末尾一定留系统回退） */
function fontCss(font: string): string {
  return font
    ? `"${font}", "Microsoft YaHei", "PingFang SC", sans-serif`
    : `"Microsoft YaHei", "PingFang SC", system-ui, sans-serif`;
}
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
  const t = useT();
  const boot = useMemo(bootData, []);
  const [items, setItems] = useState<BoardItem[]>(() => boot?.items ?? []);
  /* 文字便签：坐标同样住在 `pos`（key = `-nid`），这里的 state 只负责文字与渲染 */
  const [notes, setNotes] = useState<BoardNote[]>([]);
  const notesRef = useRef<BoardNote[]>([]);
  /** 正在编辑的那张便签（nid）；null = 没有 */
  const [editId, setEditId] = useState<number | null>(null);
  /** 编辑中的 textarea：点便签外面时要拿它的当前值去提交 */
  const taRef = useRef<HTMLTextAreaElement | null>(null);
  /* 选中集合（0.4.17）：key 和 `pos` 一致 —— 图片 id 正数、便签 `-nid` 负数。
     打组只是把这个集合"黏"下来：点组里任一员时，整组成员自动进选区。 */
  const [sel, setSel] = useState<Set<number>>(new Set());
  const selRef = useRef<Set<number>>(new Set());
  /** 成员 → 组号（0 / 不在表里 = 没打组）。只认后端，前端不自己发号 */
  const [grpMap, setGrpMap] = useState<Map<number, number>>(new Map());
  /** 本机已安装的字体族（后端枚举一次）；空数组 = 还没拿到，下拉只显示常用那几款 */
  const [sysFonts, setSysFonts] = useState<string[]>([]);
  /** 字体选择弹层（本机动辄上千款字体，原生 select 滚不动，改成带搜索的弹层） */
  const [fontPop, setFontPop] = useState(false);
  const [fontQ, setFontQ] = useState("");
  /* 左侧图层面板（0.4.15）：列出板上**所有**图片，点一行 = 置顶 + 视口居中。
     面板默认展开，但只在展开时才渲染列表（2000 张的板不打开就不付这份 DOM 成本）。 */
  const [layersOpen, setLayersOpen] = useState(true);
  /** 当前**最顶层**的那张。只记一张 —— 需求是"最近点哪张哪张在上面"，不是 PS 那种层叠序 */
  const [topId, setTopId] = useState<number | null>(null);
  /* 置顶开关（默认开）：板子是拖到一边对着主窗看参考的，被主窗盖住就失去意义。
     初值只是"新窗口的默认值"，真值问后端 —— 用户可能刚点过，本地值会撒谎。 */
  const [boardTop, setBoardTop] = useState(true);
  useEffect(() => {
    void source
      .boardIsTop()
      .then((v) => setBoardTop(v))
      .catch(() => {});
  }, []);
  /* 乐观更新：先把按钮翻过去，后端认下的值再校正一次；失败就翻回原值并静默
     （顶栏没有地方挂红字，置顶失败也不影响画布上的任何操作）。 */
  const toggleTop = useCallback(() => {
    const want = !boardTop;
    setBoardTop(want);
    void source
      .boardTop(want)
      .then((real) => setBoardTop(real))
      .catch(() => setBoardTop(!want));
  }, [boardTop]);
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
  /** 缩略图已经重试过几次（`id → 次数`）：次数进 `src` 的查询串，让 WebView 重新发请求 */
  const [thumbTry, setThumbTry] = useState<Map<number, number>>(new Map());
  const thumbTries = useRef<Map<number, number>>(new Map());
  const thumbTimers = useRef<Map<number, number>>(new Map());
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    id: number | null;
    /** 右键落在便签上：nid */
    note?: number;
    /** 右键落在标注上：aid（画布上的那层 或 图层面板里那一行） */
    ann?: number;
  } | null>(null);
  const [active, setActive] = useState<number | null>(null); // 悬停/拖动中的那张：显示缩放手柄
  /* 操作反馈（粘贴成功/失败）：画布上没有 toast，横幅也不该常驻，就一条两秒半的提示 */
  const [hint, setHint] = useState<{ text: string; bad: boolean } | null>(null);

  /* ---------------- 标注（截图工具条那一套，0.6.x） ----------------
     一笔 = 一层：`annLayers` 就是画布上的绘制顺序（z 小的在前）。
     几何存在 `op` 里（**相对本层原点**），所以拖动改的只是 x / y；
     落库的 `payload` 和 `op` 是同一个东西 —— 位置变了 payload 不用重写。
     工具的临时状态（颜色 / 粗细 / 填充）只是"下一笔的默认值"，不进库。 */
  const [annOpen, setAnnOpen] = useState(false);
  const [annTool, setAnnTool] = useState<AnnTool>("select");
  const [annColor, setAnnColor] = useState(ANN_COLORS[7]);
  /** 线宽（画布单位） */
  const [annWeight, setAnnWeight] = useState(2);
  /** 字号（文字 / 序号工具用）。和线宽分开：画线的 2px 当字号只有 8px，太离谱 */
  const [annFont, setAnnFont] = useState(18);
  const [annFill, setAnnFill] = useState(false);
  const [annPalette, setAnnPalette] = useState(false);
  const [annLayers, setAnnLayers] = useState<AnnLayer[]>([]);
  const annLayersRef = useRef<AnnLayer[]>([]);
  /** 选中的标注层（aid）；null = 没选 */
  const [annSel, setAnnSel] = useState<number | null>(null);
  const annSelRef = useRef<number | null>(null);
  /** 序号工具的下一个号（同板递增，读回来时按已有最大号续上） */
  const annNextRef = useRef(1);
  /** 文字工具的落点（画布坐标）；null = 没在输入 */
  const [annTextAt, setAnnTextAt] = useState<AnnPt | null>(null);
  const annTextRef = useRef<HTMLInputElement>(null);
  /** 输入框提交一次就锁住：Enter 提交后节点被卸载，个别情况下还会补一个 blur，
   *  不加这道闸会同一段文字落两次层。 */
  const annTextLock = useRef(false);
  /** 撤销 / 重做：整份图层数组的快照栈（和截图那边的做法一致 —— 删除 / 换层级都能回退） */
  const annUndo = useRef<AnnLayer[][]>([]);
  const annRedo = useRef<AnnLayer[][]>([]);
  /** 快照栈是 ref，改了不会重渲染；这个版本号只用来刷新撤销/重做按钮的可用态 */
  const [annVer, setAnnVer] = useState(0);
  /** 拖拽过程中的预览画布（直接改 DOM，不走 React） */
  const annLiveRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    notesRef.current = notes;
  }, [notes]);

  useEffect(() => {
    annLayersRef.current = annLayers;
  }, [annLayers]);

  useEffect(() => {
    annSelRef.current = annSel;
  }, [annSel]);

  useEffect(() => {
    selRef.current = sel;
  }, [sel]);

  /* 本机字体：只问一次，失败就让下拉停在"常用"那几款（不影响任何功能） */
  useEffect(() => {
    void source
      .systemFonts()
      .then((list) => setSysFonts(Array.isArray(list) ? list : []))
      .catch(() => {});
  }, []);

  /* 字体弹层：点面板外面就收起（点里面由面板自己 stopPropagation 挡住） */
  useEffect(() => {
    if (!fontPop) return;
    const h = (e: MouseEvent) => {
      const t = e.target as HTMLElement;
      if (!t.closest(".board-fontpop") && !t.closest(".board-notebar-fontbtn")) setFontPop(false);
    };
    window.addEventListener("mousedown", h);
    return () => window.removeEventListener("mousedown", h);
  }, [fontPop]);

  /* 点便签以外的**任何地方**（画布空白 / 图片 / 别的便签 / 图层面板 / 顶栏…）= 结束编辑。
     以前只有 textarea 自己失焦才提交，可画布 mousedown 对 textarea 直接 return、面板又
     stopPropagation，点哪儿都不失焦，编辑态只能靠 Esc 退。这里挂在**捕获阶段**：
     在 stopPropagation 之前就拿到事件，一次覆盖所有区域。
     例外：底部便签工具条 + 字体弹层 —— 它们就是编辑时用来改字号/字体/竖排的，
     点一下就退出编辑等于用不了。 */
  useEffect(() => {
    if (editId == null) return;
    const h = (e: MouseEvent) => {
      if (e.button !== 0) return;
      const t = e.target;
      if (!(t instanceof Element)) return;
      if (t.closest(`[data-note="${editId}"]`)) return;
      if (t.closest(".board-notebar, .board-fontpop")) return;
      /* blur 走的是 textarea 自己的 onBlur → commitNote：空文本照旧删便签，
         有字就存盘，不用在这里重写一遍提交逻辑。 */
      taRef.current?.blur();
    };
    window.addEventListener("mousedown", h, true);
    return () => window.removeEventListener("mousedown", h, true);
  }, [editId]);

  /* 便签的"框"就是文字本身的尺寸（CSS 不设宽高），渲染完量一次写进 `pos` ——
     组框、整组缩放、落库都要真实大小。`offsetWidth` 是布局尺寸，不吃画布 transform。 */
  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv) return;
    for (const n of notes) {
      const el = cv.querySelector<HTMLElement>(`[data-note="${n.nid}"]`);
      if (!el) continue;
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      const b = pos.current.get(-n.nid);
      if (!b || !w || !h) continue;
      if (Math.abs(b.w - w) > 0.5 || Math.abs(b.h - h) > 0.5) {
        pos.current.set(-n.nid, { ...b, w, h });
      }
    }
  }, [notes]);

  /* Shift + 滚轮 = 挪**窗口**在屏幕上的位置（owner 2026-11-19）。
     它自己管 rAF 合帧与窗口几何缓存，所以整块板只建一次；挂载/卸载跟着组件走。 */
  const winPan = useMemo(
    () =>
      createShiftWheelPan({
        onUnavailable: () =>
          setHint({ text: t("board.shift_pan_unavailable"), bad: true }),
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

  /** 内容包围盒：图片瀑布流 + 便签 + 标注。
   *  便签不参与「一键整理」，但**必须算进 fit** —— 否则 F / 自动适应之后便签会飞到视野外；
   *  标注同理（有人会把注释画在图的边角外面）。 */
  const fitSize = useMemo(() => {
    let w = cw;
    let h = ch;
    for (const n of notes) {
      const b = pos.current.get(-n.nid) ?? { x: n.x, y: n.y, w: n.w, h: n.h };
      w = Math.max(w, b.x + b.w);
      h = Math.max(h, b.y + b.h);
    }
    for (const l of annLayers) {
      w = Math.max(w, l.x + l.w);
      h = Math.max(h, l.y + l.h);
    }
    return { w, h };
  }, [notes, annLayers, cw, ch]);

  /** 按内容包围盒 fit 进窗口并居中 */
  const fit = useCallback(() => fitTo(fitSize.w, fitSize.h), [fitTo, fitSize]);

  /** 把某个成员（图片 id 正 / 便签 `-nid` 负）挪到视口正中（装不下才缩）。
   *  图层面板点一行走这里 —— 图片和文字走同一条路。 */
  const focusAt = useCallback(
    (key: number) => {
      const root = rootRef.current;
      if (!root) return;
      const pad = layersOpen ? LAYERS_W : 0;
      const W = (root.clientWidth || 1200) - pad;
      const H = root.clientHeight || 800;
      let b =
        pos.current.get(key) ??
        (key > 0 ? boxById.get(key) : undefined) ?? { x: 0, y: 0, w: CW, h: CW };
      if (key < 0) {
        /* 便签的尺寸以 DOM 为准（改字号后 pos 里那份会过期） */
        const el = elOf(key);
        if (el?.offsetWidth) b = { ...b, w: el.offsetWidth, h: el.offsetHeight };
      }
      const v = vp.current;
      /* 只在"这个装不下"时才改 k：用户当前缩放手感不该被一次点击重置 */
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

  /* ---------------- 标注：坐标 / 命中 / 落库 ----------------
     一切几何都存**画布坐标**（层的 op 是相对本层原点），所以缩放/平移时标注
     跟着图走，不需要任何补偿。屏幕坐标只在"按下/移动"这两个入口换算一次。 */

  /** 新文案的 key 还没进 locales（由主控统一补）：缺翻译时 `t()` 会原样返回 key，
   *  正好用来定位漏翻，所以这里只把 string 收窄成 TKey，不做别的。 */
  const tr = (k: string, vars?: Record<string, string | number>) => t(k as TKey, vars);

  /** 屏幕坐标 → 画布坐标 */
  const annPoint = useCallback((e: { clientX: number; clientY: number }): AnnPt => {
    const r = rootRef.current?.getBoundingClientRect();
    const v = vp.current;
    return {
      x: (e.clientX - (r?.left ?? 0) - v.x) / v.k,
      y: (e.clientY - (r?.top ?? 0) - v.y) / v.k,
    };
  }, []);

  /** 位图倍率：跟着缩放走（放大后按新倍率重画一遍，标注不会糊成一片），
   *  但给总像素数封顶 —— 整块板涂满荧光笔的层不压一下会吃掉几十 MB 显存。 */
  const annScale = (w: number, h: number) => annBmpScale(w, h, vp.current.k);

  const annSnap = () => annLayersRef.current.map((l) => ({ ...l }));

  /** 动状态之前先存一份快照：撤销/重做就是"整份图层数组"的回放（和截图那边一致） */
  const annCheckpoint = useCallback(() => {
    annUndo.current.push(annSnap());
    if (annUndo.current.length > 40) annUndo.current.shift();
    annRedo.current = [];
    setAnnVer((v) => v + 1);
  }, []);

  /** 把图层数组整体换成快照里的样子：多的删、少的补、位置/层级变了的更新。
   *  补回来的层会拿到**新的 aid**（老 aid 已经被删了），前端自己认新的那个。 */
  const annApplySnapshot = useCallback(
    async (next: AnnLayer[]) => {
      const board = boot?.board;
      const cur = annLayersRef.current;
      const curById = new Map(cur.map((l) => [l.aid, l]));
      const ids = new Set(next.map((l) => l.aid));
      for (const l of cur) {
        if (!ids.has(l.aid) && board) void boardAnnApi.del(board, l.aid).catch(() => {});
      }
      const out: AnnLayer[] = [];
      for (const l of next) {
        const old = curById.get(l.aid);
        if (old) {
          out.push(l);
          if (board && (old.x !== l.x || old.y !== l.y || old.z !== l.z)) {
            void boardAnnApi.update(board, l.aid, l.x, l.y, l.z).catch(() => {});
          }
          continue;
        }
        let aid = l.aid;
        if (board && l.aid > 0) {
          aid = await boardAnnApi
            .add(board, l.z, l.kind, JSON.stringify(l.op), l.x, l.y, l.w, l.h)
            .catch(() => l.aid);
        }
        out.push({ ...l, aid });
      }
      annLayersRef.current = out;
      setAnnLayers(out);
      setAnnSel((s) => (s != null && out.some((l) => l.aid === s) ? s : null));
    },
    [boot?.board]
  );

  /** 提交一笔：先上屏（临时负数 aid，立刻能看见），后端发号回来再换成真 aid。
   *  这样画的时候不用等 IPC，慢一点也只是"图层面板里的 aid 变一下"。 */
  const annCommit = useCallback(
    async (op: AnnOp) => {
      const b = annBox(op);
      const x = Math.floor(b.x);
      const y = Math.floor(b.y);
      const w = Math.max(1, Math.ceil(b.w) + 1);
      const h = Math.max(1, Math.ceil(b.h) + 1);
      const payload = annEncode(op, b);
      const kind = op.t as AnnTool;
      const z = annLayersRef.current.reduce((m, l) => Math.max(m, l.z), 0) + 1;
      const local = JSON.parse(payload) as AnnOp;
      const temp = -Date.now();
      annLayersRef.current = [
        ...annLayersRef.current,
        { aid: temp, z, kind, x, y, w, h, op: local },
      ];
      setAnnLayers(annLayersRef.current);
      setAnnSel(temp);
      annSelRef.current = temp;
      const board = boot?.board;
      if (!board) return;
      try {
        const aid = await boardAnnApi.add(board, z, kind, payload, x, y, w, h);
        /* 发号这段时间里这层可能已经被橡皮擦 / Delete 删掉了：那就在库里也删掉，
           否则下次开板会冒出一层"幽灵标注" */
        if (!annLayersRef.current.some((l) => l.aid === temp)) {
          void boardAnnApi.del(board, aid).catch(() => {});
          return;
        }
        annLayersRef.current = annLayersRef.current.map((l) =>
          l.aid === temp ? { ...l, aid } : l
        );
        setAnnLayers(annLayersRef.current);
        if (annSelRef.current === temp) {
          annSelRef.current = aid;
          setAnnSel(aid);
        }
      } catch (e) {
        setHint({ text: String(e), bad: true });
      }
    },
    [boot?.board]
  );

  const annRemove = useCallback(
    (aid: number) => {
      const board = boot?.board;
      annCheckpoint();
      annLayersRef.current = annLayersRef.current.filter((l) => l.aid !== aid);
      setAnnLayers(annLayersRef.current);
      if (annSelRef.current === aid) {
        annSelRef.current = null;
        setAnnSel(null);
      }
      if (board) void boardAnnApi.del(board, aid).catch(() => {});
    },
    [annCheckpoint, boot?.board]
  );

  /** 层级：`dir = +1` 上移一层（画得更晚）/ `-1` 下移一层。
   *  整摞重排 z（1..n），只有真的变了的层才写库。 */
  const annMoveZ = useCallback(
    (aid: number, dir: number) => {
      const sorted = [...annLayersRef.current].sort((a, b) => a.z - b.z || a.aid - b.aid);
      const i = sorted.findIndex((l) => l.aid === aid);
      const j = i + dir;
      if (i < 0 || j < 0 || j >= sorted.length) return;
      annCheckpoint();
      const swap = sorted[i];
      sorted[i] = sorted[j];
      sorted[j] = swap;
      const board = boot?.board;
      const next = sorted.map((l, idx) => {
        const z = idx + 1;
        if (z === l.z) return l;
        if (board) void boardAnnApi.update(board, l.aid, l.x, l.y, z).catch(() => {});
        return { ...l, z };
      });
      annLayersRef.current = next;
      setAnnLayers(next);
    },
    [annCheckpoint, boot?.board]
  );

  const annUndoStep = useCallback(() => {
    const prev = annUndo.current.pop();
    if (!prev) return;
    annRedo.current.push(annSnap());
    setAnnVer((v) => v + 1);
    void annApplySnapshot(prev);
  }, [annApplySnapshot]);

  const annRedoStep = useCallback(() => {
    const next = annRedo.current.pop();
    if (!next) return;
    annUndo.current.push(annSnap());
    setAnnVer((v) => v + 1);
    void annApplySnapshot(next);
  }, [annApplySnapshot]);

  /** 拖拽中的预览：直接画到一块临时 canvas 上（每帧只改 DOM，不走 React）。
   *  马赛克本身是 DOM 层，预览只能画个虚线框说明范围。 */
  const annPaintLive = useCallback((op: AnnOp | null) => {
    const cv = annLiveRef.current;
    if (!cv) return;
    if (!op) {
      cv.style.display = "none";
      cv.getContext("2d")?.clearRect(0, 0, cv.width, cv.height);
      return;
    }
    const b = annBox(op);
    const w = Math.max(1, b.w);
    const h = Math.max(1, b.h);
    const s = annScale(w, h);
    cv.style.display = "block";
    cv.style.left = `${b.x}px`;
    cv.style.top = `${b.y}px`;
    cv.style.width = `${w}px`;
    cv.style.height = `${h}px`;
    const bw = Math.max(1, Math.round(w * s));
    const bh = Math.max(1, Math.round(h * s));
    if (cv.width !== bw) cv.width = bw;
    if (cv.height !== bh) cv.height = bh;
    const ctx = cv.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.scale(s, s);
    ctx.translate(-b.x, -b.y);
    if (op.t === "mosaic") {
      ctx.fillStyle = "rgba(167,139,250,0.14)";
      ctx.fillRect(b.x, b.y, w, h);
      const lw = (1.5 * (window.devicePixelRatio || 1)) / s;
      ctx.strokeStyle = "rgba(167,139,250,0.9)";
      ctx.lineWidth = lw;
      ctx.setLineDash([6 * lw, 4 * lw]);
      ctx.strokeRect(b.x, b.y, w, h);
      ctx.setLineDash([]);
    } else {
      annDraw(ctx, op);
    }
  }, []);

  /** 画布点 → 最上面那一层（局部坐标做精确命中：细长的箭头不会在空处被"擦掉"） */
  const annPick = (p: AnnPt): AnnLayer | null => {
    const arr = [...annLayersRef.current].sort((a, b) => b.z - a.z || b.aid - a.aid);
    for (const l of arr) if (annHit(l.op, p.x - l.x, p.y - l.y)) return l;
    return null;
  };

  /** 起一笔。监听挂在自己身上（而不是并进画布那个大 effect）：
   *  标注手势和拖图/框选/平移是互斥的，混在一起只会让 `settle` 越来越难读。 */
  const annBegin = useCallback(
    (e: React.MouseEvent) => {
      const p = annPoint(e);
      const tool = annTool;
      /** 文字 / 序号用字号，其余用线宽 */
      const texty = tool === "text" || tool === "badge";
      if (tool === "text") {
        annTextLock.current = false;
        setAnnTextAt(p);
        return;
      }
      const op = annNewOp(tool, p, {
        color: annColor,
        w: texty ? annFont : annWeight,
        fill: annFill,
        n: annNextRef.current,
      });
      if (!op) return;
      /* 单击类（序号）：按下就落，没有拖拽阶段 */
      if (op.t === "badge") {
        annCheckpoint();
        void annCommit(op);
        annNextRef.current += 1;
        return;
      }
      let cur: AnnOp = op;
      let moved = false;
      const down = { x: e.clientX, y: e.clientY };
      annPaintLive(cur);
      const onMove = (ev: MouseEvent) => {
        if (!moved && Math.abs(ev.clientX - down.x) + Math.abs(ev.clientY - down.y) > 3) {
          moved = true;
        }
        const q = annPoint(ev);
        /* 从 `cur` 复制一份再收窄：闭包里改的可变变量，TS 不会保留类型收窄 */
        const c = cur;
        if (c.t === "pen" || c.t === "marker") {
          /* 采样抽稀：同一两个像素内连发的点对观感没贡献，只是白占内存 */
          const last = c.pts[c.pts.length - 1];
          if (Math.hypot(q.x - last.x, q.y - last.y) * vp.current.k < 1.2) return;
          cur = { ...c, pts: [...c.pts, q] };
        } else if (
          c.t === "rect" ||
          c.t === "ellipse" ||
          c.t === "line" ||
          c.t === "arrow" ||
          c.t === "mosaic"
        ) {
          cur = { ...c, b: q };
        }
        annPaintLive(cur);
      };
      const onUp = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        window.removeEventListener("blur", onBlur);
        annPaintLive(null);
        /* 手抖点一下（没拖出框）不落层：否则画布上凭空多一层看不见的空标注 */
        if (annTooSmall(cur)) return;
        annCheckpoint();
        void annCommit(cur);
      };
      /* 窗口失焦（Alt+Tab / 被系统抢走鼠标）：这一笔**丢掉**而不是硬提交 ——
         用户没松手就离开，多半是不想要了；预览也不能留在画布上 */
      const onBlur = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        window.removeEventListener("blur", onBlur);
        annPaintLive(null);
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
      window.addEventListener("blur", onBlur);
    },
    [annTool, annColor, annWeight, annFont, annFill, annPoint, annPaintLive, annCheckpoint, annCommit]
  );

  /** 文字工具：回车/失焦落一层。空文本直接丢掉 */
  const annCommitText = useCallback(
    (p: AnnPt, raw: string) => {
      if (annTextLock.current) return;
      annTextLock.current = true;
      setAnnTextAt(null);
      const s = raw.replace(/\s+$/, "");
      if (!s.trim()) return;
      const op = annNewOp("text", p, { color: annColor, w: annFont, fill: false });
      if (!op || op.t !== "text") return;
      annCheckpoint();
      void annCommit({ ...op, s });
    },
    [annColor, annFont, annCheckpoint, annCommit]
  );

  /** 选择工具：按住一层拖位置。拖动期只改那一层的 style，松手才写 state + 落库 */
  const annBeginMove = useCallback(
    (e: React.MouseEvent, layer: AnnLayer) => {
      const s = annPoint(e);
      const el =
        e.target instanceof Element ? e.target.closest<HTMLElement>("[data-ann]") : null;
      let dx = 0;
      let dy = 0;
      let moved = false;
      const onMove = (ev: MouseEvent) => {
        const q = annPoint(ev);
        dx = q.x - s.x;
        dy = q.y - s.y;
        if (!moved && Math.hypot(dx, dy) * vp.current.k < 2) return;
        moved = true;
        if (el) {
          el.style.left = `${layer.x + dx}px`;
          el.style.top = `${layer.y + dy}px`;
        }
      };
      const onUp = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        window.removeEventListener("blur", onUp);
        if (!moved) return;
        annCheckpoint();
        const nx = Math.round(layer.x + dx);
        const ny = Math.round(layer.y + dy);
        const next = annLayersRef.current.map((l) =>
          l.aid === layer.aid ? { ...l, x: nx, y: ny } : l
        );
        annLayersRef.current = next;
        setAnnLayers(next);
        const board = boot?.board;
        if (board) void boardAnnApi.update(board, layer.aid, nx, ny, layer.z).catch(() => {});
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
      /* 失焦也当作松手：位置已经改到 DOM 上了，不落库就会"看着在 A、库里还是 B" */
      window.addEventListener("blur", onUp);
    },
    [annPoint, annCheckpoint, boot?.board]
  );

  /** 图层行点一下：选中 + 视口居中（和图片/便签走同一种"点一行就跳过去"的手感） */
  const focusBox = useCallback(
    (b: Box) => {
      const root = rootRef.current;
      if (!root) return;
      const pad = layersOpen ? LAYERS_W : 0;
      const W = (root.clientWidth || 1200) - pad;
      const H = root.clientHeight || 800;
      const v = vp.current;
      const need = Math.min(1, (W * 0.86) / Math.max(1, b.w), (H * 0.86) / Math.max(1, b.h));
      const k = need < v.k ? Math.max(0.04, need) : v.k;
      v.k = k;
      v.x = pad + W / 2 - (b.x + b.w / 2) * k;
      v.y = H / 2 - (b.y + b.h / 2) * k;
      applyVp();
      setZoom(k);
      recompute();
    },
    [applyVp, recompute, layersOpen]
  );

  useEffect(() => {
    document.body.classList.add("board-body");
    document.documentElement.classList.add("board-body");
    /* 主题：灵感板是独立 webview，不自动继承主窗 data-theme（同 Peek 的做法：
       开窗时由 Rust 把当前主题塞进 boot，这里设到根上，tokens / 板面 CSS 跟着走） */
    document.documentElement.dataset.theme = boot?.theme === "light" ? "light" : "dark";
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
      source.boardLayout(board).catch(() => [] as [number, number, number, number, number, number][]),
      source.boardNotes(board).catch(() => [] as BoardNote[]),
      /* 标注读不回来（老注册库没有这张表 / JSON 坏了）就当这块板没有标注：
         一层坏数据不该让整块板打不开 */
      boardAnnApi.list(board).catch(() => [] as BoardAnnRow[]),
    ]).then(([rows, ns, anns]) => {
      if (!alive) return;
      const groups = new Map<number, number>();
      for (const [g, x, y, w, h, grp] of rows) {
        pos.current.set(g, { x, y, w, h });
        if (grp) groups.set(g, grp);
      }
      /* 便签的坐标进同一个 `pos`，key 用 `-nid`：图片 id 是正数，两边永不相撞 */
      const list: BoardNote[] = ns.map((n) => ({
        ...n,
        font: n.font ?? "",
        fs: n.fs ?? 0,
        color: n.color ?? "",
        bold: !!n.bold,
        /* 老库/老数据没有 vert（迁移后是 0）：读成横排，不会突然变竖排 */
        vert: !!n.vert,
        grp: n.grp ?? 0,
      }));
      for (const n of list) {
        pos.current.set(-n.nid, { x: n.x, y: n.y, w: n.w, h: n.h });
        if (n.grp) groups.set(-n.nid, n.grp);
      }
      /* 标注：坏行（解析不出几何）由 `annDecode` 丢掉，剩下的保持库里的 z 顺序 */
      const layers = anns
        .map((r) => annDecode(r))
        .filter((l): l is AnnLayer => l !== null)
        .sort((a, b) => a.z - b.z || a.aid - b.aid);
      annLayersRef.current = layers;
      setAnnLayers(layers);
      /* 序号接着已有最大号往下发：重开板不该又从 1 开始，否则一圈里两个 1 */
      const maxBadge = layers.reduce((m, l) => (l.op.t === "badge" ? Math.max(m, l.op.n) : m), 0);
      annNextRef.current = maxBadge + 1;
      setGrpMap(groups);
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

  /** 便签的位置/文字/外观记回库。文字与外观都从 `notesRef` 取 —— settle 里那个闭包不跟着 notes 重挂 */
  const noteRemember = useCallback(
    (nid: number) => {
      const board = boot?.board;
      const b = pos.current.get(-nid);
      const n = notesRef.current.find((x) => x.nid === nid);
      if (!board || !b || !n) return;
      void source
        .boardNotePut(
          board,
          nid,
          Math.round(b.x),
          Math.round(b.y),
          Math.round(b.w),
          Math.round(b.h),
          n.text,
          { font: n.font, fs: n.fs, color: n.color, bold: n.bold, vert: n.vert }
        )
        .catch(() => {});
    },
    [boot?.board]
  );

  /** 一批 key（图片正 / 便签负）一次性落库：组拖动、组缩放收尾走这里 */
  const rememberMany = useCallback(
    (keys: number[]) => {
      const board = boot?.board;
      if (!board) return;
      const rows: [number, number, number, number, number][] = [];
      const nids: number[] = [];
      for (const k of keys) {
        const b = pos.current.get(k);
        if (!b) continue;
        if (k > 0) {
          rows.push([k, Math.round(b.x), Math.round(b.y), Math.round(b.w), Math.round(b.h)]);
        } else {
          nids.push(-k);
        }
      }
      if (rows.length) void source.boardPlace(board, rows).catch(() => {});
      for (const nid of nids) noteRemember(nid);
    },
    [boot?.board, noteRemember]
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
    const look = { font: "", fs: NOTE_FS, color: "", bold: false, vert: false };
    try {
      const nid = await source.boardNotePut(board, null, x, y, NOTE_W, NOTE_H, "", look);
      pos.current.set(-nid, { x, y, w: NOTE_W, h: NOTE_H });
      notesRef.current = [
        ...notesRef.current,
        { nid, x, y, w: NOTE_W, h: NOTE_H, text: "", ...look, grp: 0 },
      ];
      setNotes(notesRef.current);
      setEditId(nid);
      /* 新建的便签直接选中：底部那条字体工具条立刻可用，不用再点一下 */
      setSel(new Set([-nid]));
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
        setSel((old) => {
          if (!old.has(-nid)) return old;
          const n = new Set(old);
          n.delete(-nid);
          return n;
        });
        if (board) void source.boardNoteDel(board, nid).catch(() => {});
        return;
      }
      const cur = notesRef.current.find((n) => n.nid === nid);
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
            text,
            cur
              ? { font: cur.font, fs: cur.fs, color: cur.color, bold: cur.bold, vert: cur.vert }
              : { font: "", fs: NOTE_FS_DEF, color: "", bold: false, vert: false }
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

  /* ---------------- 选中 / 打组（0.4.17） ---------------- */

  /** 把 key 扩展成「它所在组的全部成员」。没打组就它自己 */
  const withGroup = useCallback(
    (keys: Iterable<number>) => {
      const out = new Set<number>();
      const want = new Set<number>();
      for (const k of keys) {
        out.add(k);
        const g = grpMap.get(k) ?? 0;
        if (g) want.add(g);
      }
      if (want.size) for (const [k, g] of grpMap) if (want.has(g)) out.add(k);
      return out;
    },
    [grpMap]
  );

  /** 打组：选区里至少两个成员。组号由后端发（MAX+1），前端只记下来 */
  const doGroup = useCallback(async () => {
    const board = boot?.board;
    const keys = [...selRef.current];
    if (!board) return;
    if (keys.length < 2) {
      setHint({ text: t("board.group_need_two"), bad: true });
      return;
    }
    try {
      const g = await source.boardGroup(board, keys, -1);
      setGrpMap((old) => {
        const n = new Map(old);
        for (const k of keys) n.set(k, g);
        return n;
      });
      setHint({ text: t("board.group_done", { n: keys.length }), bad: false });
    } catch (e) {
      setHint({ text: String(e), bad: true });
    }
  }, [boot?.board]);

  /** 解组：选区碰到的每个组，**整组**都拆开（不是只把点中的那一个吐出来） */
  const doUngroup = useCallback(async () => {
    const board = boot?.board;
    if (!board) return;
    const gs = new Set<number>();
    for (const k of selRef.current) {
      const g = grpMap.get(k) ?? 0;
      if (g) gs.add(g);
    }
    if (!gs.size) {
      setHint({ text: t("board.group_none"), bad: true });
      return;
    }
    try {
      let n = 0;
      for (const g of gs) {
        const members: number[] = [];
        for (const [k, gg] of grpMap) if (gg === g) members.push(k);
        await source.boardGroup(board, members, 0);
        n += members.length;
        setGrpMap((old) => {
          const m = new Map(old);
          for (const k of members) m.delete(k);
          return m;
        });
      }
      setHint({ text: t("board.ungroup_done", { groups: gs.size, n }), bad: false });
    } catch (e) {
      setHint({ text: String(e), bad: true });
    }
  }, [boot?.board, grpMap]);

  /** 改便签外观：立刻作用到 DOM + state + 落库（工具条每次点击走一遍） */
  const setNoteLook = useCallback(
    (nid: number, patch: Partial<Pick<BoardNote, "font" | "fs" | "color" | "bold" | "vert">>) => {
      notesRef.current = notesRef.current.map((n) => (n.nid === nid ? { ...n, ...patch } : n));
      setNotes(notesRef.current);
      const n = notesRef.current.find((x) => x.nid === nid);
      const el = canvasRef.current?.querySelector<HTMLElement>(`[data-note="${nid}"]`);
      if (n && el) {
        el.style.fontSize = `${n.fs > 0 ? n.fs : NOTE_FS_DEF}px`;
        el.style.fontFamily = fontCss(n.font);
        el.style.color = n.color;
        el.style.fontWeight = n.bold ? "700" : "400";
      }
      noteRemember(nid);
    },
    [noteRemember]
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
    setHint({ text: t("board.tidy_done", { n: order.length }), bad: false });
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

  /* 缩略图失败重试（见 `THUMB_RETRY`）：`<img onError>` 走这里。
     次数用 ref 记（`onError` 里读 state 会拿到挂载时那个旧闭包），次数进 `src` 的 `&rt=`
     并给 `<img>` 换 `key` —— 只改 src 不换 key 时 WebView 可能仍用那条失败记录。 */
  const retryThumb = useCallback((id: number) => {
    const n = thumbTries.current.get(id) ?? 0;
    if (n >= THUMB_RETRY) {
      setBroken((old) => (old.has(id) ? old : new Set(old).add(id)));
      return;
    }
    /* 同一张已经排着一次重试就别再排队（onError 可能连发） */
    if (thumbTimers.current.has(id)) return;
    const t = window.setTimeout(() => {
      thumbTimers.current.delete(id);
      thumbTries.current.set(id, n + 1);
      setThumbTry((m) => new Map(m).set(id, n + 1));
    }, THUMB_RETRY_MS * (n + 1));
    thumbTimers.current.set(id, t);
  }, []);

  /* 关窗时把排着的重试都撤掉（不然定时器还会往一个已卸载的组件里 setState） */
  useEffect(
    () => () => {
      for (const t of thumbTimers.current.values()) window.clearTimeout(t);
      thumbTimers.current.clear();
    },
    []
  );

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
  /** 拖动中的一个成员：起始盒子 + DOM。
   *  `el` 可能是 null —— 组里**视口外**的成员没挂 `<img>`/便签节点，它照样要跟着走，
   *  只是没有 DOM 可改（位置写进 `pos`，滚回视口时自然是对的）。 */
  interface DragMember {
    key: number;
    box: Box;
    el: HTMLElement | null;
    /** 便签的起始字号（缩放时按比例乘） */
    fs: number;
  }
  /**
   * 框选（0.4.23）：**左键在空白处拖出的矩形**（中键才是平移）。
   * `active` = 已经拖过 3px，没到 3px 就松手当作"点空白清选区"。
   */
  const marq = useRef<{ sx: number; sy: number; add: boolean; active: boolean } | null>(null);
  const marqRef = useRef<HTMLDivElement | null>(null);
  const drag = useRef<{
    /** move/size = 单张；gmove/gsize = 整组（`members` 是所有成员） */
    kind: "move" | "size" | "gmove" | "gsize";
    id: number;
    sx: number;
    sy: number;
    box: Box;
    members: DragMember[];
    /** gsize：组包围盒（左上角固定，等比缩放） */
    gbox?: Box;
    /** 拖的是便签（`id` 是 `-nid`）：收尾要写 board_notes，不写 board_items */
    note?: boolean;
    /** 拖图只认左键（下面的 `e.button === 0` 已限定），所以恒为 1 —— 见 onDown 里的说明 */
    mask: number;
  } | null>(null);
  const tick = useRef(0);
  /** Alt+滚轮改大小的落库定时器（停止滚动 400ms 后写一次） */
  const sizeSave = useRef(0);

  /** 按 key 找画布里的元素（图片正 / 便签负） */
  const elOf = (key: number) =>
    canvasRef.current?.querySelector<HTMLElement>(
      key > 0 ? `[data-img="${key}"]` : `[data-note="${-key}"]`
    ) ?? null;

  /** 一组 key 的包围盒（不在 `pos` 里的忽略）。组框、整组缩放都靠它。
   *  便签的 `pos.w/h` 是上一次量的，可能刚改过字体还没更新 —— 直接从 DOM 取当前值。 */
  const bboxOf = (keys: Iterable<number>): Box | null => {
    let x0 = Infinity;
    let y0 = Infinity;
    let x1 = -Infinity;
    let y1 = -Infinity;
    let any = false;
    for (const k of keys) {
      const raw = pos.current.get(k);
      if (!raw) continue;
      let b = raw;
      if (k < 0) {
        const el = elOf(k);
        if (el?.offsetWidth) b = { ...raw, w: el.offsetWidth, h: el.offsetHeight };
      }
      any = true;
      x0 = Math.min(x0, b.x);
      y0 = Math.min(y0, b.y);
      x1 = Math.max(x1, b.x + b.w);
      y1 = Math.max(y1, b.y + b.h);
    }
    return any ? { x: x0, y: y0, w: Math.max(1, x1 - x0), h: Math.max(1, y1 - y0) } : null;
  };

  /** 把「选中框」直接摆到当前选区包围盒上：拖动中位置在变，不能等 React 重渲染 */
  const syncSelBox = () => {
    const el = canvasRef.current?.querySelector<HTMLElement>(".board-selbox");
    if (!el) return;
    const bb = bboxOf(selRef.current);
    if (!bb) {
      el.style.display = "none";
      return;
    }
    el.style.display = "";
    el.style.left = `${bb.x}px`;
    el.style.top = `${bb.y}px`;
    el.style.width = `${bb.w}px`;
    el.style.height = `${bb.h}px`;
  };

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;

    const onWheel = (e: WheelEvent) => {
      /* 面板里的滚轮 = 滚那个列表，不是缩放画布。
         必须在 `preventDefault` **之前**放行，否则列表滚不动 ——
         字体弹层（0.4.24）踩过这个：一千多款字体，滚轮全被画布吃了。 */
      const tgt = e.target instanceof HTMLElement ? e.target : null;
      if (tgt?.closest(".board-layers") || tgt?.closest(".board-fontpop")) return;
      /* 标注工具条上的滚轮留给它自己（粗细将来要能滚，至少也别把画布缩掉） */
      if (tgt?.closest(".board-anntools")) return;
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
    /** 把一个成员的新盒子写进 `pos`；有 DOM 的顺手改 style（没有的就只是坐标跟着走）。
     *  **便签只改位置和字号** —— 它的宽高由文字决定，写死 width/height 就变成"大框套小字"了。 */
    const applyMember = (m: DragMember, nb: Box, fs?: number) => {
      pos.current.set(m.key, nb);
      const fsv = fs == null ? undefined : Math.max(4, fs);
      if (m.el) {
        m.el.style.left = `${nb.x}px`;
        m.el.style.top = `${nb.y}px`;
        if (m.key > 0) {
          m.el.style.width = `${nb.w}px`;
          m.el.style.height = `${nb.h}px`;
        }
        if (fsv != null) m.el.style.fontSize = `${fsv}px`;
      }
      if (fsv != null && m.key < 0) {
        /* 便签字号跟着缩放走：写进 notes（渲染用），收尾统一 setNotes 一次 */
        notesRef.current = notesRef.current.map((n) =>
          n.nid === -m.key ? { ...n, fs: Math.round(fsv * 100) / 100 } : n
        );
      }
    };

    const settle = () => {
      if (pan.current) {
        pan.current = null;
        rootRef.current?.classList.remove("panning");
        recompute();
      }
      if (marq.current) {
        marq.current = null;
        rootRef.current?.classList.remove("marquee");
        if (marqRef.current) marqRef.current.style.display = "none";
      }
      const d = drag.current;
      if (!d) return;
      /* 位置在 `onMove` 里已经逐个写进 `pos` 了，这里只做两件事：落库 + 让 React 追一次 */
      for (const m of d.members) {
        /* 便签的尺寸是文字决定的：落库前量一次，别把上一轮字号留下的旧宽高写进去 */
        if (m.key < 0 && m.el) {
          const b = pos.current.get(m.key);
          if (b) pos.current.set(m.key, { ...b, w: m.el.offsetWidth, h: m.el.offsetHeight });
        }
      }
      rememberMany(d.members.map((m) => m.key));
      if (d.members.some((m) => m.key < 0)) setNotes(notesRef.current);
      drag.current = null;
      rootRef.current?.classList.remove("dragging-img");
      syncSelBox();
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
      /* 框选：矩形只改 DOM（不走 React），命中集合变了才更新选区 */
      const mq = marq.current;
      if (mq) {
        const x = Math.min(mq.sx, e.clientX);
        const y = Math.min(mq.sy, e.clientY);
        const w = Math.abs(e.clientX - mq.sx);
        const h = Math.abs(e.clientY - mq.sy);
        const box = marqRef.current;
        if (box) {
          box.style.left = `${x}px`;
          box.style.top = `${y}px`;
          box.style.width = `${w}px`;
          box.style.height = `${h}px`;
        }
        if (!mq.active && w + h > 3) {
          mq.active = true;
          rootRef.current?.classList.add("marquee");
        }
        if (mq.active) {
          const root = rootRef.current;
          if (root) {
            const r = root.getBoundingClientRect();
            const v = vp.current;
            /* 屏幕矩形 → 画布矩形（框选永远按"碰到就算"，和 Adobe AI 一致） */
            const cx0 = (x - r.left - v.x) / v.k;
            const cy0 = (y - r.top - v.y) / v.k;
            const cx1 = (x + w - r.left - v.x) / v.k;
            const cy1 = (y + h - r.top - v.y) / v.k;
            const next = new Set<number>();
            if (mq.add) for (const k of selRef.current) next.add(k);
            for (const [k, b] of pos.current) {
              if (b.x + b.w >= cx0 && b.x <= cx1 && b.y + b.h >= cy0 && b.y <= cy1) next.add(k);
            }
            const same =
              next.size === selRef.current.size && [...next].every((k) => selRef.current.has(k));
            if (!same) {
              selRef.current = next;
              setSel(next);
            }
          }
        }
        return;
      }
      const d = drag.current;
      if (!d) return;
      const dx = (e.clientX - d.sx) / vp.current.k;
      const dy = (e.clientY - d.sy) / vp.current.k;

      if (d.kind === "gmove") {
        /* 整组移动：每个成员各加同一份位移（各自起点 + 位移，不做累计 —— 不会漂） */
        for (const m of d.members) {
          applyMember(m, { ...m.box, x: m.box.x + dx, y: m.box.y + dy });
        }
        syncSelBox();
        return;
      }
      if (d.kind === "gsize") {
        /* 整组等比缩放：锚点是组包围盒的左上角，取位移大的轴当主导。
           图片的 w/h、便签的 w/h/字号一起乘 `s` —— 组内的相对位置和文字大小都不会走形。 */
        const g = d.gbox;
        if (!g) return;
        const byW = (g.w + dx) / g.w;
        const byH = (g.h + dy) / g.h;
        const s = Math.min(20, Math.max(0.05, Math.abs(dx) > Math.abs(dy) ? byW : byH));
        for (const m of d.members) {
          applyMember(
            m,
            m.key < 0
              ? { ...m.box, x: g.x + (m.box.x - g.x) * s, y: g.y + (m.box.y - g.y) * s }
              : {
                  x: g.x + (m.box.x - g.x) * s,
                  y: g.y + (m.box.y - g.y) * s,
                  w: m.box.w * s,
                  h: m.box.h * s,
                },
            m.key < 0 ? m.fs * s : undefined
          );
        }
        syncSelBox();
        return;
      }

      const m = d.members[0];
      if (!m) return;
      if (d.kind === "move") {
        applyMember(m, { ...d.box, x: d.box.x + dx, y: d.box.y + dy });
        return;
      }
      /* 改大小：**保持比例**，取位移大的那个轴当主导，另一个按比例跟。
         这样拖角往哪边推都自然，也不会把图拉变形。 */
      if (d.note) {
        /* 便签没有"框"可拉：拖角 = 改字号，文字变大、框自己跟着长 */
        const s = Math.max(
          0.05,
          Math.min(
            40,
            1 +
              (Math.abs(dx) > Math.abs(dy)
                ? dx / Math.max(24, d.box.w)
                : dy / Math.max(24, d.box.h))
          )
        );
        applyMember(m, d.box, m.fs * s);
        return;
      }
      const byW = d.box.w + dx;
      const byH = (d.box.h + dy) * (d.box.w / d.box.h);
      const w = Math.min(MAX_W, Math.max(MIN_W, Math.abs(dx) > Math.abs(dy) ? byW : byH));
      const s = w / d.box.w;
      applyMember(m, { x: d.box.x, y: d.box.y, w, h: d.box.h * s });
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
  }, [applyVp, recompute, rememberMany]);

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
    /* **Shift + 左键拖 = 移动整个窗口**（owner 2026-11-21），**压在图上也算**。
       板子是无边框窗（decorations(false)），能拖窗口的只有顶栏那条 drag-region；
       用户的第一反应是抓画面里任意一处去挪窗 —— 抓空白会拉出框选蓝框，
       抓图则会开始拖那张图。Shift 这条手势把两种都接过来：
       窗口跟指针走、画布内容纹丝不动（和"Shift + 滚轮挪窗口"同一语义，只是换成拖）。
       必须放在图片/便签/手柄判定**之前**：晚一步 `drag.current` 已经起手，
       同一份位移会被用两次（图跟着窗口跑）。加选因此让给 Ctrl/⌘。 */
    if (e.button === 0 && e.shiftKey) {
      e.preventDefault();
      void (async () => {
        try {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          await getCurrentWindow().startDragging();
        } catch {
          /* 浏览器预览里没有 Tauri 运行时：这条手势当没有 */
        }
      })();
      return;
    }
    const noteEl = t.closest<HTMLElement>("[data-note]");
    const handle = t.closest<HTMLElement>("[data-handle]");
    const ghandle = t.closest<HTMLElement>("[data-group-handle]");
    const cell = t.closest<HTMLElement>("[data-img]");

    /* ---------------- 标注模式 ----------------
       必须在图片/便签判定**之前**截走这一下：工具选着「矩形」时，压在图上拖
       也应该是画框，不是把那张图挪走。只有「选择」才落到原来的拖图/框选逻辑上。 */
    if (annOpen && e.button === 0 && !t.closest(".board-anntools")) {
      /* 文字输入框开着：这一下先把它提交掉（空文本自己会丢弃），别在新位置又开一个 */
      if (annTextAt) {
        annTextRef.current?.blur();
        e.preventDefault();
        return;
      }
      const p = annPoint(e);
      if (annTool === "eraser") {
        const hit = annPick(p);
        if (hit) annRemove(hit.aid);
        e.preventDefault();
        return;
      }
      if (annTool === "select") {
        const hit = annPick(p);
        if (hit) {
          annSelRef.current = hit.aid;
          setAnnSel(hit.aid);
          annBeginMove(e, hit);
          e.preventDefault();
          return;
        }
        /* 按在某一层的包围盒里、但没压到墨迹（例如空心矩形的中间）：
           这一下算"点在标注上"，不许落到框选 —— 否则在大框中间拖会拉起选框，
           看起来像"选中了框后面的图"。 */
        if (t.closest("[data-ann]")) {
          e.preventDefault();
          return;
        }
        /* 点空白：只清标注的选中，框选 / 拖图照旧 */
        if (annSelRef.current != null) {
          annSelRef.current = null;
          setAnnSel(null);
        }
      } else {
        e.preventDefault();
        annBegin(e);
        return;
      }
    }

    /** 按 key 收集要一起动的成员。DOM 可能还没有（视口外的成员），照样进列表 */
    const buildMembers = (keys: number[]): DragMember[] => {
      const out: DragMember[] = [];
      for (const k of keys) {
        const raw = pos.current.get(k);
        if (!raw) continue;
        const el = elOf(k);
        const fs = k < 0 ? (notesRef.current.find((n) => n.nid === -k)?.fs || NOTE_FS_DEF) : 0;
        /* 便签的宽高跟着文字走：以 DOM 当前量为准，`pos` 里的可能是上一轮字号留下的 */
        const box =
          k < 0 && el?.offsetWidth ? { ...raw, w: el.offsetWidth, h: el.offsetHeight } : raw;
        out.push({ key: k, box, el, fs });
      }
      return out;
    };

    /* 组框右下角 = 整组等比缩放。必须排在单张判定前面：手柄压在成员上面 */
    if (ghandle && e.button === 0 && selRef.current.size > 1) {
      const members = buildMembers([...selRef.current]);
      const gbox = bboxOf(selRef.current);
      if (members.length && gbox) {
        drag.current = {
          kind: "gsize",
          id: members[0].key,
          sx: e.clientX,
          sy: e.clientY,
          box: gbox,
          members,
          gbox,
          mask: 1,
        };
        rootRef.current?.classList.add("dragging-img");
        e.preventDefault();
        return;
      }
    }

    if ((noteEl || cell) && e.button === 0) {
      const key = noteEl ? -Number(noteEl.dataset.note) : Number(cell!.dataset.img);
      /* Ctrl / ⌘ = 加选减选（Shift 让给"拖窗口"了，见 `onDown` 开头）。**这一下不启动拖动** ——
         想多选却手一抖把它拖走，是这类画布最常见的怨气来源。 */
      if (e.ctrlKey || e.metaKey) {
        const hit = withGroup([key]);
        const next = new Set(selRef.current);
        const on = next.has(key);
        for (const k of hit) {
          if (on) next.delete(k);
          else next.add(k);
        }
        selRef.current = next;
        setSel(next);
        e.preventDefault();
        return;
      }
      /* 点中的东西已经在多选里 → 保持选区一起拖；否则选区换成它（或它所在的整组） */
      const keep = selRef.current.has(key) && selRef.current.size > 1;
      const next = keep ? selRef.current : withGroup([key]);
      if (!keep) {
        selRef.current = next;
        setSel(next);
      }
      if (cell && !noteEl) {
        /* 最近点的那张放到最顶层（需求原话：不需要 PS 那种层叠序，点谁谁在上面） */
        setTopId(Number(cell.dataset.img));
      }
      const members = buildMembers([...next]);
      if (!members.length) return;
      const group = members.length > 1;
      drag.current = {
        kind: group ? "gmove" : handle ? "size" : "move",
        id: key,
        sx: e.clientX,
        sy: e.clientY,
        box: at(key),
        members,
        note: key < 0,
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
    /* 空白处按下（0.4.23 改手感，照 Adobe AI）：
       **左键 = 框选**，**中键按住拖 = 平移画布**。
       左键不再平移 —— 那块空白是"拉一个框把东西圈起来"的地方。 */
    if (e.button === 1) {
      /* 中键按下去 WebView2 会在指针处插一个"自动滚动"锚点（那个四向箭头），
         画布跟着它滚会和我们的平移打架 —— 主流做法是直接吃掉默认行为
         （仓库里 `Snip.tsx:537` 就是同一条惯例）。 */
      e.preventDefault();
      pan.current = {
        sx: e.clientX,
        sy: e.clientY,
        ox: vp.current.x,
        oy: vp.current.y,
        /* 按下那一刻的位掩码原样记下来：左键=1、中键=4，鼠标侧键是 8/16。
           以前写死 `e.button === 1 ? 4 : 1`，侧键起手会拿到 1 → 下一帧
           `buttons & 1` 为 0 就被判成"已松手"，光标闪一下。 */
        mask: e.buttons || 4,
      };
      rootRef.current?.classList.add("panning");
      /* `.panning` 的清除统一交给上面那个 effect 的 `settle`（mouseup / 丢事件 / 失焦
         三种收尾都经它），这里不再另挂一个 once 监听 —— 两条路清同一个状态容易不同步。 */
      return;
    }
    if (e.button === 0) {
      /* 框选：起点记在屏幕坐标里，命中判定时再换算回画布坐标。
         **加选改成 Ctrl/⌘**：Shift 已经被"拖窗口"占走（同 Adobe 系里 Shift 是加选，
         但这里窗口没有别的手势能挪，取舍在 owner 那边定了）。 */
      marq.current = { sx: e.clientX, sy: e.clientY, add: e.ctrlKey || e.metaKey, active: false };
      if (!marq.current.add && selRef.current.size) {
        selRef.current = new Set();
        setSel(selRef.current);
      }
      const box = marqRef.current;
      if (box) {
        box.style.display = "block";
        box.style.left = `${e.clientX}px`;
        box.style.top = `${e.clientY}px`;
        box.style.width = "0px";
        box.style.height = "0px";
      }
      e.preventDefault();
      return;
    }
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
          ? t("board.paste_snap", { name: snap.name })
          : coll
            ? t("board.paste_coll", { name: coll.name })
            : t("board.paste_tag", { tag: tag ?? "" }),
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
          setHint({ text: t("board.add_paths_done", { n: items.length }), bad: false });
        }
        if (bad.length) {
          setHint({
            text:
              bad.length === 1
                ? bad[0]
                : t("board.add_paths_bad", { n: bad.length, first: bad[0] }),
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
      /* Ctrl+G / Ctrl+Shift+G = 打组 / 解组（0.4.17）。组内成员一起移动、一起缩放 */
      if (mod && (e.key === "g" || e.key === "G")) {
        e.preventDefault();
        if (!e.repeat) void (e.shiftKey ? doUngroup() : doGroup());
        return;
      }
      /* Ctrl+Z / Ctrl+Shift+Z = 标注的撤销 / 重做（只在工具条开着时接管：
         板本身没有别的撤销语义，抢走 Ctrl+Z 不会影响别处）。 */
      if (annOpen && mod && e.key.toLowerCase() === "z") {
        e.preventDefault();
        if (!e.repeat) (e.shiftKey ? annRedoStep : annUndoStep)();
        return;
      }
      /* Delete / Backspace = 删掉选中的那层标注 */
      if (annSelRef.current != null && (e.key === "Delete" || e.key === "Backspace")) {
        e.preventDefault();
        if (!e.repeat) annRemove(annSelRef.current);
        return;
      }
      if (e.key === "Escape") {
        if (menu) {
          setMenu(null);
          return;
        }
        /* 标注的三层退让：先退文字输入，再退图层选中，最后才关工具条 */
        if (annTextAt) {
          setAnnTextAt(null);
          return;
        }
        if (annSelRef.current != null) {
          annSelRef.current = null;
          setAnnSel(null);
          return;
        }
        if (annOpen) {
          setAnnOpen(false);
          setAnnPalette(false);
          return;
        }
        /* 有选区先清选区：Esc 的第一层语义是"取消选择"，第二层才是关板 */
        if (selRef.current.size) {
          selRef.current = new Set();
          setSel(selRef.current);
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
  }, [
    fit,
    menu,
    reset100,
    active,
    items,
    paste,
    addNote,
    doGroup,
    doUngroup,
    annOpen,
    annTextAt,
    annRedoStep,
    annUndoStep,
    annRemove,
  ]);

  /* ---------------- 菜单 ---------------- */

  /** 选区相关的菜单项（打组 / 解组）：多选或选中的是组时才出现 */
  const groupMenu = (): CtxItem[] => {
    const out: CtxItem[] = [];
    if (sel.size > 1) {
      out.push({
        label: t("board.menu_group", { n: sel.size }),
        icon: "grid",
        shortcut: "Ctrl+G",
        action: () => void doGroup(),
      });
    }
    if ([...sel].some((k) => grpMap.get(k))) {
      out.push({
        label: t("board.menu_ungroup"),
        icon: "grid",
        shortcut: "Ctrl+Shift+G",
        action: () => void doUngroup(),
      });
    }
    return out.length ? [...out, { kind: "separator" }] : [];
  };

  const menuItems: CtxItem[] = useMemo(() => {
    /* 标注层的菜单：调层级 / 删掉（右键画布上那一层，或图层面板里那一行） */
    if (menu?.ann != null) {
      const aid = menu.ann;
      const l = annLayers.find((x) => x.aid === aid);
      return [
        {
          kind: "heading",
          label: l ? tr(annKindKey(l.kind)) : tr("board.ann.section"),
        },
        { label: tr("board.ann.menu_up"), icon: "sort", action: () => annMoveZ(aid, 1) },
        { label: tr("board.ann.menu_down"), icon: "sort", action: () => annMoveZ(aid, -1) },
        { kind: "separator" },
        {
          label: tr("board.ann.menu_del"),
          icon: "trash",
          danger: true,
          action: () => annRemove(aid),
        },
      ];
    }
    /* 便签的菜单只有两件事：改字、删掉 */
    if (menu?.note != null) {
      const nid = menu.note;
      return [
        {
          label: t("board.menu_edit_note"),
          icon: "file",
          action: () => setEditId(nid),
        },
        { kind: "separator" },
        {
          label: t("board.menu_del_note"),
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
        ...groupMenu(),
        {
          label: t("board.menu_add_note"),
          icon: "file",
          shortcut: "T",
          action: () => void addNote(),
        },
        {
          label: t("board.menu_tidy"),
          icon: "grid",
          action: tidy,
        },
        /* 「标注工具」= owner 要的那条入口：右键打开截图那套工具条（再点一次收起） */
        {
          label: annOpen ? tr("board.ann.close") : tr("board.ann.open"),
          icon: "sliders",
          action: () => {
            setAnnOpen((v) => !v);
            setAnnPalette(false);
          },
        },
        { kind: "separator" },
        {
          label: t("board.menu_drag_hint"),
          icon: "import",
          action: () => setHint({ text: t("board.menu_drag_bubble"), bad: false }),
        },
        {
          label: t("board.menu_paste"),
          icon: "copy",
          shortcut: "Ctrl+V",
          action: () => void paste(),
        },
        { kind: "separator" },
        { label: t("board.menu_fit"), icon: "expand", shortcut: "F", action: fit },
        { label: t("board.menu_reset"), icon: "search", shortcut: "0", action: reset100 },
        { kind: "separator" },
        { label: t("board.menu_close"), icon: "close", shortcut: "Esc", danger: true, action: () => void source.boardClose() },
      ];
    }
    return [
      {
        label: t("board.menu_open_default"),
        icon: "file",
        action: () => void source.openDefault(it.path).catch(() => {}),
      },
      {
        label: t("board.menu_open_peek"),
        icon: "expand",
        action: () => void source.peekOpen([it.id], 0).catch(() => {}),
      },
      {
        label: t("board.menu_copy_file"),
        icon: "copy",
        shortcut: "Ctrl+C",
        action: () => void source.peekCopyFile([it.path]).catch(() => {}),
      },
      {
        label: t("board.menu_save_as"),
        icon: "file",
        action: () => void source.saveAs(it.path, it.name).catch((e) => setHint({ text: String(e), bad: true })),
      },
      {
        label: t("board.menu_reveal"),
        icon: "reveal",
        action: () => void source.reveal(it.path).catch(() => {}),
      },
      { kind: "separator" },
      /* 图上的右键也给这条入口：用户大多是先在图上点右键 */
      {
        label: annOpen ? tr("board.ann.close") : tr("board.ann.open"),
        icon: "sliders",
        action: () => {
          setAnnOpen((v) => !v);
          setAnnPalette(false);
        },
      },
      { kind: "separator" },
      { label: t("board.menu_fit"), icon: "expand", shortcut: "F", action: fit },
      { label: t("board.menu_reset"), icon: "search", shortcut: "0", action: reset100 },
      { kind: "separator" },
      { label: t("board.menu_close"), icon: "close", shortcut: "Esc", danger: true, action: () => void source.boardClose() },
    ];
  }, [
    menu,
    items,
    notes,
    fit,
    reset100,
    paste,
    addNote,
    tidy,
    delNote,
    sel,
    grpMap,
    doGroup,
    doUngroup,
    annOpen,
    annLayers,
    annMoveZ,
    annRemove,
  ]);

  if (!boot) {
    return <div className="board-miss">{t("board.missing_all")}</div>;
  }

  const shown = items.filter((it) => vis.has(it.id));
  /** 图层面板的行：**图片 + 文字**，按画布上的视觉顺序（先上后下、同一行从左到右）排。
   *  以前只列图片 —— 文字便签在板上却不在图层里，找起来只能靠眼睛扫画布。 */
  const layerRows = useMemo(() => {
    type Row = { key: number; y: number; x: number; img?: BoardItem; note?: BoardNote };
    const rows: Row[] = [];
    for (const it of items) {
      const b = pos.current.get(it.id) ?? boxById.get(it.id);
      rows.push({ key: it.id, y: b?.y ?? 0, x: b?.x ?? 0, img: it });
    }
    for (const n of notes) {
      const b = pos.current.get(-n.nid) ?? { x: n.x, y: n.y };
      rows.push({ key: -n.nid, y: b.y, x: b.x, note: n });
    }
    rows.sort((a, b) => Math.round(a.y / 80) - Math.round(b.y / 80) || a.x - b.x);
    return rows;
    /* `layersOpen` 进依赖：重新打开面板时按当前位置重排一次 */
  }, [items, notes, boxById, layersOpen]);
  /* 标注行：最上层排最前（PS 惯例），和画布的绘制顺序正好相反 */
  const annRows = useMemo(
    () => [...annLayers].sort((a, b) => b.z - a.z || b.aid - a.aid),
    [annLayers]
  );
  /* 「矩形 1」「文字 2」：同类按**创建顺序**编号（只由 aid 决定，所以重开板编号不变） */
  const annName = useMemo(() => {
    const out = new Map<number, string>();
    const cnt = new Map<string, number>();
    for (const l of [...annLayers].sort((a, b) => a.aid - b.aid)) {
      const n = (cnt.get(l.kind) ?? 0) + 1;
      cnt.set(l.kind, n);
      out.set(l.aid, `${tr(annKindKey(l.kind))} ${n}`);
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [annLayers]);
  /* 选区包围盒：只在"多选 / 选中了组"时显示。拖动中由 `syncSelBox` 直接改 DOM */
  const selBox = sel.size > 1 ? bboxOf(sel) : null;
  /* 底部字体工具条作用的那张便签：正在编辑的那张优先，否则选区里只有一张便签时 */
  const lookNote = (() => {
    if (editId != null) return notes.find((n) => n.nid === editId) ?? null;
    if (sel.size !== 1) return null;
    const k = [...sel][0];
    return k < 0 ? (notes.find((n) => n.nid === -k) ?? null) : null;
  })();
  /* 本机字体里剔掉"常用"那几款：下拉里不该出现两遍同一个字体 */
  const extraFonts = useMemo(() => sysFonts.filter((f) => !COMMON_FONT_VALS.has(f)), [sysFonts]);
  /* 便签上的字体名可能不在上面任何一组里（别的机器设的 / 字体后来卸了）——
     不补一条 option 的话，下拉会显示空白，看起来像"字体丢了" */
  const strayFont =
    lookNote?.font && !COMMON_FONT_VALS.has(lookNote.font) && !extraFonts.includes(lookNote.font)
      ? lookNote.font
      : "";
  /* 字体族名 → 显示名：内置的几家走译文，本机枚举出来的直接显示族名 */
  const fontLabel = (v: string) => {
    const key = FONTS.find(([f]) => f === v)?.[1];
    return key ? t(key) : v;
  };
  /* 字体搜索：常用排在前面，本机的全部字体跟在后面（每行用自己的字体渲染自己名字） */
  const fontHits = useMemo(() => {
    const q = fontQ.trim().toLowerCase();
    const common: { v: string; label: string }[] = [
      { v: "", label: t("board.font_default") },
      ...FONTS.filter(([v]) => v).map(([v, key]) => ({ v, label: key ? t(key) : v })),
    ];
    const hitC = q
      ? common.filter((o) => o.label.toLowerCase().includes(q) || o.v.toLowerCase().includes(q))
      : common;
    const all = extraFonts.map((f) => ({ v: f, label: f }));
    const hitA = q ? all.filter((o) => o.label.toLowerCase().includes(q)) : all;
    return { common: hitC, all: hitA };
  }, [fontQ, extraFonts]);

  /* 快照栈住在 ref 里，改它不会重渲染；`annVer` 就是"栈变过"的信号，
     拿它当缓存键算一次可用态（按钮的禁用样式靠这个） */
  const annCanUndo = useMemo(() => annUndo.current.length > 0, [annVer]);
  const annCanRedo = useMemo(() => annRedo.current.length > 0, [annVer]);

  /** 粗细 / 字号步进。文字和序号工具下这个数就是字号（和截图工具条同一套语义） */
  const annStepW = (dir: number, big: boolean) => {
    if (annTool === "text" || annTool === "badge") {
      setAnnFont((v) =>
        Math.min(ANN_FONT_MAX, Math.max(ANN_FONT_MIN, Math.round(v + dir * (big ? 4 : 1))))
      );
    } else {
      setAnnWeight((v) =>
        Math.min(ANN_W_MAX, Math.max(ANN_W_MIN, Math.round((v + dir * (big ? 1 : 0.5)) * 10) / 10))
      );
    }
  };
  const annTexty = annTool === "text" || annTool === "badge";
  /** 工具条上那个读数：线宽还是字号，看当前工具 */
  const annValue = annTexty ? annFont : annWeight;

  /* 粗细/字号支持滚轮（owner 2026-11-23，和截图工具条同一手感）。
     必须原生非 passive 挂：React 的 onWheel 在 WebView2 上是 passive，`preventDefault`
     无效 —— 而且这里不只要吞掉滚动，还得 `stopPropagation`，否则板子的滚轮缩放会跟着动。
     触控板会连发很小的 delta，累计 ≥40 才走一档，和截图那边同一个阈值。 */
  const weightRef = useRef<HTMLDivElement | null>(null);
  const annStepRef = useRef(annStepW);
  annStepRef.current = annStepW;
  useEffect(() => {
    const el = weightRef.current;
    if (!el || !annOpen) return;
    let acc = 0;
    let timer = 0;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      e.stopPropagation();
      acc += e.deltaY;
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        acc = 0;
      }, 220);
      if (Math.abs(acc) < 40) return;
      annStepRef.current(acc < 0 ? 1 : -1, e.shiftKey);
      acc = 0;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      el.removeEventListener("wheel", onWheel);
      window.clearTimeout(timer);
    };
  }, [annOpen]);

  return (
    <div
      ref={rootRef}
      className={`board-root ${annOpen && annTool !== "select" ? "annotating" : ""}`}
      onMouseDown={onDown}
      onDoubleClick={(e) => {
        const t = e.target as HTMLElement;
        /* 双击顶栏是「最大化/还原窗口」（Tauri 内置的 drag-region 行为），
           不该顺带把画布 fit 一遍 —— 和 onMouseDown 用同一条排除规则。
           标注工具条也一样：连点两下工具按钮不该把整块板重缩一遍。 */
        if (t.closest("[data-tauri-drag-region]") || t.closest(".board-anntools")) return;
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
        /* 标注：画布上那一层（data-ann）或图层面板里那一行（data-ann-row） */
        const annEl = t.closest<HTMLElement>("[data-ann]");
        const annRow = t.closest<HTMLElement>("[data-ann-row]");
        /* 图层面板里的行也能右键：图片用"这张图"的菜单，文字行用便签菜单（改字/删掉） */
        const layer = t.closest<HTMLElement>("[data-layer]");
        const layerNote = t.closest<HTMLElement>("[data-layer-note]");
        if (!cell && !note && !annEl && !annRow && t.closest(".board-layers") && !layer && !layerNote) {
          // 面板空白处：没有可作用于的对象，直接不弹（否则会弹出"画布"菜单，语义是错的）
          setMenu(null);
          return;
        }
        setMenu({
          x: e.clientX,
          y: e.clientY,
          id: cell ? Number(cell.dataset.img) : layer ? Number(layer.dataset.layer) : null,
          note: note
            ? Number(note.dataset.note)
            : layerNote
              ? Number(layerNote.dataset.layerNote)
              : undefined,
          ann: annEl
            ? Number(annEl.dataset.ann)
            : annRow
              ? Number(annRow.dataset.annRow)
              : undefined,
        });
      }}
    >
      <div ref={canvasRef} className="board-canvas">
        {shown.map((it) => {
          const b = pos.current.get(it.id) ?? boxById.get(it.id)!;
          /* 第几次取这张缩略图：0 = 第一次。非 0 时给 URL 加个 `rt` 并把 `<img>` 换 key，
             逼 WebView 重新发一次请求（只改 src 时它可能直接给缓存里那条失败记录）。 */
          const tryNo = thumbTry.get(it.id) ?? 0;
          return (
            <div
              key={it.id}
              data-img={it.id}
              className={`board-cell ${active === it.id ? "on" : ""} ${topId === it.id ? "top" : ""} ${sel.has(it.id) ? "sel" : ""} ${grpMap.get(it.id) ? "in-group" : ""} ${broken.has(it.id) ? "broken" : ""}`}
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
                    key={`t${tryNo}`}
                    src={tryNo ? `${it.src}&rt=${tryNo}` : it.src}
                    alt={it.name}
                    draggable={false}
                    decoding="async"
                    loading="lazy"
                    onError={() => retryThumb(it.id)}
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
              <span className="board-handle" data-handle title={t("board.handle_resize")} />
            </div>
          );
        })}

        {/* 标注层：一笔一层，各自一块位图，画在**本层自己的局部坐标**里。
            插在图片和便签**之间**是有意的：图片 z≤3、标注 3、便签 4；同 z 时后者压前者，
            所以标注恒在图片之上、恒在便签之下（和"便签是贴在最上面的注记"的语义一致）。
            只有「选择 / 橡皮」开着时这些层才吃鼠标事件 —— 画图时指针必须能穿过它们。 */}
        {annLayers.map((l) => (
          <div
            key={`a${l.aid}`}
            data-ann={l.aid}
            className={`board-ann ${annSel === l.aid ? "on" : ""} ${
              annOpen && (annTool === "select" || annTool === "eraser") ? "hit" : ""
            }`}
            style={{ left: l.x, top: l.y, width: l.w, height: l.h }}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            <AnnLayerView layer={l} zoom={zoom} />
          </div>
        ))}

        {/* 正在拖的那一笔：一块临时画布，每帧只改它的 style（不走 React） */}
        <canvas ref={annLiveRef} className="board-ann board-ann-live" />

        {/* 文字工具的输入框：跟着画布变换走，所以字号/位置都是画布单位。
            **非受控**：每敲一个字就 setState 会让整块板（上千张图）重渲染。 */}
        {annTextAt && (
          <input
            ref={annTextRef}
            className="board-ann-input"
            autoFocus
            spellCheck={false}
            defaultValue=""
            placeholder={tr("board.ann.text_placeholder")}
            style={{
              left: annTextAt.x,
              top: annTextAt.y,
              fontSize: Math.max(ANN_FONT_MIN, Math.round(annFont)),
              color: annColor,
            }}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") {
                e.preventDefault();
                annCommitText(annTextAt, e.currentTarget.value);
              }
              if (e.key === "Escape") {
                e.preventDefault();
                setAnnTextAt(null);
              }
            }}
            onBlur={(e) => annCommitText(annTextAt, e.currentTarget.value)}
          />
        )}

        {/* 文字便签（0.4.17 起是"文本框"）：**框就是文字本身的尺寸** —— 不设 width/height，
            由内容撑开。所以没有"一个大框套着一行字"的观感，选中也只是文字底下淡淡一层。
            缩放 = 改字号（字变大 → 框自己变大），不做"拉框裁字"。 */}
        {notes.map((n) => {
          const b = pos.current.get(-n.nid) ?? { x: n.x, y: n.y, w: n.w, h: n.h };
          const editing = editId === n.nid;
          const fs = n.fs > 0 ? n.fs : NOTE_FS_DEF;
          return (
            <div
              key={`n${n.nid}`}
              data-note={n.nid}
              className={`board-note ${editing ? "editing" : ""} ${n.vert ? "vert" : ""} ${sel.has(-n.nid) ? "sel" : ""} ${grpMap.get(-n.nid) ? "in-group" : ""}`}
              style={{
                left: b.x,
                top: b.y,
                fontSize: fs,
                fontFamily: fontCss(n.font),
                color: n.color || undefined,
                fontWeight: n.bold ? 700 : 400,
              }}
              title={editing ? undefined : t("board.note_tip")}
              onDoubleClick={(e) => {
                /* 必须拦住：外层双击是「打开贴图 / fit」，会把便签一起带走 */
                e.stopPropagation();
                setEditId(n.nid);
              }}
            >
              {editing ? (
                <textarea
                  ref={taRef}
                  className="board-note-input"
                  defaultValue={n.text}
                  autoFocus
                  spellCheck={false}
                  /* wrap=off：**只有回车才换行**。默认的 textarea 会按容器宽度软折行，
                     双击进编辑时一行字突然变两行，看起来像"文字被改了"。 */
                  wrap="off"
                  placeholder={t("board.note_placeholder")}
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
                  {n.text || t("board.note_empty")}
                </div>
              )}
              {/* 缩放手柄：编辑时藏起来（用户只要一根光标，不要任何框/角标） */}
              {!editing && (
                <span className="board-handle" data-handle title={t("board.note_handle")} />
              )}
            </div>
          );
        })}

        {/* 组框（0.4.17）：多选 / 选中组时出现的虚线框 + 右下角手柄。
            框本身不吃鼠标事件（只有手柄吃），否则整片画布都点不动了。 */}
        {selBox && (
          <div
            className="board-selbox"
            style={{ left: selBox.x, top: selBox.y, width: selBox.w, height: selBox.h }}
          >
            <span className="board-selbox-handle" data-group-handle title={t("board.selbox_handle")} />
          </div>
        )}
      </div>

      {/* 框选矩形（0.4.23）：左键在空白处拖出来的那个框。
          用 ref 直接改 style（拖动中每帧 setState 会让整块画布重渲染）。 */}
      <div className="board-marquee" ref={marqRef} />

      {/* 左侧图层面板（0.4.15，0.4.24 起**图片与文字都在里面**）：
          列表按画布上的视觉顺序排（先上后下、同一行从左到右），点一行 = 选中并在视口居中，
          文字行双击直接改字。**不做 PS 那种"列表上方压下方"的层叠语义** ——
          层级只认"最近点了谁"。 */}
      {layersOpen && (
        <div className="board-layers" onMouseDown={(e) => e.stopPropagation()}>
          <div className="board-layers-head">
            <Layers size={13} strokeWidth={1.8} className="board-layers-icon" />
            <span className="board-layers-title">{t("board.layers")}</span>
            <span className="mono board-layers-count" title={t("board.layers_count_tip")}>
              {items.length}
              {notes.length ? `+${notes.length}` : ""}
              {annLayers.length ? `+${annLayers.length}` : ""}
            </span>
            <button title={t("board.layers_collapse")} onClick={() => setLayersOpen(false)}>
              <X size={12} strokeWidth={1.9} />
            </button>
          </div>
          <div className="board-layers-list">
            {/* 标注层单独一段、排在最上面（最上层在前，和画布上的压盖关系**顺序相反**：
                这是 PS 的惯例 —— 列表顶部 = 画布最上面）。上移/下移就是改 z。 */}
            {annRows.length > 0 && (
              <>
                <div className="board-layers-sep">{tr("board.ann.section")}</div>
                {annRows.map((l) => (
                  <div
                    key={`a${l.aid}`}
                    data-ann-row={l.aid}
                    className={`board-layer board-layer-ann ${annSel === l.aid ? "on" : ""}`}
                    title={`${annName.get(l.aid) ?? ""}\n${tr("board.ann.layer_tip")}`}
                    onClick={() => {
                      annSelRef.current = l.aid;
                      setAnnSel(l.aid);
                      focusBox({ x: l.x, y: l.y, w: l.w, h: l.h });
                    }}
                  >
                    <span className="board-layer-ph board-layer-annico">
                      {ANN_ICONS[l.kind] ?? ANN_ICONS.rect}
                    </span>
                    <span className="board-layer-name">{annName.get(l.aid)}</span>
                    <span className="board-layer-annops">
                      <button
                        title={tr("board.ann.menu_up")}
                        onClick={(e) => {
                          e.stopPropagation();
                          annMoveZ(l.aid, 1);
                        }}
                      >
                        <ChevronUp size={11} strokeWidth={2} />
                      </button>
                      <button
                        title={tr("board.ann.menu_down")}
                        onClick={(e) => {
                          e.stopPropagation();
                          annMoveZ(l.aid, -1);
                        }}
                      >
                        <ChevronDown size={11} strokeWidth={2} />
                      </button>
                      <button
                        title={tr("board.ann.menu_del")}
                        onClick={(e) => {
                          e.stopPropagation();
                          annRemove(l.aid);
                        }}
                      >
                        <Trash2 size={11} strokeWidth={1.9} />
                      </button>
                    </span>
                  </div>
                ))}
              </>
            )}
            {!layerRows.length && !annRows.length && (
              <div className="board-layers-empty">{t("board.layers_empty")}</div>
            )}
            {layerRows.map((r) => {
              if (r.img) {
                const it = r.img;
                return (
                  <div
                    key={it.id}
                    data-layer={it.id}
                    className={`board-layer ${sel.has(it.id) ? "on" : ""}`}
                    title={`${it.name}\n${t("board.layer_focus_tip")}`}
                    onClick={() => {
                      setTopId(it.id);
                      const n = new Set([it.id]);
                      selRef.current = n;
                      setSel(n);
                      focusAt(it.id);
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
                    {topId === it.id && <span className="board-layer-top">{t("board.layer_top")}</span>}
                  </div>
                );
              }
              const n = r.note!;
              const preview = n.text.replace(/\s+/g, " ").trim();
              return (
                <div
                  key={`n${n.nid}`}
                  data-layer-note={n.nid}
                  className={`board-layer board-layer-note ${sel.has(-n.nid) ? "on" : ""}`}
                  title={`${preview || t("board.note_text_empty")}\n${t("board.layer_note_tip")}`}
                  onClick={() => {
                    const s = new Set([-n.nid]);
                    selRef.current = s;
                    setSel(s);
                    focusAt(-n.nid);
                  }}
                  onDoubleClick={(e) => {
                    e.stopPropagation();
                    setEditId(n.nid);
                  }}
                >
                  <span className="board-layer-ph board-layer-t">T</span>
                  <span className="board-layer-name">{preview || t("board.note_text_empty")}</span>
                  <span className="board-layer-meta mono">
                    {Math.round(n.fs || NOTE_FS_DEF)}
                  </span>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* 空板：唯一的正经动作就是「粘一张进来」，所以直接把这句话写在画布中央。
          画过标注就不算空板了 —— 不然这句提示会一直压在刚画的东西上面。 */}
      {!items.length && !notes.length && !annLayers.length && (
        <div className="board-empty">
          <span className="board-empty-title">
            {boot.snapshot ? t("board.empty_snapshot") : t("board.empty_board")}
          </span>
          <span className="board-empty-hint">
            {boot.snapshot ? (
              <>
                {t("board.empty_snapshot_hint")}
                <br />
              </>
            ) : null}
            <kbd className="mono">Ctrl+V</kbd> {t("board.empty_paste_lead")}{" "}
            <span className="mono">screenshots</span>
            {boot.snapshot
              ? t("board.empty_paste_snap", { name: boot.snapshot.name })
              : boot.coll
                ? t("board.empty_paste_coll", { name: boot.coll.name })
                : t("board.empty_paste_tag", { tag: boot.tag ?? "" })}
            <br />
            {t("board.empty_note_lead")} <kbd className="mono">T</kbd>{" "}
            {t("board.empty_note_tail")}
          </span>
        </div>
      )}

      {hint && <div className={`board-hint ${hint.bad ? "bad" : ""}`}>{hint.text}</div>}

      {/* 文本框的字体工具条（0.4.17）：只在"恰好选中一张便签"时出现，
          固定在底部中央 —— 不跟着画布平移缩放，所以永远点得到。 */}
      {/* 工具条自己不吃画布手势；`preventDefault` 是为了**别把焦点从 textarea 抢走** ——
          编辑时点字号/竖排，光标要留在便签里继续打字（原生 button 会抢焦点 → blur → 退出编辑）。 */}
      {lookNote && (
        <div
          className="board-notebar"
          onMouseDown={(e) => {
            e.stopPropagation();
            e.preventDefault();
          }}
        >
          {/* 字体按钮：点开带搜索的弹层。本机装了上千款字体，原生 select 拉不动 */}
          <button
            className="board-notebar-fontbtn"
            title={`${t("board.font_btn_tip")}${extraFonts.length ? t("board.font_btn_tip_n", { n: extraFonts.length }) : ""}`}
            onClick={() => {
              setFontQ("");
              setFontPop((v) => !v);
            }}
          >
            <span className="board-notebar-fontname">
              {lookNote.font
                ? fontLabel(lookNote.font)
                : t("board.font_default")}
            </span>
            <ChevronDown size={11} strokeWidth={2} />
          </button>
          <span className="board-notebar-sep" />
          <button
            title={t("board.font_size_down")}
            onClick={() =>
              setNoteLook(lookNote.nid, {
                fs: Math.max(6, Math.round(((lookNote.fs || NOTE_FS_DEF) / 1.15) * 10) / 10),
              })
            }
          >
            <Minus size={12} strokeWidth={1.9} />
          </button>
          <span className="mono board-notebar-size">{Math.round(lookNote.fs || NOTE_FS_DEF)}</span>
          <button
            title={t("board.font_size_up")}
            onClick={() =>
              setNoteLook(lookNote.nid, {
                fs: Math.min(400, Math.round((lookNote.fs || NOTE_FS_DEF) * 1.15 * 10) / 10),
              })
            }
          >
            <Plus size={12} strokeWidth={1.9} />
          </button>
          <span className="board-notebar-sep" />
          <button
            className={lookNote.bold ? "on" : ""}
            title={t("board.bold")}
            onClick={() => setNoteLook(lookNote.nid, { bold: !lookNote.bold })}
          >
            <BoldIcon size={12} strokeWidth={2.1} />
          </button>
          <span className="board-notebar-sep" />
          {/* 横排 / 竖排（0.5.2）：竖排 = Photoshop「直排文字工具」，
              文字自上而下、回车往**左**开新列（CSS writing-mode: vertical-rl 就是这个语义）。
              点一下立即重排 + 立即落库（setNoteLook 走 noteRemember）。 */}
          <button
            className={`board-notebar-vert ${lookNote.vert ? "on" : ""}`}
            title={
              lookNote.vert
                ? t("board.vert_on_tip")
                : t("board.vert_off_tip")
            }
            onClick={() => setNoteLook(lookNote.nid, { vert: !lookNote.vert })}
          >
            <span>{lookNote.vert ? t("board.vert") : t("board.horiz")}</span>
          </button>
          <span className="board-notebar-sep" />
          {NOTE_COLORS.map(([v, label]) => (
            <button
              key={v || "def"}
              className={`board-notebar-color ${v ? "" : "def"} ${lookNote.color === v ? "on" : ""}`}
              style={v ? { background: v } : undefined}
              title={t("board.note_color_tip", { color: t(label) })}
              onClick={() => setNoteLook(lookNote.nid, { color: v })}
            />
          ))}
        </div>
      )}

      {/* 字体选择弹层：搜索 + 每行用自己的字体渲染自己名字（选之前就能看到长什么样）。
          本机一千多款字体全列出来会卡，先渲染 200 条，剩下的靠继续输入缩小范围。 */}
      {lookNote && fontPop && (
        <div className="board-fontpop" onMouseDown={(e) => e.stopPropagation()}>
          <input
            className="board-fontpop-q"
            autoFocus
            spellCheck={false}
            placeholder={t("board.font_search", { n: extraFonts.length + FONTS.length - 1 })}
            value={fontQ}
            onChange={(e) => setFontQ(e.target.value)}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Escape") {
                e.preventDefault();
                setFontPop(false);
              }
            }}
          />
          <div className="board-fontpop-list">
            {fontHits.common.map((o) => (
              <button
                key={o.v || "def"}
                className={`board-fontpop-row ${lookNote.font === o.v ? "on" : ""}`}
                style={{ fontFamily: fontCss(o.v) }}
                onClick={() => {
                  setNoteLook(lookNote.nid, { font: o.v });
                  setFontPop(false);
                }}
              >
                {o.label}
                {o.v && <span className="board-fontpop-tag mono">{o.v}</span>}
              </button>
            ))}
            {fontHits.all.length > 0 && <div className="board-fontpop-sep">{t("board.font_local")}</div>}
            {fontHits.all.slice(0, 200).map((o) => (
              <button
                key={o.v}
                className={`board-fontpop-row ${lookNote.font === o.v ? "on" : ""}`}
                style={{ fontFamily: fontCss(o.v) }}
                title={o.v}
                onClick={() => {
                  setNoteLook(lookNote.nid, { font: o.v });
                  setFontPop(false);
                }}
              >
                {o.label}
              </button>
            ))}
            {fontHits.all.length > 200 && (
              <div className="board-fontpop-more">
                {t("board.font_more", { n: fontHits.all.length - 200 })}
              </div>
            )}
            {!fontHits.common.length && !fontHits.all.length && (
              <div className="board-fontpop-more">{t("board.font_none")}</div>
            )}
            {strayFont && (
              <div className="board-fontpop-more">
                {t("board.font_stray", { font: strayFont })}
              </div>
            )}
          </div>
        </div>
      )}

      {/* ---------------- 标注工具条 ----------------
          截图那套工具，悬浮在板底居中（不跟画布平移缩放走，所以永远点得到）。
          样式和主界面同一套纸墨令牌；分组顺序照 owner 给的清单：
          选择 / 矩形 / 椭圆 / 直线 / 箭头 / 画笔 / 荧光笔 / 文字 / 序号 / 马赛克 / 橡皮
          + 颜色 + 粗细 + 撤销 / 重做 / 完成。 */}
      {annOpen && (
        <div
          className="board-anntools"
          role="toolbar"
          aria-label={tr("board.ann.toolbar")}
          /* 便签的字体工具条也在底部中央：两条同时出现时往上让一格，别叠在一起 */
          style={{ bottom: lookNote ? 58 : 16 }}
          /* 工具条自己吃掉按下与滚轮：既不能把画布拖走，也不能把画布缩放掉 */
          onMouseDown={(e) => {
            e.stopPropagation();
            e.preventDefault();
          }}
        >
          {ANN_TOOLS.map((tool) => (
            <button
              key={tool}
              className={`board-anntool ${annTool === tool ? "on" : ""}`}
              title={tr(annToolKey(tool))}
              aria-pressed={annTool === tool}
              onClick={() => {
                setAnnTool(tool);
                setAnnPalette(false);
              }}
            >
              {ANN_ICONS[tool]}
            </button>
          ))}

          <span className="board-anntools-sep" />

          <div className="board-ann-anchor">
            <button
              className={`board-anntool board-ann-colorbtn ${annPalette ? "on" : ""}`}
              title={tr("board.ann.color")}
              onClick={() => setAnnPalette((v) => !v)}
            >
              <i className="board-ann-swatch" style={{ background: annColor }} />
            </button>
            {annPalette && (
              <div className="board-ann-palette" onMouseDown={(e) => e.stopPropagation()}>
                <div className="board-ann-palette-title">{tr("board.ann.color")}</div>
                <div className="board-ann-swatches">
                  {ANN_COLORS.map((c) => (
                    <button
                      key={c}
                      className={`board-ann-sw ${
                        c.toUpperCase() === annColor.toUpperCase() ? "on" : ""
                      }`}
                      style={{ background: c }}
                      title={c}
                      onClick={() => {
                        setAnnColor(c);
                        setAnnPalette(false);
                      }}
                    />
                  ))}
                </div>
              </div>
            )}
          </div>

          {/* 粗细（文字 / 序号工具下是字号）*/}
          <div className="board-ann-weight" ref={weightRef} title={tr("board.ann.weight")}>
            <button className="board-ann-step" onClick={() => annStepW(-1, false)}>
              <Minus size={11} strokeWidth={2} />
            </button>
            <span className="mono board-ann-weightv">
              {annTexty ? Math.round(annValue) : annValue.toFixed(1)}
            </span>
            <button className="board-ann-step" onClick={() => annStepW(1, false)}>
              <Plus size={11} strokeWidth={2} />
            </button>
          </div>

          {/* 填充只对矩形 / 椭圆有意义：和截图一样按当前工具出现，不占常驻位置 */}
          {(annTool === "rect" || annTool === "ellipse") && (
            <button
              className={`board-anntool ${annFill ? "on" : ""}`}
              title={tr("board.ann.fill")}
              aria-pressed={annFill}
              onClick={() => setAnnFill((v) => !v)}
            >
              {ANN_ICONS.fill}
            </button>
          )}

          <span className="board-anntools-sep" />

          <button
            className="board-anntool"
            disabled={!annCanUndo}
            title={tr("board.ann.undo")}
            onClick={annUndoStep}
          >
            {ANN_ICONS.undo}
          </button>
          <button
            className="board-anntool"
            disabled={!annCanRedo}
            title={tr("board.ann.redo")}
            onClick={annRedoStep}
          >
            {ANN_ICONS.redo}
          </button>

          <span className="board-anntools-sep" />

          <button
            className="board-anntool board-ann-done"
            title={tr("board.ann.done")}
            onClick={() => {
              setAnnOpen(false);
              setAnnPalette(false);
            }}
          >
            {ANN_ICONS.done}
            <span className="board-ann-donetext">{tr("board.ann.done")}</span>
          </button>
        </div>
      )}

      {/* 左下角缩放条：固定在这一角（画布怎么平移缩放都不动它）。
          图层面板展开时往右让出它的宽度，否则会浮在列表上面。 */}
      <div
        className="board-zoombar"
        style={{ left: layersOpen ? LAYERS_W + 14 : 14 }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <button title={t("board.zoom_out")} onClick={() => zoomBy(1 / 1.2)}>
          <Minus size={12} strokeWidth={1.9} />
        </button>
        <span className="mono board-zoom" onClick={reset100} title={t("board.zoom_reset_tip")}>
          {Math.round(zoom * 100)}%
        </span>
        <button title={t("board.zoom_in")} onClick={() => zoomBy(1.2)}>
          <Plus size={12} strokeWidth={1.9} />
        </button>
        <button title={t("board.fit_tip")} onClick={fit}>
          <Maximize2 size={12} strokeWidth={1.9} />
        </button>
      </div>

      {/* 顶栏：整条是拖窗口的把手（画布本身也要能拖，所以只在顶部让出这一条） */}
      <div className="board-top" data-tauri-drag-region>
        <div className="board-title" data-tauri-drag-region>
          <span className="board-name">{boot.title}</span>
          <span className="board-count mono">
            {items.length}
            {boot.more ? `/${boot.total}` : ""}
            {t("board.unit_image")}
            {!!boot.missing && t("board.missing_count", { n: boot.missing })}
          </span>
        </div>
        <div className="board-actions">
          <button
            title={t("board.paste_tip")}
            onClick={() => void paste()}
          >
            <ClipboardPaste size={13} strokeWidth={1.8} />
          </button>
          <button title={t("board.add_note_tip")} onClick={() => void addNote()}>
            <Type size={13} strokeWidth={1.8} />
          </button>
          <button title={t("board.tidy_tip")} onClick={tidy}>
            <LayoutGrid size={13} strokeWidth={1.8} />
          </button>
          <button
            title={t("board.layers_tip")}
            className={layersOpen ? "on" : ""}
            onClick={() => setLayersOpen((v) => !v)}
          >
            <Layers size={13} strokeWidth={1.8} />
          </button>
          <span className="board-sep" />
          <button title={t("board.zoom_out")} onClick={() => zoomBy(1 / 1.2)}>
            <Minus size={13} strokeWidth={1.8} />
          </button>
          <span className="mono board-zoom">{Math.round(zoom * 100)}%</span>
          <button title={t("board.zoom_in")} onClick={() => zoomBy(1.2)}>
            <Plus size={13} strokeWidth={1.8} />
          </button>
          <button title={t("board.fit_tip")} onClick={fit}>
            <Maximize2 size={13} strokeWidth={1.8} />
          </button>
          <span className="board-sep" />
          <button
            title={
              boardTop
                ? t("board.top_on_tip")
                : t("board.top_off_tip")
            }
            className={boardTop ? "on" : ""}
            onClick={toggleTop}
          >
            {boardTop ? (
              <Pin size={13} strokeWidth={1.8} />
            ) : (
              <PinOff size={13} strokeWidth={1.8} />
            )}
          </button>
          {/* 收起（不是关）：hide() 到托盘，画布状态全留着；再从主窗点同一个标签就把这块板原样拉回来 */}
          <button title={t("board.min_tip")} onClick={() => void source.boardHide()}>
            <Minimize2 size={13} strokeWidth={1.8} />
          </button>
          <button title={t("board.close_tip")} onClick={() => void source.boardClose()}>
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

/**
 * 一层标注的显示体。
 *
 * 位图倍率**跟当前缩放走**：放大之后按新倍率重画一遍，线框和文字才保持锐利；
 * 不重画的话就是一张被 CSS 拉大的旧图，放大到 200% 就糊了。
 * 重画的触发点是 `zoom`（画布缩放状态，wheel 里已经 rAF 节流过），
 * 所以滚轮缩放时最多每帧重画一次，代价是"当前可见的标注层数"级别，不是整块板。
 *
 * 马赛克例外：它不是画出来的，是盖在底图上的一层 `backdrop-filter`。
 * 选它而不是"把底图抓成像素再打块"，理由见 `.board-ann-mosaic` 的样式注释。
 */
function AnnLayerView({ layer, zoom }: { layer: AnnLayer; zoom: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const mosaic = layer.op.t === "mosaic";

  useEffect(() => {
    const cv = ref.current;
    if (!cv || layer.op.t === "mosaic") return;
    const s = annBmpScale(layer.w, layer.h, zoom);
    const bw = Math.max(1, Math.round(layer.w * s));
    const bh = Math.max(1, Math.round(layer.h * s));
    if (cv.width !== bw) cv.width = bw;
    if (cv.height !== bh) cv.height = bh;
    const ctx = cv.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.scale(s, s);
    annDraw(ctx, layer.op);
  }, [layer, zoom]);

  if (layer.op.t === "mosaic") {
    /* 像素块靠 CSS 的网格叠色暗示；不用真像素化的原因见 `.board-ann-mosaic` 的注释
       （底图是视口懒挂载的 DOM <img>，重合成一遍既不可靠也会随图移动而过期） */
    return (
      <div
        className="board-ann-mosaic"
        style={{
          backdropFilter: `blur(${layer.op.blur}px)`,
          WebkitBackdropFilter: `blur(${layer.op.blur}px)`,
        }}
      />
    );
  }
  return <canvas className="board-ann-cv" ref={ref} />;
}