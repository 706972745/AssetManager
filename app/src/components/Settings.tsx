import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { HotkeyField } from "./HotkeyField";
import { hotkeyBindings, loadHotkeys, saveHotkeys, type HotkeySet } from "../lib/hotkeys";
import { fmtSize, MAIN_BORDER_DEFAULT, MAIN_BORDER_NONE, type Prefs, type Source, type WarmProgress } from "../types";
import { Icon } from "./ui";
import { DataFolderCard } from "./DataFolderCard";
import { LibraryCard } from "./LibraryCard";
import { UpdateSection } from "./UpdateSection";
import { onAppEvent } from "../data/source";
import { LangSelect } from "./LangSelect";
import {
  LOCALES,
  setLocalePref,
  useLocalePref,
  useT,
  type LocalePref,
  type TKey,
} from "../lib/i18n";

/**
 * 设置面板：左列导航 + 右侧分组卡片 + 右下角「应用 / 保存设置」，
 * 但**语言是 FontManager 的**——0 圆角侧栏行、2px 蓝竖线选中、1px 发丝线卡片、
 * 全局那套「平时 1px、靠近 6px」的滚动条（见 tokens.css 的 *::-webkit-scrollbar）。
 *
 * 交互纪律：所有控件都真的改东西。没有「记录快捷键」「重置全部」这类没实现的空壳按钮。
 */

/* 「关于」**不再是一页设置**（owner 2026-11-19）：主页已经有「关于」面板，
   两处维护同一份信息必然先对不上再打架。设置里只留真正"会改东西"的页。 */
export type SettingsKey =
  | "general"
  | "libs"
  | "appearance"
  | "cache"
  | "backup"
  | "shortcuts"
  | "update";

/**
 * 导航项。`label` 仍写中文 —— 它是**设置搜索的关键词**（用户用中文搜"外观"要能搜到），
 * 而且 `label` 还会当兜底文案用（万一某个语言的 json 漏了这条）。
 * 界面上显示的是 `tk` 翻出来的文字。
 */
const NAV: { key: SettingsKey; label: string; tk: TKey; icon: string }[] = [
  { key: "general", label: "通用", tk: "nav.general", icon: "sliders" },
  { key: "libs", label: "素材库", tk: "nav.libs", icon: "folder" },
  { key: "appearance", label: "外观", tk: "nav.appearance", icon: "image" },
  { key: "cache", label: "缓存与索引", tk: "nav.cache", icon: "database" },
  { key: "backup", label: "库数据与备份", tk: "nav.backup", icon: "download" },
  { key: "shortcuts", label: "快捷键", tk: "nav.shortcuts", icon: "keyboard" },
  { key: "update", label: "更新", tk: "nav.update", icon: "refresh" },
];

/** 窗口外阴影的四档预设（0.6.3）：主题紫 + 黑 / 白 / 灰。
 *  不做任意颜色：owner 明确要求"不用色轮，就紫色和其他黑白灰"。 */
const SHADOW_PRESETS: { v: string; swatch: string; tk: TKey }[] = [
  { v: "", swatch: "var(--accent-a)", tk: "settings.shadowTheme" },
  { v: "#000000", swatch: "#000000", tk: "settings.shadowBlack" },
  { v: "#FFFFFF", swatch: "#FFFFFF", tk: "settings.shadowWhite" },
  { v: "#8A8A8A", swatch: "#8A8A8A", tk: "settings.shadowGray" },
];

function ShadowSwatches({
  value,
  onChange,
  t,
}: {
  value: string;
  onChange: (v: string) => void;
  t: (k: TKey) => string;
}) {
  return (
    <div className="glow-picker" role="radiogroup" aria-label={t("settings.shadowColor")}>
      {SHADOW_PRESETS.map((p) => {
        const on = p.v === "" ? !value : value.toLowerCase() === p.v.toLowerCase();
        return (
          <button
            key={p.v || "theme"}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            title={t(p.tk)}
            aria-label={t(p.tk)}
            className={on ? "glow-chip on" : "glow-chip"}
            style={{ background: p.swatch }}
            onClick={() => onChange(p.v)}
          />
        );
      })}
    </div>
  );
}

/** 主界面边框预设（本次，照 FontManager 的 FrameGlowPicker）：
 *  主题紫 + 墨黑 / 纸白 / 中灰，末尾多一枚「无边框」（宽度归零）。
 *  只控制主窗那一圈 1px 边框；贴图窗 / 灵感板的 3px 阴影仍由上面那条阴影色决定。 */
const BORDER_PRESETS: { v: string; swatch?: string; tk: TKey }[] = [
  { v: MAIN_BORDER_DEFAULT, swatch: "var(--accent-a)", tk: "settings.shadowTheme" },
  { v: "#141412", swatch: "#141412", tk: "settings.shadowBlack" },
  { v: "#ffffff", swatch: "#ffffff", tk: "settings.shadowWhite" },
  { v: "#8a8a86", swatch: "#8a8a86", tk: "settings.shadowGray" },
  { v: MAIN_BORDER_NONE, tk: "settings.borderNone" },
];

