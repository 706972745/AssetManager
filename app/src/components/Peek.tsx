import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { onAppEvent, source } from "../data/source";
import { loadPrefs } from "../types";
import { ContextMenu, type CtxItem } from "./ContextMenu";

/**
 * 贴图窗口（UI 里叫「贴图」，也就是 owner 说的「贴图」——同一件事）。
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
 *
 * **窗口里除了图什么都没有**（owner 2026-11-17：右上角 X、底部信息条全撤掉）——
 * 关闭走双击 / Esc / 右键菜单。同理窗口本身必须真透明，否则 Ctrl+滚轮降不透明度时
 * 露出来的是窗口自己的深色底（看起来就是"变黑"，而不是看到背后的东西）。
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
  theme?: "light" | "dark";
  /** 「蓝色边框」的全局状态（后端持有，建窗时注入；默认关） */
  frame?: boolean;
}

const ZOOM_STEP = 1.09; // 和果核看图同一个步长（实测每格 ×1.09）
const OPACITY_STEP = 0.08;

function bootData(): {
  items: PeekItem[];
  i: number;
  opacity: number;
  theme: "light" | "dark";
  frame: boolean;
} {
  const b = (window as unknown as { __AM_PEEK__?: PeekBoot }).__AM_PEEK__ ?? {};
  const items = (b.items ?? []).filter((x) => !!x?.src);
  const i = Math.max(0, Math.min(Number(b.i ?? 0) || 0, Math.max(0, items.length - 1)));
  const op = typeof b.opacity === "number" ? b.opacity : 1;
  /* 主题以后端注入的为准（主窗推过去的），没有才退回本地偏好 ——
     贴图是独立 webview，"设置里的主题"在主窗那侧的 localStorage 里，不保证同分区。 */
  const pref = loadPrefs().theme;
  const theme: "light" | "dark" =
    b.theme === "light" || b.theme === "dark"
      ? b.theme
      : pref === "light"
        ? "light"
        : pref === "dark"
          ? "dark"
          : (window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true)
            ? "dark"
            : "light";
  return {
    items,
    i,
    opacity: Math.min(1, Math.max(0.15, op)),
    theme,
    /* 蓝色边框：后端是唯一事实来源，默认关 */
    frame: b.frame === true,
  };
}

