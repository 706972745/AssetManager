/**
 * 回收站（**照 FontManager 的 TrashView**，owner 2026-11-17）。
 *
 * 数据在后端 `core/src/trash.rs`：删除 = 把文件搬进 `<数据目录>/trash/` 并写一份
 * `.amtrash` 账本；这里只负责列出、还原、彻底删除、清空。**彻底删除**才送系统回收站。
 *
 * 0.3.29 起（owner 2026-11-19）：每行左侧有缩略图、点一行在右侧出详情
 * （预览 + 大小 / 删除时间 / 原位置 + 三个动作）。截图贴图默认进回收站之后，
 * 一屏全是「截图 2026-11-19 19-xx-xx.png」，只看文件名认不出是哪张。
 * 缩略图地址由 `am_trash_media` 按账本 id 现开（`thumb://` 只认素材 id，回收站里的不算素材）。
 */
import { source } from "../data/source";
import type { TrashEntry } from "../types";
import { fmtSize, fmtTime } from "../types";
import { Icon } from "./ui";

/** 扩展名徽标（和网格里的 `.tile-badge` 同一套口径：大写、最长 4 个字） */
function extOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(i + 1).toUpperCase().slice(0, 4) : "FILE";
}

export function TrashView({
  items,
  thumbs,
  selected,
  onSelect,
  onRestore,
  onPurge,
  onEmpty,
  onClose,
}: {
  items: TrashEntry[];
  /** 条目 id → 预览地址（由 App 拉 `am_trash_media` 后传下来） */
  thumbs: Record<string, string>;
  selected: string | null;
  onSelect: (id: string | null) => void;
  onRestore: (ids: string[], to?: string) => void;
  onPurge: (ids: string[]) => void;
  onEmpty: () => void;
  onClose: () => void;
}) {
  /** 挑目录再还原：贴图默认进回收站，想留底就挑个自己的素材夹 */
  const restoreTo = (id: string) => {
    void source
      .pickFolder("还原到哪个文件夹")
      .then((dir) => {
        if (dir) onRestore([id], dir);
      })
      .catch(() => {});
  };

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
            <div
              key={t.id}
              className={`trash-row ${selected === t.id ? "sel" : ""}`}
              title={t.originalPath}
              onClick={() => onSelect(selected === t.id ? null : t.id)}
            >
              <span className="trash-thumb">
                {thumbs[t.id] ? (
                  <img src={thumbs[t.id]} alt="" draggable={false} />
                ) : (
                  <Icon name="image" size={15} stroke={1.5} />
                )}
              </span>
              <div className="trash-info">
                <span className="trash-name">{t.name}</span>
                <span className="trash-path mono">{t.originalPath}</span>
              </div>
              <span className="trash-meta mono">
                {fmtSize(t.size)} · {fmtTime(t.deletedAt)}
              </span>
              <button
                className="btn"
                onClick={(e) => {
                  e.stopPropagation();
                  onRestore([t.id]);
                }}
                title="还原到原来的位置"
              >
                还原
              </button>
              <button
                className="btn"
                title="还原到指定文件夹（截图贴图默认进回收站，想留底就存到自己挑的地方）"
                onClick={(e) => {
                  e.stopPropagation();
                  restoreTo(t.id);
                }}
              >
                还原到…
              </button>
              <button
                className="btn btn-danger"
                onClick={(e) => {
                  e.stopPropagation();
                  onPurge([t.id]);
                }}
              >
                彻底删除
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * 回收站条目的详情面板（右侧，和素材详情同一套骨架与宽度）。
 *
 * 为什么不复用 `Detail`：那边整块建立在 `Asset` 上（评分 / 标签 / 引用 / EXIF），
 * 回收站里的条目只有「名字 / 大小 / 删除时间 / 原位置」四个事实 —— 硬塞进 Asset
 * 只能编造 id、rating 这些假字段，下一个人读到只会被骗。
 */
/**
 * 回收站详情。**没有关闭按钮**（owner 2026-11-19）：
 * 它是常驻面板 —— 回收站里有条目时一直在右侧，选中哪条显示哪条；
 * 既然"关不掉也不该关"，右上角那个 × 只会让人点了之后困惑"面板去哪了"。
 */
export function TrashDetail({
  entry,
  src,
  onRestore,
  onPurge,
}: {
  entry: TrashEntry;
  src?: string;
  onRestore: (ids: string[], to?: string) => void;
  onPurge: (ids: string[]) => void;
}) {
  return (
    <aside className="detail trash-detail" style={{ width: 348 }} aria-label="回收站详情">
      <div className="detail-inner scroll-hot">
        <header className="detail-head">
          <span className="detail-badge mono">{extOf(entry.name)}</span>
          <span className="detail-title" title={entry.name}>
            {entry.name}
          </span>
        </header>

        <div className="detail-body">
          <div className="trash-hero">
            {src ? (
              <img src={src} alt="" draggable={false} />
            ) : (
              <span>这张图已经不在回收站里了</span>
            )}
          </div>

          <div className="detail-artmeta mono">
            <span>{fmtSize(entry.size)}</span>
            <span>回收站</span>
            <span>{fmtTime(entry.deletedAt)}</span>
          </div>

          <section>
            <div className="detail-heading">基本信息</div>
            <dl className="detail-meta">
              <dt>文件名</dt>
              <dd title={entry.name}>{entry.name}</dd>
              <dt>大小</dt>
              <dd>{fmtSize(entry.size)}</dd>
              <dt>删除时间</dt>
              <dd>{fmtTime(entry.deletedAt)}</dd>
              <dt>原位置</dt>
              <dd className="mono" title={entry.originalPath}>
                {entry.originalPath}
              </dd>
            </dl>
          </section>

          <section className="trash-detail-actions">
            <button className="btn" onClick={() => onRestore([entry.id])}>
              还原到原位置
            </button>
            <button
              className="btn"
              onClick={() => {
                void source
                  .pickFolder("还原到哪个文件夹")
                  .then((dir) => {
                    if (dir) onRestore([entry.id], dir);
                  })
                  .catch(() => {});
              }}
            >
              还原到…
            </button>
            <button className="btn btn-danger" onClick={() => onPurge([entry.id])}>
              彻底删除
            </button>
          </section>
        </div>
      </div>
    </aside>
  );
}
