import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";
import {
  ArrowUpDown,
  ChevronRight,
  Box,
  Columns2,
  Copy,
  Database,
  Eye,
  FileText,
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
  plus: ({ size, stroke, className }) => <Plus size={size} strokeWidth={stroke} className={className} />,
  close: ({ size, stroke, className }) => <X size={size} strokeWidth={stroke} className={className} />,
  chevron: ({ size, stroke, className }) => (
    <ChevronRight size={size} strokeWidth={stroke} className={className} />
  ),
  more: ({ size, stroke, className }) => (
    <MoreHorizontal size={size} strokeWidth={stroke} className={className} />
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
}

export function Toaster({ items }: { items: ToastItem[] }) {
  return (
    <div className="toaster">
      {items.map((t) => (
        <div key={t.id} className={`toast ${t.error ? "err" : ""}`}>
          <div className="toast-title">{t.title}</div>
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
}

export function Menu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const h = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", h);
    window.addEventListener("keydown", k);
    return () => {
      window.removeEventListener("mousedown", h);
      window.removeEventListener("keydown", k);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="menu"
      style={{ left: Math.min(x, window.innerWidth - 300), top: Math.min(y, window.innerHeight - 40 - items.length * 33) }}
    >
      {items.map((it, i) =>
        it.sep ? (
          <div key={i} style={{ height: 1, background: "var(--ui-line)", margin: "5px 0" }} />
        ) : (
          <button
            key={i}
            className={`menu-item ${it.on ? "on" : ""}`}
            disabled={it.disabled}
            onClick={() => {
              it.onPick?.();
              onClose();
            }}
          >
            {it.label}
          </button>
        )
      )}
    </div>
  );
}
