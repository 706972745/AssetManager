import type {
  Asset,
  BoardPastedItem,
  DataRootInfo,
  DirNode,
  DirProbe,
  MoveOutcome,
  MovePlan,
  MovePolicy,
  ItemStat,
  TagCount,
  TagGroup,
  AutoTag,
  Query,
  ScanProgress,
  ScanResult,
  WarmProgress,
  Source,
  DupGroup,
  SimilarPair,
  RefItem,
  RenameItem,
  RenameResult,
  AutoRule,
} from "../types";
import { mockSource } from "./mock";

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

declare global {
  interface Window {
    __TAURI__?: { core?: { invoke: Invoke } };
    /** Tauri 2 默认注入的 IPC 桥，不依赖 withGlobalTauri */
    __TAURI_INTERNALS__?: { invoke: Invoke };
  }
}

const invoke: Invoke | null =
  window.__TAURI_INTERNALS__?.invoke ?? window.__TAURI__?.core?.invoke ?? null;

/**
 * 缩略图走自定义协议，字节永不经过 IPC（Tauri IPC 会把图片序列化成 JSON 数组）。
 *
 * `r` 是文件 mtime：它让 URL 随内容变化 ——
 * 文件改了 URL 就变，WebView 的磁盘缓存自动失效；文件没改就一直命中。
 * 踩过的坑：早期 URL 里没有 `r`，一次 404 被缓存成 immutable，之后这张图永远空白。
 */
function thumbUrl(a: { id: number; mtime: number }, px: number): string {
  return `http://thumb.localhost/${a.id}?px=${px}&r=${a.mtime}`;
}

