import { useEffect } from "react";
import { AnimatePresence, motion } from "motion/react";
import { scrimFade, springSnappy } from "../design/springs";

/**
 * 快捷键一览（侧栏底部「快捷键」进入）。
 *
 * 为什么单独一个面板而不是 toast：以前点侧栏是把一长串快捷键塞进 toast，
 * 一屏读不完、还会挡住详情面板（owner 2026-11-17 截图指出）。
 * 现在照 FontManager 的快捷键面板做**双栏**：左栏浏览/选择，右栏文件/面板，
 * 每条都是**代码里真实注册过**的键（不写没实现的），键帽用同一套 `.kbd-set`。
 */

interface Row {
  keys: string[];
  label: string;
  /** 补充说明（灰字，跟在标签后面） */
  hint?: string;
}

interface Group {
  title: string;
  rows: Row[];
}

/** 左栏：看与选（只列代码里**真的注册过**的键；没有的不要往里写） */
const LEFT: Group[] = [
  {
    title: "浏览与预览",
    rows: [
      { keys: ["Space"], label: "打开大图预览" },
      { keys: ["B"], label: "贴图", hint: "当前选中那张；只剩图像，可拖到任意位置" },
      { keys: ["←", "→"], label: "上一张 / 下一张", hint: "大图里翻页，网格里换选中" },
      { keys: ["滚轮"], label: "大图缩放（1×–8×）" },
      { keys: ["双击"], label: "大图缩放复位 1:1" },
      { keys: ["Esc"], label: "关大图 / 关贴图 / 关浮层" },
      { keys: ["双击标题"], label: "就地重命名", hint: "不再弹窗" },
      { keys: ["Shift", "双击标题"], label: "复制文件名", hint: "0.4.11 起单击不再复制，防误触" },
    ],
  },
  {
    title: "选择",
    rows: [
      { keys: ["Ctrl", "点击"], label: "加选 / 取消这一项" },
      { keys: ["Shift", "点击"], label: "从上一个到这一个连选" },
      { keys: ["空白处拖动"], label: "框选" },
      { keys: ["Ctrl", "A"], label: "全选当前列表" },
    ],
  },
];

