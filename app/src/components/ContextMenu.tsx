import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { motion } from "motion/react";
import { Icon } from "./ui";
import { springSnappy } from "../design/springs";

/**
 * 自绘右键菜单。
 *
 * 为什么要自己做：WebView2 在页面空白处右键会弹出「返回 / 刷新 / 另存为 / 打印 / 更多工具」
 * 这套浏览器菜单（owner 2026-11-16 截图），它跟桌面软件完全不是一回事。
 * 样式照 FontManager 的 `.ctx-menu`：208 宽、4 圆角、5 内距、30 高行、1px 分隔线。
 */

export type CtxItem =
  | {
      kind?: "item";
      label: string;
      icon?: string;
      shortcut?: string;
      danger?: boolean;
      disabled?: boolean;
      action: () => void;
    }
  | { kind: "separator" }
  | { kind: "heading"; label: string }
  /**
   * 只读信息行（「所属标签 / 所属分组」）。
   *
   * 数据是异步来的：菜单必须先弹出来，不能等 SQL。所以 `load` 由菜单挂载后
   * 自己去跑，跑完只填文字；失败静默落成 `empty`，不弹错误框、也不关菜单。
   * 渲染成 div 而不是 button —— 点上去没有动作，也不会触发 onClose。
   */
  | {
      kind: "info";
      label: string;
      load: () => Promise<string>;
      empty?: string;
    };

export interface CtxState {
  x: number;
  y: number;
  items: CtxItem[];
  /**
   * 这个菜单是谁弹的（"coll" = 分组行 / "tag" = 标签行 / 其它不填）。
   *
   * 用途只有一个：侧栏那两个停靠面板的「保持展开」要**按来源**判断。
   * 以前写的是 `hold={!!ctx}` —— 于是右键**任何东西**（素材、空白、快照）
   * 都会把分组和标签两个面板同时展开（owner 2026-11-18 报的 bug）。
   */
  source?: string;
}

/**
 * 只读信息行。挂载后才取数，所以菜单不会被 SQL 卡住弹出。
 * 加载中显示 `…`，失败显示 `empty`（调用方传「无」）。
 */
function CtxInfoRow({ item }: { item: Extract<CtxItem, { kind: "info" }> }) {
  const [text, setText] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void item
      .load()
      .then((v) => {
        if (alive) setText(v);
      })
      .catch(() => {
        if (alive) setText(item.empty ?? "—");
      });
    return () => {
      alive = false;
    };
  }, [item]);

  return (
    <div className="ctx-info" role="note" title={text ?? undefined}>
      <span className="ctx-info-k">{item.label}</span>
      <span className={`ctx-info-v${text == null ? " pending" : ""}`}>{text ?? "…"}</span>
    </div>
  );
}

export function ContextMenu({ state, onClose }: { state: CtxState; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: state.x, y: state.y });

  /* 贴边翻转：菜单不能被窗口右缘/下缘切掉。
     量尺寸用 offsetWidth/offsetHeight（布局尺寸），不用 getBoundingClientRect ——
     入场时菜单带着 scale(0.96)，getBoundingClientRect 会把缩放算进去，
     贴右下角时就会多留 4% 的缝。 */
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const place = () => {
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      setPos({
        x: Math.max(6, Math.min(state.x, window.innerWidth - w - 6)),
        y: Math.max(6, Math.min(state.y, window.innerHeight - h - 6)),
      });
    };
    place();
    /* 信息段（所属标签/分组）是异步填进来的，菜单会变高 —— 光在挂载时量一次，
       贴到窗口下缘的菜单长高后会有一截跑到屏幕外。尺寸一变就重新贴边。 */
    const ro = new ResizeObserver(place);
    ro.observe(el);
    return () => ro.disconnect();
  }, [state]);

  useEffect(() => {
    const outside = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // 同 window 上的全局快捷键监听也要挡掉（见 WORKLOG §10.5 #16）
        e.preventDefault();
        e.stopImmediatePropagation();
        onClose();
      }
    };
    const close = () => onClose();
    /* 页面滚动就关菜单（菜单是 fixed 定位，页面一滚它就悬空了）。
       但**菜单自己内部的滚动不能关** —— 贴图窗口很小，菜单项多到超出窗口高度，
       用户只能靠滚轮往下翻；以前这里一律 close，表现就是"菜单显示不全又滚不动"
       （owner 2026-11-18 报的）。 */
    const onScroll = (e: Event) => {
      const t = e.target;
      if (t instanceof Node && ref.current?.contains(t)) return;
      close();
    };
    window.addEventListener("mousedown", outside, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("mousedown", outside, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [onClose]);

  const pick = (it: Extract<CtxItem, { kind?: "item" }>) => {
    if (it.disabled) return;
    onClose();
    it.action();
  };

  const rows: ReactNode = state.items.map((it, i) => {
    if (it.kind === "separator") return <div className="ctx-sep" key={`s-${i}`} />;
    if (it.kind === "heading")
      return (
        <div className="ctx-heading" key={`h-${i}`}>
          {it.label}
        </div>
      );
    if (it.kind === "info") return <CtxInfoRow key={`f-${i}`} item={it} />;
    return (
      <button
        key={`i-${i}`}
        className={`ctx-item ${it.danger ? "danger" : ""}`}
        role="menuitem"
        disabled={it.disabled}
        onClick={(e) => {
          e.stopPropagation();
          pick(it);
        }}
      >
        {it.icon ? (
          <span className="ctx-icon">
            <Icon name={it.icon} size={13} stroke={1.5} />
          </span>
        ) : (
          <span className="ctx-icon" />
        )}
        <span className="ctx-label">{it.label}</span>
        {it.shortcut && <span className="ctx-kbd mono">{it.shortcut}</span>}
      </button>
    );
  });

  return createPortal(
    <motion.div
      className="ctx-menu scroll-hot"
      ref={ref}
      style={{ left: pos.x, top: pos.y }}
      role="menu"
      /* 入场与 FontManager .ctx-menu 逐值一致：从左上角 0.96 弹出 4px，
         弹簧收口（用户对两个软件的手感要求是同一种）。 */
      initial={{ opacity: 0, scale: 0.96, y: -4 }}
      animate={{ opacity: 1, scale: 1, y: 0 }}
      transition={springSnappy}
    >
      {rows}
    </motion.div>,
    document.body
  );
}
