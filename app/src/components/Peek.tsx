import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as RPointerEvent } from "react";
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
  items?: PeekItem[];
  i?: number;
  opacity?: number;
  theme?: "light" | "dark";
  /** 「蓝色边框」的全局状态（后端持有，建窗时注入；**默认开**） */
  frame?: boolean;
}

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

function bootData(): {
  items: PeekItem[];
  i: number;
  opacity: number;
  theme: "light" | "dark";
  frame: boolean;
} {
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
    items,
    i,
    opacity: Math.min(1, Math.max(0.15, op)),
    theme,
    /* 蓝色边框：后端是唯一事实来源；boot 没给（浏览器预览 / 老壳）就按默认**开** */
    frame: b.frame !== false,
  };
}

/** 16px 线性图标（只用一个，不值得为它引依赖） */
export function Peek() {
  const t = useT();
  const boot = useMemo(bootData, []);
  const [idx, setIdx] = useState(boot.i);
  const [pinned, setPinned] = useState(true); // 默认置顶（owner 要求）
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
  /* 蓝色边框：**全局开关**，状态在后端（`PEEK_FRAME`），不是每个窗口各记一份。
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
  const markReady = useCallback(() => {
    if (shown.current) return;
    shown.current = true;
    // 双 rAF：等这一帧真的合成完，再让窗口显示
    requestAnimationFrame(() =>
      requestAnimationFrame(() => void source.peekReady().catch(() => {}))
    );
  }, []);

  const cur = boot.items[idx];

  /* 图可能已经在缓存里（onLoad 不会再触发），所以再挂一道兜底 */
  useEffect(() => {
    const t = setTimeout(markReady, 400);
    return () => clearTimeout(t);
  }, [markReady]);

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
            /* **比例校正**（owner 2026-11-19：「长图转贴图被压扁了」）。
               窗口尺寸是后端按长边算的，只要实际比例和窗口比例差一点（DPI 取整、
               老存档里的 longEdge、手拖过窗口边），`object-fit` 就会把图拉变形。
               这里量到真实像素再让后端按比例重设一次窗口 —— 只做**一次**，
               否则会和用户的滚轮缩放打架。 */
            const img = e.currentTarget;
            const box = root.current?.getBoundingClientRect();
            if (!ratioFixed.current && box && img.naturalWidth && img.naturalHeight) {
              const want = img.naturalWidth / img.naturalHeight;
              /* 拿**内容盒**比。0.4.24 起 `--peek-glow` 归零（窗就是图），
                 但这里仍然按变量算，改回去时不用再动这段。 */
              const glow = 0;
              const got = (box.width - glow) / Math.max(1, box.height - glow);
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
