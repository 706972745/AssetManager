export type Kind = 0 | 1 | 2 | 3 | 4 | 5;

/**
 * 只有图片(0)和视频(1)能出缩略图。
 *
 * 其它类型（PDF/JSON/PSD 工程稿/音频/模型）**根本没有可显示的图**：
 * 以前每张这样的瓦片都会请求一次 `thumb://`、吃一个 404、再画一块「暂无预览」的灰方块。
 * 一个库里有三万条 json 时，就是三万次必然失败的请求 + 三万块看起来像"坏了"的方块。
 */
export const PREVIEWABLE_KINDS: ReadonlySet<number> = new Set([0, 1]);

/** kind → 磁贴上的"代替缩略图"图标名（与 components/ui.tsx 的图标表对应） */
export const KIND_ICON: Record<number, string> = {
  1: "video",
  2: "audio",
  3: "model",
  4: "file",
  5: "file",
};

export interface Asset {
  id: number;
  dirId: number;
  name: string;
  ext: string;
  kind: number;
  size: number;
  mtime: number;
  /** 首次入库时间（unix 秒）。**0 = 老数据没记过**，界面显示「未知」 */
  addedAt: number;
  width: number | null;
  height: number | null;
  thumbState: number;
  /** 缩略图质量：1 = 自己解码（清晰） / 2 = 内嵌预览或系统预览（不可控） */
  quality: number;
  path: string;
  /** 用户评分 0–5（0 = 未评分） */
  rating: number;
  /** 用户注释（空串 = 没写） */
  note: string;
  /** 来源 URL（空串 = 没填） */
  sourceUrl: string;
  /** 5 个主色（`rrggbb,rrggbb,…`；空串 = 还没算过）—— 出缩略图时顺带算出来的 */
  palette: string;
  /** 色桶（red / orange / …；空串 = 没算过或图是灰的没主色）—— 按颜色筛走它 */
  color: string;
}

export interface DirNode {
  id: number;
  path: string;
  name: string;
  parentId: number | null;
  childCnt: number;
  assetCnt: number;
  hasChildren: boolean;
}

export type SortKey = "mtime_desc" | "name_asc" | "size_desc" | "rating_desc";

export interface Query {
  dirId: number | null;
  recursive: boolean;
  kinds: number[] | null;
  text: string;
  sort: SortKey;
  cursor: [number, number] | null;
  limit: number;
  /** 标签筛选：`客户稿` 这类走 asset_tags；`fmt:jpg` / `place:4k` 这类是自动标签，后端翻译成列条件 */
  tag?: string | null;
  /** 颜色筛选（T24）：色桶名（red / orange / …），空 = 不筛 */
  color?: string | null;
  /** 评分下限（1–5）：只列 n 星及以上；空 = 不筛 */
  minRating?: number | null;
  /** 分组筛选（分组 id）：只看这一组里的素材；空 = 不筛 */
  collection?: number | null;
  /** 快照筛选（快照 id）：只看这份快照里认得出的素材；空 = 不筛。跨库生效 */
  snapshot?: number | null;
}


/** 精确重复组（同一份文件的多份拷贝） */
export interface DupGroup {
  hash: string;
  size: number;
  /** 全局 id */
  ids: number[];
}

/** 相似重复对（汉明距离越小越像） */
export interface SimilarPair {
  a: number;
  b: number;
  dist: number;
}

/** 引用行（T26） */
export interface RefItem {
  srcId: number;
  srcName: string;
  srcPath: string;
  dstPath: string;
  dstName: string;
  dstId: number | null;
  missing: boolean;
}

/** 批量重命名规则（T33） */
export interface RenameRule {
  find: string;
  replace: string;
  prefix: string;
  suffix: string;
  /** 0 = 不加序号 */
  numberFrom: number;
  numberPad: number;
  numberBeforeExt: boolean;
  lower: boolean;
  upper: boolean;
  keepExt: boolean;
}

export interface RenameItem {
  id: number;
  oldName: string;
  newName: string;
  /** 非空 = 这一条不能改 */
  error: string;
}

export interface RenameResult {
  ok: number;
  errors: string[];
  dirs: string[];
}

/** 自动标签规则（T34）：全部条件都满足才算命中，空字段 = 不限制 */
export interface AutoRule {
  dir: string;
  ext: string[];
  kind: number[];
  sizeMin: number;
  sizeMax: number;
  pxMin: number;
  pxMax: number;
  needDims: boolean;
}

/** 快照（侧栏一条）。`count` 是**记下来的条数**，不是"现在找得到的张数" */
export interface SnapshotRow {
  id: number;
  name: string;
  count: number;
  createdAt: number;
  updatedAt: number;
}

/** 分组（用户自己攒的「一撮素材」）。`count` 是后端按"素材现在还在不在"算出来的真实数。 */
export interface CollRow {
  id: number;
  name: string;
  count: number;
  createdAt: number;
}

/** 加入快照的结果：新加 / 本来就有 / 失败清单 */
export interface SnapshotAdd {
  added: number;
  total: number;
  already: number;
  failed: string[];
}

/** 对账后的一条：`gid` 为空 = 这次没找到（**条目不会被删**，可能只是硬盘没插） */
export interface SnapshotResolvedItem {
  hash: string;
  name: string;
  gid: number | null;
  path: string;
  lib: string;
  size: number;
  w: number | null;
  h: number | null;
  mtime: number;
}

