import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { onAppEvent, source } from "../data/source";
import { loadPrefs } from "../types";

/**
 * 截图覆盖层（F1）—— 0.5.0 起换成「分组工具条 v2」（同事的方案 + 原型，
 * `2026-11-15-09-14-49/screenshot-toolbar-v2/`）。
 *
 * 和 0.4.x 的 Snipaste 逐项复刻版相比，**交互形状变了**，但交付纪律一条没动：
 * - 框完**不自动交付**；复制 / 保存 / 贴图都必须显式按键；
 * - `Enter` 仍然**什么都不交付**（0.1.20 定案：否则用户框完顺手 Enter 就进不到标注）；
 * - `Ctrl+C` / `Ctrl+S` 只认真人按键（注入的由 Rust 侧静默丢弃）；
 * - 坐标两套：屏幕一律 CSS 像素，画布 / 标注一律物理像素（CSS × scale）。
 *
 * v2 改动（对齐方案 §3 / §6 / §8 P0）：
 * 1. 13 个平级图标 → 6 组折叠（形状 / 画笔 / 文字 / 遮挡 / 素材）+ 选择常驻，
 *    悬停 110ms 展开托盘、移开 190ms 收起，同一时刻只开一个；
 * 2. 属性组：颜色胶囊（21 色 7×3 + 吸管 + 自定义）、线条粗细**滚轮连续调**
 *    （0.5–24px，Shift 2px / Alt 0.1px，竖向拖拽、双击复位、读数浮层）、20% 填充；
 * 3. 撤销 / 重做改成**整份标注快照栈**（24 步），橡皮擦与删除也能回退
 *    （旧的「弹出最后一条」对删除无效，是这一版顺手修掉的老问题）；
 * 4. 输出改为分裂按钮：`复制` 是唯一实心主按钮，保存 / 另存 / 贴图进下拉；
 * 5. 选区 HUD：宽高 + 屏幕坐标 + 缩放比（等宽数字，拖动时不抖）；
 * 6. 新增工具：序号标记 N、引线气泡 K、高斯模糊 B、色块 G、勾 / 叉图章 X / C。
 *
 * 已知边界（方案里也标了"占位"的项，这里**没有**假装做完）：
 * - 贴纸表情 S、插入图片 I：入口在，点了只提示"下一版"，不产生任何像素；
 * - OCR / 上传分享：不在这版范围；下拉里 OCR 置灰。
 */
interface SnipBoot {
  src: string;
  thumb?: string | null;
  avg?: [number, number, number] | null;
  scale: number;
  x: number;
  y: number;
  w: number;
  h: number;
  /** 主窗当前主题（设计稿 v1.3：截图界面跟随软件主题） */
  theme?: string;
}

function bootData(): SnipBoot | null {
  const b = (window as unknown as { __AM_SNIP__?: SnipBoot }).__AM_SNIP__;
  if (!b?.src) return null;
  return { ...b, scale: b.scale > 0 ? b.scale : 1 };
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}
interface P {
  x: number;
  y: number;
}

type Tool =
  | "select"
  | "rect"
  | "ellipse"
  | "line"
  | "arrow"
  | "pen"
  | "marker"
  | "text"
  | "badge"
  | "callout"
  | "mosaic"
  | "blur"
  | "block"
  | "check"
  | "cross"
  | "eraser";

/** 托盘里带占位项的 id（还进不了 Tool） */
type ToolId = Tool | "sticker" | "image";

type Action = "copy" | "save" | "saveas" | "peek";

/** 一条标注。坐标全是**物理像素**，相对整块虚拟桌面（不相对选区）——
 *  这样改选区大小时标注不会跟着飘。 */
type Op =
  | { t: "rect" | "ellipse"; a: P; b: P; color: string; w: number; fill: boolean }
  | { t: "line"; a: P; b: P; color: string; w: number }
  | { t: "arrow"; a: P; b: P; color: string; w: number; solid: boolean }
  | { t: "pen" | "marker"; pts: P[]; color: string; w: number; alpha: number }
  | { t: "text"; p: P; s: string; color: string; size: number }
  | { t: "badge"; p: P; n: number; color: string; r: number }
  | { t: "callout"; p: P; s: string; color: string; size: number }
  | { t: "mosaic"; a: P; b: P }
  | { t: "blur"; a: P; b: P; r: number }
  | { t: "block"; a: P; b: P; color: string }
  | { t: "check" | "cross"; p: P; color: string; r: number };

const isFreehand = (op: Op): op is Extract<Op, { t: "pen" | "marker" }> =>
  op.t === "pen" || op.t === "marker";
/** 拖拽画出来的框类（有 a/b） */
type BoxOp = Extract<Op, { a: P; b: P }>;
const isBoxOp = (op: Op): op is BoxOp => "a" in op && "b" in op;

const DIM = 0.55;
const LOUPE = { w: 184, h: 136, zoom: 8 };
const BAR_H = 56;
const WIDTH_MIN = 0.5;
const WIDTH_MAX = 24;
const FONT_MIN = 8;
const FONT_MAX = 72;
const HISTORY_MAX = 24;

/** 21 色 7×3：分行语义 = 中性（白→黑）/ 暖色警示 / 冷色与强调。
 *  强调这一列用软件自己的强调色（深色 #A78BFA / 浅色 #6C3FD1），画出来和界面同源。 */
const COLORS = [
  "#FFFFFF",
  "#F5F7FA",
  "#C9CFDA",
  "#8A93A5",
  "#4B5464",
  "#232936",
  "#000000",
  "#FF4D4F",
  "#FF7A45",
  "#FFB020",
  "#FFE14D",
  "#A0E64B",
  "#34D399",
  "#22D3EE",
  "#A78BFA",
  "#6C3FD1",
  "#7C6BFF",
  "#C084FC",
  "#F472B6",
  "#FB7185",
  "#14B8A6",
];

/* ---------------- 图标（24×24 网格，stroke 1.6，渲染 20） ---------------- */

const ico = (children: ReactNode) => (
  <svg
    viewBox="0 0 24 24"
    width="20"
    height="20"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.6"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden
  >
    {children}
  </svg>
);

const ICONS: Record<string, ReactNode> = {
  select: ico(<path d="M5 3v16.6l4.4-4.2 2.6 5 3-1.6-2.7-4.9 6-.5z" />),
  rect: ico(<rect x="4.2" y="5" width="15.6" height="14" rx="2.2" />),
  ellipse: ico(<ellipse cx="12" cy="12" rx="8" ry="6.6" />),
  line: ico(<path d="M5 19 19 5" />),
  arrow: ico(
    <>
      <path d="M4.5 19.5 19 5" />
      <path d="M12.6 5H19v6.4" />
    </>
  ),
  pen: ico(
    <>
      <path d="M4 20l4-.9L19.3 7.8a2.4 2.4 0 0 0-3.4-3.4L4.6 15.7z" />
      <path d="M14.4 6.6l3 3" />
    </>
  ),
  marker: ico(
    <>
      <path d="M7.6 16.4 15.1 8.9a2.35 2.35 0 0 1 3.3 3.3l-7.5 7.5-4.5 1.1z" />
      <path d="M13.2 10.8l3.3 3.3" />
    </>
  ),
  text: ico(
    <>
      <path d="M5 6.4V5h14v1.4" />
      <path d="M12 5v14" />
      <path d="M8.6 19h6.8" />
    </>
  ),
  badge: ico(
    <>
      <circle cx="12" cy="12" r="8" />
      <path d="M11.1 9.5 12.8 8.2V16" />
      <path d="M9.8 16h5.6" />
    </>
  ),
  callout: ico(<path d="M5 5.5h14v10h-9.2L6.8 19v-3.5H5z" />),
  mosaic: ico(
    <path
      d="M4 4h5v5H4zM13 4h7v5h-7zM4 11h5v5H4zM13 11h7v5h-7zM4 18h5v2H4zM13 18h7v2h-7z"
      fill="currentColor"
      stroke="none"
      opacity=".85"
    />
  ),
  blur: ico(
    <>
      <circle cx="12" cy="12" r="7.6" strokeDasharray="1.6 3.2" />
      <circle cx="12" cy="12" r="3.4" opacity=".55" />
    </>
  ),
  block: ico(<rect x="4.5" y="6" width="15" height="12" rx="2" fill="currentColor" stroke="none" opacity=".85" />),
  check: ico(
    <>
      <circle cx="12" cy="12" r="8" />
      <path d="M8.4 12.2l2.5 2.5 4.7-5" />
    </>
  ),
  cross: ico(
    <>
      <circle cx="12" cy="12" r="8" />
      <path d="M9.2 9.2l5.6 5.6M14.8 9.2l-5.6 5.6" />
    </>
  ),
  sticker: ico(
    <>
      <circle cx="12" cy="12" r="8" />
      <path d="M9.2 13.6c.6 1 1.7 1.6 2.8 1.6s2.2-.6 2.8-1.6" />
      <circle cx="9.4" cy="9.9" r=".95" fill="currentColor" stroke="none" />
      <circle cx="14.6" cy="9.9" r=".95" fill="currentColor" stroke="none" />
    </>
  ),
  image: ico(
    <>
      <rect x="4" y="5" width="16" height="14" rx="2" />
      <circle cx="9" cy="10" r="1.3" />
      <path d="M5 17.2l4.5-4.5 3 3 2.8-2.8 3.7 3.7" />
    </>
  ),
  fill: ico(
    <>
      <rect x="4.4" y="4.4" width="15.2" height="15.2" rx="2" />
      <path d="M4.4 12h15.2v5.6a2 2 0 0 1-2 2H6.4a2 2 0 0 1-2-2z" fill="currentColor" stroke="none" opacity=".5" />
    </>
  ),
  eraser: ico(
    <>
      <path d="M7.4 17.6 4.8 15a1.8 1.8 0 0 1 0-2.6l7.2-7.2a1.8 1.8 0 0 1 2.6 0l3.4 3.4a1.8 1.8 0 0 1 0 2.6l-6.6 6.6z" />
      <path d="M10.2 8.6l6 6" />
    </>
  ),
  undo: ico(
    <>
      <path d="M4.5 9.5h9a4.3 4.3 0 0 1 0 8.6H8.4" />
      <path d="M8 5.6 4.2 9.5 8 13.4" />
    </>
  ),
  redo: ico(
    <>
      <path d="M19.5 9.5h-9a4.3 4.3 0 0 0 0 8.6h5.1" />
      <path d="M16 5.6l3.8 3.9L16 13.4" />
    </>
  ),
  copy: ico(
    <>
      <rect x="8.2" y="8.2" width="11" height="11" rx="2" />
      <path d="M15.6 5H6.6A1.6 1.6 0 0 0 5 6.6v9" />
    </>
  ),
  save: ico(
    <>
      <path d="M5 5h9.4L19 9.6V19H5z" />
      <path d="M8.4 5v4.6h6.8V5" />
      <rect x="8.4" y="13" width="7.2" height="6" />
    </>
  ),
  pin: ico(
    <>
      <path d="M9.4 4h5.2l-1 4.8 3.4 3H6L9.4 8.8z" />
      <path d="M12 11.8V20" />
    </>
  ),
  ocr: ico(
    <>
      <path d="M4.6 8.4V5.6h2.8M16.6 5.6h2.8v2.8M19.4 15.6v2.8h-2.8M7.4 18.4H4.6v-2.8" />
      <path d="M8.6 10.6h6.8M8.6 13.6h4.4" />
    </>
  ),
  close: ico(<path d="M6.4 6.4l11.2 11.2M17.6 6.4L6.4 17.6" />),
  chevron: ico(<path d="M6 9.5l6 6 6-6" />),
  width: ico(<path d="M4 6.2h16M4 12h16M4 17.8h16" />),
  cursor: ico(
    <>
      <path d="M4 12h16" />
      <path d="M8 8v8M16 8v8" />
    </>
  ),
};

