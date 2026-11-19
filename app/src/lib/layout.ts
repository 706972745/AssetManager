/**
 * 排布计算 —— 把素材列表算成 `Rect[]`（网格 / 瀑布流共用的纯函数，不依赖 DOM）。
 *
 * 为什么单独一个文件：只有这里需要"精确算高度"，抽出来才能同时被视图和滚动闸门复用；
 * 组件只负责把算好的 `rects` 画出来。改动会影响两种视图，改完两种都要看。
 */
import type { Asset } from "../types";

export interface Rect {
  a: Asset;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Layout {
  rects: Rect[];
  height: number;
}

function aspectOf(a: Asset): number {
  const w = a.width ?? 4;
  const h = a.height ?? 3;
  if (!w || !h) return 4 / 3;
  return Math.min(5, Math.max(0.2, w / h));
}

/** 列数上限（规格 §6.4，T20）：大档 ≤8 列、小档 ≤16 列。
 *  为什么要有上限：超宽屏（或者用户把窗口拉到 4K 宽）时，
 *  按宽度除下去能铺 30 列，每格只有 120px，图全成了马赛克，
 *  而且 4 万个 `img` 节点同时在场，滚一下就掉帧。宁可让格子变大。 */
export const MAX_COLS_GRID = [16, 12, 8];
/** 瀑布流单行 ≤12 列（masonry 没有"行"的概念，这里指列数） */
export const MAX_COLS_WATERFALL = 12;

/**
 * 瀑布流单格高度上限（CSS px）。
 *
 * 为什么要有：1:8、1:20 的长截图按原比例排下去，格子能有 8~20 倍列宽高，
 * 一屏里就剩它一根，而且图片为了塞进这个格子会被 `contain` 缩到只剩一条细线
 * （两侧全是留白，owner 2026-11-19 报的就是这个）。
 *
 * 现在超过上限就**封顶**：图片宽度铺满格子、纵向溢出，悬停滚轮在格子内滚动
 * （`Views.tsx` 改 `object-position`）—— 和详情面板顶部预览同一个思路。
 * 2.4 倍列宽 / 720px 两个上限取小的；想让它更高/更矮改这里一个数就行。
 */
export const WF_TILE_RATIO = 2.4;
export const WF_TILE_MAX_H = 720;

/** 瀑布流单格的实际高度：按宽高比算，再按上限封顶 */
export function waterfallTileH(colW: number, aspect: number): number {
  const natural = colW / Math.max(0.01, aspect);
  const cap = Math.min(colW * WF_TILE_RATIO, WF_TILE_MAX_H);
  return Math.max(24, Math.min(natural, cap));
}

/** 常规网格：正方形格，按行虚拟化。
 *  列数用四舍五入而不是向下取整 —— 让格子吃掉剩下的宽度（≈ 档位值），
 *  否则每行右边会留一条 100~170px 的空带，看着像界面没铺满。 */
export function gridLayout(
  items: Asset[],
  width: number,
  tile: number,
  gap = 8,
  maxCols = MAX_COLS_GRID[MAX_COLS_GRID.length - 1]
): Layout {
  const cols = Math.max(1, Math.min(maxCols, Math.round((width + gap) / (tile + gap))));
  const cell = Math.max(24, (width - gap * (cols - 1)) / cols);
  const step = cell + gap;
  const rects: Rect[] = new Array(items.length);
  for (let i = 0; i < items.length; i++) {
    rects[i] = {
      a: items[i],
      x: (i % cols) * step,
      y: Math.floor(i / cols) * step,
      w: cell,
      h: cell,
    };
  }
  const rows = Math.ceil(items.length / cols);
  return { rects, height: Math.max(0, rows * step - gap) };
}

/** 瀑布流（真 masonry）：固定列宽 + 最短列堆叠，相邻列之间不做行对齐
 *  （这正是和「justified 等高行」的区别 —— 后者会把上下两行拉成同高）。
 *  格子高度 = 列宽 / 宽高比，但**超过 `WF_TILE_MAX_H` 就封顶**：
 *  超高图不再是"一根细线 + 两侧大白边"，而是宽铺满、格内纵向滚（见 `waterfallTileH`）。 */
export function masonryLayout(
  items: Asset[],
  width: number,
  targetW: number,
  gap = 8,
  maxCols = MAX_COLS_WATERFALL
): Layout {
  if (!items.length) return { rects: [], height: 0 };
  const cols = Math.max(1, Math.min(maxCols, Math.round((width + gap) / (Math.max(60, targetW) + gap))));
  const colW = Math.max(40, (width - gap * (cols - 1)) / cols);
  const heights = new Array<number>(cols).fill(0);
  const rects: Rect[] = new Array(items.length);

  for (let i = 0; i < items.length; i++) {
    const a = items[i];
    const aspect = aspectOf(a);
    const h = waterfallTileH(colW, aspect);
    // 最短列优先 —— 列高不齐是刻意的，不做行对齐
    let c = 0;
    for (let k = 1; k < cols; k++) if (heights[k] < heights[c] - 0.5) c = k;
    rects[i] = { a, x: c * (colW + gap), y: heights[c], w: colW, h };
    heights[c] += h + gap;
  }
  return { rects, height: Math.max(0, Math.max(...heights) - gap) };
}
