import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { source } from "../data/source";
import { ContextMenu, type CtxItem } from "./ContextMenu";

/**
 * 无边框看图窗口（UI 里叫「无边框看图」，也就是 owner 说的「贴图」——同一件事）。
 *
 * 形态照果核看图的「无边框模式」：**窗口就是那张图** —— 没有标题栏、没有边框、没有工具条，
 * 尺寸由图片比例决定（后端算，长边默认 342 DIP）。但它比果核多一层用法：
 * **它是贴图板**，想贴几张贴几张，关软件/重启电脑后没手动关的会自己回来（后端 `peeks.json`）。
 *
 * 手感约定（每一层都不许互相抢）：
 * - `滚轮`：连窗口一起缩放（锚定窗口中心）
 * - `Shift+滚轮`：**图内**缩放，锚定鼠标位置（看细节用，窗口不动）
 * - `Ctrl+滚轮`：整张贴图的不透明度
 * - 左键拖：移动窗口；中键拖：图内平移（放大后才有意义）
 * - 双击：撕掉这张贴图
 */
interface PeekItem {
  src: string;
  w: number;
  h: number;
  path: string;
  name: string;
  gid: number;
}

interface PeekBoot {
  id?: string;
  items?: PeekItem[];
  i?: number;
  opacity?: number;
}

const ZOOM_STEP = 1.09; // 和果核看图同一个步长（实测每格 ×1.09）
const OPACITY_STEP = 0.08;

function bootData(): { items: PeekItem[]; i: number; opacity: number } {
  const b = (window as unknown as { __AM_PEEK__?: PeekBoot }).__AM_PEEK__ ?? {};
  const items = (b.items ?? []).filter((x) => !!x?.src);
  const i = Math.max(0, Math.min(Number(b.i ?? 0) || 0, Math.max(0, items.length - 1)));
  const op = typeof b.opacity === "number" ? b.opacity : 1;
  return { items, i, opacity: Math.min(1, Math.max(0.15, op)) };
}

/** 16px 线性图标（只用一个，不值得为它引依赖） */
function Ico({ d, size = 12 }: { d: string[]; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {d.map((p, i) => (
        <path d={p} key={i} />
      ))}
    </svg>
  );
}
const ICO_CLOSE = ["M18 6 6 18", "M6 6l12 12"];

