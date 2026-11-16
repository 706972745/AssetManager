import { useEffect, useState } from "react";
import type { DataRootInfo, Source } from "../types";
import { Icon } from "./ui";
import { useDialog } from "./Dialog";

/**
 * 设置 → 缓存与索引 →「索引与缓存位置」（原「数据文件夹」）。
 *
 * 管的是**索引库与缩略图缓存放在哪**，不是素材本身在哪 —— 素材库在「素材库」页。
 *
 * ## 名字为什么改了（2026-11-16 真事故）
 *
 * 原来叫「数据文件夹」（照 FontManager「字体库文件夹」的字面）。用户看到的是
 * 「数据文件夹」，理解成「我的素材数据放哪」，于是把自己的素材库根设了进来。
 * 结果：索引写进了素材目录，主界面还是 0/3 空状态 —— 「设置了文件库但没有任何效果」。
 * 现在：标题写「索引与缓存位置」、副标题明说「不是素材库」，而且**选完先看一眼里面有什么**，
 * 有图就问一句「你是想把这个文件夹加入素材库吧？」。
 *
 * ## 三条交互约定（与 FontManager 一致）
 *
 * 1. **点一行 = 切到这个文件夹**；行尾「当前」徽章标明正在用的那个。
 * 2. 行尾 `×` 只从清单里摘掉，**不删硬盘上的任何东西**。
 * 3. 切换**下次启动生效**（SQLite 连接启动时就绑定了文件路径），所以底部给一个「立即重启」；
 *    保存时可以把现有索引整份复制过去（缩略图缓存不复制，按需重建）。
 */
