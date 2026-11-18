import { useEffect, useRef, useState } from "react";
import type { SnapshotRow } from "../types";
import { BlinkCaretInput, Icon } from "./ui";

/**
 * 快照分区 —— **位置、形态、交互都照 FontManager 侧栏的「快照」那一段**：
 *
 * ```
 * 目录树（浏览）
 * ─────────────────────────────
 * 快 照              [导入] [新建]   ← 小号大字距的段落标题 + 右侧两个图标按钮
 *   ▸ 相机 给这份快照起个名字…        ← 点「新建」就地长出一行输入框（不弹窗）
 *   客户参考              128
 *   场景                   64
 * ─────────────────────────────
 * 标签（独立的 dock，不在这段里面）
 * ```
 *
 * 四条硬约定（和 FontManager 一致）：
 * 1. 它是侧栏里的一个 **section**，有自己的段落标题（可折叠），**在 `.tree-scroll` 内**紧跟素材库；
 * 2. 折叠状态存 `localStorage`，下次启动还记着（存"收起"，默认展开）；
 * 3. **新建 / 改名都是就地一行输入框**（`Enter` 确定、`Esc` 取消、失焦也算确定、空名等于取消），
 *    和「添加素材库」「新建文件夹」同一套手感 —— 不要改回弹窗（owner 2026-11-17：照 FontManager）；
 * 4. 行沿用 `tree-row` 那套（相机图标 + 名字 + 数量），右键菜单由 App 提供。
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

/**
 * 就地输入行（照 FontManager `NewSnapshotInput` / `RenameInput`）。
 *
 * 三个细节都不能省：
 * - `autoFocus`：点了「新建」光标就得到，不用再点一下输入框；
 * - `onBlur` 也算确定：用户点别处时不该丢掉刚写的名字（这与「添加素材库」一致）；
 * - `Esc` 必须 `stopPropagation`：侧栏外层有全局 Esc（清选择 / 关面板），
 *   不拦住会把整块界面一起关掉。
 */
function SnapshotInput({
  initial = "",
  placeholder,
  ariaLabel,
  onCommit,
  onCancel,
}: {
  initial?: string;
  placeholder: string;
  ariaLabel: string;
  /** 传空字符串 = 用户没写名字（当成取消，交给上层判断） */
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial);

  return (
    <div className="snap-input-row">
      <span className="nav-icon">
        <Icon name="camera" size={15} stroke={1.5} />
      </span>
      <BlinkCaretInput
        autoFocus
        value={name}
        onChange={setName}
        placeholder={placeholder}
        inputClassName="snap-input"
        ariaLabel={ariaLabel}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            onCommit(name.trim());
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onCancel();
          }
        }}
        onBlur={() => onCommit(name.trim())}
      />
    </div>
  );
}

