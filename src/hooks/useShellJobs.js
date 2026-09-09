import { useEffect, useState, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";

/**
 * 后台长命令状态：集中订阅 + 拉取逻辑
 * 数据来源：
 *   - 启动时调 listRunningShells 同步一次（进程重启后仍可能有遗留作业）
 *   - 监听 shell-job（started/done/killed/error）事件
 * 返回 { jobs, stopping, stopJob, now }，jobs 元素结构：
 *   { job_id, command, session_id, startedAt (本地 ms), elapsed_ms }
 */
export function useShellJobs() {
  const [jobs, setJobs] = useState([]);
  const [stopping, setStopping] = useState([]); // 已请求停止、等待 killed 事件确认的 job_id
  const [now, setNow] = useState(Date.now());

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
            elapsed_ms: j.elapsed_ms || 0,
          }))
        );
      })
      .catch(() => {});
    listen("shell-job", (e) => {
      if (!alive) return;
      const p = e.payload || {};
      if (p.phase === "started") {
        setJobs((prev) =>
          prev.some((j) => j.job_id === p.job_id)
            ? prev
            : [
                ...prev,
                {
                  job_id: p.job_id,
                  command: p.command || "",
                  session_id: p.session_id,
                  startedAt: Date.now(),
                  elapsed_ms: 0,
                },
              ]
        );
      } else if (p.phase === "done" || p.phase === "killed" || p.phase === "error") {
        setJobs((prev) => prev.filter((j) => j.job_id !== p.job_id));
        setStopping((s) => s.filter((x) => x !== p.job_id));
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

  // 有作业在跑时每 1s 刷新一次运行时长（驱动外部组件重渲染）
  useEffect(() => {
    if (jobs.length === 0) return;
    const iv = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(iv);
  }, [jobs.length]);

  const stopJob = useCallback(async (jid) => {
    setStopping((s) => (s.includes(jid) ? s : [...s, jid]));
    try {
      await api.cancelShell(jid);
    } catch (e) {
      console.warn("cancel shell failed:", e);
      setStopping((s) => s.filter((x) => x !== jid));
    }
  }, []);

  return { jobs, stopping, stopJob, now };
}
