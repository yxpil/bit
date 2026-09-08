import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
import ThemePage from "./pages/ThemePage.jsx";
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
// 页面登记表：key -> { label, icon, page }（label 为 i18n key，渲染处 t(label)）
const PAGES = {
  chat: { label: "nav.chat", icon: IconChat, page: ChatPage },
  tools: { label: "nav.tools", icon: IconTool, page: ToolsPage },
  memory: { label: "nav.memory", icon: IconMemory, page: MemoryPage },
  skills: { label: "nav.skills", icon: IconSkill, page: SkillsPage },
  audit: { label: "nav.audit", icon: IconAudit, page: AuditPage },
  remote: { label: "nav.remote", icon: IconGlobe, page: RemotePage },
  ai: { label: "nav.ai", icon: IconSettings, page: AiSettingsPage },
  theme: { label: "nav.theme", icon: IconShirt, page: ThemePage },
};

// 主功能：对话（AI 助手本体）单独置顶
const PRIMARY = "chat";
// 次级功能：分组列在下方
const SECONDARY = ["tools", "memory", "skills", "audit", "remote", "ai", "theme"];

// 体积/速度友好格式化：B / KB / MB / GB
const fmtBytes = (b) => {
  const n = Number(b) || 0;
  if (n <= 0) return "0 B";
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(2)} GB`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${n} B`;
};

