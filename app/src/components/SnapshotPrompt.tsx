import { useEffect, useMemo, useRef, useState } from "react";
import { motion } from "motion/react";
import type { SnapshotRow } from "../types";
import { BlinkCaretInput, Icon } from "./ui";
import { scrimFade, springSnappy } from "../design/springs";

/**
 * 快照的小卡片（照 TagPrompt 的形态：遮罩 + 小卡片 + 一个输入框）。
 *
 * 三种用法共用一套壳：
 * - `tag`：右键标签 →「把标签下的图片加入快照」，列出已有快照让你选一个，
 *   也可以直接在输入框里写新名字**新建并加入**；
 * - `new`：头部 `+` 新建一个空快照；
 * - `rename`：改名（重名由后端拒绝，界面把原话显示出来）。
 */
export function SnapshotPrompt({
  mode,
  snapshots,
  tag,
  count,
  initial,
  onPick,
  onName,
  onClose,
}: {
  mode: "tag" | "new" | "rename";
  snapshots: SnapshotRow[];
  /** mode=tag：这批图来自哪个标签 */
  tag?: string;
  /** mode=tag：这批图有多少张（先给个数，别让用户以为只加了一部分） */
  count?: number;
  initial?: string;
  onPick?: (id: number) => void;
  onName: (name: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial ?? "");
  const inputRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const t = window.setTimeout(() => {
      inputRef.current?.querySelector("input")?.focus();
    }, 40);
    return () => window.clearTimeout(t);
  }, []);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [onClose]);

  const q = name.trim().toLowerCase();
  const picks = useMemo(
    () => (mode === "tag" ? snapshots.filter((s) => !q || s.name.toLowerCase().includes(q)) : []),
    [mode, snapshots, q]
  );

  const title =
    mode === "tag" ? "把图片加入快照" : mode === "new" ? "新建快照" : "给快照改名";

  return (
    <motion.div
      className="dlg-scrim"
      onMouseDown={onClose}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={scrimFade}
    >
      <motion.div
        className="dlg tag-prompt"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(e) => e.stopPropagation()}
        initial={{ opacity: 0, scale: 0.96, y: 12 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        transition={springSnappy}
      >
        <div className="dlg-title tag-prompt-title">
          <Icon name="expand" size={14} stroke={1.5} />
          {title}
        </div>

        {mode === "tag" && (
          <p className="dlg-hint snap-prompt-lead">
            标签「{tag}」下{typeof count === "number" ? ` ${count} 张图` : "的图"}
            会<b>记进快照</b>：不移动、不复制任何文件，只是记下"这些图是哪几张"。
          </p>
        )}

        <div className="tag-prompt-field" ref={inputRef}>
          <BlinkCaretInput
            autoFocus
            value={name}
            onChange={setName}
            placeholder={mode === "rename" ? "新的快照名" : "输入快照名，回车确认"}
            inputClassName="tag-prompt-input"
            ariaLabel="快照名"
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                const v = name.trim();
                if (v) onName(v);
              }
            }}
          />
        </div>

        {mode === "tag" && picks.length > 0 && (
          <div className="tag-prompt-picks">
            <div className="tag-prompt-hint">已有的快照（点一下直接加进去）</div>
            {picks.slice(0, 8).map((s) => (
              <button key={s.id} className="tag-chip pick" onClick={() => onPick?.(s.id)}>
                {s.name}
                <span className="snap-pick-count mono">{s.count}</span>
              </button>
            ))}
          </div>
        )}

        <div className="dlg-actions">
          <button className="btn" onClick={onClose}>
            取消
          </button>
          <button
            className="btn btn-accent"
            disabled={!name.trim()}
            onClick={() => onName(name.trim())}
          >
            {mode === "tag" ? `新建「${name.trim() || "…"}」并加入` : mode === "rename" ? "改名" : "新建"}
          </button>
        </div>
      </motion.div>
    </motion.div>
  );
}
