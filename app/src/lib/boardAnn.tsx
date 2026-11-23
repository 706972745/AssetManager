import type { ReactNode } from "react";

/**
 * 灵感板标注：截图工具条那一套工具，**模型照抄 `Snip.tsx` 的 Op，但代码是独立的**
 * （不 import 那个文件 —— 截图那条线有人在改，两边互不牵连）。
 *
 * 一条铁律：**一笔 = 一层**。所以这里的 `AnnOp` 是"单独一层里的几何"，
 * 坐标是**相对本层原点**（0,0 = 本层包围盒左上角），层的位置住在 `Board.tsx` 的
 * `board_ann.x/y` 里。这样拖动一层只改 x/y，几何 JSON 一辈子不用重写。
 *
 * 与截图那边的差异（都是"板"带来的，不是偷懒）：
 * - 没有 scale（物理像素 / CSS 像素）这一层：板的坐标本来就只有一个画布坐标系；
 * - 没有选区裁剪、没有交付（复制/保存）：板上的标注只是看的；
 * - 马赛克用 **DOM `backdrop-filter`**（近似）而不是截图那种"缩小再放大"的像素块：
 *   板的底图是**视口懒挂载**的 DOM `<img>`，重合成一遍既不可靠，也会在图片被拖动后
 *   过期。理由和代价见 `Board.tsx` 里 `.board-ann-mosaic` 的说明。
 */

export type AnnTool =
  | "select"
  | "rect"
  | "ellipse"
  | "line"
  | "arrow"
  | "pen"
  | "marker"
  | "text"
  | "badge"
  | "mosaic"
  | "eraser";

export interface AnnPt {
  x: number;
  y: number;
}
export interface AnnBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 一层标注的几何。写进 `board_ann.payload` 的就是它（JSON），坐标相对本层原点。 */
export type AnnOp =
  | { t: "rect" | "ellipse"; a: AnnPt; b: AnnPt; color: string; w: number; fill: boolean }
  | { t: "line"; a: AnnPt; b: AnnPt; color: string; w: number }
  | { t: "arrow"; a: AnnPt; b: AnnPt; color: string; w: number }
  | { t: "pen" | "marker"; pts: AnnPt[]; color: string; w: number; alpha: number }
  | { t: "text"; p: AnnPt; s: string; color: string; size: number }
  | { t: "badge"; p: AnnPt; n: number; color: string; r: number }
  | { t: "mosaic"; a: AnnPt; b: AnnPt; blur: number };

/** 板上的一层（运行时）。`op` 是局部坐标；`x / y / w / h` 是它在画布上的摆放。 */
export interface AnnLayer {
  aid: number;
  z: number;
  kind: AnnTool;
  x: number;
  y: number;
  w: number;
  h: number;
  op: AnnOp;
}

/* ---------------- 尺寸约束 ---------------- */

export const ANN_W_MIN = 0.5;
export const ANN_W_MAX = 24;
export const ANN_FONT_MIN = 8;
export const ANN_FONT_MAX = 72;

/** 21 色 7×3：中性 / 暖色警示 / 冷色与强调。和截图工具条同一套色板（值照抄，
 *  不 import —— 两边各一份，谁改都不会牵连对方）。 */