export interface SnapshotResolve {
  items: SnapshotResolvedItem[];
  found: number;
  missing: number;
}

export interface SnapshotZip {
  path: string;
  ok: number;
  bad: string[];
  missing: number;
}

/**
 * 双击导入的结果（0.4.2）。
 *
 * 三种导出文件共用一个形状：`kind` 说明它是哪一类，`message` 是后端拼好的一句话
 * （已经带数量，前端直接 toast），其余计数按 `kind` 取——没用到的字段是 0。
 */
export interface ImportReport {
  kind: "userdata" | "snapshot" | "package";
  message: string;
  /** 用户数据：新登记了几个库根 */
  libsAdded: number;
  /** 用户数据：回填了几张素材（评分 / 备注 / 来源 / 标签） */
  assetsUpdated: number;
  /** 用户数据：有几条在索引里找不到 */
  assetsUnknown: number;
  /** 资源包：解出来几个文件 */
  files: number;
  /** 快照：新增几条 / 跳过几条坏数据 */
  added: number;
  skipped: number;
  /** 快照：落库后的 id 与最终名字（重名会自动加 (2)） */
  snapshotId: number;
  snapshotName: string;
  /** 资产包清单回填：还原了几张的评分 / 标签 */
  rated: number;
  tagged: number;
}

/** 界面里「导入文件到当前文件夹」的结果（0.4.2） */
export interface ImportFilesReport {
  /** 真正复制进来的文件数 */
  copied: number;
  /** 整条跳过的源（源就在目标里） */
  skipped: number;
  /** 这次新入库的素材数 */
  added: number;
  /** 失败条目：`[源路径, 人话原因]` */
  failed: [string, string][];
}

/** 导出资产包的结果（0.4.2）：路径 + 打包数 + 跳过数（文件不在原位） */
export interface PackageOut {
  path: string;
  ok: number;
  skipped: number;
}

/** 整库 zip 的一个分片（超过 4 GB / 6.5 万条会拆成 part1、part2…） */
export interface LibraryZipPart {
  path: string;
  name: string;
  files: number;
  bytes: number;
}

/** 整库 zip 的结果：原图 + `_AssetManager/` 里的库数据，落在用户选的文件夹 */
export interface LibraryZipOut {
  dir: string;
  parts: LibraryZipPart[];
  /** 真正进包的素材数 */
  files: number;
  /** 索引里有、磁盘上找不到的条数 */
  skipped: number;
  /** 单个文件就超过单包上限、装不进去的条数 */
  oversized: number;
}

/** 灵感板上一张图（板上看到的永远是缩略图，操作一律落到 path 的源文件） */
export interface BoardItemDto {
  id: number;
  src: string;
  w: number;
  h: number;
  name: string;
  path: string;
  /** 动图（gif / apng / webp）：默认静止，指针停上去才播 `raw` */
  anim: boolean;
  /** 原文件地址（`px=0`）：动图悬停时换成它 */
  raw: string;
}

/** 灵感板上的一张文字便签（位置/尺寸/文字/字体外观都在注册库里，跟着板走） */
export interface BoardNote {
  nid: number;
  x: number;
  y: number;
  w: number;
  h: number;
  text: string;
  /** 字体族名（空 = 系统默认） */
  font: string;
  /** 字号（画布单位；0 = 默认 12.5。拖角缩放时按比例跟着变） */
  fs: number;
  /** 文字颜色 `#rrggbb`（空 = 默认浅色） */
  color: string;
  bold: boolean;
  /** 所属组（0 = 没打组；同一块板上相同的非零值 = 同一组） */
  grp: number;
}

/** 灵感板里粘贴进来的一张（后端已落盘 + 入库 + 打标签，这里只是它的显示信息） */
export interface BoardPastedItem {  id: number;
  /** `thumb://` 地址：板上看到的永远是缩略图，复制/打开一律走 path 的源文件 */
  src: string;
  w: number;
  h: number;
  name: string;
  path: string;
  anim: boolean;
  raw: string;
}

/** 标签 + 使用数（侧栏用） */
export interface TagCount {
  name: string;
  count: number;
  /** 颜色键（见 lib/tags.ts 的 TAG_COLORS）；缺省 = 灰 */
  color?: string | null;
  /** 所属分组名；缺省 = 未分组 */
  group?: string | null;
}

/** 标签组（只有一层）：组里放标签，组不能嵌套 */
export interface TagGroup {
  name: string;
  color?: string | null;
  /** 组内素材数（同一素材被组内多个标签命中只算一次） */
  count: number;
  /** 组里标签个数（空组也要显示出来） */
  tags: number;
}

/** 自动标签的推导结果（详情面板用）：凭什么给这条标签 */
export interface AutoTag {
  tag: string;
  reason: string;
}

/** 落盘前体检：给「>100 个 / >2GB」与「同名三选」用 */
export interface MovePlan {
  files: number;
  bytes: number;
  /** 目标目录里已有同名的项（只列名字） */
  conflicts: string[];
  crossVolume: boolean;
  dirs: string[];
}

export interface ItemStat {
  files: number;
  bytes: number;
}

/** 同名冲突策略：跳过 / 保留两者 / 替换（替换会先把旧文件送回收站） */
export type MovePolicy = "skip" | "keep_both" | "replace";

export interface MoveOutcome {
  moved: number;
  skipped: string[];
  renamed: string[];
  replaced: string[];
}

