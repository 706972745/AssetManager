import type { Asset, DirNode, Query, Source } from "../types";

/** 浏览器预览用的假数据源：形状与桌面端一致，方便 UI 单独开发。 */

const FOLDERS = [
  "素材库",
  "素材库/石材贴图",
  "素材库/木材与织物",
  "素材库/金属与玻璃",
  "素材库/灯光与影棚",
  "素材库/旧项目",
  "素材库/旧项目/2024 归档",
  "素材库/待整理",
];

const EXT_POOL = [
  ["png", 0],
  ["jpg", 0],
  ["webp", 0],
  ["tga", 0],
  ["exr", 0],
  ["psd", 0],
  ["blend", 3],
  ["glb", 3],
  ["mp4", 1],
  ["pdf", 4],
] as const;

const ASPECTS: [number, number][] = [
  [16, 9],
  [4, 3],
  [1, 1],
  [3, 4],
  [9, 16],
  [2, 1],
  [5, 4],
];

const STEMS = [
  "石材贴图",
  "木纹板",
  "拉丝金属",
  "水泥墙",
  "布料",
  "皮革",
  "砖块",
  "玻璃幕墙",
  "沙地",
  "苔藓",
  "锈蚀",
  "瓷砖",
  "天空 HDRI",
  "影棚灯光",
  "展台",
  "海报草稿",
];

/** 确定性伪随机，保证刷新后布局一致。 */
function rnd(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 4294967296;
  };
}

function buildTree(): { nodes: DirNode[]; pathToId: Map<string, number> } {
  const pathToId = new Map<string, number>();
  const nodes: DirNode[] = [];
  FOLDERS.forEach((p, i) => {
    const id = i + 1;
    pathToId.set(p, id);
    const name = p.split("/").pop()!;
    const parentPath = p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : null;
    nodes.push({
      id,
      path: p,
      name,
      parentId: parentPath ? pathToId.get(parentPath)! : null,
      childCnt: 0,
      assetCnt: 0,
      hasChildren: false,
    });
  });
  for (const n of nodes) {
    n.childCnt = nodes.filter((x) => x.parentId === n.id).length;
    n.hasChildren = n.childCnt > 0;
  }
  return { nodes, pathToId };
}

function buildAssets(nodes: DirNode[], pathToId: Map<string, number>): Asset[] {
  const r = rnd(20261115);
  const out: Asset[] = [];
  const now = Math.floor(Date.now() / 1000);
  for (let i = 0; i < 2600; i++) {
    const folder = FOLDERS[Math.floor(r() * FOLDERS.length)];
    const dirId = pathToId.get(folder)!;
    const [ext, kind] = EXT_POOL[Math.floor(r() * EXT_POOL.length)];
    const [aw, ah] = ASPECTS[Math.floor(r() * ASPECTS.length)];
    const scale = 200 + Math.floor(r() * 1800);
    const stem = STEMS[Math.floor(r() * STEMS.length)];
    const hasDim = ext !== "pdf";
    out.push({
      id: i + 1,
      dirId,
      name: `${stem}_${String(i + 1).padStart(4, "0")}.${ext}`,
      ext,
      kind,
      size: Math.floor(80 * 1024 + r() * 9 * 1024 * 1024),
      mtime: now - Math.floor(r() * 60 * 86400),
      // 假数据给一个"入库时间"：比 mtime 稍晚一点，界面上的「添加日期」才说得通
      addedAt: now - Math.floor(r() * 30 * 86400),
      width: hasDim ? Math.round(scale * (aw / Math.max(aw, ah))) : null,
      height: hasDim ? Math.round(scale * (ah / Math.max(aw, ah))) : null,
      thumbState: 0,
      quality: 1,
      path: `D:/${folder}/${stem}_${String(i + 1).padStart(4, "0")}.${ext}`,
      rating: i % 7 === 0 ? 4 : 0,
      note: "",
      sourceUrl: "",
      // 浏览器里调界面用的假数据：色板按图片 id 生成几个稳定的色值（真机由 Rust 算）
      palette: kind === 0 ? ["c8503c", "2f6f8f", "e0d6c2", "1b1b18"].join(",") : "",
      color: kind === 0 ? ["red", "blue", "amber", "gray"][i % 4] : "",
    });
  }
  for (const n of nodes) {
    n.assetCnt = out.filter((a) => a.dirId === n.id).length;
  }
  return out;
}

const tree = buildTree();
let assets = buildAssets(tree.nodes, tree.pathToId);

