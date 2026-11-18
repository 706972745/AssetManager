import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { HotkeyField } from "./HotkeyField";
import { loadHotkeys, saveHotkeys, type HotkeySet } from "../lib/hotkeys";
import { fmtSize, type Prefs, type Source, type WarmProgress } from "../types";
import { Icon } from "./ui";
import { DataFolderCard } from "./DataFolderCard";
import { LibraryCard } from "./LibraryCard";
import { onAppEvent } from "../data/source";

/**
 * 设置面板：左列导航 + 右侧分组卡片 + 右下角「应用 / 保存设置」，
 * 但**语言是 FontManager 的**——0 圆角侧栏行、2px 蓝竖线选中、1px 发丝线卡片、
 * 全局那套「平时 1px、靠近 6px」的滚动条（见 tokens.css 的 *::-webkit-scrollbar）。
 *
 * 交互纪律：所有控件都真的改东西。没有「记录快捷键」「重置全部」这类没实现的空壳按钮。
 */

export type SettingsKey = "general" | "libs" | "appearance" | "cache" | "shortcuts" | "about";

const NAV: { key: SettingsKey; label: string; icon: string }[] = [
  { key: "general", label: "常用", icon: "sliders" },
  { key: "libs", label: "素材库", icon: "folder" },
  { key: "appearance", label: "外观", icon: "image" },
  { key: "cache", label: "缓存与索引", icon: "database" },
  { key: "shortcuts", label: "快捷键", icon: "keyboard" },
  { key: "about", label: "关于", icon: "info" },
];

interface RowDef {
  id: string;
  label: string;
  hint?: string;
  /** 搜索命中的额外关键词（标签本身也会匹配） */
  kw?: string;
  /** 整块内容（自己带标签与说明），不套「左标签右控件」那一行 */
  block?: boolean;
  node: ReactNode;
}

interface CacheStat {
  bytes: number;
  files: number;
}