/** 数据文件夹配置（设置面板「素材库文件夹」那一块）。
 *  `effectiveDir` 是**真正在用**的目录：自定义不可用时后端已回退成默认，
 *  界面显示它而不是 current —— 用户看到的必须是事实。 */
/**
 * 全库缩略图预热的进度。
 *
 * 预取只管「下一屏」，第一次打开很久没看的目录仍然要现做图。
 * 预热在后台把整个库按当前档位生成一遍，跑完之后**任何位置都是立刻出来**。
 * `running=false` 且 `done===total` 就是"全库已就绪"。
 */
export interface WarmProgress {
  running: boolean;
  done: number;
  total: number;
}

export interface DataRootInfo {
  defaultDir: string;
  effectiveDir: string;
  enabled: boolean;
  current: string | null;
  dirs: string[];
  max: number;
}

/**
 * 目录体检：用户选完「数据文件夹」后先看一眼里面有什么。
 *
 * 为什么需要：2026-11-16 用户把**素材库根**设成了数据文件夹，以为「设好了素材库」，
 * 回主界面只有 0/3 和空状态 —— 「设置了文件库但没有任何效果」。
 * 名字叫「数据文件夹」，用户理解成「我的素材放哪」；词改了还不够，得当场问一句。
 */
export interface DirProbe {
  /** 近层里的图片张数（数到上限就停） */
  images: number;
  videos: number;
  /** 顶层条目数 */
  entries: number;
  /** 里面已经有 index.db —— 这个目录以前被当过数据目录 */
  hasIndexDb: boolean;
}

/** 后台移动的逐项进度（`move://progress`） */
export interface MoveProgress {
  done: number;
  total: number;
  current: string;
}

/** 后台移动的收尾（`move://done`）：ok=false 时 message 就是给人看的原因 */
export interface MoveResult {
  ok: boolean;
  message: string;
  outcome: MoveOutcome | null;
}

export interface ScanProgress {
  dirs: number;
  files: number;
  current: string;
  done: boolean;
}

export interface ScanResult {
  ok: boolean;
  message: string;
  stats: { dirs: number; files: number; images: number; elapsedMs: number };
}