/** 线条粗细图标：三条等粗线随当前值变粗（1.3→3.9，24px 也不糊成一块） */
function WidthGlyph({ w }: { w: number }) {
  const s = Math.min(3.9, Math.max(1.3, 1.1 + w * 0.26)).toFixed(2);
  return (
    <svg
      viewBox="0 0 24 24"
      width="20"
      height="20"
      fill="none"
      stroke="currentColor"
      strokeWidth={s}
      strokeLinecap="round"
      aria-hidden
    >
      <path d="M4 6.2h16M4 12h16M4 17.8h16" />
    </svg>
  );
}

/* ---------------- 工具条配置：加工具只改这里 ---------------- */

interface ToolDef {
  id: ToolId;
  name: string;
  key: string;
  icon: string;
  off?: boolean;
}
interface GroupDef {
  id: string;
  name: string;
  face: Tool;
  items: ToolDef[];
}

const GROUPS: GroupDef[] = [
  {
    id: "shape",
    name: "形状",
    face: "rect",
    items: [
      { id: "rect", name: "矩形", key: "R", icon: "rect" },
      { id: "ellipse", name: "圆形 / 椭圆", key: "E", icon: "ellipse" },
      { id: "line", name: "直线", key: "L", icon: "line" },
      { id: "arrow", name: "箭头", key: "A", icon: "arrow" },
    ],
  },
  {
    id: "brush",
    name: "画笔",
    face: "pen",
    items: [
      { id: "pen", name: "自由画笔", key: "P", icon: "pen" },
      { id: "marker", name: "荧光笔", key: "H", icon: "marker" },
    ],
  },
  {
    id: "text",
    name: "文字标注",
    face: "text",
    items: [
      { id: "text", name: "文字", key: "T", icon: "text" },
      { id: "badge", name: "序号标记", key: "N", icon: "badge" },
      { id: "callout", name: "引线气泡", key: "K", icon: "callout" },
    ],
  },
  {
    id: "mask",
    name: "遮挡 / 打码",
    face: "mosaic",
    items: [
      { id: "mosaic", name: "马赛克", key: "M", icon: "mosaic" },
      { id: "blur", name: "高斯模糊", key: "B", icon: "blur" },
      { id: "block", name: "色块遮挡", key: "G", icon: "block" },
    ],
  },
  {
    id: "sticker",
    name: "素材",
    face: "check",
    items: [
      { id: "sticker", name: "贴纸表情（下一版）", key: "S", icon: "sticker", off: true },
      { id: "check", name: "勾选图章", key: "X", icon: "check" },
      { id: "cross", name: "叉号图章", key: "C", icon: "cross" },
      { id: "image", name: "插入图片（下一版）", key: "I", icon: "image", off: true },
    ],
  },
];

const DEFAULT_FACE: Record<string, Tool> = {
  shape: "rect",
  brush: "pen",
  text: "text",
  mask: "mosaic",
  sticker: "check",
};

const groupOf = (t: Tool | null) =>
  t ? GROUPS.find((g) => g.items.some((i) => i.id === t)) ?? null : null;

function resolveTheme(): "dark" | "light" {
  const p = loadPrefs().theme;
  if (p === "light" || p === "dark") return p;
  try {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  } catch {
    return "dark";
  }
}

/* ---------------- 入口 ---------------- */

