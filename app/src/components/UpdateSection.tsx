import { useCallback, useEffect, useRef, useState } from "react";
import { onAppEvent, source } from "../data/source";
import type { Prefs, UpdateInfo, UpdateProgress } from "../types";
import { Icon } from "./ui";

/**
 * 设置 → 更新（0.1.35）。
 *
 * 逻辑照搬 FontManager 的 `UpdateSection.tsx`，界面用 AssetManager 自己的原语重写
 * （`Icon` 而不是 lucide-react、`onToast` 而不是 FM 的 toast/i18n、`.set-*` 而不是
 * `settings-*`；AM 没有 i18n，文案直接写中文）。
 *
 * ## 边界（与后端 `src-tauri/src/update.rs` 顶部一致）
 *
 * 1. **只读检查**：`am_update_check` 只打接口读版本号，不下载、不执行任何远端内容。
 * 2. **下载必须过校验**：哈希对不上后端直接删掉并抛错；装之前还会再算一次。
 * 3. **只给「按用户安装」的 NSIS 版做静默安装**：perMachine 的 MSI 需要 UAC，
 *    静默跑必然卡在一个没人能点的框上，所以这一版永远退回「打开发布页」。
 * 4. **仍然可以彻底关掉**：`updateCheck` 关掉后连版本号都不查（App 的启动检查也不发请求）。
 * 5. **仍然可以跳过某个版本**：`updateSkipped` 记住用户点过「跳过」的版本号。
 *
 * 安装动作会重启软件 —— 这是用户点「下载并安装」时就已经接受的代价
 * （Windows 不允许覆盖正在运行的 exe，必须先退出）。
 */

/**
 * 「装不了」的具体原因 → 人话。
 *
 * 后端会回报 `autoInstallBlocked`，不再只有一句含糊的「这一版没有可直接安装的包」——
 * 用户以为「点了下载」却什么都没发生时，界面要能直接说清是哪条闸没过。
 */
const BLOCKED_HINT: Record<string, string> = {
  system: "这一版是按系统范围安装的（MSI），静默升级要过一次 UAC。点「打开发布页」下载新的安装包手动装一次。",
  portable: "这份是从文件夹里直接跑的副本，没有安装记录，覆盖不了自己。点「打开发布页」下载新的安装包。",
  "no-asset": "这一版的发布里没有 NSIS 安装包，只能手动下载。",
  "no-digest": "发布里没给安装包的校验值，为了不装来路不明的文件，不做静默安装。点「打开发布页」手动下载。",
};

const blockedHint = (code: string): string =>
  BLOCKED_HINT[code] ?? "这一版不能自动安装，点「打开发布页」手动下载。";

/** 进度条下面那行字：三个阶段各一句话 */
function progressText(p: UpdateProgress): string {
  if (p.phase === "download") {
    const mb = (n: number) => (n / 1024 / 1024).toFixed(1);
    return p.total > 0
      ? `正在下载安装包 ${p.percent}%（${mb(p.received)} / ${mb(p.total)} MB）`
      : "正在下载安装包…";
  }
  if (p.phase === "verify") return "正在校验安装包…";
  if (p.phase === "ready") return "安装包已就绪，正在安装…";
  return "出错了";
}

