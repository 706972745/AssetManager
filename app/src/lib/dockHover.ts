/**
 * 侧栏停靠面板「指针停在哪个面板」—— 全局单值。
 *
 * 为什么各个面板不能自己 `:hover`（照 FontManager `lib/dockHover.ts`）：
 * 分组面板与标签面板**上下相邻**。鼠标从标签内容区移向分组头部时，必然先离开标签，
 * 标签开始收起（释放两百多像素），下方面板整体被往下推，指针当场脱靶 ——
 * 于是分组「还没展开就被甩开」。
 *
 * 改成全局单值之后：指针一进某个面板就把 active 立刻切过去，另一个面板**同一帧**开始
 * 收起，与这个的展开动画并行，净位移只剩「展开高度 − 收起高度」，指针仍落在面板内。
 *
 * 离开时由面板自己延迟 200ms 再清（给「从面板移到相邻面板」留出路）。
 */
import { useSyncExternalStore } from "react";

let active: string | null = null;
const subs = new Set<() => void>();

function emit() {
  for (const f of subs) f();
}

export function setActiveDock(id: string | null) {
  if (active === id) return;
  active = id;
  emit();
}

export function getActiveDock() {
  return active;
}

export const DOCK_LEAVE_DELAY = 200;

export function useDockHover(): string | null {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => {
        subs.delete(cb);
      };
    },
    () => active,
    () => null,
  );
}
