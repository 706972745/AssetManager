/**
 * 两个停靠面板「现在谁开着」—— 只为了让它们**同时展开时各让一步**。
 *
 * 侧栏底部可用高度是固定的（标题栏 + 目录树 + 两个面板 + 底栏四行）。两个面板各自
 * `min(300px, 42vh)` 时同时展开就是 600px：先把中间的目录树压到几乎为零，再把底栏
 * 挤出可视区 —— 而外层是 overflow:hidden，用户看到的就是「底栏不见了」。
 *
 * 两边都上报之后各自加一个 `.half` 类，高度降到 `min(220px, 30vh)`：
 * 单开仍是 300px（够看十几行），双开各 220px（目录树还能留两百多像素）。
 *
 * 与 dockHover 分开是有意的：那个管「指针在哪」，这个管「面板开着没」。
 */
import { useSyncExternalStore } from "react";

type OpenState = { tags: boolean; collections: boolean };

let state: OpenState = { tags: false, collections: false };
const subs = new Set<() => void>();

export function setDockOpen(id: "tags" | "collections", value: boolean) {
  if (state[id] === value) return;
  state = { ...state, [id]: value };
  for (const f of subs) f();
}

export function useDockOpen(): OpenState {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => {
        subs.delete(cb);
      };
    },
    () => state,
    () => ({ tags: false, collections: false }),
  );
}
