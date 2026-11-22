import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/* 版本号只有一个来源：package.json。原来的「关于」面板写死了 0.10.0 开发版，
   连着四个版本忘了改 —— 现在编译期注进来，改 package.json 就够。 */
const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8")
);

/* ESM 配置里没有 `__dirname`（package.json 是 type: module）：
   用 import.meta.url 反推项目根，fileURLToPath 顺带处理中文路径的转义。 */
const root = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  plugins: [react()],
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  clearScreen: false,
  server: { port: 1430, strictPort: true },
  build: {
    target: "chrome110",
    outDir: "dist",
    sourcemap: false,
    /* 启动页是**独立的第二个 HTML 入口**（不是 React 应用的一部分）：
       它要能快速出首帧，所以自己一份入口、自己的脚本，不进主 bundle。
       少了这一项，dist/ 里就只有 index.html，Tauri 的 splash 窗口会 404。 */
    rollupOptions: {
      input: {
        main: resolve(root, "index.html"),
        splash: resolve(root, "splash.html"),
        /* 托盘自绘菜单（右键托盘图标弹出）：**独立入口**，与 splash 同一路数 ——
           不加载 index.html（否则整个 React 应用再跑一遍）。窗口由 Rust 侧按需
           创建（main.rs 的 build_tray_menu_window）。 */
        tray: resolve(root, "tray.html"),
        /* 安装器 / 卸载器的界面（路线 C）：**独立入口**，不并进主 bundle。
           主程序带 `--installer-ui` / `--uninstall-ui` 启动时，Rust 侧
           （src-tauri/src/setup_ui.rs）直接用 `WebviewUrl::App("setup.html")` 打开它；
           少了这一项，dist/ 里没有 setup.html，装卸界面会 404 成白屏。 */
        setup: resolve(root, "setup.html"),
      },
    },
  },
});
