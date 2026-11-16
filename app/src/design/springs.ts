import type { Transition } from "motion/react";

/**
 * 动画曲线（照 FontManager `src/design/springs.ts`，数值逐条相同）。
 *
 * 两个软件的「点开一个浮层」「切一个视图」必须是同一种手感，
 * 所以曲线不各写各的：改这里等于两边一起改（另一份在 FontManager 仓里）。
 */
export const spring: Transition = { type: "spring", stiffness: 300, damping: 30, mass: 1 };
export const springSnappy: Transition = { type: "spring", stiffness: 500, damping: 35 };
export const springSoft: Transition = { type: "spring", stiffness: 180, damping: 24 };
export const springBouncy: Transition = { type: "spring", stiffness: 560, damping: 24 };

/** 0.2 秒简约缓动：即时开合类过渡（不走弹簧，不出现过冲）。 */
export const easeTop: Transition = { duration: 0.2, ease: [0.32, 0.72, 0, 1] };

/** 遮罩淡入淡出：全站统一 0.18s（与 CSS 的 --t-quick 同档）。 */
export const scrimFade: Transition = { duration: 0.18 };

export const STAGGER_CAP = 14;
export const STAGGER_STEP = 0.04;

export function staggerDelay(index: number): number {
  return Math.min(index, STAGGER_CAP) * STAGGER_STEP;
}

/** 「减少动效」时把所有弹簧降成 0 时长（组件的 MotionConfig 也管，这里是兜底）。 */
export function respectMotion(reduced: boolean): Transition | undefined {
  return reduced ? { duration: 0 } : undefined;
}
