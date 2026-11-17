import { useCallback, useEffect, useRef, useState } from "react";
import { Copy, Eraser, Highlighter, Pencil, Pin, Redo2, Save, Type, Undo2, X } from "lucide-react";
import { source } from "../data/source";

/**
 * 截图覆盖层（F1 / Alt+F1）—— **按 Snipaste 2.11.3 逐项复刻**。
 *
 * 参照物是用 Snipaste 实机截图量出来的（2026-11-17，2560×1600 @125%）：
 * - 选区边框 1px `#2080f0`，8 颗蓝点把手（白圈）；
 * - 选区外压暗 **55% 黑**（实测 255→115、45→20）；
 * - 尺寸标签：白色 13px 带投影，贴在选区**左上方**外侧；
 * - 工具栏：**浅灰 `#f2f2f2` + 1px 蓝边**、圆角 3、高 34、**右对齐选区右边界**、
 *   在选区下方 9px（放不下翻到上方），13 个按钮：形状/箭头/铅笔/记号笔/马赛克/文字/橡皮擦
 *   ｜撤销/重做｜取消/贴图/保存/复制；
 * - 放大镜：184×136、8 倍、蓝色十字 + 白色中心像素框，下方一块深色信息条
 *   （坐标 / 色块 + RGB / 换行提示）；
 * - 左下角常驻「快捷键」竖排面板（键帽 + 说明）。
 *
 * 交互也照它：一开始**没有激活任何工具**（拖框内 = 移动选区），
 * 点工具才进入标注；右键 = 取消选区（没有选区时退出）；Esc = 退出；
 * Enter = 复制并退出；Ctrl+S = 保存；Ctrl+A = 全屏；方向键微调（Shift = 改大小）。
 *
 * 坐标两套，不许混：屏幕上一律 **CSS 像素**，画布 / 标注一律**物理像素**（CSS × scale）。
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
  mode: string;
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

type Tool = "rect" | "ellipse" | "line" | "arrow" | "pen" | "marker" | "text" | "mosaic" | "eraser";
type ShapeTool = "rect" | "ellipse" | "line";
type Action = "copy" | "save" | "saveas" | "peek";

/** 一条标注。坐标全是**物理像素**，相对整块虚拟桌面（不相对选区）——
 *  这样改选区大小时标注不会跟着飘。 */
type Op =
  | { t: "rect" | "ellipse" | "line" | "arrow"; a: P; b: P; color: string; w: number }
  | { t: "pen" | "marker"; pts: P[]; color: string; w: number; alpha: number }
  | { t: "text"; p: P; s: string; color: string; size: number }
  | { t: "mosaic"; a: P; b: P };

/** 笔类标注（有 pts、没有 a/b） */
const isFreehand = (op: Op): op is Extract<Op, { t: "pen" | "marker" }> =>
  op.t === "pen" || op.t === "marker";
/** 框类标注（有 a/b） */
const isBoxOp = (op: Op): op is Extract<Op, { a: P; b: P }> =>
  op.t === "rect" || op.t === "ellipse" || op.t === "line" || op.t === "arrow" || op.t === "mosaic";

const BLUE = "#2080f0";
const SHAPE_LABEL: Record<ShapeTool, string> = { rect: "矩形", ellipse: "椭圆", line: "直线" };
const DIM = 0.55;
const COLORS = ["#f5222d", "#fa8c16", "#fadb14", "#52c41a", "#13c2c2", "#2080f0", "#722ed1", "#ffffff", "#000000"];
const LOUPE = { w: 184, h: 136, zoom: 8 };
const BAR_H = 34;

/* ---------------- 图标（照 Snipaste 的细线风格手绘） ---------------- */

const svg = (children: React.ReactNode, box = "0 0 20 20") => (
  <svg viewBox={box} width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.35" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    {children}
  </svg>
);

const IcoShape = () =>
  svg(
    <>
      <rect x="2.4" y="4.4" width="11" height="8.6" rx="1.4" />
      <circle cx="13.4" cy="12" r="4" fill="#f2f2f2" />
    </>
  );