export function UpdateSection({
  prefs,
  onPrefs,
  onToast,
}: {
  prefs: Prefs;
  onPrefs: (p: Partial<Prefs>) => void;
  onToast: (m: string, err?: boolean, d?: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** 下载 / 校验 / 安装的实时状态；null = 没在走这条路径 */
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  /**
   * 一键安装的一次性闸门。
   * `progress` 是 state，从点下到重渲染之间有一帧空档：手快连点两下
   * （或触控板双击）会进两次 downloadAndInstall，下两份安装包、起两个安装进程。
   * ref 是同步写入的，第二下直接被挡掉。失败时才解锁，允许重试。
   */
  const installLock = useRef(false);

  /* 后端每下 200ms 推一次进度；监听器随组件卸载一起摘掉。
     浏览器预览下 onAppEvent 是 no-op，这里不用另做分支。 */
  useEffect(() => onAppEvent<UpdateProgress>("update:progress", setProgress), []);

  /**
   * 检查更新。返回结果给调用方（「下载并安装」内部要先查一次再下载）；
   * `silent` = 不弹错误 toast（那条路自己会把错误写进 catch 分支）。
   */
  const check = useCallback(
    async (force: boolean, silent = false): Promise<UpdateInfo | null> => {
      setBusy(true);
      setError(null);
      try {
        const next = await source.updateCheck(force);
        setInfo(next);
        /* 无论有没有新版都记一次时间：闸门管的是「多久问一次接口」，
           不是「多久提醒一次」，否则天天点检查就天天打接口。 */
        onPrefs({ updateLastCheck: Date.now() });
        return next;
      } catch (e) {
        setError(String(e));
        if (!silent) onToast("检查更新失败", true, String(e));
        return null;
      } finally {
        setBusy(false);
      }
    },
    [onPrefs, onToast]
  );

  const skip = () => {
    if (!info) return;
    onPrefs({ updateSkipped: info.latest });
    onToast(`已跳过 ${info.latest}`, false, "下一版出来还会提醒；想提前看就去项目主页");
  };

  /**
   * 「下载并安装」完整链路（按钮常驻，不要求先点「检查更新」）：
   * 检查（顺带刷新缓存）→ 有新版 → 下载（进度条）→ 校验（后端做）→ 静默安装重启。
   * 已是最新就安静地收尾；这一版装不了（MSI / 缺直链）就退回「打开发布页」。
   */
  const downloadAndInstall = async () => {
    if (progress || installLock.current) return;
    installLock.current = true;
    setError(null);
    try {
      /* 已经知道这一版装不了：此刻按钮显示的就是「打开发布页」，点它就该跳浏览器
         （原因就在按钮下面那行字里，不用再查一遍接口） */
      if (info?.hasUpdate && !info.canAutoInstall) {
        await source.openUrl(info.url).catch((e) => onToast(String(e), true));
        return;
      }
      const cur = info?.hasUpdate ? info : await check(true, true);
      if (!cur) return;
      if (!cur.hasUpdate) {
        onToast("已是最新版本");
        return;
      }
      if (!cur.canAutoInstall) {
        /* 不擅自把浏览器顶到前面（用户以为「点了下载」结果跳走很莫名其妙）。
           说清是哪条闸没过，按钮自己会变成「打开发布页」，跳不跳由用户决定。 */
        const why = blockedHint(cur.autoInstallBlocked);
        setError(why);
        onToast("这一版不能自动安装", true, why);
        return;
      }
      setProgress({ phase: "download", received: 0, total: 0, percent: 0 });
      const path = await source.updateDownload(
        cur.setupUrl,
        cur.setupName,
        cur.sha256,
        cur.setupAssetId
      );
      setProgress({ phase: "ready", received: 1, total: 1, percent: 100 });
      await source.updateInstall(path, cur.sha256, cur.latest);
      /* 走到这里安装脚本已经起来了，后端会在 1 秒多之后退出本进程 */
    } catch (e) {
      setProgress(null);
      setError(String(e));
      onToast("下载或安装失败", true, String(e));
    } finally {
      installLock.current = false;
    }
  };

  const desktop = source.desktop;
  const downloading = progress !== null && progress.phase !== "error";
  const manualOnly = info !== null && info.hasUpdate && !info.canAutoInstall;
  const skipped = info !== null && info.hasUpdate && prefs.updateSkipped === info.latest;

  return (
    <div className="set-update">
      <div className="set-update-actions">
        <button
          className="btn"
          disabled={busy || downloading || !desktop}
          title={desktop ? "问一次 GitHub 上最新的版本号" : "浏览器预览下不可用"}
          onClick={() => void check(true)}
        >
          <Icon name="refresh" size={12} stroke={1.6} className={busy ? "set-update-spin" : undefined} />
          {busy ? "检查中…" : "立即检查"}
        </button>

        {/* 「下载并安装」常驻（不要求先点检查）。只有查到新版但这一版装不了
            （MSI / 从文件夹直接跑 / 发布缺包）时，同一位置才退化成「打开发布页」——
            那种情况下跳浏览器是唯一出路。 */}
        {!downloading && (
          <button
            className="btn"
            disabled={busy || !desktop}
            title={desktop ? "下载这一版的安装包并安装（会重启软件）" : "浏览器预览下不可用"}
            onClick={() => void downloadAndInstall()}
          >
            <Icon name={manualOnly ? "link" : "download"} size={12} stroke={1.6} />
            {manualOnly ? "打开发布页" : "下载并安装"}
          </button>
        )}

        {info?.hasUpdate && !skipped && !downloading && (
          <button
            className="btn"
            disabled={!desktop}
            /* 禁用时必须说"为什么不能点"，不能还写着"这一版不再提醒" ——
               三个按钮的 disabled 态文案口径要一致。 */
            title={desktop ? "这一版不再提醒（下一版出来还会提醒）" : "浏览器预览下不可用"}
            onClick={skip}
          >
            跳过这个版本
          </button>
        )}
      </div>

      {downloading && progress && (
        <div className="set-update-progress">
          <div
            className="set-update-bar"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={progress.percent}
            aria-label="安装包下载进度"
          >
            {/* 下限 2%：刚点下的时候也要看得见有东西在动，0% 的空槽看起来像没反应 */}
            <i style={{ width: `${Math.max(progress.percent, 2)}%` }} />
          </div>
          <div className="set-update-line">{progressText(progress)}</div>
        </div>
      )}

      {error ? (
        <div className="set-update-line">{error}</div>
      ) : info ? (
        <div className="set-update-line">
          {info.hasUpdate
            ? `发现新版本 ${info.latest}（当前 ${info.current}）`
            : `已是最新（${info.current}）`}
          {/* 只有 raw / cdn 兜底时才说明「没有发布说明」：正常路径不用画蛇添足 */}
          {info.hasUpdate && info.source !== "github" ? " · 这一版没有发布说明" : ""}
          {skipped ? " · 这一版你选择了跳过，不再提醒" : ""}
        </div>
      ) : (
        <div className="set-update-line">还没查过。点「立即检查」问一次远端版本号。</div>
      )}

      {/* 有新版但这一版装不了：说清楚是为什么，别让用户以为按钮坏了 */}
      {manualOnly && info && !error && (
        <div className="set-update-line">{blockedHint(info.autoInstallBlocked)}</div>
      )}

      {!desktop && (
        <div className="set-update-line">浏览器预览下不能检查更新，也下载不了安装包 —— 请用桌面版。</div>
      )}

      {/* 发布说明是纯文本（后端已截断到 4000 字）：用 <pre> 保换行，
          绝不当 HTML 渲染 —— 那是远端内容，渲染进来就是一个注入面。 */}
      {!error && info?.hasUpdate && info.notes.trim() && (
        <pre className="set-notes">{info.notes}</pre>
      )}
    </div>
  );
}
