import { useCallback, useEffect, useState } from "react";
import type { DirNode, Source } from "../types";
import { onAppEvent } from "../data/source";
import { Icon } from "./ui";
import { useT } from "../lib/i18n";

/**
 * 设置 → 素材库（照 FontManager 的「字体库文件夹」重排，0.1.35）。
 *
 * ## 为什么这一页必须有
 *
 * 2026-11-16 用户的原话：「加入功能文件库，类似这个的资源库地址，和 FontManager 一样」。
 * FontManager 的「字体库文件夹」管的是**字体库在哪**；我把同样的界面做成了「数据文件夹」
 * （索引和缩略图缓存放哪）—— 词是贴切的，但用户的心里模型是「我的素材库在哪」。
 * 于是他把它设成了自己的素材目录，回主界面看到 0/3 和空状态：
 * 「已经设置了文件库，但是没有任何效果」。
 *
 * 所以：**设置里必须能直接管素材库**（加/看/移除），而不是只能去侧栏点那个不起眼的 +。
 * 侧栏的 + 与这里的「添加文件夹…」走同一条路（App 的 onAddFolder），行为必须一致。
 *
 * ## 与 FontManager 的差异（有意为之）
 *
 * FontManager 的字体库是「一次只有一个是当前库」，所以它有「当前 / 切换」两种行状态。
 * AssetManager **没有"当前素材库"这个概念** —— 所有库同时在树里、可以跨库筛标签，
 * 所以这里只搬视觉（行、完整路径、图标、打开、移除、虚线添加按钮），不搬切换语义。
 * 另外 FM 的 ⓘ 是悬停才出的说明，这里换成常驻说明行（本项目的规范要求状态常驻）。
 *
 * ## 三条交互约定
 *
 * 1. 每行显示**完整路径**（用户要能确认自己加的是哪个盘哪个文件夹），行尾可打开/移除。
 * 2. `×` 只清索引，**磁盘文件一个不动**（真删文件走主界面的回收站流程）。
 * 3. 到 3 个上限就**禁用按钮并在 title 里说明原因**，不做「点了再弹一句抱歉」。
 */