export function Snip() {
  const boot = bootData();
  const [scale, setScale] = useState(boot?.scale ?? 1);
  const W = boot ? boot.w / scale : window.innerWidth;
  const H = boot ? boot.h / scale : window.innerHeight;

  const [rect, setRect] = useState<Rect | null>(null);
  const [hover, setHover] = useState<Rect | null>(null);
  const [tool, setTool] = useState<Tool | null>(null);
  const [faces, setFaces] = useState<Record<string, Tool>>(DEFAULT_FACE);
  const [color, setColor] = useState(COLORS[7]);
  const [width, setWidth] = useState(2);
  const [fontSize, setFontSize] = useState(17);
  const [fill, setFill] = useState(false);
  const [arrowSolid, setArrowSolid] = useState(true);
  const [badge, setBadge] = useState(1);
  const [tbTheme] = useState<"dark" | "light">(() => {
    /* 开窗 boot 里带了主窗当前主题（0.4.26 起）：优先用它，localStorage 的 prefs
       只是兜底（旧包没有 theme 字段时走老路） */
    const bt = boot?.theme;
    if (bt === "light" || bt === "dark") return bt;
    return resolveTheme();
  });

  /* 截图窗是**独立窗口**，App.tsx 那套 `html[data-theme]` 不会自动带过来。
     这里按软件的主题偏好补上，工具条 / HUD / 提示的配色就能全部走 tokens.css
     的深浅两套令牌（`--ui-menu` / `--text-*` / `--ui-accent`…），不再另起一套蓝。 */
  useEffect(() => {
    document.documentElement.dataset.theme = tbTheme;
  }, [tbTheme]);

  const [openGroup, setOpenGroup] = useState<string | null>(null);
  const [trayShift, setTrayShift] = useState(0);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [dropper, setDropper] = useState(false);
  const [widthHud, setWidthHud] = useState(false);

  /** 选择工具选中的那条标注（下标；ops 一变就清掉） */
  const [selOp, setSelOp] = useState<number | null>(null);
  /** 覆盖层底部那句一次性提示 */
  const [hint, setHint] = useState("");
  const hintTimer = useRef(0);
  const say = useCallback((t: string) => {
    setHint(t);
    window.clearTimeout(hintTimer.current);
    hintTimer.current = window.setTimeout(() => setHint(""), 2600);
  }, []);

  /** 选区最后一次变动的时刻：**刚框完/刚拖完 350ms 内不接交付**（防手势尾部误触） */
  const rectBorn = useRef(0);
  useEffect(() => {
    if (rect) rectBorn.current = Date.now();
  }, [rect]);

  const [ops, setOps] = useState<Op[]>([]);
  const [hist, setHist] = useState(0);
  const undoStack = useRef<Op[][]>([]);
  const redoStack = useRef<Op[][]>([]);

  const [cursor, setCursor] = useState<P>({ x: -999, y: -999 });
  const [rgb, setRgb] = useState<[number, number, number] | null>(null);
  const [hexMode, setHexMode] = useState(false);
  const [textAt, setTextAt] = useState<P | null>(null);
  const [textKind, setTextKind] = useState<"text" | "callout">("text");
  const [text, setText] = useState("");
  const textRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [tip, setTip] = useState("");
  const [space, setSpace] = useState(false);
  const [hiSrc, setHiSrc] = useState<string | null>(null);
  const [barW, setBarW] = useState(600);
  const [escArm, setEscArm] = useState(false);

  const rectRef = useRef<Rect | null>(null);
  rectRef.current = rect;
  const opsRef = useRef<Op[]>([]);
  opsRef.current = ops;
  const colorRef = useRef(color);
  colorRef.current = color;
  const widthRef = useRef(width);
  widthRef.current = width;
  const fontRef = useRef(fontSize);
  fontRef.current = fontSize;
  const toolRef = useRef<Tool | null>(null);
  toolRef.current = tool;
  const fillRef = useRef(fill);
  fillRef.current = fill;
  const solidRef = useRef(arrowSolid);
  solidRef.current = arrowSolid;
  const badgeRef = useRef(badge);
  badgeRef.current = badge;
  const scaleRef = useRef(scale);
  scaleRef.current = scale;
  const spaceRef = useRef(false);
  spaceRef.current = space;

  /* 画布 / 像素源 */
  const ink = useRef<HTMLCanvasElement>(null);
  const base = useRef<HTMLCanvasElement | null>(null);
  const [baseReady, setBaseReady] = useState(false);
  const loupeCv = useRef<HTMLCanvasElement>(null);
  const raf = useRef(0);
  const live = useRef<{ op: Op } | null>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const widthBtn = useRef<HTMLButtonElement>(null);

  /* ---------------- 历史（整份快照栈，24 步） ----------------
     旧实现是「弹出 ops 的最后一条」，所以橡皮擦 / Delete 这类"删除"根本回不去。
     标注是向量对象、不可变，直接存数组引用就够，比 getImageData 快且不占内存。 */
  const bumpHist = () => setHist((v) => v + 1);

  const recordHistory = useCallback((prev: Op[]) => {
    undoStack.current.push(prev);
    if (undoStack.current.length > HISTORY_MAX) undoStack.current.shift();
    redoStack.current = [];
    bumpHist();
  }, []);

  const commit = useCallback(
    (next: Op[]) => {
      recordHistory(opsRef.current);
      opsRef.current = next;
      setOps(next);
    },
    [recordHistory]
  );

  const undo = useCallback(() => {
    const prev = undoStack.current.pop();
    if (!prev) return;
    redoStack.current.push(opsRef.current);
    opsRef.current = prev;
    setOps(prev);
    let m = 0;
    for (const o of prev) if (o.t === "badge") m = Math.max(m, o.n);
    setBadge(m + 1);
    bumpHist();
  }, []);

  const redo = useCallback(() => {
    const next = redoStack.current.pop();
    if (!next) return;
    undoStack.current.push(opsRef.current);
    opsRef.current = next;
    setOps(next);
    let m = 0;
    for (const o of next) if (o.t === "badge") m = Math.max(m, o.n);
    setBadge(m + 1);
    bumpHist();
  }, []);

  /* ---------------- 窗口显形（治"开始黑一下"） ---------------- */

  /* **缩放比要以覆盖窗自己的实际尺寸为准**，不能信 boot 里那个数。
     `boot.scale` 是主窗口的缩放比；覆盖窗是按虚拟桌面尺寸建的，落在哪块屏上、
     那块屏的 DPI 是多少都可能和主窗不一样（多屏不同缩放时必错）。
     一旦这个数偏了，所有标注都会整体偏移（owner 报的"用工具箱有时会偏移"）。
     这里用"物理宽 / 实际 CSS 宽"反算，等于现场标定，任何 DPI 组合都对得上。 */
  useEffect(() => {
    if (!boot?.w) return;
    const bw = boot.w;
    const measure = () => {
      const cw = document.documentElement.clientWidth || window.innerWidth || 0;
      if (!cw) return;
      const k = bw / cw;
      if (k > 0 && Math.abs(k - scale) > 0.002) setScale(k);
    };
    measure();
    const t = window.setTimeout(measure, 240);
    window.addEventListener("resize", measure);
    return () => {
      window.clearTimeout(t);
      window.removeEventListener("resize", measure);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [boot?.w, scale]);

  /* 原生层把已经框好的选区交过来（0.2.4「C 方案」）：
     用户在原生层上就已经拉完框了，网页接手时不能让他重新框一次 —— 直接把这块
     区域设成选区。原生层报的是**物理像素**，这里换成 CSS 像素。 */
  useEffect(() => {
    return onAppEvent<{ x: number; y: number; w: number; h: number }>("snip:sel", (r) => {
      const k = scale > 0 ? scale : 1;
      if (!r || !(r.w > 0) || !(r.h > 0)) return;
      setRect({ x: r.x / k, y: r.y / k, w: r.w / k, h: r.h / k });
    });
  }, [scale]);

  const readyOnce = useRef(false);
  const callReady = useCallback(() => {
    if (readyOnce.current) return;
    readyOnce.current = true;
    requestAnimationFrame(() => requestAnimationFrame(() => void source.snipReady().catch(() => {})));
  }, []);

  /* **低清底图解码完就算"能用"**（0.1.29）。
     ready 只挂在 `<img src=高清>` 的 onLoad 上的话，全屏 PNG 解码 + 上传纹理要 300~400ms，
     用户按下 F1 后要等一秒多才能框选。thumb 是 480 宽的 data URI，几十毫秒就能画完。 */
  useEffect(() => {
    if (!boot?.thumb) return;
    let alive = true;
    const im = new Image();
    const done = () => {
      if (alive) callReady();
    };
    im.onload = done;
    im.onerror = done;
    im.src = boot.thumb;
    if (im.complete) done();
    return () => {
      alive = false;
    };
  }, [boot?.thumb, callReady]);

  useEffect(() => {
    document.body.classList.add("snip-body");
    document.documentElement.classList.add("snip-body");
    const t = window.setTimeout(callReady, 1200);
    return () => {
      window.clearTimeout(t);
      document.body.classList.remove("snip-body");
      document.documentElement.classList.remove("snip-body");
    };
  }, [callReady]);

  /* 高清底图**立刻**开始加载（0.1.29）：thumb 已经在背景上画着，并行加载不会让首帧变差。 */
  useEffect(() => {
    if (!boot) return;
    setHiSrc(boot.src);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [boot?.src]);

  /* 像素源：同源 data URL → 能 getImageData / toDataURL（放大镜、马赛克、导出都要）。
     **必须晚 400ms 再要**（0.2.3）：整屏 PNG 2MB 级，解码 + 建 canvas 会占主线程 300ms，
     挂在 mount 上就把首帧挤掉了。 */
  useEffect(() => {
    let alive = true;
    const t = window.setTimeout(() => {
      void source
        .snipFull()
        .then(async (url) => {
          if (!alive) return;
          const img = new Image();
          img.src = url;
          await img.decode().catch(() => {});
          if (!alive || !img.width) return;
          const c = document.createElement("canvas");
          c.width = img.width;
          c.height = img.height;
          c.getContext("2d")?.drawImage(img, 0, 0);
          base.current = c;
          setBaseReady(true);
          paint();
        })
        .catch(() => {});
    }, 400);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* ---------------- 绘制 ---------------- */

  const paint = useCallback(() => {
    const c = ink.current;
    if (!c) return;
    const ctx = c.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, c.width, c.height);
    const r = rectRef.current;
    if (!r) return;
    ctx.save();
    ctx.beginPath();
    ctx.rect(r.x * scale, r.y * scale, r.w * scale, r.h * scale);
    ctx.clip();
    for (const op of opsRef.current) drawOp(ctx, op, base.current);
    if (live.current) drawOp(ctx, live.current.op, base.current);
    ctx.restore();
  }, [scale]);

  /* 选中是按**下标**记的：ops 一变（撤销 / 删除 / 重画）下标就可能指到别的标注上 */
  useEffect(() => {
    setSelOp(null);
  }, [ops.length]);

  const schedule = useCallback(() => {
    if (raf.current) return;
    raf.current = requestAnimationFrame(() => {
      raf.current = 0;
      paint();
    });
  }, [paint]);

  useEffect(() => {
    const c = ink.current;
    if (!c || !boot) return;
    if (c.width !== boot.w || c.height !== boot.h) {
      c.width = boot.w;
      c.height = boot.h;
    }
    schedule();
  }, [boot?.w, boot?.h, rect, ops, schedule]);

  /* 放大镜重绘：**按物理像素画**（canvas 后备尺寸 × dpr），不然 8 倍放大会被二次重采样糊掉 */
  useEffect(() => {
    const c = loupeCv.current;
    const src = base.current;
    if (!c || !src) return;
    const dpr = window.devicePixelRatio || 1;
    const bw = Math.round(LOUPE.w * dpr);
    const bh = Math.round(LOUPE.h * dpr);
    if (c.width !== bw || c.height !== bh) {
      c.width = bw;
      c.height = bh;
    }
    const ctx = c.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, LOUPE.w, LOUPE.h);

    const sw = Math.round((LOUPE.w / LOUPE.zoom) * scale);
    const sh = Math.round((LOUPE.h / LOUPE.zoom) * scale);
    const cx = Math.round(cursor.x * scale);
    const cy = Math.round(cursor.y * scale);
    const sx = cx - (sw >> 1);
    const sy = cy - (sh >> 1);
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(src, sx, sy, sw, sh, 0, 0, LOUPE.w, LOUPE.h);

    ctx.strokeStyle = "rgba(74,158,255,0.6)";
    ctx.lineWidth = 2.5;
    ctx.beginPath();
    ctx.moveTo(LOUPE.w / 2, 0);
    ctx.lineTo(LOUPE.w / 2, LOUPE.h);
    ctx.moveTo(0, LOUPE.h / 2);
    ctx.lineTo(LOUPE.w, LOUPE.h / 2);
    ctx.stroke();
    const box = 9;
    const bx = Math.round(LOUPE.w / 2 - box / 2);
    const by = Math.round(LOUPE.h / 2 - box / 2);
    ctx.fillStyle = "rgba(255,255,255,0.9)";
    ctx.fillRect(bx, by, box, box);
    ctx.strokeStyle = "rgba(0,0,0,0.8)";
    ctx.lineWidth = 1;
    ctx.strokeRect(bx + 0.5, by + 0.5, box - 1, box - 1);

    try {
      const d = src.getContext("2d")?.getImageData(cx, cy, 1, 1).data;
      if (d) setRgb([d[0], d[1], d[2]]);
    } catch {
      /* 越界取不到就不显示色值 */
    }
  }, [cursor, scale, baseReady]);

  /* ---------------- 手势 ---------------- */

  type Gesture =
    | { k: "new"; from: P; detected: Rect | null; moved: boolean }
    | { k: "move"; from: P; orig: Rect }
    | { k: "resize"; h: string; from: P; orig: Rect }
    | { k: "draw" }
    | { k: "erase"; orig: Op[] }
    | { k: "opmove"; from: P; idx: number; orig: Op; snapshot: Op[] };

  const g = useRef<Gesture | null>(null);

  const clampR = (r: Rect): Rect => {
    const x = Math.max(0, Math.min(r.x, W));
    const y = Math.max(0, Math.min(r.y, H));
    return {
      x,
      y,
      w: Math.max(0, Math.min(r.w, W - x)),
      h: Math.max(0, Math.min(r.h, H - y)),
    };
  };

  const norm = (a: P, b: P): Rect => ({
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(b.x - a.x),
    h: Math.abs(b.y - a.y),
  });

  /** 命中把手的半径（CSS 像素）——比看起来大一点，好抓 */
  const HANDLE_HIT = 7;

  const hitHandle = (r: Rect, p: P): string | null => {
    const xs: [string, number][] = [
      ["w", r.x],
      ["e", r.x + r.w],
    ];
    const ys: [string, number][] = [
      ["n", r.y],
      ["s", r.y + r.h],
    ];
    for (const [hx, x] of xs) {
      for (const [hy, y] of ys) if (Math.abs(p.x - x) <= HANDLE_HIT && Math.abs(p.y - y) <= HANDLE_HIT) return hy + hx;
    }
    const cy = r.y + r.h / 2;
    const cx = r.x + r.w / 2;
    if (Math.abs(p.x - r.x) <= HANDLE_HIT && Math.abs(p.y - cy) <= HANDLE_HIT + 4) return "w";
    if (Math.abs(p.x - (r.x + r.w)) <= HANDLE_HIT && Math.abs(p.y - cy) <= HANDLE_HIT + 4) return "e";
    if (Math.abs(p.y - r.y) <= HANDLE_HIT && Math.abs(p.x - cx) <= HANDLE_HIT + 4) return "n";
    if (Math.abs(p.y - (r.y + r.h)) <= HANDLE_HIT && Math.abs(p.x - cx) <= HANDLE_HIT + 4) return "s";
    return null;
  };

  const insideR = (r: Rect, p: P) => p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h;

  /** 点在哪个标注上（从最上层往下找）。p 是 CSS 坐标，opBox 是物理坐标 */
  const hitOp = (p: P): number | null => {
    const q = { x: p.x * scale, y: p.y * scale };
    for (let i = opsRef.current.length - 1; i >= 0; i--) {
      const b = opBox(opsRef.current[i]);
      if (b && q.x >= b.x && q.x <= b.x + b.w && q.y >= b.y && q.y <= b.y + b.h) return i;
    }
    return null;
  };

  /** 鼠标 CSS 坐标 → **整屏物理坐标**（标注就存在这个坐标系里，改选区不会跟着飘） */
  const toPhys = (e: { clientX: number; clientY: number }): P => ({
    x: e.clientX * scale,
    y: e.clientY * scale,
  });

  /* ---------------- 工具选择 ---------------- */

  const isTexty = tool === "text" || tool === "callout";

  const pickTool = (t: ToolId) => {
    const def = GROUPS.flatMap((g) => g.items).find((i) => i.id === t);
    if (def?.off) {
      say(`${def.name}：入口已就位，像素级能力下一版接`);
      return;
    }
    const tt = t as Tool;
    /* 点得快的时候，悬停展开的 110ms 定时器还没到点；不清掉它，选完工具托盘还会自己弹出来 */
    window.clearTimeout(hoverT.current.open);
    setTool(tt);
    const g = groupOf(tt);
    if (g) setFaces((m) => (m[g.id] === tt ? m : { ...m, [g.id]: tt }));
    setOpenGroup(null);
    setPaletteOpen(false);
    setMenuOpen(false);
    setSelOp(null);
    if (tt === "text") say("文字：在选区内单击输入，Enter 确认");
    if (tt === "callout") say("引线气泡：单击落点后输入文字，Enter 确认");
    if (tt === "badge") say(`序号标记：单击落点，下一个编号 ${badgeRef.current}（双击图标归零）`);
    if (tt === "blur") say("高斯模糊：拖一个框，不透明度/半径跟随线条粗细");
  };

  const pickFace = (g: GroupDef) => pickTool(faces[g.id] ?? g.face);

  const resetBadge = () => {
    setBadge(1);
    setTool("badge");
    setFaces((m) => ({ ...m, text: "badge" }));
    say("序号已归零，下一个是 1");
  };

  /* 悬停展开：110ms 开 / 190ms 关。计时器都要清，否则快速横穿会残留 */
  const hoverT = useRef({ open: 0, close: 0 });
  const hoverOpen = (id: string) => {
    window.clearTimeout(hoverT.current.close);
    window.clearTimeout(hoverT.current.open);
    hoverT.current.open = window.setTimeout(() => {
      setOpenGroup(id);
      setPaletteOpen(false);
      setMenuOpen(false);
    }, 110);
  };
  const hoverClose = () => {
    window.clearTimeout(hoverT.current.open);
    hoverT.current.close = window.setTimeout(() => setOpenGroup(null), 190);
  };

  /* 托盘贴边平移：先按 --shift:0 量，再算需要的位移（必须在 rAF 里量） */
  useEffect(() => {
    if (!openGroup) {
      setTrayShift(0);
      return;
    }
    const id = requestAnimationFrame(() => {
      const el = barRef.current?.querySelector<HTMLElement>(`[data-tray="${openGroup}"]`);
      if (!el) return;
      const r = el.getBoundingClientRect();
      let s = 0;
      if (r.left < 12) s = 12 - r.left;
      else if (r.right > window.innerWidth - 12) s = window.innerWidth - 12 - r.right;
      setTrayShift(s);
    });
    return () => cancelAnimationFrame(id);
  }, [openGroup]);

  /* 点空白处收起调色板 / 输出菜单（工具条自身 stopPropagation，不会误触） */
  useEffect(() => {
    if (!paletteOpen && !menuOpen) return;
    const close = () => {
      setPaletteOpen(false);
      setMenuOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [paletteOpen, menuOpen]);

  /* ---------------- 粗细 / 字号（滚轮连续调） ---------------- */

  const widthHudTimer = useRef(0);
  const showWidthHud = useCallback(() => {
    setWidthHud(true);
    window.clearTimeout(widthHudTimer.current);
    widthHudTimer.current = window.setTimeout(() => setWidthHud(false), 1200);
  }, []);

  const setValue = useCallback(
    (v: number, withHud: boolean) => {
      const texty = toolRef.current === "text" || toolRef.current === "callout";
      if (texty) setFontSize(Math.min(FONT_MAX, Math.max(FONT_MIN, Math.round(v))));
      else setWidth(Math.min(WIDTH_MAX, Math.max(WIDTH_MIN, Math.round(v * 10) / 10)));
      if (withHud) showWidthHud();
    },
    [showWidthHud]
  );

  /** dir: +1 变粗 / -1 变细 */
  const bumpValue = useCallback(
    (dir: number, big: boolean, fine: boolean) => {
      const texty = toolRef.current === "text" || toolRef.current === "callout";
      if (texty) {
        setFontSize((v) => Math.min(FONT_MAX, Math.max(FONT_MIN, v + dir * (big ? 4 : 1))));
      } else {
        const step = fine ? 0.1 : big ? 2 : 0.5;
        setWidth((v) => Math.min(WIDTH_MAX, Math.max(WIDTH_MIN, Math.round((v + dir * step) * 10) / 10)));
      }
      showWidthHud();
    },
    [showWidthHud]
  );

  /* 滚轮必须**原生非 passive** 挂（React 的 onWheel 是 passive，preventDefault 无效，
     页面会跟着滚 —— WORKLOG §10.5 #28）。触控板会连发小 delta，累计 ≥40 才走一档。 */
  useEffect(() => {
    const el = widthBtn.current;
    if (!el) return;
    let acc = 0;
    let timer = 0;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      acc += e.deltaY;
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        acc = 0;
      }, 220);
      if (Math.abs(acc) < 40) return;
      bumpValue(acc < 0 ? 1 : -1, e.shiftKey, e.altKey);
      acc = 0;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      el.removeEventListener("wheel", onWheel);
      window.clearTimeout(timer);
    };
  }, [bumpValue]);

  /* 竖向拖拽：10px ≈ 1px（粗细）/ 0.4 号（字号），给没有滚轮的触控板用户 */
  const startWidthDrag = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.button !== 0) return;
    const texty = toolRef.current === "text" || toolRef.current === "callout";
    const start = { y: e.clientY, v: texty ? fontRef.current : widthRef.current, texty, moved: false };
    const onMove = (ev: MouseEvent) => {
      const dy = start.y - ev.clientY;
      if (Math.abs(dy) > 2) start.moved = true;
      if (!start.moved) return;
      setValue(start.v + dy * (start.texty ? 0.4 : 0.1), true);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      if (!start.moved) showWidthHud();
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  /* ---------------- 取色 ---------------- */

  const sampleColor = (e: { clientX: number; clientY: number }) => {
    const b = base.current;
    if (!b) return;
    const x = Math.round(e.clientX * scale);
    const y = Math.round(e.clientY * scale);
    try {
      const d = b.getContext("2d")?.getImageData(x, y, 1, 1).data;
      if (!d) return;
      const hex = `#${[d[0], d[1], d[2]].map((v) => v.toString(16).padStart(2, "0")).join("")}`.toUpperCase();
      setColor(hex);
      say(`已取色 ${hex}`);
    } catch {
      say("这个位置取不到颜色");
    }
  };

  /* ---------------- 指针 ---------------- */

  const cursorStyle = (() => {
    if (dropper) return "copy";
    if (!rect) return "crosshair";
    const h = hitHandle(rect, cursor);
    if (h) {
      const map: Record<string, string> = {
        nw: "nwse-resize",
        se: "nwse-resize",
        ne: "nesw-resize",
        sw: "nesw-resize",
        n: "ns-resize",
        s: "ns-resize",
        w: "ew-resize",
        e: "ew-resize",
      };
      return map[h] ?? "crosshair";
    }
    if (insideR(rect, cursor)) {
      if (!tool || space) return "move";
      return "crosshair";
    }
    return "crosshair";
  })();

  const down = (e: React.MouseEvent) => {
    setEscArm(false);
    if (e.button === 1) {
      e.preventDefault();
      if (rectRef.current) void finish("peek", "middle-click");
      return;
    }
    if (e.button !== 0) return;
    /* 别让浏览器的"按下鼠标就把焦点挪走"抢掉文字输入框的焦点 */
    e.preventDefault();
    if (textAt) submitText();

    if (dropper) {
      sampleColor(e);
      setDropper(false);
      return;
    }

    const p = { x: e.clientX, y: e.clientY };
    const r = rectRef.current;

    if (r) {
      const h = hitHandle(r, p);
      if (h) {
        g.current = { k: "resize", h, from: p, orig: r };
        return;
      }
      if (insideR(r, p)) {
        if (e.altKey) {
          sampleColor(e);
          return;
        }
        /* 选择工具：点中哪条就选哪条，按着拖 = 挪它 */
        if (tool === "select") {
          const idx = hitOp(p);
          setSelOp(idx);
          if (idx != null) {
            g.current = { k: "opmove", from: p, idx, orig: opsRef.current[idx], snapshot: opsRef.current };
          }
          return;
        }
        if (!tool || space) {
          g.current = { k: "move", from: p, orig: r };
          return;
        }
        if (tool === "eraser") {
          g.current = { k: "erase", orig: opsRef.current };
          eraseAt(p);
          return;
        }
        const q = toPhys(e);
        if (tool === "text" || tool === "callout") {
          openText(q, tool);
          return;
        }
        if (tool === "badge" || tool === "check" || tool === "cross") {
          stampAt(tool, q);
          return;
        }
        const op = newOp(tool, q, {
          color: colorRef.current,
          width: widthRef.current,
          fill: fillRef.current,
          scale,
          solid: solidRef.current,
        });
        if (!op) return;
        live.current = { op };
        g.current = { k: "draw" };
        schedule();
        return;
      }
      /* 选区外按下 = 重开一个 */
      commit([]);
      setSelOp(null);
      setTextAt(null);
      g.current = { k: "new", from: p, detected: null, moved: false };
      setRect({ x: p.x, y: p.y, w: 0, h: 0 });
      return;
    }

    /* 没有选区：点在识别到的窗口上 = 直接选中它 */
    g.current = { k: "new", from: p, detected: hover, moved: false };
    if (hover) setRect(hover);
    else setRect({ x: p.x, y: p.y, w: 0, h: 0 });
  };

  const move = (e: React.MouseEvent) => {
    const p = { x: e.clientX, y: e.clientY };
    setCursor(p);
    const ge = g.current;
    if (!ge) {
      if (!rect) void detect(p);
      return;
    }
    if (ge.k === "new") {
      if (!ge.moved && Math.abs(p.x - ge.from.x) + Math.abs(p.y - ge.from.y) > 3) ge.moved = true;
      if (ge.moved) setRect(clampR(norm(ge.from, p)));
      return;
    }
    if (ge.k === "move") {
      const dx = p.x - ge.from.x;
      const dy = p.y - ge.from.y;
      setRect(clampR({ ...ge.orig, x: ge.orig.x + dx, y: ge.orig.y + dy }));
      return;
    }
    if (ge.k === "resize") {
      setRect(clampR(resizeRect(ge.orig, ge.h, p.x - ge.from.x, p.y - ge.from.y)));
      return;
    }
    if (ge.k === "draw") {
      const d = live.current;
      if (!d) return;
      const q = toPhys(e);
      const o = d.op;
      if (isFreehand(o)) d.op = { ...o, pts: [...o.pts, q] };
      else if (isBoxOp(o)) d.op = { ...o, b: q };
      live.current = { op: d.op };
      schedule();
      return;
    }
    if (ge.k === "opmove") {
      const dx = (p.x - ge.from.x) * scale;
      const dy = (p.y - ge.from.y) * scale;
      const moved = shiftOp(ge.orig, dx, dy);
      const next = opsRef.current.map((o, i) => (i === ge.idx ? moved : o));
      opsRef.current = next;
      setOps(next);
      return;
    }
    if (ge.k === "erase") eraseAt(p);
  };

  const up = () => {
    const ge = g.current;
    g.current = null;
    if (!ge) return;
    if (ge.k === "new") {
      const r = rectRef.current;
      if (ge.detected && !ge.moved) {
        setRect(ge.detected);
        return;
      }
      if (!r || r.w < 3 || r.h < 3) {
        if (ge.detected) setRect(ge.detected);
        else setRect(null);
      }
      return;
    }
    if (ge.k === "draw") {
      const d = live.current;
      live.current = null;
      if (d) {
        const op = d.op;
        let tiny = false;
        if (isFreehand(op)) tiny = op.pts.length < 2;
        else if (isBoxOp(op)) tiny = Math.abs(op.b.x - op.a.x) < 3 && Math.abs(op.b.y - op.a.y) < 3;
        if (!tiny) commit([...opsRef.current, op]);
      }
      schedule();
      return;
    }
    if (ge.k === "opmove") {
      if (opsRef.current !== ge.snapshot) recordHistory(ge.snapshot);
      return;
    }
    if (ge.k === "erase") {
      if (opsRef.current !== ge.orig) recordHistory(ge.orig);
    }
  };

  const resizeRect = (r: Rect, h: string, dx: number, dy: number): Rect => {
    let { x, y, w, hh } = { x: r.x, y: r.y, w: r.w, hh: r.h };
    let x2 = r.x + r.w;
    let y2 = r.y + r.h;
    if (h.includes("w")) x += dx;
    if (h.includes("e")) x2 += dx;
    if (h.includes("n")) y += dy;
    if (h.includes("s")) y2 += dy;
    x = Math.min(x, x2 - 1);
    y = Math.min(y, y2 - 1);
    w = Math.max(1, x2 - x);
    hh = Math.max(1, y2 - y);
    return { x, y, w, h: hh };
  };

  /** 橡皮擦按位置擦（与「选中后 Delete」互补）。拖拽期间不记历史，松手一次性入栈 */
  const eraseAt = (p: P) => {
    const q = { x: p.x * scale, y: p.y * scale };
    for (let i = opsRef.current.length - 1; i >= 0; i--) {
      const b = opBox(opsRef.current[i]);
      if (b && q.x >= b.x && q.x <= b.x + b.w && q.y >= b.y && q.y <= b.y + b.h) {
        const next = opsRef.current.slice(0, i).concat(opsRef.current.slice(i + 1));
        opsRef.current = next;
        setOps(next);
        return;
      }
    }
  };

  /* 窗口识别（照 Snipaste）：光标移动时问一次，节流 70ms */
  const detectAt = useRef(0);
  const detect = async (p: P) => {
    const now = performance.now();
    if (now - detectAt.current < 70) return;
    detectAt.current = now;
    try {
      const r = await source.snipWindowAt(p.x, p.y, scale);
      setHover(r ? { ...r } : null);
    } catch {
      /* 忽略 */
    }
  };

  /* ---------------- 文字 / 图章 ---------------- */

  const openText = (q: P, kind: "text" | "callout") => {
    setTextKind(kind);
    setTextAt(q);
    setText("");
    requestAnimationFrame(() => textRef.current?.focus());
  };

  const submitText = () => {
    if (!textAt || !text.trim()) {
      setTextAt(null);
      setText("");
      return;
    }
    if (textKind === "callout") {
      commit([...opsRef.current, { t: "callout", p: textAt, s: text, color, size: Math.round(fontSize * scale) }]);
    } else {
      commit([...opsRef.current, { t: "text", p: textAt, s: text, color, size: Math.round(fontSize * scale) }]);
    }
    setTextAt(null);
    setText("");
  };

  /** 单击落点类：序号 / 勾 / 叉。一次点击一条，统一进历史 */
  const stampAt = (kind: "badge" | "check" | "cross", q: P) => {
    if (kind === "badge") {
      const r = 13 * scale;
      commit([...opsRef.current, { t: "badge", p: q, n: badgeRef.current, color: colorRef.current, r }]);
      setBadge(badgeRef.current + 1);
      return;
    }
    const cssR = Math.max(8, Math.min(26, 6 + widthRef.current * 1.4));
    commit([...opsRef.current, { t: kind, p: q, color: colorRef.current, r: cssR * scale }]);
  };

  /* ---------------- 交付 ---------------- */

  const compose = async (): Promise<string> => {
    const r = rectRef.current;
    if (!r || r.w < 2 || r.h < 2) throw new Error("请先框一个区域");
    if (!base.current) throw new Error("截图底图还没准备好，请稍等一秒");
    const sx = Math.round(r.x * scale);
    const sy = Math.round(r.y * scale);
    const sw = Math.max(1, Math.round(r.w * scale));
    const sh = Math.max(1, Math.round(r.h * scale));
    const c = document.createElement("canvas");
    c.width = sw;
    c.height = sh;
    const ctx = c.getContext("2d");
    if (!ctx) throw new Error("画布不可用");
    ctx.drawImage(base.current, sx, sy, sw, sh, 0, 0, sw, sh);
    ctx.save();
    ctx.translate(-sx, -sy);
    for (const op of opsRef.current) drawOp(ctx, op, base.current);
    if (live.current) drawOp(ctx, live.current.op, base.current);
    ctx.restore();
    return c.toDataURL("image/png");
  };

  /**
   * 交付：把合成图交给后端。
   *
   * `from` 只用来写日志 —— 本软件出过两次「截图完成后自动复制」，查了半天才发现是
   * **某个没说清的入口**（Alt+F1 / Enter / 双击）在偷偷调它。现在每个调用点都必须
   * 报自己的名字，日志里 `snip finish action=copy from=ctrl+c` 一眼就能定位。
   *
   * 冷却：选区刚变过 350ms 内不接（框选手势尾巴上松开鼠标那一帧可能顺带触发点击）。
   */
  const finish = async (action: Action, from = "btn") => {
    if (busy) return;
    if (Date.now() - rectBorn.current < 350) {
      say("选区刚调整过，松手后再按一次");
      return;
    }
    setBusy(true);
    setErr("");
    try {
      if (!base.current) {
        const t0 = Date.now();
        while (!base.current && Date.now() - t0 < 1200) {
          await new Promise((r) => window.setTimeout(r, 40));
        }
      }
      const png = await compose();
      let place: [number, number, number, number] | null = null;
      const r = rectRef.current;
      if (action === "peek" && boot && r) {
        place = [
          boot.x + Math.round(r.x * scale),
          boot.y + Math.round(r.y * scale),
          Math.max(1, Math.round(r.w * scale)),
          Math.max(1, Math.round(r.h * scale)),
        ];
      }
      await source.snipFinish(png, action, from, place);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  /* ---------------- 键盘 ---------------- */

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const k = e.key;
      const low = k.toLowerCase();
      const mod = e.ctrlKey || e.metaKey;
      if (k === "Shift") {
        setHexMode((v) => !v);
        return;
      }
      if (k === " " && !textAt) {
        e.preventDefault();
        setSpace(true);
        return;
      }
      if (k === "Escape") {
        e.preventDefault();
        if (openGroup) {
          setOpenGroup(null);
          return;
        }
        if (paletteOpen || menuOpen) {
          setPaletteOpen(false);
          setMenuOpen(false);
          return;
        }
        if (dropper) {
          setDropper(false);
          return;
        }
        if (selOp != null) {
          setSelOp(null);
          return;
        }
        if (textAt) {
          setTextAt(null);
          setText("");
          return;
        }
        /* 有标注时二次确认：第一次只提醒，2.6s 内再按才真退出 */
        if (opsRef.current.length && !escArm) {
          setEscArm(true);
          say("当前有未保存的标注，再按一次 Esc 放弃并退出");
          window.setTimeout(() => setEscArm(false), 2600);
          return;
        }
        void source.snipCancel();
        return;
      }
      if (mod && low === "z") {
        e.preventDefault();
        if (e.shiftKey) redo();
        else undo();
        return;
      }
      /* 交付快捷键（0.1.27）：只认**真人按键** —— 本机有程序会向前台窗口注入 Ctrl+C
         （WORKLOG §38 那条），判据在 Rust 侧（keywatch + am_snip_finish）。 */
      if (mod && low === "c") {
        e.preventDefault();
        void finish("copy", "ctrl+c");
        return;
      }
      if (mod && e.shiftKey && low === "s") {
        e.preventDefault();
        void finish("saveas", "ctrl+shift+s");
        return;
      }
      if (mod && low === "s") {
        e.preventDefault();
        void finish("save", "ctrl+s");
        return;
      }
      if (mod && low === "a") {
        e.preventDefault();
        setRect({ x: 0, y: 0, w: W, h: H });
        return;
      }
      if (k === "Enter") {
        e.preventDefault();
        if (textAt) {
          submitText();
          return;
        }
        /* **Enter 什么都不做**（0.1.20 定案）：框完下意识按 Enter 就再也进不到标注。
           方案 v2 里 Enter = 复制，本软件**刻意不跟**，理由见文件头。 */
        say("Enter 不结束截图 · Ctrl+S 存库 / Ctrl+Shift+S 另存 / Ctrl+C 复制，或点右下「复制」");
        return;
      }
      if (!mod && (k === "ArrowLeft" || k === "ArrowRight" || k === "ArrowUp" || k === "ArrowDown")) {
        e.preventDefault();
        const d = k === "ArrowLeft" ? [-1, 0] : k === "ArrowRight" ? [1, 0] : k === "ArrowUp" ? [0, -1] : [0, 1];
        setRect((r) => {
          if (!r) return r;
          if (e.shiftKey) {
            return clampR(
              resizeRect(r, d[0] < 0 ? "w" : d[0] > 0 ? "e" : d[1] < 0 ? "n" : "s", d[0], d[1])
            );
          }
          return clampR({ ...r, x: r.x + d[0], y: r.y + d[1] });
        });
        return;
      }
      if (!mod) {
        /* W/A/S/D 微调系统光标：**只在还没框选时**。框完之后 A/S 要让位给
           箭头 / 素材组 —— 旧版本把这两颗键写死在最前面，导致 A、S 工具热键永远打不到。 */
        if (!rectRef.current && (low === "w" || low === "a" || low === "s" || low === "d")) {
          e.preventDefault();
          const d = low === "w" ? [0, -1] : low === "s" ? [0, 1] : low === "a" ? [-1, 0] : [1, 0];
          void source.cursorStep(d[0], d[1]).catch(() => {});
          return;
        }
        if ((k === "Delete" || k === "Backspace") && selOp != null) {
          e.preventDefault();
          commit(opsRef.current.filter((_, i) => i !== selOp));
          setSelOp(null);
          return;
        }
        if (k === "[" || k === "]") {
          e.preventDefault();
          bumpValue(k === "]" ? 1 : -1, e.shiftKey, e.altKey);
          return;
        }
        const map: Record<string, Tool> = {
          v: "select",
          r: "rect",
          e: "ellipse",
          l: "line",
          a: "arrow",
          p: "pen",
          h: "marker",
          t: "text",
          n: "badge",
          k: "callout",
          m: "mosaic",
          b: "blur",
          g: "block",
          x: "check",
          c: "cross",
          y: "eraser",
        };
        const t = map[low];
        if (t) {
          e.preventDefault();
          pickTool(t);
          return;
        }
        if (low === "s" || low === "i") {
          e.preventDefault();
          setOpenGroup("sticker");
          say("素材组：勾 / 叉可用；贴纸与插图下一版接入");
          return;
        }
        if (low === "f") {
          e.preventDefault();
          setFill((v) => !v);
          return;
        }
        return;
      }
      if (e.shiftKey && low === "x") {
        e.preventDefault();
        setHexMode((v) => !v);
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === " ") setSpace(false);
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy, textAt, selOp, openGroup, paletteOpen, menuOpen, dropper, escArm, W, H]);

  const copyColor = async () => {
    if (!rgb) return;
    const txt = hexMode
      ? `#${rgb.map((v) => v.toString(16).padStart(2, "0")).join("")}`.toUpperCase()
      : `${rgb[0]}, ${rgb[1]}, ${rgb[2]}`;
    try {
      await navigator.clipboard.writeText(txt);
      setTip(`已复制 ${txt}`);
      window.setTimeout(() => setTip(""), 1200);
    } catch {
      setTip("复制失败");
      window.setTimeout(() => setTip(""), 1200);
    }
  };

  if (!boot) return <div className="snip-miss">截图数据丢失，请重新按一次快捷键</div>;

  /* ---------------- 布局计算 ---------------- */

  const barVisible = !!rect && rect.w > 2 && rect.h > 2 && !textAt;
  const barPos = (() => {
    if (!rect) return { left: 0, top: 0 };
    const left = Math.max(8, Math.min(rect.x + rect.w / 2 - barW / 2, W - barW - 8));
    let top = rect.y + rect.h + 14;
    if (top + BAR_H > H - 8) top = rect.y - 14 - BAR_H;
    if (top < 8) top = Math.min(H - BAR_H - 8, rect.y + rect.h + 14);
    return { left, top };
  })();
  /** 工具条贴近屏幕顶部时，托盘 / 菜单改到下方展开（否则会被裁掉） */
  const trayDown = barPos.top < 210;

  /** 工具条翻到选区**上方**时，左上角那块 HUD 会正好压在工具条底下（z 30 > 12），
      所以这种情况 HUD 挪到选区内侧顶部。 */
  const barAbove = !!rect && barPos.top + BAR_H <= rect.y + 1;
  const hudTop = rect ? (barAbove ? rect.y + 6 : rect.y > 32 ? rect.y - 30 : rect.y + 4) : 0;
  const hudLeft = rect ? Math.min(rect.x, Math.max(0, W - 214)) : 0;

  /** 左下角键位面板和工具条撞上时把面板收掉（工具条是主操作，不能被人挡住）。
      面板尺寸是固定的（5 行 × 22 + 内距），这里用常量比每次量 DOM 便宜。 */
  const KEYS_W = 410;
  const KEYS_H = 132;
  const keysOverlap =
    barVisible &&
    barPos.left < 12 + KEYS_W &&
    barPos.top + BAR_H > H - 12 - KEYS_H &&
    barPos.left + barW > 12;
  /* 放大镜什么时候出：光标在选区里（或还没选区）时才有；压到工具栏上就收起 */
  const overBar =
    barVisible &&
    cursor.x >= barPos.left - 4 &&
    cursor.x <= barPos.left + barW + 4 &&
    cursor.y >= barPos.top - 4 &&
    cursor.y <= barPos.top + BAR_H + 4;
  const showLoupe =
    baseReady && !textAt && !overBar && (!rect || insideR(rect, cursor)) && cursor.x >= 0;

  /* 放大镜默认在光标右下；贴边就翻到左上（不是硬夹，夹的话会盖住光标） */
  const loupeLeft = Math.max(
    4,
    cursor.x + 16 + LOUPE.w > W - 4 ? cursor.x - 16 - LOUPE.w : cursor.x + 16
  );
  const loupeTop = Math.max(
    4,
    cursor.y + 20 + LOUPE.h + 92 > H - 4 ? cursor.y - 20 - LOUPE.h - 92 : cursor.y + 20
  );

  const rgbText = rgb
    ? hexMode
      ? `#${rgb.map((v) => v.toString(16).padStart(2, "0")).join("").toUpperCase()}`
      : `${rgb[0]}, ${rgb[1]}, ${rgb[2]}`
    : "";

  const curGroup = groupOf(tool);
  const valueNow = isTexty ? fontSize : width;
  const trackPct = isTexty
    ? ((fontSize - FONT_MIN) / (FONT_MAX - FONT_MIN)) * 100
    : ((width - WIDTH_MIN) / (WIDTH_MAX - WIDTH_MIN)) * 100;

  const stop = (e: React.MouseEvent) => e.stopPropagation();

  /* ---------------- 渲染 ---------------- */

  return (
    <div
      className={`snip-root ${dropper ? "is-dropper" : ""}`}
      style={{
        cursor: cursorStyle,
        backgroundImage: boot.thumb ? `url("${boot.thumb}")` : undefined,
        backgroundSize: "100% 100%",
        backgroundRepeat: "no-repeat",
      }}
      onMouseDown={down}
      onMouseMove={move}
      onMouseUp={up}
      onDoubleClick={(e) => e.preventDefault()}
      onContextMenu={(e) => {
        e.preventDefault();
        if (textAt) {
          setTextAt(null);
          setText("");
          return;
        }
        if (openGroup || paletteOpen || menuOpen) {
          setOpenGroup(null);
          setPaletteOpen(false);
          setMenuOpen(false);
          return;
        }
        /* 右键：有选区就取消选区，没有就退出（Snipaste 的规矩） */
        if (rectRef.current) {
          setRect(null);
          setOps([]);
          undoStack.current = [];
          redoStack.current = [];
          setSelOp(null);
          setHover(null);
          return;
        }
        void source.snipCancel();
      }}
    >
      {hiSrc && (
        <img className="snip-bg" src={hiSrc} alt="" draggable={false} onLoad={callReady} onError={callReady} />
      )}

      {/* 标注画布：整屏物理尺寸，只在选区内可见 */}
      <canvas ref={ink} className="snip-ink" width={boot.w} height={boot.h} />

      {/* 选区外压暗 55%（Snipaste 实测值） */}
      {rect && rect.w > 0 && (
        <div className="snip-dim" style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }} />
      )}

      {/* 窗口识别框（还没有选区时） */}
      {!rect && hover && (
        <>
          <div className="snip-detect" style={{ left: hover.x, top: hover.y, width: hover.w, height: hover.h }} />
          {handles(hover, "detect")}
          <div className="snip-size" style={{ left: hover.x + 8, top: hover.y > 24 ? hover.y - 20 : hover.y + 4 }}>
            {Math.round(hover.w * scale)} × {Math.round(hover.h * scale)} px
          </div>
        </>
      )}

      {/* 选区 + HUD（宽高 / 屏幕坐标 / 缩放比，等宽数字） */}
      {rect && rect.w > 1 && rect.h > 1 && (
        <>
          <div className="snip-sel" style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }} />
          {handles(rect, "sel")}
          <div className="snip-hud" style={{ left: hudLeft, top: hudTop }}>
            <b>
              {Math.round(rect.w * scale) + 1} × {Math.round(rect.h * scale) + 1}
            </b>
            <em>
              x {Math.round(boot.x + rect.x * scale)} · y {Math.round(boot.y + rect.y * scale)}
            </em>
            <span className="zoom">◎ {Math.round(scale * 100)}%</span>
          </div>
        </>
      )}

      {/* 选中的那条标注：虚线框（物理 → CSS 要除以 scale） */}
      {selOp != null && ops[selOp] && (() => {
        const b = opBox(ops[selOp]);
        if (!b) return null;
        return (
          <div
            className="snip-opsel"
            style={{
              left: b.x / scale,
              top: b.y / scale,
              width: Math.max(4, b.w / scale),
              height: Math.max(4, b.h / scale),
            }}
          />
        );
      })()}

      {/* ---------------- 工具条 ---------------- */}
      {barVisible && (
        <div
          className="snip-bar"
          ref={barRef}
          style={{ left: barPos.left, top: barPos.top }}
          onMouseDown={stop}
          onMouseUp={stop}
          onMouseMove={stop}
          onDoubleClick={stop}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
          role="toolbar"
          aria-label="截图标注工具"
        >
          {/* 1 选择（常驻） */}
          <button
            className={`snip-tbtn ${tool === "select" ? "is-on" : ""}`}
            onClick={() => (tool === "select" ? setTool(null) : pickTool("select"))}
            aria-label="选择 / 移动标注"
          >
            {ICONS.select}
            <span className="snip-tip">
              选择 / 移动标注 <kbd>V</kbd>
            </span>
          </button>
          <span className="snip-sep" />

          {/* 2-6 折叠组 */}
          {GROUPS.map((g) => {
            const face = faces[g.id] ?? g.face;
            const def = g.items.find((i) => i.id === face) ?? g.items[0];
            const on = !!curGroup && curGroup.id === g.id;
            const changed = face !== g.face;
            return (
              <div
                key={g.id}
                className={`snip-stack ${openGroup === g.id ? "is-open" : ""}`}
                onMouseEnter={() => hoverOpen(g.id)}
                onMouseLeave={hoverClose}
              >
                <button
                  className={`snip-tbtn snip-stack__face ${on ? "is-on" : ""}`}
                  onClick={() => pickFace(g)}
                  aria-haspopup="menu"
                  aria-expanded={openGroup === g.id}
                  aria-label={g.name}
                >
                  {ICONS[def.icon]}
                  <i className={`snip-caret ${changed ? "is-alt" : ""}`} />
                  <span className="snip-tip">
                    {g.name} <kbd>{def.key}</kbd>
                  </span>
                </button>
                <div
                  className={`snip-tray ${openGroup === g.id ? "is-open" : ""} ${trayDown ? "down" : ""}`}
                  style={{ "--shift": `${trayShift}px` } as React.CSSProperties}
                  role="menu"
                  aria-label={g.name}
                  data-tray={g.id}
                >
                  {g.items.map((it) => (
                    <button
                      key={it.id}
                      className={`snip-tbtn ${tool === it.id ? "is-on" : ""} ${it.off ? "is-off" : ""}`}
                      role="menuitem"
                      onClick={() => pickTool(it.id)}
                      onDoubleClick={(e) => {
                        e.stopPropagation();
                        if (it.id === "badge") resetBadge();
                      }}
                      aria-disabled={it.off || undefined}
                    >
                      {ICONS[it.icon]}
                      <span className="snip-tip">
                        {it.name} <kbd>{it.key}</kbd>
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            );
          })}

          {/* 7 属性组：颜色 / 粗细 / 填充 */}
          <span className="snip-sep" />
          <div className="snip-prop">
            <div className="snip-anchor">
              <button
                className={`snip-chip ${paletteOpen ? "is-open" : ""}`}
                onMouseDown={stop}
                onClick={() => {
                  setPaletteOpen((v) => !v);
                  setMenuOpen(false);
                  setOpenGroup(null);
                }}
                aria-label="标注颜色"
                aria-haspopup="dialog"
                aria-expanded={paletteOpen}
              >
                <i className="snip-swatch" style={{ background: color }} />
                <span className="snip-tip">
                  标注颜色 <kbd>Alt 吸管</kbd>
                </span>
              </button>
              {paletteOpen && (
                <div
                  className={`snip-palette ${trayDown ? "down" : ""}`}
                  onMouseDown={stop}
                  role="dialog"
                  aria-label="调色板"
                >
                  <div className="snip-pal-label">标注颜色</div>
                  <div className="snip-swatches">
                    {COLORS.map((c) => (
                      <button
                        key={c}
                        className={`snip-sw ${c.toUpperCase() === color.toUpperCase() ? "is-on" : ""}`}
                        style={{ background: c }}
                        aria-label={`颜色 ${c}`}
                        onClick={() => {
                          setColor(c);
                          setPaletteOpen(false);
                          say(`已选颜色 ${c}`);
                        }}
                      />
                    ))}
                  </div>
                  <div className="snip-pal-foot">
                    <button
                      className="snip-pal-btn"
                      onClick={() => {
                        setPaletteOpen(false);
                        setDropper(true);
                        say("吸管：在图上单击取色");
                      }}
                    >
                      {ICONS.pen}
                      吸管取色
                    </button>
                    <label className="snip-pal-btn snip-pal-custom">
                      自定义…
                      <input type="color" value={color} onChange={(e) => setColor(e.target.value)} />
                    </label>
                  </div>
                </div>
              )}
            </div>

            <div className="snip-anchor">
              <button
                ref={widthBtn}
                className={`snip-chip snip-chip--width ${widthHud ? "is-open" : ""}`}
                onMouseDown={startWidthDrag}
                onDoubleClick={(e) => {
                  e.stopPropagation();
                  if (isTexty) setValue(17, true);
                  else setValue(2, true);
                }}
                aria-label={isTexty ? "字号：滚轮调节" : "线条粗细：滚轮调节"}
              >
                {isTexty ? ICONS.text : <WidthGlyph w={width} />}
                <span className="snip-tip">
                  {isTexty ? "字号" : "线条粗细"} <kbd>滚轮</kbd>
                </span>
              </button>
              {widthHud && (
                <div className={`snip-whud ${trayDown ? "down" : ""}`} role="status" aria-live="polite">
                  <div className="snip-whud__num">
                    <b>{valueNow.toFixed(isTexty ? 0 : 1)}</b>
                    <span>px</span>
                  </div>
                  <div className="snip-whud__preview">
                    <i
                      style={{
                        height: `${Math.min(14, Math.max(1, isTexty ? 6 : width))}px`,
                        background: color,
                      }}
                    />
                  </div>
                  <div className="snip-whud__track">
                    <i style={{ left: `${trackPct}%` }} />
                  </div>
                  <div className="snip-whud__range">
                    <em>{isTexty ? FONT_MIN : WIDTH_MIN}</em>
                    <em>{isTexty ? FONT_MAX : WIDTH_MAX}</em>
                  </div>
                </div>
              )}
            </div>

            <button
              className={`snip-tbtn ${fill ? "is-on" : ""}`}
              onClick={() => setFill((v) => !v)}
              aria-pressed={fill}
              aria-label="形状填充"
            >
              {ICONS.fill}
              <span className="snip-tip">
                形状填充 <kbd>F</kbd>
              </span>
            </button>

            {tool === "arrow" && (
              <button
                className={`snip-tbtn ${arrowSolid ? "is-on" : ""}`}
                onClick={() => setArrowSolid((v) => !v)}
                aria-pressed={arrowSolid}
                aria-label="箭头样式"
              >
                {ICONS.arrow}
                <span className="snip-tip">{arrowSolid ? "实心箭头" : "空心箭头"}</span>
              </button>
            )}
          </div>

          {/* 8 历史 + 橡皮擦 */}
          <span className="snip-sep" />
          <button
            className="snip-tbtn"
            disabled={!undoStack.current.length}
            onClick={undo}
            aria-label="撤销"
          >
            {ICONS.undo}
            <span className="snip-tip">
              撤销 <kbd>Ctrl+Z</kbd>
            </span>
          </button>
          <button
            className="snip-tbtn"
            disabled={!redoStack.current.length}
            onClick={redo}
            aria-label="重做"
          >
            {ICONS.redo}
            <span className="snip-tip">
              重做 <kbd>Ctrl+Shift+Z</kbd>
            </span>
          </button>
          <button
            className={`snip-tbtn ${tool === "eraser" ? "is-on" : ""}`}
            onClick={() => pickTool("eraser")}
            aria-label="橡皮擦"
          >
            {ICONS.eraser}
            <span className="snip-tip">
              橡皮擦 <kbd>Y</kbd>
            </span>
          </button>

          {/* 9 输出：复制主按钮 + 下拉 */}
          <span className="snip-sep" />
          <div className={`snip-split ${menuOpen ? "is-open" : ""}`}>
            <button className="snip-split__go" onClick={() => void finish("copy", "btn")}>
              {ICONS.copy}
              复制
            </button>
            <button
              className="snip-split__more"
              onClick={() => {
                setMenuOpen((v) => !v);
                setPaletteOpen(false);
                setOpenGroup(null);
              }}
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              aria-label="更多输出方式"
            >
              {ICONS.chevron}
            </button>
            <div
              className={`snip-menu ${trayDown ? "down" : ""} ${menuOpen ? "is-open" : ""}`}
              role="menu"
              onMouseDown={stop}
            >
              <button className="snip-mi" onClick={() => void finish("copy", "menu")}>
                {ICONS.copy}
                复制到剪贴板
                <kbd>Ctrl+C</kbd>
              </button>
              <button className="snip-mi" onClick={() => void finish("save", "menu")}>
                {ICONS.save}
                保存到截图库
                <kbd>Ctrl+S</kbd>
              </button>
              <button className="snip-mi" onClick={() => void finish("saveas", "menu")}>
                {ICONS.save}
                另存为 PNG…
                <kbd>Ctrl+Shift+S</kbd>
              </button>
              <button className="snip-mi" onClick={() => void finish("peek", "menu")}>
                {ICONS.pin}
                贴到桌面（贴图）
                <kbd>中键</kbd>
              </button>
              <div className="snip-mi-sep" />
              <button
                className="snip-mi is-off"
                onClick={() => say("OCR 文字识别：下一版接入（P1）")}
                aria-disabled
              >
                {ICONS.ocr}
                识别图中文字
                <kbd>下一版</kbd>
              </button>
            </div>
          </div>

          {/* 10 退出 */}
          <button
            className="snip-tbtn snip-close"
            onClick={() => {
              if (opsRef.current.length && !escArm) {
                setEscArm(true);
                say("当前有未保存的标注，再点一次 ✕ 放弃并退出");
                window.setTimeout(() => setEscArm(false), 2600);
                return;
              }
              void source.snipCancel();
            }}
            aria-label="退出截图"
          >
            {ICONS.close}
            <span className="snip-tip">
              退出 <kbd>Esc</kbd>
            </span>
          </button>
        </div>
      )}

      {/* 放大镜 + 信息条 */}
      {showLoupe && (
        <div className="snip-loupe" style={{ left: loupeLeft, top: loupeTop }}>
          <canvas ref={loupeCv} className="snip-loupe-cv" width={LOUPE.w} height={LOUPE.h} />
          <div className="snip-loupe-info">
            <div className="snip-coord">
              ({Math.round(cursor.x * scale)} , {Math.round(cursor.y * scale)})
            </div>
            {rgb && (
              <div
                className="snip-rgb"
                title="点击复制颜色值"
                onMouseDown={stop}
                onClick={() => void copyColor()}
              >
                <span className="snip-swatch-dot" style={{ background: `rgb(${rgb[0]},${rgb[1]},${rgb[2]})` }} />
                <span>{rgbText}</span>
              </div>
            )}
            <div className="snip-hintline">点色值复制 · Shift 切 RGB / HEX</div>
            <div className="snip-hintline">Alt 单击直接取色</div>
          </div>
        </div>
      )}

      {/* 左下角快捷键面板 */}
      <div className={`snip-keys ${keysOverlap ? "is-hidden" : ""}`} onMouseDown={stop}>
        <div className="snip-keyrow">
          <span className="keys">
            <K>V</K>
            <K>R</K>
            <K>T</K>
            <K>M</K>
          </span>
          <span>选择 / 形状 / 文字 / 遮挡</span>
        </div>
        <div className="snip-keyrow">
          <span className="keys">
            <K>[</K>
            <K>]</K>
            <K>滚轮</K>
          </span>
          <span>线条粗细（Shift 2 / Alt 0.1）</span>
        </div>
        <div className="snip-keyrow">
          <span className="keys">
            <K>Ctrl</K>
            <K>Z</K>
          </span>
          <span>撤销 / 重做（加 Shift）</span>
        </div>
        <div className="snip-keyrow">
          <span className="keys">
            <K>Ctrl</K>
            <K>C</K>
            <K>Ctrl</K>
            <K>S</K>
          </span>
          <span>复制 / 存到截图库</span>
        </div>
        <div className="snip-keyrow">
          <span className="keys">
            <K>↑</K>
            <K>↓</K>
            <K>←</K>
            <K>→</K>
          </span>
          <span>微调选区（Shift 改大小）</span>
        </div>
      </div>

      {textAt && rect && (
        <input
          ref={textRef}
          className="snip-text"
          autoFocus
          value={text}
          style={{
            left: textAt.x / scale,
            top: textAt.y / scale - (textKind === "callout" ? Math.round(fontSize * 1.6 + 14) : 0),
            color,
            fontSize,
          }}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            e.nativeEvent.stopPropagation();
            if (e.key === "Enter") submitText();
            if (e.key === "Escape") {
              setTextAt(null);
              setText("");
            }
          }}
          onBlur={submitText}
        />
      )}

      {tip && <div className="snip-toast">{tip}</div>}
      {hint && <div className="snip-hint">{hint}</div>}
      {err && <div className="snip-err">{err}</div>}
      {busy && <div className="snip-toast">处理中…</div>}
    </div>
  );
}

