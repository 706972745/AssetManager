import { useCallback, useEffect, useState } from "react";
import type { DirNode, Source } from "../types";
import { onAppEvent } from "../data/source";
import { Icon } from "./ui";

/**
 * 设置 → 素材库（照 FontManager 的「字体库文件夹」那一块）。
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
 * ## 三条交互约定（与 FontManager 一致）
 *
 * 1. 每行显示**完整路径**（用户要能确认自己加的是哪个盘哪个文件夹），行尾可打开/移除。
 * 2. `×` 只清索引，**磁盘文件一个不动**（真删文件走主界面的回收站流程）。
 * 3. 到 3 个上限就**禁用按钮并说明原因**，不做「点了再弹一句抱歉」。
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
  const [roots, setRoots] = useState<DirNode[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /* 内置库（软件自己的截图目录）：不占名额、不给移除（owner 2026-11-17） */
  const [builtin, setBuiltin] = useState<string[]>([]);

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
  /* 只数用户自己的库：内置截图库一直存在，算进去等于只剩 2 个位置 */
  const count = roots?.filter((r) => !isBuiltin(r.path)).length ?? 0;
  const full = count >= max;

  return (
    <div className="lib-card">
      <div className="lib-head">
        {/* 不再重复写一遍「素材库」：页面标题已经是它了，标题叠标题只会让人以为有两块 */}
        <span className="lib-sub">
          {count}/{max} 个　·　只建索引，不移动、不复制原文件
        </span>
        <span className="grow" />
        <button
          className="set-btn set-btn-accent"
          disabled={busy || full || !source.desktop}
          title={full ? `最多 ${max} 个素材库，先移除一个` : "选一个文件夹加入素材库"}
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
          添加文件夹…
        </button>
      </div>

      {err && <div className="lib-note">{`读取素材库失败：${err}`}</div>}

      {!roots ? (
        <div className="lib-note">正在读取…</div>
      ) : roots.length === 0 ? (
        <div className="lib-empty">
          <Icon name="folder" size={20} stroke={1.4} />
          <div className="lib-empty-title">还没有素材库</div>
          <div className="lib-empty-hint">
            点右上角「添加文件夹…」选一个放素材的目录。软件只读它、建索引，
            不会动里面的文件。
          </div>
        </div>
      ) : (
        <div className="lib-list">
          {roots.map((r) => (
            <div className="lib-row" key={r.id} title={r.path}>
              <Icon name="folder" size={14} stroke={1.5} className="lib-row-icon" />
              <span className="lib-path mono">{r.path}</span>
              <span className="lib-cnt">{r.assetCnt.toLocaleString()} 个素材</span>
              {isBuiltin(r.path) && (
                <span className="lib-builtin on" title="软件自己的截图目录：不占名额，不能移除">
                  内置
                </span>
              )}
              <button
                className="dr-act"
                title="在资源管理器中打开"
                onClick={() => void source.reveal(r.path).catch((e) => onToast(String(e), true))}
              >
                <Icon name="reveal" size={12} stroke={1.5} />
              </button>
              {/* 内置库不给 ×：删了索引，截图就"凭空消失"了 */}
              <button
                className="dr-act"
                title={isBuiltin(r.path) ? "内置库不能移除" : "从素材库移除（不删文件）"}
                disabled={isBuiltin(r.path)}
                onClick={async () => {
                  setBusy(true);
                  try {
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

      <div className="lib-foot">
        <Icon name="info" size={12} stroke={1.5} />
        <span>
          素材库是「你的文件夹在哪」；索引与缩略图缓存存在别处（见「缓存与索引」页），两者不是一回事。
        </span>
      </div>
    </div>
  );
}
