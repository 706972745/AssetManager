import { useRef, useEffect, useState, type ReactNode } from "react";
import { motion } from "motion/react";
import type { DirNode, Source } from "../types";
import { BlinkCaretInput, Icon } from "./ui";
import { useT } from "../lib/i18n";

/** 侧栏行：图标 15 + 文字，与 FontManager .nav-row 同一套内距（行内 11 起、文字 35 起）。 */
const ROW_PAD = 11;
const DEPTH_STEP = 14;
/** 最多几个素材库（与 App 的 MAX_LIBS 同值；标题上的 x/3 用它）。
 *  **只算用户自己的库** —— 内置截图库不占名额（owner 2026-11-17）。 */
const MAX_LIBS = 3;

/** 切库时"选中的那个挪到最上面"的动效：略带回弹的弹簧，跟 FontManager 同一手感。 */
const LIB_SPRING = { type: "spring", stiffness: 460, damping: 36, mass: 0.7 } as const;

/* 切库三步的节拍：必须和 CSS / motion 的真实时长对齐，否则"等动画做完"是假的。
   `KIDS_MS` = `.tree-kids` 的 grid-template-rows 过渡（见 app.css）；
   `MOVE_MS` = 上面这个 spring 的落定时间（stiffness 460 / damping 36 实测约 280–320ms）。 */
const KIDS_MS = 140;
const MOVE_MS = 320;

const sleep = (ms: number) => new Promise<void>((res) => setTimeout(res, ms));

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

