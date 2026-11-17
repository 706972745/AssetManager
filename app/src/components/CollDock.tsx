import { useEffect, useRef, useState } from "react";
import type { CollRow } from "../types";
import { BlinkCaretInput, Icon } from "./ui";

/**
 * 分组面板（0.2.x）。**位置和样式照 FontManager 的「分组」停靠面板**（owner 2026-11-17）：
 *
 * - 位置：侧栏里**紧贴底栏上方、在「标签」面板之上**（顺序 = 分组 → 标签）。
 * - 壳子：和标签面板同一套 `.tag-dock`（头部常驻 32 高、悬停展开、点标题固定、带过滤框）。
 * - 行：文件夹图标 + 名字 + 行尾数量；选中是同一套 `.tree-row.sel`（2px 蓝竖线）。
 *
 * 与「标签」的分工（照 FontManager）：标签是**属性**（一张图可以有多个），
 * 分组是**收纳**（用户点名要看的那一批），所以分组在侧栏、标签在面板里。
 */
const PIN_KEY = "am.coll.pinned.v1";

function loadPinned(): boolean {
  try {
    return localStorage.getItem(PIN_KEY) === "1";
  } catch {
    return false;
  }
}
function savePinned(v: boolean) {
  try {
    localStorage.setItem(PIN_KEY, v ? "1" : "0");
  } catch {
    /* 存不进去就本次会话有效 */
  }
}