/** 16px 线性图标（只用一个，不值得为它引依赖） */
export function Peek() {
  const boot = useMemo(bootData, []);
  const [idx, setIdx] = useState(boot.i);
  const [pinned, setPinned] = useState(true); // 默认置顶（owner 要求）
  const [err] = useState("");
  const [opac, setOpac] = useState(boot.opacity);
  /* 图内缩放（Shift+滚轮）：scale + 位移，`transform-origin: 0 0` 便于算锚点 */
  const [view, setView] = useState({ s: 1, x: 0, y: 0 });
  /* 旋转 / 镜像（0.1.18，P2.7）：只影响"看"，不动文件、不改存档 ——
     贴图是临时视图，关掉再开回来还是原样。 */
  const [rot, setRot] = useState(0);
  const [flip, setFlip] = useState({ h: false, v: false });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  /* 贴图窗没有主窗那套 toast，用一条自己会消失的提示（存库/存库失败都要有反馈） */
  const [flashMsg, setFlashMsg] = useState("");
  const flash = useCallback((m: string) => {
    setFlashMsg(m);
    window.setTimeout(() => setFlashMsg(""), 2200);
  }, []);
  /* 蓝色边框：**全局开关**，状态在后端（`PEEK_FRAME`），不是每个窗口各记一份。
     为什么改（owner 2026-11-18 连报两次"关不掉"）：贴图是多窗口，前端各存 localStorage 时，
     关掉手上这张不影响已经贴着的其他张，新贴的还按默认值开 —— 看上去就是开关没生效。
     现在默认**关**（大多数人不需要那圈线），切换时调命令，后端广播给所有贴图窗口。 */
  const [frame, setFrame] = useState(() => boot.frame === true);
  const toggleFrame = useCallback(() => {
    setFrame((v) => {
      const next = !v;
      void source.peekSetFrame(next);
      flash(next ? "蓝色边框：开" : "蓝色边框：关");
      return next;
    });
    /* 兜底：所有窗口（含本窗）都会收到 `peek:frame`，正常路径由事件驱动更新；
       这里先本地改一次是为了手感跟手，事件回来时值相同，不会闪。 */
  }, [flash]);

  /* 其他贴图窗口切了这个开关 → 本窗跟着变（全局偏好的唯一同步方式） */
  useEffect(
    () =>
      onAppEvent("peek:frame", (v) => {
        setFrame(v === true);
      }),
    []
  );

  const root = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const busy = useRef(false);
  const pan = useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);
  /* 窗口建出来是**不可见**的（后端 `.visible(false)`），首帧真画上去了才让它露面 ——
     否则 WebView2 会先闪一下自己的默认底色，中键贴图时那一闪特别难看。 */
  const shown = useRef(false);
  const markReady = useCallback(() => {
    if (shown.current) return;
    shown.current = true;
    // 双 rAF：等这一帧真的合成完，再让窗口显示
    requestAnimationFrame(() =>
      requestAnimationFrame(() => void source.peekReady().catch(() => {}))
    );
  }, []);

  const cur = boot.items[idx];

  /* 图可能已经在缓存里（onLoad 不会再触发），所以再挂一道兜底 */
  useEffect(() => {
    const t = setTimeout(markReady, 400);
    return () => clearTimeout(t);
  }, [markReady]);

  /* 主题：贴图是独立 webview，不会自动继承主窗的 data-theme。
     以前这里没人设，于是浅色主题下贴图的右键菜单仍是深色（owner 2026-11-18 报的第 6 条）。
     开窗时读一次 prefs 就够 —— 用户在设置里换主题后新开的贴图会跟上。 */
  useEffect(() => {
    document.documentElement.dataset.theme = boot.theme === "light" ? "light" : "dark";
  }, [boot.theme]);

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

  /**
   * 转 90° / 270° 时窗口的长宽要跟着换（不然图会转出窗口、留两条黑边）。
   * 换比例走 `peekRatio`：长边不变、只按新比例重算短边。
   */
  const applyRot = useCallback(
    (next: number) => {
      setRot(next);
      const it = boot.items[idx];
      if (!it?.w || !it?.h) return;
      const swap = next === 90 || next === 270;
      void source.peekRatio(swap ? it.h : it.w, swap ? it.w : it.h).catch(() => {});
    },
    [boot.items, idx]
  );

  /* 换图：窗口按新图比例变，并把存档指向新文件（重启还原的是最后看到的那张） */
  const goto = useCallback(
    (next: number) => {
      const n = boot.items.length;
      if (!n) return;
      const k = (next + n) % n;
      const it = boot.items[k];
      setIdx(k);
      setView({ s: 1, x: 0, y: 0 });
      setRot(0);
      setFlip({ h: false, v: false });
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
        // 锚定鼠标：让指针底下那个像素在缩放前后**不动**。
        // 用 **img 的** rect，不是 root 的 —— root 里有 16px 的发光的边距，
        // 拿 root 算会让锚点整体偏 16px。
        const rect = (imgRef.current ?? el).getBoundingClientRect();
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
      if (!e.ctrlKey && !e.metaKey) {
        if (k === "r") {
          e.preventDefault();
          applyRot((rot + 90) % 360);
          return;
        }
        if (k === "h") {
          e.preventDefault();
          setFlip((f) => ({ ...f, h: !f.h }));
          return;
        }
        if (k === "v") {
          e.preventDefault();
          setFlip((f) => ({ ...f, v: !f.v }));
          return;
        }
        if (k === "0") {
          e.preventDefault();
          setRot(0);
          setFlip({ h: false, v: false });
          setView({ s: 1, x: 0, y: 0 });
          return;
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close, cur, goto, idx, menu, view.s, rot, applyRot]);

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
      label: "以原始尺寸存到截图库",
      icon: "save",
      action: () => {
        if (!cur) return;
        void source
          .peekSaveLib(cur.path)
          .then(() => flash("已存到截图库（screenshots）"))
          .catch((e: unknown) => flash(String((e as Error)?.message ?? e)));
      },
    },
    {
      label: "另存到…",
      icon: "reveal",
      shortcut: "Ctrl+S",
      action: () => cur && void source.saveAs(cur.path).catch(() => {}),
    },
    { kind: "separator" },
    {
      label: "截图",
      icon: "image",
      shortcut: "F1",
      action: () => void source.snipStart().catch(() => {}),
    },
    { kind: "separator" },
    {
      label: `${pinned ? "✓ " : ""}窗口置顶`,
      icon: "star",
      action: () => void source.peekTop(!pinned).then(setPinned).catch(() => {}),
    },
    {
      label: `${frame ? "✓ " : ""}蓝色边框`,
      icon: "image",
      action: toggleFrame,
    },
    { kind: "separator" },
    {
      label: `顺时针转 90°${rot ? `（已转 ${rot}°）` : ""}`,
      icon: "play",
      shortcut: "R",
      action: () => applyRot((rot + 90) % 360),
    },
    {
      label: `${flip.h ? "✓ " : ""}左右镜像`,
      icon: "copy",
      shortcut: "H",
      action: () => setFlip((f) => ({ ...f, h: !f.h })),
    },
    {
      label: `${flip.v ? "✓ " : ""}上下镜像`,
      icon: "copy",
      shortcut: "V",
      action: () => setFlip((f) => ({ ...f, v: !f.v })),
    },
    {
      label: "还原旋转 / 镜像 / 缩放",
      icon: "search",
      shortcut: "0",
      disabled: rot === 0 && !flip.h && !flip.v && view.s <= 1,
      action: () => {
        setRot(0);
        setFlip({ h: false, v: false });
        setView({ s: 1, x: 0, y: 0 });
      },
    },
    { kind: "separator" },
    { label: "关闭这个贴图", icon: "close", shortcut: "Esc", danger: true, action: close },
  ];

  const px = cur?.src ?? "";

  return (
    <div
      ref={root}
      className={frame ? "peek-root peek-framed" : "peek-root"}
      data-tauri-drag-region
      onDoubleClick={close}
      onContextMenu={(e) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
      }}
    >
      {cur && !err && (
        <img
          ref={imgRef}
          className="peek-img"
          src={px}
          alt=""
          draggable={false}
          data-tauri-drag-region
          onLoad={markReady}
          onError={markReady}
          style={{
            opacity: opac,
            /* 顺序：镜像 → 旋转 → 图内缩放位移。
               先镜像再旋转，「左右镜像」说的就是**屏幕上**的左右（符合直觉）。 */
            transform: (() => {
              const parts: string[] = [];
              if (view.s > 1) parts.push(`translate(${view.x}px, ${view.y}px) scale(${view.s})`);
              if (rot) parts.push(`rotate(${rot}deg)`);
              if (flip.h) parts.push("scaleX(-1)");
              if (flip.v) parts.push("scaleY(-1)");
              return parts.length ? parts.join(" ") : undefined;
            })(),
            transformOrigin: "0 0",
            cursor: view.s > 1 ? "grab" : undefined,
          }}
        />
      )}

      {err && <div className="peek-err">{err}</div>}

      {flashMsg && <div className="peek-flash">{flashMsg}</div>}

      {menu && (
        <ContextMenu
          state={{ x: menu.x, y: menu.y, items: menuItems }}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}
