#!/usr/bin/env node
/**
 * 生成「窗口 → 允许调用的 IPC 命令」表，给 `src-tauri/src/ipc_guard.rs` 当白名单来源。
 *
 * 为什么要有这个脚本，而不是人手抄一遍：
 * 1. 人手抄会漏**链式写法**（`source\n  .boardIsTop()`）—— 0.5.14 那次审查就漏过，
 *    漏一条的后果是「守卫一上线，某个功能静默失效」，且只在用户点到那一刻才暴露。
 * 2. 前端加了新命令后必须能**重跑一次**就知道白名单缺什么，而不是靠记忆。
 *
 * 用法：
 *   node tools/ipc_allowlist.mjs            # 打印报告
 *   node tools/ipc_allowlist.mjs --rust     # 只打印可直接粘进 ipc_guard.rs 的数组
 *   node tools/ipc_allowlist.mjs --check    # 与 ipc_guard.rs 里现有数组比对，不一致退 1
 *
 * 口径（写死在这里，别在别处另立一套）：
 * - 窗口的「可达命令」= 从该窗口的入口文件出发，沿相对 import 递归收集到的所有文件里
 *   出现的 `source.<方法>()` / `boardAnnApi.<方法>()` / `invoke("<cmd>")` / `invoke!<T>("<cmd>")`；
 * - 方法名 → 命令名走 `app/src/data/source.ts` 的对象字面量（方法体里第一个 `invoke*("am_...")`）；
 * - 主窗不参与（主窗允许全部命令）。
 */

import { readFileSync, existsSync } from "node:fs";
import { dirname, resolve, relative, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SRC = join(ROOT, "app", "src");
const GUARD_RS = join(ROOT, "src-tauri", "src", "ipc_guard.rs");
const MAIN_RS = join(ROOT, "src-tauri", "src", "main.rs");

/** 附属窗口 → 入口文件（主窗不在表里：它允许全部） */
const WINDOWS = {
  PEEK: ["components/Peek.tsx"],
  BOARD: ["components/Board.tsx"],
  SNIP: ["components/Snip.tsx"],
  TRAY: ["tray.ts"],
  SPLASH: ["splash.ts"],
};

const read = (p) => readFileSync(p, "utf8");

/* ---------------- 1. source.ts：方法名 → 命令名 ---------------- */

/**
 * 从 `export const <name> = { ... }` 的对象字面量里抽「方法名 → 命令名」。
 * 只认「方法体内出现 invoke("am_xxx")」的那些方法；`desktop: true` 这类非方法字段跳过。
 */
function methodMap(text, startMarker) {
  const at = text.indexOf(startMarker);
  if (at < 0) throw new Error(`source.ts 里找不到 ${startMarker}`);
  // 从对象起始位置做花括号配平，拿到块体（跳过字符串里的括号：这里的方法是简单表达式，够用）
  const open = text.indexOf("{", at);
  let nest = 0;
  let end = open;
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    if (c === "{") nest++;
    else if (c === "}") {
      nest--;
      if (nest === 0) {
        end = i;
        break;
      }
    }
  }
  const body = text.slice(open + 1, end);

  const map = new Map();
  /* 找「方法头」必须做括号深度判断：`add: (\n  board: string,\n  z: number, ...)` 这种
     多行参数表里，`board:` / `z:` 长得和方法头一模一样，按行正则切会把
     `add` 的方法体切没（第一版就这么丢掉了 `am_board_ann_add`）。 */
  const heads = [];
  let depth = 0;
  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (c === "(" || c === "[" || c === "{") depth++;
    else if (c === ")" || c === "]" || c === "}") depth--;
    else if (depth === 0 && (c === "\n" || i === 0)) {
      const rest = body.slice(i + 1);
      const m = rest.match(/^[ \t]*([A-Za-z_$][\w$]*)[ \t]*:/);
      if (m) heads.push({ name: m[1], from: i + 1, body: i + 1 + m[0].length });
    }
  }
  for (let i = 0; i < heads.length; i++) {
    const seg = body.slice(heads[i].body, i + 1 < heads.length ? heads[i + 1].from : body.length);
    const cmd = seg.match(/invoke\s*!?\s*(?:<[^>]*>)?\s*\(\s*"([a-z0-9_]+)"/i);
    if (cmd) map.set(heads[i].name, cmd[1]);
  }
  return map;
}

