import { useEffect, useMemo, useRef, useState } from "react";
import { motion } from "motion/react";
import { BlinkCaretInput, Icon } from "./ui";
import { scrimFade, springSnappy } from "../design/springs";

/**
 * 批量打标签（照 FontManager `BulkTagPrompt` 那套：遮罩 + 小卡片 + 一个输入框）。
 *
 * 与 FontManager 一致的三点：
 * 1. 输入框里可以是**新标签**，也可以是已有标签（下面按输入过滤出候选，点一下直接套用）；
 * 2. 回车 = 应用，Esc = 取消；
 * 3. 一次给**一批素材**打同一个标签（单张也走这条路，不另做一套交互）。
 */
export function TagPrompt({
  targets,
  existing,
  onApply,
  onClose,
}: {
  /** 要打标签的素材数 */
  targets: number;
  /** 已有手工标签（做候选） */
  existing: string[];
  onApply: (tag: string) => void;
  onClose: () => void;
}) {
  const [tag, setTag] = useState("");
  const inputRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setTag("");
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

  const q = tag.trim().toLowerCase();
  const picks = useMemo(
    () =>
      existing
        .filter((t) => !q || t.toLowerCase().includes(q))
        .slice(0, 6),
    [existing, q]
  );

  const apply = () => {
    const v = tag.trim();
    if (!v) return;
    onApply(v);
  };

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
        aria-label={`给 ${targets} 项打标签`}
        onMouseDown={(e) => e.stopPropagation()}
        initial={{ opacity: 0, scale: 0.96, y: 12 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        transition={springSnappy}
      >
        <div className="dlg-title tag-prompt-title">
          <Icon name="tag" size={14} stroke={1.5} />
          给 {targets} 项打标签
        </div>

        <div className="tag-prompt-field" ref={inputRef}>
          <BlinkCaretInput
            autoFocus
            value={tag}
            onChange={setTag}
            placeholder="输入标签名，回车应用"
            inputClassName="tag-prompt-input"
            ariaLabel="标签名"
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                apply();
              }
            }}
          />
        </div>

        {picks.length > 0 && (
          <div className="tag-prompt-picks">
            <div className="tag-prompt-hint">已有的标签</div>
            {picks.map((t) => (
              <button
                key={t}
                className="tag-chip pick"
                onClick={() => {
                  onApply(t);
                }}
              >
                {t}
              </button>
            ))}
          </div>
        )}

        <p className="dlg-hint">
          标签只记在软件里，不改文件名、不改目录；同一个标签再打一次不会重复。
        </p>

        <div className="dlg-actions">
          <button className="btn" onClick={onClose}>
            取消
          </button>
          <button className="btn btn-accent" disabled={!tag.trim()} onClick={apply} autoFocus>
            打上「{tag.trim() || "…"}」
          </button>
        </div>
      </motion.div>
    </motion.div>
  );
}
