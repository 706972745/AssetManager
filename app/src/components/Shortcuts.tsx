import { useEffect } from "react";
import { AnimatePresence, motion } from "motion/react";
import { scrimFade, springSnappy } from "../design/springs";
import { useT, type TKey } from "../lib/i18n";

/**
 * 快捷键一览（侧栏底部「快捷键」进入）。
 *
 * 为什么单独一个面板而不是 toast：以前点侧栏是把一长串快捷键塞进 toast，
 * 一屏读不完、还会挡住详情面板（owner 2026-11-17 截图指出）。
 * 现在照 FontManager 的快捷键面板做**双栏**：左栏浏览/选择，右栏文件/面板，
 * 每条都是**代码里真实注册过**的键（不写没实现的），键帽用同一套 `.kbd-set`。
 */

/** 键帽：字符串原样显示（Space / Ctrl / ←），`tk` 走翻译（"双击" 这类中文键名） */
type Cap = string | { tk: TKey };

interface Row {
  keys: Cap[];
  label: TKey;
  /** 补充说明（灰字，跟在标签后面） */
  hint?: TKey;
}

interface Group {
  title: TKey;
  rows: Row[];
}

/** 左栏：看与选（只列代码里**真的注册过**的键；没有的不要往里写） */
const LEFT: Group[] = [
  {
    title: "shortcuts.grpBrowse",
    rows: [
      { keys: ["Space"], label: "shortcuts.r.preview" },
      { keys: ["B"], label: "shortcuts.r.peek", hint: "shortcuts.h.peek" },
      { keys: ["←", "→"], label: "shortcuts.r.prevNext", hint: "shortcuts.h.prevNext" },
      { keys: [{ tk: "shortcuts.kbdWheel" }], label: "shortcuts.r.zoom" },
      { keys: [{ tk: "shortcuts.kbdDouble" }], label: "shortcuts.r.zoomReset" },
      { keys: ["Esc"], label: "shortcuts.r.close" },
      { keys: [{ tk: "shortcuts.kbdDouble" }], label: "shortcuts.r.rename", hint: "shortcuts.h.rename" },
      { keys: ["Shift", { tk: "shortcuts.kbdTitle" }], label: "shortcuts.r.copyName", hint: "shortcuts.h.copyName" },
    ],
  },
  {
    title: "shortcuts.grpSelect",
    rows: [
      { keys: ["Ctrl", { tk: "shortcuts.kbdClick" }], label: "shortcuts.r.toggleSelect" },
      { keys: ["Shift", { tk: "shortcuts.kbdClick" }], label: "shortcuts.r.rangeSelect" },
      { keys: [{ tk: "shortcuts.kbdDragBlank" }], label: "shortcuts.r.marquee" },
      { keys: ["Ctrl", "A"], label: "shortcuts.r.selectAll" },
    ],
  },
];