/* ---------------- 小组件 ---------------- */

function K({ children }: { children: ReactNode }) {
  return <kbd className="snip-kbd">{children}</kbd>;
}

function handles(r: Rect, kind: "sel" | "detect") {
  const pts = [
    [r.x, r.y],
    [r.x + r.w / 2, r.y],
    [r.x + r.w, r.y],
    [r.x + r.w, r.y + r.h / 2],
    [r.x + r.w, r.y + r.h],
    [r.x + r.w / 2, r.y + r.h],
    [r.x, r.y + r.h],
    [r.x, r.y + r.h / 2],
  ];
  return pts.map(([x, y], i) => (
    <span key={i} className={`snip-dot ${kind}`} style={{ left: x, top: y }} />
  ));
}

/* ---------------- 绘制 ---------------- */

interface OpCfg {
  color: string;
  width: number;
  fill: boolean;
  scale: number;
  solid: boolean;
}

function newOp(t: Tool, p: P, cfg: OpCfg): Op | null {
  const { color, width, fill, scale, solid } = cfg;
  if (t === "mosaic") return { t: "mosaic", a: p, b: p };
  if (t === "blur") return { t: "blur", a: p, b: p, r: Math.max(3, width * 2) * scale };
  if (t === "block") return { t: "block", a: p, b: p, color };
  if (t === "pen" || t === "marker") {
    /* 荧光笔：宽度 = 线宽 × 4，35% 不透明度（方案 F-05） */
    const w = Math.max(1, (t === "marker" ? Math.max(4, width * 4) : width) * scale);
    return { t, pts: [p], color, w, alpha: t === "marker" ? 0.35 : 1 };
  }
  const w = Math.max(0.6, width * scale);
  if (t === "rect" || t === "ellipse") return { t, a: p, b: p, color, w, fill };
  if (t === "line") return { t, a: p, b: p, color, w };
  if (t === "arrow") return { t, a: p, b: p, color, w, solid };
  return null;
}

