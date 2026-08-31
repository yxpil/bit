import { useState } from "react";
import { IconTool, IconCheck, IconX, IconChevronDown, IconChevronRight } from "./Icons.jsx";

// 对话内单个工具调用的可视化卡片：可折叠，显示工具名 / 参数 / 结果 / 成败
export default function ToolCallCard({ call }) {
  const [open, setOpen] = useState(false);
  const ok = call.ok;
  const pretty = (v) => {
    if (v == null) return "";
    if (typeof v === "string") return v;
    try {
      return JSON.stringify(v, null, 2);
    } catch {
      return String(v);
    }
  };
  const hasParams = call.params && Object.keys(call.params || {}).length > 0;

  return (
    <div
      className={`overflow-hidden rounded-2xl border text-[12px] ${
        ok
          ? "border-neutral-200 bg-neutral-50 dark:border-neutral-800 dark:bg-neutral-900/60"
          : "border-red-200 bg-red-50 dark:border-red-900/60 dark:bg-red-950/30"
      }`}
    >
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left"
      >
        {open ? <IconChevronDown size={13} /> : <IconChevronRight size={13} />}
        <IconTool size={13} />
        <span className="font-mono font-medium">{call.tool}</span>
        <span
          className={`ml-auto flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium ${
            ok
              ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300"
              : "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300"
          }`}
        >
          {ok ? <IconCheck size={10} /> : <IconX size={10} />}
          {ok ? "成功" : "失败"}
        </span>
      </button>

      {open && (
        <div className="space-y-2 border-t border-neutral-200/70 px-3 py-2 dark:border-neutral-800/70">
          {hasParams && (
            <div>
              <div className="mb-1 text-[10px] font-semibold uppercase tracking-wide text-neutral-400">参数</div>
              <pre className="overflow-x-auto rounded-lg bg-white/70 p-2 font-mono text-[11px] leading-relaxed text-neutral-700 dark:bg-black/40 dark:text-neutral-300">
                {pretty(call.params)}
              </pre>
            </div>
          )}
          <div>
            <div className="mb-1 text-[10px] font-semibold uppercase tracking-wide text-neutral-400">结果</div>
            <pre className="overflow-x-auto rounded-lg bg-white/70 p-2 font-mono text-[11px] leading-relaxed text-neutral-700 dark:bg-black/40 dark:text-neutral-300">
              {pretty(call.result)}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}
