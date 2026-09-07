import { useState } from "react";
import { IconTool, IconCheck, IconX, IconChevronDown, IconChevronRight } from "./Icons.jsx";
import { useLang } from "../i18n.js";

// 对话内单个工具调用的可视化卡片：可折叠，头部直接展示关键信息
// （shell 显示命令本身，文件工具显示路径），展开后按工具类型分块：
// 命令 → 命令/输出/错误输出/退出码；write_file → 写入内容；edit → 原内容/新内容对照。
const trunc = (s, n = 120) => (s && s.length > n ? `${s.slice(0, n)}…` : s || "");

// 头部摘要：让用户不展开就知道执行了什么
function summarize(call) {
  const p = call.params || {};
  if (call.tool === "shell" && p.command) return { text: p.command, mono: true };
  if ((call.tool === "write_file" || call.tool === "edit" || call.tool === "send_file" || call.tool === "view_image") && p.path)
    return { text: p.path, mono: true };
  return null;
}

// 代码块：小标题 + 可滚动 mono 内容
function Block({ label, children, tone = "" }) {
  return (
    <div>
      <div className="mb-1 text-[10px] font-semibold uppercase tracking-wide text-neutral-400">{label}</div>
      <pre
        className={`max-h-56 overflow-auto rounded-lg p-2 font-mono text-[11px] leading-relaxed whitespace-pre-wrap break-all ${tone || "bg-white/70 text-neutral-700 dark:bg-black/40 dark:text-neutral-300"}`}
      >
        {children}
      </pre>
    </div>
  );
}

export default function ToolCallCard({ call }) {
  const { t } = useLang();
  // shell / 文件写入默认展开：执行了什么、改了什么应当一眼可见（块内 max-h 滚动，不会刷屏）
  const visual = call.tool === "shell" || call.tool === "write_file" || call.tool === "edit";
  const [open, setOpen] = useState(visual);
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
  const sum = summarize(call);
  const p = call.params || {};
  const r = call.result && typeof call.result === "object" ? call.result : null;
  const isShell = call.tool === "shell" && r && ("stdout" in r || "stderr" in r || "code" in r);
  const isWrite = call.tool === "write_file" && typeof p.content === "string";
  const isEdit = call.tool === "edit" && (typeof p.old_string === "string" || typeof p.new_string === "string");

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
        {sum && (
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-neutral-500 dark:text-neutral-400" title={sum.text}>
            {trunc(sum.text, 120)}
          </span>
        )}
        <span
          className={`ml-auto flex flex-none items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium ${
            ok
              ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300"
              : "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300"
          }`}
        >
          {ok ? <IconCheck size={10} /> : <IconX size={10} />}
          {ok ? t("toolcard.success") : t("toolcard.fail")}
        </span>
      </button>

      {open && (
        <div className="space-y-2 border-t border-neutral-200/70 px-3 py-2 dark:border-neutral-800/70">
          {isShell && (
            <>
              <Block label={t("toolcard.command")}>{p.command || ""}</Block>
              {r.code != null && r.code !== 0 && (
                <div className="text-[11px] text-red-600 dark:text-red-400">
                  {t("toolcard.exitCode")}: {r.code}
                </div>
              )}
              {r.stdout ? <Block label={t("toolcard.stdout")}>{r.stdout}</Block> : null}
              {r.stderr ? <Block label={t("toolcard.stderr")}>{r.stderr}</Block> : null}
              {!r.stdout && !r.stderr && r.code === 0 && (
                <div className="text-[11px] text-neutral-400">{t("toolcard.noOutput")}</div>
              )}
            </>
          )}
          {isWrite && <Block label={t("toolcard.content")}>{p.content}</Block>}
          {isEdit && (
            <>
              {p.old_string ? (
                <Block
                  label={t("toolcard.old")}
                  tone="bg-red-50 text-red-900 dark:bg-red-950/40 dark:text-red-200"
                >
                  {p.old_string}
                </Block>
              ) : null}
              <Block
                label={t("toolcard.new")}
                tone="bg-emerald-50 text-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-200"
              >
                {p.new_string || ""}
              </Block>
            </>
          )}
          {!isShell && !isWrite && !isEdit && (
            <>
              {hasParams && <Block label={t("toolcard.params")}>{pretty(call.params)}</Block>}
              <Block label={t("toolcard.result")}>{pretty(call.result)}</Block>
            </>
          )}
        </div>
      )}
    </div>
  );
}
