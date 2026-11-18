import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
} from "react";
import {
  ArrowUpDown,
  Camera,
  Check,
  ChevronRight,
  Box,
  Columns2,
  Copy,
  Database,
  Eye,
  Expand,
  FileText,
  FileUp,
  Film,
  Filter,
  Folder,
  FolderOpen,
  Image as ImageIcon,
  Info,
  Keyboard,
  LayoutGrid,
  MoreHorizontal,
  Music,
  Plus,
  RefreshCw,
  Search,
  Sparkles,
  Star,
  Tag,
  Trash2,
  Undo2,
  X,
} from "lucide-react";

/* ---------------------------------------------------------------- 图标 */
/* 图标一律**照抄 FontManager**：同一套 lucide-react（同版本 1.24.0）、同样的
   strokeWidth=1.5、同样的 24 格网格。以前这里是手搓的内联 path —— 形状接近但
   不是同一个东西，两边并排看就露馅（owner 2026-11-16：直接搬过来，别自己画）。
   设置图标是个例外：FontManager 用的是自绘的「滑杆」不是齿轮，这里逐字节复制。 */

/** 设置：与 FontManager `Titlebar.tsx` 的 SettingsGlyph 完全一致（16 格、滑杆）。 */
function SettingsGlyph({
  size = 14,
  stroke = 1.5,
  className,
}: {
  size?: number;
  stroke?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={stroke}
      strokeLinecap="round"
      className={className}
      aria-hidden="true"
    >
      <line x1="2.75" y1="4.75" x2="7.5" y2="4.75" />
      <line x1="12.9" y1="4.75" x2="13.25" y2="4.75" />
      <circle cx="10.2" cy="4.75" r="2" />
      <line x1="2.75" y1="11.25" x2="3.1" y2="11.25" />
      <line x1="8.5" y1="11.25" x2="13.25" y2="11.25" />
      <circle cx="5.8" cy="11.25" r="2" />
    </svg>
  );
}

type Glyph = (p: { size?: number; stroke?: number; className?: string }) => ReactElement;

