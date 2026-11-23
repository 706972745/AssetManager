import { memo, useEffect, useRef, useState } from "react";
import type { CollRow } from "../types";
import { DOCK_LEAVE_DELAY, getActiveDock, setActiveDock, useDockHover } from "../lib/dockHover";
import { setDockOpen, useDockOpen } from "../lib/dockOpen";
import { BlinkCaretInput, Icon } from "./ui";
import { useT } from "../lib/i18n";

/**
 * 侧栏「分组」停靠面板。**壳与行逐项照 FontManager**（owner 2026-11-18：
 * 「分组和标签的…要和 FontManager 一模一样」）：
 *
 * - 与标签面板同一个壳（`.tag-dock` 全套类复用）：头部常驻 32 高、`order:2` 钉在底栏上方；
 * - 展开由全局 `dockHover` 单值驱动、离开延迟 90ms、110ms 高度过渡；
 *   两块都开时各让一步（`.half` → `min(220px,30vh)`）；
 * - 行：`tag-row coll-row` —— 文件夹图标 13 + **序号** + 名称 + 行尾数字，25px 高 / 内距 0 7px；
 * - 新建 / 改名走 FontManager 的 `.collection-input`（32 高、强调色描边 + 3px 光晕）。
 *
 * 与「标签」的分工：标签是**属性**（一张图可以有多个），分组是**收纳**（要点名看的那一批）。
 */
const DOCK_ID = "collections";

/* 固定开关**只在本次会话有效**，不再写 localStorage（owner 2026-11-21）：
   - 以前把 pinned 存成 `am.coll.pinned.v1`，于是"装完第一次打开就是展开的"，
     想让它收起还得再点一下头部 —— 用户要的是「默认收起、不固定」；
   - 那个键还被自动化脚本写过 "1"（验证点标签不切库时），脏值会一直跟着同一个人，
     换键只是把脏值藏起来，不如干脆不持久化。
   想要一直展开就点一下头部，鼠标离开会收，重启也回到收起。 */

function CollDockImpl({
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
  const t = useT();
  const [pinned, setPinned] = useState(false);
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
      /* 拖拽期间**不清**：面板展开/收起会让指针"离开"再"回来"，而拖拽时鼠标停住
         不产生 mousemove，这个延迟一到就会把刚展开的面板收回去 ——
         表现就是"展开收起反复横跳、怎么也放不进去"（owner 2026-11-20）。
         拖拽结束由 App 的 clearDrag 统一清空。 */
      if (document.body.classList.contains("am-dragging")) return;
      // 只在「还是自己」的时候清：期间指针可能已经进了另一个面板
      if (getActiveDock() === DOCK_ID) setActiveDock(null);
    }, DOCK_LEAVE_DELAY);
  };
  useEffect(
    () => () => {
      if (leaveTimer.current !== null) window.clearTimeout(leaveTimer.current);
    },
    [],
  );

  /* 0.5.11（owner）：**没有任何"自动固定"** —— 选中的分组不再把面板撑开，
     展开只来自三件"手上正在做的事"：悬停（activeDock）、拖拽/菜单 hold、面板内聚焦；
     常驻只看用户自己点的「固定」。以前 `selectedHold` 会让面板在选中项存在时一直开着，
     鼠标走了也收不回去，看起来就像"自动固定"。 */
  const open = pinned || activeDock === DOCK_ID || !!hold || focusHold || adding || renaming !== null;
  const showingPinned = pinned;
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
    setPinned(!pinned);
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
        aria-label={showingPinned ? t("dock.head_aria_unpin", { panel: t("dock.coll.panel") }) : t("dock.head_aria_pin", { panel: t("dock.coll.panel") })}
        title={showingPinned ? t("dock.head_title_pinned") : t("dock.head_title")}
        /* 拖放到**头部** = 投到当前筛选的分组（没有就投第一个）。头部不接落点的话，
           拖到「分组」这条上松手就是毫无反应（owner 2026-11-20 复测）。 */
        data-drop-head="collections"
        onClick={togglePin}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            togglePin();
          }
        }}
      >
        <Icon name="folder-open" size={15} stroke={1.5} className="tag-dock-icon" />
        <span className="tag-dock-title">{t("dock.coll.panel")}</span>
        {/* 0.4.24：点分组不再切库，只在**当前素材库**里看这一撮。这个标记就是让用户
            一眼知道范围 —— 不然"为什么只有 8 张"永远要猜（owner 2026-11-21）。 */}
        <span className="tag-dock-scope" title={t("dock.scope_tip")}>
          {t("dock.scope")}
        </span>
        {activeId !== null ? (
          <span className="tag-dock-filter">
            <span className="tag-dock-filter-name">
              {items.find((x) => x.id === activeId)?.name ?? t("dock.coll.panel")}
            </span>
            <span
              className="tag-dock-filter-x"
              role="button"
              tabIndex={0}
              aria-label={t("dock.coll.clear_filter_aria")}
              title={t("dock.clear_filter")}
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
            placeholder={t("dock.coll.filter_placeholder")}
            fieldClassName="tag-dock-search"
            ariaLabel={t("dock.coll.filter_aria")}
            onFocus={() => setFocusHold(true)}
            onBlur={() => setFocusHold(false)}
          />
          <button
            className={`tag-tool-btn ${showingPinned ? "on" : ""}`}
            title={showingPinned ? t("dock.pin_tip_on") : t("dock.pin_tip_off")}
            aria-pressed={showingPinned}
            onClick={togglePin}
          >
            <span className="tag-pin-dot" aria-hidden />
            <span>{showingPinned ? t("dock.pinned") : t("dock.pin")}</span>
          </button>
          <button
            className="tag-tool-btn"
            title={t("dock.coll.new")}
            aria-label={t("dock.coll.new")}
            onClick={() => {
              setAdding(true);
              setDraft("");
            }}
          >
            <Icon name="plus" size={11} stroke={1.8} />
            <span>{t("dock.coll.new_short")}</span>
          </button>
        </div>

        <div className="tag-dock-scroll scroll-hot" role="listbox" aria-label={t("dock.coll.panel")}>
          {adding && (
            <div className="collection-input" ref={addRef}>
              <Icon name="folder" size={15} stroke={1.5} />
              <BlinkCaretInput
                value={draft}
                onChange={setDraft}
                placeholder={t("dock.coll.new_placeholder")}
                ariaLabel={t("dock.coll.new_aria")}
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
                  ariaLabel={t("dock.coll.name_aria")}
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
                /* 拖放落点：拖到这里 = 把素材加进这个分组（只建关联，文件不动） */
                data-drop-coll={c.id}
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
                title={t("dock.coll.row_tip", { name: c.name, n: c.count })}
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
              {t("dock.coll.empty_lead")} <b>+ {t("dock.coll.new_short")}</b>{" "}
              {t("dock.coll.empty_tail")}
            </div>
          )}
          {needle && shown.length === 0 && items.length > 0 && (
            <div className="tag-dock-empty">{t("dock.coll.no_match", { q: q.trim() })}</div>
          )}
        </div>
      </div>
    </div>
  );
}

/* memo 边界：见 TagDock 同款说明 —— 回调稳定化在 App（`useEvent`）做。 */
export const CollDock = memo(CollDockImpl);
