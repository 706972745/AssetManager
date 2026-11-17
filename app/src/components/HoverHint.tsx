/**
 * 悬停说明气泡 —— 照 FontManager 的 `HoverHintHost` 抄的一套。
 *
 * 为什么不用原生 `title`：延迟由系统定（0.5~1.5 秒不定）、样式不可控（灰底黑字、
 * 还带系统边框），和面板里的字号/颜色对不上。详情面板的「位置」正是靠悬停
 * 来看**完整路径**的（图上只能看到省略号），所以这个气泡的观感必须和 FontManager
 * 一致 —— 同样的 1 秒延迟、同样的 e3 底 + 发丝线 + 12px 小字。
 *
 * 走**事件委托**而不是给每个元素包一层组件：全项目按钮很多，包组件会破坏布局动画、
 * 让 JSX 没法看。元素只要带 `data-hint-text="已经拼好的文案"` 就行。
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

/** 需求：悬停 1 秒（与 FontManager 同值） */
const DELAY = 1000;
/** 与锚点的间距 */
const GAP = 8;
/** 距窗口边缘的最小留白 */
const EDGE = 8;

const HINT_SELECTOR = "[data-hint-text]";

interface Shown {
  text: string;
  /** 触发那一刻的矩形：延迟 1 秒后按钮可能已经滚走了 */
  rect: { left: number; top: number; width: number; height: number };
}

function Bubble({ shown }: { shown: Shown }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const { rect } = shown;
    /* 优先放上方，但上方剩不下「气泡 + 呼吸」就翻到下方（顶栏按钮上方是标题栏，
       贴着放会压住标题栏） */
    const above = rect.top - GAP - h;
    const top =
      above >= EDGE + 24
        ? above
        : Math.min(rect.top + rect.height + GAP, window.innerHeight - h - EDGE);
    const left = Math.min(
      Math.max(EDGE, rect.left + rect.width / 2 - w / 2),
      Math.max(EDGE, window.innerWidth - w - EDGE),
    );
    setPos({ left, top });
  }, [shown]);

  return (
    <div
      ref={ref}
      /* 量到尺寸之前先放到屏幕外，避免第一帧闪在左上角 */
      className={`hover-hint${pos ? " on" : ""}`}
      style={pos ? { left: pos.left, top: pos.top } : { left: -9999, top: -9999 }}
      role="tooltip"
    >
      {shown.text}
    </div>
  );
}

export function HoverHintHost() {
  const [shown, setShown] = useState<Shown | null>(null);
  /* 回调里要立刻判断「已经显示了没」，state 读到的会是旧值 */
  const shownRef = useRef<Shown | null>(null);

  useEffect(() => {
    let timer: number | undefined;
    let pending: HTMLElement | null = null;
    let shownEl: HTMLElement | null = null;

    const cancelTimer = () => {
      if (timer !== undefined) {
        window.clearTimeout(timer);
        timer = undefined;
      }
      pending = null;
    };

    const hide = () => {
      cancelTimer();
      if (!shownRef.current) return; // 没显示就不 setState，避免划过时狂重渲染
      shownEl = null;
      shownRef.current = null;
      setShown(null);
    };

    const fire = () => {
      timer = undefined;
      const el = pending;
      pending = null;
      if (!el || !el.isConnected) return;
      const literal = el.getAttribute("data-hint-text");
      if (!literal) return;
      const r = el.getBoundingClientRect();
      const next: Shown = {
        text: literal,
        rect: { left: r.left, top: r.top, width: r.width, height: r.height },
      };
      shownEl = el;
      shownRef.current = next;
      setShown(next);
    };

    const onOver = (e: PointerEvent) => {
      // 触屏 / 触控笔没有"悬停"概念，弹出来反而挡视线
      if (e.pointerType && e.pointerType !== "mouse") return;
      const el = (e.target as Element | null)?.closest?.(HINT_SELECTOR) as HTMLElement | null;
      // 在同一个按钮内部（图标 → 文字）移动：不算离开，也不重新计时
      const from = e.relatedTarget as Element | null;
      if (el && from && el.contains(from)) return;
      if (!el) {
        hide();
        return;
      }
      if (el === shownEl || el === pending) return;
      cancelTimer();
      if (shownRef.current) {
        shownEl = null;
        shownRef.current = null;
        setShown(null);
      }
      pending = el;
      timer = window.setTimeout(fire, DELAY);
    };

    const onOut = (e: PointerEvent) => {
      const to = e.relatedTarget as Element | null;
      const keep =
        to &&
        ((pending && to.closest?.(HINT_SELECTOR) === pending) ||
          (shownEl && to.closest?.(HINT_SELECTOR) === shownEl));
      if (keep) return;
      hide();
    };

    document.addEventListener("pointerover", onOver, true);
    document.addEventListener("pointerout", onOut, true);
    /* 以下任意一种都说明"用户已经不是在看的这个按钮了" */
    window.addEventListener("pointerdown", hide, true);
    window.addEventListener("keydown", hide, true);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("blur", hide);
    document.addEventListener("pointerleave", hide);

    return () => {
      cancelTimer();
      document.removeEventListener("pointerover", onOver, true);
      document.removeEventListener("pointerout", onOut, true);
      window.removeEventListener("pointerdown", hide, true);
      window.removeEventListener("keydown", hide, true);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("blur", hide);
      document.removeEventListener("pointerleave", hide);
    };
  }, []);

  return createPortal(shown ? <Bubble key="hover-hint" shown={shown} /> : null, document.body);
}
