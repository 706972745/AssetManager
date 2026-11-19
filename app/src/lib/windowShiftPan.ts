/**
 * Shift + 滚轮 = 把**窗口本身**在屏幕上挪个位置（灵感板，owner 2026-11-19）。
 *
 * 「按住 Shift 不松、滚轮当方向键用，把窗口推到屏幕任意位置」。
 * 注意它改的是**窗口的屏幕坐标**，不是画布缩放，也不是画布平移 ——
 * 画布内容相对窗口纹丝不动（这正是它和「滚轮缩放」「拖空白处平移」的分工）。
 *
 * 为什么单独一个模块（而不是塞在 Board.tsx 里）：
 * 这条链路和缩放完全不是一回事 —— 缩放只改一个 CSS 变量，而移动窗口要走 IPC
 * 交给窗口管理器，代价高得多。所以这里必须：
 *
 * 1. **合帧**：滚轮一秒能发几十个事件，每个都发一次 IPC 会把它打满。这里只把
 *    位移累加进 `acc`，用一帧一个 `requestAnimationFrame` 落地（rAF 里还没回来
 *    就不叠第二次，等它回来再补）。
 * 2. **缓存几何**：`outerPosition()` / `outerSize()` / 工作区都是 IPC 往返，
 *    绝不能每次滚轮都读。位置以「我们自己写下的值」为准（写进去就等于真实位置），
 *    只在**新手势开始时**重读一次 —— 中间用户可能用别的方式（拖标题栏）挪过窗口。
 * 3. **夹进工作区**：窗口不能被推到任务栏底下或者屏幕外再也抓不回来。
 *    工作区（去掉任务栏）是物理像素，窗口外框也是物理像素，同一套坐标直接比。
 *
 * 几条硬边界：
 *
 * - 只有调用方判断了 `Shift` 才会调到这里；本模块**不**自己判断修饰键，
 *   免得"该不该接管"散落在两处（Board 的滚轮分支里一处说清楚）。
 * - 没有 Tauri 运行时（浏览器预览）或缺少 `core:window:allow-set-position`
 *   权限时不抛异常：报一次、记 `dead`、之后静默 —— 最坏结果是这条手感没有，
 *   而不是每个滚轮都打一次失败日志。
 * - 已经贴到工作区边缘时不再发 IPC（滚轮继续滚只是空转），省掉无意义的往返。
 */

import { normalizeWheelDelta } from "./momentumScroll";

/** 一格滚轮（Chromium 上通常 100px）折算成的屏幕位移比例 —— 一格约 60px，像方向键推一格 */
const WHEEL_RATIO = 0.6;
/**
 * 单个滚轮事件最多认多少**输入** delta（CSS px）。
 *
 * 注意它封的是"滚轮给了多少"，不是"窗口走多远"：真正写进 `setPosition` 的位移是
 * `min(|delta|, MAX_STEP_CSS) × WHEEL_RATIO`，即单次最多 200×0.6 = **120 CSS px**。
 * 为什么要封：高精度滚轮/触控板偶尔一次给几千 px，不封就会一下把窗口甩出屏幕。
 */
const MAX_STEP_CSS = 200;
/** 两次滚轮间隔超过这么久（ms）就当作「新手势」：重读一次窗口真实几何 */
const GESTURE_GAP = 260;

/** 点（物理像素） */
export interface WheelPoint {
  x: number;
  y: number;
}
/** 尺寸（物理像素） */
export interface WheelSize {
  width: number;
  height: number;
}
/** 矩形（物理像素），工作区 / 显示器都用它 */
export interface WheelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 窗口几何快照：位置 + 外框尺寸 + 所在显示器工作区 */
interface Geometry {
  /** 窗口左上角（物理像素）—— 也是我们下一次增量计算的基准 */
  pos: WheelPoint;
  /** 窗口外框尺寸（物理像素） */
  size: WheelSize;
  /** 当前显示器工作区；取不到时为 null（那就只移不夹） */
  area: WheelRect | null;
}

