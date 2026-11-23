/**
 * **系统级**快捷键（软件不在前台也要能按）。
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
  /** 贴剪贴板：把剪贴板里的图贴到屏幕上（连按翻最近 10 张） */
  clip: string;
  /** 显示 / 隐藏屏幕上所有贴图（0.5.7 起默认不绑定，Alt+F3 让给 together） */
  toggle: string;
  /** 贴图与灵感板**一起**显示 / 隐藏 */
  together: string;
  /** 贴图与灵感板**交替**显示（互斥切换） */
  swap: string;
}

/* `Alt+F1`（截完直接进剪贴板）在 0.1.20 删掉了：
   它的唯一效果就是"框完区就复制并把覆盖层关掉"，用户既看不到工具栏、也没机会加标注，
   而这正是 owner 反复报的「截图完成后老是自动复制」。0.1.27 起 Ctrl+C / Ctrl+S 恢复了，但只认真人按键（见 core/src/keywatch.rs）。 */
export const DEFAULT_HOTKEYS: HotkeySet = {
  snip: "F1",
  clip: "F3",
  /* 0.5.7：Alt+F3 改成「贴图 + 灵感板一起开关」。`toggle`（只管贴图）默认留空不注册；
     还想要老语义的人可以在设置里自己绑一个键。 */
  toggle: "",
  together: "Alt+F3",
  swap: "Shift+F3",
};

const KEY = "am.hotkeys.v1";

export function loadHotkeys(): HotkeySet {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const v = JSON.parse(raw) as Partial<HotkeySet>;
      /* 老配置（0.5.6 及以前）只有 snip/clip/toggle 三个字段，且 toggle 的默认值
         就是 Alt+F3。0.5.7 把这个键让给了 together —— 直接补齐新字段会让两个动作
         抢同一个 accelerator（只有一个能注册成功）。所以"没有 together 字段"的
         老配置里，如果 toggle 还是那个默认的 Alt+F3，就清空它。 */
      const hadTogether = typeof v.together === "string";
      let toggle = typeof v.toggle === "string" ? v.toggle : DEFAULT_HOTKEYS.toggle;
      if (!hadTogether && toggle === "Alt+F3") toggle = "";
      return {
        snip: v.snip || DEFAULT_HOTKEYS.snip,
        clip: v.clip || DEFAULT_HOTKEYS.clip,
        toggle,
        together: hadTogether ? v.together! : DEFAULT_HOTKEYS.together,
        swap: typeof v.swap === "string" ? v.swap : DEFAULT_HOTKEYS.swap,
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

/**
 * `HotkeySet` → 后端 `am_hotkeys_set` 的 `(accelerator, action)` 列表。
 *
 * 启动注册（App.tsx）与设置里改键（Settings.tsx）必须用同一份**顺序**：
 * 后端拿整表判"和上次一样就跳过"，两边顺序不同会让它每次都重新注册 ——
 * Windows 上反复 unregister/register 偶发"刚注销的键还被占用"，F1 会失效。
 * 空 accelerator 原样交给后端，它会自己过滤（见 `register_hotkeys`）。
 */
export function hotkeyBindings(h: HotkeySet): [string, string][] {
  return [
    [h.snip, "snip"],
    [h.clip, "clip_peek"],
    [h.toggle, "peek_toggle"],
    [h.together, "peek_board_together"],
    [h.swap, "peek_board_swap"],
  ];
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
