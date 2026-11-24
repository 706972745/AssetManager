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

export function installScrollHot(): () => void {
  let hot: HTMLElement | null = null;
  let raf = 0;
  let lastX = 0;
  let lastY = 0;
  /** 指针已经离开窗口（或窗口失焦）：停止一切计算，直到下次真的进来 */
  let idle = false;
  /* P1-6：`overflowY` 几乎不变，按元素缓存判定结果 —— 以前每一帧对祖先链上每个节点
     都调一次 `getComputedStyle`（强制样式解析），这是鼠标移动时的固定开销。
     窗口尺寸变化会让「能不能滚」变，那时整体作废（WeakMap 没有 clear）。 */
  let cache = new WeakMap<HTMLElement, boolean>();

  const scrollsY = (el: HTMLElement): boolean => {
    const hit = cache.get(el);
    if (hit !== undefined) return hit;
    let ok = false;
    if (el.scrollHeight - el.clientHeight > 1) {
      const oy = getComputedStyle(el).overflowY;
      ok = oy === "auto" || oy === "scroll" || oy === "overlay";
    }
    cache.set(el, ok);
    return ok;
  };

  const clear = () => {
    if (hot) {
      hot.classList.remove("scroll-hot");
      hot = null;
    }
  };

  const pick = (x: number, y: number) => {
    /* 从指针下最深的元素开始往上找第一个「在右缘热区内 + 真的能滚」的容器。
       用 elementFromPoint + 祖先链，而不是 elementsFromPoint 整条命中栈：
       后者每帧都要生成一份栈（可能十几个节点），再由我们逐个读取样式 / 布局。 */
    let node: Element | null = document.elementFromPoint(x, y);
    let found: HTMLElement | null = null;
    while (node && node !== document.documentElement) {
      if (node instanceof HTMLElement) {
        // `data-scroll-hot="off"` 只排除这个元素本身，祖先照样要看（和旧实现一致）
        if (node.dataset.scrollHot !== "off") {
          const r = node.getBoundingClientRect();
          if (x >= r.right - EDGE && x <= r.right && scrollsY(node)) {
            found = node;
            break;
          }
        }
      }
      node = node.parentElement;
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
    if (e.clientX < 0 || e.clientY < 0) {
      /* 指针已经出了窗口（个别合成事件还会送来）：停掉、摘高亮，
         不必再为窗口外的移动做命中测试 */
      idle = true;
      if (raf) {
        cancelAnimationFrame(raf);
        raf = 0;
      }
      clear();
      return;
    }
    idle = false;
    lastX = e.clientX;
    lastY = e.clientY;
    // 一帧最多算一次（P1-6）：一帧里 mousemove 可能来好几次
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      if (idle) return;
      pick(lastX, lastY);
    });
  };

  const onLeave = () => {
    idle = true;
    clear();
  };

  const onResize = () => {
    cache = new WeakMap<HTMLElement, boolean>();
  };

  window.addEventListener("mousemove", onMove, { passive: true });
  window.addEventListener("blur", onLeave);
  window.addEventListener("resize", onResize);
  document.addEventListener("mouseleave", onLeave);

  return () => {
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("blur", onLeave);
    window.removeEventListener("resize", onResize);
    document.removeEventListener("mouseleave", onLeave);
    if (raf) cancelAnimationFrame(raf);
    clear();
  };
}