export interface Source {
  readonly desktop: boolean;
  roots(): Promise<DirNode[]>;
  /** 已加入的库根数量（最多 3 个库，与 FontManager 的多库上限一致） */
  libCount(): Promise<number>;
  /** 内置库路径（软件自己的截图目录）：不占 3 个名额、不能移除、永远显示 */
  builtinRoots(): Promise<string[]>;
  /** 从素材库移除一个库根：只摘注册，库里的 `.AssetManager` 一个字节都不动 */
  removeLib(path: string): Promise<number>;
  /**
   * 切库：把"当前库"切到这条路径所在的库。
   * 每个库自带一套标签/分组（存在库自己的 `.AssetManager` 里），
   * 切完必须重拉 tags / groups，否则会看到上一个库的标签。
   */
  setCurrentLib(path: string): Promise<boolean>;
  children(parent: number | null): Promise<DirNode[]>;
  list(q: Query): Promise<Asset[]>;
  total(): Promise<number>;
  thumb(a: Asset, px: number): string;
  /** 启动后台扫描（立刻返回，进度走 scan://progress 事件） */
  scanStart(path: string): Promise<void>;
  /**
   * 预取一批缩略图（后台低优先，命中缓存直接跳过）。
   * 滑动时的等待感来自「滚到哪才开始生成」—— 提前一屏做好，滑过去就是缓存命中。
   */
  prefetchThumbs(ids: number[], px: number): Promise<void>;
  /** 后台把全库缩略图预生成一遍（低优先、可停、有进度） */
  warmThumbs(px: number): Promise<WarmProgress>;
  warmStop(): Promise<WarmProgress>;
  warmStatus(): Promise<WarmProgress>;
  /** 取消正在跑的扫描（已入库的部分保留） */
  scanCancel(): Promise<boolean>;
  /** 移动前体检：体量、同名冲突、是否跨盘 */
  movePlan(paths: string[], target: string): Promise<MovePlan>;
  statPaths(paths: string[]): Promise<ItemStat>;
  /** 批量的体量（删除前确认用） */
  /** 拖出窗口：出去的是磁盘原文件（CF_HDROP / OLE） */
  dragOut(paths: string[], previewId?: number | null): Promise<void>;
  /** 监听一批目录（非递归）。返回真正盯上的数量 */
  watch(dirs: string[]): Promise<number>;
  reveal(path: string): Promise<void>;
  /** 用系统默认程序打开**原文件**（不是缩略图） */
  openDefault(path: string): Promise<void>;
  /** 把**原文件**放进剪贴板（CF_HDROP，可粘到资源管理器 / PS） */
  copyFile(paths: string[]): Promise<void>;
  rename(path: string, name: string): Promise<string>;
  mkdir(parent: string, name: string): Promise<string>;
  move(paths: string[], target: string, policy?: MovePolicy): Promise<MoveOutcome>;
  /** 批量设评分（0–5，0 = 清掉）；返回真正改动的条数 */
  setRating(ids: number[], rating: number): Promise<number>;
  /** 写注释（空串 = 清空） */
  setNote(id: number, note: string): Promise<void>;
  /** 写来源 URL（空串 = 清空） */
  setSource(id: number, url: string): Promise<void>;
  /** 用系统浏览器打开链接（只放行 http/https） */
  openUrl(url: string): Promise<void>;
  /** 手工标签 + 使用数 */
  tags(): Promise<TagCount[]>;
  /** 自动标签 + 使用数（后端按列聚合，不读前端当前页） */
  autoTags(): Promise<TagCount[]>;
  /** 单个素材的自动标签（已套用「手工优先」），带推导理由 */
  autoTagsFor(a: Asset, manual: string[]): Promise<AutoTag[]>;
  /** 一批素材各自的手工标签 */
  assetTags(ids: number[]): Promise<[number, string[]][]>;
  /** 批量打标签，返回真正新增条数 */
  /** 建一个空标签（还没有素材也会出现在侧栏） */
  tagCreate(name: string): Promise<void>;
  tagAssets(ids: number[], tag: string): Promise<number>;
  /** 批量去标签 */
  untagAssets(ids: number[], tag: string): Promise<number>;
  /** 从所有素材移除某标签 */
  deleteTag(tag: string): Promise<number>;
  /** 标签组 + 每组素材数/标签数 */
  tagGroups(): Promise<TagGroup[]>;
  /** 改标签颜色（null = 恢复默认） */
  setTagColor(tag: string, color: string | null): Promise<void>;
  /** 把标签放进分组（null = 未分组）；分组不存在会自动建 */
  setTagGroup(tag: string, group: string | null): Promise<void>;
  /** 标签改名；新名已存在时合并，返回合并过去的素材数 */
  renameTag(oldName: string, newName: string): Promise<number>;
  newGroup(name: string): Promise<void>;
  renameGroup(oldName: string, newName: string): Promise<void>;
  setGroupColor(name: string, color: string | null): Promise<void>;
  /** 解散分组（标签回到未分组，不删标签） */
  deleteGroup(name: string): Promise<number>;
  /** 后台移动（跨盘「复制→删源」不再卡住界面）：立刻返回，
   *  进度走 `move://progress`，结束走 `move://done` */
  moveStart(paths: string[], target: string, policy?: MovePolicy): Promise<void>;
  remove(paths: string[]): Promise<number>;
  undo(): Promise<string>;
  /** 缩略图缓存占用：[字节, 文件数] */
  cacheStat(): Promise<[number, number]>;
  /** 按 LRU 淘汰缓存；清理全部传 (0, 0)。返回 [淘汰前总量, 释放字节] */
  cacheGc(limit: number, target: number): Promise<[number, number]>;
  /** 索引与缓存所在的数据目录（设置 → 关于 里显示，不让用户去猜） */
  dataDir(): Promise<string>;
  /** 重建索引：清索引 + 全量重扫所有库根（进度走 scan://progress） */
  rebuildIndex(): Promise<void>;
  /** 导出**这一个库**的用户数据：`<库名>-userdata-<时间戳>.D-AssetManager`，返回落盘路径。
   *  `path` 不传 = 当前库。 */
  exportUserdata(path?: string | null): Promise<string>;
  /** 原生文件夹选择器；取消返回 null */
  pickFolder(title?: string): Promise<string | null>;
  /** 数据文件夹（索引 + 缩略图缓存）放哪：默认目录 / 自定义目录清单 */
  dataRoot(): Promise<DataRootInfo>;
  /** 看一眼目录里有什么（判断用户是不是把素材目录当成数据文件夹了） */
  probeDir(dir: string): Promise<DirProbe>;

  /* ---------------- 贴图 / 截图 / 剪贴板（0.10.0） ---------------- */
  /** 复制原文件（CF_HDROP） */
  peekCopyFile(paths: string[]): Promise<void>;
  /** 贴图不透明度（Ctrl+滚轮），写进存档 */
  peekOpacity(v: number): Promise<void>;
  /** 贴图换图后把存档指向新文件 */
  peekTrack(path: string, gid: number): Promise<void>;
  /** 「蓝色边框」全局开关：后端存状态并广播 `peek:frame` 给所有贴图窗口 */
  peekSetFrame(on: boolean): Promise<void>;
  /** 文本进剪贴板（复制显示尺寸这种） */
  clipText(text: string): Promise<void>;
  /** 另存为，返回最终路径（取消 = null） */
  saveAs(src: string, suggest?: string): Promise<string | null>;
  /** 贴图右键「以原始尺寸存到截图库」：原样复制进 screenshots，返回落地路径 */
  peekSaveLib(path: string): Promise<string>;
  /** 把当前主题告诉后端：贴图窗口建窗时要用它决定右键菜单的深浅 */
  setUiTheme(theme: "light" | "dark"): Promise<void>;
  /** F3：剪贴板图片贴到屏幕（进历史，最多 10 张） */
  clipPeek(): Promise<void>;
  /** 剪贴板历史：`[路径, 宽, 高]`，最新在前 */
  clipHistory(): Promise<[string, number, number][]>;
  /** F1：开始截图（永远进截图界面，不自动交付任何东西） */
  snipStart(): Promise<void>;
  /** 裁出选区（CSS 像素）→ `[dataURL, 物理宽, 物理高]`，给 canvas 当底图 */
  snipCrop(
    x: number,
    y: number,
    w: number,
    h: number,
    scale: number,
  ): Promise<[string, number, number]>;
  /** 交付合成好的整张图（含标注）。action: copy | save | saveas | peek */
  /** `from` 只进日志（哪个入口触发的交付），用来追"自动复制"这类问题 */
  snipFinish(
    png: string,
    action: "copy" | "save" | "saveas" | "peek",
    from?: string,
    /** 贴图落点：选区在虚拟桌面上的物理 `[x, y, w, h]`（同尺寸同位置） */
    place?: [number, number, number, number] | null,
  ): Promise<string | null>;
  snipCancel(): Promise<void>;
  /** 覆盖层底图上屏后调它，窗口这时才显示（先显示会黑闪一下） */
  snipReady(): Promise<void>;
  /** 整张截图的 data URL（同源，可 getImageData：放大镜 / 马赛克 / 导出都要用） */
  snipFull(): Promise<string>;
  /** 光标按像素微调（截图时 W/A/S/D 用） */
  cursorStep(dx: number, dy: number): Promise<void>;
  /** 光标底下的顶层窗口矩形（CSS 像素，相对覆盖层）；桌面/找不到 = null */
  snipWindowAt(
    x: number,
    y: number,
    scale: number,
  ): Promise<{ x: number; y: number; w: number; h: number } | null>;
  /** 注册系统级快捷键，返回注册失败的组合 */
  hotkeysSet(list: [string, string][]): Promise<string[]>;

