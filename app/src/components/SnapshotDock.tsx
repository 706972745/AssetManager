import type { SnapshotRow } from "../types";
import { Icon } from "./ui";

/**
 * 快照面板（0.12.0）：侧栏里「标签」上面的一块，一行一个快照。
 *
 * 交互和标签面板刻意不同：**不做悬停展开**。理由很实在 ——
 * 快照是「低频但要看得见」的东西，用户经常是"我记得存过一块板，它在哪"，
 * 藏起来就得先找热区再展开，等于多两步；而它又只占几行，常驻不挡目录树。
 *
 * - 点一行 = 打开这块快照的灵感板（把快照里的图铺到无限画布上）
 * - 右键 = 对账 / 导出快照清单 / 导出原始图片（zip）/ 改名 / 删除
 * - 头部 `+` = 新建快照（也可以从"右键标签 → 加入快照"里顺手新建）
 */
export function SnapshotDock({
  items,
  onOpen,
  onMenu,
  onCreate,
  onHeadMenu,
}: {
  items: SnapshotRow[];
  onOpen: (row: SnapshotRow) => void;
  onMenu: (id: number, x: number, y: number) => void;
  onCreate: () => void;
  /** 右键头部：新建 / 导入 */
  onHeadMenu: (x: number, y: number) => void;
}) {
  return (
    <div className="snap-dock">
      <div
        className="snap-dock-head"
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onHeadMenu(e.clientX, e.clientY);
        }}
      >
        <Icon name="expand" size={12} stroke={1.5} />
        <span className="grow">快照</span>
        {items.length > 0 && <span className="tree-count">{items.length}</span>}
        <button
          className="snap-add"
          title="新建快照"
          aria-label="新建快照"
          onClick={onCreate}
        >
          <Icon name="plus" size={11} stroke={1.8} />
        </button>
      </div>

      <div className="snap-dock-list scroll-hot">
        {items.length === 0 ? (
          <div className="snap-empty">
            右键任意标签 →「把标签下的图片加入快照」，
            <br />
            就能把这一批图存成快照（文件一个都不动）。
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
                <Icon name="expand" size={13} stroke={1.5} />
              </span>
              <span className="tree-name">{s.name}</span>
              <span className="tree-count">{s.count}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
