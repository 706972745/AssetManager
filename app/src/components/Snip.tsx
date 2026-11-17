import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowUpRight,
  Check,
  Circle,
  Copy,
  Grid2x2,
  Highlighter,
  Pencil,
  Pin,
  Redo2,
  Save,
  Square,
  Type,
  Undo2,
  X,
} from "lucide-react";
import { source } from "../data/source";

/**
 * 截图覆盖层（F1 / Alt+F1）。
 *
 * 做法照 Snipaste：**先把整块屏幕冻住**（后端 BitBlt 抓完再开这扇窗），
 * 用户在这张静态图上拖一个框，框外压暗、框内原亮度 —— 这样"选什么"和"截到什么"
 * 永远是同一张图（不会因为底下有动画而错位）。
 *
 * 松手之后**不是立刻截完**，而是进入标注态（Snipaste 2.x 的工具栏，owner 2026-11-17 要求）：
 * 矩形 / 椭圆 / 箭头 / 画笔 / 记号笔 / 文字 / 马赛克，配撤销重做、复制、保存、贴图。
 *
 * 坐标有**两套**，不许混：
 * - 屏幕上量到的都是 **CSS 像素**（`rect`、鼠标位置）；
 * - canvas 内部一律用**物理像素**（= CSS × scale）。只有这样导出的图才和屏幕一样清晰，
 *   二倍屏上也不会糊。所以每次进 canvas 都要 `* scale`。
 *
 * 底图来自后端 `am_snip_crop` 的 base64（`data:` URL 不污染画布），
 * 这样 `toDataURL` 才能把标注一起导出来。
 */
