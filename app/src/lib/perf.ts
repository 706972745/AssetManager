/**
 * 帧预算埋点（P3 T43）。
 *
 * 背景：滚动/切图"感觉卡"这件事，靠肉眼吵不出结论，得有个能报数的东西。
 * 规格里的口径是 **JS ≤ 6ms / Style+Layout ≤ 4ms / Paint ≤ 6ms**（合起来 16.7ms 一帧）。
 * 浏览器不让我们在正式包里分别取到这三段（那是 DevTools 的 Performance 面板干的活），
 * 所以这里退一步，量**能稳定拿到的三个数**：
 *
 * 1. 每帧间隔（rAF delta）→ 掉帧率。超过 16.7ms 就记一笔「性能债」；
 * 2. `longtask`（>50ms 的主线程任务）→ JS 卡顿的硬证据；
 * 3. 动画帧里的回调耗时（脚本部分）→ 接近规格里的"JS"那一段。
 *
 * **默认关**：只有 `?perf=1` 或 `localStorage["am.perf"]="1"` 才装监听。
 * 常开的话 rAF 循环本身就是耗电项，也会污染它自己要测的数。
 */
const FRAME_BUDGET = 16.7;
const JS_BUDGET = 6;

export interface PerfStat {
  frames: number;
  /** 超过 16.7ms 的帧数 */
  overFrames: number;
  /** 掉帧率（0–1） */
  dropRate: number;
  p50: number;
  p95: number;
  worst: number;
  /** 主线程长任务（>50ms）次数与总时长 */
  longTasks: number;
  longTaskMs: number;
  /** 单帧脚本耗时超 6ms 的次数 */
  overJs: number;
  /** 采样窗口（毫秒） */
  windowMs: number;
}

let started = false;
let raf = 0;
let last = 0;
let frames = 0;
let overFrames = 0;
let overJs = 0;
let worst = 0;
let longTasks = 0;
let longTaskMs = 0;
let t0 = 0;
const deltas: number[] = [];

function pct(p: number): number {
  if (!deltas.length) return 0;
  const a = [...deltas].sort((x, y) => x - y);
  return a[Math.min(a.length - 1, Math.floor(a.length * p))];
}

export function perfStat(): PerfStat {
  const windowMs = t0 ? performance.now() - t0 : 0;
  return {
    frames,
    overFrames,
    dropRate: frames ? overFrames / frames : 0,
    p50: +pct(0.5).toFixed(2),
    p95: +pct(0.95).toFixed(2),
    worst: +worst.toFixed(2),
    longTasks,
    longTaskMs: Math.round(longTaskMs),
    overJs,
    windowMs: Math.round(windowMs),
  };
}

export function perfReset() {
  frames = overFrames = overJs = longTasks = 0;
  longTaskMs = 0;
  worst = 0;
  deltas.length = 0;
  t0 = performance.now();
  last = 0;
}

export function installPerfProbe(): void {
  if (started) return;
  const on =
    new URLSearchParams(location.search).get("perf") === "1" ||
    (() => {
      try {
        return localStorage.getItem("am.perf") === "1";
      } catch {
        return false;
      }
    })();
  /* 没开也要把 dump 入口挂上：E2E 里可以手动 startPerfProbe() 再读数 */
  (window as unknown as { __amPerf?: () => PerfStat }).__amPerf = perfStat;
  (window as unknown as { __amPerfReset?: () => void }).__amPerfReset = perfReset;
  (window as unknown as { __amPerfStart?: () => void }).__amPerfStart = () => {
    perfReset();
    installPerfProbe();
  };
  if (!on) return;
  started = true;
  perfReset();

  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        longTasks += 1;
        longTaskMs += e.duration;
      }
    }).observe({ entryTypes: ["longtask"] });
  } catch {
    /* 老内核没有 longtask，不影响其它两个数 */
  }

  const tick = (ts: number) => {
    const t = performance.now();
    if (last) {
      const d = ts - last;
      if (d > 0 && d < 1000) {
        deltas.push(d);
        if (deltas.length > 2400) deltas.shift();
        frames += 1;
        if (d > FRAME_BUDGET) overFrames += 1;
        if (d > worst) worst = d;
      }
    }
    last = ts;
    /* 这一帧里我们自己的脚本花了多少（rAF 回调从进来到出去的时长） */
    const script = performance.now() - t;
    if (script > JS_BUDGET) overJs += 1;
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);

  /* 5 秒报一次，滚动测试时看控制台就能判断有没有性能债 */
  window.setInterval(() => {
    const s = perfStat();
    if (s.frames > 30 && (s.overFrames > 0 || s.longTasks > 0)) {
      // eslint-disable-next-line no-console
      console.info(
        `[perf] ${s.frames} 帧 / 掉帧 ${(s.dropRate * 100).toFixed(1)}% ` +
          `(p50 ${s.p50}ms · p95 ${s.p95}ms · 最差 ${s.worst}ms) · ` +
          `长任务 ${s.longTasks} 次/${s.longTaskMs}ms · 脚本超预算 ${s.overJs} 次`
      );
    }
    perfReset();
  }, 5000);

  window.addEventListener("beforeunload", () => cancelAnimationFrame(raf));
}
