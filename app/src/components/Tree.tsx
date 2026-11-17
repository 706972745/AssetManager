import { useRef, useEffect, useState, type ReactNode } from "react";
import { motion } from "motion/react";
import type { DirNode, Source } from "../types";
import { BlinkCaretInput, Icon } from "./ui";

/** 侧栏行：图标 15 + 文字，与 FontManager .nav-row 同一套内距（行内 11 起、文字 35 起）。 */
const ROW_PAD = 11;
const DEPTH_STEP = 14;
/** 最多几个素材库（与 App 的 MAX_LIBS 同值；标题上的 x/3 用它）。
 *  **只算用户自己的库** —— 内置截图库不占名额（owner 2026-11-17）。 */
const MAX_LIBS = 3;

/** 切库时"选中的那个挪到最上面"的动效：略带回弹的弹簧，跟 FontManager 同一手感。 */
const LIB_SPRING = { type: "spring", stiffness: 460, damping: 36, mass: 0.7 } as const;

/** 路径比较口径（与后端 same_path 一致）：斜杠统一、去尾分隔符、小写。 */
function samePath(a: string, b: string) {
  const f = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  return f(a) === f(b);
}

/** 展开状态持久化的 key（T14）。
 *  存**路径**而不是目录 id：重建索引后 id 会变，存 id 就会把不相干的目录
 *  张冠李戴地展开 —— 那种 bug 用户根本看不懂是怎么来的。 */
const OPEN_KEY = "am.tree.open.v1";

/** 返回 `null` = **从来没存过**（首次启动），空 Set = 用户手动全收起来了。
 *  两者必须分开：否则用户收好树，重启又被"贴心"地展开一次。 */
function loadOpenPaths(): Set<string> | null {
  try {
    const raw = localStorage.getItem(OPEN_KEY);
    if (!raw) return null;
    const arr = JSON.parse(raw);
    return Array.isArray(arr) ? new Set(arr.filter((x) => typeof x === "string")) : null;
  } catch {
    return null;
  }
}

function saveOpenPaths(paths: Set<string>) {
  try {
    localStorage.setItem(OPEN_KEY, JSON.stringify([...paths]));
  } catch {
    /* 存不进去就本次会话有效 */
  }
}

/** 当前"打开着的"那个素材库（存路径，重建索引后 id 会变）。
 *  FontManager 的字体库是同一个模型：**多个库只开一个**，其余在列表里等着切换。 */
const ACTIVE_LIB_KEY = "am.lib.active.v1";

function loadActiveLib(): string | null {
  try {
    return localStorage.getItem(ACTIVE_LIB_KEY);
  } catch {
    return null;
  }
}

function saveActiveLib(path: string) {
  try {
    localStorage.setItem(ACTIVE_LIB_KEY, path);
  } catch {
    /* 存不进去就本次会话有效 */
  }
}

interface Props {
  source: Source;
  selected: number | null;
  onSelect: (n: DirNode) => void;
  onDropTo: (target: DirNode) => void;
  /** 侧栏 + 内联输入提交后回调：把这条路径加进库（真的会扫盘） */
  onAddPath: (path: string) => void;
  onOpenLibrary: () => void;
  onTrash: () => void;
  onAbout: () => void;
  onShortcuts: () => void;
  refreshKey: number;
  /** 快照面板（0.12.0）：常驻在标签面板上方一行行列出快照 */
  snapshotDock?: ReactNode;
  /** 标签抽屉（默认收起、悬浮左缘展开）。侧栏只负责给它一块位置，内容与状态都在 App 里 */
  tagDock?: ReactNode;
}

