import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { motion } from "motion/react";
import type { AutoRule, TagGroup } from "../types";
import { COLOR_LABEL, TAG_COLORS, colorClass } from "../lib/tags";
import { useT } from "../lib/i18n";
import { BlinkCaretInput, Icon } from "./ui";
import { springSoft } from "../design/springs";

/**
 * 标签 / 分组的编辑卡（0.5.0；0.1.28 照 FontManager 改成**跟随鼠标的浮层**）。
 *
 * 为什么改形态：以前是带遮罩的居中模态框（`.dlg-scrim` + `.dlg`），
 * owner 2026-11-18 指出「这个和之前（FontManager）的也不对」——
 * FontManager 的是一张 236px 宽的小卡，**出现在你右键的那个位置旁边**，
 * 不遮界面、点外面就关。现在按它做：`position: fixed` + `left/top` 由调用方给的坐标算。
 *
 * 纪律保持不变：**改动即时生效，没有「保存」**（每项都能一键改回来），
 * 只有改名走回车 / 失焦提交，避免边打字边写库。
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
  x,
  y,
  readOnly = false,
  extra,
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
  /** 弹出位置（视口坐标，通常是右键点 / 按钮中心） */
  x: number;
  y: number;
  /** 自动标签：只能看，不能改名 / 改色 / 删除（它是按规则算出来的） */
  readOnly?: boolean;
  /** 卡片底部的额外动作（只看这个标签 / 灵感板 / 快照 …） */
  extra?: ReactNode;
}) {
  const t = useT();
  const [draft, setDraft] = useState(name);
  /* 自动规则的草稿：扩展名用逗号串编辑（用户心智是"写一串扩展名"，不是点标签） */
  const [r, setR] = useState<AutoRule>(
    rule ?? { dir: "", ext: [], kind: [], sizeMin: 0, sizeMax: 0, pxMin: 0, pxMax: 0, needDims: false }
  );
  const [extText, setExtText] = useState((rule?.ext ?? []).join(", "));
  const [mbMin, setMbMin] = useState(rule && rule.sizeMin > 0 ? Math.round(rule.sizeMin / 1048576) : 0);
  const [showRule, setShowRule] = useState(false);
  const [creating, setCreating] = useState(false);
  const [newGroup, setNewGroup] = useState("");
  const boxRef = useRef<HTMLDivElement>(null);
  const newRef = useRef<HTMLDivElement>(null);

  useEffect(() => setDraft(name), [name]);
  useEffect(() => {
    if (!creating) return;
    const timer = window.setTimeout(() => newRef.current?.querySelector("input")?.focus(), 30);
    return () => window.clearTimeout(timer);
  }, [creating]);

  /* 点外面关（浮层没有遮罩，这是唯一的"取消"手势之一） */
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        onClose();
      }
    };
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [onClose]);

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
  /* 位置：优先"右下方贴着点击点"，放不下就往左/往上翻（不能只看固定尺寸猜，
     卡片高度随内容变：标签卡有分组和规则，分组卡没有）。 */
  const [pos, setPos] = useState({ left: x - 212, top: y - 6 });
  useLayoutEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    setPos({
      left: Math.max(8, Math.min(x - w + 24, window.innerWidth - w - 8)),
      top: Math.max(8, Math.min(y - 6, window.innerHeight - h - 8)),
    });
  }, [x, y, rule, creating, readOnly]);

  return (
    <motion.div
      ref={boxRef}
      className="tag-edit"
      style={pos}
      role="dialog"
      aria-label={isTag ? t("dock.tags.edit_aria", { name }) : t("dock.group_edit_aria", { name })}
      initial={{ opacity: 0, y: -4, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={springSoft}
    >
      <div className="tag-edit-head">
        <Icon name={isTag ? "tag" : "folder"} size={12} stroke={1.6} />
        <span>{isTag ? t("tag.edit_tag") : t("tag.edit_group")}</span>
      </div>

      <div className="tag-edit-label">{t("tag.name")}</div>
      {readOnly ? (
        <div className="tag-edit-ro">{name}</div>
      ) : (
      <BlinkCaretInput
        autoFocus
        value={draft}
        onChange={setDraft}
        placeholder={isTag ? t("chrome.tag_name_aria") : t("tag.group_name")}
        inputClassName="tag-edit-input"
        ariaLabel={isTag ? t("chrome.tag_name_aria") : t("tag.group_name")}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commitName();
          }
        }}
        onBlur={commitName}
      />
      )}

      <div className="tag-edit-label">{t("tag.color")}</div>
      <div className={`tag-swatches ${readOnly ? "ro" : ""}`}>
        <button
          className={`tag-swatch tc-gray ${!color || color === "gray" ? "on" : ""}`}
          onClick={() => onColor(null)}
          title={t("tag.default_gray")}
          aria-label={t("tag.default_color")}
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
          <div className="tag-edit-label">{t("app.common.group")}</div>
          <div className="tag-edit-groups">
            <button className={`tag-chip pick ${!group ? "on" : ""}`} onClick={() => onGroup(null)}>
              {t("dock.tags.ungrouped")}
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
                + {t("tag.new_group")}
              </button>
            )}
          </div>
          {creating && (
            <div className="tag-edit-newgroup" ref={newRef}>
              <BlinkCaretInput
                autoFocus
                value={newGroup}
                onChange={setNewGroup}
                placeholder={t("tag.new_group_ph")}
                inputClassName="tag-edit-input"
                ariaLabel={t("tag.new_group_aria")}
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

          <button
            className={`tag-edit-fold ${showRule ? "open" : ""}`}
            onClick={() => setShowRule((v) => !v)}
            aria-expanded={showRule}
          >
            <Icon name="chevron" size={9} stroke={2} />
            {t("tag.auto_rule")}
            {rule ? t("tag.auto_rule_on") : ""}
          </button>
          {showRule && (
          <div className="tag-rule">
            <div className="tag-rule-grid">
              <label>
                <span>{t("tag.rule_dir")}</span>
                <input
                  className="mono"
                  value={r.dir}
                  placeholder={t("tag.rule_dir_ph")}
                  onChange={(e) => setR({ ...r, dir: e.target.value })}
                />
              </label>
              <label>
                <span>{t("tag.rule_ext")}</span>
                <input
                  className="mono"
                  value={extText}
                  placeholder="psd, ai, psb"
                  onChange={(e) => setExtText(e.target.value)}
                />
              </label>
              <label>
                <span>{t("tag.rule_mb")}</span>
                <input
                  type="number"
                  min={0}
                  className="mono"
                  value={mbMin}
                  onChange={(e) => setMbMin(Number(e.target.value) || 0)}
                />
              </label>
              <label>
                <span>{t("tag.rule_px")}</span>
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
                {t("tag.rule_save")}
              </button>
              {rule && (
                <button className="btn btn-tiny" onClick={() => onRule(null)}>
                  {t("tag.rule_clear")}
                </button>
              )}
            </div>
          </div>
          )}
        </>
      )}

      {extra}

      <p className="tag-edit-hint">
        {readOnly
          ? t("tag.hint_auto")
          : isTag
            ? t("tag.hint_tag")
            : t("tag.hint_group")}
      </p>

      <div className="tag-edit-foot">
        <button className="tag-edit-danger" onClick={onDelete} disabled={readOnly}>
          {isTag ? t("tag.delete_tag") : t("tag.delete_group")}
        </button>
        <button className="tag-edit-done" onClick={onClose}>
          {t("common.done")}
        </button>
      </div>
    </motion.div>
  );
}
