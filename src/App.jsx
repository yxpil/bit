import { useEffect, useState } from "react";
import { api } from "./api.js";
import { useTheme } from "./useTheme.js";
import { useLang } from "./i18n.js";
import logoUrl from "./assets/logo-64.png";
import TitleBar from "./components/TitleBar.jsx";
import ChatPage from "./pages/ChatPage.jsx";
import ToolsPage from "./pages/ToolsPage.jsx";
import MemoryPage from "./pages/MemoryPage.jsx";
import SkillsPage from "./pages/SkillsPage.jsx";
import AuditPage from "./pages/AuditPage.jsx";
import RemotePage from "./pages/RemotePage.jsx";
import AiSettingsPage from "./pages/AiSettingsPage.jsx";
import {
  IconChat,
  IconTool,
  IconMemory,
  IconSkill,
  IconAudit,
  IconGlobe,
  IconSettings,
  IconSun,
  IconMoon,
  IconInfo,
} from "./components/Icons.jsx";

// 页面登记表：key -> { label, icon, page }（label 为 i18n key，渲染处 t(label)）
const PAGES = {
  chat: { label: "nav.chat", icon: IconChat, page: ChatPage },
  tools: { label: "nav.tools", icon: IconTool, page: ToolsPage },
  memory: { label: "nav.memory", icon: IconMemory, page: MemoryPage },
  skills: { label: "nav.skills", icon: IconSkill, page: SkillsPage },
  audit: { label: "nav.audit", icon: IconAudit, page: AuditPage },
  remote: { label: "nav.remote", icon: IconGlobe, page: RemotePage },
  ai: { label: "nav.ai", icon: IconSettings, page: AiSettingsPage },
};

// 主功能：对话（AI 助手本体）单独置顶
const PRIMARY = "chat";
// 次级功能：分组列在下方
const SECONDARY = ["tools", "memory", "skills", "audit", "remote", "ai"];

export default function App() {
  const [tab, setTab] = useState("chat");
  const [stats, setStats] = useState(null);
  const { isDark, toggle } = useTheme();
  const { t, lang, toggleLang } = useLang();
  const [showAbout, setShowAbout] = useState(false);

  const refresh = () => api.overview().then(setStats).catch(() => {});
  useEffect(() => {
    refresh();
  }, [tab]);

  const current = PAGES[tab] || PAGES[PRIMARY];
  const Page = current.page;

  // 单个导航项渲染
  const NavItem = ({ k }) => {
    const { label, icon: Icon } = PAGES[k];
    const active = tab === k;
    return (
      <button
        onClick={() => setTab(k)}
        className={`flex w-full items-center gap-3 rounded-full px-3.5 py-2 text-[13px] font-medium transition-all duration-200 ${
          active
            ? "bg-neutral-900 text-white shadow-sm dark:bg-white dark:text-black"
            : "text-neutral-500 hover:bg-neutral-900/5 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-white/5 dark:hover:text-white"
        }`}
      >
        <Icon size={16} />
        {t(label)}
      </button>
    );
  };

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-neutral-100 text-neutral-900 dark:bg-black dark:text-neutral-100">
      <TitleBar />

      <div className="flex min-h-0 flex-1">
        {/* 侧边栏：毛玻璃 */}
        <aside className="flex w-56 shrink-0 flex-col border-r border-neutral-200/70 bg-white/60 backdrop-blur-md dark:border-neutral-800/70 dark:bg-black/40">
          {/* 品牌区 */}
          <div className="group flex items-center gap-2.5 border-b border-neutral-200/70 px-4 py-4 dark:border-neutral-800/70">
            <div className="flex h-9 w-9 items-center justify-center rounded-full bg-neutral-900 shadow-md transition-transform duration-300 ease-out group-hover:-rotate-6 group-hover:scale-105 dark:bg-white">
              <img
                src={logoUrl}
                alt="BIT"
                className="h-5 w-5 rounded-full bg-white dark:bg-black"
              />
            </div>
            <div className="min-w-0 leading-tight">
              <h1 className="text-base font-bold tracking-tight">BIT</h1>
              <p className="truncate text-[10px] text-neutral-400">{t("app.tagline")}</p>
            </div>
          </div>

          {/* 导航：对话主功能置顶，其余分组 */}
          <nav className="flex flex-1 flex-col gap-1 overflow-y-auto px-3 py-3">
            <NavItem k={PRIMARY} />

            <p className="mt-3 px-3.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-neutral-400">
              {t("nav.capabilities")}
            </p>
            {SECONDARY.map((k) => (
              <NavItem key={k} k={k} />
            ))}
          </nav>
        </aside>

        {/* 右侧：顶栏 + 主内容 */}
        <div className="flex min-w-0 flex-1 flex-col">
          {/* 顶栏：面包屑 + 右上角图标按钮组 */}
          <header className="flex h-14 shrink-0 items-center gap-4 border-b border-neutral-200/70 bg-white/60 px-6 backdrop-blur-md dark:border-neutral-800/70 dark:bg-black/40">
            <div className="flex min-w-0 flex-1 items-center gap-2">
              <span className="text-sm font-semibold text-neutral-900 dark:text-neutral-100">
                BIT
              </span>
              <span className="text-neutral-300 dark:text-neutral-700">/</span>
              <span className="truncate text-sm text-neutral-500 dark:text-neutral-400">
                {t(current.label)}
              </span>
            </div>
            <div className="flex items-center gap-1.5">
              <button
                onClick={toggle}
                title={t(isDark ? "app.switchLight" : "app.switchDark")}
                className="rounded-full p-2 text-neutral-600 transition-colors hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-900"
              >
                {isDark ? <IconSun size={17} /> : <IconMoon size={17} />}
              </button>
              <button
                onClick={() => setTab("ai")}
                title={t("nav.ai")}
                className="rounded-full p-2 text-neutral-600 transition-colors hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-900"
              >
                <IconSettings size={17} />
              </button>
              <button
                onClick={() => setShowAbout(true)}
                title={t("app.about")}
                className="rounded-full p-2 text-neutral-600 transition-colors hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-900"
              >
                <IconInfo size={17} />
              </button>
              <button
                onClick={toggleLang}
                title={lang === "zh" ? "切换到 English" : "Switch to 中文"}
                className="rounded-full p-2 text-neutral-600 transition-colors hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-900"
              >
                <span className="text-xs font-semibold">{lang === "zh" ? "EN" : "中"}</span>
              </button>
            </div>
          </header>

          {/* 主内容 */}
          <main className="min-h-0 flex-1 overflow-auto p-6">
            <Page onStats={refresh} stats={stats} />
          </main>
        </div>
      </div>

      {/* 关于弹窗 */}
      {showAbout && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"
          onClick={() => setShowAbout(false)}
        >
          <div
            className="card w-80 text-center"
            onClick={(e) => e.stopPropagation()}
          >
            <img src={logoUrl} alt="BIT" className="mx-auto mb-3 h-14 w-14 rounded-full" />
            <h2 className="text-lg font-bold">BIT</h2>
            <p className="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
              Agent Tool Hub · v0.1.3
            </p>
            <p className="mt-3 text-xs leading-relaxed text-neutral-500 dark:text-neutral-400">
              {t("app.aboutDesc")}
            </p>
            <button
              onClick={() => setShowAbout(false)}
              className="pill pill-hover mx-auto mt-4"
            >
              {t("common.ok")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
