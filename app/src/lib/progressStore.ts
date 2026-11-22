/**
 * 扫描 / 批量进度——底部进度条专用的轻量 store（写法照 `lib/dockHover.ts`）。
 *
 * 为什么不用 App 顶层 state：后端 `scan://progress` 每 120ms（`core/src/scan.rs`）、
 * `move://progress` 每 100ms（`core/src/fsops.rs`）推一次——挂在 App state 上就是整棵树
 * 按 8–10Hz 重渲，而真正变的只有底部那一条；拆出来后只有 `ProgressStrip` 重渲。
 * 硬约束：`getSnapshot` 必须返回**稳定引用**——值没变时返回同一个对象。React 用
 * `Object.is` 比较快照，每次新建对象会被当成"状态又变了"，直接无限重渲（比原来更糟）。
 */
import { useSyncExternalStore } from "react";

/** 扫描态（与原先 App 的 scan state 同口径；`active=false` 时其余字段归零） */
export type ScanStripState = { active: boolean; dirs: number; files: number; current: string };
/** 批量态；null = 没有批量任务（与原先 App 的 batch state 同口径） */
export type BatchStripState = { done: number; total: number; label?: string };
export type ProgressSnapshot = { scan: ScanStripState; batch: BatchStripState | null };

const IDLE_SCAN: ScanStripState = { active: false, dirs: 0, files: 0, current: "" };
let snap: ProgressSnapshot = { scan: IDLE_SCAN, batch: null };
const subs = new Set<() => void>();

/** 写一份进度（字段没变就提前返回，快照引用不动，否则订阅者会白重渲，见文件头）。
 *  label 里带"正在移动 · <当前文件>"，所以逐条比 label 就是比进度条文案。 */
export function setProgress(kind: "scan", data: ScanStripState): void;
export function setProgress(kind: "batch", data: BatchStripState | null): void;
export function setProgress(kind: keyof ProgressSnapshot, data: ScanStripState | BatchStripState | null) {
  if (kind === "scan") {
    const cur = snap.scan;
    const next = data as ScanStripState;
    if (cur.active === next.active && cur.dirs === next.dirs && cur.files === next.files && cur.current === next.current) return;
    snap = { scan: next, batch: snap.batch };
  } else {
    const cur = snap.batch;
    const next = data as BatchStripState | null;
    if (cur === next) return;
    if (cur && next && cur.done === next.done && cur.total === next.total && cur.label === next.label) return;
    snap = { scan: snap.scan, batch: next };
  }
  for (const f of subs) f();
}

/** 结束 / 取消任务，恢复空闲态（扫描 active=false；批量 null = 不显示） */
export function clearProgress(kind: keyof ProgressSnapshot) {
  if (kind === "scan") setProgress("scan", IDLE_SCAN);
  else setProgress("batch", null);
}

/** 订阅两条进度：底部同一时刻只画一条（batch 优先 scan），合成一个快照就够 */
export function useProgress(): ProgressSnapshot {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => snap,
    () => snap,
  );
}