export default function App() {
  const [tab, setTab] = useState("chat");
  const [stats, setStats] = useState(null);
  const { isDark, toggle } = useTheme();
  const { t, lang, toggleLang } = useLang();
  const [showAbout, setShowAbout] = useState(false);
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
    // 前端挂载信号：供 CI 冒烟检测「渲染期异常导致黑屏」（v0.5.14 elevErr 事故的回归闸门）。
    // 树挂载成功才会执行本 effect；任何页面渲染崩溃都会让它缺席，Windows 冒烟据此判失败
    invoke("ui_mounted").catch(() => {});
  }, []);

  // 安卓版扫码下载（bit-mobile 仓库 Releases）：打开「关于」时懒加载二维码 SVG（后端离线渲染）
  const [androidQr, setAndroidQr] = useState("");
  const ANDROID_URL = "https://github.com/yxpil/bit-mobile/releases/latest";
  useEffect(() => {
    if (showAbout && !androidQr) {
      invoke("qr_svg_url", { url: ANDROID_URL }).then(setAndroidQr).catch(() => {});
    }
  }, [showAbout, androidQr]);

  // ——「关于」弹窗里的版本更新面板 ——
  // upd = check_updates 结果；dl = 下载进度（后端广播）；checking / updErr 为过程态
  const [upd, setUpd] = useState(null); // { current, latest, has_update, notes, downloaded }
  const [checking, setChecking] = useState(false);
  const [updErr, setUpdErr] = useState("");
  const [dl, setDl] = useState(null); // { downloaded, total, speed }

  // 弹窗打开期间订阅后端广播：update-progress 推进进度条 / update-state(downloaded) 收尾
  useEffect(() => {
    if (!showAbout) return;
    const unP = listen("update-progress", (e) => {
      const p = e.payload || {};
      if (p.state === "downloading") {
        setDl({ downloaded: p.downloaded || 0, total: p.total || 0, speed: p.speed || 0 });
      }
    });
    const unS = listen("update-state", (e) => {
      const s = e.payload || {};
      if (s.state === "downloaded") {
        setUpd((prev) => ({ ...(prev || {}), downloaded: true }));
        setDl(null);
      }
    });
    return () => {
      unP.then((f) => f()).catch(() => {});
      unS.then((f) => f()).catch(() => {});
    };
  }, [showAbout]);

  // 主动检查更新：面板右上按钮触发
  const doCheckUpdate = async () => {
    setChecking(true);
    setUpdErr("");
    setDl(null);
    try {
      setUpd(await api.checkUpdates());
    } catch (e) {
      setUpdErr(typeof e === "string" ? e : String(e));
    } finally {
      setChecking(false);
    }
  };

  // 手动下载更新：进度/完成由上面的事件监听推进（失败回显到面板）
  const doDownloadUpdate = async () => {
    setDl({ downloaded: 0, total: 0, speed: 0 });
    setUpdErr("");
    try {
      const r = await api.updateDownload();
      if (r?.state === "downloaded") {
        setUpd((prev) => ({ ...(prev || {}), downloaded: true }));
        setDl(null);
      }
    } catch (e) {
      setUpdErr(typeof e === "string" ? e : String(e));
      setDl(null);
    }
  };

  // 远程服务端口被占用自动切换：事件实时推送；启动时事件可能早于 JS 监听丢失，
  // 故挂载时再查一次 get_remote_status 兜底（switched_from 有值即展示提示条）
  const [portSwitched, setPortSwitched] = useState(null); // { from, to }
  useEffect(() => {
    invoke("get_remote_status")
      .then((s) => {
        if (s?.switched_from) setPortSwitched({ from: s.switched_from, to: s.addr });
      })
      .catch(() => {});
    const un = listen("remote-port-switched", (e) => {
      const p = e.payload || {};
      if (p.from && p.to) setPortSwitched({ from: p.from, to: p.addr || `:${p.to}` });
    });
    return () => un.then((f) => f()).catch(() => {});
  }, []);

  // 全局捕获右键：屏蔽 WebView 默认菜单；输入框保留原生气泡（复制/粘贴仍可用）
  useEffect(() => {
    const onCtx = (e) => {
      const el = e.target;
      const editable =
        el instanceof HTMLInputElement ||
        el instanceof HTMLTextAreaElement ||
        el?.isContentEditable;
      if (!editable) e.preventDefault();
    };
    window.addEventListener("contextmenu", onCtx, true);
    return () => window.removeEventListener("contextmenu", onCtx, true);
  }, []);

  // 隐藏彩蛋菜单：键入 ↑↓↑↓←→←→BABA（或经典魂斗罗序列 ↑↑↓↓←→←→BA）打开 yxpil.com
  useEffect(() => {
    const seqUser = [
      "ArrowUp", "ArrowDown", "ArrowUp", "ArrowDown",
      "ArrowLeft", "ArrowRight", "ArrowLeft", "ArrowRight", "b", "a", "b", "a",
    ];
    const seqKonami = [
      "ArrowUp", "ArrowUp", "ArrowDown", "ArrowDown",
      "ArrowLeft", "ArrowRight", "ArrowLeft", "ArrowRight", "b", "a",
    ];
    const buf = [];
    const matches = (seq) =>
      seq.length <= buf.length && buf.slice(-seq.length).every((k, i) => k === seq[i]);
    const onKey = (e) => {
      // Cmd+Q / Ctrl+Q：走正常退出链路（notify guardian + silent update）
      // macOS Tauri RunEvent::ExitRequested 不触发 Cmd+Q（issue #9198），
      // 必须前端拦截手动调 quit_app，否则 guardian 以为主进程意外死亡会拉起来
      if ((e.metaKey || e.ctrlKey) && (e.key === "q" || e.key === "Q")) {
        e.preventDefault();
        api.quitApp();
        return;
      }
      buf.push(e.key.length === 1 ? e.key.toLowerCase() : e.key);
      if (buf.length > seqUser.length) buf.shift();
      if (matches(seqUser) || matches(seqKonami)) {
        buf.length = 0;
        invoke("open_external", { url: "https://yxpil.com" }).catch(() => {});
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const refresh = () => api.overview().then(setStats).catch(() => {});
  useEffect(() => {
    refresh();
  }, [tab]);

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
      className="mx-auto flex h-10 w-10 items-center justify-center rounded-full text-neutral-400 transition-colors hover:bg-neutral-200/60 hover:text-neutral-900 dark:hover:bg-neutral-800/60 dark:hover:text-white"
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
    <div className="flex h-screen flex-col overflow-hidden text-neutral-900 dark:text-neutral-100">
      <TitleBar />

      {/* 端口切换提示条：远程端口被占用已自动切换（可关闭，纯色不透明） */}
      {portSwitched && (
        <div className="flex shrink-0 items-center justify-center gap-3 bg-neutral-900 px-4 py-1.5 text-xs text-white">
          <span>
            {t("remote.portSwitched")}
            <span className="font-mono font-semibold">{portSwitched.to}</span>
          </span>
          <button
            onClick={() => setPortSwitched(null)}
            title={t("common.close")}
            className="rounded px-1 text-neutral-400 transition-colors hover:text-white"
          >
            ✕
          </button>
        </div>
      )}

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

          {/* 栏底：明暗切换 / 语言 / 关于 */}
          <div className="relative flex w-full flex-col gap-0.5 px-1.5">
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

      {/* 关于弹窗：版本信息 + 主动更新（检查 / 下载进度 / 重启更新） */}
      {showAbout && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"
          onClick={() => setShowAbout(false)}
        >
          <div
            className="card max-h-[calc(100vh-32px)] w-[380px] max-w-[calc(100vw-24px)] overflow-y-auto text-center"
            onClick={(e) => e.stopPropagation()}
          >
            <img src={logoUrl} alt="BIT" className="mx-auto mb-2 h-14 w-14 rounded-full" />
            <div className="flex items-center justify-center gap-2">
              <h2 className="text-lg font-bold leading-tight">BIT</h2>
              {appVersion && (
                <span className="chip px-2 py-0 font-mono text-[10px]">v{appVersion}</span>
              )}
            </div>
            <p className="mt-1 text-[11px] text-neutral-500 dark:text-neutral-400">{t("app.tagline")}</p>
            <p className="mx-auto mt-2 max-w-[330px] text-[11px] leading-relaxed text-neutral-400">
              {t("app.aboutDesc")}
            </p>

            {/* 版本更新面板：主动检查 / 下载进度 + 速度 / 重启更新 */}
            <div className="mt-4 rounded-2xl border border-neutral-200 bg-white/70 p-3 text-left shadow-sm dark:border-neutral-800 dark:bg-neutral-950/40">
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs font-semibold text-neutral-700 dark:text-neutral-200">
                  {t("app.update")}
                </span>
                <button
                  onClick={doCheckUpdate}
                  disabled={checking || !!dl}
                  className={`accent-solid inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-[11px] font-semibold transition select-none disabled:cursor-not-allowed disabled:opacity-50 ${
                    checking ? "" : "hover:brightness-110"
                  }`}
                >
                  {checking && (
                    <span className="h-2.5 w-2.5 animate-spin rounded-full border border-current border-t-transparent" />
                  )}
                  {checking ? t("app.checking") : t("app.checkUpdate")}
                </button>
              </div>

              {updErr && <p className="mt-2 break-words text-[11px] text-red-500">{updErr}</p>}

              {!upd && !checking && !updErr && (
                <p className="mt-2 text-[11px] text-neutral-400">{t("app.checkHint")}</p>
              )}

              {upd && (
                <div className="mt-2 space-y-2">
                  {upd.has_update ? (
                    <p className="text-xs font-medium text-neutral-700 dark:text-neutral-200">
                      {t("app.newVersion")}
                      <span className="accent-solid ml-1.5 rounded px-1.5 py-0.5 font-mono text-[11px] font-bold">
                        v{upd.latest}
                      </span>
                    </p>
                  ) : (
                    <p className="text-xs text-neutral-500">{t("app.upToDate")}</p>
                  )}

                  {upd.has_update && upd.notes && (
                    <div className="rounded-lg bg-neutral-100/90 px-2 py-1.5 dark:bg-neutral-800/60">
                      <p className="text-[10px] font-semibold uppercase tracking-wide text-neutral-400">
                        {t("app.notes")}
                      </p>
                      <p className="mt-0.5 max-h-14 overflow-hidden text-[11px] leading-snug text-neutral-600 dark:text-neutral-300">
                        {upd.notes}
                      </p>
                    </div>
                  )}

                  {upd.has_update && !upd.downloaded && !dl && (
                    <div className="flex items-center gap-2">
                      <button
                        onClick={doDownloadUpdate}
                        className="accent-solid inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-[11px] font-semibold transition select-none hover:brightness-110"
                      >
                        {t("app.downloadUpdate")}
                      </button>
                      {upd.url && (
                        <a
                          href="#"
                          onClick={(e) => {
                            e.preventDefault();
                            invoke("open_external", { url: upd.url }).catch(() => {});
                          }}
                          className="text-[11px] text-neutral-400 underline-offset-2 hover:text-neutral-700 hover:underline dark:hover:text-neutral-200"
                        >
                          {t("app.releasesOpen")}
                        </a>
                      )}
                    </div>
                  )}

                  {upd.downloaded && (
                    <button
                      onClick={() => api.updateApply().catch(() => {})}
                      className="accent-solid inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-[11px] font-semibold transition select-none hover:brightness-110"
                    >
                      {t("title.updateApply")}
                    </button>
                  )}

                  {dl && (
                    <div>
                      <div className="flex items-center justify-between text-[11px] text-neutral-500 dark:text-neutral-400">
                        <span>{t("app.downloading")}…</span>
                        <span className="font-mono">
                          {(() => {
                            const pct =
                              dl.total > 0
                                ? Math.min(100, Math.round((dl.downloaded / dl.total) * 100))
                                : null;
                            return `${pct !== null ? `${pct}% · ` : ""}${fmtBytes(dl.speed)}/s`;
                          })()}
                        </span>
                      </div>
                      <div className="mt-1 h-1.5 w-full overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-800">
                        <div
                          className={`accent-solid h-full rounded-full transition-[width] duration-200 ${
                            dl.total > 0 ? "" : "animate-pulse"
                          }`}
                          style={{
                            width:
                              dl.total > 0
                                ? `${Math.min(100, (dl.downloaded / dl.total) * 100)}%`
                                : "100%",
                          }}
                        />
                      </div>
                      <div className="mt-1 text-right font-mono text-[10px] text-neutral-400">
                        {fmtBytes(dl.downloaded)}
                        {dl.total > 0 ? ` / ${fmtBytes(dl.total)}` : ""}
                      </div>
                    </div>
                  )}
                </div>
              )}
            </div>

            {/* 安卓版：二维码 + 打开下载页（二维码黑码白底，扫码可靠性优先于主题） */}
            <div className="mt-3 flex items-center gap-3 rounded-2xl border border-neutral-200 bg-white/70 p-2 text-left shadow-sm dark:border-neutral-800 dark:bg-neutral-950/40">
              <div className="h-20 w-20 shrink-0 rounded-xl border border-neutral-200 bg-white p-1.5 dark:border-neutral-700">
                {androidQr ? (
                  <div
                    className="h-full w-full [&>svg]:block [&>svg]:h-full [&>svg]:w-full"
                    dangerouslySetInnerHTML={{ __html: androidQr }}
                  />
                ) : (
                  <div className="flex h-full w-full items-center justify-center text-xs text-neutral-400">
                    …
                  </div>
                )}
              </div>
              <div className="min-w-0 flex-1">
                <p className="text-xs font-medium text-neutral-700 dark:text-neutral-200">
                  {t("app.android")}
                </p>
                <p className="mt-0.5 text-[10px] leading-snug text-neutral-400">{t("app.androidQr")}</p>
                <button
                  onClick={() => invoke("open_external", { url: ANDROID_URL }).catch(() => {})}
                  className="mt-1 text-[11px] font-medium text-neutral-500 underline-offset-2 hover:text-neutral-900 hover:underline dark:text-neutral-400 dark:hover:text-white"
                >
                  {t("app.androidOpen")} ↗
                </button>
              </div>
            </div>

            {/* 底部操作：QQ 群 / GitHub / 关闭 */}
            <div className="mt-3 flex flex-wrap items-center justify-center gap-2">
              <button
                onClick={() => invoke("open_external", { url: "https://qm.qq.com/q/qlFr8ct0ps" }).catch(() => {})}
                className="pill pill-outline pill-hover px-3 py-1 text-xs"
              >
                {t("app.qqGroup")}
              </button>
              <button
                onClick={() => invoke("open_external", { url: "https://github.com/yxpil/bit" }).catch(() => {})}
                className="pill pill-outline pill-hover px-3 py-1 text-xs"
              >
                GitHub
              </button>
              <button onClick={() => setShowAbout(false)} className="pill pill-hover px-4 py-1 text-xs">
                {t("common.ok")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
