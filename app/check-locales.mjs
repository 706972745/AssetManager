/**
 * check-locales.mjs —— 校验 `src/locales/*.json`（照 FontManager 的同名脚本）。
 *
 * `en.json` 是源：其它语言缺 key 会**回落英文**（运行时不会白屏），
 * 所以缺 key 只报**覆盖率**、不算失败 —— 允许分批翻译。
 * 真正算失败的是两类会让界面出错的：
 *   1. 占位符对不上（`{percent}` 写成 `{percent }`，插值会原样显示大括号）
 *   2. 多出来的 key（拼错了，永远不会有东西用它 —— 等于这段翻译白写）
 *
 * 用法：
 *   node check-locales.mjs          # 报覆盖率
 *   node check-locales.mjs --strict # 覆盖率不到 100% 也返回非 0（发版前用）
 */
import { readFileSync, readdirSync } from "fs";
import { join } from "path";

const DIR = "src/locales";
const CATEGORIES = ["zero", "one", "two", "few", "many", "other"];
const STRICT = process.argv.includes("--strict");

const read = (f) => JSON.parse(readFileSync(join(DIR, f), "utf8"));
const placeholders = (s) =>
  [...String(s).matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");

/** 复数 key（`x_one` / `x_other`）按 base 名归一：各语言的复数类别不一样，不该算缺失 */
function base(key) {
  const at = key.lastIndexOf("_");
  if (at < 0) return key;
  return CATEGORIES.includes(key.slice(at + 1)) ? key.slice(0, at) : key;
}

const en = read("en.json");
const enBases = new Set(Object.keys(en).map(base));

let failed = false;
const rows = [];

for (const file of readdirSync(DIR).filter((f) => f.endsWith(".json") && f !== "en.json")) {
  const dict = read(file);
  const keys = Object.keys(dict);
  const bases = new Set(keys.map(base));

  const missing = [...enBases].filter((k) => !bases.has(k));
  const extra = [...bases].filter((k) => !enBases.has(k));
  const cover = ((enBases.size - missing.length) / enBases.size) * 100;

  rows.push({ file, total: bases.size, missing: missing.length, cover });

  /* 占位符：只比同名的 key（复数按 base 找 en 里任意一个同类） */
  for (const k of keys) {
    const ref = en[k] ?? en[k.replace(/_(zero|one|two|few|many|other)$/, "_other")];
    if (ref === undefined) continue;
    if (placeholders(ref) !== placeholders(dict[k])) {
      console.error(
        `✗ ${file} "${k}" 占位符不一致：en={${placeholders(ref)}} vs 本语言={${placeholders(dict[k])}}`,
      );
      failed = true;
    }
  }
  if (extra.length) {
    console.error(`✗ ${file} 有 en 里不存在的 key：${extra.join(", ")}`);
    failed = true;
  }
}

console.log(`\nen.json：${Object.keys(en).length} 条（${enBases.size} 个 key）\n`);
for (const r of rows.sort((a, b) => a.cover - b.cover)) {
  const bar = "█".repeat(Math.round(r.cover / 5)).padEnd(20, "·");
  console.log(`  ${r.file.padEnd(11)} ${bar} ${r.cover.toFixed(1).padStart(5)}%  缺 ${r.missing} 条`);
}

const lowest = Math.min(...rows.map((r) => r.cover));
if (STRICT && lowest < 100) {
  console.error(`\n✗ --strict：最低覆盖率 ${lowest.toFixed(1)}%，要求 100%`);
  process.exit(1);
}
if (failed) {
  console.error("\n✗ 有占位符不一致或多余 key（见上）");
  process.exit(1);
}
console.log("\n✓ 占位符与 key 集合都一致；缺失的按覆盖率显示（运行时回落英文）\n");