  /* ---------------- 贴图窗口（0.9.0） ---------------- */
  /** 开一扇无边框窗口，`ids` 是整个浏览集合，`index` 是当前那张 */
  peekOpen(ids: number[], index: number, longEdge?: number): Promise<void>;
  /** 缩放窗口本身（连窗口一起缩放，锚定中心） */
  peekZoom(step: number): Promise<void>;
  /** 按新图比例改窗口尺寸（长边不变） */
  peekRatio(w: number, h: number): Promise<void>;
  peekClose(): Promise<void>;
  /** 开一块灵感板：该标签下所有素材铺到无限画布上。返回张数（0 = 已有那块被聚焦） */
  boardOpen(tag: string, title?: string): Promise<number>;
  boardClose(): Promise<void>;
  /**
   * 灵感板里 Ctrl+V：剪贴板图片存进**程序目录\screenshots**、入库、打上这块板的标签，
   * 返回刚入库的那张（前端直接贴到视口中心）。
   */
  boardPaste(tag: string): Promise<BoardPastedItem>;

  /* ---------------- 分组（0.2.x） ----------------
     分组 = 用户点名要看的那一批素材（跨库）。存的是素材 id，**不搬文件**；
     删分组只删"这一撮"的名单，素材一个都不动。 */
  collList(): Promise<CollRow[]>;
  collCreate(name: string): Promise<number>;
  collRename(id: number, name: string): Promise<void>;
  collDelete(id: number): Promise<void>;
  /** 把一批素材加进分组，返回真正新增的条数 */
  collAdd(id: number, ids: number[]): Promise<number>;
  /** 把**整个标签**下的素材加进分组（只建关联，原文件不动）。返回真正新增的条数 */
  collAddTag(id: number, tag: string): Promise<number>;
  collRemove(id: number, ids: number[]): Promise<number>;
  /** 这张素材在哪些分组里 */
  collOf(gid: number): Promise<number[]>;
  /** 为这个分组开一块灵感板（与快照板同一套画布）。返回上板张数 */
  collOpen(id: number): Promise<number>;
  /** 分组板里 Ctrl+V：剪贴板图片存盘入库后加进这个分组，返回刚入库的那张 */
  collPaste(id: number): Promise<BoardPastedItem>;