export function Peek() {
  const boot = useMemo(bootData, []);
  const [idx, setIdx] = useState(boot.i);
  const [pinned, setPinned] = useState(true); // 默认置顶（owner 要求）
  const [hover, setHover] = useState(false);
  const [err] = useState("");
  const [opac, setOpac] = useState(boot.opacity);
  /* 图内缩放（Shift+滚轮）：scale + 位移，`transform-origin: 0 0` 便于算锚点 */
  const [view, setView] = useState({ s: 1, x: 0, y: 0 });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  const root = useRef<HTMLDivElement>(null);
  const busy = useRef(false);
  const pan = useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);

  const cur = boot.items[idx];

  useEffect(() => {
    document.body.classList.add("peek-body");
    document.documentElement.classList.add("peek-body");
    return () => {
      document.body.classList.remove("peek-body");
      document.documentElement.classList.remove("peek-body");
    };
  }, []);

  /* 关窗：**从存档里删掉**（用户主动关的，下次开机不该回来） */
  const close = useCallback(() => {
    void source.peekClose();
  }, []);

  /* 换图：窗口按新图比例变，并把存档指向新文件（重启还原的是最后看到的那张） */
  const goto = useCallback(
    (next: number) => {
      const n = boot.items.length;
      if (!n) return;
      const k = (next + n) % n;
      const it = boot.items[k];
      setIdx(k);
      setView({ s: 1, x: 0, y: 0 });
      if (it.w && it.h) void source.peekRatio(it.w, it.h).catch(() => {});
      void source.peekTrack(it.path, it.gid).catch(() => {});
    },
    [boot.items]
  );

  /* 滚轮三分支：Shift=图内、Ctrl=不透明度、其余=连窗口一起缩放 */
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const up = e.deltaY < 0;

      if (e.shiftKey) {
        // 锚定鼠标：让指针底下那个像素在缩放前后**不动**
        const rect = el.getBoundingClientRect();
        const px = e.clientX - rect.left;
        const py = e.clientY - rect.top;
        setView((v) => {
          const s2 = Math.min(12, Math.max(1, v.s * (up ? ZOOM_STEP : 1 / ZOOM_STEP)));
          if (s2 === 1) return { s: 1, x: 0, y: 0 };
          const k = s2 / v.s;
          return { s: s2, x: px - (px - v.x) * k, y: py - (py - v.y) * k };
        });
        return;
      }

      if (e.ctrlKey) {
        setOpac((o) => {
          const v = Math.min(1, Math.max(0.15, o + (up ? OPACITY_STEP : -OPACITY_STEP)));
          void source.peekOpacity(v).catch(() => {});
          return v;
        });
        return;
      }

      if (busy.current) return;
      busy.current = true;
      void source
        .peekZoom(up ? ZOOM_STEP : 1 / ZOOM_STEP)
        .catch(() => {})
        .finally(() => {
          busy.current = false;
        });
    };
    // 原生 passive:false（React 的 onWheel 在 WebView2 上可能是 passive，preventDefault 会失效）
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  /* 键盘 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      if ((e.ctrlKey || e.metaKey) && k === "c") {
        e.preventDefault();
        if (cur) void source.peekCopyFile([cur.path]).catch(() => {});
        return;
      }
      if ((e.ctrlKey || e.metaKey) && k === "s") {
        e.preventDefault();
        if (cur) void source.saveAs(cur.path).catch(() => {});
        return;
      }
      if (e.key === "Escape") {
        if (menu) {
          setMenu(null);
          return;
        }
        if (view.s > 1) {
          setView({ s: 1, x: 0, y: 0 });
          return;
        }
        close();
        return;
      }
      if (e.key === "ArrowLeft") goto(idx - 1);
      if (e.key === "ArrowRight") goto(idx + 1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close, cur, goto, idx, menu, view.s]);

  /* 中键拖动 = 图内平移（放大后才有意义）。左键留给"拖窗口"（drag region）。 */
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (e.button !== 1 || view.s <= 1) return;
      e.preventDefault();
      pan.current = { x: e.clientX, y: e.clientY, ox: view.x, oy: view.y };
    };
    const onMove = (e: MouseEvent) => {
      const p = pan.current;
      if (!p) return;
      setView((v) => ({ ...v, x: p.ox + (e.clientX - p.x), y: p.oy + (e.clientY - p.y) }));
    };
    const onUp = () => {
      pan.current = null;
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [view.s]);

  /* 右键菜单 */
  const menuItems: CtxItem[] = [
    {
      label: "复制原图文件",
      icon: "copy",
      shortcut: "Ctrl+C",
      action: () => cur && void source.peekCopyFile([cur.path]).catch(() => {}),
    },
    {
      label: "复制当前显示尺寸",
      icon: "copy",
      action: () => {
        // 屏幕上真实占用的像素（不是图片原始尺寸）—— 排版截图时要用的是这个数
        const w = Math.round(window.innerWidth * window.devicePixelRatio);
        const h = Math.round(window.innerHeight * window.devicePixelRatio);
        void source.clipText(`${w}×${h}`).catch(() => {});
      },
    },
    {
      label: "保存到…",
      icon: "reveal",
      shortcut: "Ctrl+S",
      action: () => cur && void source.saveAs(cur.path).catch(() => {}),
    },
    { kind: "separator" },
    {
      label: `${pinned ? "✓ " : ""}窗口置顶`,
      icon: "star",
      action: () => void source.peekTop(!pinned).then(setPinned).catch(() => {}),
    },
    {
      label: "还原图内缩放",
      icon: "search",
      disabled: view.s <= 1,
      action: () => setView({ s: 1, x: 0, y: 0 }),
    },
    { kind: "separator" },
    { label: "关闭这扇无边框看图", icon: "close", shortcut: "Esc", danger: true, action: close },
  ];

  const px = cur?.src ?? "";

  return (
    <div
      ref={root}
      className="peek-root"
      data-tauri-drag-region
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onDoubleClick={close}
      onContextMenu={(e) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
      }}
    >
      {cur && !err && (
        <img
          className="peek-img"
          src={px}
          alt=""
          draggable={false}
          data-tauri-drag-region
          style={{
            opacity: opac,
            transform:
              view.s > 1 ? `translate(${view.x}px, ${view.y}px) scale(${view.s})` : undefined,
            transformOrigin: "0 0",
            cursor: view.s > 1 ? "grab" : undefined,
          }}
        />
      )}

      {err && <div className="peek-err">{err}</div>}

      {cur && (
        <div className={`peek-meta ${hover ? "on" : ""}`} data-tauri-drag-region>
          <span className="mono peek-name">{cur.name}</span>
          {cur.w > 0 && (
            <span className="mono peek-dim">
              {cur.w}×{cur.h}
            </span>
          )}
          {view.s > 1 && <span className="mono peek-dim">{Math.round(view.s * 100)}%</span>}
          {opac < 0.999 && <span className="mono peek-dim">{Math.round(opac * 100)}%</span>}
        </div>
      )}

      {/* 只剩一个关闭键（置顶/全屏图标按 owner 要求撤掉，置顶默认开） */}
      <div className={`peek-tools ${hover ? "on" : ""}`}>
        <button className="peek-btn" title="关闭（Esc / 双击）" onClick={close}>
          <Ico d={ICO_CLOSE} />
        </button>
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