export function Settings({
  open,
  initialSection,
  prefs,
  source,
  onApply,
  onClose,
  onToast,
  onResetDenied,
  onAddFolder,
  onAddPath,
  onRemoveLib,
}: {
  open: boolean;
  /** 首次打开定位到哪一页（`?settings=cache` 用；不传就是「常用」） */
  initialSection?: SettingsKey;
  prefs: Prefs;
  source: Source;
  onApply: (p: Prefs) => void;
  onClose: () => void;
  onToast: (msg: string, err?: boolean, detail?: string) => void;
  /** 清空「不再建议」的自动标签名单 */
  onResetDenied: () => void;
  /** 添加素材库（走 App 那条：上限检查 + 重复判断 + 扫描进度） */
  onAddFolder: () => Promise<void>;
  /** 按指定路径加库（「数据文件夹选错了 → 改成加入素材库」那条路用） */
  onAddPath: (path: string) => Promise<void>;
  /** 从素材库移除一个库根（只清索引） */
  onRemoveLib: (path: string) => Promise<void>;
}) {
  const [draft, setDraft] = useState<Prefs>(prefs);
  const [section, setSection] = useState<SettingsKey>(initialSection ?? "general");
  /* 元数据 / 引用 / 库文件：都只在面板打开时拉，避免平白跑 IO */
  useEffect(() => {
    if (!open || !source.desktop) return;
    let dead = false;
    void source.metaMissing().then((v) => !dead && setMetaMiss(v)).catch(() => {});
    void source.refsSummary().then((v) => !dead && setRefSum(v)).catch(() => {});
    void source.libFiles().then((v) => !dead && setLibFiles(v)).catch(() => {});
    return () => {
      dead = true;
    };
  }, [open, source]);
  const [q, setQ] = useState("");
  const [stat, setStat] = useState<CacheStat | null>(null);
  const [total, setTotal] = useState<number | null>(null);
  const [warm, setWarm] = useState<WarmProgress | null>(null);
  const [busy, setBusy] = useState(false);
  /* 元数据缺口（T24/T26）：打开设置时拉一次，补算后刷新 */
  const [metaMiss, setMetaMiss] = useState<[number, number] | null>(null);
  const [refSum, setRefSum] = useState<[number, number] | null>(null);
  /* 每个库一个 DB 文件（T28）：用户要能看见"一个库一个文件" */
  const [libFiles, setLibFiles] = useState<[string, string, number][]>([]);
  /* 系统级快捷键（F1/F3）：改动立刻注册，注册失败的组合如实回显 */
  const [hotkeys, setHotkeys] = useState<HotkeySet>(() => loadHotkeys());

  /** 改一个系统快捷键：存起来 → 立刻重新注册三个 → 失败的单独说 */
  const applyHotkey = useCallback(
    async (which: keyof HotkeySet, accel: string) => {
      const next = { ...hotkeys, [which]: accel };
      setHotkeys(next);
      saveHotkeys(next);
      try {
        const failed = await source.hotkeysSet([
          [next.snip, "snip"],
          [next.clip, "clip_peek"],
        ]);
        if (failed.length) {
          onToast(`这些组合被别的软件占了：${failed.join("、")}`, true);
        } else {
          onToast(accel ? `已改为 ${accel}` : "已清空（这个功能暂时没有快捷键）");
        }
      } catch (e) {
        onToast(String(e), true);
      }
    },
    [hotkeys, onToast]
  );

  /** 重建索引是重操作（几万文件要十几秒），点一次先变成"再点一次确认" */
  const [confirmRebuild, setConfirmRebuild] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);

  /* 所有改动**即时生效**（FontManager 纪律 §9.3：底栏只写「即时生效」+「完成」）。
     以前这里有 draft + dirty + 「应用 / 保存设置」三个按钮，用户会以为不点就丢；
     而且真正的失败路径只有一条（写 localStorage 失败），不值得为它挡一层确认。 */
  const patch = (p: Partial<Prefs>) => {
    const next = { ...draft, ...p };
    setDraft(next);
    onApply(next);
  };

  const dirty = false;

  const refreshStat = () => {
    void source
      .cacheStat()
      .then(([bytes, files]) => setStat({ bytes, files }))
      .catch(() => setStat(null));
  };

  /* 每次打开都重新取真实数字：缓存占用、索引条数、数据目录 */
  useEffect(() => {
    if (!open) return;
    setDraft(prefs);
    setQ("");
    refreshStat();
    void source.total().then(setTotal).catch(() => setTotal(null));
    void source.warmStatus().then(setWarm).catch(() => setWarm(null));
    const off = onAppEvent<WarmProgress>("warm://progress", setWarm);
    return off;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  /* Esc 关闭；「应用 / 关闭」都走这里之前的手动调用 */
  useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // 两个监听都挂在 window 上：stopPropagation 只挡「传播到别的节点」，
        // 挡不住同一节点上后注册的冒泡监听（App 的全局快捷键）。必须 immediate，
        // 否则一次 Esc 会既关设置、又把详情面板也收掉。
        e.preventDefault();
        e.stopImmediatePropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [open, onClose]);

  useEffect(() => {
    bodyRef.current?.scrollTo({ top: 0 });
  }, [section, q]);

  const groups = useMemo<{ key: SettingsKey; title: string; rows: RowDef[] }[]>(() => {
    const seg = (value: number | string, options: { v: number | string; label: string }[], on: (v: any) => void) => (
      <div className="seg">
        {options.map((o) => (
          <button
            key={String(o.v)}
            className={`seg-btn ${value === o.v ? "on" : ""}`}
            onClick={() => on(o.v)}
          >
            {o.label}
          </button>
        ))}
      </div>
    );

    return [
      {
        key: "general",
        title: "常用",
        rows: [
          {
            id: "size",
            label: "默认缩略图尺寸",
            hint: "启动时用它；顶栏滑杆只改当前这一次",
            kw: "缩略图 尺寸 大小 格宽",
            node: seg(draft.size, [
              { v: 0, label: "小" },
              { v: 1, label: "中" },
              { v: 2, label: "大" },
            ], (v) => patch({ size: v as number })),
          },
          {
            id: "view",
            label: "默认视图",
            hint: "网格 / 瀑布流",
            kw: "视图 网格 瀑布流",
            node: seg(draft.view, [
              { v: "grid", label: "网格" },
              { v: "waterfall", label: "瀑布流" },
            ], (v) => patch({ view: v as Prefs["view"] })),
          },
        ],
      },
      {
        /* 素材库 = 用户的文件夹在哪（FontManager 那把「字体库文件夹」的直接对位）。
           放在这里的原因见 LibraryCard 顶部注释：用户找不到侧栏那个 +，
           来设置里找「库地址」，找不到就会把「数据文件夹」当素材库设。 */
        key: "libs",
        title: "素材库",
        rows: [
          {
            id: "library",
            label: "素材库",
            block: true,
            kw: "素材库 库地址 文件夹 添加 移除 路径 library root folder",
            node: (
              <LibraryCard
                source={source}
                onAddFolder={onAddFolder}
                onRemoveLib={onRemoveLib}
                onToast={onToast}
              />
            ),
          },
        ],
      },
      {
        key: "appearance",
        title: "外观",
        rows: [
          {
            id: "theme",
            label: "主题",
            hint: "深色是出厂设定（与 FontManager 同一套纸墨配色）；浅色适合亮环境",
            kw: "主题 深色 浅色 暗色 亮色 theme dark light",
            node: seg(draft.theme, [
              { v: "dark", label: "深色" },
              { v: "light", label: "浅色" },
            ], (v) => patch({ theme: v as Prefs["theme"] })),
          },
          {
            id: "motion",
            label: "动态效果",
            hint: "「减少」会把全站过渡压到 1ms，并关掉滚轮惯性——和系统「减少动态效果」同一效果",
            kw: "动效 动画 减少 motion reduce 惯性",
            node: seg(draft.motion, [
              { v: "system", label: "跟随系统" },
              { v: "reduced", label: "减少" },
            ], (v) => patch({ motion: v as Prefs["motion"] })),
          },
          {
            id: "tilename",
            label: "悬停显示文件名",
            hint: "关闭后只显示缩略图与格式徽标，界面更干净",
            kw: "文件名 悬停 hover 标签",
            node: (
              <button
                className={`switch ${draft.showTileName ? "on" : ""}`}
                role="switch"
                aria-checked={draft.showTileName}
                onClick={() => patch({ showTileName: !draft.showTileName })}
              >
                <span className="switch-knob" />
              </button>
            ),
          },
          {
            id: "autotags",
            label: "显示自动标签",
            hint: "按原始文件的格式 / 类型 / 像素 / 比例 / 体积推导；关掉后详情只留你手打的标签",
            kw: "自动标签 auto derived 标签 tag",
            node: (
              <button
                className={`switch ${draft.autoTags ? "on" : ""}`}
                role="switch"
                aria-checked={draft.autoTags}
                onClick={() => patch({ autoTags: !draft.autoTags })}
              >
                <span className="switch-knob" />
              </button>
            ),
          },
          {
            id: "autotags-reset",
            label: "恢复被否决的自动标签",
            hint: "在自动标签上右键「不再建议」只记在本机，这里一次性清空",
            kw: "否决 denied 不再建议 恢复 重置",
            node: (
              <button className="btn btn-tiny" onClick={onResetDenied}>
                恢复
              </button>
            ),
          },
        ],
      },
      {
        key: "cache",
        title: "缓存与索引",
        rows: [
          {
            id: "cache-size",
            label: "缩略图缓存",
            hint: stat ? `${stat.files.toLocaleString()} 个文件，全部可再生` : "正在统计…",
            kw: "缓存 占用 大小 缩略图 cache",
            node: (
              <span className="set-value mono">
                {stat ? fmtSize(stat.bytes) : "—"}
              </span>
            ),
          },
          {
            id: "warm-auto",
            label: "启动时自动预热缩略图",
            hint: "空闲时把全库的图先做一遍，之后滑到任何位置都是立刻出图",
            kw: "预热 预生成 后台 缩略图 warm 自动",
            node: (
              <button
                className={`switch ${draft.warmThumbs ? "on" : ""}`}
                role="switch"
                aria-checked={draft.warmThumbs}
                aria-label="启动时自动预热缩略图"
                onClick={() => patch({ warmThumbs: !draft.warmThumbs })}
              >
                <span className="switch-knob" />
              </button>
            ),
          },
          {
            id: "warm-now",
            label: "预生成全部缩略图",
            hint: !warm?.total
              ? "还没跑过。跑一次之后，整个库滑动全程零等待（低优先，随时可停）"
              : warm.running
                ? `进行中 ${warm.done.toLocaleString()} / ${warm.total.toLocaleString()}`
                : warm.done >= warm.total
                  ? `已完成 ${warm.total.toLocaleString()} 张 —— 现在滑到哪都是立刻出来`
                  : `上次停在 ${warm.done.toLocaleString()} / ${warm.total.toLocaleString()}`,
            kw: "预热 预生成 全库 缩略图 warm 立即",
            node: (
              <button
                className="btn"
                disabled={busy || !source.desktop}
                onClick={async () => {
                  setBusy(true);
                  try {
                    if (warm?.running) {
                      setWarm(await source.warmStop());
                      onToast("已停止预热", false, "已生成的缩略图都留着，下次接着做");
                    } else {
                      setWarm(await source.warmThumbs(draft.size === 2 ? 512 : draft.size === 0 ? 256 : 384));
                      onToast("已开始后台预热", false, "低优先跑，滚动时会让路；不用等它");
                    }
                  } catch (e) {
                    onToast(String(e), true);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {warm?.running ? "停止" : "开始"}
              </button>
            ),
          },
          {
            id: "meta-palette",
            label: "素材元数据",
            hint: "主色板与相似指纹在出缩略图时顺带算好；这里可以把老缓存的那批补算上",
            kw: "色板 主色 palette 颜色 指纹 重复 元数据",
            node: (
              <div className="set-inline">
                <span className="set-value mono">
                  {metaMiss ? `缺色板 ${metaMiss[0]} · 缺哈希 ${metaMiss[1]}` : "—"}
                </span>
                <button
                  className="btn"
                  disabled={busy || !source.desktop || (metaMiss?.[0] ?? 0) === 0}
                  onClick={async () => {
                    setBusy(true);
                    try {
                      const [done, left] = await source.fillPalette(1200);
                      const v = await source.metaMissing();
                      setMetaMiss(v);
                      onToast(
                        done > 0 ? `补算 ${done} 张` : "没有可补算的（缓存里还没有缩略图）",
                        false,
                        left > 0 ? `还剩 ${left} 张，再点一次继续` : undefined
                      );
                    } catch (e) {
                      onToast(String(e), true);
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  补算
                </button>
              </div>
            ),
          },
          {
            id: "refs",
            label: "工程引用",
            hint: "扫 psd / ai / blend / max / c4d 这类文件里写的外部素材路径，用来发现「工程还在、素材没了」",
            kw: "引用 refs 工程 psd blend 丢失 缺失",
            node: (
              <div className="set-inline">
                <span className="set-value mono">
                  {refSum ? `${refSum[0]} 条 · 丢失 ${refSum[1]}` : "—"}
                </span>
                <button
                  className="btn"
                  disabled={busy || !source.desktop}
                  onClick={async () => {
                    setBusy(true);
                    try {
                      const [n, r] = await source.refsScan(120);
                      const v = await source.refsSummary();
                      setRefSum(v);
                      onToast(`扫了 ${n} 个工程文件，记下 ${r} 条引用`);
                    } catch (e) {
                      onToast(String(e), true);
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  扫描
                </button>
              </div>
            ),
          },
          {
            id: "cache-limit",
            label: "缓存上限",
            hint: "超过后按最久未用淘汰（下次清理时生效）",
            kw: "上限 限制 淘汰 lru",
            node: (
              <div className="set-num">
                <input
                  type="number"
                  min={256}
                  step={256}
                  value={draft.cacheLimitMB}
                  onChange={(e) => {
                    const v = Number(e.target.value);
                    if (Number.isFinite(v) && v > 0) patch({ cacheLimitMB: Math.round(v) });
                  }}
                />
                <span className="set-unit">MB</span>
              </div>
            ),
          },
          {
            id: "cache-clear",
            label: "清理缩略图缓存",
            hint: "只删 cache/thumbs，不动任何原文件；清理后滚动时会重新生成",
            kw: "清理 删除 释放 空间 gc",
            node: (
              <button
                className="btn"
                disabled={busy || !source.desktop}
                onClick={async () => {
                  setBusy(true);
                  try {
                    const [before, freed] = await source.cacheGc(0, 0);
                    refreshStat();
                    onToast(
                      freed > 0 ? "缓存已清理" : "缓存已经是空的",
                      false,
                      freed > 0 ? `释放 ${fmtSize(freed)}（清理前 ${fmtSize(before)}）` : undefined
                    );
                  } catch (e) {
                    onToast(String(e), true);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {busy ? "清理中…" : "立即清理"}
              </button>
            ),
          },
          {
            id: "indexed",
            label: "已索引素材",
            hint: "索引是派生数据，随时可以重建；原文件不受影响",
            kw: "索引 素材 数量 条数",
            node: <span className="set-value mono">{total === null ? "—" : total.toLocaleString()}</span>,
          },
          {
            id: "rebuild",
            label: "重建索引",
            hint: confirmRebuild
              ? "会清空索引并全量重扫所有库根（几万文件约十几秒，期间可以取消）"
              : "索引跟磁盘对不上、换过硬盘的时候用；原文件不动",
            kw: "重建 索引 rebuild 重扫 修复",
            node: (
              <button
                className={`btn ${confirmRebuild ? "btn-danger" : ""}`}
                disabled={busy || !source.desktop}
                onClick={async () => {
                  if (!confirmRebuild) {
                    setConfirmRebuild(true);
                    window.setTimeout(() => setConfirmRebuild(false), 5000);
                    return;
                  }
                  setConfirmRebuild(false);
                  setBusy(true);
                  try {
                    await source.rebuildIndex();
                    onToast("已开始重建", false, "后台在跑，扫完会自动刷新；想停就点界面底部的取消");
                  } catch (e) {
                    onToast(String(e), true);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {confirmRebuild ? "再点一次确认" : "重建"}
              </button>
            ),
          },
          {
            id: "export",
            label: "导出用户数据",
            hint: "库根 + 评分 + 备注，写成 userdata-时间戳.json（索引可重建，这份是人写的东西）",
            kw: "导出 备份 export userdata 换机",
            node: (
              <button
                className="btn"
                disabled={busy || !source.desktop}
                onClick={async () => {
                  setBusy(true);
                  try {
                    const p = await source.exportUserdata();
                    onToast("已导出用户数据", false, p);
                  } catch (e) {
                    onToast(String(e), true);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                导出
              </button>
            ),
          },
          {
            id: "libfiles",
            label: "每个库的索引文件",
            block: true,
            kw: "库 文件 db 索引 独立 备份 拆库",
            hint: "拆库之后每个素材库各有一个独立文件：单个库坏了不影响别的，整库搬走只要拷一个文件",
            node: (
              <div className="lib-files">
                {libFiles.length === 0 && <span className="set-value">—</span>}
                {libFiles.map(([root, file, bytes]) => (
                  <div className="lib-file" key={file}>
                    <span className="lib-file-root" title={root}>
                      {root}
                    </span>
                    <span className="lib-file-path mono" title={file}>
                      {file}
                    </span>
                    <span className="set-value mono">{fmtSize(bytes)}</span>
                  </div>
                ))}
              </div>
            ),
          },
          {
            id: "datadir",
            /* 标签改成「索引与缓存位置」：叫「数据文件夹」时用户会理解成「我的素材放哪」，
               2026-11-16 就是这么把素材库根设进来的（然后觉得"没有任何效果"）。 */
            label: "索引与缓存位置",
            block: true,
            kw: "数据 目录 路径 位置 文件夹 自定义 迁移 换盘 data root 索引 缓存",
            node: (
              <DataFolderCard source={source} onToast={onToast} onAddPath={onAddPath} />
            ),
          },
        ],
      },
      {
        key: "shortcuts",
        title: "快捷键",
        rows: [
          {
            id: "hk-snip",
            label: "截图（选完区存到截图目录）",
            hint: "系统级：软件不在前台也能按",
            kw: "截图 抓屏 snip screenshot f1 系统 全局",
            node: (
              <HotkeyField
                value={hotkeys.snip}
                onChange={(v) => void applyHotkey("snip", v)}
              />
            ),
          },
          {
            id: "hk-clip",
            label: "把剪贴板里的图用无边框贴图打开",
            hint: "最近 10 张会留在历史里，窗内按 ← → 翻",
            kw: "无边框 看图 贴图 剪贴板 粘贴 f3 paste 系统 全局",
            node: (
              <HotkeyField value={hotkeys.clip} onChange={(v) => void applyHotkey("clip", v)} />
            ),
          },
          { id: "k1", label: "全屏预览选中的素材", kw: "空格 预览", node: <Kbd keys={["Space"]} /> },
          { id: "k2", label: "上一个 / 下一个", kw: "方向键 切换", node: <Kbd keys={["←", "→"]} /> },
          { id: "k3", label: "关闭预览 / 收起详情 / 关闭面板", kw: "esc 关闭 退出", node: <Kbd keys={["Esc"]} /> },
          { id: "k4", label: "撤销上一步文件操作", kw: "撤销 ctrl z", node: <Kbd keys={["Ctrl", "Z"]} /> },
          { id: "k5", label: "打开设置", kw: "设置 ctrl 逗号", node: <Kbd keys={["Ctrl", ","]} /> },
          {
            id: "k6",
            label: "命令面板（所有操作都能搜）",
            kw: "命令 面板 搜索 ctrl k palette",
            node: <Kbd keys={["Ctrl", "K"]} />,
          },
          { id: "k6", label: "用默认程序打开原文件", kw: "打开 双击", node: <Kbd keys={["双击"]} /> },
        ],
      },
      {
        key: "about",
        title: "关于",
        rows: [
          {
            id: "ver",
            label: "版本",
            kw: "版本 version 号",
            node: <span className="set-value mono">{__APP_VERSION__} 开发版</span>,
          },
          {
            id: "what",
            label: "定位",
            hint: "素材管理器，主场是百万级库的流畅度。索引式库：不搬动、不复制你的原文件。",
            kw: "说明 介绍 定位",
            node: <span className="set-value">索引式素材库</span>,
          },
          {
            id: "rule",
            label: "一条铁律",
            hint: "界面里的缩略图只用于显示；重命名 / 删除 / 拖出，作用对象永远是磁盘上的原文件。",
            kw: "铁律 原文件 缩略图",
            node: <span className="set-value">缩略图只是显示层</span>,
          },
        ],
      },
    ];
  }, [draft, stat, total, busy]);

  const needle = q.trim().toLowerCase();
  const shown = needle
    ? groups
        .map((g) => ({
          ...g,
          rows: g.rows.filter(
            (r) => r.label.toLowerCase().includes(needle) || (r.kw ?? "").toLowerCase().includes(needle)
          ),
        }))
        .filter((g) => g.rows.length > 0)
    : groups.filter((g) => g.key === section);

  const current = NAV.find((n) => n.key === section) ?? NAV[0];

  if (!open) return null;

  /* 入场走 CSS keyframes（不用 motion）：设置面板是常驻的大块 DOM，
     它的入场每帧都要重绘整棵子树，纯 CSS 动画交给合成器更稳；
     而且「减少动效」开关压的是 CSS 令牌，CSS 动画能自动跟着降级。 */
  return (
    <div className="set-scrim" onMouseDown={onClose}>
      <div
        className="set-sheet"
        role="dialog"
        aria-label="偏好设置"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <aside className="set-nav">
          <div className="set-nav-title">偏好设置</div>
          <div className="set-search">
            <Icon name="search" size={13} stroke={1.7} className="set-search-icon" />
            <input
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder="搜索设置…"
              aria-label="搜索设置"
            />
            {q && (
              <button className="set-search-clear" onClick={() => setQ("")} title="清空">
                <Icon name="close" size={11} stroke={1.8} />
              </button>
            )}
          </div>
          <div className="set-nav-list">
            {NAV.map((n) => (
              <button
                key={n.key}
                className={`set-nav-row ${section === n.key && !needle ? "sel" : ""}`}
                onClick={() => {
                  setQ("");
                  setSection(n.key);
                }}
              >
                <span className="set-nav-icon">
                  <Icon name={n.icon} size={14} stroke={1.5} />
                </span>
                <span className="set-nav-label">{n.label}</span>
              </button>
            ))}
          </div>
        </aside>

        <section className="set-main">
          <header className="set-head">
            <h2 className="set-head-title">{needle ? `搜索：${q.trim()}` : current.label}</h2>
            <button className="set-close" onClick={onClose} title="关闭（Esc）">
              <Icon name="close" size={15} stroke={1.5} />
            </button>
          </header>

          <div className="set-body scroll-hot" ref={bodyRef}>
            {shown.length === 0 ? (
              <p className="set-empty">没有匹配「{q.trim()}」的设置项。</p>
            ) : (
              shown.map((g) => (
                <div className="set-card" key={g.key}>
                  <div className="set-card-head">
                    {g.title}
                    <span className="set-card-count">{g.rows.length}</span>
                  </div>
                  {g.rows.map((r) =>
                    r.block ? (
                      <div className="set-row set-row-block" key={r.id}>
                        {r.node}
                      </div>
                    ) : (
                      <div className="set-row" key={r.id}>
                        <div className="set-row-text">
                          <div className="set-row-label">{r.label}</div>
                          {r.hint && <div className="set-row-hint">{r.hint}</div>}
                        </div>
                        <div className="set-row-ctl">{r.node}</div>
                      </div>
                    )
                  )}
                </div>
              ))
            )}
          </div>

          <footer className="set-foot">
            <span className="set-dirty">所有改动即时生效，不需要点保存</span>
            <button className="set-btn set-btn-accent" onClick={onClose}>
              完成
            </button>
          </footer>
        </section>
      </div>
    </div>
  );
}

function Kbd({ keys }: { keys: string[] }) {
  return (
    <span className="kbd-set">
      {keys.map((k) => (
        <kbd key={k}>{k}</kbd>
      ))}
    </span>
  );
}