/** 整条标注平移（物理像素） */
function shiftOp(op: Op, dx: number, dy: number): Op {
  const mv = (p: P): P => ({ x: p.x + dx, y: p.y + dy });
  switch (op.t) {
    case "pen":
    case "marker":
      return { ...op, pts: op.pts.map(mv) };
    case "text":
    case "callout":
    case "badge":
    case "check":
    case "cross":
      return { ...op, p: mv(op.p) };
    default: {
      const o = op as BoxOp;
      return { ...o, a: mv(o.a), b: mv(o.b) };
    }
  }
}

/** 量文字宽度：CJK 用 0.62 估算会明显偏窄，这里用 canvas 真量 */
let measureCv: HTMLCanvasElement | null = null;
function textW(s: string, size: number) {
  if (!measureCv) measureCv = document.createElement("canvas");
  const c = measureCv.getContext("2d");
  if (!c) return s.length * size * 0.62;
  c.font = `600 ${size}px "Microsoft YaHei", "Inter Variable", sans-serif`;
  return c.measureText(s).width;
}

/** 引线气泡的几何：气泡在落点上方（贴顶时翻到下方），尾巴指着落点 */
function calloutGeom(op: Extract<Op, { t: "callout" }>) {
  const w = Math.max(textW(op.s || " ", op.size) + 20, 56);
  const h = op.size * 1.5 + 12;
  const above = op.p.y - h - 16 >= 0;
  return {
    x: op.p.x - 12,
    y: above ? op.p.y - h - 16 : op.p.y + 16,
    w,
    h,
    above,
  };
}

