import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IconMinus, IconSquare, IconX } from "./Icons.jsx";
import { useLang } from "../i18n.js";

const fmtK = (n) => (n >= 1024 ? `${(n / 1024).toFixed(1)}K` : String(n));

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
        {memMB !== null && (
          <>
            <span className="opacity-50">·</span>
            <span>{memMB} MB</span>
          </>
        )}
      </div>

      {/* 弹性拖动区 */}
      <div data-tauri-drag-region className="flex-1" />

      {/* 窗口按钮 */}
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
    </div>
  );
}