export const ANN_COLORS = [
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

/** 工具顺序 = 工具条上的顺序（也是图层面板里"同类编号"的稳定依据） */
export const ANN_TOOLS: AnnTool[] = [
  "select",
  "rect",
  "ellipse",
  "line",
  "arrow",
  "pen",
  "marker",
  "text",
  "badge",
  "mosaic",
  "eraser",
];

/** 工具名 / 图层名 / 提示的文案 key。前端只写 key，翻译由主控统一补
 *  （`t()` 缺 key 时原样显示 key，正好用来定位漏翻）。 */
export const annToolKey = (t: AnnTool) => `board.ann.tool_${t}`;
export const annKindKey = (t: AnnTool) => `board.ann.kind_${t}`;

/* ---------------- 图标（24×24 网格，stroke 1.6，渲染 20） ----------------
   照截图工具条的图形画一遍：两个界面用同一套笔画，用户不用重新认图标。 */

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

export const ANN_ICONS: Record<string, ReactNode> = {
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
  mosaic: ico(
    <path
      d="M4 4h5v5H4zM13 4h7v5h-7zM4 11h5v5H4zM13 11h7v5h-7zM4 18h5v2H4zM13 18h7v2h-7z"
      fill="currentColor"
      stroke="none"
      opacity=".85"
    />
  ),
  eraser: ico(
    <>
      <path d="M7.4 17.6 4.8 15a1.8 1.8 0 0 1 0-2.6l7.2-7.2a1.8 1.8 0 0 1 2.6 0l3.4 3.4a1.8 1.8 0 0 1 0 2.6l-6.6 6.6z" />
      <path d="M10.2 8.6l6 6" />
    </>
  ),
  fill: ico(
    <>
      <rect x="4.4" y="4.4" width="15.2" height="15.2" rx="2" />
      <path
        d="M4.4 12h15.2v5.6a2 2 0 0 1-2 2H6.4a2 2 0 0 1-2-2z"
        fill="currentColor"
        stroke="none"
        opacity=".5"
      />
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
  done: ico(<path d="M4.5 12.5l4.6 4.6L19.5 6.8" />),
};

/** 线条粗细图标：三条线随当前值变粗（和截图工具条同一种"读数在图标上"的做法） */
export function AnnWidthGlyph({ w }: { w: number }) {
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

/* ---------------- 几何 ---------------- */

/** 量文字宽度。CJK 用 0.62 估算会明显偏窄，这里让 canvas 真量（量具画布只建一次）。 */
let measureCv: HTMLCanvasElement | null = null;
export function annTextWidth(s: string, size: number): number {
  if (!measureCv) measureCv = document.createElement("canvas");
  const c = measureCv.getContext("2d");
  if (!c) return s.length * size * 0.62;
  c.font = `600 ${size}px "Microsoft YaHei", "Inter Variable", sans-serif`;
  return c.measureText(s).width;
}

/** 一层标注的位图倍率：跟着画布缩放走（放大后按新倍率重画一遍，矢量标注才不会糊），
 *  同时按总像素数封顶 —— 整块板涂满荧光笔的层不压一下会吃掉几十 MB 显存。 */
export function annBmpScale(w: number, h: number, zoom: number): number {
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  let s = Math.min(4, Math.max(0.5, dpr * zoom));
  const cap = 16e6;
  if (w * s * h * s > cap) s = Math.max(0.2, Math.sqrt(cap / Math.max(1, w * h)));
  return s;
}

/** 包围盒（局部坐标）。笔画类要留半根线宽的余量，否则 4px 的线会被裁掉一半。 */
export function annBox(op: AnnOp): AnnBox {
  switch (op.t) {
    case "rect":
    case "ellipse":
    case "line":
    case "arrow": {
      const x0 = Math.min(op.a.x, op.b.x);
      const y0 = Math.min(op.a.y, op.b.y);
      const w = Math.abs(op.b.x - op.a.x);
      const h = Math.abs(op.b.y - op.a.y);
      const pad = op.t === "arrow" ? Math.max(op.w / 2 + 2, 8) : op.w / 2 + 2;
      return { x: x0 - pad, y: y0 - pad, w: w + pad * 2, h: h + pad * 2 };
    }
    case "pen":
    case "marker": {
      if (!op.pts.length) return { x: 0, y: 0, w: 1, h: 1 };
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
    case "text": {
      const w = annTextWidth(op.s || " ", op.size);
      return { x: op.p.x - 4, y: op.p.y - 4, w: w + 8, h: op.size * 1.5 + 8 };
    }
    case "badge":
      return { x: op.p.x - op.r - 2, y: op.p.y - op.r - 2, w: op.r * 2 + 4, h: op.r * 2 + 4 };
    case "mosaic": {
      const x0 = Math.min(op.a.x, op.b.x);
      const y0 = Math.min(op.a.y, op.b.y);
      return { x: x0, y: y0, w: Math.abs(op.b.x - op.a.x), h: Math.abs(op.b.y - op.a.y) };
    }
  }
}

/** 整条几何平移（建层时从画布坐标挪进局部坐标就靠它） */
export function annShift(op: AnnOp, dx: number, dy: number): AnnOp {
  const mv = (p: AnnPt): AnnPt => ({ x: p.x + dx, y: p.y + dy });
  switch (op.t) {
    case "pen":
    case "marker":
      return { ...op, pts: op.pts.map(mv) };
    case "text":
    case "badge":
      return { ...op, p: mv(op.p) };
    default:
      return { ...op, a: mv(op.a), b: mv(op.b) };
  }
}

/**
 * 命中测试（局部坐标）：橡皮擦 / 选择工具靠它决定"点中的是哪一条"。
 * 矩形和椭圆按**边框**算（填充的按整块算），线/画笔按点到折线的距离算 ——
 * 只按包围盒判会让细长的箭头在空处也能被点中，擦起来像"删错了"。
 */
export function annHit(op: AnnOp, px: number, py: number): boolean {
  const q = { x: px, y: py };
  const tol = 6;
  const nearSeg = (a: AnnPt, b: AnnPt, t: number) => {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const len2 = dx * dx + dy * dy;
    const u = len2 > 0 ? Math.max(0, Math.min(1, ((q.x - a.x) * dx + (q.y - a.y) * dy) / len2)) : 0;
    const cx = a.x + dx * u;
    const cy = a.y + dy * u;
    return Math.hypot(q.x - cx, q.y - cy) <= t;
  };
  switch (op.t) {
    case "rect": {
      const b = annBox(op);
      if (b.w <= 0 || b.h <= 0) return false;
      if (op.fill) return q.x >= b.x && q.x <= b.x + b.w && q.y >= b.y && q.y <= b.y + b.h;
      const t = op.w / 2 + tol;
      const inside = q.x >= b.x - t && q.x <= b.x + b.w + t && q.y >= b.y - t && q.y <= b.y + b.h + t;
      const hollow =
        q.x >= b.x + t && q.x <= b.x + b.w - t && q.y >= b.y + t && q.y <= b.y + b.h - t;
      return inside && !hollow;
    }
    case "ellipse": {
      const b = annBox(op);
      const rx = b.w / 2 || 1;
      const ry = b.h / 2 || 1;
      const cx = b.x + rx;
      const cy = b.y + ry;
      const d = ((q.x - cx) / (rx + op.w / 2 + tol)) ** 2 + ((q.y - cy) / (ry + op.w / 2 + tol)) ** 2;
      if (op.fill) return d <= 1;
      /* 边框：外椭圆内、内椭圆外。内缩量取线宽 + 容差，细线也好点中 */
      const shrink = Math.max(1, op.w / 2 + tol);
      const din =
        ((q.x - cx) / Math.max(1, rx - shrink)) ** 2 + ((q.y - cy) / Math.max(1, ry - shrink)) ** 2;
      return d <= 1 && din >= 1;
    }
    case "line":
    case "arrow":
      return nearSeg(op.a, op.b, op.w / 2 + tol);
    case "pen":
    case "marker": {
      const t = op.w / 2 + tol;
      for (let i = 1; i < op.pts.length; i++) if (nearSeg(op.pts[i - 1], op.pts[i], t)) return true;
      return op.pts.length === 1
        ? Math.hypot(q.x - op.pts[0].x, q.y - op.pts[0].y) <= t
        : false;
    }
    case "text":
    case "badge":
    case "mosaic": {
      const b = annBox(op);
      return q.x >= b.x && q.x <= b.x + b.w && q.y >= b.y && q.y <= b.y + b.h;
    }
  }
}

/* ---------------- 新建一条几何 ---------------- */

export interface AnnCfg {
  color: string;
  /** 线宽 / 字号（画布单位，已经是最终值） */
  w: number;
  fill: boolean;
  /** 序号用 */
  n?: number;
}

export function annNewOp(tool: AnnTool, p: AnnPt, cfg: AnnCfg): AnnOp | null {
  const { color, w, fill } = cfg;
  switch (tool) {
    case "rect":
    case "ellipse":
      return { t: tool, a: p, b: p, color, w: Math.max(0.6, w), fill };
    case "line":
      return { t: "line", a: p, b: p, color, w: Math.max(0.6, w) };
    case "arrow":
      return { t: "arrow", a: p, b: p, color, w: Math.max(0.6, w) };
    case "pen":
      return { t: "pen", pts: [p], color, w: Math.max(1, w), alpha: 1 };
    case "marker":
      /* 荧光笔：宽度 ×4、35% 不透明（截图那边的实测值），落笔就是一条可透视的粗带 */
      return { t: "marker", pts: [p], color, w: Math.max(4, w * 4), alpha: 0.35 };
    case "text":
      return { t: "text", p, s: "", color, size: Math.max(ANN_FONT_MIN, Math.round(w)) };
    case "badge":
      /* 序号圆跟着"字号"变大小（默认 18 → 半径 13，和截图那边一致）：
         粗细控件在序号工具下也有意义，不做成一个点了没反应的木疙瘩 */
      return {
        t: "badge",
        p,
        n: cfg.n ?? 1,
        color,
        r: Math.max(8, Math.min(30, Math.round(w * 0.75))),
      };
    case "mosaic":
      return { t: "mosaic", a: p, b: p, blur: Math.max(4, w * 2.5) };
    default:
      return null;
  }
}

/** 拖出来的框类够不够大（太小的当成"手抖"，不落层） */
export function annTooSmall(op: AnnOp): boolean {
  switch (op.t) {
    case "pen":
    case "marker":
      return op.pts.length < 2;
    case "text":
    case "badge":
      return false;
    default:
      return Math.abs(op.b.x - op.a.x) < 3 && Math.abs(op.b.y - op.a.y) < 3;
  }
}

/* ---------------- 画 ---------------- */

/** 底色上该用白字还是深字（序号圆里那个数字） */
function readableOn(hex: string): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return "#fff";
  const n = parseInt(m[1], 16);
  const r = (n >> 16) & 255;
  const g = (n >> 8) & 255;
  const b = n & 255;
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return lum > 0.62 ? "#141412" : "#fff";
}

/**
 * 把一条几何画到 ctx 的**当前用户空间**里。
 * 调用方负责先把变换设成"这条几何自己的局部坐标"（层画布先 scale 再 translate）。
 * 马赛克不在这里画 —— 它是一层 DOM（见 `Board.tsx`）。
 */
export function annDraw(ctx: CanvasRenderingContext2D, op: AnnOp) {
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
      /* 填充 20%：不遮住底下的图（和截图那边同一条规则） */
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
      /* 细长箭杆 + 实心三角头（头长 = 线宽 × 4，和截图一致） */
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
      /* 描一层半透明暗边：白字压在浅色图上也能读 */
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
    case "mosaic":
      /* 有意留空：马赛克是 DOM 上的 backdrop-filter 层（见 Board.tsx 的说明） */
      break;
  }
  ctx.restore();
}

/* ---------------- 落库 / 读回 ---------------- */

/** 画布坐标的几何 → 局部坐标的 payload 字符串（建层时算一次，之后永不变） */
export function annEncode(op: AnnOp, box: AnnBox): string {
  return JSON.stringify(annShift(op, -box.x, -box.y));
}

/** 库里的一行 → 运行时层。坏数据（别的版本 / 手工改坏的 JSON）直接丢掉，
 *  不能让一块板因为一层坏标注整个打不开。 */
export function annDecode(
  row: { aid: number; z: number; kind: string; payload: string; x: number; y: number; w: number; h: number }
): AnnLayer | null {
  let op: AnnOp;
  try {
    op = JSON.parse(row.payload) as AnnOp;
  } catch {
    return null;
  }
  if (!op || typeof op !== "object" || !("t" in op)) return null;
  const tool = ANN_TOOLS.includes(op.t as AnnTool) ? (op.t as AnnTool) : null;
  if (!tool) return null;
  return {
    aid: row.aid,
    z: row.z,
    kind: tool,
    x: row.x,
    y: row.y,
    w: Math.max(1, row.w),
    h: Math.max(1, row.h),
    op,
  };
}