export function Tree({
  source,
  selected,
  onSelect,
  onDropTo,
  onAddPath,
  onOpenLibrary,
  onTrash,
  onAbout,
  onShortcuts,
  refreshKey,
  snapshotDock,
  tagDock,
}: Props) {
  const [roots, setRoots] = useState<DirNode[]>([]);
  const [kids, setKids] = useState<Map<number, DirNode[]>>(new Map());
  /* 一次只开一个库：activePath 指的那个展开目录树，其余只留一行「切换」 */
  const [activePath, setActivePath] = useState<string | null>(loadActiveLib);
  const [open, setOpen] = useState<Set<number>>(new Set());
  /* 展开的目录路径集合（持久化用）；id ↔ path 的映射靠 nodesRef 维护 */
  const openPaths = useRef<Set<string> | null>(loadOpenPaths());
  const openSet = () => {
    if (!openPaths.current) openPaths.current = new Set();
    return openPaths.current;
  };
  const nodesRef = useRef<Map<number, DirNode>>(new Map());
  const [sectionOpen, setSectionOpen] = useState(true);
  const [hover, setHover] = useState<number | null>(null);
  /* 侧栏 + 的内联输入（与 FontManager 的「+ 新建分组」同一套互动：
     原地长出一行输入框，回车提交 / Esc 取消 / 失焦提交）。 */
  const [adding, setAdding] = useState(false);
  const [addPath, setAddPath] = useState("");
  const closedRef = useRef(false);
  /* 内置库（软件自己的截图目录）：不占名额、不能移除、永远在列表里 */
  const [builtin, setBuiltin] = useState<string[]>([]);

  useEffect(() => {
    void source
      .builtinRoots()
      .then(setBuiltin)
      .catch(() => setBuiltin([]));
  }, [source]);

  const isBuiltin = (p: string) => builtin.some((b) => samePath(b, p));

  const openAdd = () => {
    closedRef.current = false;
    setAddPath("");
    setAdding(true);
  };
  const closeAdd = () => {
    closedRef.current = true;
    setAdding(false);
    setAddPath("");
  };
  const commitAdd = () => {
    if (closedRef.current) return;
    const p = addPath.trim();
    closedRef.current = true;
    setAdding(false);
    setAddPath("");
    if (p) onAddPath(p);
  };

  const remember = (list: DirNode[]) => {
    for (const n of list) nodesRef.current.set(n.id, n);
  };

  /** 按持久化的路径把树恢复出来：逐层 load，最多 6 层（再深就让用户自己点）。 */
  const restore = async (roots: DirNode[]) => {
    if (openPaths.current === null) {
      // 首次启动：只有一个库就顺手展开，别让用户对着一个折叠箭头猜
      if (roots.length === 1) {
        setOpen(new Set([roots[0].id]));
        openPaths.current = new Set([roots[0].path]);
        saveOpenPaths(openPaths.current);
      } else {
        openPaths.current = new Set();
      }
      return;
    }
    const want = openPaths.current;
    if (!want.size) {
      setOpen(new Set());
      return;
    }
    const next = new Set<number>();
    let level = roots.filter((r) => want.has(r.path));
    for (const n of level) next.add(n.id);
    for (let depth = 0; depth < 6 && level.length; depth++) {
      const kidsList = await Promise.all(level.map((n) => source.children(n.id).catch(() => [])));
      const merged = new Map<number, DirNode[]>();
      level.forEach((n, i) => merged.set(n.id, kidsList[i]));
      setKids((m) => {
        const nm = new Map(m);
        for (const [k, v] of merged) nm.set(k, v);
        return nm;
      });
      const flat = kidsList.flat();
      remember(flat);
      level = flat.filter((c) => want.has(c.path));
      for (const n of level) next.add(n.id);
    }
    setOpen(next);
  };

  useEffect(() => {
    void source.roots().then((r) => {
      // 库根可能变了（重建索引 / 加删库），把已经不存在的路径丢掉，
      // 否则 localStorage 里会越攒越多死路径
      setRoots(r);
      remember(r);
      void restore(r);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source, refreshKey]);

  const load = async (id: number) => {
    if (kids.has(id)) return;
    const ch = await source.children(id);
    remember(ch);
    setKids((m) => new Map(m).set(id, ch));
  };

  const toggle = async (n: DirNode) => {
    const next = new Set(open);
    const paths = openSet();
    if (next.has(n.id)) {
      next.delete(n.id);
      paths.delete(n.path);
    } else {
      next.add(n.id);
      paths.add(n.path);
      await load(n.id);
    }
    setOpen(next);
    saveOpenPaths(paths);
  };

  /* 点了某个目录但它是折叠的？展开它（用户预期"我点了就该看到里面"）。
     注意不改 openPaths —— 这是被动展开，用户没做过"展开"的决定。 */
  const selectNode = (n: DirNode) => {
    onSelect(n);
  };

  /* 当前库：存的路径还在就认它；被移除/换盘了就退回第一个库 */
  /* **截图（内置）单独一段**（owner 2026-11-17：和快照一样，不是素材库的一行）：
     它不参与"一次只开一个"的切库，也不占名额，所以先把两类库拆开。 */
  const shotRoot = roots.find((r) => isBuiltin(r.path)) ?? null;
  const userRoots = roots.filter((r) => !isBuiltin(r.path));
  const [shotOpen, setShotOpen] = useState(true);

  const activeRoot = userRoots.find((r) => r.path === activePath) ?? userRoots[0] ?? null;

  /** 从某个节点往上找到它所属的库根（nodesRef 只存已加载的节点，够用） */
  const rootOf = (id: number): DirNode | null => {
    let cur: DirNode | undefined = nodesRef.current.get(id);
    const seen = new Set<number>();
    while (cur && cur.parentId !== null && !seen.has(cur.id)) {
      seen.add(cur.id);
      cur = nodesRef.current.get(cur.parentId);
    }
    return cur ?? null;
  };

  /* 选中项落在别的库里（右键移除库、外部跳转、快照激活）→ 那个库自动变成"开着的" */
  useEffect(() => {
    if (selected == null) return;
    const r = rootOf(selected);
    /* 内置截图库不是"打开着的素材库"，别把 activePath 改到它头上 */
    if (r && !isBuiltin(r.path) && r.path !== activeRoot?.path) {
      setActivePath(r.path);
      saveActiveLib(r.path);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, roots]);

  /** 切库 = 打开它（展开 + 载入子目录）并把列表切到它的库根 */
  const switchLib = (r: DirNode) => {
    setActivePath(r.path);
    saveActiveLib(r.path);
    void (async () => {
      if (!open.has(r.id)) {
        await load(r.id);
        const next = new Set(open).add(r.id);
        setOpen(next);
        const paths = openSet();
        paths.add(r.path);
        saveOpenPaths(paths);
      }
      onSelect(r);
    })();
  };

  /** 内置库小标：一眼看出"这行不是你加的" */
  const builtinChip = (bi: boolean, active: boolean) =>
    bi ? (
      <span
        className={`lib-builtin${active ? " on" : ""}`}
        title="软件自己的截图目录：不占素材库名额，也不能移除"
      >
        内置
      </span>
    ) : null;

  /** 非当前库：一行「图标 + 名字 + 数量 + 切换」，点整行就切过去 */
  const libRow = (r: DirNode) => {
    const bi = isBuiltin(r.path);
    return (
      <div
        key={r.id}
        className="tree-row lib-row"
        data-dir-path={r.path}
        data-dir-name={r.name}
        data-dir-root="1"
        /* 内置库：右键菜单靠它不给「从素材库移除」 */
        data-dir-builtin={bi ? "1" : undefined}
        style={{ paddingLeft: ROW_PAD }}
        onClick={() => switchLib(r)}
        title={`${r.path}\n点一下切换到这个素材库${
          bi ? "\n（软件自己的截图目录，不占素材库名额）" : ""
        }`}
      >
        <span className="nav-icon lib-off">
          <Icon name="folder" size={15} stroke={1.5} />
        </span>
        {/* 内置库显示成「截图」：目录名 screenshots 是英文，用户看不出这是软件自己的 */}
        <span className="tree-name lib-off">{bi ? "截图" : r.name}</span>
        {builtinChip(bi, false)}
        <span className="tree-count">{r.assetCnt > 0 ? r.assetCnt : 0}</span>
        <span className="lib-switch">切换</span>
      </div>
    );
  };

  /** 一行目录（**只画自己这一行**；子目录由 row / libBlock 铺） */
  const rowSelf = (n: DirNode, depth: number) => {
    const isOpen = open.has(n.id);
    const bi = n.parentId === null && isBuiltin(n.path);
    return (
      <div
        className={`tree-row ${selected === n.id ? "sel" : ""} ${hover === n.id ? "drop-hover" : ""}`}
        data-dir-path={n.path}
        data-dir-name={n.name}
        /* 库根标记：右键菜单靠它决定要不要给「从素材库移除」（子目录没有这一项） */
        data-dir-root={n.parentId === null ? "1" : undefined}
        data-dir-builtin={bi ? "1" : undefined}
        style={{ paddingLeft: ROW_PAD + depth * DEPTH_STEP }}
        onClick={() => selectNode(n)}
        onDragOver={(e) => {
          e.preventDefault();
          setHover(n.id);
        }}
        onDragLeave={() => setHover(null)}
        onDrop={(e) => {
          e.preventDefault();
          setHover(null);
          // 只认「应用内拖动」这个自定义类型：从资源管理器拖进来的文件
          // 不该把当前选中的素材搬走（那是两个完全不同的意图）
          if (!e.dataTransfer.types.includes("application/x-am-assets")) return;
          onDropTo(n);
        }}
        title={n.path}
      >
        {/* 箭头压在左内距里（绝对定位），这样图标/文字与参照物同一列 */}
        <span
          className={`tree-caret ${isOpen ? "open" : ""}`}
          style={{
            left: depth * DEPTH_STEP,
            visibility: n.hasChildren ? "visible" : "hidden",
          }}
          onClick={(e) => {
            e.stopPropagation();
            if (n.hasChildren) void toggle(n);
          }}
        >
          <Icon name="chevron" size={10} stroke={2} />
        </span>
        <span className="nav-icon">
          <Icon name="folder" size={15} stroke={1.5} />
        </span>
        <span className="tree-name">{bi ? "截图" : n.name}</span>
        {builtinChip(bi, true)}
        <span className="tree-count">{n.assetCnt > 0 ? n.assetCnt : 0}</span>
      </div>
    );
  };

  const row = (n: DirNode, depth: number) => {
    const isOpen = open.has(n.id);
    const list = kids.get(n.id);
    return (
      <div key={n.id}>
        {rowSelf(n, depth)}
        {isOpen && list?.map((c) => row(c, depth + 1))}
      </div>
    );
  };

  /** 一个库 = 一块（行 + 它展开的子树）。
   *
   * 为什么要按「块」包一层 `motion.div layout`：owner 2026-11-17 要求
   * **切到哪个库，哪个库就挪到最上面，并且要有移动上去的动画**。
   * 用 motion 的 layout 动画，顺序一变浏览器就做 FLIP 位移 ——
   * 手写 transform 得自己测前后位置、还要跟着子树高度变化走，容易抖。 */
  const libBlock = (r: DirNode) => {
    const isActive = r.path === activeRoot?.path;
    return (
      <motion.div
        key={r.path}
        layout
        transition={LIB_SPRING}
        className="lib-block"
        /* 子树高度变化（展开/收起）时不缩放文字，只做位移 */
        style={{ position: "relative" }}
      >
        {isActive ? rowSelf(r, 0) : libRow(r)}
        {isActive && open.has(r.id) && kids.get(r.id)?.map((c) => row(c, 1))}
      </motion.div>
    );
  };

  /* 顺序：**当前库永远第一**，其余按加入次序排队。切库时这一行会滑到顶上。 */
  const orderedRoots = (() => {
    const act = userRoots.find((r) => r.path === activeRoot?.path);
    if (!act) return userRoots;
    return [act, ...userRoots.filter((r) => r.id !== act.id)];
  })();

  /* 名额只数用户自己的库：内置截图库一直存在，算进去等于只剩 2 个位置 */
  const userLibCount = userRoots.length;

  return (
    <aside className="tree">
      <div className="tree-scroll scroll-hot">
        <div className="tree-section">
          <div className="tree-heading-row">
            <span
              className={`caret ${sectionOpen ? "" : "closed"}`}
              onClick={() => setSectionOpen((v) => !v)}
              style={{ cursor: "default" }}
            >
              <Icon name="chevron" size={10} stroke={2} className="rot90" />
            </span>
            <span className="grow">素材库</span>
            {/* 多库上限（最多 3 个）写在标题上：用户不用点开菜单就知道还能加几个。
                **内置截图库不计入** —— 否则三个自己的库只能挂两个。 */}
            <span
              className="lib-quota mono"
              title={`已加入 ${userLibCount} / ${MAX_LIBS} 个素材库（软件自己的截图目录不计入）`}
            >
              {userLibCount}/{MAX_LIBS}
            </span>
            <button
              className="side-add"
              title="添加素材文件夹"
              aria-label="添加素材文件夹"
              onClick={(e) => {
                e.stopPropagation();
                if (adding) closeAdd();
                else openAdd();
              }}
            >
              <Icon name="plus" size={12} stroke={1.8} />
            </button>
          </div>
          {adding && (
            <div className="tree-input-row">
              <span className="nav-icon">
                <Icon name="folder" size={15} stroke={1.5} />
              </span>
              <BlinkCaretInput
                autoFocus
                value={addPath}
                onChange={setAddPath}
                placeholder="粘贴文件夹路径，回车加入"
                inputClassName="tree-input"
                ariaLabel="加入素材库的文件夹路径"
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitAdd();
                  else if (e.key === "Escape") {
                    e.stopPropagation();
                    closeAdd();
                  }
                }}
                onBlur={commitAdd}
              />
            </div>
          )}
          {sectionOpen && orderedRoots.map(libBlock)}
        </div>

        {/* 截图（内置）：**单独一段**，和「快照」并列，不是素材库里的一行
            （owner 2026-11-17 原话：「截图哪个单独一个，和快照一样」）。
            它不占那 3 个名额、也不能移除 —— 混在素材库里就会被当成"用户第 3 个库"，
            用户再加自己的库时莫名其妙被顶掉。 */}
        {shotRoot && (
          <div className="tree-section">
            <div
              className="tree-heading-row shot-head"
              data-dir-path={shotRoot.path}
              data-dir-name={shotRoot.name}
              data-dir-root="1"
              data-dir-builtin="1"
              title={`${shotRoot.path}\n软件截图存放目录（内置，不占素材库名额）`}
              onClick={() => {
                /* 点段头 = 打开这个库：展开子目录 + 列表切到它（和点一行目录同一个结果） */
                void (async () => {
                  if (!open.has(shotRoot.id)) {
                    await load(shotRoot.id);
                    setOpen(new Set(open).add(shotRoot.id));
                  }
                  onSelect(shotRoot);
                })();
              }}
            >
              <span
                className={`caret ${shotOpen ? "" : "closed"}`}
                onClick={(e) => {
                  e.stopPropagation();
                  setShotOpen((v) => !v);
                }}
              >
                <Icon name="chevron" size={10} stroke={2} className="rot90" />
              </span>
              <span className="grow">截图</span>
              <span className="lib-builtin">内置</span>
              <span className="tree-count">{shotRoot.assetCnt > 0 ? shotRoot.assetCnt : 0}</span>
            </div>
            {shotOpen && kids.get(shotRoot.id)?.map((c) => row(c, 0))}
          </div>
        )}

        {/* 快照段 = 本滚动区里的第二个 section，紧跟「素材库」这一段（照 FontManager：
            `.sidebar-scroll` 里放 browse + snapshots 两段）。
            **不能**放到滚动区外面：`.tree-scroll` 是 flex:1，放到外面会被推到侧栏底部，
            看着就像「标签」上方多出来的一块底栏（owner 2026-11-17 指出过一次）。 */}
        {snapshotDock}
      </div>

      {tagDock}

      {/* 底栏固定四行（照 FontManager：打开库 / 回收站 / 关于 / 快捷键） */}
      <div className="tree-foot">
        <div
          className="tree-row"
          onClick={onOpenLibrary}
          title="在资源管理器中打开库根目录"
        >
          <span className="nav-icon">
            <Icon name="reveal" size={15} stroke={1.5} />
          </span>
          <span className="tree-name">在资源管理器中打开库</span>
        </div>
        <div className="tree-row" onClick={onTrash} title="打开 Windows 回收站（删除就是进这里，可还原）">
          <span className="nav-icon">
            <Icon name="trash" size={15} stroke={1.5} />
          </span>
          <span className="tree-name">回收站</span>
        </div>
        <div className="tree-row" onClick={onAbout} title="关于">
          <span className="nav-icon">
            <Icon name="info" size={15} stroke={1.5} />
          </span>
          <span className="tree-name">关于</span>
        </div>
        <div className="tree-row" onClick={onShortcuts} title="快捷键一览">
          <span className="nav-icon">
            <Icon name="keyboard" size={15} stroke={1.5} />
          </span>
          <span className="tree-name">快捷键</span>
          <span className="side-kbd">?</span>
        </div>
      </div>
    </aside>
  );
}
