import type { ReactNode } from "react";
import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { scrimFade, springSnappy } from "../design/springs";

/**
 * 一句话确认框（Promise 版）。
 *
 * 为什么自己画：`window.confirm` 在 WebView2 里是**系统模态框**，样式跟这套界面
 * 完全不搭，而且会挡住整个窗口（连拖拽状态都会被打断）。这里用抽屉式小卡片，
 * 支持最多三个按钮 —— 「同名三选」（保留两者 / 替换 / 跳过）正好三个。
 */
export interface DlgButton {
  label: string;
  value: string;
  tone?: "accent" | "danger";
}

export interface AskOptions {
  title: string;
  body?: ReactNode;
  buttons: DlgButton[];
  /** 点遮罩 / 按 Esc 的返回值，默认 null */
  cancelValue?: string | null;
}

export function useDialog() {
  const [opts, setOpts] = useState<AskOptions | null>(null);
  const resolver = useRef<((v: string | null) => void) | null>(null);

  const ask = (o: AskOptions) =>
    new Promise<string | null>((resolve) => {
      resolver.current = resolve;
      setOpts(o);
    });

  const close = (v: string | null) => {
    resolver.current?.(v);
    resolver.current = null;
    setOpts(null);
  };

  useEffect(() => {
    if (!opts) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        close(null);
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts]);

  const node = (
    <AnimatePresence>
      {opts && (
        <motion.div
          key="dlg-scrim"
          className="dlg-scrim"
          onMouseDown={() => close(null)}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={scrimFade}
        >
          <motion.div
            className="dlg"
            role="dialog"
            aria-modal="true"
            aria-label={opts.title}
            onMouseDown={(e) => e.stopPropagation()}
            /* 与 FontManager DestructiveConfirmDialog 同一组参数：
               遮罩淡入 0.18s、卡片 scale 0.96 + y12 弹上来。 */
            initial={{ opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.97, y: 8, transition: { duration: 0.13 } }}
            transition={springSnappy}
          >
            <div className="dlg-title">{opts.title}</div>
            {opts.body && <div className="dlg-body">{opts.body}</div>}
            <div className="dlg-actions">
              {opts.buttons.map((b) => (
                <button
                  key={b.value}
                  className={`btn ${b.tone === "accent" ? "btn-accent" : ""} ${
                    b.tone === "danger" ? "btn-danger" : ""
                  }`}
                  autoFocus={b.tone === "accent"}
                  onClick={() => close(b.value)}
                >
                  {b.label}
                </button>
              ))}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );

  return { ask, node, close };
}