const IcoArrow = () =>
  svg(
    <>
      <path d="M2.6 14.6 L7.4 9.6 L10.6 11.8 L16 4.9" />
      <path d="M12.6 4.6 L16.4 4.5 L16.3 8.3" />
    </>
  );

const IcoMosaic = () =>
  svg(
    <>
      <rect x="2.6" y="6.4" width="9.2" height="9.2" fill="currentColor" stroke="none" opacity="0.9" />
      <rect x="5.4" y="3.4" width="9.2" height="9.2" fill="#f2f2f2" stroke="currentColor" />
      <path d="M5.4 6.6h4.6M5.4 9.7h4.6M8.6 3.4v4.6M11.6 3.4v4.6" />
    </>
  );

/* ---------------- 入口 ---------------- */

export function Snip() {
  const boot = bootData();
  const [scale, setScale] = useState(boot?.scale ?? 1);
  const W = boot ? boot.w / scale : window.innerWidth;
  const H = boot ? boot.h / scale : window.innerHeight;

  const [rect, setRect] = useState<Rect | null>(null);
  const [hover, setHover] = useState<Rect | null>(null);
  const [tool, setTool] = useState<Tool | null>(null);
  const [shape, setShape] = useState<ShapeTool>("rect");
  const [color, setColor] = useState(COLORS[0]);
  const [widthScale, setWidthScale] = useState(1);
  const [ops, setOps] = useState<Op[]>([]);
  const [redo, setRedo] = useState<Op[]>([]);
  const [cursor, setCursor] = useState<P>({ x: -999, y: -999 });
  const [rgb, setRgb] = useState<[number, number, number] | null>(null);
  const [hexMode, setHexMode] = useState(false);
  const [options, setOptions] = useState(false);
  const [textAt, setTextAt] = useState<P | null>(null);
  const [text, setText] = useState("");
  const textRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [tip, setTip] = useState("");
  const [space, setSpace] = useState(false);
  const [hiSrc, setHiSrc] = useState<string | null>(null);
  const [barW, setBarW] = useState(424);

  const rectRef = useRef<Rect | null>(null);
  rectRef.current = rect;
  const opsRef = useRef<Op[]>([]);
  opsRef.current = ops;
  const colorRef = useRef(color);
  colorRef.current = color;
  const toolRef = useRef<Tool | null>(null);
  toolRef.current = tool;
  const spaceRef = useRef(false);
  spaceRef.current = space;

  /* 画布 / 像素源 */
  const ink = useRef<HTMLCanvasElement>(null);
  const base = useRef<HTMLCanvasElement | null>(null);
  const [baseReady, setBaseReady] = useState(false);
  const loupeCv = useRef<HTMLCanvasElement>(null);
  const raf = useRef(0);
  const live = useRef<{ op: Op } | null>(null);

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

  const readyOnce = useRef(false);
  const callReady = useCallback(() => {
    if (readyOnce.current) return;
    readyOnce.current = true;
    requestAnimationFrame(() => requestAnimationFrame(() => void source.snipReady().catch(() => {})));
  }, []);

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

  /* 高清底图延后挂载：先让 480 宽的小图上屏，WebView 首帧来得更快 */
  useEffect(() => {
    if (!boot) return;
    if (!boot.thumb) {
      setHiSrc(boot.src);
      return;
    }
    const t = window.setTimeout(() => setHiSrc(boot.src), 500);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [boot?.src, boot?.thumb]);

  /* 像素源：同源 data URL → 能 getImageData / toDataURL（放大镜、马赛克、导出都要） */
  useEffect(() => {
    let alive = true;
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
    return () => {
      alive = false;
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

    /* 源区域：CSS 尺寸 / 倍数 × 缩放比 = 物理像素 */
    const sw = Math.round((LOUPE.w / LOUPE.zoom) * scale);
    const sh = Math.round((LOUPE.h / LOUPE.zoom) * scale);
    const cx = Math.round(cursor.x * scale);
    const cy = Math.round(cursor.y * scale);
    const sx = cx - (sw >> 1);
    const sy = cy - (sh >> 1);
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(src, sx, sy, sw, sh, 0, 0, LOUPE.w, LOUPE.h);

    /* 十字线（半透明蓝）+ 中心像素框 */
    ctx.strokeStyle = "rgba(32,128,240,0.55)";
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

    /* 取色 */
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
    | { k: "erase" };

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

  /** 鼠标 CSS 坐标 → **整屏物理坐标**（标注就存在这个坐标系里，改选区不会跟着飘） */
  const toPhys = (e: { clientX: number; clientY: number }): P => ({
    x: e.clientX * scale,
    y: e.clientY * scale,
  });

  /** 点工具：没选中 → 选中；已经选中 → 开/关它的选项（颜色 / 线宽）。
   *  这个"再点一次出选项"是 Snipaste 的手感，工具栏上就不再放色块了。 */
  const pickTool = (t: Tool) => {
    if (tool === t) setOptions((v) => !v);
    else {
      setTool(t);
      setOptions(false);
    }
  };

  const commitOp = (op: Op) => {
    setOps((os) => [...os, op]);
    setRedo([]);
  };

  /** 光标形状 */
  const cursorStyle = (() => {
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
    if (e.button === 1) {
      e.preventDefault();
      if (rectRef.current) void finish("peek");
      return;
    }
    if (e.button !== 0) return;
    /* 别让浏览器的"按下鼠标就把焦点挪走"抢掉文字输入框的焦点
       （不 preventDefault 的话：setTextAt 后 input 刚 focus，紧接着就被 blur 掉，
       onBlur 又把它当"输入完了"，于是文字工具像没反应） */
    e.preventDefault();
    if (textAt) submitText();
    const p = { x: e.clientX, y: e.clientY };
    const r = rectRef.current;

    if (r) {
      const h = hitHandle(r, p);
      if (h) {
        g.current = { k: "resize", h, from: p, orig: r };
        return;
      }
      if (insideR(r, p)) {
        if (!tool || space) {
          g.current = { k: "move", from: p, orig: r };
          return;
        }
        if (tool === "eraser") {
          g.current = { k: "erase" };
          eraseAt(p);
          return;
        }
        const q = toPhys(e);
        if (tool === "text") {
          setTextAt(q);
          setText("");
          requestAnimationFrame(() => textRef.current?.focus());
          return;
        }
        live.current = { op: newOp(tool, q, color, widthScale, scale) };
        g.current = { k: "draw" };
        schedule();
        return;
      }
      /* 选区外按下 = 重开一个 */
      setOps([]);
      setRedo([]);
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
    if (ge.k === "erase") eraseAt(p);
  };

  const up = () => {
    const ge = g.current;
    g.current = null;
    if (!ge) return;
    if (ge.k === "new") {
      const r = rectRef.current;
      if (ge.detected && !ge.moved) {
        setRect(ge.detected); // 单击识别到的窗口
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
        if (!tiny) commitOp(op);
      }
      schedule();
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

  const eraseAt = (p: P) => {
    const q = { x: p.x * scale, y: p.y * scale };
    setOps((os) => {
      for (let i = os.length - 1; i >= 0; i--) {
        const b = opBox(os[i], scale);
        if (b && q.x >= b.x && q.x <= b.x + b.w && q.y >= b.y && q.y <= b.y + b.h) {
          const next = os.slice(0, i).concat(os.slice(i + 1));
          return next;
        }
      }
      return os;
    });
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

  const submitText = () => {
    if (!textAt || !text.trim()) {
      setTextAt(null);
      setText("");
      return;
    }
    commitOp({ t: "text", p: textAt, s: text, color, size: Math.round(17 * scale) });
    setTextAt(null);
    setText("");
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

  const finish = async (action: Action) => {
    if (busy) return;
    setBusy(true);
    setErr("");
    try {
      const png = await compose();
      await source.snipFinish(png, action);
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
        if (textAt) {
          setTextAt(null);
          setText("");
          return;
        }
        if (options) {
          setOptions(false);
          return;
        }
        void source.snipCancel();
        return;
      }
      if (mod && low === "z") {
        e.preventDefault();
        if (e.shiftKey) {
          setRedo((rd) => {
            if (!rd.length) return rd;
            setOps((os) => [...os, rd[rd.length - 1]]);
            return rd.slice(0, -1);
          });
        } else {
          setOps((os) => {
            if (!os.length) return os;
            setRedo((rd) => [...rd, os[os.length - 1]]);
            return os.slice(0, -1);
          });
        }
        return;
      }
      if (mod && low === "c") {
        e.preventDefault();
        void finish("copy");
        return;
      }
      if (mod && low === "s") {
        e.preventDefault();
        void finish("save");
        return;
      }
      if (mod && low === "a") {
        e.preventDefault();
        setOps([]);
        setRedo([]);
        setRect({ x: 0, y: 0, w: W, h: H });
        return;
      }
      if (k === "Enter") {
        e.preventDefault();
        /* 正在输入文字时，Enter 是"提交这段文字"，不是"完成截图" */
        if (textAt) {
          submitText();
          return;
        }
        void finish("copy");
        return;
      }
      if (!mod && (k === "ArrowLeft" || k === "ArrowRight" || k === "ArrowUp" || k === "ArrowDown")) {
        e.preventDefault();
        const d = k === "ArrowLeft" ? [-1, 0] : k === "ArrowRight" ? [1, 0] : k === "ArrowUp" ? [0, -1] : [0, 1];
        setRect((r) => {
          if (!r) return r;
          if (e.shiftKey) {
            return clampR(
              resizeRect(
                r,
                d[0] < 0 ? "w" : d[0] > 0 ? "e" : d[1] < 0 ? "n" : "s",
                d[0],
                d[1]
              )
            );
          }
          return clampR({ ...r, x: r.x + d[0], y: r.y + d[1] });
        });
        return;
      }
      if (!mod) {
        if (low === "w" || low === "a" || low === "s" || low === "d") {
          e.preventDefault();
          const d = low === "w" ? [0, -1] : low === "s" ? [0, 1] : low === "a" ? [-1, 0] : [1, 0];
          void source.cursorStep(d[0], d[1]).catch(() => {});
          return;
        }
        const map: Record<string, Tool | "shape"> = {
          r: "shape",
          e: "ellipse",
          l: "line",
          a: "arrow",
          p: "pen",
          h: "marker",
          m: "mosaic",
          t: "text",
          x: "eraser",
        };
        const t = map[low];
        if (t) {
          e.preventDefault();
          if (t === "shape") setTool(shape);
          else setTool(t);
          return;
        }
        if (low === "c") {
          e.preventDefault();
          void copyColor();
          return;
        }
        return;
      }
      /* Ctrl+Shift+X：RGB/HEX */
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
  }, [busy, options, textAt, W, H]);

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
    let top = rect.y + rect.h + 9;
    if (top + BAR_H > H - 2) top = rect.y - 9 - BAR_H;
    if (top < 2) top = Math.min(H - BAR_H - 2, rect.y + rect.h + 9);
    let left = rect.x + rect.w - barW;
    left = Math.max(2, Math.min(left, W - barW - 2));
    return { left, top };
  })();

  const labelTop = rect ? (rect.y > 24 ? rect.y - 20 : rect.y + 4) : 0;
  /* 放大镜什么时候出：Snipaste 的规矩是"光标在选区里（或还没选区）时才有"，
     光标跑到选区外面、或者压到工具栏上就收起来。 */
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

  /* ---------------- 渲染 ---------------- */

  return (
    <div
      className="snip-root"
      style={{
        cursor: cursorStyle,
        backgroundImage: boot.thumb ? `url("${boot.thumb}")` : undefined,
        backgroundSize: "100% 100%",
        backgroundRepeat: "no-repeat",
      }}
      onMouseDown={down}
      onMouseMove={move}
      onMouseUp={up}
      onDoubleClick={(e) => {
        if (rectRef.current && insideR(rectRef.current, { x: e.clientX, y: e.clientY })) void finish("copy");
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        if (textAt) {
          setTextAt(null);
          setText("");
          return;
        }
        if (options) {
          setOptions(false);
          return;
        }
        /* 右键：有选区就取消选区，没有就退出（Snipaste 的规矩） */
        if (rectRef.current) {
          setRect(null);
          setOps([]);
          setRedo([]);
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
        <div
          className="snip-dim"
          style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
        />
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

      {/* 选区 */}
      {rect && rect.w > 1 && rect.h > 1 && (
        <>
          <div className="snip-sel" style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }} />
          {handles(rect, "sel")}
          <div className="snip-size" style={{ left: rect.x + 8, top: labelTop }}>
            {Math.round(rect.w * scale) + 1} × {Math.round(rect.h * scale) + 1} px
          </div>
        </>
      )}

      {/* 工具栏 */}
      {barVisible && (
        <div
          className="snip-bar"
          ref={(el) => {
            if (el && Math.abs(el.offsetWidth - barW) > 1) setBarW(el.offsetWidth);
          }}
          style={{ left: barPos.left, top: barPos.top }}
          onMouseDown={(e) => e.stopPropagation()}
          onMouseUp={(e) => e.stopPropagation()}
          onMouseMove={(e) => e.stopPropagation()}
          onDoubleClick={(e) => e.stopPropagation()}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
        >
          <BarBtn
            label={`形状：${SHAPE_LABEL[shape]}（再点切换）`}
            keyHint="R / E / L"
            active={tool === shape}
            onPick={() => {
              if (tool === shape) setShape((s) => (s === "rect" ? "ellipse" : s === "ellipse" ? "line" : "rect"));
              else setTool(shape);
            }}
            icon={<IcoShape />}
          />
          <BarBtn
            label="箭头"
            keyHint="A"
            active={tool === "arrow"}
            badge
            onPick={() => pickTool("arrow")}
            icon={<IcoArrow />}
          />
          <BarBtn label="铅笔" keyHint="P" active={tool === "pen"} onPick={() => pickTool("pen")} icon={<Pencil size={15} strokeWidth={1.5} />} />
          <BarBtn
            label="记号笔"
            keyHint="H"
            active={tool === "marker"}
            onPick={() => pickTool("marker")}
            icon={<Highlighter size={15} strokeWidth={1.5} />}
          />
          <BarBtn label="马赛克" keyHint="M" active={tool === "mosaic"} onPick={() => pickTool("mosaic")} icon={<IcoMosaic />} />
          <BarBtn label="文字" keyHint="T" active={tool === "text"} onPick={() => pickTool("text")} icon={<Type size={15} strokeWidth={1.5} />} />
          <BarBtn label="橡皮擦" keyHint="X" active={tool === "eraser"} onPick={() => pickTool("eraser")} icon={<Eraser size={15} strokeWidth={1.5} />} />
          <span className="snip-sep" />
          <BarBtn
            label="撤销"
            keyHint="Ctrl+Z"
            disabled={!ops.length}
            onPick={() =>
              setOps((os) => {
                if (!os.length) return os;
                setRedo((rd) => [...rd, os[os.length - 1]]);
                return os.slice(0, -1);
              })
            }
            icon={<Undo2 size={15} strokeWidth={1.5} />}
          />
          <BarBtn
            label="重做"
            keyHint="Ctrl+Shift+Z"
            disabled={!redo.length}
            onPick={() =>
              setRedo((rd) => {
                if (!rd.length) return rd;
                setOps((os) => [...os, rd[rd.length - 1]]);
                return rd.slice(0, -1);
              })
            }
            icon={<Redo2 size={15} strokeWidth={1.5} />}
          />
          <span className="snip-sep" />
          <BarBtn label="取消" keyHint="Esc" danger onPick={() => void source.snipCancel()} icon={<X size={15} strokeWidth={1.6} />} />
          <BarBtn label="贴图" keyHint="中键" onPick={() => void finish("peek")} icon={<Pin size={15} strokeWidth={1.5} />} />
          <BarBtn label="保存到截图库" keyHint="Ctrl+S" onPick={() => void finish("save")} icon={<Save size={15} strokeWidth={1.5} />} />
          <BarBtn label="复制到剪贴板" keyHint="Enter" onPick={() => void finish("copy")} icon={<Copy size={15} strokeWidth={1.5} />} />
        </div>
      )}

      {/* 工具选项（点同一个工具两次弹出）：颜色 + 线宽 */}
      {barVisible && options && (
        <div
          className="snip-opts"
          style={{ left: barPos.left, top: barPos.top + BAR_H + 4 }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <div className="snip-opts-row">
            {COLORS.map((c) => (
              <button
                key={c}
                className={`snip-color ${color === c ? "on" : ""}`}
                style={{ background: c }}
                title={c}
                onClick={() => setColor(c)}
              />
            ))}
          </div>
          <div className="snip-opts-row">
            {[1, 2, 3].map((n) => (
              <button
                key={n}
                className={`snip-width ${widthScale === n ? "on" : ""}`}
                onClick={() => setWidthScale(n)}
                title={`线宽 ${n}`}
              >
                <span style={{ height: n * 2 }} />
              </button>
            ))}
          </div>
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
              <div className="snip-rgb">
                <span className="snip-swatch-dot" style={{ background: `rgb(${rgb[0]},${rgb[1]},${rgb[2]})` }} />
                <span>{rgbText}</span>
              </div>
            )}
            <div className="snip-hintline">按 C 复制颜色值</div>
            <div className="snip-hintline">Shift 切换 RGB / HEX</div>
          </div>
        </div>
      )}

      {/* 左下角快捷键面板（Snipaste 常驻那块） */}
      <div className="snip-keys" onMouseDown={(e) => e.stopPropagation()}>
        <div className="snip-keyrow">
          <span className="keys">
            <K>W</K>
            <K>A</K>
            <K>S</K>
            <K>D</K>
          </span>
          <span>按 1 像素微调光标</span>
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
        <div className="snip-keyrow">
          <span className="keys">
            <K>Ctrl</K>
            <K>A</K>
          </span>
          <span>选中整屏</span>
        </div>
        <div className="snip-keyrow">
          <span className="keys">
            <K>Space</K>
            <K>拖动</K>
          </span>
          <span>移动选区</span>
        </div>
        <div className="snip-keyrow">
          <span className="keys">
            <K>Enter</K>
          </span>
          <span>复制并退出</span>
        </div>
        <div className="snip-keyrow">
          <span className="keys">
            <K>Ctrl</K>
            <K>S</K>
          </span>
          <span>保存到截图库</span>
        </div>
      </div>

      {textAt && rect && (
        <input
          ref={textRef}
          className="snip-text"
          autoFocus
          value={text}
          style={{ left: textAt.x / scale, top: textAt.y / scale, color, fontSize: 17 }}
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
      {err && <div className="snip-err">{err}</div>}
      {busy && <div className="snip-toast">处理中…</div>}
    </div>
  );
}

/* ---------------- 小组件 ---------------- */

function K({ children }: { children: React.ReactNode }) {
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

function BarBtn({
  label,
  keyHint,
  icon,
  active,
  disabled,
  danger,
  badge,
  onPick,
}: {
  label: string;
  keyHint?: string;
  icon: React.ReactNode;
  active?: boolean;
  disabled?: boolean;
  danger?: boolean;
  badge?: boolean;
  onPick: () => void;
}) {
  return (
    <button
      className={`snip-btn ${active ? "on" : ""} ${danger ? "danger" : ""}`}
      disabled={disabled}
      onClick={onPick}
      onMouseEnter={(e) => {
        const el = e.currentTarget;
        const r = el.getBoundingClientRect();
        el.dataset.tip = keyHint ? `${label}   ${keyHint}` : label;
        el.style.setProperty("--tip-x", `${r.left + r.width / 2}px`);
      }}
      aria-label={label}
    >
      {icon}
      {badge && <i className="snip-badge" />}
      <span className="snip-tip">{keyHint ? `${label}   ${keyHint}` : label}</span>
    </button>
  );
}

/* ---------------- 绘制 ---------------- */

function newOp(t: Exclude<Tool, "eraser">, p: P, color: string, ws: number, scale: number): Op {
  const lw = Math.max(1, Math.round((t === "marker" ? 14 : 2.4) * ws * scale));
  if (t === "mosaic") return { t: "mosaic", a: p, b: p };
  if (t === "pen" || t === "marker") {
    return { t, pts: [p], color, w: lw, alpha: t === "marker" ? 0.32 : 1 };
  }
  if (t === "text") return { t: "text", p, s: "", color, size: Math.round(17 * scale) };
  return { t, a: p, b: p, color, w: lw };
}

function opBox(op: Op, _scale: number): Rect | null {
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
  if (op.t === "text") return { x: op.p.x - 4, y: op.p.y - 4, w: op.s.length * op.size * 0.62 + 8, h: op.size * 1.5 };
  if (!isBoxOp(op)) return null;
  const x = Math.min(op.a.x, op.b.x);
  const y = Math.min(op.a.y, op.b.y);
  const w = Math.abs(op.b.x - op.a.x);
  const h = Math.abs(op.b.y - op.a.y);
  const pad = op.t === "mosaic" ? 0 : op.w / 2 + 2;
  return { x: x - pad, y: y - pad, w: w + pad * 2, h: h + pad * 2 };
}

/** 马赛克用的临时画布（每次复用，别在循环里 new） */
let scratch: HTMLCanvasElement | null = null;

function drawOp(ctx: CanvasRenderingContext2D, op: Op, src: HTMLCanvasElement | null) {
  ctx.save();
  if (op.t === "rect" || op.t === "ellipse") {
    const x = Math.min(op.a.x, op.b.x);
    const y = Math.min(op.a.y, op.b.y);
    const w = Math.abs(op.b.x - op.a.x);
    const h = Math.abs(op.b.y - op.a.y);
    ctx.strokeStyle = op.color;
    ctx.lineWidth = op.w;
    ctx.lineJoin = "round";
    ctx.beginPath();
    if (op.t === "rect") ctx.rect(x, y, w, h);
    else ctx.ellipse(x + w / 2, y + h / 2, w / 2, h / 2, 0, 0, Math.PI * 2);
    ctx.stroke();
  } else if (op.t === "line") {
    ctx.strokeStyle = op.color;
    ctx.lineWidth = op.w;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(op.a.x, op.a.y);
    ctx.lineTo(op.b.x, op.b.y);
    ctx.stroke();
  } else if (op.t === "arrow") {
    /* 照 Snipaste：细长箭杆 + 实心三角头（头长 = 线宽 × 4） */
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
    ctx.fill();
  } else if (op.t === "pen" || op.t === "marker") {
    if (op.pts.length < 2) {
      ctx.restore();
      return;
    }
    ctx.globalAlpha = op.alpha;
    ctx.strokeStyle = op.color;
    ctx.lineWidth = op.w;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.beginPath();
    ctx.moveTo(op.pts[0].x, op.pts[0].y);
    for (let i = 1; i < op.pts.length; i++) ctx.lineTo(op.pts[i].x, op.pts[i].y);
    ctx.stroke();
  } else if (op.t === "text") {
    ctx.fillStyle = op.color;
    ctx.font = `600 ${op.size}px "Microsoft YaHei", "Inter Variable", sans-serif`;
    ctx.textBaseline = "top";
    ctx.lineWidth = Math.max(2, op.size / 9);
    ctx.strokeStyle = "rgba(0,0,0,0.5)";
    ctx.lineJoin = "round";
    ctx.strokeText(op.s, op.p.x, op.p.y);
    ctx.fillText(op.s, op.p.x, op.p.y);
  } else if (op.t === "mosaic") {
    if (!src) {
      ctx.restore();
      return;
    }
    const x0 = Math.max(0, Math.min(op.a.x, op.b.x));
    const y0 = Math.max(0, Math.min(op.a.y, op.b.y));
    const x1 = Math.min(src.width, Math.max(op.a.x, op.b.x));
    const y1 = Math.min(src.height, Math.max(op.a.y, op.b.y));
    const w = x1 - x0;
    const h = y1 - y0;
    if (w < 2 || h < 2) {
      ctx.restore();
      return;
    }
    /* 缩小再放大 + 关掉插值 = 经典像素块（比逐块 getImageData 快得多，也不用读像素） */
    const blk = 11;
    const sw = Math.max(1, Math.round(w / blk));
    const sh = Math.max(1, Math.round(h / blk));
    if (!scratch) scratch = document.createElement("canvas");
    scratch.width = sw;
    scratch.height = sh;
    const sctx = scratch.getContext("2d");
    if (!sctx) {
      ctx.restore();
      return;
    }
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
  }
  ctx.restore();
}
