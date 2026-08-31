import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

// 助手消息的 Markdown 渲染：适配黑白胶囊设计，支持 GFM（表格/删除线/任务列表等）。
// 使用 react-markdown（不走 dangerouslySetInnerHTML，天然防 XSS）。
export default function Markdown({ children }) {
  return (
    <div className="md text-sm leading-relaxed">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          // 段落之间留出间距
          p: ({ node, ...p }) => <p className="my-1.5 first:mt-0 last:mb-0" {...p} />,
          // 列表
          ul: ({ node, ...p }) => <ul className="my-1.5 list-disc space-y-1 pl-5" {...p} />,
          ol: ({ node, ...p }) => <ol className="my-1.5 list-decimal space-y-1 pl-5" {...p} />,
          li: ({ node, ...p }) => <li className="marker:text-neutral-400" {...p} />,
          // 标题
          h1: ({ node, ...p }) => <h1 className="mb-1.5 mt-2 text-base font-semibold first:mt-0" {...p} />,
          h2: ({ node, ...p }) => <h2 className="mb-1.5 mt-2 text-[15px] font-semibold first:mt-0" {...p} />,
          h3: ({ node, ...p }) => <h3 className="mb-1 mt-2 text-sm font-semibold first:mt-0" {...p} />,
          // 强调
          strong: ({ node, ...p }) => <strong className="font-semibold" {...p} />,
          em: ({ node, ...p }) => <em className="italic" {...p} />,
          a: ({ node, ...p }) => (
            <a className="underline underline-offset-2 hover:opacity-80" target="_blank" rel="noreferrer" {...p} />
          ),
          // 引用
          blockquote: ({ node, ...p }) => (
            <blockquote
              className="my-1.5 border-l-2 border-neutral-300 pl-3 text-neutral-600 dark:border-neutral-700 dark:text-neutral-400"
              {...p}
            />
          ),
          hr: ({ node, ...p }) => <hr className="my-2 border-neutral-200 dark:border-neutral-800" {...p} />,
          // 行内代码 / 代码块
          code: ({ node, inline, className, children, ...p }) =>
            inline ? (
              <code
                className="rounded bg-neutral-200/70 px-1 py-0.5 font-mono text-[0.85em] dark:bg-neutral-800"
                {...p}
              >
                {children}
              </code>
            ) : (
              <code className="font-mono text-[0.85em]" {...p}>
                {children}
              </code>
            ),
          pre: ({ node, ...p }) => (
            <pre
              className="my-1.5 overflow-x-auto rounded-lg bg-neutral-100 p-2.5 text-[0.85em] leading-relaxed dark:bg-black/40"
              {...p}
            />
          ),
          // 表格（GFM）
          table: ({ node, ...p }) => (
            <div className="my-1.5 overflow-x-auto">
              <table className="w-full border-collapse text-[0.9em]" {...p} />
            </div>
          ),
          th: ({ node, ...p }) => (
            <th
              className="border border-neutral-200 bg-neutral-100 px-2 py-1 text-left font-semibold dark:border-neutral-800 dark:bg-neutral-900"
              {...p}
            />
          ),
          td: ({ node, ...p }) => (
            <td className="border border-neutral-200 px-2 py-1 dark:border-neutral-800" {...p} />
          ),
        }}
      >
        {children || ""}
      </ReactMarkdown>
    </div>
  );
}
