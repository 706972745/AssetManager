import { useEffect, useRef, useState } from "react";
import type { CollRow } from "../types";
import { DOCK_LEAVE_DELAY, getActiveDock, setActiveDock, useDockHover } from "../lib/dockHover";
import { setDockOpen, useDockOpen } from "../lib/dockOpen";
import { BlinkCaretInput, Icon } from "./ui";

/**
 * 侧栏「分组」停靠面板。**壳与行逐项照 FontManager**（owner 2026-11-18：
 * 「分组和标签的…要和 FontManager 一模一样」）：
 *
 * - 与标签面板同一个壳（`.tag-dock` 全套类复用）：头部常驻 32 高、`order:2` 钉在底栏上方；
 * - 展开由全局 `dockHover` 单值驱动、离开延迟 200ms、110ms 高度过渡；
 *   两块都开时各让一步（`.half` → `min(220px,30vh)`）；
 * - 行：`tag-row coll-row` —— 文件夹图标 13 + **序号** + 名称 + 行尾数字，25px 高 / 内距 0 7px；
 * - 新建 / 改名走 FontManager 的 `.collection-input`（32 高、强调色描边 + 3px 光晕）。
 *
 * 与「标签」的分工：标签是**属性**（一张图可以有多个），分组是**收纳**（要点名看的那一批）。
 */
