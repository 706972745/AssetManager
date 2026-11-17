import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { source } from "../data/source";
import type { Asset } from "../types";

/**
 * 无边框看图窗口（`#/peek?ids=1,2,3&i=0`）。
 *
 * 形态照果核看图的「无边框模式」：**窗口就是那张图** —— 没有标题栏、没有边框、
 * 没有工具条，窗口尺寸由图片比例决定（后端算，长边 342 DIP 起）。
 *
 * 三条手感必须对齐（少一条就不是那个东西了）：
 * 1. **滚轮 = 连窗口一起缩放**，锚定窗口中心。不是 CSS transform —— 那样窗口边缘会
 *    脱离图片，截图时留一圈透明，穿帮；
 * 2. **窗口任意位置都能拖**（`data-tauri-drag-region`），不需要找标题栏；
 * 3. 鼠标进窗口才浮出小按钮（置顶 / 关闭），静止时完全干净。
 *
 * 为什么值得单独做一扇窗：owner 要拿它**配合屏幕截图**用 —— 把参考图贴在角落、
 * 缩到刚好不挡工作区，然后截屏。主窗口做不到这件事（它有侧栏、顶栏、详情栏）。
 */
const ZOOM_STEP = 1.09; // 和果核看图同一个步长（实测每格 ×1.09）

/** 启动数据由 Rust 的 initialization_script 注入（`window.__AM_PEEK__`）。 */
function bootData(): { ids: number[]; i: number } {
  const b = (window as unknown as { __AM_PEEK__?: { ids?: number[]; i?: number } }).__AM_PEEK__;
  const ids = (b?.ids ?? []).filter((v) => Number.isFinite(v) && v > 0);
  const i = Math.max(0, Math.min(Number(b?.i ?? 0) || 0, Math.max(0, ids.length - 1)));
  return { ids, i };
}

/** 16px 线性图标（照 lucide 的路径，不额外引依赖：这里只要三个） */
function Ico({ d, size = 12 }: { d: string[]; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {d.map((p, i) => (
        <path d={p} key={i} />
      ))}
    </svg>
  );
}

const ICO_PIN = [
  "M12 17v5",
  "M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z",
];
const ICO_FULL = [
  "M8 3H5a2 2 0 0 0-2 2v3",
  "M21 8V5a2 2 0 0 0-2-2h-3",
  "M3 16v3a2 2 0 0 0 2 2h3",
  "M16 21h3a2 2 0 0 0 2-2v-3",
];
const ICO_CLOSE = ["M18 6 6 18", "M6 6l12 12"];

