import { useEffect, useMemo, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { scrimFade, springSnappy } from "../design/springs";
import { fmtSize, type Asset, type DupGroup, type RefItem, type RenameItem, type RenameRule, type SimilarPair, type Source } from "../types";
import { Icon } from "./ui";
import { useT, type TKey } from "../lib/i18n";

/**
 * 工具面板：**重复检测 / 丢失的引用 / 批量重命名** 三件事共用一个浮层。
 *
 * 为什么不各做一个面板：三件事都是"看一批结果、再决定动不动手"，
 * 骨架完全一样（工具条 + 结果列表 + 底部动作）。合成一个面板省两套壳，
 * 也让用户只需要记住一个入口（Ctrl+K → 输入「重复 / 引用 / 重命名」）。
 *
 * 危险动作的纪律：删除永远走回收站；重命名先出**预览**再执行，且走 `FsOps`
 * 所以还能 Ctrl+Z 撤销。
 */

export type ToolTab = "dupes" | "refs" | "rename";

const TABS: { key: ToolTab; label: TKey }[] = [
  { key: "dupes", label: "tools.tabDupes" },
  { key: "refs", label: "tools.tabRefs" },
  { key: "rename", label: "tools.tabRename" },
];

const EMPTY_RULE: RenameRule = {
  find: "",
  replace: "",
  prefix: "",
  suffix: "",
  numberFrom: 0,
  numberPad: 2,
  numberBeforeExt: true,
  lower: false,
  upper: false,
  keepExt: true,
};

export function Tools({
  open,
  tab,
  ids,
  source,
  onClose,
  onToast,
  onPick,
  thumbSrc,
}: {
  open: boolean;
  tab: ToolTab;
  /** 批量重命名作用的对象（当前选中） */
  ids: number[];
  source: Source;
  onClose: () => void;
  onToast: (msg: string, err?: boolean, detail?: string) => void;
  /** 在网格里选中这些素材（跳过去看） */
  onPick: (ids: number[]) => void;
  thumbSrc: (a: Asset, px: number) => string;
}) {
  const t = useT();
  const [cur, setCur] = useState<ToolTab>(tab);
  /* `useState` 只在首次挂载取一次 `tab`，而这个面板是常驻挂载的（`open` 只控制显隐）——
     不补这个同步，从批量条点「批量重命名」打开的还是上次那个页签，
     用户看到的是"重复检测"，自然以为按钮没做（0.3.30）。 */
  useEffect(() => {
    if (open) setCur(tab);
  }, [open, tab]);
  const [busy, setBusy] = useState(false);

  // 重复
  const [groups, setGroups] = useState<DupGroup[]>([]);
  const [similar, setSimilar] = useState<SimilarPair[]>([]);
  const [left, setLeft] = useState(0);
  const [mode, setMode] = useState<"exact" | "similar">("exact");

  // 引用
  const [refSum, setRefSum] = useState<[number, number]>([0, 0]);
  const [broken, setBroken] = useState<RefItem[]>([]);

  // 重命名
  const [rule, setRule] = useState<RenameRule>(EMPTY_RULE);
  const [plan, setPlan] = useState<RenameItem[]>([]);

  // 缩略图/名字：只取结果里出现过的 id
  const [meta, setMeta] = useState<Record<number, Asset>>({});

  useEffect(() => {
    if (open) setCur(tab);
  }, [open, tab]);

  // 面板打开时顺手拉一次引用概况（很便宜：两列计数）
  useEffect(() => {
    if (!open || cur !== "refs") return;
    void source
      .refsSummary()
      .then(setRefSum)
      .catch(() => {});
  }, [open, cur, source]);

  // 把结果里的 id 换成一页 Asset（缩略图 + 文件名 + 体积）
  useEffect(() => {
    if (!open) return;
    const want: number[] = [];
    if (cur === "dupes") {
      if (mode === "exact") groups.slice(0, 24).forEach((g) => want.push(...g.ids.slice(0, 8)));
      else similar.slice(0, 60).forEach((p) => want.push(p.a, p.b));
    }
    if (!want.length) return;
    const miss = want.filter((id) => !meta[id]);
    if (!miss.length) return;
    void source
      .assetsByIds([...new Set(miss)])
      .then((rows) => {
        setMeta((m) => {
          const next = { ...m };
          for (const a of rows) next[a.id] = a;
          return next;
        });
      })
      .catch(() => {});
  }, [open, cur, mode, groups, similar, meta, source]);

  const scanDupes = async () => {
    setBusy(true);
    try {
      if (mode === "exact") {
        const [g, rest] = await source.dupesExact(2000);
        setGroups(g);
        setLeft(rest);
        onToast(
          g.length ? t("tools.toast.dupesFound", { count: g.length }) : t("tools.toast.dupesNone"),
          false,
          rest > 0 ? t("tools.toast.dupesLeft", { count: rest }) : undefined
        );
      } else {
        const list = await source.dupesSimilar(6, 200);
        setSimilar(list);
        onToast(
          list.length ? t("tools.toast.similarFound", { count: list.length }) : t("tools.toast.similarNone")
        );
      }
    } catch (e) {
      onToast(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  const scanRefs = async () => {
    setBusy(true);
    try {
      const [n, refs] = await source.refsScan(120);
      const [t2, m] = await source.refsSummary();
      setRefSum([t2, m]);
      const list = await source.brokenRefs(300);
      setBroken(list);
      onToast(
        t("tools.toast.refsScanned", { files: n, refs }),
        false,
        t("tools.toast.refsDetail", { total: t2, missing: m })
      );
    } catch (e) {
      onToast(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  const makePlan = async () => {
    if (!ids.length) {
      onToast(t("tools.toast.renameNeedSelection"), true);
      return;
    }
    try {
      const rows = await source.renamePlan(ids, rule);
      setPlan(rows);
      const bad = rows.filter((r) => r.error).length;
      onToast(
        bad
          ? t("tools.toast.renamePreviewBad", { count: rows.length, bad })
          : t("tools.toast.renamePreview", { count: rows.length }),
        bad > 0
      );
    } catch (e) {
      onToast(String(e), true);
    }
  };

  const applyPlan = async () => {
    const ok = plan.filter((r) => !r.error);
    if (!ok.length) return;
    setBusy(true);
    try {
      const res = await source.renameApply(ok.map((r) => [r.id, r.newName]));
      onToast(
        res.errors.length
          ? t("tools.toast.renamedSome", { ok: res.ok, bad: res.errors.length })
          : t("tools.toast.renamed", { ok: res.ok }),
        res.errors.length > 0,
        res.errors.slice(0, 3).join(t("app.common.list_sep"))
      );
      setPlan([]);
    } catch (e) {
      onToast(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  const dupStats = useMemo(() => {
    const waste = groups.reduce(
      (sum, g) => sum + (meta[g.ids[0]] ? meta[g.ids[0]].size * (g.ids.length - 1) : 0),
      0
    );
    return waste;
  }, [groups, meta]);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          key="tools-scrim"
          className="set-scrim"
          onMouseDown={onClose}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={scrimFade}
        >
          <motion.div
            className="tool-panel"
            role="dialog"
            aria-modal="true"
            aria-label={t("tools.title")}
            onMouseDown={(e) => e.stopPropagation()}
            initial={{ opacity: 0, scale: 0.97, y: 10 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.98, y: 6, transition: { duration: 0.13 } }}
            transition={springSnappy}
          >
            <header className="tool-head">
              <div className="tool-tabs">
                {TABS.map((tb) => (
                  <button
                    key={tb.key}
                    className={`tool-tab ${cur === tb.key ? "on" : ""}`}
                    onClick={() => setCur(tb.key)}
                  >
                    {t(tb.label)}
                  </button>
                ))}
              </div>
              <button className="kb-close" onClick={onClose} title={t("tools.closeEsc")}>
                Esc
              </button>
            </header>

            <div className="tool-body scroll-hot">
              {cur === "dupes" && (
                <>
                  <div className="tool-bar">
                    <div className="tool-seg">
                      <button
                        className={`tool-seg-btn ${mode === "exact" ? "on" : ""}`}
                        onClick={() => setMode("exact")}
                      >
                        {t("tools.exact")}
                      </button>
                      <button
                        className={`tool-seg-btn ${mode === "similar" ? "on" : ""}`}
                        onClick={() => setMode("similar")}
                      >
                        {t("tools.similar")}
                      </button>
                    </div>
                    <button className="btn btn-accent" disabled={busy} onClick={() => void scanDupes()}>
                      {busy
                        ? t("tools.scanning")
                        : mode === "exact"
                          ? t("tools.scanExact")
                          : t("tools.scanSimilar")}
                    </button>
                    {mode === "exact" && left > 0 && (
                      <span className="tool-note">{t("tools.leftNote", { count: left })}</span>
                    )}
                    {mode === "exact" && groups.length > 0 && (
                      <span className="tool-note">
                        {t("tools.groupStats", { count: groups.length, size: fmtSize(dupStats) })}
                      </span>
                    )}
                  </div>

                  {groups.length === 0 && similar.length === 0 && (
                    <p className="tool-empty">
                      {mode === "exact" ? t("tools.emptyExact") : t("tools.emptySimilar")}
                    </p>
                  )}

                  {groups.map((g) => (
                    <div className="dup-group" key={g.hash}>
                      <div className="dup-head">
                        <span className="mono">
                          {t("tools.dupHead", { count: g.ids.length, size: fmtSize(g.size) })}
                        </span>
                        <button className="link-btn" onClick={() => onPick(g.ids)}>
                          {t("tools.viewInGrid")}
                        </button>
                      </div>
                      <div className="dup-files">
                        {g.ids.slice(0, 8).map((id) => {
                          const a = meta[id];
                          return (
                            <div className="dup-file" key={id} title={a?.name ?? ""}>
                              {a ? <img src={thumbSrc(a, 256)} alt="" /> : <div className="dup-ph" />}
                              <span>{a?.name ?? `#${id}`}</span>
                            </div>
                          );
                        })}
                      </div>
                    </div>
                  ))}

                  {similar.map((p) => (
                    <div className="dup-pair" key={`${p.a}-${p.b}`}>
                      {[p.a, p.b].map((id) => {
                        const a = meta[id];
                        return (
                          <div className="dup-file" key={id} title={a?.name ?? ""}>
                            {a ? <img src={thumbSrc(a, 256)} alt="" /> : <div className="dup-ph" />}
                            <span>{a?.name ?? `#${id}`}</span>
                          </div>
                        );
                      })}
                      <div className="dup-pair-meta">
                        <span className="mono">{t("tools.distance", { dist: p.dist })}</span>
                        <button className="link-btn" onClick={() => onPick([p.a, p.b])}>
                          {t("tools.view")}
                        </button>
                      </div>
                    </div>
                  ))}
                </>
              )}

              {cur === "refs" && (
                <>
                  <div className="tool-bar">
                    <button className="btn btn-accent" disabled={busy} onClick={() => void scanRefs()}>
                      {busy ? t("tools.scanning") : t("tools.scanRefs")}
                    </button>
                    <span className="tool-note">
                      {t("tools.refsStats", { total: refSum[0], missing: refSum[1] })}
                    </span>
                  </div>
                  {broken.length === 0 ? (
                    <p className="tool-empty">{t("tools.emptyRefs")}</p>
                  ) : (
                    broken.map((r, i) => (
                      <div className="ref-row" key={`${r.srcId}-${r.dstPath}-${i}`}>
                        <div className="ref-src">
                          <Icon name="file" size={13} stroke={1.5} />
                          <span title={r.srcPath}>{r.srcName}</span>
                        </div>
                        <div className="ref-dst mono" title={r.dstPath}>
                          {r.dstPath}
                        </div>
                      </div>
                    ))
                  )}
                </>
              )}

              {cur === "rename" && (
                <>
                  <div className="tool-bar">
                    <span className="tool-note">
                      {t("tools.selectedCount", { count: ids.length })}
                      {ids.length === 0 && t("tools.selectInGrid")}
                    </span>
                  </div>
                  <div className="rn-form">
                    <label>
                      <span>{t("tools.find")}</span>
                      <input
                        className="mono"
                        value={rule.find}
                        placeholder={t("tools.findPlaceholder")}
                        onChange={(e) => setRule({ ...rule, find: e.target.value })}
                      />
                    </label>
                    <label>
                      <span>{t("tools.replace")}</span>
                      <input
                        className="mono"
                        value={rule.replace}
                        placeholder={t("tools.replacePlaceholder")}
                        onChange={(e) => setRule({ ...rule, replace: e.target.value })}
                      />
                    </label>
                    <label>
                      <span>{t("tools.prefix")}</span>
                      <input
                        className="mono"
                        value={rule.prefix}
                        onChange={(e) => setRule({ ...rule, prefix: e.target.value })}
                      />
                    </label>
                    <label>
                      <span>{t("tools.suffix")}</span>
                      <input
                        className="mono"
                        value={rule.suffix}
                        onChange={(e) => setRule({ ...rule, suffix: e.target.value })}
                      />
                    </label>
                    <label>
                      <span>{t("tools.numberFrom")}</span>
                      <input
                        className="mono"
                        type="number"
                        min={0}
                        value={rule.numberFrom}
                        title={t("tools.numberFromTip")}
                        onChange={(e) => setRule({ ...rule, numberFrom: Number(e.target.value) || 0 })}
                      />
                    </label>
                    <label>
                      <span>{t("tools.numberPad")}</span>
                      <input
                        className="mono"
                        type="number"
                        min={0}
                        max={8}
                        value={rule.numberPad}
                        onChange={(e) => setRule({ ...rule, numberPad: Number(e.target.value) || 0 })}
                      />
                    </label>
                  </div>
                  <div className="rn-flags">
                    <label className="chk">
                      <input
                        type="checkbox"
                        checked={rule.keepExt}
                        onChange={(e) => setRule({ ...rule, keepExt: e.target.checked })}
                      />
                      {t("tools.keepExt")}
                    </label>
                    <label className="chk">
                      <input
                        type="checkbox"
                        checked={rule.lower}
                        onChange={(e) => setRule({ ...rule, lower: e.target.checked, upper: false })}
                      />
                      {t("tools.lower")}
                    </label>
                    <label className="chk">
                      <input
                        type="checkbox"
                        checked={rule.upper}
                        onChange={(e) => setRule({ ...rule, upper: e.target.checked, lower: false })}
                      />
                      {t("tools.upper")}
                    </label>
                    <button className="btn" disabled={busy || !ids.length} onClick={() => void makePlan()}>
                      {t("tools.makePlan")}
                    </button>
                    <button
                      className="btn btn-accent"
                      disabled={busy || plan.filter((r) => !r.error).length === 0}
                      onClick={() => void applyPlan()}
                    >
                      {t("tools.applyPlan")}
                    </button>
                  </div>

                  {plan.length > 0 && (
                    <table className="rn-table">
                      <thead>
                        <tr>
                          <th>{t("tools.oldName")}</th>
                          <th>{t("tools.newName")}</th>
                          <th>{t("tools.status")}</th>
                        </tr>
                      </thead>
                      <tbody>
                        {plan.slice(0, 400).map((r) => (
                          <tr key={r.id} className={r.error ? "bad" : ""}>
                            <td className="mono">{r.oldName}</td>
                            <td className="mono">{r.newName}</td>
                            <td>{r.error || t("tools.ok")}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  )}
                </>
              )}
            </div>

            <footer className="kb-foot">{t("tools.footer")}</footer>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