const sourceText = read(join(SRC, "data", "source.ts"));
const tauriSource = methodMap(sourceText, "export const tauriSource");
const boardAnn = methodMap(sourceText, "export const boardAnnApi");

/* ---------------- 2. 递归收集一个入口可达的源文件 ---------------- */

function resolveImport(fromFile, spec, seen) {
  if (!spec.startsWith(".")) return null; // 第三方包（react / @tauri-apps/api）不跟
  const base = resolve(dirname(fromFile), spec);
  for (const cand of [base, `${base}.ts`, `${base}.tsx`, join(base, "index.ts"), join(base, "index.tsx")]) {
    if (existsSync(cand) && !seen.has(cand)) {
      try {
        if (!readFileSync(cand, "utf8").length) continue;
      } catch {
        continue;
      }
      return cand;
    }
  }
  return null;
}

function collect(entryRel) {
  const files = new Set();
  const stack = [join(SRC, entryRel)];
  while (stack.length) {
    const f = stack.pop();
    if (files.has(f) || !existsSync(f)) continue;
    files.add(f);
    const text = read(f);
    const re = /(?:from|import)\s*\(?\s*["']([^"']+)["']/g;
    let m;
    while ((m = re.exec(text))) {
      const r = resolveImport(f, m[1], files);
      if (r) stack.push(r);
    }
    // 动态 import("…")
    const dyn = /import\s*\(\s*["']([^"']+)["']\s*\)/g;
    while ((m = dyn.exec(text))) {
      const r = resolveImport(f, m[1], files);
      if (r) stack.push(r);
    }
  }
  return files;
}

/* ---------------- 3. 从文件集合里抽「用了哪些方法 / 哪些命令」 ---------------- */

/** 对象别名：`import { source as S }` 之类也要认 */
function objectAliases(files) {
  const aliases = { source: new Set(["source"]), boardAnnApi: new Set(["boardAnnApi"]) };
  for (const f of files) {
    const text = read(f);
    const re = /import\s*\{([^}]*)\}\s*from\s*["'][^"']*data\/source["']/g;
    let m;
    while ((m = re.exec(text))) {
      for (const part of m[1].split(",")) {
        const [orig, alias] = part.trim().split(/\s+as\s+/);
        if (!orig) continue;
        if (aliases[orig] && alias) aliases[orig].add(alias);
      }
    }
    // 本地再起个别名：const S = source;
    const re2 = /\bconst\s+([A-Za-z_$][\w$]*)\s*=\s*(source|boardAnnApi)\b/g;
    while ((m = re2.exec(text))) aliases[m[2]].add(m[1]);
  }
  return aliases;
}

/** 这两个文件是**命令定义表**本身（source.ts 里 157 个字面量、mock.ts 是浏览器假数据），
    扫它们会把「全部命令」当成「这个窗口在用」——必须排除，否则白名单等于没做。 */
const DEFINITIONS = new Set(["data/source.ts", "data/mock.ts"]);