export function LibraryCard({
  source,
  onAddFolder,
  onRemoveLib,
  onToast,
}: {
  source: Source;
  /** 走 App 的「添加文件夹」流程（含上限检查、重复判断、扫描进度） */
  onAddFolder: () => Promise<void>;
  onRemoveLib: (path: string) => Promise<void>;
  onToast: (msg: string, err?: boolean, detail?: string) => void;
}) {
  const t = useT();
  const [roots, setRoots] = useState<DirNode[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /* 内置库（软件自己的截图目录）：不占名额、不给移除（owner 2026-11-17） */
  const [builtin, setBuiltin] = useState<string[]>([]);
  /* 正在导出哪个库（按钮置灰 + 防连点；导出整库要读几分钟盘） */
  const [exporting, setExporting] = useState<string | null>(null);
  /* 正在导出哪个库的**用户数据**（.D）：只写一个 JSON，快得多，但仍防连点 */
  const [dataExporting, setDataExporting] = useState<string | null>(null);

  useEffect(() => {
    void source
      .builtinRoots()
      .then(setBuiltin)
      .catch(() => setBuiltin([]));
  }, [source]);

  const samePath = (a: string, b: string) => {
    const f = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
    return f(a) === f(b);
  };
  const isBuiltin = (p: string) => builtin.some((b) => samePath(b, p));

  /**
   * 导出**这一个库**（owner 2026-11-20：多库时每个库后面都要有自己的导出按钮，
   * 不要把几个库攒成一个包一起导）。
   *
   * 走的是整库 zip：原图保留库内相对路径 + `_AssetManager/` 里放库数据，
   * 文件名带年月日，超过 4 GB 由后端自动分片。
   */
  const doExport = async (root: string) => {
    setExporting(root);
    try {
      const r = await source.exportLibraryZip(root);
      if (!r) return; // 用户取消了选文件夹
      const names = r.parts.map((p) => p.name).join(t("app.common.list_sep"));
      const notes: string[] = [];
      if (r.parts.length > 1) notes.push(t("app.toast.zip_parts", { count: r.parts.length }));
      if (r.skipped > 0) notes.push(t("app.toast.zip_skipped", { count: r.skipped }));
      if (r.oversized > 0) notes.push(t("app.toast.zip_oversized", { count: r.oversized }));
      onToast(
        t("app.toast.zip_exported", { count: r.files, names }),
        r.skipped > 0 || r.oversized > 0,
        notes.join(" · ") || undefined
      );
    } catch (e) {
      onToast(String(e), true);
    } finally {
      setExporting(null);
    }
  };

  /** 每行后面那个导出按钮（内置库也有：截图也是用户的资产） */
  const exportBtn = (root: string) => (
    <button
      className="lib-act"
      title={t("lib.exportZipTip")}
      disabled={busy || exporting !== null || !source.desktop}
      onClick={() => void doExport(root)}
    >
      <Icon name={exporting === root ? "refresh" : "download"} size={12} stroke={1.5} />
    </button>
  );

  /**
   * 只导**这一个库**的用户数据（0.4.5，owner 2026-11-20："按库导"）。
   *
   * 产物是 `<库名>-userdata-<时间戳>.D-AssetManager`：库根 + 评分 + 备注 + 标签/分组，
   * 不含原图 —— 换机、重扫之后导回来就能把"人写的东西"找齐。
   * 和上面的 zip 分开两个按钮：一个是"整个库搬走"，一个是"只搬我写过的批注"。
   */
  const doExportData = async (root: string) => {
    setDataExporting(root);
    try {
      const p = await source.exportUserdata(root);
      onToast(t("settings.toast.userdataExported"), false, p);
    } catch (e) {
      onToast(String(e), true);
    } finally {
      setDataExporting(null);
    }
  };

  const dataBtn = (root: string) => (
    <button
      className="lib-act"
      title={t("lib.exportDataTip")}
      disabled={busy || dataExporting !== null || !source.desktop}
      onClick={() => void doExportData(root)}
    >
      <Icon name={dataExporting === root ? "refresh" : "file"} size={12} stroke={1.5} />
    </button>
  );

  const reload = useCallback(() => {
    if (!source.desktop) {
      setRoots([]);
      return;
    }
    void source
      .roots()
      .then((r) => {
        setRoots(r);
        setErr(null);
      })
      .catch((e) => {
        setRoots([]);
        setErr(String(e));
      });
  }, [source]);

  useEffect(reload, [reload]);

  /* 扫描是后台跑的：`onAddFolder` 立刻返回，库要等扫完才出现在 roots() 里。
     不听这个事件的话，用户点完「添加文件夹…」看着列表还是空的 —— 又是"没效果"。 */
  useEffect(() => onAppEvent("scan://done", () => reload()), [reload]);

  const max = 3;
  const all = roots ?? [];
  /* 内置库单独一段、排在最上面：它不是用户加的，混在"3 个名额"里会让人以为
     自己只剩 2 个位置（实际它不占名额）。 */
  const builtinRows = all.filter((r) => isBuiltin(r.path));
  const userRows = all.filter((r) => !isBuiltin(r.path));
  const count = userRows.length;
  const full = count >= max;

  const addTitle = !source.desktop
    ? t("lib.addDesktopOnly")
    : full
      ? t("lib.addFull", { max })
      : t("lib.addTip");

  return (
    <div className="lib-card">
      <div className="lib-head">
        {/* 不再重复写一遍「素材库」：页面标题已经是它了，标题叠标题只会让人以为有两块 */}
        <span className="lib-sub">
          {t("lib.count", { count, max })}
        </span>
        <span className="grow" />
        {/* 虚线按钮 = 「这里还能放东西」（照 FM 的 .settings-add-folder 数值）。
            禁用时**不**加 pointer-events:none：那样 title 的说明就永远看不到了，
            而"为什么不能点"恰恰要靠 title 说。 */}
        <button
          className="lib-add"
          disabled={busy || full || !source.desktop}
          title={addTitle}
          onClick={async () => {
            setBusy(true);
            try {
              await onAddFolder();
              // 扫描可能在跑，先刷一次；扫完那条事件还会再刷一次
              reload();
            } finally {
              setBusy(false);
            }
          }}
        >
          <Icon name="plus" size={12} stroke={1.7} />
          {t("lib.addFolder")}
        </button>
      </div>

      {err && <div className="lib-note">{t("lib.readFailed", { error: err })}</div>}

      {!roots ? (
        <div className="lib-note">{t("lib.loading")}</div>
      ) : (
        <>
          {builtinRows.length > 0 && (
            <div className="lib-list">
              {builtinRows.map((r) => (
                <div
                  className="lib-row is-builtin"
                  key={r.id}
                  title={t("lib.builtinTip")}
                >
                  <Icon name="folder" size={14} stroke={1.5} className="lib-row-icon" />
                  <span className="lib-path mono" title={r.path}>
                    {r.path}
                  </span>
                  <span className="lib-cnt">
                    {t("app.common.n_assets", { count: r.assetCnt.toLocaleString() })}
                  </span>
                  <span className="lib-tag" title={t("lib.builtinChipTip")}>
                    {t("tree.builtin_chip")}
                  </span>
                  {dataBtn(r.path)}
                  {exportBtn(r.path)}
                  <button
                    className="lib-act lib-open"
                    title={t("app.menu.reveal_open")}
                    onClick={() => void source.reveal(r.path).catch((e) => onToast(String(e), true))}
                  >
                    <Icon name="reveal" size={12} stroke={1.5} />
                  </button>
                </div>
              ))}
            </div>
          )}

          {userRows.length > 0 && (
            <div className="lib-list">
              {userRows.map((r) => (
                <div className="lib-row" key={r.id} title={r.path}>
                  <Icon name="folder" size={14} stroke={1.5} className="lib-row-icon" />
                  <span className="lib-path mono" title={r.path}>
                    {r.path}
                  </span>
                  <span className="lib-cnt">
                    {t("app.common.n_assets", { count: r.assetCnt.toLocaleString() })}
                  </span>
                  {dataBtn(r.path)}
                  {exportBtn(r.path)}
                  <button
                    className="lib-act lib-open"
                    title={t("app.menu.reveal_open")}
                    onClick={() => void source.reveal(r.path).catch((e) => onToast(String(e), true))}
                  >
                    <Icon name="reveal" size={12} stroke={1.5} />
                  </button>
                  <button
                    className="lib-act is-danger"
                    title={t("lib.removeTip")}
                    disabled={busy}
                    onClick={async () => {
                      setBusy(true);
                      try {
                        /* App 那条会先弹一次确认（说明"文件一个都不会动"），
                           移除成功后由它弹「已从素材库移除」—— 这里不再重复弹，
                           否则一次点击出两条一样的提示。 */
                        await onRemoveLib(r.path);
                        reload();
                      } finally {
                        setBusy(false);
                      }
                    }}
                  >
                    <Icon name="close" size={11} stroke={1.8} />
                  </button>
                </div>
              ))}
            </div>
          )}

          {count === 0 && (
            <div className="lib-empty">
              <Icon name="folder" size={20} stroke={1.4} />
              {/* 措辞必须区分「一个库都没有」和「只有内置库」：全新安装时上面那条
                  内置截图库是存在的，写「还没有素材库」会让用户觉得界面在胡说。
                  `count` 仍然只数用户的库（内置库不占 3 个名额，这条判定不能动）。 */}
              <div className="lib-empty-title">{t("lib.emptyTitle")}</div>
              <div className="lib-empty-hint">{t("lib.emptyHint")}</div>
            </div>
          )}
        </>
      )}

      <div className="lib-foot">
        <Icon name="info" size={12} stroke={1.5} />
        <span>{t("lib.foot")}</span>
      </div>
    </div>
  );
}