/** 名字 → 图标。名字保持中文语义（grid / waterfall / scan…），实现换成 lucide。 */
const ICONS: Record<string, Glyph> = {
  sliders: ({ size, stroke, className }) => (
    <SettingsGlyph size={size} stroke={stroke} className={className} />
  ),
  /* 设置：标题栏与设置面板都用 FontManager 那枚滑杆 */
  settings: ({ size, stroke, className }) => (
    <SettingsGlyph size={size} stroke={stroke} className={className} />
  ),
  database: ({ size, stroke, className }) => (
    <Database size={size} strokeWidth={stroke} className={className} />
  ),
  grid: ({ size, stroke, className }) => (
    <LayoutGrid size={size} strokeWidth={stroke} className={className} />
  ),
  waterfall: ({ size, stroke, className }) => (
    <Columns2 size={size} strokeWidth={stroke} className={className} />
  ),
  scan: ({ size, stroke, className }) => (
    <RefreshCw size={size} strokeWidth={stroke} className={className} />
  ),
  folder: ({ size, stroke, className }) => (
    <Folder size={size} strokeWidth={stroke} className={className} />
  ),
  /* 打开状态的文件夹：分组面板（照 FontManager 的 `FolderOpen`）用它，
     和「新建文件夹」那枚合着的文件夹区分开 */
  "folder-open": ({ size, stroke, className }) => (
    <FolderOpen size={size} strokeWidth={stroke} className={className} />
  ),
  plus: ({ size, stroke, className }) => <Plus size={size} strokeWidth={stroke} className={className} />,
  close: ({ size, stroke, className }) => <X size={size} strokeWidth={stroke} className={className} />,
  chevron: ({ size, stroke, className }) => (
    <ChevronRight size={size} strokeWidth={stroke} className={className} />
  ),
  more: ({ size, stroke, className }) => (
    <MoreHorizontal size={size} strokeWidth={stroke} className={className} />
  ),
  /* 快照（0.12.1）：照 FontManager 侧栏给快照行用的相机图标 */
  camera: ({ size, stroke, className }) => (
    <Camera size={size} strokeWidth={stroke} className={className} />
  ),
  /* 导入（快照 / 未来别的导入都用它） */
  import: ({ size, stroke, className }) => (
    <FileUp size={size} strokeWidth={stroke} className={className} />
  ),
  image: ({ size, stroke, className }) => (
    <ImageIcon size={size} strokeWidth={stroke} className={className} />
  ),
  /* 非图片类素材在磁贴上的"代替缩略图"（JSON/PDF/视频/音频/模型）——
     有这组图标，一条没有预览的文件才是「这是个文档」，而不是「缩略图挂了」。 */
  star: ({ size, stroke, className }) => <Star size={size} strokeWidth={stroke} className={className} />,
  file: ({ size, stroke, className }) => <FileText size={size} strokeWidth={stroke} className={className} />,
  video: ({ size, stroke, className }) => <Film size={size} strokeWidth={stroke} className={className} />,
  audio: ({ size, stroke, className }) => <Music size={size} strokeWidth={stroke} className={className} />,
  model: ({ size, stroke, className }) => <Box size={size} strokeWidth={stroke} className={className} />,
  undo: ({ size, stroke, className }) => <Undo2 size={size} strokeWidth={stroke} className={className} />,
  search: ({ size, stroke, className }) => (
    <Search size={size} strokeWidth={stroke} className={className} />
  ),
  funnel: ({ size, stroke, className }) => (
    <Filter size={size} strokeWidth={stroke} className={className} />
  ),
  reveal: ({ size, stroke, className }) => (
    <FolderOpen size={size} strokeWidth={stroke} className={className} />
  ),
  copy: ({ size, stroke, className }) => <Copy size={size} strokeWidth={stroke} className={className} />,
  trash: ({ size, stroke, className }) => <Trash2 size={size} strokeWidth={stroke} className={className} />,
  tag: ({ size, stroke, className }) => <Tag size={size} strokeWidth={stroke} className={className} />,
  info: ({ size, stroke, className }) => <Info size={size} strokeWidth={stroke} className={className} />,
  play: ({ size, stroke, className }) => <Eye size={size} strokeWidth={stroke} className={className} />,
  /* 无边框贴图（照果核看图）：四角向外的箭头 */
  expand: ({ size, stroke, className }) => (
    <Expand size={size} strokeWidth={stroke} className={className} />
  ),
  sort: ({ size, stroke, className }) => (
    <ArrowUpDown size={size} strokeWidth={stroke} className={className} />
  ),
  keyboard: ({ size, stroke, className }) => (
    <Keyboard size={size} strokeWidth={stroke} className={className} />
  ),
};

export function Icon({
  name,
  size = 14,
  stroke = 1.5,
  className,
}: {
  name: keyof typeof ICONS | string;
  size?: number;
  stroke?: number;
  className?: string;
}) {
  const G = ICONS[name];
  if (!G) return null;
  return <G size={size} stroke={stroke} className={className} />;
}

/** 品牌标：三张错位叠片（拼贴纸片风，母题换「叠片」）。 */
export function BrandMark({ size = 21 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden>
      <rect x="3.5" y="6.5" width="12" height="14" rx="1.5" fill="currentColor" opacity="0.16" />
      <rect x="6.5" y="4.5" width="12" height="14" rx="1.5" fill="currentColor" opacity="0.3" />
      <rect
        x="9.5"
        y="2.5"
        width="11.5"
        height="13.5"
        rx="1.5"
        fill="var(--ui-bg)"
        stroke="currentColor"
        strokeWidth="1.3"
      />
      <path d="M12 12.2l2.1-3 1.5 2 1.1-1.4 1.3 2.4z" fill="currentColor" opacity="0.55" />
      <circle cx="12.6" cy="6.6" r="1" fill="currentColor" opacity="0.55" />
    </svg>
  );
}

/* ---------------------------------------------------------------- 块光标 */

/** 蓝块比被覆盖字符左右各多出的内边距（口径：清晰度优先于严丝合缝）。 */
const BLOCK_PAD = 4;

