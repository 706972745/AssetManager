import { StrictMode, Suspense, lazy } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./styles/tokens.css";
import "./styles/app.css";
import { installMomentumScroll } from "./lib/momentumScroll";
import { installScrollHot } from "./lib/scrollHot";
import { installPerfProbe } from "./lib/perf";
import { hydrateLocalePref, isLocalePref, prepareLocale } from "./lib/i18n";
import { loadPrefs } from "./types";

/* P1-2：四个窗口组件**按窗口懒加载**。
   以前 index.html 一个入口静态 import 了 App / Peek / Snip / Board 四份代码：
   贴图窗只是为了画一张图，也要解析整个主窗（含 9 份语言包）。现在各自一个 chunk，
   贴图窗只付 Peek 的钱。不拆独立 html 入口是因为那要改 Rust 侧的窗口 URL 与构建配置。 */
const App = lazy(() => import("./App"));
const Peek = lazy(() =>
  import("./components/Peek").then((m) => ({ default: m.Peek }))
);
const Snip = lazy(() =>
  import("./components/Snip").then((m) => ({ default: m.Snip }))
);
const Board = lazy(() =>
  import("./components/Board").then((m) => ({ default: m.Board }))
);

/* 同一个前端包跑三种窗口：主窗（App）、贴图（Peek）、截图覆盖层（Snip）。
   用**初始化脚本注入的全局变量**区分（Rust 侧 `initialization_script` 写 __AM_PEEK__）：
   URL 里带 `#/peek` 或 `?peek=1` 都会让新窗口导航失败（WebviewUrl::App 把整串当路径）。 */
const isPeek = !!(window as unknown as { __AM_PEEK__?: unknown }).__AM_PEEK__;
const isSnip = !!(window as unknown as { __AM_SNIP__?: unknown }).__AM_SNIP__;
const isBoard = !!(window as unknown as { __AM_BOARD__?: unknown }).__AM_BOARD__;

/* 主窗在下面的 boot 里要先 await 语言包，App 的 chunk 又是动态 import ——
   不预取的话这两个请求是串行的。这里只是提前发起，模块表会复用同一个 promise，
   `lazy(() => import("./App"))` 拿到的还是同一份。贴图 / 截图 / 板窗**不**预取。 */
if (!isPeek && !isSnip && !isBoard) void import("./App").catch(() => {});

/* 语言必须在**第一次渲染之前**定下来（0.3.30）：放进 App 的 effect 里做，
   首帧会先按浏览器语言画一遍、再跳成用户选的语言，屏幕上闪一下。
   偏好还是以 prefs 为准（设置页写的是它），i18n 自己那份 localStorage 只是缓存。 */
if (!isPeek && !isSnip && !isBoard) {
  const saved = loadPrefs().locale;
  if (isLocalePref(saved)) hydrateLocalePref(saved);
}

/* 惯性滚动（照 FontManager）：Windows 上的 WebView2 滚轮没有惯性，松手就停。
   这里统一接管成「继续滑行 + 指数减速」。自带边界（到底交还原生、输入框 /
   Ctrl 滚轮 / 横向滚动一律放行、减动效时关闭）。 */
/* 贴图窗口**不装惯性滚动**：它没有可滚动的内容，
   而惯性滚动会把一次滚轮拆成十几帧合成事件 —— 我们的"滚轮缩放窗口"会被放大十几倍。 */
if (!isPeek && !isSnip && !isBoard) installMomentumScroll();

/* 滚动条「靠近变粗」：指针进容器右缘 14px 内就给那个容器挂 .scroll-hot。
   ::-webkit-scrollbar 只认宿主元素的类，所以必须逐个容器挂，不能挂在父级。 */
if (!isPeek && !isSnip && !isBoard) installScrollHot();

/* 帧预算埋点（P3 T43）：默认关，`?perf=1` 或 localStorage["am.perf"]="1" 才真的开始采样。
   永远挂上 window.__amPerf() 供自测脚本读一份当前统计。 */
installPerfProbe();

/* 关掉 WebView2 自带的右键菜单（返回 / 刷新 / 另存为 / 打印）：
   软件自己的菜单由 React 处理。capture 阶段只 preventDefault、不 stopPropagation，
   输入框保留系统编辑菜单（复制 / 粘贴 / 输入法）。 */
document.addEventListener(
  "contextmenu",
  (e) => {
    const t = e.target as HTMLElement | null;
    if (t && t.closest("input, textarea, [contenteditable='true']")) return;
    e.preventDefault();
  },
  true,
);

/* 语言包改成动态 import 之后（P1-2），首次渲染前要先把**当前那一份**取回来，
   否则首帧会把 en 兜底画出来再跳成用户选的语言。取不到（chunk 损坏等）就直接用 en，
   界面不许因此空着。 */
async function boot() {
  await prepareLocale();
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <Suspense fallback={null}>
        {isSnip ? <Snip /> : isBoard ? <Board /> : isPeek ? <Peek /> : <App />}
      </Suspense>
    </StrictMode>
  );
}

void boot();