interface SnipBoot {
  src: string;
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

type Tool = "rect" | "ellipse" | "arrow" | "pen" | "marker" | "text" | "mosaic";
type Action = "copy" | "save" | "saveas" | "peek";

/** 一条标注。坐标全是**物理像素**（相对选区左上角）。 */
type Op =
  | { t: "rect" | "ellipse" | "arrow"; a: P; b: P; color: string; w: number }
  | { t: "pen"; pts: P[]; color: string; w: number; alpha: number }
  | { t: "text"; p: P; s: string; color: string; size: number }
  | { t: "mosaic"; a: P; b: P };

const COLORS = ["#ff3b30", "#ffcc00", "#34c759", "#0a84ff", "#ffffff", "#1c1c1e"];

const TOOLS: { id: Tool; label: string; icon: typeof Square }[] = [
  { id: "rect", label: "矩形", icon: Square },
  { id: "ellipse", label: "椭圆", icon: Circle },
  { id: "arrow", label: "箭头", icon: ArrowUpRight },
  { id: "pen", label: "画笔", icon: Pencil },
  { id: "marker", label: "记号笔", icon: Highlighter },
  { id: "text", label: "文字", icon: Type },
  { id: "mosaic", label: "马赛克", icon: Grid2x2 },
];

export function Snip() {
  const boot = bootData();
  const scale = boot?.scale ?? 1;

  const [rect, setRect] = useState<Rect | null>(null);
  const [dragFrom, setDragFrom] = useState<P | null>(null);
  const [tool, setTool] = useState<Tool>("rect");
  const [color, setColor] = useState(COLORS[0]);
  const [palette, setPalette] = useState(false);
  const [ops, setOps] = useState<Op[]>([]);
  const [redo, setRedo] = useState<Op[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [textAt, setTextAt] = useState<P | null>(null); // 物理坐标
  const [text, setText] = useState("");

  const cur = useRef<Rect | null>(null);
  cur.current = rect;
  const opsRef = useRef<Op[]>([]);
  opsRef.current = ops;
  const draw = useRef<{ op: Op; from: P } | null>(null);

  const cv = useRef<HTMLCanvasElement>(null);
  const base = useRef<{ img: HTMLImageElement; key: string } | null>(null);
  const loading = useRef<Promise<void> | null>(null);
  const raf = useRef(0);

  useEffect(() => {
    document.body.classList.add("snip-body");
    document.documentElement.classList.add("snip-body");
    return () => {
      document.body.classList.remove("snip-body");
      document.documentElement.classList.remove("snip-body");
    };
  }, []);

  /* ---------------- 画布：底图 + 标注 ---------------- */

  const paint = useCallback(() => {
    const c = cv.current;
    const r = cur.current;
    if (!c || !r) return;
    const ctx = c.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, c.width, c.height);
    const b = base.current;
    if (b) ctx.drawImage(b.img, 0, 0, c.width, c.height);
    for (const op of opsRef.current) drawOp(ctx, op, c, scale);
    if (draw.current) drawOp(ctx, draw.current.op, c, scale);
  }, [scale]);

  const schedule = useCallback(() => {
    if (raf.current) return;
    raf.current = requestAnimationFrame(() => {
      raf.current = 0;
      paint();
    });
  }, [paint]);

  /** 把当前选区的像素从后端裁出来当底图（同一选区只取一次） */
  const ensureBase = useCallback(async () => {
    const r = cur.current;
    if (!r) throw new Error("还没选区");
    const key = `${r.x},${r.y},${r.w},${r.h}`;
    if (base.current?.key === key) return;
    if (loading.current) await loading.current;
    if (base.current?.key === key) return;
    const run = (async () => {
      const [url] = await source.snipCrop(r.x, r.y, r.w, r.h, scale);
      const img = new Image();
      img.src = url;
      await img.decode();
      base.current = { img, key };
      paint();
    })();
    loading.current = run;
    try {
      await run;
    } finally {
      loading.current = null;
    }
  }, [scale, paint]);

  /* 选区定了（松手）才拉底图：拖动过程中 rect 每帧都在变，
     要是跟着每次变都去裁剪一遍，后端会被刷成筛子。
     拖动时也不渲染 canvas —— 那时候没有底图，画出来是块空白。 */
  useEffect(() => {
    const c = cv.current;
    if (!c || !rect) return;
    const pw = Math.max(1, Math.round(rect.w * scale));
    const ph = Math.max(1, Math.round(rect.h * scale));
    if (c.width !== pw || c.height !== ph) {
      c.width = pw;
      c.height = ph;
    }
    paint();
    if (!dragFrom) void ensureBase().catch((e) => setErr(String(e)));
  }, [rect, dragFrom, scale, paint, ensureBase]);

  /* ---------------- 导出 ---------------- */

  const finish = useCallback(
    async (action: Action) => {
      if (busy) return;
      const r = cur.current;
      if (!r || r.w < 2 || r.h < 2) return;
      setBusy(true);
      setErr("");
      try {
        await ensureBase();
        paint();
        const c = cv.current;
        if (!c) throw new Error("画布还没准备好");
        await source.snipFinish(c.toDataURL("image/png"), action);
      } catch (e) {
        // **不能默默取消**：窗口一闪就没了、什么都没留下，用户只会以为软件坏了
        setErr(String(e));
      } finally {
        setBusy(false);
      }
    },
    [busy, ensureBase, paint]
  );

  /* ---------------- 键盘 ---------------- */

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      const mod = e.ctrlKey || e.metaKey;
      if (k === "escape") {
        e.preventDefault();
        if (textAt) {
          setTextAt(null);
          setText("");
          return;
        }
        if (palette) {
          setPalette(false);
          return;
        }
        void source.snipCancel();
        return;
      }
      if (mod && k === "z") {
        e.preventDefault();
        if (e.shiftKey) {
          setRedo((rd) => {
            if (!rd.length) return rd;
            const op = rd[rd.length - 1];
            setOps((os) => [...os, op]);
            return rd.slice(0, -1);
          });
        } else {
          setOps((os) => {
            if (!os.length) return os;
            const op = os[os.length - 1];
            setRedo((rd) => [...rd, op]);
            return os.slice(0, -1);
          });
        }
        return;
      }
      if (mod && k === "c") {
        e.preventDefault();
        void finish("copy");
        return;
      }
      if (mod && k === "s") {
        e.preventDefault();
        void finish("saveas");
        return;
      }
      if (k === "enter") {
        e.preventDefault();
        void finish("save");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [finish, palette, textAt]);

  useEffect(() => {
    schedule();
  }, [ops, rect, schedule]);

  if (!boot) {
    return <div className="snip-miss">截图数据丢失，请重新按一次快捷键</div>;
  }

  /* ---------------- 交互 ---------------- */

  /** 鼠标 CSS 坐标 → 选区内的物理坐标 */
  const toLocal = (e: { clientX: number; clientY: number }): P | null => {
    const r = cur.current;
    if (!r) return null;
    return { x: (e.clientX - r.x) * scale, y: (e.clientY - r.y) * scale };
  };

  const inside = (e: { clientX: number; clientY: number }): boolean => {
    const r = cur.current;
    if (!r) return false;
    return (
      e.clientX >= r.x && e.clientX <= r.x + r.w && e.clientY >= r.y && e.clientY <= r.y + r.h
    );
  };

  const commitOp = (op: Op) => {
    setOps((os) => [...os, op]);
    setRedo([]);
  };

  const down = (e: React.MouseEvent) => {
    if (e.button === 1) {
      // 中键：直接贴到屏幕上（Snipaste 的贴图）
      e.preventDefault();
      if (cur.current) void finish("peek");
      return;
    }
    if (e.button !== 0) return;
    const r = cur.current;

    if (!r) {
      const p = { x: e.clientX, y: e.clientY };
      setDragFrom(p);
      setRect({ x: p.x, y: p.y, w: 0, h: 0 });
      return;
    }

    // 已有选区：框内用当前工具画，框外 = 重新选
    if (!inside(e)) {
      const p = { x: e.clientX, y: e.clientY };
      base.current = null;
      setOps([]);
      setRedo([]);
      setTextAt(null);
      setDragFrom(p);
      setRect({ x: p.x, y: p.y, w: 0, h: 0 });
      return;
    }

    const p = toLocal(e);
    if (!p) return;
    if (tool === "text") {
      setTextAt(p);
      setText("");
      return;
    }
    const lw = lineWidth(tool, scale);
    if (tool === "rect" || tool === "ellipse" || tool === "arrow" || tool === "mosaic") {
      draw.current = {
        op:
          tool === "mosaic"
            ? { t: "mosaic", a: p, b: p }
            : { t: tool, a: p, b: p, color, w: lw },
        from: p,
      };
    } else {
      draw.current = {
        op: {
          t: "pen",
          pts: [p],
          color,
          w: lw,
          alpha: tool === "marker" ? 0.3 : 1,
        },
        from: p,
      };
    }
    schedule();
  };

  const move = (e: React.MouseEvent) => {
    if (dragFrom) {
      const x = Math.min(dragFrom.x, e.clientX);
      const y = Math.min(dragFrom.y, e.clientY);
      setRect({
        x,
        y,
        w: Math.abs(e.clientX - dragFrom.x),
        h: Math.abs(e.clientY - dragFrom.y),
      });
      return;
    }
    const d = draw.current;
    if (!d) return;
    const p = toLocal(e);
    if (!p) return;
    if (d.op.t === "pen") d.op = { ...d.op, pts: [...d.op.pts, p] };
    else if (d.op.t !== "text") d.op = { ...d.op, b: p };
    draw.current = { ...d, op: d.op };
    schedule();
  };

  const up = () => {
    if (dragFrom) {
      setDragFrom(null);
      const r = cur.current;
      // 点一下没拖动 = 重新开始选（不误触发"截一个 0×0"）
      if (!r || r.w < 4 || r.h < 4) {
        setRect(null);
        return;
      }
      // Alt+F1（mode=copy）：松手就复制走，不再要一次确认
      if (boot.mode === "copy") void finish("copy");
      return;
    }
    const d = draw.current;
    if (!d) return;
    draw.current = null;
    // 太小的手势当误触丢掉（一个 2px 的矩形什么都不像）
    const tooSmall =
      d.op.t === "pen"
        ? d.op.pts.length < 2
        : d.op.t !== "text"
          ? Math.abs(d.op.b.x - d.op.a.x) < 3 && Math.abs(d.op.b.y - d.op.a.y) < 3
          : false;
    if (!tooSmall) commitOp(d.op);
    schedule();
  };

  const submitText = () => {
    if (!textAt || !text.trim()) {
      setTextAt(null);
      setText("");
      return;
    }
    commitOp({
      t: "text",
      p: textAt,
      s: text,
      color,
      size: Math.round(18 * scale),
    });
    setTextAt(null);
    setText("");
  };

  /* ---------------- 渲染 ---------------- */

  const toolbar = rect && rect.w > 8 && rect.h > 8 ? barPos(rect) : null;

  return (
    <div
      className="snip-root"
      onMouseDown={down}
      onMouseMove={move}
      onMouseUp={up}
      onDoubleClick={() => void finish("copy")}
      onContextMenu={(e) => {
        e.preventDefault();
        void source.snipCancel();
      }}
    >
      <img className="snip-bg" src={boot.src} alt="" draggable={false} />

      {rect && rect.w > 0 && (
        <>
          {/* 选区：框外压暗靠一个超大 box-shadow，比铺四块遮罩省事且没有缝 */}
          <div
            className="snip-sel"
            style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
          />
          {/* 标注画布：CSS 尺寸 = 选区，像素尺寸 = 物理尺寸（高清）。
              只画在**已经定下来的**选区上（拖动中没有底图）。 */}
          {!dragFrom && (
            <canvas
              ref={cv}
              className="snip-canvas"
              style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
            />
          )}
          <div
            className="snip-size mono"
            style={{ left: rect.x, top: rect.y > 26 ? rect.y - 24 : rect.y + rect.h + 6 }}
          >
            {Math.round(rect.w * scale)} × {Math.round(rect.h * scale)}
          </div>
          {/* 四角把手（纯装饰：真正的取舍靠重新拖，不做逐边调整） */}
          {[
            [rect.x, rect.y],
            [rect.x + rect.w, rect.y],
            [rect.x, rect.y + rect.h],
            [rect.x + rect.w, rect.y + rect.h],
          ].map(([cx, cy], i) => (
            <span className="snip-dot" key={i} style={{ left: cx, top: cy }} />
          ))}
        </>
      )}

      {/* 工具栏（照 Snipaste 2.11.3：贴在选区下方，放不下就翻到上方） */}
      {toolbar && (
        <div
          className="snip-bar"
          style={{ left: toolbar.left, top: toolbar.top }}
          onMouseDown={(e) => e.stopPropagation()}
          onMouseMove={(e) => e.stopPropagation()}
          onMouseUp={(e) => e.stopPropagation()}
          onDoubleClick={(e) => e.stopPropagation()}
        >
          {TOOLS.map((t) => (
            <button
              key={t.id}
              className={`snip-tool ${tool === t.id ? "on" : ""}`}
              title={t.label}
              onClick={() => setTool(t.id)}
            >
              <t.icon size={15} strokeWidth={1.7} />
            </button>
          ))}
          <button
            className="snip-swatch"
            title="颜色"
            onClick={() => setPalette((v) => !v)}
            style={{ background: color }}
          />
          <span className="snip-bar-sep" />
          <button
            className="snip-tool"
            title="撤销（Ctrl+Z）"
            disabled={!ops.length}
            onClick={() => {
              setOps((os) => {
                if (!os.length) return os;
                setRedo((rd) => [...rd, os[os.length - 1]]);
                return os.slice(0, -1);
              });
            }}
          >
            <Undo2 size={15} strokeWidth={1.7} />
          </button>
          <button
            className="snip-tool"
            title="重做（Ctrl+Shift+Z）"
            disabled={!redo.length}
            onClick={() => {
              setRedo((rd) => {
                if (!rd.length) return rd;
                const op = rd[rd.length - 1];
                setOps((os) => [...os, op]);
                return rd.slice(0, -1);
              });
            }}
          >
            <Redo2 size={15} strokeWidth={1.7} />
          </button>
          <span className="snip-bar-sep" />
          <button
            className="snip-tool"
            title="复制到剪贴板（Ctrl+C / 双击）"
            onClick={() => void finish("copy")}
          >
            <Copy size={15} strokeWidth={1.7} />
          </button>
          <button
            className="snip-tool"
            title="保存到截图目录（Enter）"
            onClick={() => void finish("save")}
          >
            <Save size={15} strokeWidth={1.7} />
          </button>
          <button
            className="snip-tool"
            title="贴到屏幕（鼠标中键）"
            onClick={() => void finish("peek")}
          >
            <Pin size={15} strokeWidth={1.7} />
          </button>
          <button
            className="snip-tool"
            title="完成（Enter）"
            onClick={() => void finish("save")}
          >
            <Check size={15} strokeWidth={1.7} />
          </button>
          <button
            className="snip-tool danger"
            title="取消（Esc）"
            onClick={() => void source.snipCancel()}
          >
            <X size={15} strokeWidth={1.7} />
          </button>
        </div>
      )}

      {/* 调色板 */}
      {toolbar && palette && (
        <div
          className="snip-palette"
          style={{ left: toolbar.left, top: toolbar.top + 38 }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          {COLORS.map((c) => (
            <button
              key={c}
              className={color === c ? "on" : ""}
              style={{ background: c }}
              onClick={() => {
                setColor(c);
                setPalette(false);
              }}
            />
          ))}
        </div>
      )}

      {/* 文字输入 */}
      {textAt && rect && (
        <input
          className="snip-text mono"
          autoFocus
          value={text}
          style={{
            left: rect.x + textAt.x / scale,
            top: rect.y + textAt.y / scale,
            color,
          }}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") submitText();
            if (e.key === "Escape") {
              setTextAt(null);
              setText("");
            }
          }}
          onBlur={submitText}
        />
      )}

      {err && <div className="snip-err mono">{err}</div>}
      {busy && <div className="snip-busy mono">处理中…</div>}

      {!rect && (
        <div className="snip-hint mono">
          拖动选择区域 · 中键贴到屏幕 · Enter 保存 · Ctrl+C 复制 · Esc 取消
        </div>
      )}
    </div>
  );
}