export interface CaretInputProps {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  /** 输入框自身的类名（.topbar-field / .tree-input …） */
  inputClassName?: string;
  /** wrapper 的类名（管伸展与裁切，不要往这里放 input 的边框样式） */
  fieldClassName?: string;
  onKeyDown?: (e: React.KeyboardEvent<HTMLInputElement>) => void;
  onFocus?: () => void;
  onBlur?: () => void;
  autoFocus?: boolean;
  spellCheck?: boolean;
  ariaLabel?: string;
}

/** 镜像与白字形共用：字体六项逐条复制（只抄 className 会漏 letterSpacing / textTransform）。 */
function syncFont(el: HTMLElement, cs: CSSStyleDeclaration) {
  el.style.fontFamily = cs.fontFamily;
  el.style.fontSize = cs.fontSize;
  el.style.fontWeight = cs.fontWeight;
  el.style.fontStyle = cs.fontStyle;
  el.style.letterSpacing = cs.letterSpacing;
  el.style.textTransform = cs.textTransform;
}

function isHighSurrogate(c: string | undefined): boolean {
  return c !== undefined && c >= "\ud800" && c <= "\udbff";
}
function isLowSurrogate(c: string | undefined): boolean {
  return c !== undefined && c >= "\udc00" && c <= "\udfff";
}

/** 居中 / 右对齐输入框里文本相对内容区左缘的偏移（镜像测宽是左对齐口径）。 */
function textStartOffset(textAlign: string, contentWidth: number, textWidth: number): number {
  const spare = contentWidth - textWidth;
  if (!Number.isFinite(spare) || spare <= 0) return 0;
  if (textAlign === "center") return spare / 2;
  if (textAlign === "right" || textAlign === "end") return spare;
  return 0;
}

/**
 * 自绘块状光标（与 FontManager 的搜索栏同一套几何，视觉上必须一样）：
 * - 块盖住光标**前面**那个字符，宽 = 字符 advance + 左右各 4，高 = 字号 + 2 且垂直居中
 *   （块高只跟字号走，绝不撑满 36px 输入框）；
 * - 块内叠一个白色同字符 span，字体与 input 完全一致 → 视觉上那个字「变白」；
 * - 空输入退化成一小块贴左；1.1s steps(1,end) 硬切闪烁；
 * - 中文 IME 组合期间藏起自绘块、恢复原生光标（否则候选框位置会飘）。
 * 定位靠隐藏镜像 span 量宽，不用 canvas —— 字距 / 连字 / 缩放都跟着浏览器走。
 */
