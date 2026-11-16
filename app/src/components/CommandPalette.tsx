import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { scrimFade, springSnappy } from "../design/springs";
import { Icon } from "./ui";

/**
 * 命令面板（T27）：`Ctrl/⌘ + K` 唤起，560×420，键盘全程可达。
 *
 * 为什么要有它：素材管理器的操作面铺得很开（顶栏排序/尺寸/视图、右键菜单、设置里的
 * 重建索引/缓存/预热）。熟练之后手会离开鼠标，但快捷键只有空格/Ctrl+Z 那几个。
 * 命令面板把这些**已有的动作**汇总成一张可搜索的清单 —— 它自己不发明任何新功能，
 * 所以不存在"面板里有、别处没有"的分裂（每条命令都指向 App 里同一个 handler）。
 *
 * 三条纪律（照 FontManager 的同名面板）：
 * 1. 只列**现在真能执行**的（比如没选中素材就不出现"删除选中项"）；
 * 2. 搜索吃 label + 关键词，能搜"缓存"找到"清理缩略图缓存"；
 * 3. 键盘优先：↑↓ 移动、Enter 执行、Esc 关闭，鼠标只是备选。
 */
export interface Command {
  id: string;
  label: string;
  /** 右侧灰字提示（快捷键、影响范围等） */
  hint?: string;
  /** 搜索关键词（label 之外的同义词） */
  keywords?: string;
  icon?: string;
  run: () => void;
}

function score(c: Command, q: string): number {
  const hay = `${c.label} ${c.keywords ?? ""}`.toLowerCase();
  const label = c.label.toLowerCase();
  if (label.startsWith(q)) return 3000 - label.length;
  if (label.includes(q)) return 2000 - label.length;
  if (hay.includes(q)) return 1000 - hay.length;
  return -1;
}

export function CommandPalette({
  open,
  onClose,
  commands,
}: {
  open: boolean;
  onClose: () => void;
  commands: Command[];
}) {
  const [q, setQ] = useState("");
  const [cursor, setCursor] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    setQ("");
    setCursor(0);
    // 等浮层挂上再聚焦（motion 的入场会先渲染一帧）
    requestAnimationFrame(() => inputRef.current?.focus());
  }, [open]);

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const all = needle
      ? commands
          .map((c) => ({ c, s: score(c, needle) }))
          .filter((x) => x.s > 0)
          .sort((a, b) => b.s - a.s)
          .map((x) => x.c)
      : commands;
    return all.slice(0, 60);
  }, [commands, q]);

  useEffect(() => {
    setCursor((c) => Math.min(c, Math.max(0, shown.length - 1)));
  }, [shown.length]);

  /* 键盘：输入框在焦点上时也要能上下移动，所以监听在输入框自己身上 */
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => (shown.length ? (c + 1) % shown.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => (shown.length ? (c - 1 + shown.length) % shown.length : 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const c = shown[cursor];
      if (c) {
        onClose();
        c.run();
      }
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  };

  // 选中项滚进视野
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(".cmdk-row.on");
    el?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="cmdk-scrim"
          onMouseDown={onClose}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={scrimFade}
        >
          <motion.div
            className="cmdk"
            role="dialog"
            aria-label="命令面板"
            onMouseDown={(e) => e.stopPropagation()}
            initial={{ opacity: 0, y: -8, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -6, scale: 0.99 }}
            transition={springSnappy}
          >
            <div className="cmdk-head">
              <Icon name="search" size={14} stroke={1.6} className="cmdk-ico" />
              <input
                ref={inputRef}
                className="cmdk-input"
                value={q}
                placeholder="输入命令…（设置、缓存、排序、视图、评分）"
                onChange={(e) => {
                  setQ(e.target.value);
                  setCursor(0);
                }}
                onKeyDown={onKey}
                spellCheck={false}
              />
              <span className="cmdk-count mono">{shown.length}</span>
            </div>

            <div className="cmdk-list" ref={listRef} role="listbox">
              {shown.length === 0 && <div className="cmdk-empty">没有匹配的命令</div>}
              {shown.map((c, i) => (
                <button
                  key={c.id}
                  role="option"
                  aria-selected={i === cursor}
                  className={`cmdk-row ${i === cursor ? "on" : ""}`}
                  onMouseEnter={() => setCursor(i)}
                  onClick={() => {
                    onClose();
                    c.run();
                  }}
                >
                  <Icon name={c.icon ?? "sliders"} size={13} stroke={1.5} className="cmdk-row-ico" />
                  <span className="cmdk-label">{c.label}</span>
                  {c.hint && <span className="cmdk-hint">{c.hint}</span>}
                </button>
              ))}
            </div>

            <div className="cmdk-foot">
              <span>
                <kbd>↑</kbd>
                <kbd>↓</kbd> 选择
              </span>
              <span>
                <kbd>Enter</kbd> 执行
              </span>
              <span>
                <kbd>Esc</kbd> 关闭
              </span>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
