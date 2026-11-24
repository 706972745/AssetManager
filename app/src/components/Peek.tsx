import { useCallback, useEffect, useRef, useState, type PointerEvent as RPointerEvent } from "react";
import { onAppEvent, source } from "../data/source";
import { loadPrefs } from "../types";
import { ContextMenu, type CtxItem } from "./ContextMenu";
import { useT } from "../lib/i18n";

/**
 * 贴图窗口（UI 里叫「贴图」，也就是 owner 说的「贴图」——同一件事）。
 *
 * 形态照果核看图的「无边框模式」：**窗口就是那张图** —— 没有标题栏、没有边框、没有工具条，
 * 尺寸由图片比例决定（后端算，长边默认 342 DIP）。但它比果核多一层用法：
 * **它是贴图板**，想贴几张贴几张，关软件/重启电脑后没手动关的会自己回来（后端 `peeks.json`）。
 *
 * 手感约定（每一层都不许互相抢）：
 * - `滚轮`：连窗口一起缩放（锚定窗口中心）
 * - `Shift+滚轮`：**图内**缩放，锚定鼠标位置（看细节用，窗口不动）
 * - `Ctrl+滚轮`：整张贴图的不透明度
 * - 左键拖：`view.s === 1` 时移动**窗口**（老手感）；`view.s > 1` 时改成**图内平移** ——
 *   放大后用户要的显然是"看别处"，而不是"整扇窗跟着跑"（owner 2026-11-23：
 *   「长图放很大后拖拽这个长图有bug，拖不动」）。放大状态下想挪窗口先 Esc/0 复位。
 * - 中键拖：图内平移（放大后才有意义，和左键同一套 clamp）
 * - 双击：撕掉这张贴图
 *
 * **窗口里除了图什么都没有**（owner 2026-11-17：右上角 X、底部信息条全撤掉）——
 * 关闭走双击 / Esc / 右键菜单。同理窗口本身必须真透明，否则 Ctrl+滚轮降不透明度时
 * 露出来的是窗口自己的深色底（看起来就是"变黑"，而不是看到背后的东西）。
 */
interface PeekItem {
  src: string;
  w: number;
  h: number;
  path: string;
  name: string;
  gid: number;
}

interface PeekBoot {
  id?: string;
  seq?: number;
  items?: PeekItem[];
  i?: number;
  opacity?: number;
  theme?: "light" | "dark";
  /** 「阴影」的全局状态（后端持有，建窗时注入；**默认开**） */
  frame?: boolean;
  /** 置顶（后端存档里的值） */
  top?: boolean;
  /** 贴图耗时埋点开关（后端 `AM_PEEK_PERF=1` 时注入） */
  perf?: boolean;
}

/** 前端脚本开始执行的时间（模块求值那一刻），用于量「建窗 → 前端起来」这段 */
const T_EVAL = Date.now();

const ZOOM_STEP = 1.09; // 和果核看图同一个步长（实测每格 ×1.09）
const OPACITY_STEP = 0.08;
const MAX_SCALE = 12;

type PeekView = { s: number; x: number; y: number };
const ORIGIN_VIEW: PeekView = { s: 1, x: 0, y: 0 };

/** 窗口的内容盒（= `img` 的布局盒，**不受 transform 影响**）。 */
function peekBox(root: HTMLElement | null, img: HTMLImageElement | null) {
  const r = root?.getBoundingClientRect();
  if (r && r.width > 0 && r.height > 0) return { w: r.width, h: r.height };
  return { w: img?.clientWidth ?? 0, h: img?.clientHeight ?? 0 };
}

/**
 * 把图内缩放/位移钳在"图永远盖满窗口"的范围里。
 *
 * 为什么必须有它（owner 2026-11-23「长图放很大后拖不动」）：原来 `view.x/y` 是**无界**的，
 * Shift+滚轮按鼠标锚点缩放时 x/y 会被推到几千像素外，图整个滑出窗口 —— 窗口里只剩
 * 透明区，没有任何东西可以抓住，看上去就是"图丢了/拖不动"。
 *
 * 坐标系：`img` 是 `transform-origin: 0 0`，`transform: translate(x,y) scale(s) rotate(rot)`。
 * 缩放前的元素盒是 `winW × winH`（`width/height: 100%`），所以屏幕足迹就是
 * `(旋转时换过长短边的尺寸) × s`；绕左上角转 90/270/180 会把足迹甩到负轴一侧，
 * 这一段 `ox/oy` 就是在补这个偏移。镜像只翻符号、不改尺寸，所以不参与计算。
 *
 * 某一维放大后仍然装不满窗口（旋转 + 极端比例才会遇到）→ 该维**居中锁死**，
 * 宁可不动，也不给"拖出留白"的机会。
 */