function svgThumb(a: Asset, px: number): string {
  const w = a.width ?? 1200;
  const h = a.height ?? 900;
  const hue = (a.id * 37) % 360;
  const hue2 = (hue + 40) % 360;
  const label = a.name.replace(/\.[^.]+$/, "");
  const fs = Math.max(14, Math.round(px / 14));
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">` +
    `<stop offset="0" stop-color="hsl(${hue} 34% 62%)"/>` +
    `<stop offset="1" stop-color="hsl(${hue2} 28% 34%)"/></linearGradient></defs>` +
    `<rect width="${w}" height="${h}" fill="url(#g)"/>` +
    `<circle cx="${w * 0.72}" cy="${h * 0.28}" r="${Math.min(w, h) * 0.18}" fill="hsl(${hue2} 40% 78%)" opacity="0.5"/>` +
    `<rect x="0" y="${h * 0.68}" width="${w}" height="${h * 0.32}" fill="hsl(${hue} 20% 20%)" opacity="0.35"/>` +
    `<text x="${w * 0.5}" y="${h * 0.55}" font-family="monospace" font-size="${fs * 1.6}" fill="rgba(255,255,255,.82)" text-anchor="middle">${label.slice(-12)}</text>` +
    `</svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

function norm(a: Asset): Asset {
  return a;
}

export const mockSource: Source = {
  desktop: false,

  async roots() {
    return tree.nodes.filter((n) => n.parentId === null);
  },

  async libCount() {
    return tree.nodes.filter((n) => n.parentId === null).length;
  },

  /* 浏览器预览里没有"软件自己的截图目录"，内置库为空 */
  async builtinRoots() {
    return [];
  },

  async removeLib() {
    return 0;
  },

  async setCurrentLib() {
    /* 浏览器预览只有一个假库，切库是空操作 */
    return false;
  },

  async children(parent) {
    return tree.nodes.filter((n) => n.parentId === parent);
  },

  async list(q) {
    let rows = assets;
    if (q.dirId != null) {
      if (q.recursive) {
        const base = tree.nodes.find((n) => n.id === q.dirId)?.path ?? "";
        const ids = new Set(tree.nodes.filter((n) => n.path.startsWith(base)).map((n) => n.id));
        rows = rows.filter((a) => ids.has(a.dirId));
      } else {
        rows = rows.filter((a) => a.dirId === q.dirId);
      }
    }
    if (q.kinds && q.kinds.length) rows = rows.filter((a) => q.kinds!.includes(a.kind));
    if (q.text.trim()) {
      const t = q.text.trim().toLowerCase();
      rows = rows.filter((a) => a.name.toLowerCase().includes(t) || a.path.toLowerCase().includes(t));
    }
    const sorted = [...rows].sort((a, b) => {
      if (q.sort === "name_asc") return a.name.localeCompare(b.name, "zh");
      if (q.sort === "size_desc") return b.size - a.size;
      return b.mtime - a.mtime;
    });
    let start = 0;
    if (q.cursor) {
      if (q.sort === "name_asc") start = q.cursor[0];
      else {
        const idx = sorted.findIndex((a) =>
          q.sort === "size_desc" ? a.size === q.cursor![0] && a.id === q.cursor![1] : a.mtime === q.cursor![0] && a.id === q.cursor![1]
        );
        start = idx >= 0 ? idx + 1 : 0;
      }
    }
    return sorted.slice(start, start + q.limit).map(norm);
  },

  async total() {
    return assets.length;
  },

  thumb(a, px) {
    return svgThumb(a, px);
  },

  async scanStart() {
    throw new Error("浏览器预览模式不支持扫描，请用桌面端");
  },
  async scanCancel() {
    return true;
  },
  async movePlan(paths) {
    return { files: paths.length, bytes: 0, conflicts: [], crossVolume: false, dirs: [] };
  },
  async statPaths(paths) {
    return { files: paths.length, bytes: 0 };
  },
  async dragOut() {
    throw new Error("浏览器预览模式不支持拖出，请用桌面端");
  },
  async watch() {
    return 0;
  },
  async rebuildIndex() {
    throw new Error("浏览器预览模式不支持重建索引，请用桌面端");
  },
  async exportUserdata() {
    throw new Error("浏览器预览模式不支持导出，请用桌面端");
  },
  async pickFolder() {
    return null;
  },
  async prefetchThumbs() {
    /* 浏览器预览没有真缩略图管线，什么都不用做 */
  },
  async warmThumbs() {
    return { running: false, done: 0, total: 0 };
  },
  async warmStop() {
    return { running: false, done: 0, total: 0 };
  },
  async warmStatus() {
    return { running: false, done: 0, total: 0 };
  },
  async dataRoot() {
    // 浏览器预览里没有真实数据目录，给一组能展示布局的假值
    return {
      defaultDir: "%LOCALAPPDATA%\\AssetManager",
      effectiveDir: "%LOCALAPPDATA%\\AssetManager",
      enabled: false,
      current: null,
      dirs: [],
      max: 3,
    };
  },
  async probeDir() {
    // 浏览器里没有真实文件系统，返回「空目录」即可（不会触发误选提示）
    return { images: 0, videos: 0, entries: 0, hasIndexDb: false };
  },
  async setDataRoot() {
    throw new Error("浏览器预览模式不支持切换数据文件夹，请用桌面端");
  },
  async relaunch() {
    throw new Error("浏览器预览模式不支持重启，请用桌面端");
  },
  async openRecycle() {
    throw new Error("浏览器预览模式不支持打开回收站");
  },
  async trashList() {
    return [];
  },
  async trashRestore() {
    return 0;
  },
  async trashPurge() {
    return 0;
  },
  async trashEmpty() {
    return 0;
  },
  async reveal() {
    throw new Error("浏览器预览模式不支持定位文件");
  },
  async openDefault() {
    throw new Error("浏览器预览模式不支持打开原文件，请用桌面端");
  },
  async copyFile() {
    throw new Error("浏览器预览模式不支持复制原文件，请用桌面端");
  },
  async rename(_p, name) {
    const a = assets.find((x) => x.path === _p);
    if (!a) throw new Error("找不到这个文件");
    const dir = a.path.slice(0, a.path.lastIndexOf("/"));
    a.name = name;
    a.path = `${dir}/${name}`;
    assets = [...assets];
    return a.path;
  },
  async mkdir() {
    throw new Error("浏览器预览模式不支持建文件夹");
  },
  async move() {
    throw new Error("浏览器预览模式不支持移动文件");
  },
  async moveStart() {
    throw new Error("浏览器预览模式不支持移动文件");
  },
  async setRating(ids: number[], rating: number) {
    return ids.length && rating >= 0 ? ids.length : 0;
  },
  async setNote() {
    /* 浏览器预览里不落盘；真机走数据库 */
  },
  async setSource() {
    /* 同上 */
  },
  async openUrl(url: string) {
    window.open(url, "_blank");
  },
  async tags() {
    return [];
  },
  async autoTags() {
    return [];
  },
  async autoTagsFor() {
    return [];
  },
  async assetTags() {
    return [];
  },
  async tagCreate() {
    /* mock：新建空标签，列表本来就随 refreshKey 重算 */
  },
  async tagAssets() {
    throw new Error("浏览器预览模式不支持打标签");
  },
  async untagAssets() {
    throw new Error("浏览器预览模式不支持去标签");
  },
  async deleteTag() {
    throw new Error("浏览器预览模式不支持删除标签");
  },
  async tagGroups() {
    return [];
  },
  async setTagColor() {
    throw new Error("浏览器预览模式不支持改标签颜色");
  },
  async setTagGroup() {
    throw new Error("浏览器预览模式不支持分组");
  },
  async renameTag() {
    throw new Error("浏览器预览模式不支持重命名标签");
  },
  async newGroup() {
    throw new Error("浏览器预览模式不支持新建分组");
  },
  async renameGroup() {
    throw new Error("浏览器预览模式不支持重命名分组");
  },
  async setGroupColor() {
    throw new Error("浏览器预览模式不支持改分组颜色");
  },
  async deleteGroup() {
    throw new Error("浏览器预览模式不支持删除分组");
  },
  async remove() {
    throw new Error("浏览器预览模式不支持删除");
  },
  async undo() {
    throw new Error("浏览器预览模式没有可撤销的操作");
  },
  /* 浏览器预览没有真实缓存：给一组假数字，让设置面板的排版能看全 */
  async cacheStat() {
    return [186 * 1024 * 1024, 1240] as [number, number];
  },
  async cacheGc() {
    return [186 * 1024 * 1024, 0] as [number, number];
  },
  async dataDir() {
    return "%LOCALAPPDATA%\\AssetManager（浏览器预览为假路径）";
  },
  /* ---- T24/T25/T26/T33：浏览器预览下给空数据，界面排版照常能看 ---- */
  async peekCopyFile() {},
  async peekOpacity() {},
  async peekTrack() {},
  async peekSetFrame() {},
  async clipText(text: string) {
    await navigator.clipboard?.writeText(text);
  },
  async saveAs() {
    return null;
  },
  async peekSaveLib() {
    throw new Error("浏览器预览下不能写截图库，请用桌面版");
  },
  async setUiTheme() {},
  async clipPeek() {
    throw new Error("浏览器预览模式没有剪贴板贴图");
  },
  async clipHistory() {
    return [];
  },
  async snipStart() {
    throw new Error("浏览器预览模式没有截图");
  },
  async snipCrop() {
    throw new Error("浏览器预览模式没有截图");
  },
  async snipFinish() {
    return null;
  },
  async snipCancel() {},
  async snipReady() {},
  async snipFull() {
    throw new Error("浏览器预览模式没有截图");
  },
  async snipWindowAt() {
    return null;
  },
  async cursorStep() {},
  async hotkeysSet() {
    return [];
  },
  async peekOpen() {
    throw new Error("浏览器预览模式没有独立的无边框窗口");
  },
  async peekZoom() {},
  async peekRatio() {},
  async peekClose() {},
  async boardOpen() {
    throw new Error("浏览器预览模式没有灵感板");
  },
  async boardClose() {},
  async boardPaste() {
    throw new Error("浏览器预览模式没有灵感板");
  },
  async collList() {
    return [];
  },
  async collCreate() {
    return 0;
  },
  async collRename() {},
  async collDelete() {},
  async collAdd() {
    return 0;
  },
  async collAddTag() {
    return 0;
  },
  async collRemove() {
    return 0;
  },
  async collOf() {
    return [];
  },
  async collOpen() {
    throw new Error("浏览器预览模式没有灵感板");
  },
  async collPaste() {
    throw new Error("浏览器预览模式没有灵感板");
  },
  async snapshotList() {
    return [];
  },
  async snapshotCreate() {
    throw new Error("浏览器预览模式没有快照");
  },
  async snapshotRename() {},
  async snapshotDelete() {},
  async snapshotAdd() {
    throw new Error("浏览器预览模式没有快照");
  },
  async snapshotAddTag() {
    throw new Error("浏览器预览模式没有快照");
  },
  async snapshotCombine() {
    throw new Error("浏览器预览模式没有快照");
  },
  async boardLayout() {
    return [];
  },
  async boardPlace() {
    return 0;
  },
  async boardAddPaths() {
    return [[], ["浏览器预览模式没有素材库"]];
  },
  async snapshotResolve() {
    return { items: [], found: 0, missing: 0 };
  },
  async snapshotRemove() {
    return 0;
  },
  async snapshotExport() {
    return null;
  },
  async snapshotImport() {
    return null;
  },
  async snapshotExportZip() {
    return null;
  },
  async snapshotOpen() {
    throw new Error("浏览器预览模式没有灵感板");
  },
  async snapshotPaste() {
    throw new Error("浏览器预览模式没有灵感板");
  },
  async peekTop(on: boolean) {
    return on;
  },
  async peekReady() {},
  async assetsByIds(ids: number[]) {
    return assets.filter((a) => ids.includes(a.id));
  },
  async colorCounts() {
    const m = new Map<string, number>();
    for (const a of assets) if (a.color) m.set(a.color, (m.get(a.color) ?? 0) + 1);
    return [...m.entries()] as [string, number][];
  },
  async metaMissing() {
    return [0, 0] as [number, number];
  },
  async fillPalette() {
    return [0, 0] as [number, number];
  },
  async dupesExact() {
    return [[], 0] as [import("../types").DupGroup[], number];
  },
  async dupesSimilar() {
    return [];
  },
  async refsScan() {
    return [0, 0] as [number, number];
  },
  async refsSummary() {
    return [0, 0] as [number, number];
  },
  async brokenRefs() {
    return [];
  },
  async refsOf() {
    return [];
  },
  async refsUsing() {
    return [];
  },
  async renamePlan() {
    throw new Error("浏览器预览模式不支持批量重命名");
  },
  async renameApply() {
    throw new Error("浏览器预览模式不支持批量重命名");
  },
  async tagGetRule() {
    return null;
  },
  async tagSetRule() {
    throw new Error("浏览器预览模式不支持自动标签规则");
  },
  async libFiles() {
    return [] as [string, string, number][];
  },
  async resizeSave() {
    throw new Error("浏览器预览模式不支持调整尺寸");
  },
  async exif() {
    return null;
  },

  /* 浏览器预览里的「检查更新」：永远返回"已是最新"，不弹任何红字 ——
     预览页没有安装包可装，装成"有新版"只会让人去点一个必然失败的按钮。 */
  async updateCheck() {
    return {
      current: "0.1.35",
      latest: "0.1.35",
      hasUpdate: false,
      url: "https://github.com/706972745/AssetManager/releases",
      notes: "",
      publishedAt: "",
      source: "github",
      setupUrl: "",
      setupName: "",
      sha256: "",
      setupAssetId: 0,
      canAutoInstall: false,
      installKind: "portable",
      autoInstallBlocked: "portable",
    };
  },
  async updateDownload() {
    throw new Error("浏览器预览下不能下载安装包，请用桌面版");
  },
  async updateInstall() {
    throw new Error("浏览器预览下不能安装更新，请用桌面版");
  },
};
