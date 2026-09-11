import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkBreaks from "remark-breaks";
import rehypeKatex from "rehype-katex";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import { useEffect, useRef, useState } from "react";
import "katex/dist/katex.min.css";

// 助手消息的 Markdown 渲染（全面版）：
//   GFM（表格/删除线/任务列表）· 数学公式（KaTeX，$行内 / $$块级$$）
//   · 单换行即换行（聊天习惯）· 内嵌 HTML/SVG（经白名单消毒：script/事件属性一律剥除，天然防 XSS）
//   · mermaid 图（```mermaid 代码块 → 动态加载渲染，失败回退显示源码）
// 注意：不用 dangerouslySetInnerHTML 裸插；所有原始 HTML 先过 rehype-sanitize 白名单。

// SVG 白名单：默认 schema 不含 svg 标签，追加常用绘图元素（无 script/foreignObject）
const SVG_TAGS = [
  "svg", "g", "defs", "symbol", "use", "title", "desc",
  "path", "rect", "circle", "ellipse", "line", "polyline", "polygon",
  "text", "tspan", "textPath",
  "marker", "pattern", "clipPath", "mask",
  "linearGradient", "radialGradient", "stop",
  "filter", "feGaussianBlur", "feOffset", "feBlend", "feFlood", "feComposite",
];
// 通用属性白名单（几何/外观；事件处理器 on* 与 style 里的表达式不在列 = 自动剥除）
const SVG_ATTRS = [
  "class", "id", "width", "height", "viewBox", "preserveAspectRatio", "xmlns",
  "x", "y", "x1", "x2", "y1", "y2", "cx", "cy", "r", "rx", "ry",
  "d", "points", "transform", "opacity", "fill", "fill-opacity", "fill-rule",
  "stroke", "stroke-width", "stroke-opacity", "stroke-dasharray", "stroke-linecap", "stroke-linejoin",
  "font-family", "font-size", "font-weight", "font-style", "text-anchor", "dominant-baseline",
  "dx", "dy", "rotate", "gradientUnits", "offset", "stop-color", "stop-opacity",
  "clip-path", "clip-rule", "mask", "marker", "marker-start", "marker-mid", "marker-end",
  "patternUnits", "filter", "in", "in2", "stdDeviation", "result", "mode",
];
const sanitizeSchema = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames || []), ...SVG_TAGS],
  attributes: {
    ...defaultSchema.attributes,
    "*": [...(defaultSchema.attributes?.["*"] || []), ...SVG_ATTRS],
  },
};

// mermaid 图渲染：动态 import（不进主包），失败时回退显示源码
// 三个关键约束（违反任一都会出现"合法的图随机报 syntax error / 渲染好的图过一会儿变回文字"）：
//   1. initialize 只做一次（主题变化时除外）——渲染中途重初始化会重置全局解析状态
//   2. render 串行执行——mermaid 全局状态非并发安全，并发 render 会互相打断报 parse error
//   3. 成功结果按 code+主题 缓存——流式输出/消息重渲染导致组件重挂载时直接复用，
//      不再触碰 mermaid 全局状态（这是"图出来后又集体变回文字"的根治手段）
let mermaidSeq = 0;
let mermaidInited = false;
let mermaidInitedDark = null;
const mermaidSvgCache = new Map(); // key: `${dark ? "d" : "l"}:${code}` -> svg
let mermaidChain = Promise.resolve(); // 渲染串行链
function Mermaid({ code, dark }) {
  const cacheKey = `${dark ? "d" : "l"}:${code}`;
  const [svg, setSvg] = useState(() => mermaidSvgCache.get(cacheKey) || "");
  const [err, setErr] = useState(false);
  useEffect(() => {
    if (mermaidSvgCache.has(cacheKey)) {
      setSvg(mermaidSvgCache.get(cacheKey));
      setErr(false);
      return;
    }
    let alive = true;
    (async () => {
      try {
        const mermaid = (await import("mermaid")).default;
        if (!mermaidInited || mermaidInitedDark !== dark) {
          mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default", securityLevel: "strict" });
          mermaidInited = true;
          mermaidInitedDark = dark;
        }
        // 第一步：自己先用 parse 验证（suppressErrors：不抛异常、无 DOM 副作用）。
        // 验证不过 = 不是合法的图，直接回退源码，绝不进 render——render 的错误路径
        // 会往 body 里插错误节点（"syntax error in text" 把页面顶上去的就是它）。
        const valid = await mermaid.parse(code, { suppressErrors: true });
        if (!valid) {
          if (alive) setErr(true);
          return;
        }
        // 第二步：验证通过才排进串行链渲染
        const run = () => mermaid.render(`mmd-${++mermaidSeq}`, code);
        const task = mermaidChain.then(run, run);
        mermaidChain = task.catch(() => {});
        const out = await task;
        if (alive) {
          mermaidSvgCache.set(cacheKey, out);
          setSvg(out);
          setErr(false);
        }
      } catch {
        // 兜底清理：mermaid render 失败时可能在 body 残留 #dmermaid-*/#dmmd-* 错误节点
        document.querySelectorAll("[id^='dmermaid'], [id^='dmmd-']").forEach((n) => n.remove());
        if (alive) setErr(true);
      }
    })();
    return () => { alive = false; };
  }, [cacheKey]);
  if (err) return <pre className="my-1.5 overflow-x-auto rounded-lg bg-neutral-100 p-2.5 text-[0.85em] dark:bg-black/40"><code className="font-mono text-[0.85em]">{code}</code></pre>;
  if (!svg) return <div className="my-1.5 text-xs text-neutral-400">mermaid 渲染中…</div>;
  // securityLevel strict + sanitize 同源：mermaid 输出的 svg 已自净，这里再用 img 承载双保险
  return <div className="my-2 overflow-x-auto" dangerouslySetInnerHTML={{ __html: svg }} />;
}