function clampView(v: PeekView, rot: number, winW: number, winH: number): PeekView {
  const s = Math.min(MAX_SCALE, Math.max(1, v.s));
  /* s === 1 时**一个像素都不偏**：不挂 transform，和 0.5.x 之前完全一样
     （`object-fit: contain` 自己居中，我们别去插一脚）。 */
  if (s <= 1) return ORIGIN_VIEW;
  if (!winW || !winH) return v;

  const swap = rot === 90 || rot === 270;
  const fw = (swap ? winH : winW) * s; // 足迹宽
  const fh = (swap ? winW : winH) * s; // 足迹高
  const ox = rot === 90 ? -winH * s : rot === 180 ? -winW * s : 0;
  const oy = rot === 270 ? -winW * s : rot === 180 ? -winH * s : 0;

  /* 足迹在屏幕上的左边 = ox + t，要满足 winW - fw <= 左边 <= 0 */
  const axis = (t: number, origin: number, foot: number, win: number) => {
    if (foot <= win) return (win - foot) / 2 - origin;
    return Math.min(-origin, Math.max(win - foot - origin, t));
  };
  return {
    s,
    x: axis(v.x, ox, fw, winW),
    y: axis(v.y, oy, fh, winH),
  };
}

interface PeekBootData {
  id: string;
  items: PeekItem[];
  i: number;
  opacity: number;
  theme: "light" | "dark";
  frame: boolean;
  perf: boolean;
  /** 置顶（存档里的值；右键菜单的 ✓ 跟着它走） */
  top: boolean;
  /** 后端每次换图 +1，用来丢掉重复的 boot */
  seq: number;
}

type RawInvoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/** 直接拿 Tauri 的 IPC 桥：换图（`am_peek_boot`）与埋点不走 `source` 那层接口 ——
    它们只属于贴图窗，不值得把 `Source` 接口撑大。 */
function rawInvoke(): RawInvoke | null {
  const w = window as unknown as {
    __TAURI_INTERNALS__?: { invoke: RawInvoke };
    __TAURI__?: { core?: { invoke: RawInvoke } };
  };
  return w.__TAURI_INTERNALS__?.invoke ?? w.__TAURI__?.core?.invoke ?? null;
}

/** 后端推来的 boot（`peek:swap` / `am_peek_boot`）→ 组件内部形状（带默认值与校验） */
function normalizeBoot(raw: PeekBoot | null | undefined): PeekBootData | null {
  if (!raw || typeof raw !== "object") return null;
  const items = (raw.items ?? []).filter((x) => !!x?.src);
  if (!items.length) return null;
  const i = Math.max(0, Math.min(Number(raw.i ?? 0) || 0, items.length - 1));
  const op = typeof raw.opacity === "number" ? raw.opacity : 1;
  return {
    id: raw.id ?? "-",
    items,
    i,
    opacity: Math.min(1, Math.max(0.15, op)),
    theme: raw.theme === "light" ? "light" : "dark",
    frame: raw.frame !== false,
    perf: raw.perf === true,
    top: raw.top !== false,
    seq: Number(raw.seq ?? 0) || 0,
  };
}

function bootData(): PeekBootData {
  const b = (window as unknown as { __AM_PEEK__?: PeekBoot }).__AM_PEEK__ ?? {};
  const items = (b.items ?? []).filter((x) => !!x?.src);
  const i = Math.max(0, Math.min(Number(b.i ?? 0) || 0, Math.max(0, items.length - 1)));
  const op = typeof b.opacity === "number" ? b.opacity : 1;
  /* 主题以后端注入的为准（主窗推过去的），没有才退回本地偏好 ——
     贴图是独立 webview，"设置里的主题"在主窗那侧的 localStorage 里，不保证同分区。 */
  const pref = loadPrefs().theme;
  const theme: "light" | "dark" =
    b.theme === "light" || b.theme === "dark"
      ? b.theme
      : pref === "light"
        ? "light"
        : pref === "dark"
          ? "dark"
          : (window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true)
            ? "dark"
            : "light";
  return {
    id: b.id ?? "-",
    items,
    i,
    opacity: Math.min(1, Math.max(0.15, op)),
    theme,
    /* 阴影：后端是唯一事实来源；boot 没给（浏览器预览 / 老壳）就按默认**开** */
    frame: b.frame !== false,
    perf: b.perf === true,
    top: b.top !== false,
    seq: Number(b.seq ?? 0) || 0,
  };
}

