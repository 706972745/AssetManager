import { useEffect, useRef, useState } from "react";
import { accelOf } from "../lib/hotkeys";

/**
 * 系统级快捷键的录制框。
 *
 * 点一下进入"按下组合键…"，按下什么就记什么；Esc 放弃、Backspace 清空。
 * 为什么不给下拉框选：快捷键是"手上习惯"，用户按一遍比在几百项里找一个快得多
 * （果核看图也是这个做法）。
 */
export function HotkeyField({
  value,
  onChange,
  placeholder = "未设置",
}: {
  value: string;
  onChange: (accel: string) => void;
  placeholder?: string;
}) {
  const [rec, setRec] = useState(false);
  const box = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!rec) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setRec(false);
        return;
      }
      if (e.key === "Backspace") {
        onChange("");
        setRec(false);
        return;
      }
      const accel = accelOf(e);
      if (!accel) return; // 只按了修饰键，继续等
      onChange(accel);
      setRec(false);
    };
    // capture：别让 App 的全局快捷键（Ctrl+K / F1）把这次录制抢走
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [rec, onChange]);

  return (
    <button
      ref={box}
      className={`hk-field mono ${rec ? "rec" : ""}`}
      onClick={() => setRec(true)}
      title="点击后按下新的组合键；Esc 放弃，Backspace 清空"
    >
      {rec ? "按下组合键…" : value || placeholder}
    </button>
  );
}