  /* ---------------- 快照（0.12.0） ----------------
     快照记的是**图片的内容指纹**（不是 id、不是路径），所以图片改名、换目录、
     换到另一个素材库之后，导回来照样能对上同一张图。导出的是清单，不含图片字节。 */
  snapshotList(): Promise<SnapshotRow[]>;
  /** 新建（同名直接返回已有的那个）。返回 id */
  snapshotCreate(name: string): Promise<number>;
  snapshotRename(id: number, name: string): Promise<void>;
  snapshotDelete(id: number): Promise<void>;
  /** 把一批素材加进快照 */
  snapshotAdd(id: number, ids: number[]): Promise<SnapshotAdd>;
  /** 把某个标签下的全部图片加进快照（文件不动） */
  snapshotAddTag(id: number, tag: string): Promise<SnapshotAdd>;
  /** 对账：按内容哈希在当前所有库里重新找一遍 */
  snapshotResolve(id: number): Promise<SnapshotResolve>;
  snapshotRemove(id: number, hashes: string[]): Promise<number>;
  /** 导出清单（.S-AssetManager）。返回最终路径，取消 = null */
  snapshotExport(id: number): Promise<string | null>;
  /** 导入清单（选文件对话框那版）。返回 `[id, 最终名字, 新增条数, 跳过条数]`，取消 = null */
  snapshotImport(): Promise<[number, string, number, number] | null>;
  /** 导出原始图片（.P-AssetManager）。取消 = null */
  snapshotExportZip(id: number): Promise<SnapshotZip | null>;
  /**
   * 双击 / 命令行传进来的导出文件：后端按**内容**判类型再导入。
   * `.P` 包会先让用户选解压目录 —— 那一步取消返回 null（不算错误）。
   */
  openImportFile(path: string): Promise<ImportReport | null>;
  /**
   * 把一批**任意格式**的文件复制进 `target`（必须是库里的目录）并入库。
   * `target` 后端会再校验一次，传库外路径会被拒绝。
   */
  importFiles(paths: string[], target: string): Promise<ImportFilesReport>;
  /** 选文件（不限格式）→ 复制进 `target`。取消 = null */
  pickAndImport(target: string): Promise<ImportFilesReport | null>;
  /** 导出资产包（**整个当前素材库**）。`path` 用来定位库，取消 = null */
  exportPackage(path?: string | null): Promise<PackageOut | null>;
  /**
   * 整库导成**普通 zip**（原图按库内相对路径 + `_AssetManager/` 里的软件数据），
   * 文件名带年月日；超过单包上限自动分片。`path` 用来定位库，取消 = null。
   */
  exportLibraryZip(path?: string | null): Promise<LibraryZipOut | null>;
  /** 开一块快照灵感板 */
  snapshotOpen(id: number): Promise<number>;
  /** 快照板里 Ctrl+V：存截图目录 + 入库 + 加进这个快照 */
  snapshotPaste(id: number): Promise<BoardPastedItem>;
  /**
   * 合并 / 差集（P2.9）：把几份快照合成**一份新快照**，原件不动。
   * 返回 `[新快照 id, 条数, 其中新写入的行数]`；重名自动加 ` (2)`。
   */
  snapshotCombine(
    ids: number[],
    name: string,
    mode: "union" | "diff",
  ): Promise<[number, number, number]>;
  /** 灵感板摆位：读上次摆的位置/尺寸/组（`[gid,x,y,w,h,grp]`）。读不到就空 */
  boardLayout(board: string): Promise<[number, number, number, number, number, number][]>;
  /**
   * 往板上**拖文件进来**：只收已入库的（不在库里的如实报原因，不静默丢）。
   * 返回 `[能上板的条目, 不能上板的原因]`
   */
  boardAddPaths(paths: string[]): Promise<[BoardItemDto[], string[]]>;
  /** 灵感板摆位：记住这几张（松手时逐张写） */
  boardPlace(
    board: string,
    rows: [number, number, number, number, number][],
  ): Promise<number>;
  /** 灵感板便签：读这块板上所有文字注释（含字体外观与组号） */
  boardNotes(board: string): Promise<BoardNote[]>;
  /** 灵感板便签：新建（`nid = null`）或更新一张，返回 nid。`look = null` 保留原字体外观 */
  boardNotePut(
    board: string,
    nid: number | null,
    x: number,
    y: number,
    w: number,
    h: number,
    text: string,
    look: { font: string; fs: number; color: string; bold: boolean } | null,
  ): Promise<number>;
  /** 灵感板便签：删一张 */
  boardNoteDel(board: string, nid: number): Promise<number>;
  /** 灵感板打组 / 解组：`grp = -1` 新建（后端发号并返回），`0` 解组 */
  boardGroup(board: string, keys: number[], grp: number): Promise<number>;
  peekTop(on: boolean): Promise<boolean>;
  /** 图已经画上去了 → 才让后端把窗口显示出来（否则建窗时会闪一下默认底色） */
  peekReady(): Promise<void>;

  /* ---------------- T24 主色板 / T25 查重 / T26 引用 / T33 批量重命名 ---------------- */
  /** 按一批全局 id 取素材（查重/引用面板要显示缩略图） */
  assetsByIds(ids: number[]): Promise<Asset[]>;
  /** 各色桶的素材数（颜色筛选的口径） */
  colorCounts(): Promise<[string, number][]>;
  /** `[缺主色板, 缺全文件哈希]` */
  metaMissing(): Promise<[number, number]>;
  /** 补算主色板：返回 `[本次补了, 还剩]` */
  fillPalette(limit?: number): Promise<[number, number]>;
  /** 精确重复：`[重复组, 还剩多少文件没算哈希]` */
  dupesExact(limit?: number): Promise<[DupGroup[], number]>;
  dupesSimilar(maxDist?: number, limit?: number): Promise<SimilarPair[]>;
  /** 扫一批工程文件的引用：`[扫了几个, 记下几条]` */
  refsScan(limit?: number): Promise<[number, number]>;
  /** `[引用总数, 丢失条数]` */
  refsSummary(): Promise<[number, number]>;
  brokenRefs(limit?: number): Promise<RefItem[]>;
  refsOf(id: number): Promise<RefItem[]>;
  refsUsing(id: number): Promise<RefItem[]>;
  renamePlan(ids: number[], rule: RenameRule): Promise<RenameItem[]>;
  renameApply(items: [number, string][]): Promise<RenameResult>;
  tagGetRule(tag: string): Promise<AutoRule | null>;
  tagSetRule(tag: string, rule: AutoRule | null): Promise<void>;
  /** 每个库的独立 DB 文件：`[库根, 文件路径, 字节]` */
  libFiles(): Promise<[string, string, number][]>;
  /** 缩放另存（T38）。maxEdge=0 表示只转格式。取消返回 null */
  resizeSave(path: string, maxEdge: number, format?: string): Promise<ResizeOut | null>;
  /** 读 EXIF 拍摄信息（T41）；读不到返回 null */
  exif(path: string): Promise<ExifInfo | null>;
  /** 写配置。**下次启动生效**；`copyIndex` 为真时顺带把索引复制过去。
   *  `force` 为真 = 用户已经看过「这看起来是素材目录」的提示，仍要用它当数据位置 */
  setDataRoot(input: {
    enabled: boolean;
    current: string | null;
    dirs: string[];
    copyIndex: boolean;
    force?: boolean;
  }): Promise<string>;
  /** 重启软件（换数据文件夹后立刻生效） */
  relaunch(): Promise<void>;
  /** 打开系统回收站 */
  openRecycle(): Promise<void>;
  /* 应用内回收站（0.1.6）：删除=搬进 <data>/trash，下面四个管列表与清账 */
  trashList(): Promise<TrashEntry[]>;
  /** 回收站条目的预览地址（顺序与入参一致；`null` = 文件已经不在了）。
   *  只认账本里的 id —— 回收站里的文件已经不是素材，`thumb://` 取不到。 */
  trashMedia(ids: string[]): Promise<(string | null)[]>;
  /* ---- 界面偏好 / 缩放 / 自启（0.3.30） ----
     缩放和托盘这几项必须让**后端也存一份**：启动时最小化、关闭拦截都发生在
     webview 起来之前，后端读不到 localStorage（见 main.rs 的 UiPrefs 注释）。 */
  /** 把「关闭到托盘 / 启动时最小化 / 缩放」同步给后端 */
  uiPrefsSet(p: { closeToTray: boolean; startMinimized: boolean; uiScale: number | null }): Promise<void>;
  /** 设缩放；传 null = 自动。返回**实际生效**的倍率（前端拿它显示"当前 100%"） */
  setUiScale(scale: number | null): Promise<number>;
  setAutostart(on: boolean): Promise<void>;
  getAutostart(): Promise<boolean>;
  /** 还原（`to` 给了就还原到那个目录，不给回原位置） */
  trashRestore(ids: string[], to?: string): Promise<number>;
  trashPurge(ids: string[]): Promise<number>;
  trashEmpty(): Promise<number>;

