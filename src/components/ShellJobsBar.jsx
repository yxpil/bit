import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { useLang } from "../i18n";
import { IconStop, IconTerminal } from "./Icons";

/**
 * 后台长命令状态条（挂在聊天页输入框上方，子代理状态条之下）。
 * 数据来自后端 shellbg：shell 命令超过前台窗口仍没跑完时自动转后台并广播
 * `shell-job`（started / done / killed）事件；这里实时展示命令、运行时长，
 * 并提供逐条「停止」按钮。空态不占位，只在有命令在跑时出现。
 */
export default function ShellJobsBar() {
  const { t } = useLang();
  const [jobs, setJobs] = useState([]); // [{ job_id, command, session_id, startedAt(本地 ms) }]
  const [stopping, setStopping] = useState([]); // 已请求停止、等待 killed 事件确认的 job
  const [now, setNow] = useState(Date.now());

  // 初始状态（进程重启后仍可能在跑的作业）+ 事件订阅
  useEffect(() => {
    let alive = true;
    let unlisten;
    api
      .listRunningShells()
      .then((arr) => {
        if (!alive || !Array.isArray(arr)) return;
        setJobs(
          arr.map((j) => ({
            job_id: j.job_id,
            command: j.command,
            session_id: j.session_id,
            startedAt: Date.now() - (j.elapsed_ms || 0),
          }))
        );
      })
      .catch(() => {});
    listen("shell-job", (e) => {
      if (!alive) return;
      const p = e.payload || {};
      if (p.phase === "started") {
        const existing = p.job_id;
        setJobs((prev) => {
          if (prev.some((j) => j.job_id === existing)) return prev;
          return [
            ...prev,
            {
              job_id: p.job_id,
              command: p.command || "",
              session_id: p.session_id,
              startedAt: Date.now(),
            },
          ];
        });
      } else if (p.phase === "done" || p.phase === "killed" || p.phase === "error") {
        setJobs((prev) => prev.filter((j) => j.job_id !== p.job_id));
      }
    })
      .then((f) => {
        unlisten = f;
      })
      .catch(() => {});
    return () => {
      alive = false;
      if (unlisten) unlisten();
    };
  }, []);

  // 每秒刷新运行时长
  useEffect(() => {
    if (jobs.length === 0) return;
    const iv = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(iv);
  }, [jobs.length]);

  if (jobs.length === 0) return null;

  const stopJob = async (jid) => {
    setStopping((s) => [...s, jid]);
    try {
      await api.cancelShell(jid);
    } catch (e) {
      console.warn("cancel shell failed:", e);
      setStopping((s) => s.filter((x) => x !== jid));
    }
  };

  const fmt = (startedAt) => {
    const s = Math.max(0, Math.floor((now - startedAt) / 1000));
    const mm = Math.floor(s / 60);
    const ss = s % 60;
    return mm > 0 ? `${mm}m${ss}s` : `${ss}s`;
  };

  return (
    <div className="flex flex-wrap items-center gap-1.5 rounded-xl border border-sky-200 bg-sky-50/70 px-2.5 py-1.5 text-xs dark:border-sky-900/50 dark:bg-sky-950/40">
      <span
        className="flex shrink-0 items-center gap-1 font-medium text-sky-700 dark:text-sky-300"
        title={t("chat.bgShellHint")}
      >
        <IconTerminal size={14} className="animate-pulse" />
        {t("chat.bgShells")}
      </span>
      {jobs.map((j) => (
        <span
          key={j.job_id}
          className="flex min-w-0 max-w-full items-center gap-1.5 rounded-lg bg-white px-1.5 py-0.5 ring-1 ring-neutral-200 dark:bg-neutral-800 dark:ring-neutral-700"
        >
          <code className="max-w-[40vw] truncate font-mono text-[11px]" title={j.command}>
            {j.command}
          </code>
          <span className="shrink-0 tabular-nums text-neutral-400">{fmt(j.startedAt)}</span>
          <button
            onClick={() => stopJob(j.job_id)}
            disabled={stopping.includes(j.job_id)}
            title={`${t("chat.stop")} ${j.job_id}`}
            className="shrink-0 rounded p-0.5 text-neutral-400 hover:bg-red-500/10 hover:text-red-500 disabled:opacity-50"
          >
            {stopping.includes(j.job_id) ? (
              <span className="text-[10px]">…</span>
            ) : (
              <IconStop size={11} />
            )}
          </button>
        </span>
      ))}
    </div>
  );
}