export function BlinkCaretInput({
  value,
  onChange,
  placeholder,
  inputClassName = "",
  fieldClassName = "",
  onKeyDown,
  onFocus,
  onBlur,
  autoFocus,
  spellCheck = false,
  ariaLabel,
}: CaretInputProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const caretRef = useRef<HTMLSpanElement>(null);
  const glyphRef = useRef<HTMLSpanElement>(null);
  const mirrorRef = useRef<HTMLSpanElement>(null);
  const [focused, setFocused] = useState(false);
  const [composing, setComposing] = useState(false);

  const reposition = useCallback(() => {
    const input = inputRef.current;
    const mirror = mirrorRef.current;
    const caret = caretRef.current;
    if (!input || !mirror || !caret) return;

    const cs = getComputedStyle(input);
    const glyph = glyphRef.current;
    const val = input.value;
    const sel = input.selectionStart ?? val.length;

    // 纵向：块高只跟字号走，垂直居中
    const fs = Number.parseFloat(cs.fontSize) || 13;
    const caretH = Math.max(14, Math.round(fs + 2));
    const blockTop = input.offsetTop + Math.max(0, Math.round((input.clientHeight - caretH) / 2));
    caret.style.height = `${caretH}px`;
    caret.style.top = `${blockTop}px`;

    const padL = Number.parseFloat(cs.paddingLeft) || 0;
    const padR = Number.parseFloat(cs.paddingRight) || 0;
    const contentW = Math.max(0, input.clientWidth - padL - padR);

    syncFont(mirror, cs);
    let textOffset = 0;
    const align = cs.textAlign;
    if (align === "center" || align === "right" || align === "end") {
      mirror.textContent = val;
      textOffset = textStartOffset(align, contentW, mirror.offsetWidth);
    }

    // 空输入：没有字符可盖，退化成一小块
    if (!val.length) {
      caret.style.left = `${padL}px`;
      caret.style.width = "";
      caret.classList.add("blink-caret-empty");
      if (glyph) glyph.textContent = "";
      mirror.textContent = "";
      return;
    }
    caret.classList.remove("blink-caret-empty");

    const clamped = Math.max(0, Math.min(sel, val.length));
    let idx = clamped > 0 ? clamped - 1 : 0;
    // 增补平面（emoji / 生僻字）占两个 code unit，回退到高代理，整对一起盖
    if (idx > 0 && isLowSurrogate(val[idx]) && isHighSurrogate(val[idx - 1])) idx -= 1;
    const charLen = isHighSurrogate(val[idx]) && isLowSurrogate(val[idx + 1]) ? 2 : 1;

    mirror.textContent = val.slice(0, idx);
    const charLeft = mirror.offsetWidth + textOffset - input.scrollLeft;
    mirror.textContent = val.slice(idx, idx + charLen);
    const charW = mirror.offsetWidth;

    const left = Math.max(0, charLeft - BLOCK_PAD);
    const glyphShift = charLeft - left;
    caret.style.left = `${padL + left}px`;
    caret.style.width = `${Math.max(0, glyphShift + charW + BLOCK_PAD)}px`;

    if (glyph) {
      glyph.textContent = val.slice(idx, idx + charLen);
      syncFont(glyph, cs);
      glyph.style.left = `${glyphShift}px`;
      const padT = Number.parseFloat(cs.paddingTop) || 0;
      const padB = Number.parseFloat(cs.paddingBottom) || 0;
      const borderT = Number.parseFloat(cs.borderTopWidth) || 0;
      const contentH = Math.max(0, input.clientHeight - padT - padB);
      const contentTop = input.offsetTop + borderT + padT;
      const lh = Number.parseFloat(cs.lineHeight);
      if (Number.isFinite(lh) && lh > 0) {
        glyph.style.lineHeight = `${lh}px`;
        glyph.style.top = `${contentTop + Math.max(0, (contentH - lh) / 2) - blockTop}px`;
      } else {
        glyph.style.lineHeight = `${contentH}px`;
        glyph.style.top = `${contentTop - blockTop}px`;
      }
    }
  }, []);

  // 受控值被外部改写（清空、点建议项）不产生原生 input 事件，每次渲染兜一次
  useEffect(() => {
    reposition();
  });

  useEffect(() => {
    const el = inputRef.current;
    if (!el || !focused) return;
    const h = () => reposition();
    const events = ["input", "select", "click", "keyup", "scroll"] as const;
    for (const ev of events) el.addEventListener(ev, h);
    window.addEventListener("resize", h);
    return () => {
      for (const ev of events) el.removeEventListener(ev, h);
      window.removeEventListener("resize", h);
    };
  }, [focused, reposition]);

  return (
    <span
      className={`blink-caret-field ${fieldClassName}${composing ? " is-composing" : ""}`}
      data-composing={composing || undefined}
    >
      <input
        ref={inputRef}
        className={inputClassName}
        value={value}
        placeholder={placeholder}
        autoFocus={autoFocus}
        spellCheck={spellCheck}
        aria-label={ariaLabel}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        onFocus={() => {
          setFocused(true);
          onFocus?.();
        }}
        onBlur={() => {
          setFocused(false);
          onBlur?.();
        }}
        onCompositionStart={() => {
          setComposing(true);
          reposition();
        }}
        onCompositionEnd={() => {
          setComposing(false);
          requestAnimationFrame(reposition);
        }}
      />
      {focused && !composing && (
        <span className="blink-caret" ref={caretRef} aria-hidden>
          <span className="blink-caret-glyph" ref={glyphRef} aria-hidden />
        </span>
      )}
      <span className="blink-caret-mirror" ref={mirrorRef} aria-hidden />
    </span>
  );
}

/* ---------------------------------------------------------------- Toast */

