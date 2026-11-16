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
  /** 从素材库移除一个库根：只清索引，磁盘文件不动 */
  removeLib(path: string): Promise<number>;
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
  /** 导出 userdata-<ts>.json，返回落盘路径 */
  exportUserdata(): Promise<string>;
  /** 原生文件夹选择器；取消返回 null */
  pickFolder(title?: string): Promise<string | null>;
  /** 数据文件夹（索引 + 缩略图缓存）放哪：默认目录 / 自定义目录清单 */
  dataRoot(): Promise<DataRootInfo>;
  /** 看一眼目录里有什么（判断用户是不是把素材目录当成数据文件夹了） */
  probeDir(dir: string): Promise<DirProbe>;

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
  theme: "dark" | "light";
  /** 动效：system = 跟随系统；reduced = 减少动态效果（全站时长压到 1ms） */
  motion: "system" | "reduced";
  /**
   * 后台把全库缩略图预生成一遍（低优先、可停）。
   * 开了之后第一次浏览任何目录都不用等出图；代价是空闲时占一点 CPU 和磁盘。
   */
  warmThumbs: boolean;
}

export const DEFAULT_PREFS: Prefs = {
  size: 1,
  view: "grid",
  showTileName: true,
  autoTags: true,
  cacheLimitMB: 2048,
  theme: "dark",
  motion: "system",
  warmThumbs: true,
};

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
      theme: p.theme === "light" ? "light" : "dark",
      motion: p.motion === "reduced" ? "reduced" : "system",
      warmThumbs: p.warmThumbs !== false,
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