type WindowModule = typeof import("@tauri-apps/api/window");
type DpiModule = typeof import("@tauri-apps/api/dpi");
type WindowHandle = ReturnType<WindowModule["getCurrentWindow"]>;

export interface ShiftWheelPanOptions {
  /**
   * 移动窗口不可用时调**一次**（没有 Tauri 运行时 / 缺窗口权限），
   * 让调用方给用户一句实话，而不是"滚了没反应"。
   */
  onUnavailable?: (reason: unknown) => void;
}

export interface ShiftWheelPan {
  /** 把一个 wheel 事件喂进来（调用方应已 `preventDefault`） */
  handle(e: WheelEvent): void;
  /**
   * 丢掉还没落地的那一帧。
   *
   * 故意**不是** `dispose`：控制器只被一个组件持有、也不挂任何全局监听，真正要做的
   * 只有"取消挂起的那一帧 rAF"。React 18 的 StrictMode 会「挂载→清理→再挂载」，
   * 若在这里把自己标成永久失效，第二次挂载后 Shift+滚轮就彻底不动了 —— 所以 cancel
   * 之后仍然可以继续 `handle`。真正的不可用（没有运行时 / 缺权限）走 `dead`，那才是永久的。
   */
  cancel(): void;
}

/**
 * 把窗口左上角夹进工作区：整扇窗都要在里面；窗口比工作区还大时贴住左上角。
 *
 * 纯函数，和 IPC 无关 —— 边界行为可以单独核对（都是物理像素，不用换单位）。
 */
export function clampWindowPosition(
  desired: WheelPoint,
  size: WheelSize,
  area: WheelRect | null,
): WheelPoint {
  if (!area) return { x: desired.x, y: desired.y };
  const maxX = area.x + Math.max(0, area.width - size.width);
  const maxY = area.y + Math.max(0, area.height - size.height);
  return {
    x: Math.min(Math.max(desired.x, area.x), Math.max(area.x, maxX)),
    y: Math.min(Math.max(desired.y, area.y), Math.max(area.y, maxY)),
  };
}

/** 一格滚轮折算成位移（CSS px），并夹住异常大的单次增量 */
function wheelStep(raw: number): number {
  if (!raw) return 0;
  const capped = raw > MAX_STEP_CSS ? MAX_STEP_CSS : raw < -MAX_STEP_CSS ? -MAX_STEP_CSS : raw;
  return capped * WHEEL_RATIO;
}

/**
 * 造一个 Shift+滚轮挪窗口的控制器。一个窗口一个（Board 里 `useMemo` 只建一次）。
 */
