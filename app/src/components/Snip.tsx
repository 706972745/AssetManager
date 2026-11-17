import { useCallback, useEffect, useRef, useState } from "react";
import { source } from "../data/source";

/**
 * 截图覆盖层（F1 / Alt+F1）。
 *
 * 做法照 Snipaste：**先把整块屏幕冻住**（后端 `BitBlt` 抓完再开这扇窗），
 * 用户在这张静态图上拖一个框，框外压暗、框内原亮度 —— 这样"选什么"和"截到什么"
 * 永远是同一张图（不会因为底下有动画而错位）。
 *
 * 窗口盖住整个虚拟桌面，坐标换算只有一个系数：`物理像素 = CSS 像素 × scale`。
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

export function Snip() {
  const boot = bootData();
  const [rect, setRect] = useState<Rect | null>(null);
  const [dragFrom, setDragFrom] = useState<{ x: number; y: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const cur = useRef<Rect | null>(null);
  cur.current = rect;

  useEffect(() => {
    document.body.classList.add("snip-body");
    document.documentElement.classList.add("snip-body");
    return () => {
      document.body.classList.remove("snip-body");
      document.documentElement.classList.remove("snip-body");
    };
  }, []);

  const commit = useCallback(
    async (saveAs: boolean) => {
      const r = cur.current;
      if (!r || r.w < 2 || r.h < 2 || busy) return;
      setBusy(true);
      setErr("");
      try {
        await source.snipCommit(r.x, r.y, r.w, r.h, saveAs);
      } catch (e) {
        // **不能默默取消**：窗口一闪就没了、什么都没留下，用户只会以为软件坏了。
        // 留在覆盖层上把原因写出来（后端已经把细节记进 runtime.log）
        setErr(String(e));
      } finally {
        setBusy(false);
      }
    },
    [busy]
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        void source.snipCancel();
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        void commit(false);
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void commit(true);
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "c") {
        e.preventDefault();
        void commit(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [commit]);

  if (!boot) {
    return <div className="snip-miss">截图数据丢失，请重新按一次快捷键</div>;
  }

  const down = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const p = { x: e.clientX, y: e.clientY };
    setDragFrom(p);
    setRect({ x: p.x, y: p.y, w: 0, h: 0 });
  };
  const move = (e: React.MouseEvent) => {
    if (!dragFrom) return;
    const x = Math.min(dragFrom.x, e.clientX);
    const y = Math.min(dragFrom.y, e.clientY);
    setRect({ x, y, w: Math.abs(e.clientX - dragFrom.x), h: Math.abs(e.clientY - dragFrom.y) });
  };
  const up = () => {
    if (!dragFrom) return;
    setDragFrom(null);
    const r = cur.current;
    // 点一下没拖动 = 重新开始选（不误触发"截一个 0×0"）
    if (!r || r.w < 4 || r.h < 4) {
      setRect(null);
      return;
    }
    // Alt+F1（mode=copy）：松手就复制走，不再要一次确认
    if (boot.mode === "copy") void commit(false);
  };

  return (
    <div
      className="snip-root"
      onMouseDown={down}
      onMouseMove={move}
      onMouseUp={up}
      onDoubleClick={() => void commit(false)}
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
          <div
            className="snip-size mono"
            style={{ left: rect.x, top: rect.y > 26 ? rect.y - 24 : rect.y + rect.h + 6 }}
          >
            {Math.round(rect.w * boot.scale)} × {Math.round(rect.h * boot.scale)}
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

      {err && <div className="snip-err mono">{err}</div>}

      {!rect && (
        <div className="snip-hint mono">
          拖动选择区域 · Enter / Ctrl+S 保存 · Ctrl+C 复制 · Esc 取消
        </div>
      )}
    </div>
  );
}
