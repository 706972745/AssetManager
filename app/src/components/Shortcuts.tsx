import { useEffect } from "react";
import { AnimatePresence, motion } from "motion/react";
import { scrimFade, springSnappy } from "../design/springs";

/**
 * 快捷键一览（侧栏底部「快捷键」进入）。
 *
 * 为什么单独一个面板而不是 toast：以前点侧栏是把一长串快捷键塞进 toast，
 * 一屏读不完、还会挡住详情面板（owner 2026-11-17 截图指出）。
 * 现在照 FontManager 的快捷键面板做**双栏**：左栏浏览/选择，右栏文件/面板，
 * 每条都是**代码里真实注册过**的键（不写没实现的），键帽用同一套 `.kbd-set`。
 */

interface Row {
  keys: string[];
  label: string;
  /** 补充说明（灰字，跟在标签后面） */
  hint?: string;
}

interface Group {
  title: string;
  rows: Row[];
}

/** 左栏：看与选 */
const LEFT: Group[] = [
  {
    title: "浏览",
    rows: [
      { keys: ["Space"], label: "全屏预览选中的素材" },
      { keys: ["B"], label: "无边框看图", hint: "只剩那张图，拖到任意位置截图；滚轮缩窗口" },
      { keys: ["←", "→"], label: "上一个 / 下一个", hint: "大图里翻页，网格里换选中" },
      { keys: ["Esc"], label: "关大图 / 取消多选" },
      { keys: ["双击"], label: "用默认程序打开原文件" },
      { keys: ["滚轮"], label: "大图里缩放（1×–8×）" },
      { keys: ["双击大图"], label: "缩放复位到 1:1" },
    ],
  },
  {
    title: "选择",
    rows: [
      { keys: ["Ctrl", "点击"], label: "加选 / 取消这一项" },
      { keys: ["Shift", "点击"], label: "从上一个到这一个连选" },
      { keys: ["空白处拖动"], label: "框选" },
      { keys: ["Ctrl", "A"], label: "全选当前列表" },
    ],
  },
];

/** 右栏：干活 */
const RIGHT: Group[] = [
  {
    title: "文件（都作用在原文件上）",
    rows: [
      { keys: ["拖出窗口"], label: "复制原文件", hint: "拖到 PS / 资源管理器 / 浏览器地址栏" },
      { keys: ["拖到目录树"], label: "真实移动到这个文件夹" },
      { keys: ["Ctrl", "Z"], label: "撤销上一步文件操作" },
      { keys: ["T"], label: "给选中的素材打标签" },
    ],
  },
  {
    title: "面板",
    rows: [
      { keys: ["Ctrl", "K"], label: "命令面板", hint: "所有操作都能搜到" },
      { keys: ["Ctrl", ","], label: "设置" },
      { keys: ["Esc"], label: "关无边框窗口（在里面）" },
      { keys: ["Esc"], label: "关闭当前浮层" },
    ],
  },
];

function GroupBlock({ g }: { g: Group }) {
  return (
    <section className="kb-group">
      <div className="kb-group-title">{g.title}</div>
      {g.rows.map((r) => (
        <div className="kb-row" key={`${g.title}-${r.label}`}>
          <span className="kb-label">
            {r.label}
            {r.hint && <span className="kb-hint">{r.hint}</span>}
          </span>
          <span className="kbd-set kb-keys">
            {r.keys.map((k) => (
              <kbd key={k}>{k}</kbd>
            ))}
          </span>
        </div>
      ))}
    </section>
  );
}

export function Shortcuts({ open, onClose }: { open: boolean; onClose: () => void }) {
  // Esc 关面板：capture + stopImmediatePropagation，别让 App 的全局 Esc 顺手关掉别的东西
  useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [open, onClose]);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          key="kb-scrim"
          className="set-scrim"
          onMouseDown={onClose}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={scrimFade}
        >
          <motion.div
            className="kb-panel"
            role="dialog"
            aria-modal="true"
            aria-label="快捷键"
            onMouseDown={(e) => e.stopPropagation()}
            initial={{ opacity: 0, scale: 0.97, y: 10 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.98, y: 6, transition: { duration: 0.13 } }}
            transition={springSnappy}
          >
            <header className="kb-head">
              <span className="kb-title">快捷键</span>
              <button className="kb-close" onClick={onClose} title="关闭（Esc）">
                Esc
              </button>
            </header>
            <div className="kb-body">
              <div className="kb-col">
                {LEFT.map((g) => (
                  <GroupBlock key={g.title} g={g} />
                ))}
              </div>
              <div className="kb-col">
                {RIGHT.map((g) => (
                  <GroupBlock key={g.title} g={g} />
                ))}
              </div>
            </div>
            <footer className="kb-foot">
              拖出去 / 移走的永远是磁盘上的原文件；缩略图只负责显示。
            </footer>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
