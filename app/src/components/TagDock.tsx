import { useEffect, useRef, useState } from "react";
import type { TagCount, TagGroup } from "../types";
import { autoTagLabel, colorClass, groupAutoTags, isAutoTag } from "../lib/tags";
import { DOCK_LEAVE_DELAY, getActiveDock, setActiveDock, useDockHover } from "../lib/dockHover";
import { setDockOpen, useDockOpen } from "../lib/dockOpen";
import { BlinkCaretInput, Icon } from "./ui";

/**
 * 侧栏「标签」停靠面板。
 *
 * 形态与数值**逐项照 FontManager 2.3.x 的 `TagDock`**（owner 2026-11-18：
 * 「分组和标签的界面样式和展开高度要和 FontManager 一模一样，包括里面的显示效果」）：
 *
 * - 头部（`🏷 标签`）**永远可见、位置不动**，靠 `order: 2` 钉在底栏上方 —— 它才敢当点击目标；
 * - 悬停展开由 `lib/dockHover.ts` 的全局单值驱动（**不用 CSS `:hover`**：分组与标签上下相邻，
 *   各自 :hover 时从标签移到分组会让标签先收起、把分组推走，指针脱靶，分组永远展不开）；
 *   离开延迟 200ms；
 * - 展开/收起 110ms 高度过渡；单开 `min(300px,42vh)`，两块都开时各让一步 `min(220px,30vh)`；
 * - 行：25px 高 / 内距 0 7px / 圆角 5 / gap 7 / 11px 500（与侧栏 32px 的导航行**刻意不同**）；
 * - 选中 = `--ui-hover` 浅灰底 + 2px 竖线（配过色的标签竖线与行名跟标签色走）；
 * - 收起时头部仍显示「正在筛的标签」胶囊，筛选状态不会被藏起来；收起时清掉过滤词。
 */