export function Peek() {
  const boot = useMemo(bootData, []);
  const [list, setList] = useState<Asset[]>([]);
  const [idx, setIdx] = useState(boot.i);
  const [pinned, setPinned] = useState(false);
  const [hover, setHover] = useState(false);
  const [err, setErr] = useState("");
  /** Ctrl+滚轮的"图内缩放"（窗口不变，超出部分裁掉） */
  const [scale, setScale] = useState(1);
  const root = useRef<HTMLDivElement>(null);
  const busy = useRef(false);

  /* 画布底色：图没铺满的几个像素（比例取整）用近黑垫底，不留白边。 */
  useEffect(() => {
    document.body.classList.add("peek-body");
    document.documentElement.classList.add("peek-body");
    return () => {
      document.body.classList.remove("peek-body");
      document.documentElement.classList.remove("peek-body");
    };
  }, []);

  useEffect(() => {
    if (!boot.ids.length) {
      setErr("没有可看的素材");
      return;
    }
    let dead = false;
    void source
      .assetsByIds(boot.ids)
      .then((rows) => {
        if (dead) return;
        // 后端可能漏掉已删除的 id：按 ids 顺序重整一遍，宁可少一张也不串位
        const byId = new Map(rows.map((a) => [a.id, a]));
        const ordered = boot.ids.map((id) => byId.get(id)).filter((a): a is Asset => !!a);
        setList(ordered);
        if (!ordered.length) setErr("这些素材都不在库里了");
      })
      .catch((e) => !dead && setErr(String(e)));
    return () => {
      dead = true;
    };
  }, [boot.ids]);

  const cur = list[idx];

  /* 关窗前记下当前长边：下次开同一张图不会又跳回 342 DIP */
  const rememberSize = useCallback(() => {
    try {
      const long = Math.round(Math.max(window.innerWidth, window.innerHeight));
      if (long > 0) localStorage.setItem("am.peek.longEdge", String(long));
    } catch {
      // localStorage 不可用就算了，不值得为它弹错
    }
  }, []);


  /* 换图：窗口跟着新图比例变（长边不变、中心不动）—— 否则 16:9 切到 3:4 会变形 */
  const goto = useCallback(
    (next: number) => {
      if (!list.length) return;
      const n = (next + list.length) % list.length;
      setIdx(n);
      setScale(1);
      const a = list[n];
      if (a?.width && a?.height) void source.peekRatio(a.width, a.height).catch(() => {});
    },
    [list]
  );

  /* 滚轮：默认缩放窗口；Ctrl+滚轮缩放图内容（图不动窗口，超出裁切） */
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const up = e.deltaY < 0;
      if (e.ctrlKey) {
        setScale((s) => Math.min(8, Math.max(1, s * (up ? ZOOM_STEP : 1 / ZOOM_STEP))));
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
    // 原生 passive:false —— React 的 onWheel 在部分 WebView2 上是 passive，
    // preventDefault 会失效，整个窗口跟着滚（踩过，见 WORKLOG §10.5 #28）
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      switch (e.key) {
        case "Escape":
          if (scale > 1) {
            setScale(1);
            return;
          }
          rememberSize();
          void source.peekClose();
          break;
        case "ArrowLeft":
          goto(idx - 1);
          break;
        case "ArrowRight":
          goto(idx + 1);
          break;
        case "F11":
          e.preventDefault();
          void source.peekFullscreen(true);
          break;
        case "t":
        case "T":
          if (e.altKey) {
            e.preventDefault();
            void source.peekTop(!pinned).then(setPinned).catch(() => {});
          }
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [goto, idx, scale, pinned, rememberSize]);

  const px = cur ? source.thumb(cur, 1600) : "";

  return (
    <div
      ref={root}
      className="peek-root"
      data-tauri-drag-region
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      {cur && !err && (
        <img
          className="peek-img"
          src={px}
          alt=""
          draggable={false}
          data-tauri-drag-region
          style={{ transform: scale > 1 ? `scale(${scale})` : undefined }}
          onDoubleClick={() => setScale((s) => (s > 1 ? 1 : 2))}
        />
      )}

      {err && <div className="peek-err">{err}</div>}

      {/* 底部左：序号 / 总数 · 尺寸。静止时半透明，不抢图。 */}
      {cur && (
        <div className={`peek-meta ${hover ? "on" : ""}`} data-tauri-drag-region>
          <span className="mono">
            {idx + 1} / {list.length}
          </span>
          {cur.width && cur.height && (
            <span className="mono peek-dim">
              {cur.width}×{cur.height}
            </span>
          )}
          {scale > 1 && <span className="mono peek-dim">{Math.round(scale * 100)}%</span>}
        </div>
      )}

      {/* 右上：置顶 / 全屏 / 关闭。只有鼠标进来才出现。 */}
      <div className={`peek-tools ${hover ? "on" : ""}`}>
        <button
          className={`peek-btn ${pinned ? "on" : ""}`}
          title={pinned ? "取消置顶（Alt+T）" : "窗口置顶（Alt+T）"}
          onClick={() => void source.peekTop(!pinned).then(setPinned).catch(() => {})}
        >
          <Ico d={ICO_PIN} />
        </button>
        <button
          className="peek-btn"
          title="全屏（F11）"
          onClick={() => void source.peekFullscreen(true).catch(() => {})}
        >
          <Ico d={ICO_FULL} />
        </button>
        <button
          className="peek-btn"
          title="关闭（Esc）"
          onClick={() => {
            rememberSize();
            void source.peekClose();
          }}
        >
          <Ico d={ICO_CLOSE} />
        </button>
      </div>
    </div>
  );
}
