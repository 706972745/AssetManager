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
 *
 * 0.4.x：支持批量 —— 行首复选框 + Ctrl/Shift 点选，选中后顶栏出现批量动作。
 * 后端 `am_trash_restore` / `am_trash_purge` 本来就吃 `Vec<String>`，
 * 所以这里只是把「一次一条」换成「一次一批」，没有新增 IPC。
 */
import { source } from "../data/source";
import type { TrashEntry } from "../types";
import { fmtSize, fmtTime } from "../types";
import { Icon } from "./ui";
import { useT } from "../lib/i18n";

/** 扩展名徽标（和网格里的 `.tile-badge` 同一套口径：大写、最长 4 个字） */
function extOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(i + 1).toUpperCase().slice(0, 4) : "FILE";
}

export function TrashView({
  items,
  thumbs,
  checked,
  active,
  onPick,
  onToggle,
  onSelectAll,
  onClearSel,
  onRestore,
  onPurge,
  onEmpty,
  onClose,
}: {
  items: TrashEntry[];
  /** 条目 id → 预览地址（由 App 拉 `am_trash_media` 后传下来） */
  thumbs: Record<string, string>;
  /** 已勾选的条目（批量动作用；单选时只有一条） */
  checked: Set<string>;
  /** 详情面板当前显示的那条 —— 单独存，因为 Ctrl 取消勾选后详情不该跟着消失 */
  active: string | null;
  /** 点一行：mods 决定是单选、Ctrl 加减还是 Shift 连选（和网格同一套手感） */
  onPick: (id: string, mods: { ctrl: boolean; shift: boolean }) => void;
  /** 行首复选框：只加/减一条，不改锚点 */
  onToggle: (id: string) => void;
  onSelectAll: () => void;
  onClearSel: () => void;
  onRestore: (ids: string[], to?: string) => void;
  onPurge: (ids: string[]) => void;
  onEmpty: () => void;
  onClose: () => void;
}) {
  const t = useT();
  /** 挑目录再还原：贴图默认进回收站，想留底就挑个自己的素材夹 */
  const restoreTo = (ids: string[]) => {
    void source
      .pickFolder(t("trash.pickFolder"))
      .then((dir) => {
        if (dir) onRestore(ids, dir);
      })
      .catch(() => {});
  };
  const ids = [...checked];

  return (
    <div className="trash-view">
      <div className="trash-bar">
        <span className="grow">{t("trash.count", { count: items.length })}</span>
        <button className="btn" onClick={onClose}>
          {t("trash.exit")}
        </button>
        <button className="btn" onClick={onEmpty} disabled={items.length === 0}>
          {t("trash.empty")}
        </button>
      </div>

      {/* 批量条：只在有勾选时出现。三条动作都直接吃整个勾选集合，逐条的按钮仍在行尾 */}
      {ids.length > 0 && (
        <div className="trash-batch" role="toolbar" aria-label={t("app.batch.title")}>
          <span className="trash-batch-count mono">
            {t("app.batch.selected", { count: ids.length })}
          </span>
          <button className="btn btn-tiny" onClick={() => onRestore(ids)}>
            {t("trash.restoreN", { count: ids.length })}
          </button>
          <button className="btn btn-tiny" onClick={() => restoreTo(ids)}>
            {t("trash.restoreToN", { count: ids.length })}
          </button>
          <button className="btn btn-tiny btn-danger" onClick={() => onPurge(ids)}>
            {t("trash.purgeN", { count: ids.length })}
          </button>
          <span className="grow" />
          <button
            className="btn btn-tiny"
            onClick={onSelectAll}
            disabled={ids.length === items.length}
          >
            {t("app.menu.select_all")}
          </button>
          <button className="btn btn-tiny" onClick={onClearSel}>
            {t("app.batch.clear")}
          </button>
        </div>
      )}

      {items.length === 0 ? (
        <div className="empty-state">
          <div className="empty-inner">
            <span className="empty-tile">
              <Icon name="trash" size={26} stroke={1.5} />
            </span>
            <h2>{t("trash.emptyTitle")}</h2>
            <p>{t("trash.emptyBody")}</p>
          </div>
        </div>
      ) : (
        <div className="trash-list">
          {items.map((row) => (
            <div
              key={row.id}
              className={`trash-row ${checked.has(row.id) ? "sel" : ""} ${
                active === row.id ? "active" : ""
              }`}
              title={row.originalPath}
              onClick={(e) => onPick(row.id, { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey })}
            >
              <input
                type="checkbox"
                className="trash-check"
                checked={checked.has(row.id)}
                onChange={() => onToggle(row.id)}
                onClick={(e) => e.stopPropagation()}
                aria-label={row.name}
              />
              <span className="trash-thumb">
                {thumbs[row.id] ? (
                  <img src={thumbs[row.id]} alt="" draggable={false} />
                ) : (
                  <Icon name="image" size={15} stroke={1.5} />
                )}
              </span>
              <div className="trash-info">
                <span className="trash-name">{row.name}</span>
                <span className="trash-path mono">{row.originalPath}</span>
              </div>
              <span className="trash-meta mono">
                {fmtSize(row.size)} · {fmtTime(row.deletedAt)}
              </span>
              <button
                className="btn"
                onClick={(e) => {
                  e.stopPropagation();
                  onRestore([row.id]);
                }}
                title={t("trash.restoreTitle")}
              >
                {t("trash.restore")}
              </button>
              <button
                className="btn"
                title={t("trash.restoreToTitle")}
                onClick={(e) => {
                  e.stopPropagation();
                  restoreTo([row.id]);
                }}
              >
                {t("trash.restoreTo")}
              </button>
              <button
                className="btn btn-danger"
                onClick={(e) => {
                  e.stopPropagation();
                  onPurge([row.id]);
                }}
              >
                {t("trash.purge")}
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
  const t = useT();
  return (
    <aside className="detail trash-detail" style={{ width: 348 }} aria-label={t("trash.detailAria")}>
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
              <span>{t("trash.gone")}</span>
            )}
          </div>

          <div className="detail-artmeta mono">
            <span>{fmtSize(entry.size)}</span>
            <span>{t("trash.badge")}</span>
            <span>{fmtTime(entry.deletedAt)}</span>
          </div>

          <section>
            <div className="detail-heading">{t("trash.basicInfo")}</div>
            <dl className="detail-meta">
              <dt>{t("trash.fileName")}</dt>
              <dd title={entry.name}>{entry.name}</dd>
              <dt>{t("trash.size")}</dt>
              <dd>{fmtSize(entry.size)}</dd>
              <dt>{t("trash.deletedAt")}</dt>
              <dd>{fmtTime(entry.deletedAt)}</dd>
              <dt>{t("trash.originalPath")}</dt>
              <dd className="mono" title={entry.originalPath}>
                {entry.originalPath}
              </dd>
            </dl>
          </section>

          <section className="trash-detail-actions">
            <button className="btn" onClick={() => onRestore([entry.id])}>
              {t("trash.restoreHere")}
            </button>
            <button
              className="btn"
              onClick={() => {
                void source
                  .pickFolder(t("trash.pickFolder"))
                  .then((dir) => {
                    if (dir) onRestore([entry.id], dir);
                  })
                  .catch(() => {});
              }}
            >
              {t("trash.restoreTo")}
            </button>
            <button className="btn btn-danger" onClick={() => onPurge([entry.id])}>
              {t("trash.purge")}
            </button>
          </section>
        </div>
      </div>
    </aside>
  );
}

/**
 * 多选时的详情占位（右侧，宽度和单条详情一致）。
 *
 * 为什么不是「继续显示最后点的那条」：勾了 20 条之后，右侧摆着其中一条的
 * 大图和「还原到原位置」，会让人以为动作只作用于这一条 —— 这里只报数量，
 * 真正的动作在顶栏批量条上。
 */
export function TrashMultiDetail({ count }: { count: number }) {
  const t = useT();
  return (
    <aside
      className="detail trash-detail trash-multi"
      style={{ width: 348 }}
      aria-label={t("trash.detailAria")}
    >
      <div className="detail-inner">
        <div className="trash-multi-body">
          <span className="trash-multi-count mono">{t("app.batch.selected", { count })}</span>
          <p>{t("trash.multiHint")}</p>
        </div>
      </div>
    </aside>
  );
}