  /* ---------------- 检查更新（0.1.35） ----------------
     三条边界与后端 update.rs 顶部一致：检查只读、下载必须过哈希、安装只给
     「按用户安装」的 NSIS 版。前端只负责把用户点的那一下串起来。 */
  /** 查最新版本。`force` = 用户手点的，跳过后端 10 分钟进程内缓存 */
  updateCheck(force?: boolean): Promise<UpdateInfo>;
  /**
   * 下载安装包到本地缓存目录，返回落盘路径；sha256 对不上后端会删掉并抛错。
   * `assetId` 是 GitHub Asset 的数字 id（直链限流时的兜底通道）。
   */
  updateDownload(url: string, name: string, sha256: string, assetId: number): Promise<string>;
  /** 校验 + 静默安装并重启；`version` 只用于回显"正在装的是哪一版" */
  updateInstall(path: string, sha256: string, version?: string): Promise<void>;
}

/** 用户偏好：所有改动即时生效（与 FontManager 同一纪律，没有「保存」按钮），落在 localStorage */
export interface Prefs {
  /** 默认缩略图档位 0/1/2 = 小/中/大 */
  size: number;
  /** 默认视图 */
  view: "grid" | "waterfall";
  /** 悬停时是否显示文件名 */
  showTileName: boolean;
  /** 是否在详情里显示自动标签（按原始文件的格式/类型/像素/比例/体积推导） */
  autoTags: boolean;
  /** 缓存上限（MB） */
  cacheLimitMB: number;
  /** 主题：默认深色（与 FontManager 出厂一致），浅色可选 */
  /** "system" = 跟随系统（默认）；其余为强制深浅 */
  theme: "system" | "dark" | "light";
  /** 动效：system = 跟随系统；reduced = 减少动态效果（全站时长压到 1ms） */
  motion: "system" | "reduced";
  /**
   * 后台把全库缩略图预生成一遍（低优先、可停）。
   * 开了之后第一次浏览任何目录都不用等出图；代价是空闲时占一点 CPU 和磁盘。
   */
  warmThumbs: boolean;
  /**
   * 自动检查更新（0.1.35，照 FontManager）。
   *
   * 关掉之后**连版本号都不查**：不发任何网络请求。开着时只在启动后静默问一次，
   * 有新版本才在「关于」页与顶栏提示，绝不擅自下载。
   */
  updateCheck: boolean;
  /** 用户点过「跳过这个版本」的版本号（空串 = 没跳过任何版本） */
  updateSkipped: string;
  /**
   * 上次**检查**更新的时间戳（毫秒，0 = 从没查过）。
   * 24 小时闸门靠它判「到点了没有」：GitHub 匿名接口 60 次/小时/IP，
   * 多开几个实例 + 反复点「立即检查」很容易被限流。
   */
  updateLastCheck: number;
  /**
   * 界面缩放（0.3.30）：`null` = 自动（按显示器工作区挑档）。
   * 存的是真实倍率（0.8 / 0.9 / 1 / 1.1 / 1.25），不是档位索引 ——
   * 后端 `set_zoom` 直接吃这个值，中间少一层映射就少一处对不上。
   */
  uiScale: number | null;
  /** 关闭窗口 = 收进托盘（不退出） */
  closeToTray: boolean;
  /** 启动后直接收进托盘 */
  startMinimized: boolean;
  /**
   * 界面语言：`system` = 跟随系统（默认）；其余是具体语言代码。
   * 类型放宽成 string 是为了**老数据/新语言**都不炸：真正的白名单校验在 i18n 里。
   */
  locale: string;
}

