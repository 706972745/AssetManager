import { useEffect, useMemo, useRef, useState } from "react";
import { motion } from "motion/react";
import type { SnapshotRow } from "../types";
import { BlinkCaretInput, Icon } from "./ui";
import { scrimFade, springSnappy } from "../design/springs";

/**
 * 「把标签下的图片加入快照」这张小卡片（照 TagPrompt 的形态：遮罩 + 小卡片 + 一个输入框）。
 *
 * 这里**只做"选一个已有快照 / 写个新名字建好并加入"这一件事**。
 * 空白快照的新建与改名**不在这里** —— 那是侧栏快照段里的一行就地输入（照 FontManager），
 * 见 `SnapshotDock.tsx` 顶部注释第 3 条。
 */
export function SnapshotPrompt({
  snapshots,
  tag,
  count,
  onPick,
  onName,
  onClose,
}: {
  snapshots: SnapshotRow[];
  /** 这批图来自哪个标签 */
  tag?: string;
  /** 这批图有多少张（先给个数，别让用户以为只加了一部分） */
  count?: number;
  onPick?: (id: number) => void;
  onName: (name: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
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
    () => snapshots.filter((s) => !q || s.name.toLowerCase().includes(q)),
    [snapshots, q]
  );

  const title = "把图片加入快照";

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

        <p className="dlg-hint snap-prompt-lead">
          标签「{tag}」下{typeof count === "number" ? ` ${count} 张图` : "的图"}
          会<b>记进快照</b>：不移动、不复制任何文件，只是记下「这些图是哪几张」。
        </p>

        <div className="tag-prompt-field" ref={inputRef}>
          <BlinkCaretInput
            autoFocus
            value={name}
            onChange={setName}
            placeholder="输入新快照名，回车「建好并加入」"
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

        {picks.length > 0 && (
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
            {`新建「${name.trim() || "…"}」并加入`}
          </button>
        </div>
      </motion.div>
    </motion.div>
  );
}
