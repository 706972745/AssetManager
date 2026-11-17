/**
 * 三个**系统级**快捷键（软件不在前台也要能按）。
 *
 * 存 localStorage 而不是 `peeks.json`：它属于"界面偏好"，和贴图存档的生命周期不一样 ——
 * 贴图删了不该把键位也丢回默认。
 *
 * 真正注册在 Rust（`am_hotkeys_set`）。这里是**唯一的事实来源**：启动时注册一次，
 * 设置里改完立刻再注册一次；注册失败的组合会被 Rust 原样返回，界面上要如实告诉用户
 * （"F1 被别的软件占了"这种事，静默失败比报错更气人）。
 */

export interface HotkeySet {
  /** 截图：选完区保存到截图目录 */
  snip: string;
  /** 贴剪贴板：把剪贴板里的图贴到屏幕上 */
  clip: string;
  /** 截图后直接进剪贴板（不落盘） */
  snipCopy: string;
}

export const DEFAULT_HOTKEYS: HotkeySet = { snip: "F1", clip: "F3", snipCopy: "Alt+F1" };

const KEY = "am.hotkeys.v1";

export function loadHotkeys(): HotkeySet {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const v = JSON.parse(raw) as Partial<HotkeySet>;
      return {
        snip: v.snip || DEFAULT_HOTKEYS.snip,
        clip: v.clip || DEFAULT_HOTKEYS.clip,
        snipCopy: v.snipCopy || DEFAULT_HOTKEYS.snipCopy,
      };
    }
  } catch {
    // 坏 JSON 就用默认值，别让设置面板打不开
  }
  return { ...DEFAULT_HOTKEYS };
}

export function saveHotkeys(v: HotkeySet) {
  try {
    localStorage.setItem(KEY, JSON.stringify(v));
  } catch {
    // 存不了就算，本次会话仍然生效
  }
}

/** 按键事件 → Tauri 认的 accelerator（`Ctrl+Shift+F1` 这种写法） */
export function accelOf(e: KeyboardEvent | React.KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");

  const k = e.key;
  // 只有修饰键按下时不算一个完整组合（用户还在打字）
  if (["Control", "Alt", "Shift", "Meta", "CapsLock"].includes(k)) return "";
  let name = k;
  if (k === " ") name = "Space";
  else if (k.length === 1) name = k.toUpperCase();
  else if (k.startsWith("Arrow")) name = k.replace("Arrow", "");
  parts.push(name);
  return parts.join("+");
}