export const DEFAULT_PREFS: Prefs = {
  size: 1,
  view: "grid",
  showTileName: true,
  autoTags: true,
  cacheLimitMB: 2048,
  theme: "system",
  motion: "system",
  warmThumbs: true,
  updateCheck: true,
  updateSkipped: "",
  updateLastCheck: 0,
  uiScale: null,
  /* 「关闭到托盘」「启动时最小化」默认都**关**：托盘是新增能力，
     默认开启会让人以为"关不掉软件"（FontManager 也是默认关）。 */
  closeToTray: false,
  startMinimized: false,
  locale: "system",
};

/* ---------------- 检查更新（0.1.35） ---------------- */

/** 检查更新的结果（后端 `am_update_check`，字段名是 camelCase） */
export interface UpdateInfo {
  /** 当前运行的版本 */
  current: string;
  /** 远端最新版本 */
  latest: string;
  hasUpdate: boolean;
  /** 发布页地址 */
  url: string;
  /** 发布说明（已截断到 4000 字符） */
  notes: string;
  publishedAt: string;
  /** github = Releases API；raw / cdn = 只能读到版本号的兜底源 */
  source: string;
  setupUrl: string;
  setupName: string;
  sha256: string;
  setupAssetId: number;
  /** 能不能一键静默安装（直链 + 校验值齐全 + 当前是按用户安装） */
  canAutoInstall: boolean;
  /** user / system / portable */
  installKind: string;
  /** 装不了时是哪一条没过：system / portable / no-asset / no-digest（空 = 没阻塞） */
  autoInstallBlocked: string;
}

/** 安装包下载进度（`update:progress` 事件） */
export interface UpdateProgress {
  phase: "download" | "verify" | "ready" | "error";
  received: number;
  total: number;
  percent: number;
  error?: string | null;
}

const PREFS_KEY = "am.prefs.v1";

export function loadPrefs(): Prefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return { ...DEFAULT_PREFS };
    const p = JSON.parse(raw) as Partial<Prefs>;
    return {
      size: typeof p.size === "number" && p.size >= 0 && p.size <= 2 ? p.size : DEFAULT_PREFS.size,
      view: p.view === "waterfall" ? "waterfall" : "grid",
      showTileName: p.showTileName !== false,
      autoTags: p.autoTags !== false,
      cacheLimitMB:
        typeof p.cacheLimitMB === "number" && p.cacheLimitMB > 0
          ? p.cacheLimitMB
          : DEFAULT_PREFS.cacheLimitMB,
      theme: p.theme === "light" || p.theme === "dark" ? p.theme : "system",
      motion: p.motion === "reduced" ? "reduced" : "system",
      warmThumbs: p.warmThumbs !== false,
      updateCheck: p.updateCheck !== false,
      updateSkipped: typeof p.updateSkipped === "string" ? p.updateSkipped : "",
      /* 脏值（字符串 / NaN / 负数 / undefined）一律当「没查过」：
         宁可多查一次，也不要因为一个坏值把提醒永久关掉（与 updateGate 同一口径） */
      updateLastCheck:
        typeof p.updateLastCheck === "number" && Number.isFinite(p.updateLastCheck) && p.updateLastCheck >= 0
          ? p.updateLastCheck
          : 0,
      /* 缩放只接受合理区间：存坏了（0 / 负数 / 字符串）就当自动，
         而不是把界面缩成看不见的一团 */
      uiScale:
        typeof p.uiScale === "number" && p.uiScale >= 0.5 && p.uiScale <= 2
          ? p.uiScale
          : null,
      closeToTray: p.closeToTray === true,
      startMinimized: p.startMinimized === true,
      locale: typeof p.locale === "string" && p.locale ? p.locale : "system",
    };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export function savePrefs(p: Prefs): void {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(p));
  } catch {
    /* 隐私模式写不进去就当本次会话有效 */
  }
}

export const KIND_IMAGE = 0;
export const THUMB_FAILED = 2;
export const THUMB_UNSUPPORTED = 3;

export const KIND_LABEL: Record<number, string> = {
  0: "图片",
  1: "视频",
  2: "音频",
  3: "3D",
  4: "文档",
  5: "其他",
};

export function badge(ext: string): string {
  if (!ext) return "FILE";
  let s = ext.toUpperCase();
  if (s.length > 4) s = s.slice(0, 4);
  if (s === "JPEG") s = "JPG";
  return s;
}

export function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1048576).toFixed(1)} MB`;
  return `${(n / 1073741824).toFixed(2)} GB`;
}

export function fmtNum(n: number): string {
  if (n < 10000) return String(n);
  if (n < 100000000) return `${(n / 10000).toFixed(1)} 万`;
  return `${(n / 100000000).toFixed(2)} 亿`;
}

export function fmtTime(ts: number): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  const p = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 回收站条目（后端 core::trash::TrashEntry，serde camelCase） */
export interface TrashEntry {
  id: string;
  originalPath: string;
  trashedPath: string;
  name: string;
  size: number;
  deletedAt: number;
}

/** 缩放另存的结果 */
export interface ResizeOut {
  path: string;
  width: number;
  height: number;
  bytes: number;
}

/** 拍摄信息（EXIF）。字段可能缺，缺了界面就不显示那一行 */
export interface ExifInfo {
  make: string;
  model: string;
  lens: string;
  fNumber: number | null;
  exposure: string | null;
  iso: number | null;
  focal: number | null;
  taken: string | null;
  software: string;
}
