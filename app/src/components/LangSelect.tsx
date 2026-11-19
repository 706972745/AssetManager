import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check } from "lucide-react";
import { LOCALES, detectLocale, useT, type LocalePref } from "../lib/i18n";
import { Icon } from "./ui";

/**
 * 界面语言选择器（0.3.30，照 FontManager 的 `LanguageSelect`）。
 *
 * 为什么不用原生 `<select>`：
 * 1. 原生下拉的样式由系统画，跟软件完全不是一个观感（灰底、系统箭头、点击才出现）；
 * 2. 九种语言要**同时看到本名和英文名**（`日本語` / Japanese）—— 原生 option 只有一行；
 * 3. 「跟随系统」还得把当前系统语言写出来（`跟随系统 · 简体中文`），原生做不到。
 *
 * 和 FontManager 的差异：它用 motion + 自己算上下翻转；这里考虑到设置面板本身
 * 是 348 / 半屏的小浮层，选择器只用一个 fixed 菜单 + 越界翻转，够用且少一层依赖。
 */

interface Choice {
  value: LocalePref;
  /** 语言自己的写法（给人看的主文案） */
  label: string;
  /** 右侧灰字：跟随系统时显示解析出来的语言，其它显示英文名 */
  hint: string;
}

const MENU_MIN_W = 232;
const ITEM_H = 34;
const GAP = 6;

export function LangSelect({
  value,
  onChange,
}: {
  value: LocalePref;
  onChange: (v: LocalePref) => void;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [box, setBox] = useState<{ left: number; top: number; width: number } | null>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const systemName = LOCALES.find((l) => l.code === detectLocale())?.name ?? "";
  const choices: Choice[] = [
    { value: "system", label: t("lang.system"), hint: systemName },
    ...LOCALES.map((l) => ({ value: l.code as LocalePref, label: l.name, hint: l.english })),
  ];
  const cur = choices.find((c) => c.value === value) ?? choices[0];
  const idx = Math.max(0, choices.findIndex((c) => c.value === value));

  /** 定位：默认贴按钮下方右对齐；下方放不下就翻到上方（设置面板在屏幕下方时很常见） */
  const place = () => {
    const r = btnRef.current?.getBoundingClientRect();
    if (!r) return;
    const width = Math.max(r.width, MENU_MIN_W);
    const height = choices.length * ITEM_H + 12;
    const below = r.bottom + GAP;
    const flip = below + height > window.innerHeight - 8 && r.top - GAP - height > 8;
    setBox({
      left: Math.max(8, Math.min(r.right - width, window.innerWidth - width - 8)),
      top: flip ? r.top - GAP - height : below,
      width,
    });
  };

  const openMenu = () => {
    place();
    setActive(idx);
    setOpen(true);
  };

  /* 点外面 / 滚走就关：设置面板本身可滚，菜单是 fixed 的，滚动后不关会"漂"在原地 */
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (btnRef.current?.contains(t) || menuRef.current?.contains(t)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        /* 必须拦住：否则这一下 Esc 会把整个设置面板关掉 */
        e.stopPropagation();
        e.preventDefault();
        setOpen(false);
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        setActive((i) => Math.min(choices.length - 1, i + 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setActive((i) => Math.max(0, i - 1));
      } else if (e.key === "Enter") {
        e.preventDefault();
        pick(choices[active]);
      }
    };
    const onScroll = () => setOpen(false);
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [open, active, choices]);

  const pick = (c: Choice) => {
    onChange(c.value);
    setOpen(false);
  };

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={`lang-trigger ${open ? "on" : ""}`}
        onClick={() => (open ? setOpen(false) : openMenu())}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={t("lang.label")}
      >
        <span className="lang-trigger-label">{cur.label}</span>
        <Icon name="chevron" size={11} stroke={1.8} className="lang-trigger-caret" />
      </button>

      {open &&
        box &&
        createPortal(
          <div
            ref={menuRef}
            className="lang-menu"
            role="listbox"
            style={{ left: box.left, top: box.top, width: box.width }}
          >
            {choices.map((c, i) => (
              <button
                key={c.value}
                role="option"
                aria-selected={c.value === value}
                className={`lang-item ${i === active ? "on" : ""}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(c)}
              >
                <span className="lang-item-name">{c.label}</span>
                <span className="lang-item-hint">{c.hint}</span>
                {c.value === value && (
                  <span className="lang-item-tick" aria-hidden="true">
                    <Check size={11} strokeWidth={2} />
                  </span>
                )}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