export function CollDock({
  items,
  activeId,
  onPick,
  onCreate,
  onRename,
  onMenu,
  hold,
  /** 另一方（标签面板）开着时各让一步，别把目录树挤没了 */
  half,
}: {
  items: CollRow[];
  activeId: number | null;
  onPick: (id: number | null) => void;
  /** 就地输入建组：回车确定 / Esc 取消 / 空名不建 */
  onCreate: (name: string) => void;
  onRename: (id: number, name: string) => void;
  /** 右键：交给 App 弹自己的菜单（改名 / 删除） */
  onMenu: (row: CollRow, x: number, y: number) => void;
  hold?: boolean;
  half?: boolean;
}) {
  const [pinned, setPinned] = useState(loadPinned);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  const [renaming, setRenaming] = useState<{ id: number; name: string } | null>(null);
  const [q, setQ] = useState("");
  const addRef = useRef<HTMLDivElement>(null);
  const renameRef = useRef<HTMLDivElement>(null);

  const open = pinned || !!hold;

  useEffect(() => {
    if (!adding) return;
    const t = window.setTimeout(() => addRef.current?.querySelector("input")?.focus(), 40);
    return () => window.clearTimeout(t);
  }, [adding]);
  useEffect(() => {
    if (!renaming) return;
    const t = window.setTimeout(() => renameRef.current?.querySelector("input")?.focus(), 40);
    return () => window.clearTimeout(t);
  }, [renaming]);

  const togglePin = () => {
    const next = !pinned;
    setPinned(next);
    savePinned(next);
  };

  const commitCreate = () => {
    const v = draft.trim();
    setAdding(false);
    setDraft("");
    if (v) onCreate(v);
  };
  const commitRename = () => {
    const cur = renaming;
    setRenaming(null);
    if (cur && cur.name.trim() && cur.name.trim() !== items.find((x) => x.id === cur.id)?.name) {
      onRename(cur.id, cur.name.trim());
    }
  };

  const needle = q.trim().toLowerCase();
  const shown = needle ? items.filter((x) => x.name.toLowerCase().includes(needle)) : items;

  return (
    <div className={`tag-dock coll-dock ${open ? "open" : ""} ${pinned ? "pinned" : ""} ${open && half ? "half" : ""}`}>
      <div
        className="tag-dock-head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        aria-label={pinned ? "取消固定并收起分组面板" : "固定展开分组面板"}
        title={pinned ? "已固定：点一下取消固定" : "点一下固定展开（不点则随鼠标离开自动收起）"}
        onClick={togglePin}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            togglePin();
          }
        }}
      >
        <Icon name="folder-open" size={15} stroke={1.5} className="tag-dock-icon" />
        <span className="grow">分组</span>
        {activeId !== null ? (
          <span className="tag-dock-filter">
            <span className="tag-dock-filter-name">
              {items.find((x) => x.id === activeId)?.name ?? "分组"}
            </span>
            <span
              className="tag-dock-filter-x"
              role="button"
              tabIndex={0}
              aria-label="清除分组筛选"
              title="清除筛选"
              onClick={(e) => {
                e.stopPropagation();
                onPick(null);
              }}
            >
              <Icon name="close" size={8} stroke={2.4} />
            </span>
          </span>
        ) : (
          items.length > 0 && <span className="tree-count">{items.length}</span>
        )}
        <span className={`tag-dock-caret ${open ? "" : "closed"}`}>
          <Icon name="chevron" size={9} stroke={2} className="rot90" />
        </span>
      </div>

      <div className="tag-dock-body">
        <div className="tag-dock-tools">
          {items.length > 8 && (
            <BlinkCaretInput
              value={q}
              onChange={setQ}
              placeholder="过滤分组…"
              inputClassName="tag-search"
              ariaLabel="过滤分组"
            />
          )}
          <button
            className={`tag-tool-btn ${pinned ? "on" : ""}`}
            title={pinned ? "取消固定（收起后随鼠标离开自动收起）" : "固定展开（鼠标离开也不收）"}
            aria-pressed={pinned}
            onClick={togglePin}
          >
            <span className="tag-pin-dot" aria-hidden />
            <span>{pinned ? "已固定" : "固定"}</span>
          </button>
          <button
            className="tag-tool-btn"
            title="新建分组"
            aria-label="新建分组"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              setAdding((v) => !v);
              setDraft("");
            }}
          >
            <Icon name="plus" size={11} stroke={1.8} />
            <span>分组</span>
          </button>
        </div>

        {adding && (
          <div className="tree-input-row tag-group-add" ref={addRef}>
            <span className="nav-icon">
              <Icon name="folder" size={13} stroke={1.5} />
            </span>
            <BlinkCaretInput
              autoFocus
              value={draft}
              onChange={setDraft}
              placeholder="新分组名，回车建立"
              inputClassName="tree-input"
              ariaLabel="新分组名"
              onKeyDown={(e) => {
                if (e.key === "Enter") commitCreate();
                else if (e.key === "Escape") {
                  e.stopPropagation();
                  setAdding(false);
                  setDraft("");
                }
              }}
              onBlur={commitCreate}
            />
          </div>
        )}

        <div className="tag-dock-scroll scroll-hot" role="listbox" aria-label="分组">
          {shown.map((c) =>
            renaming?.id === c.id ? (
              <div className="tree-input-row" key={c.id} ref={renameRef}>
                <span className="nav-icon">
                  <Icon name="folder" size={13} stroke={1.5} />
                </span>
                <BlinkCaretInput
                  autoFocus
                  value={renaming.name}
                  onChange={(v) => setRenaming({ id: c.id, name: v })}
                  inputClassName="tree-input"
                  ariaLabel="分组名"
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitRename();
                    else if (e.key === "Escape") {
                      e.stopPropagation();
                      setRenaming(null);
                    }
                  }}
                  onBlur={commitRename}
                />
              </div>
            ) : (
              <div
                key={c.id}
                role="option"
                tabIndex={0}
                aria-selected={activeId === c.id}
                className={`tree-row coll-row ${activeId === c.id ? "sel" : ""}`}
                onClick={() => {
                  /* 点了就自动固定：用户想看结果，面板随鼠标离开收起会像"点了没生效" */
                  if (!pinned) {
                    setPinned(true);
                    savePinned(true);
                  }
                  onPick(activeId === c.id ? null : c.id);
                }}
                onDoubleClick={(e) => {
                  e.stopPropagation();
                  setRenaming({ id: c.id, name: c.name });
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  onMenu(c, e.clientX, e.clientY);
                }}
                title={`${c.name}｜${c.count} 项；双击改名，右键更多`}
              >
                <span className="coll-icon">
                  <Icon name="folder-open" size={13} stroke={1.5} />
                </span>
                <span className="tree-name">{c.name}</span>
                <span className="tree-count">{c.count}</span>
              </div>
            )
          )}

          {items.length === 0 && (
            <div className="tag-dock-empty">
              还没有分组。选中素材后「加入分组」，或者点上面的 <b>+ 分组</b> 先建一个。
            </div>
          )}
          {needle && shown.length === 0 && items.length > 0 && (
            <div className="tag-dock-empty">没有匹配「{q.trim()}」的分组</div>
          )}
        </div>
      </div>
    </div>
  );
}