/* ---------------- 绘制 ---------------- */

function lineWidth(tool: Tool, scale: number): number {
  if (tool === "marker") return Math.round(14 * scale);
  if (tool === "pen") return Math.round(2.5 * scale) || 1;
  return Math.round(3 * scale) || 1;
}

function drawOp(ctx: CanvasRenderingContext2D, op: Op, cv: HTMLCanvasElement, scale: number) {
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
  } else if (op.t === "arrow") {
    const { a, b } = op;
    const head = Math.max(9 * scale, op.w * 3.4);
    const ang = Math.atan2(b.y - a.y, b.x - a.x);
    ctx.strokeStyle = op.color;
    ctx.fillStyle = op.color;
    ctx.lineWidth = op.w;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x - Math.cos(ang) * head * 0.55, b.y - Math.sin(ang) * head * 0.55);
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(b.x, b.y);
    ctx.lineTo(b.x - Math.cos(ang - 0.42) * head, b.y - Math.sin(ang - 0.42) * head);
    ctx.lineTo(b.x - Math.cos(ang + 0.42) * head, b.y - Math.sin(ang + 0.42) * head);
    ctx.closePath();
    ctx.fill();
  } else if (op.t === "pen") {
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
    ctx.font = `600 ${op.size}px "Inter Variable", "Microsoft YaHei", sans-serif`;
    ctx.textBaseline = "top";
    // 描一层深色边：浅色底上的白字也读得出来
    ctx.lineWidth = Math.max(2, op.size / 9);
    ctx.strokeStyle = "rgba(0,0,0,0.55)";
    ctx.lineJoin = "round";
    ctx.strokeText(op.s, op.p.x, op.p.y);
    ctx.fillText(op.s, op.p.x, op.p.y);
  } else if (op.t === "mosaic") {
    const x0 = Math.max(0, Math.min(op.a.x, op.b.x));
    const y0 = Math.max(0, Math.min(op.a.y, op.b.y));
    const x1 = Math.min(cv.width, Math.max(op.a.x, op.b.x));
    const y1 = Math.min(cv.height, Math.max(op.a.y, op.b.y));
    const bw = x1 - x0;
    const bh = y1 - y0;
    if (bw < 1 || bh < 1) {
      ctx.restore();
      return;
    }
    const blk = Math.max(6, Math.round(10 * scale));
    // 一次取整块区域的像素，再按块取中心色 —— 逐块 getImageData 会慢到卡住
    const src = ctx.getImageData(x0, y0, bw, bh).data;
    for (let y = 0; y < bh; y += blk) {
      for (let x = 0; x < bw; x += blk) {
        const cx = Math.min(bw - 1, x + (blk >> 1));
        const cy = Math.min(bh - 1, y + (blk >> 1));
        const i = (cy * bw + cx) * 4;
        ctx.fillStyle = `rgb(${src[i]},${src[i + 1]},${src[i + 2]})`;
        ctx.fillRect(x0 + x, y0 + y, Math.min(blk, bw - x), Math.min(blk, bh - y));
      }
    }
  }
  ctx.restore();
}

/** 工具栏不越出窗口，也不盖住太多选区 */
function barPos(r: Rect): { left: number; top: number } {
  const W = 430; // 估计宽度，只用于夹紧
  const below = r.y + r.h + 10;
  const top = below + 36 < window.innerHeight ? below : Math.max(6, r.y - 44);
  const center = r.x + r.w / 2;
  const left = Math.min(Math.max(6, center - W / 2), Math.max(6, window.innerWidth - W - 6));
  return { left, top };
}