/** 右栏：干活（文件动作一律落在磁盘上的原文件，不是缩略图） */
const RIGHT: Group[] = [
  {
    title: "文件",
    rows: [
      { keys: ["拖出窗口"], label: "拖给别的程序 / 移动文件", hint: "拖到文件夹：同盘=移动、跨盘=复制；拖给 PS / 浏览器=复制" },
      { keys: ["拖到目录树"], label: "真实移动到这个文件夹" },
      { keys: ["Ctrl", "C"], label: "复制原文件", hint: "CF_HDROP，粘出来是文件不是位图" },
      { keys: ["Ctrl", "Z"], label: "撤销上一步文件操作" },
      { keys: ["T"], label: "给选中的素材打标签", hint: "多选则整批一起打" },
      { keys: ["右键"], label: "在鼠标旁看这张图的详情", hint: "点外部或 Esc 关掉" },
      { keys: ["Shift", "右键"], label: "打开原来的右键菜单", hint: "打开 / 贴图 / 分组 / 重命名…" },
    ],
  },
  {
    title: "截图 · 贴图（系统级）",
    rows: [
      { keys: ["F1"], label: "选区截图", hint: "松手进标注态，不自动复制、不自动关闭" },
      { keys: ["F3"], label: "贴剪贴板图（无边框窗口）" },
    ],
  },
  {
    /* 键位以 `Snip.tsx` 的 keydown 为准（0.1.27）：Ctrl+C 复制、Ctrl+S 保存，
       两者只认真人按键（外部程序注入的会被静默丢弃）。 */
    title: "截图标注",
    rows: [
      { keys: ["Ctrl", "C"], label: "复制到剪贴板", hint: "只有显式复制才写剪贴板" },
      { keys: ["Ctrl", "S"], label: "保存到截图库", hint: "直接写安装目录的 screenshots" },
      { keys: ["Ctrl", "Shift", "S"], label: "另存为…", hint: "自己选路径" },
      { keys: ["中键"], label: "贴到屏幕上（无边框窗口）", hint: "与截图同尺寸、同位置" },
      { keys: ["Enter"], label: "不结束截图", hint: "框完可直接画标注" },
      { keys: ["V"], label: "选择 / 移动标注" },
      { keys: ["R", "E", "L", "A"], label: "矩形 / 椭圆 / 直线 / 箭头" },
      { keys: ["P", "H"], label: "画笔 / 荧光笔" },
      { keys: ["T", "N", "K"], label: "文字 / 序号 / 引线气泡", hint: "N 双击图标可归零" },
      { keys: ["M", "B", "G"], label: "马赛克 / 高斯模糊 / 色块" },
      { keys: ["X", "C"], label: "勾选 / 叉号图章" },
      { keys: ["[", "]"], label: "线条粗细 −/+", hint: "Shift 2px、Alt 0.1px；也能在粗细按钮上滚轮" },
      { keys: ["F"], label: "形状填充（当前色 20%）" },
      { keys: ["Y"], label: "橡皮擦" },
      { keys: ["Delete"], label: "删掉选中的那条标注" },
      { keys: ["Ctrl", "Z"], label: "撤销", hint: "Ctrl+Shift+Z 重做" },
      { keys: ["Space", "拖动"], label: "移动选区", hint: "按住空格拖画面" },
      { keys: ["Ctrl", "A"], label: "选中整屏" },
      { keys: ["↑", "↓", "←", "→"], label: "微调选区", hint: "Shift = 改大小；框选前 W/A/S/D 微调光标" },
      { keys: ["Shift"], label: "切换 RGB / HEX", hint: "放大镜取色；Alt 单击直接取色" },
      { keys: ["Esc"], label: "取消选区 / 退出截图", hint: "有标注时再按一次才退出" },
    ],
  },
  {
    title: "灵感板 · 面板",
    rows: [
      { keys: ["Ctrl", "V"], label: "灵感板：贴剪贴板图入库" },
      { keys: ["T"], label: "灵感板：加一张文字便签", hint: "双击便签改字，右下角缩放" },
      { keys: ["Shift", "点击"], label: "灵感板：加选 / 减选", hint: "点组里一员 = 整组选中" },
      { keys: ["Ctrl", "G"], label: "灵感板：打组", hint: "组内一起移动、一起缩放" },
      { keys: ["Ctrl", "Shift", "G"], label: "灵感板：解组" },
      { keys: ["L"], label: "灵感板：开合左侧图层面板", hint: "点谁谁置顶" },
      { keys: ["F"], label: "灵感板：适应窗口", hint: "0 = 回到 100%" },
      { keys: ["Ctrl", "K"], label: "命令面板", hint: "所有操作都能搜到" },
      { keys: ["Ctrl", ","], label: "设置" },
    ],
  },
];

function GroupBlock({ g }: { g: Group }) {
  return (
    <section className="kb-group">
      <div className="kb-group-title">{g.title}</div>
      {g.rows.map((r) => (
        <div className="kb-row" key={`${g.title}-${r.label}`}>
          <span className="kb-label">
            {r.label}
            {r.hint && <span className="kb-hint">{r.hint}</span>}
          </span>
          <span className="kbd-set kb-keys">
            {r.keys.map((k) => (
              <kbd key={k}>{k}</kbd>
            ))}
          </span>
        </div>
      ))}
    </section>
  );
}

export function Shortcuts({ open, onClose }: { open: boolean; onClose: () => void }) {
  // Esc 关面板：capture + stopImmediatePropagation，别让 App 的全局 Esc 顺手关掉别的东西
  useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [open, onClose]);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          key="kb-scrim"
          className="set-scrim"
          onMouseDown={onClose}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={scrimFade}
        >
          <motion.div
            className="kb-panel"
            role="dialog"
            aria-modal="true"
            aria-label="快捷键"
            onMouseDown={(e) => e.stopPropagation()}
            initial={{ opacity: 0, scale: 0.97, y: 10 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.98, y: 6, transition: { duration: 0.13 } }}
            transition={springSnappy}
          >
            <header className="kb-head">
              <span className="kb-title">快捷键</span>
              <button className="kb-close" onClick={onClose} title="关闭（Esc）">
                Esc
              </button>
            </header>
            <div className="kb-body">
              <div className="kb-col">
                {LEFT.map((g) => (
                  <GroupBlock key={g.title} g={g} />
                ))}
              </div>
              <div className="kb-col">
                {RIGHT.map((g) => (
                  <GroupBlock key={g.title} g={g} />
                ))}
              </div>
            </div>
            <footer className="kb-foot">
              拖出去 / 移走的永远是磁盘上的原文件；缩略图只负责显示。
            </footer>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
