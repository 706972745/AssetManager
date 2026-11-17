import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { motion } from "motion/react";
import type { SnapshotRow } from "../types";
import { BlinkCaretInput, Icon } from "./ui";
import { scrimFade, springSnappy } from "../design/springs";

/**
 * 「把图片加入快照」这张小卡片（照 TagPrompt 的形态：遮罩 + 小卡片 + 一个输入框）。
 *
 * 这里**只做"选一个已有快照 / 写个新名字建好并加入"这一件事**。
 * 空白快照的新建与改名**不在这里** —— 那是侧栏快照段里的一行就地输入（照 FontManager），
 * 见 `SnapshotDock.tsx` 顶部注释第 3 条。
 *
 * 0.1.18 起同一张卡片服务四种入口（标题 / 说明由上层给）：
 * 标签右键加入、选中素材加入、与另一份**合并**、与另一份**做差集**。
 * 合并/差集**不需要新名字**（名字由两份快照的名字拼出来），所以传 `canCreate={false}`
 * 就只剩"选另一份"这一件事 —— 别让用户以为还得自己起名。
 */
export function SnapshotPrompt({
  snapshots,
  title = "把图片加入快照",
  lead,
  picksHint = "已有的快照（点一下直接加进去）",
  excludeIds,
  canCreate = true,
  onPick,
  onName,
  onClose,
}: {
  snapshots: SnapshotRow[];
  title?: string;
  /** 说明这一段：说清"会发生什么 / 不会发生什么" */
  lead?: ReactNode;
  picksHint?: string;
  /** 不列出来的快照（合并/差集时排除自己） */
  excludeIds?: number[];
  /** 能不能"写个新名字建好并加入"。合并/差集为 false */
  canCreate?: boolean;
  onPick?: (id: number) => void;
  onName: (name: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const inputRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!canCreate) return;
    const t = window.setTimeout(() => {
      inputRef.current?.querySelector("input")?.focus();
    }, 40);
    return () => window.clearTimeout(t);
  }, [canCreate]);

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
  const picks = useMemo(() => {
    const skip = new Set(excludeIds ?? []);
    return snapshots.filter(
      (s) => !skip.has(s.id) && (!q || s.name.toLowerCase().includes(q))
    );
  }, [snapshots, q, excludeIds]);

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

        {lead && <p className="dlg-hint snap-prompt-lead">{lead}</p>}

        {canCreate && (
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
        )}

        {picks.length > 0 && (
          <div className="tag-prompt-picks">
            <div className="tag-prompt-hint">{picksHint}</div>
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
          {canCreate && (
            <button
              className="btn btn-accent"
              disabled={!name.trim()}
              onClick={() => onName(name.trim())}
            >
              {`新建「${name.trim() || "…"}」并加入`}
            </button>
          )}
        </div>
      </motion.div>
    </motion.div>
  );
}
