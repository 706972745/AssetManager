import { useEffect, useRef, useState } from "react";
import type { TagCount, TagGroup } from "../types";
import { autoTagLabel, colorClass, groupAutoTags, isAutoTag } from "../lib/tags";
import { BlinkCaretInput, Icon } from "./ui";

/**
 * 标签面板（0.5.0 二次改版：从「悬浮抽屉」改成「可折叠的侧栏分区」）。
 *
 * 第一版是「左缘 12px 热区 + 盖住半边树的浮层抽屉」，实机上用起来难受：
 * ① 得把鼠标精确贴到窗口最边缘才会出来，差几像素就没反应；
 * ② 展开后盖住半边目录树，想看下面的目录得先等它自己收；
 * ③ 收起还要等 220ms，来回切标签时一直在跟时序较劲。
 *
 * 现在改成**侧栏里一个真正的分区**：
 * - 「标签」这一行**永远在**（就在底栏上方），不藏起来 —— 找得到，也不用瞄准；
 * - 悬停这一行 / 面板 → 就地展开（占侧栏下半截），**目录树只是被压缩，不被盖住**；
 * - 点这一行 = **固定展开**（再点收起），固定在 `localStorage` 里，下次启动还是展开的；
 * - 展开/收起是 150ms 高度过渡，不抢焦点、不闪；
 * - 收起时头部仍然显示「正在筛的标签」，筛选状态不会因为收起面板就看不见。
 */