// 无语言标注代码块的 mermaid 自动识别：历史消息里模型常把图代码包在普通 ```
// 里（没标 mermaid），只认标记会永远显示源码。按首行关键字保守判定，防误伤普通脚本。
const MERMAID_FIRST_LINE =
  /^\s*(flowchart|sequenceDiagram|classDiagram|stateDiagram(-v2)?|erDiagram|journey|gantt|pie|mindmap|timeline|quadrantChart|quadrant|requirementDiagram|gitGraph|graph\s+(TB|TD|BT|RL|LR)\b|C4(Context|Container|Component|Dynamic|Deployment)\b|sankey(-beta)?|xychart(-beta)?|block(-beta)?|zenuml)\b/;
const FENCE_RE = /^\s{0,3}(`{3,}|~{3,})/;

// 图体行特征：生命周期关键字或消息箭头（->>、-->>、-)、--)、-->、-.-、==）
const DIAGRAM_LINE =
  /^\s*(autonumber\b|participant\s|actor\s|note\s|alt\b|else\b|opt\b|loop\b|par\b|and\b|critical\b|option\b|break\b|rect\b|title\s|legend\s|end\b)|->>|-->>|-\)|--\)|-->|-\.|==/;

// 围栏外散落的 mermaid 段落 → 合并成完整 ```mermaid 围栏（真实事故：模型把整幅
// 时序图当普通文本输出，一行围栏都没有）。捕获规则：
//   头部段落逐行收；空行向后看——下一非空行仍是图行才跨过；出现散文立即收尾；
//   紧跟（中间只允许空行）的无语言围栏把内容并进来。截坏的由 parse 闸门兜底回退源码。
function rescueMermaid(src) {
  const lines = src.split("\n");
  const out = [];
  let inFence = false;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (FENCE_RE.test(line)) { inFence = !inFence; out.push(line); continue; }
    if (!inFence && MERMAID_FIRST_LINE.test(line)) {
      out.push("```mermaid");
      out.push(line);
      i++;
      let blank = 0;
      while (i < lines.length) {
        const l = lines[i];
        if (FENCE_RE.test(l)) break; // 真实围栏：并入
        if (l.trim() === "") {
          // 空行：向后看，下一非空行仍是图行才继续（否则在此收尾）
          let j = i + 1;
          while (j < lines.length && lines[j].trim() === "") j++;
          if (j >= lines.length || !DIAGRAM_LINE.test(lines[j])) break;
          out.push(l);
          blank++;
          if (blank > 30) break; // 安全阀
          i++;
          continue;
        }
        if (!DIAGRAM_LINE.test(l)) break; // 散文开始：收尾
        out.push(l);
        i++;
      }
      // 紧跟真实围栏（中间允许只有空行）→ 并入其内容（跳过它自己的开/闭栏行）
      {
        let j = i;
        while (j < lines.length && lines[j].trim() === "") j++;
        if (j < lines.length && FENCE_RE.test(lines[j])) {
          let k = j + 1;
          while (k < lines.length && !FENCE_RE.test(lines[k])) { out.push(lines[k]); k++; }
          i = k < lines.length ? k + 1 : k; // 跳过闭合围栏
        }
      }
      out.push("```");
      continue;
    }
    out.push(line);
  }
  return out.join("\n");
}

export default function Markdown({ children }) {
  const dark = typeof document !== "undefined" && document.documentElement.classList.contains("dark");
  return (
    <div className="md text-sm leading-relaxed">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath, remarkBreaks]}
        rehypePlugins={[rehypeRaw, [rehypeSanitize, sanitizeSchema], [rehypeKatex, { throwOnError: false }]]}
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
          // 行内代码 / 代码块（mermaid 特判渲染成图）
          code: ({ node, inline, className, children, ...p }) => {
            const lang = /language-(\w+)/.exec(className || "")?.[1];
            // 防御性扁平化：children 理论上恒为 string，但插件组合可能混入元素节点，
            // String([obj]) 会产出字面量 "[object Object]"——只保留字符串片段
            const text = Array.isArray(children)
              ? children.filter((c) => typeof c === "string").join("")
              : typeof children === "string"
                ? children
                : "";
            if (!inline && lang === "mermaid") return <Mermaid code={text.trim()} dark={dark} />;
            // 无/通用语言标注 + 首行像 mermaid → 当图渲染（救历史消息里没标语言的图代码）
            const firstLine = text.split("\n", 1)[0];
            if (!inline && (!lang || /^(text|txt|diag)$/i.test(lang)) && MERMAID_FIRST_LINE.test(firstLine)) {
              return <Mermaid code={text.trim()} dark={dark} />;
            }
            return inline ? (
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
            );
          },
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
        {rescueMermaid(children || "")}
      </ReactMarkdown>
    </div>
  );
}
