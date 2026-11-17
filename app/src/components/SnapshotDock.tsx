import { useEffect, useState } from "react";
import type { SnapshotRow } from "../types";
import { Icon } from "./ui";

/**
 * 快照分区（0.12.1）—— **位置与形态都照 FontManager 侧栏的「快照」那一段**：
 *
 * ```
 * 目录树（浏览）
 * ─────────────────────────────
 * 快 照              [导入] [新建]   ← 小号大字距的段落标题 + 右侧两个图标按钮
 *   客户参考              128
 *   场景                   64
 * ─────────────────────────────
 * 标签（独立的 dock，不在这段里面）
 * ```
 *
 * 三条硬约定（和 FontManager 一致，**别再改回"贴在标签上的第二个 dock"**）：
 * 1. 它是侧栏里的一个 **section**，有自己的一行段落标题（可折叠），不是标签面板的一部分；
 * 2. 折叠状态存 `localStorage`，下次启动还记着（存"收起"，默认展开）；
 * 3. 行沿用 `tree-row` 那套（相机图标 + 名字 + 数量），右键菜单由 App 提供。
 */
const CLOSED_KEY = "am.snaps.closed.v1";

function loadClosed(): boolean {
  try {
    return localStorage.getItem(CLOSED_KEY) === "1";
  } catch {
    return false;
  }
}

function saveClosed(v: boolean) {
  try {
    localStorage.setItem(CLOSED_KEY, v ? "1" : "0");
  } catch {
    /* 存不进去就本次会话有效 */
  }
}

export function SnapshotDock({
  items,
  onOpen,
  onMenu,
  onCreate,
  onImport,
}: {
  items: SnapshotRow[];
  onOpen: (row: SnapshotRow) => void;
  onMenu: (id: number, x: number, y: number) => void;
  onCreate: () => void;
  onImport: () => void;
}) {
  const [closed, setClosed] = useState(loadClosed);

  useEffect(() => {
    saveClosed(closed);
  }, [closed]);

  return (
    <div className={`snap-section ${closed ? "snap-collapsed" : ""}`}>
      <div className="snap-heading">
        <button
          className="snap-toggle"
          onClick={() => setClosed((v) => !v)}
          aria-expanded={!closed}
          title={closed ? "展开快照" : "收起快照"}
        >
          <span className="snap-chevron">
            <Icon name="chevron" size={12} stroke={2} className={closed ? "" : "rot90"} />
          </span>
          <span>快照</span>
        </button>
        <button
          className="snap-add"
          title="导入快照（.json，只含图片编号）"
          aria-label="导入快照"
          onClick={onImport}
        >
          <Icon name="import" size={13} stroke={1.8} />
        </button>
        <button className="snap-add" title="新建快照" aria-label="新建快照" onClick={onCreate}>
          <Icon name="plus" size={13} stroke={1.8} />
        </button>
      </div>

      {!closed && (
        <div className="snap-list scroll-hot">
          {items.length === 0 ? (
            <div className="snap-empty">
              还没有快照。右键任意<b>标签</b> →「把标签下的图片加入快照」，
              就能把这一批图记成一份快照（文件一个都不动）。
            </div>
          ) : (
            items.map((s) => (
              <div
                key={s.id}
                className="tree-row snap-row"
                title={`${s.name}｜${s.count} 张｜点击打开灵感板，右键更多`}
                onClick={() => onOpen(s)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  onMenu(s.id, e.clientX, e.clientY);
                }}
              >
                <span className="nav-icon">
                  <Icon name="camera" size={15} stroke={1.5} />
                </span>
                <span className="tree-name">{s.name}</span>
                <span className="tree-count">{s.count}</span>
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}
