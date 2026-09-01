import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
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
  IconPlus,
  IconShirt,
} from "./components/Icons.jsx";

// 调色盘预设：null = 恢复默认黑白
const ACCENT_PRESETS = [
  { color: null, label: "default" },
  { color: "#e11d48", label: "rose" },
  { color: "#ea580c", label: "orange" },
  { color: "#f59e0b", label: "amber" },
  { color: "#16a34a", label: "green" },
  { color: "#0891b2", label: "cyan" },
  { color: "#2563eb", label: "blue" },
  { color: "#7c3aed", label: "violet" },
  { color: "#db2777", label: "pink" },
];

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
  const { isDark, toggle, accent, setAccent } = useTheme();
  const { t, lang, toggleLang } = useLang();
  const [showAbout, setShowAbout] = useState(false);
  const [showPalette, setShowPalette] = useState(false);
  const [customColor, setCustomColor] = useState("#2563eb");
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  const refresh = () => api.overview().then(setStats).catch(() => {});
  useEffect(() => {
    refresh();
  }, [tab]);

  const current = PAGES[tab] || PAGES[PRIMARY];
  const Page = current.page;

  // 图标导航项：圆形小圆片（悬停显示名称），把宽度留给内容区
  const NavItem = ({ k }) => {
    const { label, icon: Icon } = PAGES[k];
    const active = tab === k;
    return (
      <button
        onClick={() => setTab(k)}
        title={t(label)}
        className={`mx-auto flex h-10 w-10 items-center justify-center rounded-full transition-all duration-200 hover:scale-105 active:scale-95 ${
          active
            ? "accent-solid shadow-sm"
            : "text-neutral-500 hover:bg-neutral-900/5 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-white/5 dark:hover:text-white"
        }`}
      >
        <Icon size={18} />
      </button>
    );
  };

  // 新对话小加号：位于图标栏顶部对话按钮下方，点击切到对话页并新建
  const NewChatBtn = () => (
    <button
      onClick={() => {
        setTab(PRIMARY);
        window.dispatchEvent(new CustomEvent("bit-new-session"));
      }}
      title={t("chat.newChat")}
      className="mx-auto flex h-10 w-10 items-center justify-center rounded-full border border-dashed border-neutral-300 text-neutral-500 transition-colors hover:border-neutral-900 hover:text-neutral-900 dark:border-neutral-700 dark:hover:border-white dark:hover:text-white"
    >
      <IconPlus size={16} />
    </button>
  );

  // 栏底圆形小按钮（主题 / 语言 / 关于）
  const RailBtn = ({ onClick, title, children }) => (
    <button
      onClick={onClick}
      title={title}
      className="flex w-full items-center justify-center rounded-xl py-2 text-neutral-500 transition-colors hover:bg-neutral-900/5 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-white/5 dark:hover:text-white"
    >
      {children}
    </button>
  );

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-neutral-100 text-neutral-900 dark:bg-black dark:text-neutral-100">
      <TitleBar />

      <div className="flex min-h-0 flex-1">
        {/* 图标侧栏：无分隔线，与窗口背景融为一体 */}
        <aside className="flex w-14 shrink-0 flex-col items-center gap-1 py-3">
          {/* 导航：对话主功能置顶，其余分组 */}
          <nav className="flex w-full flex-1 flex-col gap-1 px-1.5 pt-1">
            <NavItem k={PRIMARY} />
            <NewChatBtn />
            <div className="mx-auto my-2 h-px w-6 bg-neutral-200 dark:bg-neutral-800" />
            {SECONDARY.map((k) => (
              <NavItem key={k} k={k} />
            ))}
          </nav>

          {/* 栏底：换主题色 / 明暗 / 语言 / 关于 */}
          <div className="relative flex w-full flex-col gap-0.5 px-1.5">
            <RailBtn onClick={() => setShowPalette((v) => !v)} title={t("theme.accent")}>
              <IconShirt size={17} />
            </RailBtn>
            <RailBtn onClick={toggle} title={t(isDark ? "app.switchLight" : "app.switchDark")}>
              {isDark ? <IconSun size={17} /> : <IconMoon size={17} />}
            </RailBtn>
            <RailBtn onClick={toggleLang} title={lang === "zh" ? "切换到 English" : "Switch to 中文"}>
              <span className="text-xs font-semibold">{lang === "zh" ? "EN" : "中"}</span>
            </RailBtn>
            <RailBtn onClick={() => setShowAbout(true)} title={t("app.about")}>
              <IconInfo size={17} />
            </RailBtn>
          </div>
        </aside>

        {/* 主内容：所有页面常驻挂载（display 切换），保证对话页的
            执行状态 / 等待队列 / 审批监听在切换页面后不丢失 */}
        <main className="min-h-0 flex-1 overflow-hidden">
          {Object.entries(PAGES).map(([k, { page: Page }]) => (
            <div
              key={k}
              className={`h-full overflow-auto ${k === PRIMARY ? "p-3" : "p-6"}`}
              style={{ display: tab === k ? "block" : "none" }}
            >
              <Page onStats={refresh} stats={stats} visible={tab === k} />
            </div>
          ))}
        </main>
      </div>

      {/* 调色盘弹层：预设 + 自定义取色器，点击外部关闭 */}
      {showPalette && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setShowPalette(false)} />
          <div className="anim-rise card fixed bottom-14 left-16 z-50 w-60 p-4">
            <p className="mb-3 text-xs font-semibold text-neutral-500 dark:text-neutral-400">
              {t("theme.accent")}
            </p>
            <div className="mb-3 flex flex-wrap gap-2">
              {ACCENT_PRESETS.map(({ color, label }) => (
                <button
                  key={label}
                  title={color ? label : t("theme.reset")}
                  onClick={() => setAccent(color || "")}
                  className={`h-7 w-7 rounded-full border-2 transition-transform duration-200 hover:scale-110 ${
                    (accent || "") === (color || "") && (accent || label) === (color || label)
                      ? "scale-110 border-neutral-900 dark:border-white"
                      : "border-transparent"
                  }`}
                  style={{
                    background:
                      color ||
                      "conic-gradient(#ef4444,#f59e0b,#22c55e,#06b6d4,#6366f1,#d946ef,#ef4444)",
                  }}
                />
              ))}
            </div>
            <div className="flex items-center gap-2">
              <input
                type="color"
                value={customColor}
                onChange={(e) => {
                  setCustomColor(e.target.value);
                  setAccent(e.target.value);
                }}
                className="h-8 w-12 cursor-pointer rounded-lg border border-neutral-200 bg-transparent p-0.5 dark:border-neutral-700"
              />
              <span className="text-xs text-neutral-400">{t("theme.custom")}</span>
            </div>
          </div>
        </>
      )}

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
              Agent Tool Hub{appVersion ? ` · v${appVersion}` : ""}
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
