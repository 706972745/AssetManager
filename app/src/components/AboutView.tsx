/**
 * 关于（**照 FontManager 的 AboutView 版式与动效**，owner 2026-11-17）。
 *
 * 结构：图标 → 名称 → 版本 + 许可 → 一句话说明 → 能力网格 → 技术栈 chips → 链接卡。
 * 动效（2026-11-22 照 FM 逐项搬，曲线直接用 `design/springs.ts` 的同一条）：
 *   · 整页淡入 + 上移 16px（spring）
 *   · 图标轻微缩放淡入、版本行延迟淡入
 *   · 功能格 8px 上移 + 40ms 交错（springSoft）
 *   · 链接卡 10px 上移 + 交错、hover 抬 1px、按下透明度 0.96
 * 颜色一律用 AM 令牌（强调色是 --ui-accent 紫，不是 FM 的蓝）。
 *
 * 链接（照 FontManager 两张卡的做法）：**本项目仓库** + **GitHub 主页**。
 * 两张卡图标必须不同 —— 都画 GitHub 字形会看着像重复（FontManager 那条注释同理）。
 * 插件仓库还没建，等建了再加第三张（用拼图图标）。
 *
 * 赞助区（2026-11-23）：链接卡下面「现在还早，刚好；」整块居中 —— 标题 / 三段正文 /
 * 一段弱化的声明 / 两张二维码（左支付宝、右微信）。二维码图是用户给的手机截图，
 * 已用 PIL 裁掉底部那行「…服务部 经营」、补白边后导出到 `assets/about/`。
 */
import { motion, useReducedMotion } from "motion/react";
import {
  ArrowUpRight,
  FolderOpen,
  Image as ImageIcon,
  Search,
  ShieldCheck,
  Sparkles,
  Star,
  Tags,
  UserRound,
  Zap,
} from "lucide-react";
import { spring, springSnappy, springSoft, staggerDelay } from "../design/springs";
import { useT, type TKey } from "../lib/i18n";
// 二维码（640×640 PNG，白边已烘焙进图里）。用 ESM import 让 vite 带 hash 打包，
// 不走 `public/` —— `public/` 里的文件不会被 tauri 的资源指纹处理。
import qrAlipay from "../assets/about/sponsor-alipay.png";
import qrWechat from "../assets/about/sponsor-wechat.png";

const FEATURES: { key: TKey; icon: React.ReactNode }[] = [
  { key: "about.featIndex", icon: <FolderOpen size={14} strokeWidth={1.5} /> },
  { key: "about.featWarmup", icon: <Zap size={14} strokeWidth={1.5} /> },
  { key: "about.featTags", icon: <Tags size={14} strokeWidth={1.5} /> },
  { key: "about.featRating", icon: <Star size={14} strokeWidth={1.5} /> },
  { key: "about.featSnapshot", icon: <Sparkles size={14} strokeWidth={1.5} /> },
  { key: "about.featBoard", icon: <ImageIcon size={14} strokeWidth={1.5} /> },
  { key: "about.featPalette", icon: <Search size={14} strokeWidth={1.5} /> },
  { key: "about.featDrag", icon: <ArrowUpRight size={14} strokeWidth={1.5} /> },
  { key: "about.featLocal", icon: <ShieldCheck size={14} strokeWidth={1.5} /> },
];

const STACK = ["Tauri v2", "Rust", "React 18", "TypeScript", "Vite", "SQLite"];

/** GitHub 字形（官方 mark）。两张卡里给「仓库」用，主页那张改用头像图标区分。 */
function GithubGlyph() {
  return (
    <svg width={16} height={16} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.55 0-.27-.01-1.17-.02-2.12-3.2.7-3.87-1.36-3.87-1.36-.52-1.33-1.28-1.68-1.28-1.68-1.04-.71.08-.7.08-.7 1.15.08 1.76 1.19 1.76 1.19 1.03 1.76 2.69 1.25 3.35.96.1-.75.4-1.25.72-1.54-2.55-.29-5.23-1.28-5.23-5.68 0-1.26.45-2.28 1.19-3.09-.12-.29-.52-1.46.11-3.05 0 0 .97-.31 3.17 1.18a11 11 0 0 1 5.77 0c2.2-1.49 3.16-1.18 3.16-1.18.63 1.59.24 2.76.12 3.05.74.81 1.18 1.83 1.18 3.09 0 4.41-2.69 5.38-5.25 5.67.41.35.77 1.05.77 2.12 0 1.53-.01 2.76-.01 3.14 0 .3.2.66.8.55A10.52 10.52 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z" />
    </svg>
  );
}

interface LinkItem {
  key: TKey;
  sub: string;
  url: string;
  icon: React.ReactNode;
}

/**
 * 是否关掉动效：系统偏好（motion 自带）或软件内开关（App 写到 html[data-motion]）。
 * 两条都看 —— 和分页里 CSS 令牌的「双轨」口径一致（tokens.css §减少动效）。
 */
function useNoMotion(): boolean {
  const system = useReducedMotion() === true;
  return system || document.documentElement.dataset.motion === "reduced";
}