const PIN_KEY = "am.coll.pinned.v1";
const DOCK_ID = "collections";

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
  renameId,
  hold,
}: {
  items: CollRow[];
  activeId: number | null;
  onPick: (id: number | null) => void;
  /** 就地输入建组：回车确定 / Esc 取消 / 空名不建 */
  onCreate: (name: string) => void;
  onRename: (id: number, name: string) => void;
  /** 右键：交给 App 弹自己的菜单（改名 / 从分组建快照 / 只看 / 删除） */
  onMenu: (row: CollRow, x: number, y: number) => void;
  /**
   * 让某一行**进入就地改名**（右键菜单「重命名」用）。
   * 改名状态本来住在组件内部（双击行也走它），外面只要把这个 id 递进来即可。
   */
  renameId?: number | null;
  hold?: boolean;
  /** @deprecated 由 dockOpen 自动判定两块是否同开，调用点不用再传 */
  half?: boolean;
}) {
  const [pinned, setPinned] = useState(loadPinned);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  const [renaming, setRenaming] = useState<{ id: number; name: string } | null>(null);
  const [q, setQ] = useState("");
  const [focusHold, setFocusHold] = useState(false);
  const addRef = useRef<HTMLDivElement>(null);
  const renameRef = useRef<HTMLDivElement>(null);
  const leaveTimer = useRef<number | null>(null);

  const activeDock = useDockHover();
  const dockOpen = useDockOpen();

  /* 右键菜单点了「重命名」→ 外部把 id 递进来，这里进就地输入。
     面板的 open 条件里已经算上 `renaming !== null`，所以它会自己展开。

     用 `consumedRename` 保证**只消费一次**：`items` 每次刷新都是新数组，
     光靠 `[renameId, items]` 会让改名提交后（列表刷新）又进一次改名状态。 */
  const consumedRename = useRef<number | null>(null);
  useEffect(() => {
    if (renameId == null || consumedRename.current === renameId) return;
    const c = items.find((v) => v.id === renameId);
    if (!c) return;
    consumedRename.current = renameId;
    setRenaming({ id: c.id, name: c.name });
  }, [renameId, items]);

  const dockEnter = () => {
    if (leaveTimer.current !== null) {
      window.clearTimeout(leaveTimer.current);
      leaveTimer.current = null;
    }
    setActiveDock(DOCK_ID);
  };
  const dockLeave = () => {
    if (leaveTimer.current !== null) window.clearTimeout(leaveTimer.current);
    leaveTimer.current = window.setTimeout(() => {
      leaveTimer.current = null;
      if (getActiveDock() === DOCK_ID) setActiveDock(null);
    }, DOCK_LEAVE_DELAY);
  };
  useEffect(
    () => () => {
      if (leaveTimer.current !== null) window.clearTimeout(leaveTimer.current);
    },
    [],
  );

  /** 看某个分组时面板保持展开（不改用户的固定开关） */
  const selectedHold = activeId !== null;
  const open = pinned || selectedHold || activeDock === DOCK_ID || !!hold || focusHold || adding || renaming !== null;
  const showingPinned = pinned || selectedHold;
  const half = open && dockOpen.tags;

  useEffect(() => {
    setDockOpen("collections", open);
    return () => setDockOpen("collections", false);
  }, [open]);

  useEffect(() => {
    if (!open) setQ((v) => (v === "" ? v : ""));
  }, [open]);

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
    if (selectedHold && !pinned) {
      onPick(null);
      return;
    }
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
    <div
      className={`tag-dock coll-dock ${open ? "open" : ""} ${pinned ? "pinned" : ""} ${half ? "half" : ""}`}
      onMouseEnter={dockEnter}
      onMouseLeave={dockLeave}
    >
      <div
        className="tag-dock-head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        aria-label={showingPinned ? "取消固定并收起分组面板" : "固定展开分组面板"}
        title={showingPinned ? "已固定：点一下取消固定" : "点一下固定展开（不点则随鼠标离开自动收起）"}
        onClick={togglePin}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            togglePin();
          }
        }}
      >
        <Icon name="folder-open" size={15} stroke={1.5} className="tag-dock-icon" />
        <span className="tag-dock-title">分组</span>
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
              <Icon name="close" size={9} stroke={2.4} />
            </span>
          </span>
        ) : (
          items.length > 0 && <span className="tag-dock-count tabular">{items.length}</span>
        )}
        <span className={`tag-dock-caret ${open ? "" : "closed"}`}>
          <Icon name="chevron" size={10} stroke={2} />
        </span>
      </div>

      <div className="tag-dock-body">
        <div className="tag-dock-tools">
          <BlinkCaretInput
            value={q}
            onChange={setQ}
            placeholder="过滤分组…"
            fieldClassName="tag-dock-search"
            ariaLabel="过滤分组"
            onFocus={() => setFocusHold(true)}
            onBlur={() => setFocusHold(false)}
          />
          <button
            className={`tag-tool-btn ${showingPinned ? "on" : ""}`}
            title={showingPinned ? "取消固定（收起后随鼠标离开自动收起）" : "固定展开（鼠标离开也不收）"}
            aria-pressed={showingPinned}
            onClick={togglePin}
          >
            <span className="tag-pin-dot" aria-hidden />
            <span>{showingPinned ? "已固定" : "固定"}</span>
          </button>
          <button
            className="tag-tool-btn"
            title="新建分组"
            aria-label="新建分组"
            onClick={() => {
              setAdding(true);
              setDraft("");
            }}
          >
            <Icon name="plus" size={11} stroke={1.8} />
            <span>分组</span>
          </button>
        </div>

        <div className="tag-dock-scroll scroll-hot" role="listbox" aria-label="分组">
          {adding && (
            <div className="collection-input" ref={addRef}>
              <Icon name="folder" size={15} stroke={1.5} />
              <BlinkCaretInput
                value={draft}
                onChange={setDraft}
                placeholder="新分组名，回车建立"
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

          {shown.map((c, ci) =>
            renaming?.id === c.id ? (
              <div className="collection-input" key={c.id} ref={renameRef}>
                <Icon name="folder-open" size={15} stroke={1.5} />
                <BlinkCaretInput
                  value={renaming.name}
                  onChange={(v) => setRenaming({ id: c.id, name: v })}
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
                className={`tag-row coll-row ${activeId === c.id ? "sel" : ""}`}
                onClick={() => onPick(activeId === c.id ? null : c.id)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onPick(activeId === c.id ? null : c.id);
                  } else if (e.key === "F2") {
                    e.preventDefault();
                    setRenaming({ id: c.id, name: c.name });
                  }
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
                  <Icon name="folder-open" size={13} stroke={1.6} />
                </span>
                <span className="coll-no tabular">{ci + 1}</span>
                <span className="tag-row-name">{c.name}</span>
                <span className="tag-row-count tabular">{c.count}</span>
              </div>
            ),
          )}

          {items.length === 0 && !adding && (
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
