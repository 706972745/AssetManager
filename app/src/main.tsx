import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./styles/tokens.css";
import "./styles/app.css";
import App from "./App";
import { installMomentumScroll } from "./lib/momentumScroll";
import { installScrollHot } from "./lib/scrollHot";

/* 惯性滚动（照 FontManager）：Windows 上的 WebView2 滚轮没有惯性，松手就停。
   这里统一接管成「继续滑行 + 指数减速」。自带边界（到底交还原生、输入框 /
   Ctrl 滚轮 / 横向滚动一律放行、减动效时关闭）。 */
installMomentumScroll();

/* 滚动条「靠近变粗」：指针进容器右缘 14px 内就给那个容器挂 .scroll-hot。
   ::-webkit-scrollbar 只认宿主元素的类，所以必须逐个容器挂，不能挂在父级。 */
installScrollHot();

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

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
