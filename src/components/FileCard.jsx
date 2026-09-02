import { useState } from "react";
import { IconFile } from "./Icons.jsx";
import { useLang } from "../i18n.js";
import { api } from "../api.js";

// 聊天内文件卡片：智能体 send_file 发来的文件，如同收到一条文件消息。
// 可用系统默认程序打开，或定位到所在文件夹。
export default function FileCard({ path, bytes, note }) {
  const { t } = useLang();
  const [err, setErr] = useState("");
  const name = (path || "").replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path || "";
  const size =
    typeof bytes === "number" && bytes > 0
      ? bytes >= 1048576
        ? (bytes / 1048576).toFixed(1) + " MB"
        : bytes >= 1024
          ? (bytes / 1024).toFixed(1) + " KB"
          : bytes + " B"
      : "";
  const open = async (reveal) => {
    try {
      setErr("");
      await api.openPath(path, reveal);
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <div className="flex max-w-[85%] flex-col gap-1">
      <div className="flex items-center gap-3 rounded-2xl border border-neutral-200 bg-white px-3.5 py-2.5 dark:border-neutral-800 dark:bg-neutral-900">
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-neutral-100 text-neutral-600 dark:bg-neutral-800 dark:text-neutral-300">
          <IconFile size={16} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium text-neutral-900 dark:text-neutral-100">{name}</div>
          <div className="truncate text-[11px] text-neutral-400">
            {t("filecard.fromAgent")}
            {size ? ` · ${size}` : ""}
            {note ? ` · ${note}` : ""}
          </div>
        </div>
        <button
          onClick={() => open(false)}
          className="accent-solid rounded-full px-3 py-1 text-xs font-medium transition-colors"
        >
          {t("filecard.open")}
        </button>
        <button
          onClick={() => open(true)}
          className="rounded-full border border-neutral-300 px-3 py-1 text-xs font-medium text-neutral-600 transition-colors hover:bg-neutral-100 hover:text-neutral-900 dark:border-neutral-700 dark:text-neutral-300 dark:hover:bg-neutral-800 dark:hover:text-white"
        >
          {t("filecard.reveal")}
        </button>
      </div>
      {err && <div className="px-1 text-[11px] text-red-500">{err}</div>}
    </div>
  );
}