export function SnapshotDock({
  items,
  creating,
  renamingId,
  activeId,
  onOpen,
  onPick,
  onMenu,
  onStartCreate,
  onCreate,
  onCancelCreate,
  onRename,
  onCancelRename,
  onImport,
}: {
  items: SnapshotRow[];
  /** 正在新建（头部 + 或命令面板触发，状态在 App 里，输入行在这里） */
  creating: boolean;
  /** 正在改名的那条（null = 没有） */
  renamingId: number | null;
  /** 正在筛选的那份快照（点击行 = 只看它，和点分组同一个效果） */
  activeId?: number | null;
  /** 双击才打开灵感板（owner 2026-11-18：单击不该直接跳走） */
  onOpen: (row: SnapshotRow) => void;
  onPick: (row: SnapshotRow) => void;
  onMenu: (id: number, x: number, y: number) => void;
  onStartCreate: () => void;
  /** 名字为空字符串时表示用户什么都没写（上层应当只退出编辑态） */
  onCreate: (name: string) => void;
  onCancelCreate: () => void;
  onRename: (id: number, name: string) => void;
  onCancelRename: () => void;
  onImport: () => void;
}) {
  const [closed, setClosed] = useState(loadClosed);
  /* 单击「只看这份」要延后 200ms 再执行：
     双击时浏览器会先发两次 click、再发 dblclick —— 立即执行的话双击等于
     "筛上 → 取消 → 打开灵感板"，列表先闪一下，而且第二次点击可能落在重排后的另一行上。 */
  const clickTimer = useRef<number | null>(null);

  useEffect(() => {
    saveClosed(closed);
  }, [closed]);

  /* 从命令面板 / 空态按钮进来时，如果这段正好是收起的，先展开再长出输入行，
     否则用户看到"什么都没发生"。 */
  useEffect(() => {
    if (creating) setClosed(false);
  }, [creating]);

  return (
    <div className={`snap-section ${closed ? "snap-collapsed" : ""}`}>
      <div className="snap-heading">
        <button
          className="snap-toggle"
          onClick={() => setClosed((v) => !v)}
          aria-expanded={!closed}
          /* 悬停气泡照 FontManager 的 hint.snapshotsSection：说清"这是什么 + 点标题干什么"。
             不再用原生 title（灰底黑字、延迟不定，两边的观感差得最明显）。 */
          data-hint-text="快照：把一批图片记成一份清单，之后一键打开灵感板（只记图片，不搬文件）；点标题展开或收起"
        >
          <span className="snap-chevron">
            <Icon name="chevron" size={12} stroke={2} className={closed ? "" : "rot90"} />
          </span>
          <span>快照</span>
        </button>
        <button
          className="snap-add"
          data-hint-text="导入快照清单（.json，只含图片编号，不含图片本身）"
          aria-label="导入快照"
          onClick={onImport}
        >
          <Icon name="import" size={13} stroke={1.8} />
        </button>
        <button
          className="snap-add"
          data-hint-text="新建快照：给眼下这批图片起个名字（不复制、不移动文件）"
          aria-label="新建快照"
          onClick={onStartCreate}
        >
          <Icon name="plus" size={13} stroke={1.8} />
        </button>
      </div>

      {!closed && (
        <div className="snap-list scroll-hot">
          {creating && (
            <SnapshotInput
              placeholder="给这份快照起个名字，回车确定"
              ariaLabel="新快照名"
              onCommit={onCreate}
              onCancel={onCancelCreate}
            />
          )}

          {items.map((s) =>
            renamingId === s.id ? (
              <SnapshotInput
                key={s.id}
                initial={s.name}
                placeholder="新的快照名"
                ariaLabel="快照新名"
                onCommit={(n) => onRename(s.id, n)}
                onCancel={onCancelRename}
              />
            ) : (
              <div
                key={s.id}
                className={`tree-row snap-row${activeId === s.id ? " sel" : ""}`}
                title={`${s.name}｜${s.count} 张｜单击只看这份快照，双击打开灵感板，右键更多`}
                onClick={() => {
                  if (clickTimer.current) window.clearTimeout(clickTimer.current);
                  clickTimer.current = window.setTimeout(() => {
                    clickTimer.current = null;
                    onPick(s);
                  }, 200);
                }}
                onDoubleClick={() => {
                  if (clickTimer.current) {
                    window.clearTimeout(clickTimer.current);
                    clickTimer.current = null;
                  }
                  onOpen(s);
                }}
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
            )
          )}

          {/* 空态本身就是入口（照 FontManager `.sidebar-hint` 的虚线框 + 一句「创建第一个快照」）。
              以前这里是"两行灰字 + 小字说明"，和 FontManager 长得不一样；
              解释文字挪到段头的气泡里（悬停段头才出来，不占地方）。 */}
          {items.length === 0 && !creating && (
            <button
              className="snap-empty"
              onClick={onStartCreate}
              data-hint-text="快照只记图片，不搬文件；也可以右键任意「标签」→ 把标签下的图片加入快照"
            >
              创建第一个快照
            </button>
          )}
        </div>
      )}
    </div>
  );
}
