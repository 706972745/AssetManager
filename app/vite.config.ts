import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/* 版本号只有一个来源：package.json。原来的「关于」面板写死了 0.10.0 开发版，
   连着四个版本忘了改 —— 现在编译期注进来，改 package.json 就够。 */
const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8")
);

export default defineConfig({
  plugins: [react()],
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  clearScreen: false,
  server: { port: 1430, strictPort: true },
  build: {
    target: "chrome110",
    outDir: "dist",
    sourcemap: false,
  },
});