/** 右栏：干活（文件动作一律落在磁盘上的原文件，不是缩略图） */
const RIGHT: Group[] = [
  {
    title: "shortcuts.grpFiles",
    rows: [
      { keys: [{ tk: "shortcuts.kbdDragOut" }], label: "shortcuts.r.dragOut", hint: "shortcuts.h.dragOut" },
      { keys: [{ tk: "shortcuts.kbdDragTree" }], label: "shortcuts.r.dragTree" },
      { keys: ["Ctrl", "C"], label: "app.menu.copy_file", hint: "shortcuts.h.copyFile" },
      { keys: ["Ctrl", "Z"], label: "app.cmd.undo" },
      { keys: ["T"], label: "shortcuts.r.tag", hint: "shortcuts.h.tag" },
      { keys: [{ tk: "shortcuts.kbdRightClick" }], label: "shortcuts.r.contextInfo", hint: "shortcuts.h.contextInfo" },
      { keys: ["Shift", { tk: "shortcuts.kbdRightClick" }], label: "shortcuts.r.contextMenu", hint: "shortcuts.h.contextMenu" },
    ],
  },
  {
    title: "shortcuts.grpCapture",
    rows: [
      { keys: ["F1"], label: "shortcuts.r.snip", hint: "shortcuts.h.snip" },
      { keys: ["F3"], label: "shortcuts.r.clipPeek" },
    ],
  },
  {
    /* F2 是**固定**的系统键（Rust 里和 F1/F3 走同一张热键表注册），
       设置面板不提供改键，所以这里只列出来 */
    title: "shortcuts.grpWindow",
    rows: [{ keys: ["F2"], label: "shortcuts.r.toggleMain", hint: "shortcuts.h.toggleMain" }],
  },
  {
    /* 键位以 `Snip.tsx` 的 keydown 为准（0.1.27）：Ctrl+C 复制、Ctrl+S 保存，
       两者只认真人按键（外部程序注入的会被静默丢弃）。 */
    title: "shortcuts.grpAnnotate",
    rows: [
      { keys: ["Ctrl", "C"], label: "shortcuts.r.annCopy", hint: "shortcuts.h.annCopy" },
      { keys: ["Ctrl", "S"], label: "shortcuts.r.annSave", hint: "shortcuts.h.annSave" },
      { keys: ["Ctrl", "Shift", "S"], label: "shortcuts.r.annSaveAs", hint: "shortcuts.h.annSaveAs" },
      { keys: [{ tk: "shortcuts.kbdMiddleClick" }], label: "shortcuts.r.annPeek", hint: "shortcuts.h.annPeek" },
      { keys: ["Enter"], label: "shortcuts.r.annStay", hint: "shortcuts.h.annStay" },
      { keys: ["V"], label: "shortcuts.r.annSelect" },
      { keys: ["R", "E", "L", "A"], label: "shortcuts.r.annShapes" },
      { keys: ["P", "H"], label: "shortcuts.r.annBrush" },
      { keys: ["T", "N", "K"], label: "shortcuts.r.annText", hint: "shortcuts.h.annText" },
      { keys: ["M", "B", "G"], label: "shortcuts.r.annMosaic" },
      { keys: ["X", "C"], label: "shortcuts.r.annStamp" },
      { keys: ["[", "]"], label: "shortcuts.r.annWidth", hint: "shortcuts.h.annWidth" },
      { keys: ["F"], label: "shortcuts.r.annFill" },
      { keys: ["Y"], label: "shortcuts.r.annEraser" },
      { keys: ["Delete"], label: "shortcuts.r.annDelete" },
      { keys: ["Ctrl", "Z"], label: "shortcuts.r.annUndo", hint: "shortcuts.h.annUndo" },
      { keys: ["Space", { tk: "shortcuts.kbdDrag" }], label: "shortcuts.r.annMove", hint: "shortcuts.h.annMove" },
      { keys: ["Ctrl", "A"], label: "shortcuts.r.annAll" },
      { keys: ["↑", "↓", "←", "→"], label: "shortcuts.r.annNudge", hint: "shortcuts.h.annNudge" },
      { keys: ["Shift"], label: "shortcuts.r.annColor", hint: "shortcuts.h.annColor" },
      { keys: ["Esc"], label: "shortcuts.r.annCancel", hint: "shortcuts.h.annCancel" },
    ],
  },
  {
    title: "shortcuts.grpBoard",
    rows: [
      { keys: ["Ctrl", "V"], label: "shortcuts.r.boardPaste" },
      { keys: ["T"], label: "shortcuts.r.boardNote", hint: "shortcuts.h.boardNote" },
      { keys: ["Shift", { tk: "shortcuts.kbdClick" }], label: "shortcuts.r.boardMulti", hint: "shortcuts.h.boardMulti" },
      { keys: ["Ctrl", "G"], label: "shortcuts.r.boardGroup", hint: "shortcuts.h.boardGroup" },
      { keys: ["Ctrl", "Shift", "G"], label: "shortcuts.r.boardUngroup" },
      { keys: ["L"], label: "shortcuts.r.boardLayers", hint: "shortcuts.h.boardLayers" },
      { keys: ["F"], label: "shortcuts.r.boardFit", hint: "shortcuts.h.boardFit" },
      { keys: ["Ctrl", "K"], label: "shortcuts.r.commandPalette", hint: "shortcuts.h.commandPalette" },
      { keys: ["Ctrl", ","], label: "shortcuts.r.settings" },
    ],
  },
];

function GroupBlock({ g }: { g: Group }) {
  const t = useT();
  return (
    <section className="kb-group">
      <div className="kb-group-title">{t(g.title)}</div>
      {g.rows.map((r) => (
        <div className="kb-row" key={`${g.title}-${r.label}`}>
          <span className="kb-label">
            {t(r.label)}
            {r.hint && <span className="kb-hint">{t(r.hint)}</span>}
          </span>
          <span className="kbd-set kb-keys">
            {r.keys.map((k, i) => (
              <kbd key={i}>{typeof k === "string" ? k : t(k.tk)}</kbd>
            ))}
          </span>
        </div>
      ))}
    </section>
  );
}

export function Shortcuts({ open, onClose }: { open: boolean; onClose: () => void }) {
  const t = useT();
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
            aria-label={t("nav.shortcuts")}
            onMouseDown={(e) => e.stopPropagation()}
            initial={{ opacity: 0, scale: 0.97, y: 10 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.98, y: 6, transition: { duration: 0.13 } }}
            transition={springSnappy}
          >
            <header className="kb-head">
              <span className="kb-title">{t("nav.shortcuts")}</span>
              <button className="kb-close" onClick={onClose} title={t("shortcuts.closeEsc")}>
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
            <footer className="kb-foot">{t("shortcuts.footer")}</footer>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