const REPO = "https://github.com/706972745/AssetManager";
const HOME = "https://github.com/706972745";

const LINKS: LinkItem[] = [
  {
    key: "about.repo",
    sub: "github.com/706972745/AssetManager",
    url: REPO,
    icon: <GithubGlyph />,
  },
  {
    key: "about.githubHome",
    sub: "github.com/706972745",
    url: HOME,
    // 头像图标：和上面那张的 GitHub 字形拉开，一眼能分清"项目"和"人"
    icon: <UserRound size={16} strokeWidth={1.5} />,
  },
];

/** 一张链接卡：入场交错 + hover 抬 1px + 按下透明度 0.96（FM 同款）。 */
function LinkCard({
  item,
  delay,
  onOpenUrl,
}: {
  item: LinkItem;
  delay: number;
  onOpenUrl: (url: string) => void;
}) {
  const t = useT();
  const reduce = useNoMotion();
  return (
    <motion.button
      type="button"
      className="about-link"
      onClick={() => onOpenUrl(item.url)}
      initial={reduce ? false : { opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={reduce ? { duration: 0 } : { ...springSoft, delay }}
      whileHover={reduce ? undefined : { y: -1, transition: spring }}
      whileTap={reduce ? undefined : { opacity: 0.96, transition: springSnappy }}
    >
      <span className="about-link-icon">{item.icon}</span>
      <span className="about-link-label">{t(item.key)}</span>
      <ArrowUpRight size={14} strokeWidth={1.5} className="about-link-arrow" />
      <span className="about-link-sub mono">{item.sub}</span>
    </motion.button>
  );
}

/** 赞助二维码：左支付宝、右微信（顺序是 owner 定的，别调）。 */
const QR_CODES: { src: string; key: TKey }[] = [
  { src: qrAlipay, key: "about.stillEarly.alipay" },
  { src: qrWechat, key: "about.stillEarly.wechat" },
];

export function AboutView({ onOpenUrl }: { onOpenUrl: (url: string) => void }) {
  const t = useT();
  const reduce = useNoMotion();
  return (
    <motion.div
      className="about-view"
      initial={reduce ? false : { opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={reduce ? { duration: 0 } : spring}
    >
      <motion.img
        className="about-icon"
        src="/icon.png"
        alt=""
        draggable={false}
        initial={reduce ? false : { opacity: 0, scale: 0.92 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={reduce ? { duration: 0 } : { ...springSoft, delay: 0.04 }}
      />
      <h1 className="about-name">AssetManager</h1>
      <motion.div
        className="about-version tabular"
        initial={reduce ? false : { opacity: 0, y: 4 }}
        animate={{ opacity: 1, y: 0 }}
        transition={reduce ? { duration: 0 } : { ...springSoft, delay: 0.08 }}
      >
        {t("about.version", { version: __APP_VERSION__ })}
      </motion.div>
      <p className="about-desc">{t("about.desc")}</p>

      <div className="about-features" aria-label={t("about.featuresAria")}>
        {FEATURES.map((f, i) => (
          <motion.div
            key={f.key}
            className="about-feature"
            initial={reduce ? false : { opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={reduce ? { duration: 0 } : { ...springSoft, delay: staggerDelay(i) }}
          >
            <span className="about-feature-icon">{f.icon}</span>
            <span className="about-feature-text">{t(f.key)}</span>
          </motion.div>
        ))}
      </div>

      <div className="about-stack" aria-label={t("about.stackAria")}>
        {STACK.map((s) => (
          <span key={s} className="about-chip tabular">
            {s}
          </span>
        ))}
      </div>

      <div className="about-section">
        <span className="about-section-title">{t("about.sectionTitle")}</span>
      </div>
      <div className="about-links">
        {LINKS.map((l, i) => (
          <LinkCard key={l.url} item={l} delay={staggerDelay(i)} onOpenUrl={onOpenUrl} />
        ))}
      </div>

      {/* 赞助区：文案一字不改（owner 手写），标点（；——「」）照抄 */}
      <motion.section
        className="about-sponsor"
        aria-label={t("about.stillEarly.title")}
        initial={reduce ? false : { opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={reduce ? { duration: 0 } : { ...springSoft, delay: 0.12 }}
      >
        <h2 className="about-sponsor-title">{t("about.stillEarly.title")}</h2>
        <p className="about-sponsor-copy">{t("about.stillEarly.p1")}</p>
        <p className="about-sponsor-copy">{t("about.stillEarly.p2")}</p>
        <p className="about-sponsor-copy">{t("about.stillEarly.p3")}</p>
        <p className="about-sponsor-note">{t("about.stillEarly.disclaimer")}</p>
        <div className="about-sponsor-qr">
          {QR_CODES.map((q) => (
            <figure key={q.key} className="about-sponsor-card">
              <img src={q.src} alt="" draggable={false} />
              <figcaption className="about-sponsor-cap">{t(q.key)}</figcaption>
            </figure>
          ))}
        </div>
      </motion.section>
    </motion.div>
  );
}