/* ---------------- 复用窗的事件总线 ----------------

   为什么**不**在 React 的 effect 里 `listen`：热备窗是**隐藏**的，WebView2 会把它的
   渲染和副作用往后拖 —— 实测预热窗的 effect 直到被复用的那一刻才第一次跑。要是监听器
   也等 effect，`peek:swap` 早就发完了（事件不重放）。所以：
   · 监听器在**模块求值**时就装好（脚本一跑就装，比任何换图都早）；
   · 装好后补拉一次 `am_peek_boot`，覆盖"事件比监听器先到"那一瞬；
   · 组件挂载前收到的 boot 先存在 `pendingBoot`，挂载时补上。
   这两条路 + `seq` 去重，既不会漏，也不会把同一张图解两次。 */
let swapHandler: ((b: PeekBoot) => void) | null = null;
let idleHandler: (() => void) | null = null;
let pendingBoot: PeekBoot | null = null;
let pendingIdle = false;

/* **这扇窗自己的 label**。
   为什么必须过滤：Tauri 2 的事件是"广播"口径 —— 后端哪怕用 `emit_to(指定窗口)`，
   JS 侧用默认 `{kind:'Any'}` 注册的监听也一律收到（`Any` 永远匹配，见 tauri
   `manager/mod.rs` 的 `filter_target` / `match_any_or_filter`）。所以「关掉一张贴图 →
   其余贴图全被清空」「换一张图 → 所有贴图都变成那张」就是这么来的：
   `peek:idle` / `peek:swap` 被每一扇贴图窗都执行了一遍。
   口径：载荷里带 label（`peek:idle`）或 boot.id（`peek:swap`），对不上就丢掉。
   拿不到 label 时**不过滤**（宁可多执行一次，也不要因为拿不到而全哑）。 */
const MY_LABEL: string =
  (window as unknown as { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } } })
    .__TAURI_INTERNALS__?.metadata?.currentWindow?.label ||
  (window as unknown as { __AM_PEEK__?: { id?: string } }).__AM_PEEK__?.id ||
  "";

/** 这条事件是不是发给我的 */
function forMe(label: unknown): boolean {
  if (!MY_LABEL) return true;
  if (typeof label !== "string" || !label) return true;
  return label === MY_LABEL;
}

function acceptBoot(raw: PeekBoot | null | undefined) {
  if (!raw) return;
  if (!forMe(raw.id)) return; // 别人的换图事件，别跟着换
  if (swapHandler) swapHandler(raw);
  else pendingBoot = raw;
}

function installPeekBus(): void {
  if (!(window as unknown as { __AM_PEEK__?: unknown }).__AM_PEEK__) return;
  void import("@tauri-apps/api/event")
    .then(({ listen }) =>
      Promise.all([
        listen<PeekBoot>("peek:swap", (e) => acceptBoot(e.payload)),
        listen<{ label?: string } | null>("peek:idle", (e) => {
          if (!forMe(e.payload?.label)) return; // 别人被收进热备，跟我无关
          if (idleHandler) idleHandler();
          else pendingIdle = true;
        }),
      ]).then(() => {
        /* 注册好之后再补拉：这段时间里后端可能已经换过图（事件落空） */
        const inv = rawInvoke();
        if (!inv) return;
        void inv("am_peek_boot")
          .then((b) => acceptBoot(b as PeekBoot))
          .catch(() => {});
      })
    )
    .catch(() => {});
}

installPeekBus();