export function DataFolderCard({
  source,
  onToast,
  onAddPath,
}: {
  source: Source;
  onToast: (msg: string, err?: boolean, detail?: string) => void;
  /** 把文件夹加入素材库（用户选错地方时的正确出口） */
  onAddPath: (path: string) => Promise<void>;
}) {
  const dlg = useDialog();
  const [cfg, setCfg] = useState<DataRootInfo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** 保存成功但还没重启：界面要提示"重启才生效"，否则用户会以为没生效 */
  const [pending, setPending] = useState(false);

  const reload = () => {
    void source
      .dataRoot()
      .then((c) => {
        setCfg(c);
        setErr(null);
      })
      .catch((e) => {
        setCfg(null);
        // 读失败要说清原因，不能糊成"浏览器预览里没有……"—— 桌面端读失败是真 bug
        setErr(String(e));
      });
  };

  useEffect(reload, []);

  const save = async (
    next: { enabled: boolean; current: string | null; dirs: string[] },
    opts?: { copy?: boolean; force?: boolean }
  ) => {
    setBusy(true);
    try {
      const msg = await source.setDataRoot({
        ...next,
        copyIndex: opts?.copy ?? true,
        force: opts?.force ?? false,
      });
      onToast("索引位置已更新", false, msg);
      setPending(true);
      reload();
    } catch (e) {
      onToast(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  /**
   * 选一个位置 —— **先体检再保存**。
   *
   * 这里就是 2026-11-16 那次"没效果"的现场：用户选了自己的素材库根，
   * 代码二话不说就存了，重启后索引落在素材目录里，库列表还是空的。
   * 现在只要看见图/视频，就停下来问「你是想加素材库吧」，并给一条正确的出口。
   */
  const addFolder = async () => {
    if (!cfg) return;
    if (cfg.dirs.length >= cfg.max) {
      onToast(`最多 ${cfg.max} 个位置`, true);
      return;
    }
    try {
      const picked = await source.pickFolder("选择索引与缩略图缓存的存放位置（不是素材库）");
      if (!picked) return;

      const probe = await source.probeDir(picked).catch(() => null);
      if (probe && (probe.images > 0 || probe.videos > 0)) {
        const choice = await dlg.ask({
          title: "这个文件夹里都是素材",
          body: (
            <div className="grow">
              <p>
                这里已经有 <b>{probe.images.toLocaleString()}</b> 张图片
                {probe.videos > 0 ? `、${probe.videos.toLocaleString()} 个视频` : ""}。
              </p>
              <p style={{ marginTop: 8 }}>
                「索引与缓存位置」只放数据库和缩略图，<b>放在这里不会让素材出现在软件里</b>，
                反而会在你的素材目录里多出 <span className="mono">index.db</span>、
                <span className="mono">cache</span> 这些东西。
              </p>
              <p className="mono ellipsis" style={{ marginTop: 8, opacity: 0.7 }}>
                {picked}
              </p>
            </div>
          ),
          buttons: [
            { label: "取消", value: "cancel" },
            { label: "仍用作索引位置", value: "force" },
            { label: "加入素材库", value: "lib", tone: "accent" },
          ],
        });

        if (choice === "lib") {
          const n = await source.libCount().catch(() => 0);
          if (n >= 3) {
            onToast("素材库已经有 3 个了，先在「素材库」页移除一个", true);
            return;
          }
          await onAddPath(picked);
          return;
        }
        if (choice !== "force") return;
        if (cfg.dirs.some((d) => d.toLowerCase() === picked.toLowerCase())) {
          await save({ enabled: true, current: picked, dirs: cfg.dirs }, { force: true });
        } else {
          await save(
            { enabled: true, current: picked, dirs: [...cfg.dirs, picked] },
            { force: true }
          );
        }
        return;
      }

      if (cfg.dirs.some((d) => d.toLowerCase() === picked.toLowerCase())) {
        await save({ enabled: true, current: picked, dirs: cfg.dirs });
        return;
      }
      await save({ enabled: true, current: picked, dirs: [...cfg.dirs, picked] });
    } catch (e) {
      onToast(String(e), true);
    }
  };

  const remove = async (dir: string) => {
    if (!cfg) return;
    const next = cfg.dirs.filter((d) => d !== dir);
    if (next.length === 0) {
      await save({ enabled: false, current: null, dirs: [] }, { copy: false });
      return;
    }
    const current = cfg.current === dir ? next[0] : cfg.current;
    await save({ enabled: true, current, dirs: next });
  };

  if (!cfg) {
    return (
      <div className="dr-note">
        {err
          ? `读取索引位置失败：${err}`
          : source.desktop
            ? "正在读取索引位置…"
            : "浏览器预览里没有真实数据目录（桌面端才有）。"}
      </div>
    );
  }

  return (
    <div className="dr-card">
      <div className="dr-head">
        <span className="dr-title">索引与缓存位置</span>
        <span className="dr-sub">
          只放 index.db 与缩略图；<b>素材库不在这里改</b> —— 去左侧「素材库」页
        </span>
      </div>

      {/* 默认位置：只读灰条，和 FontManager 那条「默认 …」完全同构 */}
      <div className="dr-default" title={cfg.defaultDir}>
        <span className="dr-tag">默认</span>
        <Icon name="folder" size={13} stroke={1.5} />
        <span className="dr-path mono">{cfg.defaultDir}</span>
      </div>

      <div className="dr-switch-row">
        <button
          className={`switch ${cfg.enabled ? "on" : ""}`}
          role="switch"
          aria-checked={cfg.enabled}
          aria-label="使用自定义位置"
          disabled={busy}
          onClick={() => {
            if (cfg.enabled) {
              void save({ enabled: false, current: null, dirs: cfg.dirs }, { copy: false });
            } else if (cfg.dirs.length > 0) {
              void save({ enabled: true, current: cfg.dirs[0], dirs: cfg.dirs });
            } else {
              void addFolder();
            }
          }}
        >
          <span className="switch-knob" />
        </button>
        <span className="dr-switch-label">使用自定义位置</span>
        <span className="grow" />
        <button className="set-btn" onClick={() => void addFolder()} disabled={busy}>
          <Icon name="plus" size={12} stroke={1.7} />
          选择文件夹…
        </button>
      </div>

      {cfg.dirs.length > 0 && (
        <div className="dr-list">
          {cfg.dirs.map((d) => {
            const isCurrent = cfg.enabled && cfg.current === d;
            const isEffective = cfg.effectiveDir === d;
            return (
              <div
                key={d}
                className={`dr-row ${isCurrent ? "sel" : ""}`}
                onClick={() => {
                  if (!isCurrent || !cfg.enabled) {
                    void save({ enabled: true, current: d, dirs: cfg.dirs });
                  }
                }}
                title={isCurrent ? "正在使用" : "点一下切到这个文件夹"}
              >
                <Icon name="folder" size={13} stroke={1.5} className="dr-row-icon" />
                <span className="dr-path mono">{d}</span>
                {isEffective && <span className="dr-badge">当前</span>}
                <button
                  className="dr-act"
                  title="在资源管理器中打开"
                  onClick={(e) => {
                    e.stopPropagation();
                    void source.reveal(d).catch((err) => onToast(String(err), true));
                  }}
                >
                  <Icon name="reveal" size={12} stroke={1.5} />
                </button>
                <button
                  className="dr-act"
                  title="从清单移除（不删硬盘上的任何东西）"
                  onClick={(e) => {
                    e.stopPropagation();
                    void remove(d);
                  }}
                >
                  <Icon name="close" size={11} stroke={1.8} />
                </button>
              </div>
            );
          })}
        </div>
      )}

      <div className="dr-foot">
        <span className="dr-note">
          当前生效：<span className="mono">{cfg.effectiveDir}</span>
          {pending ? "　·　已改动，重启后生效" : ""}
        </span>
        <button
          className="set-btn"
          disabled={busy || !pending}
          onClick={() => void source.relaunch().catch((e) => onToast(String(e), true))}
        >
          立即重启
        </button>
      </div>
      {dlg.node}
    </div>
  );
}
