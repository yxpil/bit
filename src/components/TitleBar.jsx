import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { PhysicalPosition, currentMonitor, getCurrentWindow, primaryMonitor } from "@tauri-apps/api/window";
import { IconMinus, IconSquare, IconX } from "./Icons.jsx";
import { useLang } from "../i18n.js";

const fmtK = (n) => (n >= 1024 ? `${(n / 1024).toFixed(1)}K` : String(n));

// macOS 使用原生 Overlay 标题栏（系统红绿灯按钮），隐藏自绘窗口按钮并为其留出空间；
// Windows/Linux 保持自绘无边框标题栏不变
const isMac = navigator.platform.toUpperCase().includes("MAC");

// 自定义无边框标题栏：与窗口按钮同高的页眉仪表盘（版本/会话/Token/内存），
// 数据不侵占内容区；中段可拖动，右侧窗口按钮
export default function TitleBar() {
  const { t } = useLang();
  const win = () => getCurrentWindow();
  const minimize = () => win().minimize().catch(() => {});
  const toggleMax = () => win().toggleMaximize().catch(() => {});
  // 关闭走 CloseRequested → 隐藏到托盘（后端已拦截），此处直接调用即可
  const close = () => win().close().catch(() => {});

  const [ver, setVer] = useState("");
  const [dash, setDash] = useState({ sessions: 0, tokens: 0, limitK: 128 });
  const [memMB, setMemMB] = useState(null);

  useEffect(() => {
    let timer = null;
    let unlisten = null;
    const clampIntoView = async () => {
      try {
        const w = win();
        if (await w.isMaximized()) return;
        const [pos, size, monitor] = await Promise.all([
          w.outerPosition(),
          w.outerSize(),
          currentMonitor().catch(() => null),
        ]);
        const target = monitor || (await primaryMonitor().catch(() => null));
        const area = target?.workArea;
        if (!area) return;
        const minVisibleX = Math.min(160, Math.max(72, Math.round(size.width * 0.2)));
        const titleBarHeight = 36;
        const minX = area.position.x - size.width + minVisibleX;
        const maxX = area.position.x + area.size.width - minVisibleX;
        const minY = area.position.y;
        const maxY = area.position.y + area.size.height - titleBarHeight;
        const nextX = Math.min(maxX, Math.max(minX, pos.x));
        const nextY = Math.min(maxY, Math.max(minY, pos.y));
        if (nextX !== pos.x || nextY !== pos.y) {
          await w.setPosition(new PhysicalPosition(nextX, nextY));
        }
      } catch {
        /* 忽略：旧权限或平台差异时保持原生行为 */
      }
    };
    const scheduleClamp = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(clampIntoView, 140);
    };
    clampIntoView();
    win()
      .onMoved(scheduleClamp)
      .then((f) => {
        unlisten = f;
      })
      .catch(() => {});
    return () => {
      if (timer) clearTimeout(timer);
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    getVersion().then(setVer).catch(() => {});
  }, []);

  // ChatPage 广播会话数与当前上下文 Token
  useEffect(() => {
    const h = (e) => setDash(e.detail || {});
    window.addEventListener("bit-dash", h);
    return () => window.removeEventListener("bit-dash", h);
  }, []);

  // 本进程内存占用，3 秒轮询
  useEffect(() => {
    let stop = false;
    const poll = async () => {
      try {
        const b = await invoke("mem_usage");
        if (!stop) setMemMB(Math.round(b / 1048576));
      } catch {
        /* 忽略：命令不可用时隐藏该项 */
      }
    };
    poll();
    const timer = setInterval(poll, 3000);
    return () => {
      stop = true;
      clearInterval(timer);
    };
  }, []);

  return (
    <div
      data-tauri-drag-region
      className="flex h-9 shrink-0 select-none items-center gap-1 px-3"
      style={isMac ? { paddingLeft: 76 } : undefined}
    >
      {/* 页眉仪表盘（可拖动） */}
      <div
        data-tauri-drag-region
        className="anim-rise flex items-center gap-2 text-[11px] text-neutral-400 dark:text-neutral-500"
      >
        <span
          className={`h-2 w-2 rounded-full ${
            dash.running ? "animate-pulse bg-amber-400" : "bg-emerald-500"
          }`}
        />
        {ver && <span className="font-semibold">v{ver}</span>}
        <span className="opacity-50">·</span>
        <span>
          {dash.sessions} {t("chat.unitSessions")}
        </span>
        <span className="opacity-50">·</span>
        <span>
          {fmtK(dash.tokens)} / {dash.limitK}K tok
        </span>
        {dash.showCache && (
          <>
            <span className="opacity-50">·</span>
            <span>
              {t("chat.cacheHit")} {Math.round((dash.cacheHitRate || 0) * 100)}%
            </span>
          </>
        )}
        {memMB !== null && (
          <>
            <span className="opacity-50">·</span>
            <span>{memMB} MB</span>
          </>
        )}
      </div>

      {/* 弹性拖动区 */}
      <div data-tauri-drag-region className="flex-1" />

      {/* 窗口按钮（macOS 由系统红绿灯提供，不重复渲染） */}
      {!isMac && (
        <>
          <button
            onClick={minimize}
            aria-label={t("title.minimize")}
            className="flex h-7 w-7 items-center justify-center rounded-full text-neutral-500 transition-colors hover:bg-neutral-200 hover:text-neutral-900 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
          >
            <IconMinus size={13} />
          </button>
          <button
            onClick={toggleMax}
            aria-label={t("title.maximize")}
            className="flex h-7 w-7 items-center justify-center rounded-full text-neutral-500 transition-colors hover:bg-neutral-200 hover:text-neutral-900 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
          >
            <IconSquare size={12} />
          </button>
          <button
            onClick={close}
            aria-label={t("common.close")}
            className="flex h-7 w-7 items-center justify-center rounded-full text-neutral-500 transition-colors hover:bg-red-600 hover:text-white"
          >
            <IconX size={14} />
          </button>
        </>
      )}
    </div>
  );
}