const PIN_KEY = "am.tags.pinned.v1";
const CLOSED_KEY = "am.tags.groups.closed.v1";
const DOCK_ID = "tags";

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
function loadClosed(): Set<string> {
  try {
    const raw = localStorage.getItem(CLOSED_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(arr) ? arr.filter((x) => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}
function saveClosed(s: Set<string>) {
  try {
    localStorage.setItem(CLOSED_KEY, JSON.stringify([...s]));
  } catch {
    /* 同上 */
  }
}

export function TagDock({
  tags,
  groups,
  autoTags,
  activeTag,
  onPickTag,
  onEditTag,
  onEditGroup,
  onNewGroup,
  onNewTag,
  onTagMenu,
  onGroupMenu,
  hold,
}: {
  tags: TagCount[];
  groups: TagGroup[];
  autoTags: TagCount[];
  activeTag: string | null;
  onPickTag: (tag: string | null) => void;
  /** 打开标签编辑卡（改名 / 颜色 / 分组 / 删除） */
  onEditTag: (name: string) => void;
  onEditGroup: (name: string) => void;
  onNewGroup: (name: string) => void;
  /** 新建一个空标签（owner 2026-11-18：标签面板里该建标签，不是建分组） */
  onNewTag: (name: string) => void;
  /** 右键一条标签 → 交给 App 弹自己的菜单（不要整库那套） */
  onTagMenu: (tag: string, x: number, y: number) => void;
  onGroupMenu: (name: string, x: number, y: number) => void;
  /** 有弹层开着（编辑卡 / 批量打标签 / 右键菜单）→ 面板必须留在原地 */
  hold?: boolean;
}) {
  const [pinned, setPinned] = useState(loadPinned);
  const [closed, setClosed] = useState<Set<string>>(loadClosed);
  const [autoOpen, setAutoOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  const [q, setQ] = useState("");
  /** 过滤框聚焦就钉住：中文输入法的候选词窗是独立窗口，指针一进它 `:hover` 就断，
   *  面板会在打字途中收起（FontManager 2026-11-16 报过同样的现象）。 */
  const [focusHold, setFocusHold] = useState(false);
  const addRef = useRef<HTMLDivElement>(null);
  const leaveTimer = useRef<number | null>(null);

  const activeDock = useDockHover();
  const dockOpen = useDockOpen();

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

  /** 有标签正在筛时面板保持展开（不改用户的固定开关） */
  const selectedHold = activeTag !== null;
  const open = pinned || selectedHold || activeDock === DOCK_ID || !!hold || focusHold;
  /** 「已固定」的显示口径：手动固定的，或被选中项撑开的 */
  const showingPinned = pinned || selectedHold;
  /** 两块同时开 → 各让一步 */
  const half = open && dockOpen.collections;

  useEffect(() => {
    setDockOpen("tags", open);
    return () => setDockOpen("tags", false);
  }, [open]);

  /** 收起时清掉过滤词：否则下次展开看到一片「没有匹配」，像标签丢了 */
  useEffect(() => {
    if (!open) setQ((v) => (v === "" ? v : ""));
  }, [open]);

  useEffect(() => {
    if (!adding) return;
    const t = window.setTimeout(() => addRef.current?.querySelector("input")?.focus(), 40);
    return () => window.clearTimeout(t);
  }, [adding]);

  // Esc：先取消固定再收起（输入框里的 Esc 归输入框自己处理）
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !open) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA")) return;
      setPinned(false);
      savePinned(false);
      setFocusHold(false);
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [open]);

  const togglePin = () => {
    // 面板是被「选中的标签」撑开时，点头部的意思是收起它 —— 先取消筛选，
    // 否则点了没反应（pinned 本来是 false，切了也看不出变化）。
    if (selectedHold && !pinned) {
      onPickTag(null);
      return;
    }
    const next = !pinned;
    setPinned(next);
    savePinned(next);
  };

  const toggleGroup = (name: string) => {
    const n = new Set(closed);
    if (n.has(name)) n.delete(name);
    else n.add(name);
    setClosed(n);
    saveClosed(n);
  };

  /** 这里只建**标签**：分组有它自己的面板（CollDock），别在标签面板里建分组 */
  const commitTag = () => {
    const v = draft.trim();
    setAdding(false);
    setDraft("");
    if (v) onNewTag(v);
  };

  const colorOf = (name: string) => tags.find((t) => t.name === name)?.color ?? null;

  const needle = q.trim().toLowerCase();
  const hit = (s: string, label?: string) =>
    !needle || s.toLowerCase().includes(needle) || (label ?? "").toLowerCase().includes(needle);
  const shownTags = tags.filter((t) => hit(t.name));
  const shownAuto = autoTags.filter((t) => hit(t.name, autoTagLabel(t.name)));

  /** 组内排序：按使用数降序，同数按名字（「常用的浮前面」在组内也一样） */
  const sortIn = (list: TagCount[]) =>
    [...list].sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));

  const row = (t: TagCount, isAuto = false) => (
    <div
      key={`${isAuto ? "auto:" : ""}${t.name}`}
      className={`tag-row ${isAuto ? "auto" : ""} ${colorClass(t.color)} ${
        activeTag === t.name ? "sel" : ""
      }`}
      role="option"
      tabIndex={0}
      aria-selected={activeTag === t.name}
      onClick={() => onPickTag(activeTag === t.name ? null : t.name)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onPickTag(activeTag === t.name ? null : t.name);
        } else if (e.key === "F2" && !isAuto) {
          e.preventDefault();
          onEditTag(t.name);
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        /* 自动标签也要能弹（只是卡片里只读）—— 以前这里 `if (!isAuto)` 直接吞掉，
           用户右键自动标签时"什么都没发生"，报的就是「标签右击菜单打不开」。 */
        onTagMenu(t.name, e.clientX, e.clientY);
      }}
      title={`${isAuto ? autoTagLabel(t.name) : t.name} · ${t.count}`}
    >
      <span className="tag-dot" aria-hidden />
      <span className="tag-row-name">{isAuto ? autoTagLabel(t.name) : t.name}</span>
      {!isAuto && (
        <button
          className="tag-row-more"
          title="改颜色 / 分组 / 改名"
          aria-label={`编辑标签 ${t.name}`}
          onClick={(e) => {
            e.stopPropagation();
            onEditTag(t.name);
          }}
        >
          <Icon name="more" size={12} stroke={2} />
        </button>
      )}
      <span className="tag-row-count tabular">{t.count}</span>
    </div>
  );

  const ungrouped = sortIn(shownTags.filter((t) => !t.group));

  return (
    <div
      className={`tag-dock ${open ? "open" : ""} ${pinned ? "pinned" : ""} ${half ? "half" : ""}`}
      onMouseEnter={dockEnter}
      onMouseLeave={dockLeave}
    >
      {/* 头部永远在、且**位置不动**（CSS 用 order 把它钉在底栏上方）：
          所以它可以安全地当点击目标 —— 点一下 = 固定展开 / 收起。 */}
      <div
        className="tag-dock-head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        aria-label={showingPinned ? "取消固定并收起标签面板" : "固定展开标签面板"}
        title={showingPinned ? "已固定：点一下取消固定" : "点一下固定展开（不点则随鼠标离开自动收起）"}
        onClick={togglePin}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            togglePin();
          }
        }}
      >
        <Icon name="tag" size={15} stroke={1.5} className="tag-dock-icon" />
        <span className="tag-dock-title">标签</span>

        {/* 收起时也要看得见「正在按什么筛」，否则状态被藏起来 */}
        {activeTag ? (
          <span className={`tag-dock-filter ${colorClass(colorOf(activeTag))}`}>
            <span className="tag-dot" aria-hidden />
            <span className="tag-dock-filter-name">
              {isAutoTag(activeTag) ? autoTagLabel(activeTag) : activeTag}
            </span>
            <span
              className="tag-dock-filter-x"
              role="button"
              tabIndex={0}
              aria-label="清除标签筛选"
              title="清除筛选"
              onClick={(e) => {
                e.stopPropagation();
                onPickTag(null);
              }}
            >
              <Icon name="close" size={9} stroke={2.4} />
            </span>
          </span>
        ) : (
          tags.length > 0 && <span className="tag-dock-count tabular">{tags.length}</span>
        )}

        <span className={`tag-dock-caret ${open ? "" : "closed"}`}>
          <Icon name="chevron" size={10} stroke={2} />
        </span>
      </div>

      <div className="tag-dock-body">
        <div className="tag-dock-tools">
          {/* 常显：分组面板一直是常显的（`过滤分组…`），标签这边按 `total > 8` 藏起来
              就变成"同一个位置有时有框有时没有" —— owner 直接问"我标签搜索框去哪里了"。 */}
          <BlinkCaretInput
            value={q}
            onChange={setQ}
            placeholder="过滤标签…"
            fieldClassName="tag-dock-search"
            ariaLabel="过滤标签"
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
            title="新建标签"
            aria-label="新建标签"
            onClick={() => {
              setAdding((v) => !v);
              setDraft("");
            }}
          >
            <Icon name="plus" size={11} stroke={1.8} />
            <span>标签</span>
          </button>
        </div>

        {adding && (
          <div className="tag-group-add" ref={addRef}>
            <BlinkCaretInput
              value={draft}
              onChange={setDraft}
              placeholder="新标签名，回车建立"
              ariaLabel="新标签名"
              onKeyDown={(e) => {
                if (e.key === "Enter") commitTag();
                else if (e.key === "Escape") {
                  e.stopPropagation();
                  setAdding(false);
                  setDraft("");
                }
              }}
              onBlur={commitTag}
            />
          </div>
        )}

        <div className="tag-dock-scroll scroll-hot" role="listbox" aria-label="标签">
          {groups.map((g) => {
            if (needle && !hit(g.name) && shownTags.filter((t) => t.group === g.name).length === 0)
              return null;
            const items = sortIn(shownTags.filter((t) => t.group === g.name));
            const isClosed = !needle && closed.has(g.name);
            return (
              <div key={g.name} className="tag-group">
                <div
                  className="tag-group-row"
                  onClick={() => toggleGroup(g.name)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    onGroupMenu(g.name, e.clientX, e.clientY);
                  }}
                  title={`${g.name} · ${items.length}`}
                >
                  <span className={`tag-caret ${isClosed ? "closed" : ""}`}>
                    <Icon name="chevron" size={9} stroke={2} />
                  </span>
                  <span className={`tag-dot ${colorClass(g.color)}`} aria-hidden />
                  <span className="tag-row-name">{g.name}</span>
                  <button
                    className="tag-row-more"
                    title="改颜色 / 改名 / 解散分组"
                    aria-label={`编辑分组 ${g.name}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      onEditGroup(g.name);
                    }}
                  >
                    <Icon name="more" size={12} stroke={2} />
                  </button>
                  <span className="tag-row-count tabular">
                    {items.reduce((n, m) => n + m.count, 0)}
                  </span>
                </div>
                {!isClosed && items.map((t) => row(t))}
                {!isClosed && items.length === 0 && (
                  <div className="tag-dock-empty tag-dock-empty-inline">空分组。右键任意标签 → 分组</div>
                )}
              </div>
            );
          })}

          {groups.length > 0 && ungrouped.length > 0 && (
            <div className="tag-subheading">未分组</div>
          )}
          {ungrouped.map((t) => row(t))}

          {tags.length === 0 && shownAuto.length === 0 && (
            <div className="tag-dock-empty">
              还没有标签。选中素材后按 <b>T</b> 打标签；打完会出现在这里，可以上色、分组。
            </div>
          )}

          {shownAuto.length > 0 && (
            <>
              <div
                className="tag-subheading tag-subheading-toggle"
                onClick={() => setAutoOpen((v) => !v)}
                title="按原始文件的格式 / 类型 / 像素 / 比例 / 体积自动推导，不能手工修改"
              >
                <span className={`tag-caret ${autoOpen ? "" : "closed"}`}>
                  <Icon name="chevron" size={9} stroke={2} />
                </span>
                自动标签
              </div>
              {autoOpen &&
                groupAutoTags(shownAuto).map((sec) => (
                  <div key={sec.ns} className="tag-ns">
                    <div className="tag-ns-row">{sec.label}</div>
                    {sec.items.map((t) => row(t, true))}
                  </div>
                ))}
            </>
          )}

          {needle && shownTags.length === 0 && shownAuto.length === 0 && (
            <div className="tag-dock-empty">没有匹配「{q.trim()}」的标签</div>
          )}
        </div>
      </div>
    </div>
  );
}