function usedBy(files) {
  const aliases = objectAliases(files);
  for (const f of [...files]) {
    if (DEFINITIONS.has(relative(SRC, f).replace(/\\/g, "/"))) files.delete(f);
  }
  const all = new Set([...aliases.source, ...aliases.boardAnnApi]);
  /** 命令名 → 出处（文件:方法），用来给报告加注 */
  const commands = new Map();
  const unknown = [];
  for (const f of files) {
    const text = read(f);
    const rel = relative(SRC, f).replace(/\\/g, "/");

    // 3a. 直字面量：invoke("am_x") / invoke!<T>("am_x")
    const lit = /invoke\s*!?\s*(?:<[^>]*>)?\s*\(\s*"([a-z0-9_]+)"/gi;
    let m;
    while ((m = lit.exec(text))) {
      if (!commands.has(m[1])) commands.set(m[1], `${rel}`);
    }

    // 3b. 对象方法：<alias>.<method> —— 允许换行（`.boardIsTop()` 这种链式）
    for (const f2 of [["source", aliases.source], ["boardAnnApi", aliases.boardAnnApi]]) {
      const map = f2[0] === "source" ? tauriSource : boardAnn;
      for (const a of f2[1]) {
        const re = new RegExp(`\\b${a}\\s*\\n?\\s*\\.\\s*([A-Za-z_$][\\w$]*)`, "g");
        while ((m = re.exec(text))) {
          const cmd = map.get(m[1]);
          if (!cmd) {
            unknown.push(`${rel}: ${a}.${m[1]}（source.ts 里没有对应方法）`);
            continue;
          }
          if (!commands.has(cmd)) commands.set(cmd, `${rel}:${a}.${m[1]}`);
        }
      }
    }
  }
  return { commands, unknown };
}

/* ---------------- 4. 后端注册表 ---------------- */

function registeredCommands() {
  const text = read(MAIN_RS);
  const at = text.indexOf("generate_handler![");
  const end = text.indexOf("])", at);
  const body = text.slice(at, end);
  const set = new Set();
  for (const m of body.matchAll(/\b([a-z][a-z0-9_]*)\s*,/g)) set.add(m[1]);
  return set;
}

/* ---------------- 5. 输出 ---------------- */

const reg = registeredCommands();
const report = {};
const problems = [];

for (const [win, entries] of Object.entries(WINDOWS)) {
  const files = new Set();
  for (const e of entries) for (const f of collect(e)) files.add(f);
  const { commands, unknown } = usedBy(files);
  /* 字面量兜底：有些地方绕过了 `source`，直接拿原始 IPC 桥并起个别名
     （`Peek.tsx` 里 `const inv = rawInvoke(); void inv("am_peek_boot")`）。
     别名认不全就会漏，而漏的后果是「守卫上线后这个功能静默失效」；
     宁可多放行（多放行只是守卫弱一点，不会坏功能）。 */
  for (const f of files) {
    const rel = relative(SRC, f).replace(/\\/g, "/");
    if (DEFINITIONS.has(rel)) continue;
    const text = read(f);
    for (const cmd of reg) {
      if (!commands.has(cmd) && text.includes(`"${cmd}"`)) commands.set(cmd, `${rel}:字面量`);
    }
  }
  for (const u of unknown) problems.push(`${win} · ${u}`);
  const list = [...commands.keys()].sort();
  for (const c of list) if (!reg.has(c)) problems.push(`${win} · 命令 ${c} 没有在 main.rs 注册（脚本解析或前端有误）`);
  report[win] = { list, from: commands };
}

const asRust = () => {
  const lines = [];
  for (const [win, { list }] of Object.entries(report)) {
    lines.push(`/// ${win}`);
    lines.push(`const ${win}: &[&str] = &[`);
    for (const c of list) lines.push(`    "${c}",`);
    lines.push(`];`);
    lines.push("");
  }
  return lines.join("\n");
};

if (process.argv.includes("--rust")) {
  console.log(asRust());
} else if (process.argv.includes("--check")) {
  if (!existsSync(GUARD_RS)) {
    console.error(`✗ 找不到 ${relative(ROOT, GUARD_RS)}`);
    process.exit(1);
  }
  const rs = read(GUARD_RS);
  let bad = 0;
  for (const [win, { list }] of Object.entries(report)) {
    const at = rs.indexOf(`const ${win}: &[&str] = &[`);
    if (at < 0) {
      console.error(`✗ ipc_guard.rs 里没有 ${win} 白名单`);
      bad++;
      continue;
    }
    const block = rs.slice(at, rs.indexOf("];", at));
    const have = new Set([...block.matchAll(/"([a-z0-9_]+)"/g)].map((m) => m[1]));
    const miss = list.filter((c) => !have.has(c));
    const extra = [...have].filter((c) => !list.includes(c));
    if (miss.length) {
      console.error(`✗ ${win} 少了：${miss.join(", ")}`);
      bad++;
    }
    if (extra.length) {
      console.error(`! ${win} 多了（不一定是错，可能是即将删除的功能）：${extra.join(", ")}`);
    }
  }
  if (bad) process.exit(1);
  console.log("✓ 白名单与前端调用点一致");
} else {
  for (const [win, { list, from }] of Object.entries(report)) {
    console.log(`\n## ${win}（${list.length} 条）`);
    for (const c of list) console.log(`  ${c.padEnd(28)} ${from.get(c)}`);
  }
  if (problems.length) {
    console.log(`\n## 需要人看的 ${problems.length} 条`);
    for (const p of problems) console.log(`  ! ${p}`);
  }
}
