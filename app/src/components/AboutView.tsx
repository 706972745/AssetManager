/**
 * 关于（**照 FontManager 的 AboutView 版式**，owner 2026-11-17）。
 *
 * 结构：图标 → 名称 → 版本 + 许可 → 一句话说明 → 能力网格 → 技术栈 chips → 链接卡。
 * 链接：现阶段只有主仓库，插件仓库还没建，所以只留一张卡（owner：先用 GitHub 主页占位）。
 */
import {
  ArrowUpRight,
  FolderOpen,
  Image as ImageIcon,
  Search,
  ShieldCheck,
  Sparkles,
  Star,
  Tags,
  Zap,
} from "lucide-react";

const FEATURES: { label: string; icon: React.ReactNode }[] = [
  { label: "索引式素材库：不搬动原文件", icon: <FolderOpen size={14} strokeWidth={1.5} /> },
  { label: "缩略图预热与惰性加载", icon: <Zap size={14} strokeWidth={1.5} /> },
  { label: "标签：手动 + 自动 + 分组配色", icon: <Tags size={14} strokeWidth={1.5} /> },
  { label: "评分 / 备注 / 来源 URL", icon: <Star size={14} strokeWidth={1.5} /> },
  { label: "快照：记住你选中的那批", icon: <Sparkles size={14} strokeWidth={1.5} /> },
  { label: "灵感板：无限画布拼图", icon: <ImageIcon size={14} strokeWidth={1.5} /> },
  { label: "命令面板与全套快捷键", icon: <Search size={14} strokeWidth={1.5} /> },
  { label: "拖出即用：PS / 资源管理器", icon: <ArrowUpRight size={14} strokeWidth={1.5} /> },
  { label: "本地优先，不上传任何文件", icon: <ShieldCheck size={14} strokeWidth={1.5} /> },
];

const STACK = ["Tauri v2", "Rust", "React 18", "TypeScript", "Vite", "SQLite"];

const REPO = "https://github.com/706972745";

export function AboutView({ onOpenUrl }: { onOpenUrl: (url: string) => void }) {
  return (
    <div className="about-view">
      <img className="about-icon" src="/icon.png" alt="" draggable={false} />
      <h1 className="about-name">AssetManager</h1>
      <div className="about-version">
        {`版本 ${__APP_VERSION__} · GPL-3.0`}
      </div>
      <p className="about-desc">
        索引式素材管理器：只读你的文件夹、不搬动原始文件。找图、打标签、拼灵感板、拖进设计软件，
        都在一个窗口里完成。
      </p>

      <div className="about-features" aria-label="能力一览">
        {FEATURES.map((f) => (
          <div key={f.label} className="about-feature">
            <span className="about-feature-icon">{f.icon}</span>
            <span className="about-feature-text">{f.label}</span>
          </div>
        ))}
      </div>

      <div className="about-stack" aria-label="技术栈">
        {STACK.map((s) => (
          <span key={s} className="about-chip">
            {s}
          </span>
        ))}
      </div>

      <div className="about-section">
        <span className="about-section-title">本项目仓库</span>
      </div>
      <div className="about-links">
        <button className="about-link" onClick={() => onOpenUrl(REPO)}>
          <span className="about-link-icon">
            <svg width={16} height={16} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.55 0-.27-.01-1.17-.02-2.12-3.2.7-3.87-1.36-3.87-1.36-.52-1.33-1.28-1.68-1.28-1.68-1.04-.71.08-.7.08-.7 1.15.08 1.76 1.19 1.76 1.19 1.03 1.76 2.69 1.25 3.35.96.1-.75.4-1.25.72-1.54-2.55-.29-5.23-1.28-5.23-5.68 0-1.26.45-2.28 1.19-3.09-.12-.29-.52-1.46.11-3.05 0 0 .97-.31 3.17 1.18a11 11 0 0 1 5.77 0c2.2-1.49 3.16-1.18 3.16-1.18.63 1.59.24 2.76.12 3.05.74.81 1.18 1.83 1.18 3.09 0 4.41-2.69 5.38-5.25 5.67.41.35.77 1.05.77 2.12 0 1.53-.01 2.76-.01 3.14 0 .3.2.66.8.55A10.52 10.52 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z" />
            </svg>
          </span>
          <span className="about-link-label">GitHub 主页</span>
          <ArrowUpRight size={14} strokeWidth={1.5} className="about-link-arrow" />
          <span className="about-link-sub mono">github.com/706972745</span>
        </button>
      </div>
    </div>
  );
}