function BorderSwatches({
  value,
  onChange,
  t,
}: {
  value: string;
  onChange: (v: string) => void;
  t: (k: TKey) => string;
}) {
  return (
    <div className="glow-picker" role="radiogroup" aria-label={t("settings.mainBorder")}>
      {BORDER_PRESETS.map((p) => {
        const on = value === p.v;
        const none = p.v === MAIN_BORDER_NONE;
        return (
          <button
            key={p.v}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            title={t(p.tk)}
            aria-label={t(p.tk)}
            className={`glow-chip${none ? " none" : ""}${on ? " on" : ""}`}
            style={p.swatch ? { background: p.swatch } : undefined}
            onClick={() => onChange(p.v)}
          />
        );
      })}
    </div>
  );
}

interface RowDef {
  id: string;
  label: string;
  hint?: string;
  /** 标签前的 lucide 图标名（走 `ui.tsx` 的 ICONS 映射；没有贴切的就不写） */
  icon?: string;
  /** 搜索命中的额外关键词（标签本身也会匹配） */
  kw?: string;
  /** hint 收进标签后的 ⓘ，不在标签下压一行灰字（语言行用） */
  hintInline?: boolean;
  /** 整块内容（自己带标签与说明），不套「左标签右控件」那一行 */
  block?: boolean;
  /** 原样渲染的块：不带 `.set-row`，所以不会被行发丝线切开（翻译块用） */
  bare?: boolean;
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
  activePath,
  updateAvailable,
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
  /**
   * 当前打开的目录（0.4.2）：导出资产包要按它定位"是哪个库"。
   * null = 还没选目录（那种情况下后端会退到当前库 / 第一个库）。
   */
  activePath?: string | null;
  /**
   * 启动时静默检查到的可更新版本号（null = 没有/还没查）。
   * 只在左侧「更新」那一行点一个小圆点 —— 设置面板关着的时候不占任何屏幕面积，
   * 用户主动打开设置才看得到「有这么一页该去点」，不做自动弹窗。
   */
  updateAvailable?: string | null;
}) {
  const t = useT();
  /* 语言偏好必须订阅：`t` 是模块级稳定函数，光调 `useT()` 不会让 `groups` 的
     useMemo 重算 —— 切了语言设置页文案会不跟着变。`localePref` 变化才是真信号。 */
  const localePref = useLocalePref();
  /* 开机自启的真实状态在注册表（后端），前端不知道；打开面板时问一次 */
  const [autoStart, setAutoStart] = useState<boolean | null>(null);
  /* 缩放实际生效倍率：选"自动"时后端会算一个值回来，界面要显示它 */
  const [scaleNow, setScaleNow] = useState<number | null>(null);
  /* 「导出资产包」那一行要显示的库名（null = 还没问到 / 没有库） */
  const [libName, setLibName] = useState<string | null>(null);

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
  /* 打开面板：问一次自启状态，并按当前偏好应用一次缩放（顺便拿到实际倍率） */
  useEffect(() => {
    if (!open || !source.desktop) return;
    void source.getAutostart().then(setAutoStart).catch(() => setAutoStart(null));
    void source.setUiScale(draft.uiScale).then(setScaleNow).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, source]);

  /* 「导出资产包」那一行要写清**导的是哪个库**（owner 2026-11-20：
     避免用户以为"只导当前文件夹"）。库名 = activePath 所属库根的文件夹名；
     嵌套库根时长前缀优先，与 core 的 `lib_for_path` 同一口径；拿不到就退回静态文案。 */
  useEffect(() => {
    if (!open || !source.desktop) return;
    let dead = false;
    void source
      .roots()
      .then((rs) => {
        if (dead) return;
        if (!rs.length) {
          setLibName(null);
          return;
        }
        if (!activePath) {
          setLibName(rs[0]?.name ?? null);
          return;
        }
        const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
        const ap = norm(activePath);
        const hit = rs
          .filter((r) => ap === norm(r.path) || ap.startsWith(`${norm(r.path)}\\`))
          .sort((a, b) => b.path.length - a.path.length)[0];
        setLibName(hit?.name ?? rs[0]?.name ?? null);
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, [open, activePath, source]);

  /* 「关闭到托盘 / 启动时最小化 / 缩放 / 回收站保留天数」必须同步给后端：
     这些都发生在 webview 起来之前，后端读不到 localStorage（见 main.rs 的 UiPrefs）。 */
  useEffect(() => {
    void source
      .uiPrefsSet({
        closeToTray: draft.closeToTray,
        startMinimized: draft.startMinimized,
        uiScale: draft.uiScale,
        trashRetentionDays: draft.trashRetentionDays,
        shadowColor: draft.shadowColor,
      })
      .catch(() => {});
  }, [
    draft.closeToTray,
    draft.startMinimized,
    draft.uiScale,
    draft.trashRetentionDays,
    draft.shadowColor,
    source,
  ]);

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
  /* 系统级快捷键（F1/F3/Alt+F3 组合/Shift+F3 交替）：改动立刻注册，注册失败的组合如实回显 */
  const [hotkeys, setHotkeys] = useState<HotkeySet>(() => loadHotkeys());
  /* 多库导出（ZIP / 用户数据）先弹窗选一个库：绝不把几个库攒成一份
     （owner 2026-11-20）。null = 没在选择中；run 是选定之后要做的那件事。 */
  const [libPick, setLibPick] = useState<{ roots: string[]; run: (root: string) => void } | null>(
    null
  );

  /** 改一个系统快捷键：存起来 → 立刻重新注册整套 → 失败的单独说 */
  const applyHotkey = useCallback(
    async (which: keyof HotkeySet, accel: string) => {
      const next = { ...hotkeys, [which]: accel };
      setHotkeys(next);
      saveHotkeys(next);
      try {
        const failed = await source.hotkeysSet(hotkeyBindings(next));
        if (failed.length) {
          onToast(t("settings.toast.hotkeysTaken", { keys: failed.join(t("app.common.list_sep")) }), true);
        } else {
          onToast(
            accel ? t("settings.toast.hotkeySet", { accel }) : t("settings.toast.hotkeyCleared")
          );
        }
      } catch (e) {
        onToast(String(e), true);
      }
    },
    [hotkeys, onToast, t]
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

  /**
   * 导出**一个**库的整库 zip（`root = null` 时由后端退到当前库）。
   *
   * 抽成函数是因为两个入口共用：设置里那一行（多库时先选中一个）、
   * 以及「素材库」页每一行后面的按钮。提示文案只有一份，别在两个地方漂移。
   */
  const runZipExport = useCallback(
    async (root: string | null) => {
      setBusy(true);
      try {
        const r = await source.exportLibraryZip(root);
        if (!r) return; // 用户在选文件夹那一步点了取消
        const names = r.parts.map((p) => p.name).join(t("app.common.list_sep"));
        const notes: string[] = [];
        if (r.parts.length > 1) notes.push(t("app.toast.zip_parts", { count: r.parts.length }));
        if (r.skipped > 0) notes.push(t("settings.toast.zipSkipped", { count: r.skipped }));
        if (r.oversized > 0) notes.push(t("app.toast.zip_oversized", { count: r.oversized }));
        onToast(
          t("app.toast.zip_exported", { count: r.files, names }),
          r.skipped > 0 || r.oversized > 0,
          notes.join(" · ") || undefined
        );
      } catch (e) {
        onToast(String(e), true);
      } finally {
        setBusy(false);
      }
    },
    [source, onToast, t]
  );

  /** 导出**一个库**的用户数据（0.4.5：`<库名>-userdata-<时间戳>.D-AssetManager`）。
      与 ZIP 不同，它不带原图，只有库根 + 评分 + 备注 + 标签 —— 换机后先扫一遍再导回来。 */
  const runDataExport = useCallback(
    async (root: string | null) => {
      setBusy(true);
      try {
        const p = await source.exportUserdata(root);
        onToast(t("settings.toast.userdataExported"), false, p);
      } catch (e) {
        onToast(String(e), true);
      } finally {
        setBusy(false);
      }
    },
    [source, onToast, t]
  );

  const groups = useMemo<
    { key: SettingsKey; tk: TKey; rows: RowDef[] }[]
  >(() => {
    /* 分段控件：`fill` = 撑满整行、各档等分（界面缩放那一排，FM `.scale-toggle`）。 */
    const seg = (
      value: number | string,
      options: { v: number | string; label: string }[],
      on: (v: any) => void,
      fill = false
    ) => (
      <div className={`seg ${fill ? "seg-fill" : ""}`}>
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

    /** 开关（和 seg 同一个位置定义，省得到处重复 role/aria） */
    const sw = (on: boolean, toggle: () => void) => (
      <button
        className={`switch ${on ? "on" : ""}`}
        role="switch"
        aria-checked={on}
        onClick={toggle}
      >
        <span className="switch-knob" />
      </button>
    );

    return [
      /* ---- 语言（0.3.30，owner 2026-11-19 指定照 FontManager）----
         九种语言 + 跟随系统；缺翻译自动回落英文（见 lib/i18n.ts）。 */
      {
        key: "general",
        tk: "lang.title",
        rows: [
          {
            id: "language",
            label: t("lang.label"),
            /* 语言行照 FontManager：图标 + 标签 + ⓘ，说明不再压成第二行灰字 */
            icon: "languages",
            hint: t("settings.langHint"),
            hintInline: true,
            kw: t("settings.kw.language"),
            node: (
              <LangSelect
                value={localePref}
                onChange={(v) => {
                  setLocalePref(v);
                  patch({ locale: v });
                }}
              />
            ),
          },
          {
            /* 「没有你的语言？」那一块：翻译指南 + 申请添加语言。
               这两个按钮真的能打开 GitHub —— 不做成不可点的装饰。
               `bare`：不进「左标签右控件」那行，也不吃 `.set-row + .set-row`
               的整行发丝线 —— 虚线框自己就是分界（照 FontManager 的
               `.settings-translate` 紧贴在语言行下面）。 */
            id: "language-missing",
            label: "",
            bare: true,
            kw: t("settings.kw.translate"),
            node: (
              <div className="set-translate">
                <div className="set-translate-text">
                  <div className="set-translate-title">
                    {t("lang.missing")}
                    <SetHint text={t("settings.translateHint")} />
                  </div>
                </div>
                <div className="set-translate-actions">
                  <button
                    className="set-data-btn"
                    onClick={() =>
                      void source
                        .openUrl("https://github.com/706972745/AssetManager#readme")
                        .catch((e) => onToast(String(e), true))
                    }
                  >
                    <Icon name="book-open" size={13} stroke={1.5} />
                    {t("lang.guide")}
                    <Icon
                      name="arrow-up-right"
                      size={12}
                      stroke={1.5}
                      className="set-btn-arrow"
                    />
                  </button>
                  <button
                    className="set-data-btn"
                    onClick={() =>
                      void source
                        .openUrl("https://github.com/706972745/AssetManager/issues/new")
                        .catch((e) => onToast(String(e), true))
                    }
                  >
                    <Icon name="message-plus" size={13} stroke={1.5} />
                    {t("lang.request")}
                    <Icon
                      name="arrow-up-right"
                      size={12}
                      stroke={1.5}
                      className="set-btn-arrow"
                    />
                  </button>
                </div>
              </div>
            ),
          },
        ],
      },
      /* ---- 系统：关闭到托盘 / 开机自启 / 启动时最小化 ---- */
      {
        key: "general",
        tk: "sys.title",
        rows: [
          {
            id: "closeToTray",
            label: t("settings.closeToTray"),
            hint: t("settings.closeToTrayHint"),
            kw: t("settings.kw.closeTray"),
            node: sw(draft.closeToTray, () => patch({ closeToTray: !draft.closeToTray })),
          },
          {
            id: "autostart",
            label: t("sys.autostart"),
            icon: "power",
            hint: t("sys.autostartHint"),
            kw: t("settings.kw.autostart"),
            node: sw(!!autoStart, () => {
              const next = !autoStart;
              /* 乐观更新：注册表写入通常几毫秒，但失败要退回去并说清楚 */
              setAutoStart(next);
              void source
                .setAutostart(next)
                .then(() =>
                  onToast(next ? t("settings.toast.autostartOn") : t("settings.toast.autostartOff"))
                )
                .catch((e) => {
                  setAutoStart(!next);
                  onToast(String(e), true);
                });
            }),
          },
          {
            id: "startMinimized",
            label: t("sys.startMinimized"),
            icon: "minimize",
            hint: t("sys.startMinimizedHint"),
            kw: t("settings.kw.startMinimized"),
            node: sw(draft.startMinimized, () => patch({ startMinimized: !draft.startMinimized })),
          },
        ],
      },
      {
        key: "general",
        tk: "nav.general",
        rows: [
          {
            id: "size",
            label: t("settings.defaultSize"),
            hint: t("settings.defaultSizeHint"),
            kw: t("settings.kw.size"),
            node: seg(draft.size, [
              { v: 0, label: t("topbar.size_small") },
              { v: 1, label: t("topbar.size_medium") },
              { v: 2, label: t("topbar.size_large") },
            ], (v) => patch({ size: v as number })),
          },
          {
            id: "view",
            label: t("settings.defaultView"),
            hint: t("settings.defaultViewHint"),
            icon: "grid",
            kw: t("settings.kw.view"),
            node: seg(draft.view, [
              { v: "grid", label: t("settings.viewGrid") },
              { v: "waterfall", label: t("settings.viewWaterfall") },
            ], (v) => patch({ view: v as Prefs["view"] })),
          },
        ],
      },
      {
        /* 素材库 = 用户的文件夹在哪（FontManager 那把「字体库文件夹」的直接对位）。
           放在这里的原因见 LibraryCard 顶部注释：用户找不到侧栏那个 +，
           来设置里找「库地址」，找不到就会把「数据文件夹」当素材库设。 */
        key: "libs",
        tk: "nav.libs",
        rows: [
          {
            id: "library",
            label: t("nav.libs"),
            block: true,
            kw: t("settings.kw.library"),
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
        tk: "nav.appearance",
        rows: [
          {
            id: "theme",
            label: t("settings.theme"),
            hint: t("settings.themeHint"),
            kw: t("settings.kw.theme"),
            node: seg(draft.theme, [
              { v: "light", label: t("settings.themeLight") },
              { v: "dark", label: t("settings.themeDark") },
              { v: "system", label: t("settings.themeSystem") },
            ], (v) => patch({ theme: v as Prefs["theme"] })),
          },
          {
            id: "shadowColor",
            label: t("settings.shadowColor"),
            hint: t("settings.shadowColorHint"),
            kw: t("settings.kw.shadowColor"),
            node: (
              <ShadowSwatches
                value={draft.shadowColor}
                onChange={(v) => patch({ shadowColor: v })}
                t={t}
              />
            ),
          },
          {
            id: "mainBorder",
            label: t("settings.mainBorder"),
            hint: t("settings.mainBorderHint"),
            kw: t("settings.kw.mainBorder"),
            node: (
              <BorderSwatches
                value={draft.mainBorder}
                onChange={(v) => patch({ mainBorder: v })}
                t={t}
              />
            ),
          },
          {
            id: "motion",
            label: t("settings.motion"),
            hint: t("settings.motionHint"),
            kw: t("settings.kw.motion"),
            node: seg(draft.motion, [
              { v: "system", label: t("lang.system") },
              { v: "reduced", label: t("settings.motionReduced") },
            ], (v) => patch({ motion: v as Prefs["motion"] })),
          },
          {
            id: "tilename",
            label: t("settings.tileName"),
            hint: t("settings.tileNameHint"),
            kw: t("settings.kw.tileName"),
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
            label: t("settings.autoTags"),
            hint: t("settings.autoTagsHint"),
            icon: "tag",
            kw: t("settings.kw.autoTags"),
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
            label: t("settings.resetDenied"),
            hint: t("settings.resetDeniedHint"),
            icon: "undo",
            kw: t("settings.kw.resetDenied"),
            node: (
              <button className="btn btn-tiny" onClick={onResetDenied}>
                {t("settings.restore")}
              </button>
            ),
          },
          /* ---- 界面缩放（0.3.30，图2 那一排）----
             走 `set_zoom`（浏览器缩放级别），不是 CSS `zoom`：
             这个软件的拖拽 / 框选 / 灯箱全都在算 getBoundingClientRect，
             CSS zoom 会把这些值一起放大，坐标换算全乱。 */
          {
            id: "uiscale",
            label: "",
            block: true,
            kw: t("settings.kw.zoom"),
            node: (
              <div className="set-zoom">
                <div className="set-zoom-head">
                  <span className="set-zoom-title">{t("zoom.title")}</span>
                  <span className="set-zoom-hint">
                    {t("zoom.hint", { percent: `${Math.round((scaleNow ?? 1) * 100)}%` })}
                  </span>
                </div>
                {seg(draft.uiScale === null ? "auto" : String(draft.uiScale), [
                  { v: "auto", label: t("zoom.auto") },
                  { v: "0.8", label: "80%" },
                  { v: "0.9", label: "90%" },
                  { v: "1", label: "100%" },
                  { v: "1.1", label: "110%" },
                  { v: "1.25", label: "125%" },
                ], (v) => {
                  const next = v === "auto" ? null : Number(v);
                  patch({ uiScale: next });
                  /* 立刻应用并把后端算出的实际倍率显示出来（选"自动"时才知道是多少） */
                  void source.setUiScale(next).then(setScaleNow).catch(() => {});
                }, true)}
              </div>
            ),
          },
        ],
      },
      {
        key: "cache",
        tk: "nav.cache",
        rows: [
          {
            id: "cache-size",
            label: t("settings.thumbCache"),
            icon: "database",
            hint: stat
              ? t("settings.cacheStat", { count: stat.files.toLocaleString() })
              : t("settings.cacheCounting"),
            kw: t("settings.kw.cacheSize"),
            node: (
              <span className="set-value mono">
                {stat ? fmtSize(stat.bytes) : "—"}
              </span>
            ),
          },
          {
            id: "warm-auto",
            label: t("settings.warmAuto"),
            hint: t("settings.warmAutoHint"),
            kw: t("settings.kw.warmAuto"),
            node: (
              <button
                className={`switch ${draft.warmThumbs ? "on" : ""}`}
                role="switch"
                aria-checked={draft.warmThumbs}
                aria-label={t("settings.warmAuto")}
                onClick={() => patch({ warmThumbs: !draft.warmThumbs })}
              >
                <span className="switch-knob" />
              </button>
            ),
          },
          {
            id: "warm-now",
            label: t("settings.warmNow"),
            hint: !warm?.total
              ? t("settings.warmNever")
              : warm.running
                ? t("settings.warmRunning", {
                    done: warm.done.toLocaleString(),
                    total: warm.total.toLocaleString(),
                  })
                : warm.done >= warm.total
                  ? t("settings.warmDone", { total: warm.total.toLocaleString() })
                  : t("settings.warmStopped", {
                      done: warm.done.toLocaleString(),
                      total: warm.total.toLocaleString(),
                    }),
            kw: t("settings.kw.warmNow"),
            node: (
              <button
                className="btn"
                disabled={busy || !source.desktop}
                onClick={async () => {
                  setBusy(true);
                  try {
                    if (warm?.running) {
                      setWarm(await source.warmStop());
                      onToast(t("settings.toast.warmStopped"), false, t("settings.toast.warmStoppedDetail"));
                    } else {
                      setWarm(await source.warmThumbs(draft.size === 2 ? 512 : draft.size === 0 ? 256 : 384));
                      onToast(t("settings.toast.warmStarted"), false, t("settings.toast.warmStartedDetail"));
                    }
                  } catch (e) {
                    onToast(String(e), true);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {warm?.running ? t("settings.stop") : t("settings.start")}
              </button>
            ),
          },
          {
            id: "meta-palette",
            label: t("settings.metaPalette"),
            hint: t("settings.metaPaletteHint"),
            kw: t("settings.kw.palette"),
            node: (
              <div className="set-inline">
                <span className="set-value mono">
                  {metaMiss
                    ? t("settings.metaMissing", { palette: metaMiss[0], hash: metaMiss[1] })
                    : "—"}
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
                        done > 0
                          ? t("settings.toast.paletteFilled", { count: done })
                          : t("settings.toast.paletteNone"),
                        false,
                        left > 0 ? t("settings.toast.paletteLeft", { count: left }) : undefined
                      );
                    } catch (e) {
                      onToast(String(e), true);
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  {t("settings.fillIn")}
                </button>
              </div>
            ),
          },
          {
            id: "refs",
            label: t("settings.refs"),
            hint: t("settings.refsHint"),
            icon: "link",
            kw: t("settings.kw.refs"),
            node: (
              <div className="set-inline">
                <span className="set-value mono">
                  {refSum
                    ? t("settings.refsSummary", { total: refSum[0], missing: refSum[1] })
                    : "—"}
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
                      onToast(t("settings.toast.refsScanned", { files: n, refs: r }));
                    } catch (e) {
                      onToast(String(e), true);
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  {t("settings.scan")}
                </button>
              </div>
            ),
          },
          {
            id: "cache-limit",
            label: t("settings.cacheLimit"),
            hint: t("settings.cacheLimitHint"),
            kw: t("settings.kw.cacheLimit"),
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
          /* 0.5.7：软件自己的回收站留多少天。`0` = 永不自动清理。
             清理只在启动时后台跑一次（见 main.rs 的启动线程），不是定时器 ——
             素材管理器常驻几天不动，定时器往往一次也不触发，反而更不可预期。 */
          {
            id: "trash-retention",
            label: t("settings.trashRetention"),
            hint: t("settings.trashRetentionHint"),
            icon: "trash",
            kw: t("settings.kw.trashRetention"),
            node: (
              <div className="set-num">
                <input
                  type="number"
                  min={0}
                  step={1}
                  value={draft.trashRetentionDays}
                  onChange={(e) => {
                    /* 空输入框不能当 0：用户全选删除准备重打时要保持原值，
                       否则一中间态就把「自动清理」悄悄关掉了 */
                    if (e.target.value === "") return;
                    const v = Number(e.target.value);
                    if (Number.isFinite(v) && v >= 0) {
                      patch({ trashRetentionDays: Math.round(v) });
                    }
                  }}
                />
                <span className="set-unit">{t("settings.trashRetentionUnit")}</span>
              </div>
            ),
          },
          {
            id: "cache-clear",
            label: t("settings.cacheClear"),
            hint: t("settings.cacheClearHint"),
            kw: t("settings.kw.cacheClear"),
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
                      freed > 0 ? t("app.toast.cache_cleared") : t("app.toast.cache_empty"),
                      false,
                      freed > 0
                        ? t("app.toast.cache_freed", {
                            freed: fmtSize(freed),
                            before: fmtSize(before),
                          })
                        : undefined
                    );
                  } catch (e) {
                    onToast(String(e), true);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {busy ? t("settings.cleaning") : t("settings.cleanNow")}
              </button>
            ),
          },
          {
            id: "indexed",
            label: t("settings.indexed"),
            hint: t("settings.indexedHint"),
            kw: t("settings.kw.indexed"),
            node: <span className="set-value mono">{total === null ? "—" : total.toLocaleString()}</span>,
          },
          {
            id: "rebuild",
            label: t("settings.rebuild"),
            icon: "refresh",
            hint: confirmRebuild
              ? t("settings.rebuildConfirmHint")
              : t("settings.rebuildHint"),
            kw: t("settings.kw.rebuild"),
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
                    onToast(
                      t("app.toast.reindex_started"),
                      false,
                      t("settings.toast.reindexStartedDetail")
                    );
                  } catch (e) {
                    onToast(String(e), true);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {confirmRebuild ? t("settings.confirmAgain") : t("app.common.rebuild")}
              </button>
            ),
          },
        ],
      },
      {
        /* ---- 库数据与备份（0.4.12）----
           导出三件套（.D 用户数据 / .P 资产包 / 整库 ZIP）原来混在「缓存与索引」里，
           owner 2026-11-21：「有些本该放在一起的却分开在不同的地方了」——
           全部库数据的进出（导出 + 库索引文件 + 数据位置）收进同一页。 */
        key: "backup",
        tk: "nav.backup",
        rows: [
          {
            id: "export",
            label: t("settings.exportUserdata"),
            icon: "download",
            hint: libName
              ? t("settings.exportUserdataHintLib", { name: libName })
              : t("settings.exportUserdataHint"),
            kw: t("settings.kw.exportData"),
            node: (
              <button
                className="btn"
                disabled={busy || !source.desktop}
                onClick={async () => {
                  const roots = libFiles.map(([root]) => root);
                  /* 多库 → 先选；单库 → 直接导。绝不把几个库合成一份（owner 2026-11-20） */
                  if (roots.length > 1) {
                    setLibPick({ roots, run: (r) => void runDataExport(r) });
                    return;
                  }
                  await runDataExport(roots[0] ?? activePath ?? null);
                }}
              >
                {t("settings.export")}
              </button>
            ),
          },
          {
            id: "package",
            label: t("settings.exportPackage"),
            icon: "download",
            hint: libName
              ? t("settings.exportPackageHintLib", { name: libName })
              : t("settings.exportPackageHint"),
            kw: t("settings.kw.exportPackage"),
            node: (
              <button
                className="btn"
                disabled={busy || !source.desktop}
                onClick={async () => {
                  setBusy(true);
                  try {
                    const r = await source.exportPackage(activePath ?? null);
                    /* null = 用户在保存对话框里点了取消 */
                    if (!r) return;
                    const file = r.path.split(/[\\/]/).pop() ?? r.path;
                    onToast(
                      t("settings.toast.packageExported", { count: r.ok, file }),
                      r.skipped > 0,
                      r.skipped
                        ? t("settings.toast.zipSkipped", { count: r.skipped })
                        : undefined
                    );
                  } catch (e) {
                    onToast(String(e), true);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {t("settings.export")}
              </button>
            ),
          },
          {
            id: "library-zip",
            label: t("settings.exportZip"),
            icon: "download",
            hint: libName
              ? t("settings.exportZipHintLib", { name: libName })
              : t("settings.exportZipHint"),
            kw: t("settings.kw.exportZip"),
            node: (
              <button
                className="btn"
                disabled={busy || !source.desktop}
                onClick={async () => {
                  const roots = libFiles.map(([root]) => root);
                  /* 多库 → 先选；单库 → 直接导。绝不"所有库打成一个包"（owner 2026-11-20） */
                  if (roots.length > 1) {
                    setLibPick({ roots, run: (r) => void runZipExport(r) });
                    return;
                  }
                  await runZipExport(roots[0] ?? activePath ?? null);
                }}
              >
                {t("settings.export")}
              </button>
            ),
          },
          {
            id: "libfiles",
            label: t("settings.libFiles"),
            block: true,
            kw: t("settings.kw.libFiles"),
            hint: t("settings.libFilesHint"),
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
            label: t("settings.dataRoot"),
            block: true,
            kw: t("settings.kw.dataRoot"),
            node: (
              <DataFolderCard source={source} onToast={onToast} onAddPath={onAddPath} />
            ),
          },
        ],
      },
      {
        key: "shortcuts",
        tk: "nav.shortcuts",
        rows: [
          {
            id: "hk-snip",
            label: t("app.menu.snip"),
            hint: t("shortcuts.hkSnipHint"),
            kw: t("settings.kw.hkSnip"),
            node: (
              <HotkeyField
                value={hotkeys.snip}
                onChange={(v) => void applyHotkey("snip", v)}
              />
            ),
          },
          {
            id: "hk-clip",
            label: t("app.menu.clip_peek"),
            hint: t("shortcuts.hkClipHint"),
            kw: t("settings.kw.hkClip"),
            node: (
              <HotkeyField value={hotkeys.clip} onChange={(v) => void applyHotkey("clip", v)} />
            ),
          },
          {
            id: "hk-toggle",
            label: t("shortcuts.hkToggle"),
            hint: t("shortcuts.hkToggleHint"),
            kw: t("settings.kw.hkToggle"),
            node: (
              <HotkeyField
                value={hotkeys.toggle}
                onChange={(v) => void applyHotkey("toggle", v)}
              />
            ),
          },
          /* 0.5.7：Alt+F3 归「一起开关」，Shift+F3 归「交替」；
             老的「只控制贴图」默认不绑键，留在上一行给需要的人自己配。 */
          {
            id: "hk-together",
            label: t("shortcuts.hkTogether"),
            hint: t("shortcuts.hkTogetherHint"),
            kw: t("settings.kw.hkTogether"),
            node: (
              <HotkeyField
                value={hotkeys.together}
                onChange={(v) => void applyHotkey("together", v)}
              />
            ),
          },
          {
            id: "hk-swap",
            label: t("shortcuts.hkSwap"),
            hint: t("shortcuts.hkSwapHint"),
            kw: t("settings.kw.hkSwap"),
            node: (
              <HotkeyField
                value={hotkeys.swap}
                onChange={(v) => void applyHotkey("swap", v)}
              />
            ),
          },
          { id: "k1", label: t("app.cmd.preview"), kw: t("settings.kw.preview"), node: <Kbd keys={["Space"]} /> },
          { id: "k2", label: t("shortcuts.prevNext"), kw: t("settings.kw.prevNext"), node: <Kbd keys={["←", "→"]} /> },
          { id: "k3", label: t("shortcuts.closeAll"), kw: t("settings.kw.closeAll"), node: <Kbd keys={["Esc"]} /> },
          { id: "k4", label: t("app.cmd.undo"), kw: t("settings.kw.undo"), node: <Kbd keys={["Ctrl", "Z"]} /> },
          { id: "k5", label: t("shortcuts.openSettings"), kw: t("settings.kw.openSettings"), node: <Kbd keys={["Ctrl", ","]} /> },
          {
            id: "k6",
            label: t("shortcuts.commandPalette"),
            kw: t("settings.kw.commandPalette"),
            node: <Kbd keys={["Ctrl", "K"]} />,
          },
          { id: "k6", label: t("shortcuts.openDefault"), kw: t("settings.kw.openDefault"), node: <Kbd keys={[t("shortcuts.kbdDouble")]} /> },
        ],
      },
      {
        /* 更新（0.1.35）：一开关 + 一整块（UpdateSection 自带按钮 / 进度条 / 状态行）。
           开关放在这一页而不是「常用」，是因为用户找更新一定先点「更新」这一页；
           关掉它连版本号都不查（不是"查了不提醒"），这一点必须写在 hint 里。 */
        key: "update",
        tk: "nav.update",
        rows: [
          {
            id: "update-auto",
            label: t("settings.updateAuto"),
            hint: t("settings.updateAutoHint"),
            icon: "refresh",
            kw: t("settings.kw.updateAuto"),
            node: (
              <button
                className={`switch ${draft.updateCheck ? "on" : ""}`}
                role="switch"
                aria-checked={draft.updateCheck}
                aria-label={t("settings.updateAuto")}
                onClick={() => patch({ updateCheck: !draft.updateCheck })}
              >
                <span className="switch-knob" />
              </button>
            ),
          },
          {
            id: "update-run",
            label: t("settings.updateRun"),
            block: true,
            kw: t("settings.kw.updateRun"),
            node: <UpdateSection prefs={draft} onPrefs={patch} onToast={onToast} />,
          },
        ],
      },
    ];
  }, [draft, stat, total, busy, t, localePref, autoStart, scaleNow, libName, activePath, source, onToast, libFiles, runZipExport, runDataExport, setLibPick]);

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
    <>
    <div className="set-scrim" onMouseDown={onClose}>
      <div
        className="set-sheet"
        role="dialog"
        aria-label={t("settings.dialogAria")}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <aside className="set-nav">
          <div className="set-nav-title">{t("settings.title")}</div>
          <div className="set-search">
            <Icon name="search" size={12} stroke={1.7} className="set-search-icon" />
            <input
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder={t("common.searchSettings")}
              aria-label={t("common.searchSettings")}
            />
            {q && (
              <button
                className="set-search-clear"
                onClick={() => setQ("")}
                title={t("settings.clearSearch")}
              >
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
                <span className="set-nav-label">{t(n.tk)}</span>
                {/* 有小圆点 = 启动时静默查到了新版（0.1.35）。role="img" 是为了让
                    aria-label 真被读屏念出来 —— 光秃秃的 <span> 里的 aria-label 会被忽略。 */}
                {n.key === "update" && updateAvailable ? (
                  <span
                    className="set-nav-dot"
                    role="img"
                    aria-label={t("settings.updateDot", { version: updateAvailable })}
                    title={t("settings.updateDot", { version: updateAvailable })}
                  />
                ) : null}
              </button>
            ))}
          </div>
        </aside>

        <section className="set-main">
          <header className="set-head">
            <h2 className="set-head-title">
              {needle ? t("settings.searchPrefix", { query: q.trim() }) : t(current.tk)}
            </h2>
            <button className="set-close" onClick={onClose} title={t("settings.closeEsc")}>
              <Icon name="close" size={15} stroke={1.5} />
            </button>
          </header>

          <div className="set-body scroll-hot" ref={bodyRef}>
            {shown.length === 0 ? (
              <p className="set-empty">{t("settings.empty", { query: q.trim() })}</p>
            ) : (
              /* key 用 `section:标题 key`：语言 / 系统 / 通用 三张卡片的 section 都是 "general"，
                 同 key 的兄弟节点会被 React 复用 —— 表现是「点快捷键，内容却是语言/系统，
                 还重复一遍」。0.3.30 装上去当天就被 owner 抓到，这里必须唯一。 */
              shown.map((g) => (
                /* 一个分组 = 一张卡，标题进卡内做**卡头**（FontManager `.detail-heading`）：
                   13/600 + 下边线，卡片无内距，行的左右 14px 与卡头对齐。 */
                <div className="set-sec" key={`${g.key}:${g.tk}`}>
                  <div className="set-card">
                    <div className="set-card-head">{t(g.tk)}</div>
                    {g.rows.map((r) =>
                      r.bare ? (
                        <div className="set-bare" key={r.id}>
                          {r.node}
                        </div>
                      ) : r.block ? (
                        <div className="set-row set-row-block" key={r.id}>
                          {r.node}
                        </div>
                      ) : (
                        <div className="set-row" key={r.id}>
                          <div className="set-row-text">
                            <div className="set-row-label">
                              {r.icon && (
                                <span className="set-row-icon">
                                  <Icon name={r.icon} size={13} stroke={1.5} />
                                </span>
                              )}
                              {r.label}
                              {r.hint && r.hintInline && <SetHint text={r.hint} />}
                            </div>
                            {r.hint && !r.hintInline && (
                              <div className="set-row-hint">{r.hint}</div>
                            )}
                          </div>
                          <div className="set-row-ctl">{r.node}</div>
                        </div>
                      )
                    )}
                  </div>
                </div>
              ))
            )}
          </div>

          <footer className="set-foot">
            <span className="set-dirty">{t("settings.footerNote")}</span>
            <button className="set-btn set-btn-accent" onClick={onClose}>
              {t("common.done")}
            </button>
          </footer>
        </section>
      </div>
    </div>

    {libPick && (
      <div className="dlg-scrim" onMouseDown={() => setLibPick(null)}>
        <div
          className="dlg"
          role="dialog"
          aria-modal="true"
          aria-label={t("settings.pickLibAria")}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <div className="dlg-title">{t("settings.pickLibTitle")}</div>
          <div className="dlg-body">
            <div className="zippick">
              {libPick.roots.map((root) => (
                <button
                  key={root}
                  className="zippick-row"
                  title={root}
                  onClick={() => {
                    const run = libPick.run;
                    setLibPick(null);
                    run(root);
                  }}
                >
                  <Icon name="folder" size={14} stroke={1.5} />
                  <span className="zippick-name">
                    {root.split(/[\\/]/).filter(Boolean).pop() ?? root}
                  </span>
                  <span className="zippick-path mono">{root}</span>
                </button>
              ))}
            </div>
          </div>
          <div className="dlg-actions">
            <button className="btn" onClick={() => setLibPick(null)}>
              {t("app.common.cancel")}
            </button>
          </div>
        </div>
      </div>
    )}
    </>
  );
}

/** 设置页专用的 ⓘ（照 FontManager `SettingHint`，132–138 行）。
 *
 *  说明文字收进悬停气泡：面板里一条设置占两行灰字，扫读时反而找不到设置项。
 *  气泡本身不是自己画的 —— 全项目共用 `HoverHintHost`（HoverHint.tsx）那套
 *  事件委托 + 边缘翻转，这里只负责提供一个可聚焦、带文案的锚点。
 *  用 `.set-hint` 而不是 `.settings-hint`：设置页所有类名都是 `.set-*`，
 *  不把 FontManager 的全局类引进这个项目。 */
function SetHint({ text }: { text: string }) {
  return (
    <span className="set-hint" data-hint-text={text} role="img" aria-label={text} tabIndex={0}>
      <Icon name="info" size={12} stroke={1.7} />
    </span>
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