function opBox(op: Op): Rect | null {
  if (isFreehand(op)) {
    if (!op.pts.length) return null;
    let x0 = Infinity,
      y0 = Infinity,
      x1 = -Infinity,
      y1 = -Infinity;
    for (const p of op.pts) {
      x0 = Math.min(x0, p.x);
      y0 = Math.min(y0, p.y);
      x1 = Math.max(x1, p.x);
      y1 = Math.max(y1, p.y);
    }
    const pad = op.w / 2 + 2;
    return { x: x0 - pad, y: y0 - pad, w: x1 - x0 + pad * 2, h: y1 - y0 + pad * 2 };
  }
  if (op.t === "text") {
    return { x: op.p.x - 4, y: op.p.y - 4, w: textW(op.s, op.size) + 8, h: op.size * 1.5 };
  }
  if (op.t === "badge" || op.t === "check" || op.t === "cross") {
    return { x: op.p.x - op.r - 2, y: op.p.y - op.r - 2, w: op.r * 2 + 4, h: op.r * 2 + 4 };
  }
  if (op.t === "callout") return calloutGeom(op);
  if (!isBoxOp(op)) return null;
  const x = Math.min(op.a.x, op.b.x);
  const y = Math.min(op.a.y, op.b.y);
  const w = Math.abs(op.b.x - op.a.x);
  const h = Math.abs(op.b.y - op.a.y);
  let pad = 0;
  if (op.t === "rect" || op.t === "line") pad = op.w / 2 + 2;
  else if (op.t === "arrow") pad = Math.max(op.w / 2 + 2, 8);
  return { x: x - pad, y: y - pad, w: w + pad * 2, h: h + pad * 2 };
}

