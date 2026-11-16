import { useEffect, useRef, useState } from "react";
import { motion } from "motion/react";
import type { AutoRule, TagGroup } from "../types";
import { COLOR_LABEL, TAG_COLORS, colorClass } from "../lib/tags";
import { BlinkCaretInput, Icon } from "./ui";
import { scrimFade, springSnappy } from "../design/springs";

/**
 * 标签 / 分组的编辑卡（0.5.0）。
 *
 * 为什么两种东西共用一张卡：用户心里「标签」和「标签组」是同一件事的两个字段
 * （颜色 + 归到哪一堆），分成两个弹窗只会让人来回找入口。差别只有两处：
 * 分组没有「所属分组」，标签有；删除按钮的文案不同。
 *
 * 改动**即时生效**（点色板就变、点分组就归位），没有「保存」——
 * 这些操作都可以一键改回来，做一层草稿反而让人怀疑「我到底存没存」。
 * 只有改名走失焦/回车提交，避免边打字边改库。
 */
export function TagEdit({
  kind,
  name,
  color,
  group,
  groups,
  onRename,
  onColor,
  onGroup,
  onDelete,
  onClose,
  rule,
  onRule,
}: {
  kind: "tag" | "group";
  name: string;
  color: string | null;
  /** 标签所属分组（kind=group 时无意义） */
  group: string | null;
  groups: TagGroup[];
  onRename: (next: string) => void;
  onColor: (color: string | null) => void;
  onGroup: (group: string | null) => void;
  onDelete: () => void;
  onClose: () => void;
  /** T34 自动规则（只有标签有；分组没有） */
  rule: AutoRule | null;
  onRule: (r: AutoRule | null) => void;
}) {
  const [draft, setDraft] = useState(name);
  /* 自动规则的草稿：扩展名用逗号串编辑（用户心智是"写一串扩展名"，不是点标签） */
  const [r, setR] = useState<AutoRule>(
    rule ?? { dir: "", ext: [], kind: [], sizeMin: 0, sizeMax: 0, pxMin: 0, pxMax: 0, needDims: false }
  );
  const [extText, setExtText] = useState((rule?.ext ?? []).join(", "));
  const [mbMin, setMbMin] = useState(rule && rule.sizeMin > 0 ? Math.round(rule.sizeMin / 1048576) : 0);
  const [creating, setCreating] = useState(false);
  const [newGroup, setNewGroup] = useState("");
  const newRef = useRef<HTMLDivElement>(null);

  useEffect(() => setDraft(name), [name]);
  useEffect(() => {
    if (!creating) return;
    const t = window.setTimeout(() => newRef.current?.querySelector("input")?.focus(), 30);
    return () => window.clearTimeout(t);
  }, [creating]);

  const commitName = () => {
    const v = draft.trim();
    if (!v || v === name) {
      setDraft(name);
      return;
    }
    onRename(v);
  };

  const commitNewGroup = () => {
    const v = newGroup.trim();
    setCreating(false);
    setNewGroup("");
    if (v) onGroup(v);
  };

  const isTag = kind === "tag";

  return (
    <motion.div
      className="dlg-scrim"
      onMouseDown={onClose}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={scrimFade}
    >
      <motion.div
        className="dlg tag-edit"
        role="dialog"
        aria-modal="true"
        aria-label={isTag ? `编辑标签 ${name}` : `编辑分组 ${name}`}
        onMouseDown={(e) => e.stopPropagation()}
        initial={{ opacity: 0, scale: 0.96, y: 12 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        transition={springSnappy}
      >
        <div className="dlg-title tag-edit-title">
          <Icon name="tag" size={14} stroke={1.5} />
          {isTag ? "标签" : "分组"}
        </div>

        <div className="tag-edit-field">
          <BlinkCaretInput
            autoFocus
            value={draft}
            onChange={setDraft}
            placeholder={isTag ? "标签名" : "分组名"}
            inputClassName="tag-edit-input"
            ariaLabel={isTag ? "标签名" : "分组名"}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commitName();
              } else if (e.key === "Escape") {
                e.stopPropagation();
                onClose();
              }
            }}
            onBlur={commitName}
          />
        </div>

        <div className="tag-edit-label">颜色</div>
        <div className="tag-swatches">
          <button
            className={`tag-swatch tc-gray ${!color || color === "gray" ? "on" : ""}`}
            onClick={() => onColor(null)}
            title="默认（灰）"
            aria-label="默认颜色"
          />
          {TAG_COLORS.filter((c) => c !== "gray").map((c) => (
            <button
              key={c}
              className={`tag-swatch tc-${c} ${color === c ? "on" : ""}`}
              onClick={() => onColor(c)}
              title={COLOR_LABEL[c] ?? c}
              aria-label={COLOR_LABEL[c] ?? c}
            />
          ))}
        </div>

        {isTag && (
          <>
            <div className="tag-edit-label">
              分组
              <span className="tag-edit-sub">只影响侧栏怎么摆，不移动文件</span>
            </div>
            <div className="tag-edit-groups">
              <button
                className={`tag-chip pick ${!group ? "on" : ""}`}
                onClick={() => onGroup(null)}
              >
                未分组
              </button>
              {groups.map((g) => (
                <button
                  key={g.name}
                  className={`tag-chip pick ${group === g.name ? "on" : ""}`}
                  onClick={() => onGroup(g.name)}
                >
                  <span className={`tag-dot ${colorClass(g.color)}`} aria-hidden />
                  {g.name}
                </button>
              ))}
              {!creating && (
                <button
                  className="tag-chip pick new"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => setCreating(true)}
                >
                  + 新建分组
                </button>
              )}
            </div>
            {creating && (
              <div className="tag-edit-newgroup" ref={newRef}>
                <BlinkCaretInput
                  autoFocus
                  value={newGroup}
                  onChange={setNewGroup}
                  placeholder="新分组名，回车加入"
                  inputClassName="tag-edit-input"
                  ariaLabel="新分组名"
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitNewGroup();
                    else if (e.key === "Escape") {
                      e.stopPropagation();
                      setCreating(false);
                      setNewGroup("");
                    }
                  }}
                  onBlur={commitNewGroup}
                />
              </div>
            )}
          </>
        )}

        {isTag && (
          <div className="tag-rule">
            <div className="tag-rule-head">
              <span>自动规则</span>
              <span className="tag-rule-hint">
                命中条件的素材自动算作有这个标签；规则是"活"的，不写进索引
              </span>
            </div>
            <div className="tag-rule-grid">
              <label>
                <span>目录包含</span>
                <input
                  className="mono"
                  value={r.dir}
                  placeholder="如 客户/2026"
                  onChange={(e) => setR({ ...r, dir: e.target.value })}
                />
              </label>
              <label>
                <span>扩展名</span>
                <input
                  className="mono"
                  value={extText}
                  placeholder="psd, ai, psb"
                  onChange={(e) => setExtText(e.target.value)}
                />
              </label>
              <label>
                <span>最小体积 MB</span>
                <input
                  type="number"
                  min={0}
                  className="mono"
                  value={mbMin}
                  onChange={(e) => setMbMin(Number(e.target.value) || 0)}
                />
              </label>
              <label>
                <span>最小长边 px</span>
                <input
                  type="number"
                  min={0}
                  className="mono"
                  value={r.pxMin}
                  onChange={(e) => setR({ ...r, pxMin: Number(e.target.value) || 0 })}
                />
              </label>
            </div>
            <div className="tag-rule-actions">
              <button
                className="btn btn-tiny"
                onClick={() => {
                  const ext = extText
                    .split(/[,，\s]+/)
                    .map((x) => x.trim().toLowerCase().replace(/^\./, ""))
                    .filter(Boolean);
                  onRule({
                    ...r,
                    ext,
                    sizeMin: mbMin > 0 ? mbMin * 1048576 : 0,
                  });
                }}
              >
                保存规则
              </button>
              {rule && (
                <button className="btn btn-tiny" onClick={() => onRule(null)}>
                  清除规则
                </button>
              )}
            </div>
          </div>
        )}

        <p className="dlg-hint">
          {isTag
            ? "颜色和分组只改显示，不改文件名、不动磁盘；自动标签（格式/类型/像素/比例/体积）不能在这里改。"
            : "解散分组不会删除标签：组里的标签会回到「未分组」，素材上的标签一条都不会少。"}
        </p>

        <div className="dlg-actions">
          <button className="btn btn-danger" onClick={onDelete}>
            {isTag ? "删除标签" : "解散分组"}
          </button>
          <span className="grow" />
          <button className="btn btn-accent" onClick={onClose}>
            完成
          </button>
        </div>
      </motion.div>
    </motion.div>
  );
}