export function createShiftWheelPan(opts: ShiftWheelPanOptions = {}): ShiftWheelPan {
  /** 几何缓存：`null` = 这一轮手势还没读过（下次 flush 前重读） */
  let geom: Geometry | null = null;
  /** 懒加载的 Tauri 模块（浏览器预览时永远为 null） */
  let winModule: WindowModule | null = null;
  let dpiModule: DpiModule | null = null;
  let win: WindowHandle | null = null;

  /** 还没落地的位移（物理像素） */
  let accX = 0;
  let accY = 0;
  /** 上一次滚轮的时刻（ms），用来识别新手势 */
  let lastAt = 0;
  let raf = 0;
  /** 正在等一次 `setPosition` 回来：期间不再叠加 IPC */
  let inflight = false;
  /** 试过一次就不再来（没有运行时 / 缺权限） */
  let dead = false;

  const fail = (reason: unknown): void => {
    if (dead) return;
    dead = true;
    console.warn("[board] Shift+滚轮 移动窗口不可用", reason);
    opts.onUnavailable?.(reason);
  };

  /** 读一次窗口几何（IPC，只在新手势开始时读） */
  const readGeometry = async (): Promise<Geometry | null> => {
    try {
      if (!win || !winModule || !dpiModule) {
        const [w, d] = await Promise.all([
          import("@tauri-apps/api/window"),
          import("@tauri-apps/api/dpi"),
        ]);
        winModule = w;
        dpiModule = d;
        win = w.getCurrentWindow();
      }
      const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
      /* 工作区（去掉任务栏）：读不到就退化成「只移不夹」—— 能移总比不能移好 */
      let area: WheelRect | null = null;
      try {
        const mon = await winModule.currentMonitor();
        if (mon) {
          area = {
            x: mon.workArea.position.x,
            y: mon.workArea.position.y,
            width: mon.workArea.size.width,
            height: mon.workArea.size.height,
          };
        }
      } catch {
        area = null;
      }
      return {
        pos: { x: pos.x, y: pos.y },
        size: { width: size.width, height: size.height },
        area,
      };
    } catch (err) {
      fail(err);
      return null;
    }
  };

  /** 把累加的位移落地：读几何 → 夹边界 → 一次 `setPosition` */
  const flush = async (): Promise<void> => {
    if (dead || inflight) return;
    if (!accX && !accY) return;
    inflight = true;
    try {
      const g = geom ?? (await readGeometry());
      if (!g) return; // fail() 里已经报过，别再往下走
      geom = g;
      const dx = accX;
      const dy = accY;
      // 先清账再等 IPC：await 期间新滚进来的位移属于下一帧，不能重复计算
      accX = 0;
      accY = 0;
      const next = clampWindowPosition({ x: g.pos.x + dx, y: g.pos.y + dy }, g.size, g.area);
      // 已经贴住工作区边缘：位置没变就不要再发 IPC（滚轮继续滚只是空转）
      if (next.x === g.pos.x && next.y === g.pos.y) return;
      g.pos = next;
      if (!win || !dpiModule) return;
      await win.setPosition(
        new dpiModule.PhysicalPosition(Math.round(next.x), Math.round(next.y)),
      );
    } catch (err) {
      fail(err);
    } finally {
      inflight = false;
      if (!dead && (accX || accY)) schedule();
    }
  };

  /** 合帧：一帧最多一次 IPC */
  function schedule(): void {
    if (dead || raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      void flush();
    });
  }

  const handle = (e: WheelEvent): void => {
    if (dead) return;
    /* deltaMode：0=像素 1=行 2=页。折算复用惯性滚动那份（`LINE_PX` 只有一个值，
       两处各写一份迟早会不一致）。 */
    const pagePx = window.innerHeight || 800;
    const stepX = wheelStep(normalizeWheelDelta(e.deltaX, e.deltaMode, pagePx));
    const stepY = wheelStep(normalizeWheelDelta(e.deltaY, e.deltaMode, pagePx));
    if (!stepX && !stepY) return;

    const now = performance.now();
    // 新手势：中间可能拖过标题栏 / 被程序改过位置，以真实几何重来
    if (now - lastAt > GESTURE_GAP) geom = null;
    lastAt = now;

    /* 位移是**物理像素**（`setPosition` 收的就是物理坐标），CSS px 要乘
       devicePixelRatio 才能和 4K 屏的缩放对上。 */
    const k = window.devicePixelRatio || 1;
    /* 一个事件只用一个轴：竖滚管上下、真的横滚（倾轮 / 触控板横滑）管左右。
       为什么以 deltaY 为主 —— WebView2 按住 Shift 滚竖轮时 **deltaY 还在**
       （Peek 的 Shift+滚轮缩放就是靠 `e.deltaY < 0` 判断方向的，一直是好的），
       所以"滚轮当方向键"就是上下；只有 deltaY 为 0 时才认 deltaX，
       两个都不为 0（斜着甩触控板）则按位移大的那个走，不做斜向合成 ——
       斜着挪窗口没人想要，还会让"一格一格推"变得不可预期。 */
    if (Math.abs(stepY) >= Math.abs(stepX)) accY += stepY * k;
    else accX += stepX * k;
    schedule();
  };

  return {
    handle,
    cancel(): void {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      accX = 0;
      accY = 0;
      // 几何缓存也丢掉：下次真的再滚之前，窗口可能已经被拖到别的显示器上了
      geom = null;
    },
  };
}