export function loadActiveLib(): string | null {
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

/**
 * 指针是不是落在标题栏左侧的品牌区（图标 + 应用名，右边再放宽 40px）。
 *
 * 为什么不直接 `closest(".titlebar-brand")`：那块是 `pointer-events: none`
 * （不能让品牌区抢走标题栏的拖动窗口手势），事件目标是背后的 `.titlebar`，
 * 结构上永远匹配不到。矩形每次现取：换语言、换字号、窗口宽度变化都会改它。
 */
function hitBrand(e: PointerEvent): boolean {
  const el = document.querySelector(".titlebar-brand");
  if (!el) return false;
  const r = el.getBoundingClientRect();
  return (
    e.clientY >= r.top - 4 &&
    e.clientY <= r.bottom + 4 &&
    e.clientX >= r.left &&
    e.clientX <= r.right + 40
  );
}

interface Props {
  source: Source;
  selected: number | null;
  onSelect: (n: DirNode) => void;
  onDropTo: (target: DirNode) => void;
  /** 拖动一个文件夹行（源文件级）：由 App 接管为应用内拖拽，出窗才交系统 */
  onDragDir: (path: string, x: number, y: number) => void;
  /** 出错/成功要说话（新建文件夹这类"真的动了磁盘"的动作不能静默） */
  onToast: (msg: string, err?: boolean, detail?: string) => void;
  onOpenLibrary: () => void;
  onTrash: () => void;
  onAbout: () => void;
  onShortcuts: () => void;
  refreshKey: number;
  /** 快照面板（0.12.0）：常驻在标签面板上方一行行列出快照 */
  snapshotDock?: ReactNode;
  /** 分组面板（0.2.x）：位置照 FontManager —— 底栏上方、**标签面板之上** */
  collDock?: ReactNode;
  /** 标签抽屉（默认收起、悬浮左缘展开）。侧栏只负责给它一块位置，内容与状态都在 App 里 */
  tagDock?: ReactNode;
  /**
   * 侧栏里有弹层开着（右键菜单 / 标签编辑卡 / 打标签弹窗）→ 抽屉**不许自动收起**。
   * 这些弹层都 portal 到 body，指针进菜单就不再属于 `.tree` 了。
   */
  holdOpen?: boolean;
}

export function Tree({
  source,
  selected,
  onSelect,
  onDropTo,
  onDragDir,
  onToast,
  onOpenLibrary,
  onTrash,
  onAbout,
  onShortcuts,
  refreshKey,
  snapshotDock,
  collDock,
  tagDock,
  holdOpen = false,
}: Props) {
  const t = useT();
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
  /** 已经拉过子列表的目录（`refreshKey` 变化时整体作废） */
  const loaded = useRef<Set<number>>(new Set());

  /* 0.4.24：侧栏默认收在视口左侧外，鼠标进左边缘 10px 才滑出来（覆盖网格，不挤压）。
     owner 2026-11-21：素材库不是每分钟都在点，但图要一直看得多。

     ⚠️ 用 document 级 pointermove 看坐标，**不能只挂 onPointerEnter/Leave**：
     侧栏滑出后会盖住那条热区，指针其实"从没进入侧栏"，离开时 leave 也不会来，
     结果就是滑出来之后再也收不回去（2026-11-21 实测踩过，自动化测试里 autoClose=false）。 */
  const [railOpen, setRailOpen] = useState(false);
  const railTimer = useRef<number | null>(null);
  /* 菜单/弹层开着时**不许收**（owner 2026-11-21）：
     右键菜单是 portal 到 body 的，指针一进菜单就不在 `.tree` 里了 —— 以前 240ms 后
     整条侧栏滑走，菜单还悬在半空，"右击分组 → 去点菜单项"这个过程根本走不完。
     用 ref 而不是把监听重挂一遍：监听注册一次就够，事件里读当时的值。 */
  const holdRef = useRef(false);
  holdRef.current = holdOpen;
  useEffect(() => {
    const onMove = (e: PointerEvent) => {
      const t = e.target as HTMLElement | null;
      /* 三处都算"想开侧栏"（owner 2026-11-21 逐条补的）：
         ① 视口左缘 10px（原始热区）；
         ② 已经滑出来的侧栏上（不然一进去就再也收不回去，见下面的注释）；
         ③ **标题栏左边那块品牌区**（图标 + "AssetManager"）—— 鼠标往上飘到窗口左上角
            是很自然的动作，那里离素材库最近，扫过去就该把侧栏带出来。
            这里只能**按坐标判**：`.titlebar-brand` 是 `pointer-events: none`
            （不能让品牌区抢走标题栏的拖窗口手势），事件目标是它背后的 `.titlebar`，
            `closest('.titlebar-brand')` 永远匹配不到 —— 第一版就是这么白写的。 */
      const keepOpen =
        e.clientX <= 10 ||
        (!!t && !!t.closest(".tree")) ||
        (e.clientY < 60 && e.clientX < 360 && hitBrand(e));
      if (keepOpen) {
        if (railTimer.current !== null) {
          window.clearTimeout(railTimer.current);
          railTimer.current = null;
        }
        setRailOpen(true);
        return;
      }
      if (holdRef.current) {
        if (railTimer.current !== null) {
          window.clearTimeout(railTimer.current);
          railTimer.current = null;
        }
        return;
      }
      if (railTimer.current !== null) window.clearTimeout(railTimer.current);
      railTimer.current = window.setTimeout(() => setRailOpen(false), 240);
    };
    document.addEventListener("pointermove", onMove, true);
    return () => {
      document.removeEventListener("pointermove", onMove, true);
      if (railTimer.current !== null) window.clearTimeout(railTimer.current);
    };
  }, []);

  const nodesRef = useRef<Map<number, DirNode>>(new Map());
  const [sectionOpen, setSectionOpen] = useState(true);
  const [hover, setHover] = useState<number | null>(null);
  /* 侧栏 + = **在当前库里就地新建文件夹**（owner 2026-11-17：
     "现在选中哪个文件库，就在哪个文件库下面创建文件夹，系统级的、库地址下也真实创建"）。
     交互照 FontManager 的「+ 新建分组」：原地长出一行输入框，回车提交 / Esc 取消 / 失焦提交。 */
  const [creating, setCreating] = useState<{ id: number; path: string } | null>(null);
  const [newName, setNewName] = useState("");
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

  /* 文件夹行也能拖（**源文件级**）：拖到别的目录 = 移动这个文件夹，
     拖到资源管理器 / PS = 把这个文件夹拷出去（系统拖放的 COPY 语义）。
     按下先记坐标，移动超过 6px 才起拖 —— 否则单击选目录会被当成拖动。 */
  const dirPress = useRef<{ path: string; x: number; y: number } | null>(null);
  const dirFired = useRef(false);
  const dirFiredTimer = useRef(0);

  const pressDir = (path: string, e: React.MouseEvent) => {
    if (e.button !== 0) return;
    dirPress.current = { path, x: e.clientX, y: e.clientY };
  };

  /** 拖动之后浏览器还会补一个 click（= 切到那个目录）：吃掉它，
      否则「把文件夹拖出去」会顺手把左侧选中的目录换掉。 */
  const eatClickAfterDrag = () => dirFired.current;

  useEffect(() => {
    const move = (e: MouseEvent) => {
      const p = dirPress.current;
      if (!p || e.buttons !== 1) return;
      if (Math.hypot(e.clientX - p.x, e.clientY - p.y) < 6) return;
      dirPress.current = null;
      dirFired.current = true;
      if (dirFiredTimer.current) window.clearTimeout(dirFiredTimer.current);
      dirFiredTimer.current = window.setTimeout(() => {
        dirFired.current = false;
      }, 400);
      onDragDir(p.path, e.clientX, e.clientY);
    };
    const up = () => {
      dirPress.current = null;
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    window.addEventListener("blur", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      window.removeEventListener("blur", up);
      if (dirFiredTimer.current) window.clearTimeout(dirFiredTimer.current);
    };
  }, [onDragDir]);

  /** 「+」：在当前库里新建文件夹。
   *
   * 建在哪：**当前选中的目录**（它属于当前库时）→ 否则**当前库根**。
   * 为什么不是一律建在库根：用户点选的是某个子目录时，把新文件夹建到库根
   * 等于"我明明站在这个文件夹里，你却跑到最上层建"。两种选择都限制在
   * **当前这个库**之内 —— 绝不会跑到别的库或库外去。 */
  const startCreate = () => {
    const root = activeRoot;
    if (!root) {
      onToast(t("tree.no_library"), true);
      return;
    }
    const sel = selected != null ? nodesRef.current.get(selected) : null;
    let parent = root;
    if (sel) {
      const r = rootOf(sel.id);
      if (r && r.path === root.path) parent = sel;
    }
    closedRef.current = false;
    setNewName(t("tree.new_folder"));
    setCreating({ id: parent.id, path: parent.path });
    /* 输入行画在父行下面：父目录没展开就先把展开/子目录准备好，否则输入框看不见 */
    void (async () => {
      if (!open.has(parent.id)) {
        await load(parent.id);
        setOpen((prev) => new Set(prev).add(parent.id));
        const paths = openSet();
        paths.add(parent.path);
        saveOpenPaths(paths);
      }
    })();
  };

  const cancelCreate = () => {
    closedRef.current = true;
    setCreating(null);
    setNewName("");
  };

  /** 回车/失焦提交：真的在磁盘上建（`am_mkdir` 走 core::fsops，同名会自动加 (2)） */
  const commitCreate = async () => {
    if (closedRef.current) return;
    closedRef.current = true;
    const target = creating;
    const name = newName.trim();
    setCreating(null);
    setNewName("");
    if (!target || !name) return;
    try {
      const made = await source.mkdir(target.path, name);
      /* 建完立刻把这个父目录的子列表拉一遍（不重扫全库：新建的空文件夹没有素材，
         索引里补上目录行就够，下一次增量扫描会自然对齐） */
      const ch = await source.children(target.id);
      remember(ch);
      loaded.current.add(target.id);
      setKids((m) => new Map(m).set(target.id, ch));
      const created = ch.find((c) => c.path === made) ?? ch.find((c) => c.name === name);
      if (created) onSelect(created);
      onToast(t("tree.folder_created"), false, made);
    } catch (e) {
      onToast(String(e), true);
    }
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
      /* 恢复树时已经拉过这些层的子列表 → 记下来，免得用户点开时再拉一遍 */
      for (const n of level) loaded.current.add(n.id);
      level = flat.filter((c) => want.has(c.path));
      for (const n of level) next.add(n.id);
    }
    setOpen(next);
  };

  useEffect(() => {
    /* 每次外部刷新（新建/删除文件夹、扫描完、移动完）都把"这层拉过了"的记号清掉，
       否则 `load()` 看到 kids 里有缓存就直接返回 —— 新建的文件夹永远不出现
       （owner 2026-11-19：「软件里新建的文件夹，软件里看不到」）。 */
    loaded.current.clear();
    void source.roots().then((r) => {
      // 库根可能变了（重建索引 / 加删库），把已经不存在的路径丢掉，
      // 否则 localStorage 里会越攒越多死路径
      setRoots(r);
      remember(r);
      void restore(r);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source, refreshKey]);

  const load = async (id: number, force = false) => {
    if (!force && loaded.current.has(id)) return;
    loaded.current.add(id);
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

  /** 切库 = 三步**排队**（owner 2026-11-19：以前"点击后的动画太乱"）。
   *
   *  ① 先把当下这个库收起来（子树 0fr，140ms）
   *  ② 再把点中的库挪到最上面（`layout="position"` 的 FLIP 位移动画）
   *  ③ 最后展开它（子树 1fr，140ms）
   *
   *  为什么必须串行：这三件事落在**同一个 `.lib-block`** 上 ——
   *  收起/展开是 `grid-template-rows` 过渡，挪位是 transform 的 FLIP，
   *  而且两行的内容同时要换（当前库行带箭头、普通行带「切换」）。
   *  三步一起发的话，高度、位移、内容三个动画互相拉扯，位移的起点还在变，
   *  看上去就是"一顿乱跳"。等前一步真做完（`sleep` 对齐 CSS 时长）再走下一步。
   */
  const switchToken = useRef(0);
  const switchLib = async (r: DirNode) => {
    const prev = activeRoot;
    /* 点的是已经开着的库：没有"收起 / 挪上去"可做，只保证它是展开的 */
    if (prev && samePath(prev.path, r.path)) {
      if (!open.has(r.id)) {
        await load(r.id);
        setOpen((c) => new Set(c).add(r.id));
      }
      onSelect(r);
      return;
    }

    /* 连点保护：每次切库领一个号，await 回来发现号变了就说明用户又点了别的库，
       本次链路就地放弃 —— 否则两条链会各收各的、各展各的。 */
    const my = ++switchToken.current;
    const alive = () => switchToken.current === my;

    /* ① 收起当下这个库（只关子树；`activePath` 不动，所以它还留在第一行） */
    if (prev && open.has(prev.id)) {
      setOpen((c) => {
        const n = new Set(c);
        n.delete(prev.id);
        return n;
      });
      const paths = openSet();
      paths.delete(prev.path);
      saveOpenPaths(paths);
      await sleep(KIDS_MS);
      if (!alive()) return;
    }

    /* ② 改 activePath：排序函数（`orderedRoots`）立刻把它排到第一，motion 做位移 */
    setActivePath(r.path);
    saveActiveLib(r.path);
    /* 列表内容这时候就换（用户点库的主目的），不必等位移动画跑完 */
    onSelect(r);
    await sleep(MOVE_MS);
    if (!alive()) return;

    /* ③ 展开目标库（`load` 内部按 `loaded` 去重，重复调用不会重复发请求） */
    await load(r.id);
    if (!alive()) return;
    setOpen((c) => new Set(c).add(r.id));
    const paths2 = openSet();
    paths2.add(r.path);
    saveOpenPaths(paths2);
  };

  /** 内置库小标：一眼看出"这行不是你加的" */
  const builtinChip = (bi: boolean, active: boolean) =>
    bi ? (
      <span
        className={`lib-builtin${active ? " on" : ""}`}
        title={t("tree.builtin_tip")}
      >
        {t("tree.builtin_chip")}
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
        onMouseDown={(e) => pressDir(r.path, e)}
        onClick={() => {
          if (eatClickAfterDrag()) return;
          void switchLib(r);
        }}
        title={`${r.path}\n${t("tree.switch_to_lib")}${
          bi ? `\n${t("tree.builtin_note")}` : ""
        }`}
      >
        <span className="nav-icon lib-off">
          <Icon name="folder" size={15} stroke={1.5} />
        </span>
        {/* 内置库显示成「截图」：目录名 screenshots 是英文，用户看不出这是软件自己的 */}
        <span className="tree-name lib-off">{bi ? t("tree.screenshots") : r.name}</span>
        {builtinChip(bi, false)}
        <span className="tree-count">{r.assetCnt > 0 ? r.assetCnt : 0}</span>
        <span className="lib-switch">{t("tree.switch")}</span>
      </div>
    );
  };

  /** 新建文件夹的行内输入：紧跟在父行下面，缩进 = 父 + 1 层 */
  const createRow = (parentId: number, depth: number) =>
    creating?.id === parentId ? (
      <div className="tree-input-row" style={{ paddingLeft: ROW_PAD + depth * DEPTH_STEP }}>
        <span className="nav-icon">
          <Icon name="folder" size={15} stroke={1.5} />
        </span>
        <BlinkCaretInput
          autoFocus
          value={newName}
          onChange={setNewName}
          placeholder={t("tree.new_folder_placeholder")}
          inputClassName="tree-input"
          ariaLabel={t("tree.new_folder_aria")}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void commitCreate();
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              cancelCreate();
            }
          }}
          onBlur={() => void commitCreate()}
        />
      </div>
    ) : null;

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
        onMouseDown={(e) => pressDir(n.path, e)}
        onClick={() => {
          if (eatClickAfterDrag()) return;
          selectNode(n);
        }}
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
        <span className="tree-name">{bi ? t("tree.screenshots") : n.name}</span>
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
        {createRow(n.id, depth + 1)}
        <div className={`tree-kids ${isOpen ? "open" : ""}`}>
          <div className="tree-kids-in">{list?.map((c) => row(c, depth + 1))}</div>
        </div>
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
        /* `layout="position"` 而不是 `layout`：切库时只做**位移**动画（需求就是"挪到最上面"），
           不参与尺寸动画 —— 展开/收起子树时高度变化不再触发 FLIP 测量与缩放，
           那正是"展开的时候一顿一顿"的来源（owner 2026-11-19）。 */
        layout="position"
        transition={LIB_SPRING}
        className="lib-block"
        style={{ position: "relative" }}
      >
        {isActive ? rowSelf(r, 0) : libRow(r)}
        {createRow(r.id, 1)}
        <div className={`tree-kids ${isActive && open.has(r.id) ? "open" : ""}`}>
          <div className="tree-kids-in">{kids.get(r.id)?.map((c) => row(c, 1))}</div>
        </div>
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
      <aside className={`tree ${railOpen ? "is-open" : ""}`}>
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
            <span className="grow">{t("tree.libraries")}</span>
            {/* 多库上限（最多 3 个）写在标题上：用户不用点开菜单就知道还能加几个。
                **内置截图库不计入** —— 否则三个自己的库只能挂两个。 */}
            <span
              className="lib-quota mono"
              title={t("tree.lib_quota_tip", { used: userLibCount, max: MAX_LIBS })}
            >
              {userLibCount}/{MAX_LIBS}
            </span>
            <button
              className="side-add"
              title={t("tree.new_folder_tip")}
              aria-label={t("tree.new_folder_short")}
              onClick={(e) => {
                e.stopPropagation();
                startCreate();
              }}
            >
              <Icon name="plus" size={12} stroke={1.8} />
            </button>
          </div>
          {sectionOpen && orderedRoots.map(libBlock)}
        </div>

        {/* 截图（内置）：**单独一段**，和「快照」并列，不是素材库里的一行
            （owner 2026-11-17 原话：「截图哪个单独一个，和快照一样」）。
            它不占那 3 个名额、也不能移除 —— 混在素材库里就会被当成"用户第 3 个库"，
            用户再加自己的库时莫名其妙被顶掉。 */}
        {shotRoot && (
          <div className="tree-section">
            <div
              className={`tree-heading-row shot-head ${selected === shotRoot.id ? "sel" : ""}`}
              data-dir-path={shotRoot.path}
              data-dir-name={shotRoot.name}
              data-dir-root="1"
              data-dir-builtin="1"
              title={`${shotRoot.path}\n${t("tree.shot_dir_tip")}`}
              onMouseDown={(e) => pressDir(shotRoot.path, e)}
              onClick={() => {
                if (eatClickAfterDrag()) return;
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
              {/* 前面的箭头**换成**图片图标（不是额外再加一个）：相机留给快照段，
                  这一段是软件截图落在磁盘上的目录。图标本身承担折叠开关。 */}
              <span
                className={`caret shot-caret ${shotOpen ? "" : "closed"}`}
                onClick={(e) => {
                  e.stopPropagation();
                  setShotOpen((v) => !v);
                }}
                title={shotOpen ? t("tree.collapse") : t("tree.expand")}
              >
                <Icon name="image" size={15} stroke={1.5} />
              </span>
              <span className="grow">{t("tree.screenshots")}</span>
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

      {collDock}
      {tagDock}

      {/* 底栏固定四行（照 FontManager：打开库 / 回收站 / 关于 / 快捷键） */}
      <div className="tree-foot">
        <div
          className="tree-row"
          onClick={onOpenLibrary}
          title={t("tree.reveal_lib_tip")}
        >
          <span className="nav-icon">
            <Icon name="reveal" size={15} stroke={1.5} />
          </span>
          <span className="tree-name">{t("tree.reveal_lib")}</span>
        </div>
        <div className="tree-row" onClick={onTrash} title={t("tree.trash_tip")}>
          <span className="nav-icon">
            <Icon name="trash" size={15} stroke={1.5} />
          </span>
          <span className="tree-name">{t("tree.trash")}</span>
        </div>
        <div className="tree-row" onClick={onAbout} title={t("tree.about")}>
          <span className="nav-icon">
            <Icon name="info" size={15} stroke={1.5} />
          </span>
          <span className="tree-name">{t("tree.about")}</span>
        </div>
        <div className="tree-row" onClick={onShortcuts} title={t("tree.shortcuts_tip")}>
          <span className="nav-icon">
            <Icon name="keyboard" size={15} stroke={1.5} />
          </span>
          <span className="tree-name">{t("tree.shortcuts")}</span>
          {/* 照 FontManager 的 `<kbd class="kbd nav-kbd">?</kbd>`：语义是键帽，不是按钮 */}
          <kbd className="side-kbd">?</kbd>
        </div>
      </div>
      </aside>
  );
}
