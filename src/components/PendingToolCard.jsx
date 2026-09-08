// 执行中的工具调用占位卡：后端 round_tools_starting 事件触发渲染，
// 带 spinner + 工具名 + 关键参数摘要，tools 事件到达后被 ToolCallCard 替换。
// 设计：半透明灰底 + 柔和 spinner + 等宽字体名，视觉上明确是"进行中"而非"完成"
import { IconTool } from "./Icons.jsx";

const trunc = (s, n = 120) => (s && s.length > n ? `${s.slice(0, n)}…` : s || "");

export default function PendingToolCard({ tool, summary }) {
  return (
    <div className="flex items-center gap-2 overflow-hidden rounded-2xl border border-neutral-200 bg-neutral-50 px-3 py-2 text-[12px] dark:border-neutral-800 dark:bg-neutral-900/60">
      {/* 柔和 spinner：环形渐变，border 透明 + 一边有颜色，1s 匀速 */}
      <span className="inline-block h-3.5 w-3.5 shrink-0 animate-spin rounded-full border-2 border-neutral-200 border-t-neutral-500 dark:border-neutral-700 dark:border-t-neutral-300" />
      <IconTool size={13} className="shrink-0 text-neutral-500" />
      <span className="font-mono font-medium text-neutral-700 dark:text-neutral-200">{tool}</span>
      {summary && (
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-neutral-400" title={summary}>
          {trunc(summary, 120)}
        </span>
      )}
      <span className="shrink-0 text-[10px] text-neutral-400">执行中…</span>
    </div>
  );
}