const PIN_KEY = "am.tags.pinned.v1";
const CLOSED_KEY = "am.tags.groups.closed.v1";

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
  /** 右键一条标签 → 交给 App 弹自己的菜单（不要整库那套） */
  onTagMenu: (tag: string, x: number, y: number) => void;
  onGroupMenu: (name: string, x: number, y: number) => void;
  /** 有弹层开着（编辑卡 / 批量打标签）→ 面板必须留在原地：
   *  弹层是盖在窗口上的，指针一进弹层就算"离开面板"，不 hold 的话
   *  右键标签 → 编辑卡弹出的同时面板就自己收起来，关掉卡还得重新找位置。 */
  hold?: boolean;
}) {
  const [pinned, setPinned] = useState(loadPinned);
  const [closed, setClosed] = useState<Set<string>>(loadClosed);
  const [autoOpen, setAutoOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  const [q, setQ] = useState("");
  const addRef = useRef<HTMLDivElement>(null);

  /* 注意：**悬停展开是纯 CSS（`.tag-dock:hover`）**，不在这里用 state + 定时器。
     用 JS 管 hover 的后果是：进出事件偶尔丢、离开还要等一个 90~260ms 的定时器，
     实机上就是"鼠标都走开了它还赖着 / 贴上去半天不出来"。
     JS 只管两件 CSS 管不了的事：固定展开（pinned）、弹层开着时不许收（hold）。 */
  const open = pinned || !!hold;

  // 聚焦延到 effect：按钮的 click 收尾晚于 input 挂载，直接 autoFocus 会被抢走焦点
  useEffect(() => {
    if (!adding) return;
    const t = window.setTimeout(() => addRef.current?.querySelector("input")?.focus(), 40);
    return () => window.clearTimeout(t);
  }, [adding]);

  // Esc：先取消固定再收起（正在输入时不拦，交给输入框自己处理）
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !open) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
      setPinned(false);
      savePinned(false);
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [open]);

  const togglePin = () => {
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

  const commitGroup = () => {
    const v = draft.trim();
    setAdding(false);
    setDraft("");
    if (v) onNewGroup(v);
  };

  const colorOf = (name: string) => tags.find((t) => t.name === name)?.color ?? null;

  const needle = q.trim().toLowerCase();
  const hit = (s: string) => !needle || s.toLowerCase().includes(needle);
  const shownTags = tags.filter((t) => hit(t.name));
  const shownAuto = autoTags.filter((t) => hit(t.name) || hit(autoTagLabel(t.name)));
  const total = tags.length + autoTags.length;

  const tagRow = (t: TagCount) => (
    <div
      key={t.name}
      className={`tree-row tag-row ${activeTag === t.name ? "sel" : ""} ${colorClass(t.color)}`}
      onClick={() => onPickTag(activeTag === t.name ? null : t.name)}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        onTagMenu(t.name, e.clientX, e.clientY);
      }}
      title={`${t.name}${t.count ? ` · ${t.count} 项` : ""}｜右键改颜色 / 分组`}
    >
      <span className="tag-dot" aria-hidden />
      <span className="tree-name">{t.name}</span>
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
      <span className="tree-count">{t.count}</span>
    </div>
  );

  const ungrouped = shownTags.filter((t) => !t.group);

  return (
    <div className={`tag-dock ${open ? "open" : ""} ${pinned ? "pinned" : ""}`}>
      {/* 头部永远在、且**位置不动**（CSS 用 order 把它钉在底栏上方）：
          所以它可以安全地当点击目标 —— 点一下 = 固定展开 / 收起。 */}
      <div
        className="tag-dock-head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        aria-label={pinned ? "取消固定并收起标签面板" : "固定展开标签面板"}
        title={pinned ? "已固定：点一下取消固定" : "点一下固定展开（不点则随鼠标离开自动收起）"}
        onClick={togglePin}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            togglePin();
          }
        }}
      >

        <Icon name="tag" size={15} stroke={1.5} className="tag-dock-icon" />
        <span className="grow">标签</span>

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
              <Icon name="close" size={8} stroke={2.4} />
            </span>
          </span>
        ) : (
          tags.length > 0 && <span className="tree-count">{tags.length}</span>
        )}

        <span className={`tag-dock-caret ${open ? "" : "closed"}`}>
          <Icon name="chevron" size={9} stroke={2} className="rot90" />
        </span>
      </div>

      <div className="tag-dock-body">
        <div className="tag-dock-tools">
          {total > 8 && (
            <BlinkCaretInput
              value={q}
              onChange={setQ}
              placeholder="过滤标签…"
              inputClassName="tag-search"
              ariaLabel="过滤标签"
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
                if (e.key === "Enter") commitGroup();
                else if (e.key === "Escape") {
                  e.stopPropagation();
                  setAdding(false);
                  setDraft("");
                }
              }}
              onBlur={commitGroup}
            />
          </div>
        )}

        <div className="tag-dock-scroll scroll-hot">
          {groups.map((g) => {
            const list = shownTags.filter((t) => t.group === g.name);
            // 搜索时：组名或组内任一标签命中就保留这个组，否则整组不占位置
            if (needle && !hit(g.name) && list.length === 0) return null;
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
                  title={`${g.name}｜${g.tags} 个标签 · ${g.count} 项；右键改颜色/改名`}
                >
                  <span className={`caret ${isClosed ? "closed" : ""}`}>
                    <Icon name="chevron" size={9} stroke={2} className="rot90" />
                  </span>
                  <span className={`tag-dot ${colorClass(g.color)}`} aria-hidden />
                  <span className="tree-name">{g.name}</span>
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
                  <span className="tree-count">{g.count}</span>
                </div>
                {!isClosed && list.map(tagRow)}
                {!isClosed && list.length === 0 && (
                  <div className="tree-empty">空分组。右键任意标签 → 分组</div>
                )}
              </div>
            );
          })}

          {groups.length > 0 && ungrouped.length > 0 && (
            <div className="tag-subheading">未分组</div>
          )}
          {ungrouped.map(tagRow)}

          {tags.length === 0 && (
            <div className="tag-dock-empty">
              还没有标签。选中素材后按 <b>T</b> 打标签；打完会出现在这里，可以上色、分组。
            </div>
          )}

          {shownAuto.length > 0 && (
            <>
              <div
                className="tree-subheading"
                onClick={() => setAutoOpen((v) => !v)}
                title="按原始文件的格式 / 类型 / 像素 / 比例 / 体积自动推导，不能手工修改"
              >
                <span className={`caret ${autoOpen ? "" : "closed"}`}>
                  <Icon name="chevron" size={9} stroke={2} className="rot90" />
                </span>
                自动标签
              </div>
              {autoOpen &&
                groupAutoTags(shownAuto).map((sec) => (
                  <div key={sec.ns} className="tag-ns">
                    <div className="tag-ns-row">{sec.label}</div>
                    {sec.items.map((t) => (
                      <div
                        key={t.name}
                        className={`tree-row tag-row auto ${activeTag === t.name ? "sel" : ""}`}
                        onClick={() => onPickTag(activeTag === t.name ? null : t.name)}
                        title={`自动标签「${t.name}」· ${t.count} 项（由原始文件推导，改了文件才会变）`}
                      >
                        <span className="tag-dot auto" aria-hidden />
                        <span className="tree-name">
                          {isAutoTag(t.name) ? autoTagLabel(t.name) : t.name}
                        </span>
                        <span className="tree-count">{t.count}</span>
                      </div>
                    ))}
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