export interface ToastItem {
  id: number;
  title: string;
  detail?: string;
  error?: boolean;
  /** 连续同一条提示被合并的次数（1 = 没合并；>1 时标题后显示 `×N`） */
  count: number;
}

export function Toaster({ items }: { items: ToastItem[] }) {
  return (
    <div className="toaster">
      {items.map((t) => (
        <div key={t.id} className={`toast ${t.error ? "err" : ""}`}>
          <div className="toast-title">
            {t.title}
            {t.count > 1 && (
              <span className="toast-count" aria-label={`×${t.count}`}>
                ×{t.count}
              </span>
            )}
          </div>
          {t.detail && <div className="toast-detail">{t.detail}</div>}
        </div>
      ))}
    </div>
  );
}

/* ---------------------------------------------------------------- 菜单 */

export interface MenuItem {
  label: string;
  onPick?: () => void;
  disabled?: boolean;
  on?: boolean;
  sep?: boolean;
  /** 行首小圆点（任意 CSS background：纯色或渐变）。颜色筛选靠它一眼认色 */
  dot?: string;
  /** 圆点的附加类（标签行用 `.tc-*` 把 `--tc` 带进来，浅深主题各一套色） */
  dotClass?: string;
  /** 行首线性图标。标签行照 FontManager：自动标签 = 魔棒，手工标签 = 标签图标。
      两者都画，只是图形不同 —— 否则同一列里文字左右错开（FontManager 2026-11-14 踩过）。 */
  icon?: ReactNode;
  /** 段头：不可点的小标题（照 FontManager 的 `.filter-heading`） */
  heading?: boolean;
  /** 选中项右侧打勾（FontManager 的 `.sort-item` 就是这样，比只把文字变蓝清楚） */
  check?: boolean;
  /** 右侧灰色附注（数量）。数字用等宽，和侧栏计数同一套观感 */
  meta?: string;
  /** 二级菜单项：命中后在菜单左侧展开一块 fixed 面板（照 FontManager 的 FilterSubTrigger） */
  sub?: MenuItem[];
}

/**
 * 浮层菜单。有两种形态，**不要混用**：
 *
 * - 默认（右键菜单）：FontManager `.ctx-menu` 的定版值 —— 208 宽 / 5 内距 / 30 高行。
 * - `variant="pop"`（顶栏下拉：筛选 / 排序 / 颜色）：FontManager `.sort-menu` 的定版值 ——
 *   min-width 150/176、4 内距、8px 11px 行、13px 字、勾选图标。owner 2026-11-17 要求
 *   顶栏这些下拉和 FontManager 长得一模一样（以前这里用的是 11px 字 + 30 高行的右键菜单尺寸，
 *   并排看明显两套）。
 */