/** 在给定底色上该用白字还是深字（序号 / 引线气泡都是"色底 + 字"） */
function readableOn(hex: string) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return "#fff";
  const n = parseInt(m[1], 16);
  const r = (n >> 16) & 255;
  const g = (n >> 8) & 255;
  const b = n & 255;
  /* 相对亮度（sRGB 近似）：>0.62 用深字，否则白字 */
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return lum > 0.62 ? "#141412" : "#fff";
}

/** 马赛克 / 模糊用的临时画布（每次复用，别在循环里 new） */
let scratch: HTMLCanvasElement | null = null;
let blurScratch: HTMLCanvasElement | null = null;

function rrPath(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const rad = Math.max(0, Math.min(r, w / 2, h / 2));
  ctx.beginPath();
  ctx.moveTo(x + rad, y);
  ctx.arcTo(x + w, y, x + w, y + h, rad);
  ctx.arcTo(x + w, y + h, x, y + h, rad);
  ctx.arcTo(x, y + h, x, y, rad);
  ctx.arcTo(x, y, x + w, y, rad);
  ctx.closePath();
}

function drawOp(ctx: CanvasRenderingContext2D, op: Op, src: HTMLCanvasElement | null) {
  ctx.save();
  switch (op.t) {
    case "rect":
    case "ellipse": {
      const x = Math.min(op.a.x, op.b.x);
      const y = Math.min(op.a.y, op.b.y);
      const w = Math.abs(op.b.x - op.a.x);
      const h = Math.abs(op.b.y - op.a.y);
      ctx.beginPath();
      if (op.t === "rect") ctx.rect(x, y, w, h);
      else ctx.ellipse(x + w / 2, y + h / 2, w / 2, h / 2, 0, 0, Math.PI * 2);
      /* 填充 = 当前色 20%：不遮底图文字（方案 F-03） */
      if (op.fill) {
        ctx.globalAlpha = 0.2;
        ctx.fillStyle = op.color;
        ctx.fill();
        ctx.globalAlpha = 1;
      }
      ctx.strokeStyle = op.color;
      ctx.lineWidth = op.w;
      ctx.lineJoin = "round";
      ctx.stroke();
      break;
    }
    case "line": {
      ctx.strokeStyle = op.color;
      ctx.lineWidth = op.w;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(op.a.x, op.a.y);
      ctx.lineTo(op.b.x, op.b.y);
      ctx.stroke();
      break;
    }
    case "arrow": {
      /* 照 Snipaste：细长箭杆 + 三角头（头长 = 线宽 × 4） */
      const { a, b } = op;
      const head = Math.max(op.w * 4, 12);
      const ang = Math.atan2(b.y - a.y, b.x - a.x);
      ctx.strokeStyle = op.color;
      ctx.fillStyle = op.color;
      ctx.lineWidth = op.w;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x - Math.cos(ang) * head * 0.7, b.y - Math.sin(ang) * head * 0.7);
      ctx.stroke();
      ctx.beginPath();
      ctx.moveTo(b.x, b.y);
      ctx.lineTo(b.x - Math.cos(ang - 0.4) * head, b.y - Math.sin(ang - 0.4) * head);
      ctx.lineTo(b.x - Math.cos(ang + 0.4) * head, b.y - Math.sin(ang + 0.4) * head);
      ctx.closePath();
      if (op.solid === false) {
        ctx.lineJoin = "round";
        ctx.lineWidth = Math.max(1, op.w * 0.8);
        ctx.stroke();
      } else {
        ctx.fill();
      }
      break;
    }
    case "pen":
    case "marker": {
      if (op.pts.length < 2) break;
      ctx.globalAlpha = op.alpha;
      ctx.strokeStyle = op.color;
      ctx.lineWidth = op.w;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.beginPath();
      ctx.moveTo(op.pts[0].x, op.pts[0].y);
      for (let i = 1; i < op.pts.length; i++) ctx.lineTo(op.pts[i].x, op.pts[i].y);
      ctx.stroke();
      break;
    }
    case "text": {
      ctx.fillStyle = op.color;
      ctx.font = `600 ${op.size}px "Microsoft YaHei", "Inter Variable", sans-serif`;
      ctx.textBaseline = "top";
      ctx.lineWidth = Math.max(2, op.size / 9);
      ctx.strokeStyle = "rgba(0,0,0,0.5)";
      ctx.lineJoin = "round";
      ctx.strokeText(op.s, op.p.x, op.p.y);
      ctx.fillText(op.s, op.p.x, op.p.y);
      break;
    }
    case "badge": {
      const { p, r } = op;
      ctx.fillStyle = op.color;
      ctx.beginPath();
      ctx.arc(p.x, p.y, r, 0, Math.PI * 2);
      ctx.fill();
      const ink = readableOn(op.color);
      ctx.strokeStyle = ink === "#fff" ? "rgba(255,255,255,0.92)" : "rgba(20,20,18,0.55)";
      ctx.lineWidth = Math.max(1.5, r * 0.11);
      ctx.stroke();
      ctx.fillStyle = ink;
      ctx.font = `600 ${Math.round(r * 1.15)}px "Inter Variable", "Microsoft YaHei", sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(op.n), p.x, p.y + r * 0.04);
      break;
    }
    case "callout": {
      const g = calloutGeom(op);
      ctx.fillStyle = op.color;
      /* 尾巴：从上/下沿指向落点 */
      ctx.beginPath();
      if (g.above) {
        ctx.moveTo(g.x + 16, g.y + g.h - 1);
        ctx.lineTo(g.x + 16 + 11, g.y + g.h - 1);
        ctx.lineTo(op.p.x, op.p.y + 1);
      } else {
        ctx.moveTo(g.x + 16, g.y + 1);
        ctx.lineTo(g.x + 16 + 11, g.y + 1);
        ctx.lineTo(op.p.x, op.p.y - 1);
      }
      ctx.closePath();
      ctx.fill();
      rrPath(ctx, g.x, g.y, g.w, g.h, 8);
      ctx.fill();
      ctx.fillStyle = readableOn(op.color);
      ctx.font = `600 ${op.size}px "Microsoft YaHei", "Inter Variable", sans-serif`;
      ctx.textBaseline = "top";
      ctx.fillText(op.s, g.x + 10, g.y + (g.h - op.size * 1.18) / 2);
      break;
    }
    case "block": {
      const x = Math.min(op.a.x, op.b.x);
      const y = Math.min(op.a.y, op.b.y);
      ctx.globalAlpha = 0.92;
      ctx.fillStyle = op.color;
      ctx.fillRect(x, y, Math.abs(op.b.x - op.a.x), Math.abs(op.b.y - op.a.y));
      ctx.globalAlpha = 1;
      break;
    }
    case "check":
    case "cross": {
      const { p, r } = op;
      ctx.strokeStyle = op.color;
      ctx.lineWidth = Math.max(2, r * 0.16);
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.arc(p.x, p.y, r, 0, Math.PI * 2);
      ctx.stroke();
      const k = r * 0.46;
      ctx.beginPath();
      if (op.t === "check") {
        ctx.moveTo(p.x - k, p.y + r * 0.04);
        ctx.lineTo(p.x - k * 0.16, p.y + k * 0.9);
        ctx.lineTo(p.x + k, p.y - k * 0.6);
      } else {
        ctx.moveTo(p.x - k, p.y - k);
        ctx.lineTo(p.x + k, p.y + k);
        ctx.moveTo(p.x + k, p.y - k);
        ctx.lineTo(p.x - k, p.y + k);
      }
      ctx.stroke();
      break;
    }
    case "mosaic": {
      if (!src) break;
      const x0 = Math.max(0, Math.min(op.a.x, op.b.x));
      const y0 = Math.max(0, Math.min(op.a.y, op.b.y));
      const x1 = Math.min(src.width, Math.max(op.a.x, op.b.x));
      const y1 = Math.min(src.height, Math.max(op.a.y, op.b.y));
      const w = x1 - x0;
      const h = y1 - y0;
      if (w < 2 || h < 2) break;
      /* 缩小再放大 + 关插值 = 经典像素块（比逐块 getImageData 快得多） */
      const blk = 11;
      const sw = Math.max(1, Math.round(w / blk));
      const sh = Math.max(1, Math.round(h / blk));
      if (!scratch) scratch = document.createElement("canvas");
      scratch.width = sw;
      scratch.height = sh;
      const sctx = scratch.getContext("2d");
      if (!sctx) break;
      sctx.imageSmoothingEnabled = true;
      sctx.clearRect(0, 0, sw, sh);
      sctx.drawImage(src, x0, y0, w, h, 0, 0, sw, sh);
      ctx.save();
      ctx.beginPath();
      ctx.rect(x0, y0, w, h);
      ctx.clip();
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(scratch, 0, 0, sw, sh, x0, y0, sw * blk, sh * blk);
      ctx.restore();
      break;
    }
    case "blur": {
      if (!src) break;
      const x0 = Math.max(0, Math.min(op.a.x, op.b.x));
      const y0 = Math.max(0, Math.min(op.a.y, op.b.y));
      const x1 = Math.min(src.width, Math.max(op.a.x, op.b.x));
      const y1 = Math.min(src.height, Math.max(op.a.y, op.b.y));
      const w = x1 - x0;
      const h = y1 - y0;
      if (w < 2 || h < 2) break;
      const pad = Math.max(8, Math.round(op.r * 2));
      const sx = Math.max(0, x0 - pad);
      const sy = Math.max(0, y0 - pad);
      const ex = Math.min(src.width, x1 + pad);
      const ey = Math.min(src.height, y1 + pad);
      const sw = ex - sx;
      const sh = ey - sy;
      if (!blurScratch) blurScratch = document.createElement("canvas");
      blurScratch.width = sw;
      blurScratch.height = sh;
      const sctx = blurScratch.getContext("2d");
      if (!sctx) break;
      sctx.clearRect(0, 0, sw, sh);
      /* WebView2（Chromium）从 Chrome 52 起就支持 canvas filter，不需要降级分支 */
      sctx.filter = `blur(${op.r}px)`;
      sctx.drawImage(src, sx, sy, sw, sh, 0, 0, sw, sh);
      sctx.filter = "none";
      ctx.save();
      ctx.beginPath();
      ctx.rect(x0, y0, w, h);
      ctx.clip();
      ctx.drawImage(blurScratch, 0, 0, sw, sh, sx, sy, sw, sh);
      ctx.restore();
      break;
    }
  }
  ctx.restore();
}
