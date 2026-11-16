/**
 * 滚动条「靠近变粗」（照 FontManager 的做法）。
 *
 * 全局滚动条平时是 1px 发丝线（tokens.css 里那条 `*::-webkit-scrollbar-thumb`），
 * 但 1px 抓不住。做法是给**指针当前靠近的那个滚动容器**挂 `.scroll-hot`，
 * 命中后 thumb 的透明 border 从 4.5px 收成 2px → 视觉 6px。
 *
 * 为什么不逐个容器写 hover：`::-webkit-scrollbar` 只认宿主元素的类，
 * 而「靠近右缘 14px」这个热区无法用 CSS 表达（CSS 只能 `:hover` 整个元素）。
 *
 * 热区按**指针到容器右边界的距离**算，不是滚动条本身的宽度 —— 条在变粗之前只有 1px，
 * 真要压在 1px 上才能变粗，等于永远抓不住。
 */

/** 命中热区：指针距容器右缘多少像素以内就算「要抓滚动条」 */
const EDGE = 14;

/** 可滚的容器才值得挂（`scrollHeight <= clientHeight` 的容器没有条） */
function scrollsY(el: HTMLElement): boolean {
  if (el.scrollHeight - el.clientHeight <= 1) return false;
  const oy = getComputedStyle(el).overflowY;
  return oy === "auto" || oy === "scroll" || oy === "overlay";
}

export function installScrollHot(): () => void {
  let hot: HTMLElement | null = null;
  let raf = 0;
  let lastX = 0;
  let lastY = 0;

  const clear = () => {
    if (hot) {
      hot.classList.remove("scroll-hot");
      hot = null;
    }
  };

  const pick = (x: number, y: number) => {
    // 从指针下最深的元素开始往上找第一个「在右缘热区内 + 真的能滚」的容器
    const stack = document.elementsFromPoint(x, y);
    let found: HTMLElement | null = null;
    for (const node of stack) {
      if (!(node instanceof HTMLElement)) continue;
      if (node.dataset.scrollHot === "off") continue;
      if (!scrollsY(node)) continue;
      const r = node.getBoundingClientRect();
      if (x >= r.right - EDGE && x <= r.right) {
        found = node;
        break;
      }
    }
    if (found !== hot) {
      clear();
      if (found) {
        found.classList.add("scroll-hot");
        hot = found;
      }
    }
  };

  const onMove = (e: MouseEvent) => {
    lastX = e.clientX;
    lastY = e.clientY;
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      pick(lastX, lastY);
    });
  };

  const onLeave = () => clear();

  window.addEventListener("mousemove", onMove, { passive: true });
  window.addEventListener("blur", onLeave);
  document.addEventListener("mouseleave", onLeave);

  return () => {
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("blur", onLeave);
    document.removeEventListener("mouseleave", onLeave);
    if (raf) cancelAnimationFrame(raf);
    clear();
  };
}