export const tauriSource: Source = {
  desktop: true,
  roots: () => invoke!<DirNode[]>("am_roots"),
  libCount: () => invoke!<number>("am_lib_count"),
  removeLib: (path: string) => invoke!<number>("am_remove_lib", { path }),
  children: (parent) => invoke!<DirNode[]>("am_children", { parent }),
  list: (q: Query) => invoke!<Asset[]>("am_list", { q }),
  total: () => invoke!<number>("am_total"),
  thumb: (a: Asset, px: number) => thumbUrl(a, px),
  scanStart: (path: string) => invoke!<void>("am_scan_start", { path }),
  prefetchThumbs: (ids: number[], px: number) =>
    invoke!<void>("am_prefetch_thumbs", { ids, px }),
  warmThumbs: (px: number) => invoke!<WarmProgress>("am_warm_start", { px }),
  warmStop: () => invoke!<WarmProgress>("am_warm_stop"),
  warmStatus: () => invoke!<WarmProgress>("am_warm_status"),
  scanCancel: () => invoke!<boolean>("am_scan_cancel"),
  movePlan: (paths, target) => invoke!<MovePlan>("am_move_plan", { paths, target }),
  statPaths: (paths) => invoke!<ItemStat>("am_stat_paths", { paths }),
  dragOut: (paths, previewId) => invoke!<void>("am_drag_out", { paths, previewId }),
  watch: (dirs) => invoke!<number>("am_watch", { dirs }),
  reveal: (path: string) => invoke!<void>("am_reveal", { path }),
  openDefault: (path: string) => invoke!<void>("am_open_path", { path }),
  copyFile: (paths: string[]) => invoke!<void>("am_copy_files", { paths }),
  rename: (path: string, name: string) => invoke!<string>("am_rename", { path, name }),
  mkdir: (parent: string, name: string) => invoke!<string>("am_mkdir", { parent, name }),
  move: (paths: string[], target: string, policy: MovePolicy = "skip") =>
    invoke!<MoveOutcome>("am_move", { paths, target, policy }),
  moveStart: (paths: string[], target: string, policy: MovePolicy = "skip") =>
    invoke!<void>("am_move_start", { paths, target, policy }),
  setRating: (ids: number[], rating: number) =>
    invoke!<number>("am_set_rating", { ids, rating }),
  setNote: (id: number, note: string) => invoke!<void>("am_set_note", { id, note }),
  setSource: (id: number, url: string) => invoke!<void>("am_set_source", { id, url }),
  openUrl: (url: string) => invoke!<void>("am_open_url", { url }),
  tags: () => invoke!<TagCount[]>("am_tags"),
  autoTags: () => invoke!<TagCount[]>("am_auto_tags"),
  autoTagsFor: (a, manual) => invoke!<AutoTag[]>("am_auto_tags_for", { asset: a, manual }),
  assetTags: (ids) => invoke!<[number, string[]][]>("am_asset_tags", { ids }),
  tagAssets: (ids, tag) => invoke!<number>("am_tag_assets", { ids, tag }),
  untagAssets: (ids, tag) => invoke!<number>("am_untag_assets", { ids, tag }),
  deleteTag: (tag) => invoke!<number>("am_delete_tag", { tag }),
  tagGroups: () => invoke!<TagGroup[]>("am_tag_groups"),
  setTagColor: (tag, color) => invoke!<void>("am_tag_set_color", { tag, color }),
  setTagGroup: (tag, group) => invoke!<void>("am_tag_set_group", { tag, group }),
  renameTag: (oldName, newName) => invoke!<number>("am_tag_rename", { old: oldName, new: newName }),
  newGroup: (name) => invoke!<void>("am_group_new", { name }),
  renameGroup: (oldName, newName) => invoke!<void>("am_group_rename", { old: oldName, new: newName }),
  setGroupColor: (name, color) => invoke!<void>("am_group_set_color", { name, color }),
  deleteGroup: (name) => invoke!<number>("am_group_delete", { name }),
  remove: (paths: string[]) => invoke!<number>("am_delete", { paths }),
  undo: () => invoke!<string>("am_undo"),
  cacheStat: () => invoke!<[number, number]>("am_cache_stat"),
  cacheGc: (limit: number, target: number) =>
    invoke!<[number, number]>("am_cache_gc", { limit, target }),
  dataDir: () => invoke!<string>("am_data_dir"),
  rebuildIndex: () => invoke!<void>("am_rebuild_index"),
  exportUserdata: () => invoke!<string>("am_export_userdata"),
  pickFolder: (title) => invoke!<string | null>("am_pick_folder", { title: title ?? null }),
  dataRoot: () => invoke!<DataRootInfo>("am_data_root"),
  probeDir: (dir) => invoke!<DirProbe>("am_probe_dir", { dir }),
  setDataRoot: (input) =>
    invoke!<string>("am_set_data_root", {
      enabled: input.enabled,
      current: input.current,
      dirs: input.dirs,
      copyIndex: input.copyIndex,
      force: input.force ?? false,
    }),
  relaunch: () => invoke!<void>("am_relaunch"),
  openRecycle: () => invoke!<void>("am_open_recycle"),
  assetsByIds: (ids) => invoke!<Asset[]>("am_assets_by_ids", { ids }),
  peekOpen: (ids, index, longEdge) => invoke!<void>("am_peek_open", { ids, index, longEdge }),
  peekCopyFile: (paths) => invoke!<void>("am_peek_copy_file", { paths }),
  peekOpacity: (v) => invoke!<void>("am_peek_opacity", { v }),
  peekTrack: (path, gid) => invoke!<void>("am_peek_track", { path, gid }),
  clipText: (text) => invoke!<void>("am_clip_text", { text }),
  saveAs: (src, suggest) => invoke!<string | null>("am_save_as", { src, suggest }),
  clipPeek: () => invoke!<void>("am_clip_peek"),
  clipHistory: () => invoke!<[string, number, number][]>("am_clip_history"),
  snipStart: (mode) => invoke!<void>("am_snip_start", { mode }),
  snipCrop: (x, y, w, h, scale) =>
    invoke!<[string, number, number]>("am_snip_crop", { x, y, w, h, scale }),
  snipFinish: (png, action) => invoke!<string | null>("am_snip_finish", { png, action }),
  snipCancel: () => invoke!<void>("am_snip_cancel"),
  hotkeysSet: (list) => invoke!<string[]>("am_hotkeys_set", { list }),
  peekZoom: (step) => invoke!<void>("am_peek_zoom", { step }),
  peekRatio: (w, h) => invoke!<void>("am_peek_ratio", { w, h }),
  peekClose: () => invoke!<void>("am_peek_close"),
  boardOpen: (tag, title) => invoke!<number>("am_board_open", { tag, title: title ?? null }),
  boardClose: () => invoke!<void>("am_board_close"),
  boardPaste: (tag) => invoke!<BoardPastedItem>("am_board_paste", { tag }),
  peekTop: (on) => invoke!<boolean>("am_peek_top", { on }),
  peekReady: () => invoke!<void>("am_peek_ready"),
  colorCounts: () => invoke!<[string, number][]>("am_color_counts"),
  metaMissing: () => invoke!<[number, number]>("am_meta_missing"),
  fillPalette: (limit) => invoke!<[number, number]>("am_fill_palette", { limit }),
  dupesExact: (limit) => invoke!<[DupGroup[], number]>("am_dupes_exact", { limit }),
  dupesSimilar: (maxDist, limit) =>
    invoke!<SimilarPair[]>("am_dupes_similar", { maxDist, limit }),
  refsScan: (limit) => invoke!<[number, number]>("am_refs_scan", { limit }),
  refsSummary: () => invoke!<[number, number]>("am_refs_summary"),
  brokenRefs: (limit) => invoke!<RefItem[]>("am_broken_refs", { limit }),
  refsOf: (id) => invoke!<RefItem[]>("am_refs_of", { id }),
  refsUsing: (id) => invoke!<RefItem[]>("am_refs_using", { id }),
  renamePlan: (ids, rule) => invoke!<RenameItem[]>("am_rename_plan", { ids, rule }),
  renameApply: (items) => invoke!<RenameResult>("am_rename_apply", { items }),
  tagGetRule: (tag) => invoke!<AutoRule | null>("am_tag_get_rule", { tag }),
  tagSetRule: (tag, rule) => invoke!<void>("am_tag_set_rule", { tag, rule }),
  libFiles: () => invoke!<[string, string, number][]>("am_lib_files"),
};

export const source: Source = invoke ? tauriSource : mockSource;

/**
 * 订阅后端事件（扫描进度 / 目录变更 / 拖拽结束）。
 *
 * 走 `@tauri-apps/api/event` 而不是手写 IPC：事件通道有自己的一套握手协议，
 * 手搓很容易在 Tauri 升级后静默失联。浏览器预览下直接 no-op。
 */
export type Unlisten = () => void;

export function onAppEvent<T>(name: string, cb: (payload: T) => void): Unlisten {
  if (!invoke) return () => {};
  let stop: Unlisten = () => {};
  let dead = false;
  void import("@tauri-apps/api/event")
    .then(({ listen }) =>
      listen<T>(name, (e) => cb(e.payload)).then((un) => {
        if (dead) un();
        else stop = un;
      })
    )
    .catch(() => {});
  return () => {
    dead = true;
    stop();
  };
}

export type { ScanProgress, ScanResult } from "../types";
