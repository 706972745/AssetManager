/**
 * 回收站（**照 FontManager 的 TrashView**，owner 2026-11-17）。
 *
 * 数据在后端 `core/src/trash.rs`：删除 = 把文件搬进 `<数据目录>/trash/` 并写一份
 * `.amtrash` 账本；这里只负责列出、还原、彻底删除、清空。**彻底删除**才送系统回收站。
 */
import type { TrashEntry } from "../types";
import { fmtSize, fmtTime } from "../types";
import { Icon } from "./ui";

export function TrashView({
  items,
  onRestore,
  onPurge,
  onEmpty,
  onClose,
}: {
  items: TrashEntry[];
  onRestore: (ids: string[]) => void;
  onPurge: (ids: string[]) => void;
  onEmpty: () => void;
  onClose: () => void;
}) {
  return (
    <div className="trash-view">
      <div className="trash-bar">
        <span className="grow">{`回收站里 ${items.length} 项`}</span>
        <button className="btn" onClick={onClose}>
          退出回收站
        </button>
        <button className="btn" onClick={onEmpty} disabled={items.length === 0}>
          清空回收站
        </button>
      </div>

      {items.length === 0 ? (
        <div className="empty-state">
          <div className="empty-inner">
            <span className="empty-tile">
              <Icon name="trash" size={26} stroke={1.5} />
            </span>
            <h2>回收站是空的</h2>
            <p>移到回收站的文件会先放在这里，可以还原；清空或彻底删除时才进系统回收站。</p>
          </div>
        </div>
      ) : (
        <div className="trash-list">
          {items.map((t) => (
            <div key={t.id} className="trash-row" title={t.originalPath}>
              <span className="nav-icon">
                <Icon name="image" size={15} stroke={1.5} />
              </span>
              <div className="trash-info">
                <span className="trash-name">{t.name}</span>
                <span className="trash-path mono">{t.originalPath}</span>
              </div>
              <span className="trash-meta mono">
                {fmtSize(t.size)} · {fmtTime(t.deletedAt)}
              </span>
              <button className="btn" onClick={() => onRestore([t.id])}>
                还原
              </button>
              <button className="btn btn-danger" onClick={() => onPurge([t.id])}>
                彻底删除
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