export function Menu({
  x,
  y,
  items,
  onClose,
  variant = "ctx",
  align = "left",
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
  variant?: "ctx" | "pop";
  /** pop 形态下菜单盒贴哪条边：FontManager 的两个下拉都是 right:0（贴按钮右沿） */
  align?: "left" | "right";
}) {
  const ref = useRef<HTMLDivElement>(null);
  const subRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLElement | null>(null);
  const timer = useRef<number | null>(null);
  /** 二级面板（照 FontManager 的 FilterSubTrigger）：fixed 定位、向左弹，
   *  因为 .menu 自己 overflow-y:auto，绝对定位的子面板会被裁掉。 */
  const [sub, setSub] = useState<{ items: MenuItem[]; left: number; top: number } | null>(null);

  const maxH = Math.min(480, window.innerHeight - 24);
  /* 宽度不写死：pop 形态的菜单宽度由 CSS（min-width + 内容）决定，
     所以「贴右沿」用 right 而不是算 left——不需要先量宽度，窄了宽了都不会跳。 */
  const posStyle: CSSProperties =
    align === "right"
      ? { right: Math.max(8, window.innerWidth - x) }
      : { left: Math.min(x, window.innerWidth - 300) };

  const keep = () => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
  };
  const openSub = (el: HTMLElement, subItems: MenuItem[]) => {
    keep();
    triggerRef.current = el;
    const r = el.getBoundingClientRect();
    /* 位置只给锚点（入口行的左上角），**向左弹**交给 CSS 的
       `transform: translate(-100%, -6px) translateX(-6px)` —— 和 FontManager 的
       `.filter-submenu` 同一套：面板宽度是内容决定的，用 JS 减宽度就得先量，白抖一次。 */
    setSub({
      items: subItems,
      left: r.left,
      top: r.top,
    });
  };
  const closeSubSoon = () => {
    keep();
    timer.current = window.setTimeout(() => {
      /* 到点复查：指针还压在入口行或子面板上就不收（FontManager 的 240ms 口径） */
      if (!triggerRef.current?.matches(":hover") && !subRef.current?.matches(":hover")) setSub(null);
      timer.current = null;
    }, 240);
  };

  useEffect(() => {
    const h = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!ref.current?.contains(t) && !subRef.current?.contains(t)) onClose();
    };
    const k = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (sub) setSub(null);
      else onClose();
    };
    window.addEventListener("mousedown", h);
    window.addEventListener("keydown", k);
    return () => {
      window.removeEventListener("mousedown", h);
      window.removeEventListener("keydown", k);
      keep();
    };
  }, [onClose, sub]);

  const renderRow = (it: MenuItem, i: number, inSub = false) => {
    if (it.sep) return <div key={i} className="menu-sep" role="none" />;
    if (it.heading)
      return (
        <div key={i} className="menu-heading" role="none">
          {it.label}
        </div>
      );
    return (
      <button
        key={i}
        className={`menu-item ${it.on ? "on" : ""} ${it.sub?.length ? "has-sub" : ""}`}
        disabled={it.disabled}
        onClick={(e) => {
          if (it.sub?.length && !inSub) {
            if (sub) setSub(null);
            else openSub(e.currentTarget, it.sub);
            return;
          }
          it.onPick?.();
          onClose();
        }}
        onMouseEnter={(e) => {
          if (!inSub && it.sub?.length) openSub(e.currentTarget, it.sub);
        }}
        onMouseLeave={() => {
          if (!inSub && it.sub?.length) closeSubSoon();
        }}
      >
        {it.icon && (
          <span className="menu-ico" aria-hidden>
            {it.icon}
          </span>
        )}
        {(it.dot || it.dotClass) && (
          <span
            className={`menu-dot ${it.dotClass ?? ""}`}
            style={it.dot ? { background: it.dot } : undefined}
            aria-hidden
          />
        )}
        <span className="menu-label">{it.label}</span>
        {it.meta && <span className="menu-meta">{it.meta}</span>}
        {/* 勾选照 FontManager：lucide 的 Check（13/12，stroke 2），不是一个 ✓ 字符
            —— 字符在不同字体下大小/基线都不一样，两个软件并排看就露馅 */}
        {it.check && (
          <Check size={inSub ? 12 : 13} strokeWidth={2} className="menu-check" aria-hidden />
        )}
        {it.sub?.length && !inSub && (
          <ChevronRight size={12} strokeWidth={1.6} className="menu-arrow" aria-hidden />
        )}
      </button>
    );
  };

  return (
    <>
      <div
        ref={ref}
        className={`menu ${variant === "pop" ? "pop" : ""}`}
        style={{
          ...posStyle,
          top: Math.max(8, Math.min(y, window.innerHeight - maxH - 8)),
          maxHeight: maxH,
        }}
      >
        {items.map((it, i) => renderRow(it, i))}
      </div>
      {sub && (
        <div
          ref={subRef}
          className="menu submenu"
          style={{ left: sub.left, top: sub.top }}
          /* 指针进面板 → 取消"稍后收起"的计时器；出去 → 重新计时。
             没有这两个的话，从入口行移到面板（中间隔 6px）的瞬间计时器就开始跑，
             到点发现面板上没指针就把面板收掉 —— 用户的手感是「刚移过去就没了」。 */
          onMouseEnter={keep}
          onMouseLeave={closeSubSoon}
        >
          {sub.items.map((it, i) => renderRow(it, i, true))}
        </div>
      )}
    </>
  );
}
