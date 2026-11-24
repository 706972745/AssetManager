import type {
  Asset,
  AutoRule,
  AutoTag,
  BoardAnnRow,
  BoardItemDto,
  BoardNote,
  BoardPastedItem,
  CollRow,
  DataRootInfo,
  DirNode,
  DirProbe,
  DupGroup,
  ExifInfo,
  ImportFilesReport,
  ImportReport,
  ItemStat,
  LibraryZipOut,
  MoveOutcome,
  MovePlan,
  MovePolicy,
  PackageOut,
  Query,
  RefItem,
  RenameItem,
  RenameResult,
  ResizeOut,
  ScanProgress,
  ScanResult,
  SimilarPair,
  SnapshotAdd,
  SnapshotResolve,
  SnapshotRow,
  SnapshotStats,
  SnapshotZip,
  Source,
  TagCount,
  TagGroup,
  TrashEntry,
  UpdateInfo,
  WarmProgress,
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
 *
 * `v` 是**出图口径版本**：URL 里只有 mtime 时，改了出图规则（比如 0.3.12 起极端比例改按短边）
 * 会因为 WebView 里那份 `immutable` 缓存继续吐老图 —— 用户看到的就是"改完还是糊"。
 * 每次动缩略图口径就把这个数 +1，等于一次性把全库的 HTTP 缓存作废（本地文件仍是命中）。
 */
const THUMB_VER = 2;

function thumbUrl(a: { id: number; mtime: number }, px: number): string {
  return `http://thumb.localhost/${a.id}?px=${px}&r=${a.mtime}&v=${THUMB_VER}`;
}

export const tauriSource: Source = {
  desktop: true,
  roots: () => invoke!<DirNode[]>("am_roots"),
  libCount: () => invoke!<number>("am_lib_count"),
  builtinRoots: () => invoke!<string[]>("am_builtin_roots"),
  removeLib: (path: string) => invoke!<number>("am_remove_lib", { path }),
  setCurrentLib: (path: string) => invoke!<boolean>("am_set_current_lib", { path }),
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
  /** 最近 `ms` 内**真人**按下的键是不是 `vk`（虚拟键码）。
   *  注入的按键不算 —— 本机有程序会向前台窗口注入 Ctrl+C（见 core/src/keywatch.rs），
   *  这是 Ctrl+C 复制原文件的第一道闸，判不了（钩子没装上）时后端放行。 */
  keyRecent: (vk: number, ms = 1200) => invoke!<boolean>("am_key_recent", { vk, ms }),
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
  tagCreate: (name) => invoke!<void>("am_tag_create", { name }),
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
  exportUserdata: (path) =>
    invoke!<string>("am_export_userdata", { path: path ?? null }),
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
  /* 应用内回收站（0.1.6） */
  trashList: () => invoke!<TrashEntry[]>("am_trash_list"),
  trashMedia: (ids) => invoke!<(string | null)[]>("am_trash_media", { ids }),
  uiPrefsSet: (prefs) => invoke!<void>("am_ui_prefs_set", { prefs }),
  setUiScale: (scale) => invoke!<number>("am_set_ui_scale", { scale }),
  setAutostart: (enabled) => invoke!<void>("am_set_autostart", { enabled }),
  getAutostart: () => invoke!<boolean>("am_get_autostart"),
  trashRestore: (ids: string[], to?: string) =>
    invoke!<number>("am_trash_restore", { ids, to: to ?? null }),
  trashPurge: (ids: string[]) => invoke!<number>("am_trash_purge", { ids }),
  trashEmpty: () => invoke!<number>("am_trash_empty"),
  assetsByIds: (ids) => invoke!<Asset[]>("am_assets_by_ids", { ids }),
  peekOpen: (ids, index, longEdge) => invoke!<void>("am_peek_open", { ids, index, longEdge }),
  peekCopyFile: (paths) => invoke!<void>("am_peek_copy_file", { paths }),
  peekOpacity: (v) => invoke!<void>("am_peek_opacity", { v }),
  peekTrack: (path, gid) => invoke!<void>("am_peek_track", { path, gid }),
  peekSetFrame: (on: boolean) => invoke!<void>("am_peek_set_frame", { on }),
  clipText: (text) => invoke!<void>("am_clip_text", { text }),
  saveAs: (src, suggest) => invoke!<string | null>("am_save_as", { src, suggest }),
  peekSaveLib: (path) => invoke!<string>("am_peek_save_lib", { path }),
  setUiTheme: (theme) => invoke!<void>("am_ui_theme", { theme }),
  clipPeek: () => invoke!<void>("am_clip_peek"),
  clipHistory: () => invoke!<[string, number, number][]>("am_clip_history"),
  snipStart: () => invoke!<void>("am_snip_start"),
  snipCrop: (x, y, w, h, scale) =>
    invoke!<[string, number, number]>("am_snip_crop", { x, y, w, h, scale }),
  /** `place` = 选区在虚拟桌面上的物理 `[x, y, w, h]`；贴图靠它"摆在截图那个位置"。 */
  snipFinish: (png, action, from, place) =>
    invoke!<string | null>("am_snip_finish", {
      png,
      action,
      from: from ?? "btn",
      place: place ?? null,
    }),
  snipCancel: () => invoke!<void>("am_snip_cancel"),
  snipReady: () => invoke!<void>("am_snip_ready"),
  snipFull: () => invoke!<string>("am_snip_full"),
  cursorStep: (dx, dy) => invoke!<void>("am_cursor_step", { dx, dy }),
  snipWindowAt: (x, y, scale) =>
    invoke!<{ x: number; y: number; w: number; h: number } | null>("am_snip_window_at", {
      x,
      y,
      scale,
    }),
  hotkeysSet: (list) => invoke!<string[]>("am_hotkeys_set", { list }),
  peekZoom: (step) => invoke!<void>("am_peek_zoom", { step }),
  peekRatio: (w, h) => invoke!<void>("am_peek_ratio", { w, h }),
  peekClose: () => invoke!<void>("am_peek_close"),
  boardOpen: (tag, title) => invoke!<number>("am_board_open", { tag, title: title ?? null }),
  boardClose: () => invoke!<void>("am_board_close"),
  boardHide: () => invoke!<void>("am_board_hide"),
  boardTop: (on) => invoke!<boolean>("am_board_top", { on }),
  boardIsTop: () => invoke!<boolean>("am_board_is_top"),
  boardPaste: (tag) => invoke!<BoardPastedItem>("am_board_paste", { tag }),

  /* ---------------- 分组（0.2.x） ---------------- */
  collList: () => invoke!<CollRow[]>("am_coll_list"),
  collCreate: (name) => invoke!<number>("am_coll_create", { name }),
  collRename: (id, name) => invoke!<void>("am_coll_rename", { id, name }),
  collDelete: (id) => invoke!<void>("am_coll_delete", { id }),
  collAdd: (id, ids) => invoke!<number>("am_coll_add", { id, ids }),
  collAddTag: (id, tag) => invoke!<number>("am_coll_add_tag", { id, tag }),
  collRemove: (id, ids) => invoke!<number>("am_coll_remove", { id, ids }),
  collOf: (gid) => invoke!<number[]>("am_coll_of", { gid }),
  collOpen: (id) => invoke!<number>("am_coll_open", { coll: id }),
  collPaste: (id) => invoke!<BoardPastedItem>("am_coll_paste", { coll: id }),

  /* ---------------- 快照（0.12.0） ---------------- */
  snapshotList: () => invoke!<SnapshotRow[]>("am_snapshot_list"),
  snapshotStats: (id) => invoke!<SnapshotStats>("am_snapshot_stats", { snapshot: id }),
  snapshotCreate: (name) => invoke!<number>("am_snapshot_create", { name }),
  snapshotRename: (id, name) => invoke!<void>("am_snapshot_rename", { id, name }),
  snapshotDelete: (id) => invoke!<void>("am_snapshot_delete", { id }),
  snapshotAdd: (id, ids) => invoke!<SnapshotAdd>("am_snapshot_add", { snapshot: id, ids }),
  snapshotAddTag: (id, tag) => invoke!<SnapshotAdd>("am_snapshot_add_tag", { snapshot: id, tag }),
  snapshotResolve: (id) => invoke!<SnapshotResolve>("am_snapshot_resolve", { snapshot: id }),
  snapshotRemove: (id, hashes) => invoke!<number>("am_snapshot_remove", { snapshot: id, hashes }),
  snapshotExport: (id) => invoke!<string | null>("am_snapshot_export", { snapshot: id }),
  snapshotImport: () =>
    invoke!<[number, string, number, number] | null>("am_snapshot_import"),
  snapshotExportZip: (id) => invoke!<SnapshotZip | null>("am_snapshot_export_zip", { snapshot: id }),
  /* 双击导入（0.4.2）：路径由后端经 `am://open-file` 递进来，这里只负责调一次。 */
  openImportFile: (path) => invoke!<ImportReport | null>("am_open_import_file", { path }),
  /* 界面导入（0.4.2）：任意格式的文件复制进当前文件夹 */
  importFiles: (paths, target) =>
    invoke!<ImportFilesReport>("am_import_files", { paths, target }),
  pickAndImport: (target) => invoke!<ImportFilesReport | null>("am_pick_and_import", { target }),
  exportPackage: (path) =>
    invoke!<PackageOut | null>("am_export_package", { path: path ?? null }),
  exportLibraryZip: (path) =>
    invoke!<LibraryZipOut | null>("am_export_library_zip", { path: path ?? null }),
  snapshotOpen: (id) => invoke!<number>("am_snapshot_open", { snapshot: id }),
  snapshotPaste: (id) => invoke!<BoardPastedItem>("am_snapshot_paste", { snapshot: id }),
  snapshotCombine: (ids, name, mode) =>
    invoke!<[number, number, number]>("am_snapshot_combine", { ids, name, mode }),
  boardLayout: (board) =>
    invoke!<[number, number, number, number, number, number][]>("am_board_layout", { board }),
  boardPlace: (board, rows) => invoke!<number>("am_board_place", { board, rows }),
  boardNotes: (board) => invoke!<BoardNote[]>("am_board_notes", { board }),
  /** 便签落库。`look` 传 null = 只挪位置，保留库里原有字体外观 */
  boardNotePut: (board, nid, x, y, w, h, text, look) =>
    invoke!<number>("am_board_note_put", {
      board,
      nid,
      x,
      y,
      w,
      h,
      text,
      font: look ? look.font : null,
      fs: look ? look.fs : null,
      color: look ? look.color : null,
      bold: look ? look.bold : null,
      vert: look ? look.vert : null,
    }),
  boardNoteDel: (board, nid) => invoke!<number>("am_board_note_del", { board, nid }),
  /** 打组 / 解组：`grp = -1` 新建（后端发号并返回），`0` 解组 */
  boardGroup: (board, keys, grp) => invoke!<number>("am_board_group", { board, keys, grp }),
  systemFonts: () => invoke!<string[]>("am_system_fonts"),
  boardAddPaths: (paths) =>
    invoke!<[BoardItemDto[], string[]]>("am_board_add_paths", { paths }),
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
  resizeSave: (path, maxEdge, format) =>
    invoke!<ResizeOut | null>("am_resize_save", { path, maxEdge, format }),
  exif: (path) => invoke!<ExifInfo | null>("am_exif", { path }),

  /* ---------------- 检查更新（0.1.35） ----------------
     参数名按 Rust 侧的 snake_case 写 camelCase：Tauri 2 会把 JS 的 `assetId`
     自动映射到 `asset_id`，写错成 `assetID` / `id` 只会在运行时**静默**报缺参数。 */
  updateCheck: (force = false) => invoke!<UpdateInfo>("am_update_check", { force }),
  updateDownload: (url, name, sha256, assetId) =>
    invoke!<string>("am_update_download", { url, name, sha256, assetId }),
  /* version 是 Option<String>：不传版本号时要显式给 null，不能给 undefined
     （undefined 在 IPC 序列化里会被整条丢掉，Rust 侧会拿到"缺参数"而不是 None） */
  updateInstall: (path, sha256, version) =>
    invoke!<void>("am_update_install", { path, sha256, version: version ?? null }),
};

export const source: Source = invoke ? tauriSource : mockSource;

/* ---------------- 灵感板标注（一笔 = 一层） ----------------
   没有塞进 `Source` / `mockSource`：这两个是全站共用的大接口，标注只是灵感板
   自己的小接口，塞进去等于逼 mock 也实现一遍。这里直接走同一个 `invoke`，
   浏览器预览（没有 Tauri 运行时）用一个内存 Map 顶着 —— 预览本来就没有注册库，
   刷新即丢是预期行为。 */
const annMem = new Map<string, BoardAnnRow[]>();
let annMemId = 0;

export const boardAnnApi = {
  list: (board: string): Promise<BoardAnnRow[]> =>
    invoke
      ? invoke<BoardAnnRow[]>("am_board_ann", { board })
      : Promise.resolve([...(annMem.get(board) ?? [])]),

  add: (
    board: string,
    z: number,
    kind: string,
    payload: string,
    x: number,
    y: number,
    w: number,
    h: number,
  ): Promise<number> => {
    if (invoke) return invoke<number>("am_board_ann_add", { board, z, kind, payload, x, y, w, h });
    const aid = ++annMemId;
    annMem.set(board, [...(annMem.get(board) ?? []), { aid, z, kind, payload, x, y, w, h }]);
    return Promise.resolve(aid);
  },

  update: (board: string, aid: number, x: number, y: number, z: number): Promise<number> => {
    if (invoke) return invoke<number>("am_board_ann_update", { board, aid, x, y, z });
    const rows = annMem.get(board) ?? [];
    annMem.set(board, rows.map((r) => (r.aid === aid ? { ...r, x, y, z } : r)));
    return Promise.resolve(rows.some((r) => r.aid === aid) ? 1 : 0);
  },

  del: (board: string, aid: number): Promise<number> => {
    if (invoke) return invoke<number>("am_board_ann_del", { board, aid });
    const rows = annMem.get(board) ?? [];
    annMem.set(board, rows.filter((r) => r.aid !== aid));
    return Promise.resolve(rows.length - (annMem.get(board)?.length ?? 0));
  },

  clear: (board: string): Promise<number> => {
    if (invoke) return invoke<number>("am_board_ann_clear", { board });
    const n = annMem.get(board)?.length ?? 0;
    annMem.set(board, []);
    return Promise.resolve(n);
  },
};


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