/** 16px 线性图标（只用一个，不值得为它引依赖） */
export function Peek() {
  const t = useT();
  /* boot 是**可替换的**：复用窗（用户撕掉的那扇 / 启动预热的空白窗）不重新加载页面，
     后端把新图推过来（`peek:swap`）后这里整体换一份，所有视图状态跟着复位。 */
  const [boot, setBoot] = useState<PeekBootData>(bootData);
  /* 贴图耗时埋点：只把阶段名 + 客户端时间戳交给后端（它统一写文件、统一时钟）。
     平时（`AM_PEEK_PERF` 没开）这里全是空转，一个 IPC 都不发。 */
  const perfMark = useCallback(
    (stage: string, at = Date.now()) => {
      if (!boot.perf) return;
      const inv = rawInvoke();
      if (!inv) return;
      void inv("am_peek_perf", { label: boot.id, stage, at }).catch(() => {});
    },
    [boot.perf, boot.id]
  );
  const [idx, setIdx] = useState(boot.i);
  const [pinned, setPinned] = useState(boot.top); // 默认置顶（owner 要求）
  const [err] = useState("");
  const [opac, setOpac] = useState(boot.opacity);
  /* 图内缩放（Shift+滚轮）：scale + 位移，`transform-origin: 0 0` 便于算锚点。
     `x/y` 一律经过 `clampView`，永远落在"图盖满窗口"的范围里。 */
  const [view, setView] = useState<PeekView>(ORIGIN_VIEW);
  /* 正在拖图（只用来切 cursor 和关掉 transform 过渡） */
  const [panning, setPanning] = useState(false);
  /* 旋转 / 镜像（0.1.18，P2.7）：只影响"看"，不动文件、不改存档 ——
     贴图是临时视图，关掉再开回来还是原样。 */
  const [rot, setRot] = useState(0);
  const [flip, setFlip] = useState({ h: false, v: false });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  /* 贴图窗没有主窗那套 toast，用一条自己会消失的提示（存库/存库失败都要有反馈） */
  const [flashMsg, setFlashMsg] = useState("");
  const flash = useCallback((m: string) => {
    setFlashMsg(m);
    window.setTimeout(() => setFlashMsg(""), 2200);
  }, []);
  /* 阴影：**全局开关**，状态在后端（`PEEK_FRAME`），不是每个窗口各记一份。
     为什么改（owner 2026-11-18 连报两次"关不掉"）：贴图是多窗口，前端各存 localStorage 时，
     关掉手上这张不影响已经贴着的其他张，新贴的还按默认值开 —— 看上去就是开关没生效。
     默认**开**（owner 2026-11-19：白底网页/文档上贴图需要一圈轮廓才看得出边界），
     右键菜单里随时可关；切换时调命令，后端广播给所有贴图窗口。 */
  const [frame, setFrame] = useState(() => boot.frame !== false);
  const toggleFrame = useCallback(() => {
    setFrame((v) => {
      const next = !v;
      void source.peekSetFrame(next);
      flash(next ? t("peek.frameOn") : t("peek.frameOff"));
      return next;
    });
    /* 兜底：所有窗口（含本窗）都会收到 `peek:frame`，正常路径由事件驱动更新；
       这里先本地改一次是为了手感跟手，事件回来时值相同，不会闪。 */
  }, [flash, t]);

  /* 其他贴图窗口切了这个开关 → 本窗跟着变（全局偏好的唯一同步方式） */
  useEffect(
    () =>
      onAppEvent("peek:frame", (v) => {
        setFrame(v === true);
      }),
    []
  );

  const root = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const busy = useRef(false);
  /* 拖图会话：`id` 是 pointerId，抓不到指针对应关系就不知道该不该跟手 */
  const pan = useRef<{ id: number; x: number; y: number; ox: number; oy: number } | null>(null);
  /* `rot` 的镜像 ref：滚轮 / resize 那些只挂一次的监听里要读旋转（不能进依赖，
     否则每转一次就要重挂一遍监听）。 */
  const rotRef = useRef(rot);
  /* 窗口建出来是**不可见**的（后端 `.visible(false)`），首帧真画上去了才让它露面 ——
     否则 WebView2 会先闪一下自己的默认底色，中键贴图时那一闪特别难看。 */
  const shown = useRef(false);
  /* 比例只校正一次：换图（←/→）会重新触发 onLoad，那时按新图再校一次是对的，
     但同一张图反复 load（缓存/重排）不能每次都改窗口，否则会和滚轮缩放抢 */
  const ratioFixed = useRef(false);
  /* 有没有图可显示（复用窗在闲置状态是空的，这时候**不许**把窗口 show 出来） */
  const hasItem = useRef(false);
  hasItem.current = boot.items.length > 0;
  /* "图已经在缓存里、onLoad 不会再触发"时的兜底定时器（换图后重新上弦） */
  const readyTimer = useRef(0);
  const markReady = useCallback(() => {
    if (shown.current || !hasItem.current) return;
    shown.current = true;
    // 双 rAF：等这一帧真的合成完，再让窗口显示
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        perfMark("fe_ready");
        void source.peekReady().catch(() => {});
      })
    );
  }, [perfMark]);
  const armReady = useCallback(
    (ms = 450) => {
      window.clearTimeout(readyTimer.current);
      readyTimer.current = window.setTimeout(() => markReady(), ms);
    },
    [markReady]
  );

  /**
   * 换一份 boot（复用窗收到后端推来的新图时走这里）。
   *
   * **所有视图状态都要复位**：新图的索引、旋转、镜像、图内缩放、不透明度、置顶、边框……
   * 漏一个就会出现"新贴的图还带着上一张的缩放/旋转"。`shown` 也要复位 ——
   * 下一次 `onLoad` 才轮到它重新 `am_peek_ready` 把窗露出来。
   */
  /* 收到过的最大 `seq`：`peek:swap` 和 mount 补拉可能各送一份同一张图 */
  const appliedSeq = useRef(boot.seq);
  const applyBoot = useCallback(
    (raw: PeekBoot | null | undefined) => {
      const b = normalizeBoot(raw);
      if (!b) return;
      if (b.seq && b.seq <= appliedSeq.current) return;
      appliedSeq.current = b.seq;
      shown.current = false;
      ratioFixed.current = false;
      pan.current = null;
      setPanning(false);
      setMenu(null);
      setView(ORIGIN_VIEW);
      setRot(0);
      setFlip({ h: false, v: false });
      setIdx(b.i);
      setOpac(b.opacity);
      setFrame(b.frame);
      setPinned(b.top);
      setBoot(b);
      /* 同一张图再贴一次时浏览器不会重发 load 事件 —— 靠这个兜底把窗露出来 */
      armReady();
    },
    [armReady]
  );

  /** 收进热备：把图放掉（`<img>` 卸载才会释放解码后的位图），等下一次换图 */
  const goIdle = useCallback(() => {
    shown.current = false;
    ratioFixed.current = false;
    window.clearTimeout(readyTimer.current);
    setView(ORIGIN_VIEW);
    setRot(0);
    setFlip({ h: false, v: false });
    setMenu(null);
    setBoot((b) => ({ ...b, items: [], i: 0 }));
  }, []);

  const cur = boot.items[idx];

  /* 外阴影色：贴图是独立 webview，从后端读当前值 + 订阅主窗改色。 */
  useEffect(() => source.initShadowColor(), [source]);

  /* 图可能已经在缓存里（onLoad 不会再触发），所以再挂一道兜底 */
  useEffect(() => {
    armReady(400);
    return () => window.clearTimeout(readyTimer.current);
  }, [armReady]);

  /**
   * 把总线接到组件状态上（监听器本身在模块求值时就装好了，见 `installPeekBus`）。
   * 组件挂载前就到的 boot 存在 `pendingBoot`，这里补上。
   */
  useEffect(() => {
    swapHandler = (b) => applyBoot(b);
    idleHandler = () => goIdle();
    if (pendingBoot) {
      const b = pendingBoot;
      pendingBoot = null;
      applyBoot(b);
    }
    if (pendingIdle) {
      pendingIdle = false;
      goIdle();
    }
    return () => {
      swapHandler = null;
      idleHandler = null;
    };
  }, [applyBoot, goIdle]);

  /* 埋点：脚本求值 / React 首次提交。中间那段是 bundle 解析 + 首帧渲染。 */
  useEffect(() => {
    perfMark("fe_eval", T_EVAL);
    perfMark("fe_mount");
  }, [perfMark]);

  /* 主题：贴图是独立 webview，不会自动继承主窗的 data-theme。
     以前这里没人设，于是浅色主题下贴图的右键菜单仍是深色（owner 2026-11-18 报的第 6 条）。
     开窗时读一次 prefs 就够 —— 用户在设置里换主题后新开的贴图会跟上。 */
  useEffect(() => {
    document.documentElement.dataset.theme = boot.theme === "light" ? "light" : "dark";
  }, [boot.theme]);

  useEffect(() => {
    document.body.classList.add("peek-body");
    document.documentElement.classList.add("peek-body");
    return () => {
      document.body.classList.remove("peek-body");
      document.documentElement.classList.remove("peek-body");
    };
  }, []);

  /* 关窗：**从存档里删掉**（用户主动关的，下次开机不该回来） */
  const close = useCallback(() => {
    void source.peekClose();
  }, []);

  /**
   * 转 90° / 270° 时窗口的长宽要跟着换（不然图会转出窗口、留两条黑边）。
   * 换比例走 `peekRatio`：长边不变、只按新比例重算短边。
   */
  const applyRot = useCallback(
    (next: number) => {
      setRot(next);
      const it = boot.items[idx];
      if (!it?.w || !it?.h) return;
      const swap = next === 90 || next === 270;
      void source.peekRatio(swap ? it.h : it.w, swap ? it.w : it.h).catch(() => {});
    },
    [boot.items, idx]
  );

  /* 换图：窗口按新图比例变，并把存档指向新文件（重启还原的是最后看到的那张） */
  const goto = useCallback(
    (next: number) => {
      const n = boot.items.length;
      if (!n) return;
      const k = (next + n) % n;
      const it = boot.items[k];
      setIdx(k);
      setView({ s: 1, x: 0, y: 0 });
      setRot(0);
      setFlip({ h: false, v: false });
      /* 换图后允许 onLoad 再按真实像素校一次比例（`it.w/h` 是索引里的值，可能是缩略图尺寸） */
      ratioFixed.current = false;
      if (it.w && it.h) void source.peekRatio(it.w, it.h).catch(() => {});
      void source.peekTrack(it.path, it.gid).catch(() => {});
    },
    [boot.items]
  );

  /* 按**当前**窗口尺寸重新钳一次位移。
     用在：窗口缩放（滚轮）/ 换图 / 旋转 之后 —— 盒子变了，旧的 x/y 可能已经越界，
     不重算就会留下"图被顶出窗口"的残留状态。 */
  const reclamp = useCallback(() => {
    const b = peekBox(root.current, imgRef.current);
    setView((v) => (v.s > 1 ? clampView(v, rotRef.current, b.w, b.h) : v));
  }, []);

  /* 滚轮三分支：Shift=图内、Ctrl=不透明度、其余=连窗口一起缩放 */
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const up = e.deltaY < 0;

      if (e.shiftKey) {
        /* 锚定鼠标：让指针底下那个像素在缩放前后**不动**。
           算式直接在**屏幕坐标**里写 —— `translate(x,y) scale(s)`、origin `0 0` 下，
           屏幕点 u 对应内容点 (u - x) / s；缩放后要满足 x2 + 内容点 * s2 = u，
           解出 `x2 = u - (u - x) * k`（k = s2 / s）。
           以前用的是"img 的 rect 再减 x"的写法，x = 0 时等价，一旦平移过就会差
           `x * (1 - k)`，越缩越偏；现在平移可用了，这个偏差必须修掉。 */
        const b = peekBox(el, imgRef.current);
        const ux = e.clientX;
        const uy = e.clientY;
        setView((v) => {
          const s2 = Math.min(MAX_SCALE, Math.max(1, v.s * (up ? ZOOM_STEP : 1 / ZOOM_STEP)));
          if (s2 === 1) return ORIGIN_VIEW;
          const k = s2 / v.s;
          return clampView(
            { s: s2, x: ux - (ux - v.x) * k, y: uy - (uy - v.y) * k },
            rotRef.current,
            b.w,
            b.h
          );
        });
        return;
      }

      if (e.ctrlKey) {
        setOpac((o) => {
          const v = Math.min(1, Math.max(0.15, o + (up ? OPACITY_STEP : -OPACITY_STEP)));
          void source.peekOpacity(v).catch(() => {});
          return v;
        });
        return;
      }

      if (busy.current) return;
      busy.current = true;
      void source
        .peekZoom(up ? ZOOM_STEP : 1 / ZOOM_STEP)
        .catch(() => {})
        .finally(() => {
          busy.current = false;
          /* 窗口刚改完尺寸，`resize` 事件不一定这一帧就到 —— 下一帧主动再钳一次，
             保证"缩完窗口图仍然盖满窗口" */
          requestAnimationFrame(() => reclamp());
        });
    };
    // 原生 passive:false（React 的 onWheel 在 WebView2 上可能是 passive，preventDefault 会失效）
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [reclamp]);

  /* 键盘 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      if ((e.ctrlKey || e.metaKey) && k === "c") {
        e.preventDefault();
        if (cur) void source.peekCopyFile([cur.path]).catch(() => {});
        return;
      }
      if ((e.ctrlKey || e.metaKey) && k === "s") {
        e.preventDefault();
        if (cur) void source.saveAs(cur.path).catch(() => {});
        return;
      }
      if (e.key === "Escape") {
        if (menu) {
          setMenu(null);
          return;
        }
        if (view.s > 1) {
          setView({ s: 1, x: 0, y: 0 });
          return;
        }
        close();
        return;
      }
      if (e.key === "ArrowLeft") goto(idx - 1);
      if (e.key === "ArrowRight") goto(idx + 1);
      if (!e.ctrlKey && !e.metaKey) {
        if (k === "r") {
          e.preventDefault();
          applyRot((rot + 90) % 360);
          return;
        }
        if (k === "h") {
          e.preventDefault();
          setFlip((f) => ({ ...f, h: !f.h }));
          return;
        }
        if (k === "v") {
          e.preventDefault();
          setFlip((f) => ({ ...f, v: !f.v }));
          return;
        }
        if (k === "0") {
          e.preventDefault();
          setRot(0);
          setFlip({ h: false, v: false });
          setView({ s: 1, x: 0, y: 0 });
          return;
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close, cur, goto, idx, menu, view.s, rot, applyRot]);

  /* 窗口尺寸变了（滚轮缩放窗口 / 换图 / 旋转换比例）→ 位移必须**重新 clamp**。
     残留的 x/y 会把图顶到窗口外，用户看到的就是"图没了、还拖不回来"。 */
  useEffect(() => {
    window.addEventListener("resize", reclamp);
    return () => window.removeEventListener("resize", reclamp);
  }, [reclamp]);

  /* 旋转 90/270 会把足迹的长短边换掉 → 旋转后也要按新足迹再算一次
     （窗口比例是异步改的，那次 resize 还会再算一遍，重复算无害、结果一致） */
  useEffect(() => {
    rotRef.current = rot;
    const b = peekBox(root.current, imgRef.current);
    setView((v) => (v.s > 1 ? clampView(v, rot, b.w, b.h) : v));
  }, [rot]);

  /* 右键菜单 */
  const menuItems: CtxItem[] = [
    {
      label: t("peek.copyFile"),
      icon: "copy",
      shortcut: "Ctrl+C",
      action: () => cur && void source.peekCopyFile([cur.path]).catch(() => {}),
    },
    {
      label: t("peek.copyDisplay"),
      icon: "copy",
      action: () => {
        // 屏幕上真实占用的像素（不是图片原始尺寸）—— 排版截图时要用的是这个数
        const w = Math.round(window.innerWidth * window.devicePixelRatio);
        const h = Math.round(window.innerHeight * window.devicePixelRatio);
        void source.clipText(`${w}×${h}`).catch(() => {});
      },
    },
    {
      label: t("peek.saveLib"),
      icon: "save",
      action: () => {
        if (!cur) return;
        void source
          .peekSaveLib(cur.path)
          .then(() => flash(t("peek.savedLib")))
          .catch((e: unknown) => flash(String((e as Error)?.message ?? e)));
      },
    },
    {
      label: t("peek.saveAs"),
      icon: "reveal",
      shortcut: "Ctrl+S",
      action: () => cur && void source.saveAs(cur.path).catch(() => {}),
    },
    { kind: "separator" },
    {
      label: t("peek.snip"),
      icon: "image",
      shortcut: "F1",
      action: () => void source.snipStart().catch(() => {}),
    },
    { kind: "separator" },
    {
      label: `${pinned ? "✓ " : ""}${t("peek.top")}`,
      icon: "star",
      action: () => void source.peekTop(!pinned).then(setPinned).catch(() => {}),
    },
    {
      label: `${frame ? "✓ " : ""}${t("peek.frame")}`,
      icon: "image",
      action: toggleFrame,
    },
    { kind: "separator" },
    {
      label: rot ? t("peek.rotateMore", { deg: rot }) : t("peek.rotate"),
      icon: "play",
      shortcut: "R",
      action: () => applyRot((rot + 90) % 360),
    },
    {
      label: `${flip.h ? "✓ " : ""}${t("peek.flipH")}`,
      icon: "copy",
      shortcut: "H",
      action: () => setFlip((f) => ({ ...f, h: !f.h })),
    },
    {
      label: `${flip.v ? "✓ " : ""}${t("peek.flipV")}`,
      icon: "copy",
      shortcut: "V",
      action: () => setFlip((f) => ({ ...f, v: !f.v })),
    },
    {
      label: t("peek.resetView"),
      icon: "search",
      shortcut: "0",
      disabled: rot === 0 && !flip.h && !flip.v && view.s <= 1,
      action: () => {
        setRot(0);
        setFlip({ h: false, v: false });
        setView({ s: 1, x: 0, y: 0 });
      },
    },
    { kind: "separator" },
    { label: t("peek.close"), icon: "close", shortcut: "Esc", danger: true, action: close },
  ];

  const px = cur?.src ?? "";

  /* 图内平移（`view.s > 1`）。左键 + 中键都走这一套 ——
     左键为什么得抢回来：`img` 上有 `data-tauri-drag-region`，Tauri 是**在 document 上**
     听 `mousedown`（tauri `window/scripts/drag.js`），按下去就把整扇窗拖走了。图跟着窗一起动，
     相对窗口没位移，用户看到的就是"长图放很大后拖不动"。所以放大时把属性换成 `"false"`：
     Tauri 认这个值，并且会连祖先一起否决，左键这才轮得到我们。
     另外用 pointer capture 而不是 window 上的 mousemove：贴图窗只有 342px 宽，
     指针一滑出窗口就再也收不到 move，拖到一半会断（这也是"拖不动"的一部分）。 */
  const endPan = (e: RPointerEvent<HTMLImageElement>) => {
    if (pan.current?.id !== e.pointerId) return;
    pan.current = null;
    setPanning(false);
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  };

  const onPanDown = (e: RPointerEvent<HTMLImageElement>) => {
    /* s === 1：一个字节都不拦，原样交给 drag region 拖窗口（老手感不能变） */
    if (view.s <= 1) return;
    if (e.button !== 0 && e.button !== 1) return; // 右键留给菜单
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    pan.current = { id: e.pointerId, x: e.clientX, y: e.clientY, ox: view.x, oy: view.y };
    setPanning(true);
  };

  const onPanMove = (e: RPointerEvent<HTMLImageElement>) => {
    const p = pan.current;
    if (!p || p.id !== e.pointerId) return;
    const nx = p.ox + (e.clientX - p.x);
    const ny = p.oy + (e.clientY - p.y);
    const b = peekBox(root.current, imgRef.current);
    setView((v) => clampView({ s: v.s, x: nx, y: ny }, rotRef.current, b.w, b.h));
  };

  return (
    <div
      ref={root}
      className={frame ? "peek-root peek-framed" : "peek-root"}
      data-tauri-drag-region
      onDoubleClick={close}
      onContextMenu={(e) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
      }}
    >
      {cur && !err && (
        <img
          ref={imgRef}
          className={panning ? "peek-img peek-img-drag" : "peek-img"}
          src={px}
          alt=""
          draggable={false}
          /* 放大后要自己接管左键 → 显式 `false`（Tauri 认这个值，且否决祖先的 drag region）；
             没放大时保持老行为：按在图上就是拖窗口。 */
          data-tauri-drag-region={view.s > 1 ? "false" : ""}
          onPointerDown={onPanDown}
          onPointerMove={onPanMove}
          onPointerUp={endPan}
          onPointerCancel={endPan}
          onLostPointerCapture={() => {
            pan.current = null;
            setPanning(false);
          }}
          onLoad={(e) => {
            perfMark("fe_load");
            /* **比例校正**（owner 2026-11-19：「长图转贴图被压扁了」）。
               窗口尺寸是后端按长边算的，只要实际比例和窗口比例差一点（DPI 取整、
               老存档里的 longEdge、手拖过窗口边），`object-fit` 就会把图拉变形。
               这里量到真实像素再让后端按比例重设一次窗口 —— 只做**一次**，
               否则会和用户的滚轮缩放打架。 */
            const img = e.currentTarget;
            const box = root.current?.getBoundingClientRect();
            if (!ratioFixed.current && box && img.naturalWidth && img.naturalHeight) {
              const want = img.naturalWidth / img.naturalHeight;
              /* `box` 就是**图的框**：`.peek-root` 用 `inset: var(--peek-glow)` 已经缩到图上了，
                 不用再减边距（0.6.x 起 glow=3px，减了反而判错）。 */
              const got = box.width / Math.max(1, box.height);
              if (Math.abs(want - got) / want > 0.02) {
                ratioFixed.current = true;
                void source.peekRatio(img.naturalWidth, img.naturalHeight).catch(() => {});
              }
            }
            markReady();
          }}
          onError={markReady}
          style={{
            opacity: opac,
            /* 顺序：镜像 → 旋转 → 图内缩放位移。
               先镜像再旋转，「左右镜像」说的就是**屏幕上**的左右（符合直觉）。 */
            transform: (() => {
              const parts: string[] = [];
              if (view.s > 1) parts.push(`translate(${view.x}px, ${view.y}px) scale(${view.s})`);
              if (rot) parts.push(`rotate(${rot}deg)`);
              if (flip.h) parts.push("scaleX(-1)");
              if (flip.v) parts.push("scaleY(-1)");
              return parts.length ? parts.join(" ") : undefined;
            })(),
            transformOrigin: "0 0",
            cursor: view.s > 1 ? (panning ? "grabbing" : "grab") : undefined,
          }}
        />
      )}

      {err && <div className="peek-err">{err}</div>}

      {flashMsg && <div className="peek-flash">{flashMsg}</div>}

      {menu && (
        <ContextMenu
          state={{ x: menu.x, y: menu.y, items: menuItems }}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}
