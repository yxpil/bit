// mermaid 渲染管线诊断：复刻 Markdown.jsx 的插件栈，检查 code 组件收到的 children 类型
// 用法：node scripts/test-md.mjs
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkBreaks from "remark-breaks";
import rehypeKatex from "rehype-katex";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";

const SVG_TAGS = ["svg", "g", "defs", "path", "rect", "circle", "line", "text", "tspan"];
const SVG_ATTRS = ["class", "id", "width", "height", "viewBox", "d", "fill", "stroke", "xmlns"];
const sanitizeSchema = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames || []), ...SVG_TAGS],
  attributes: {
    ...defaultSchema.attributes,
    "*": [...(defaultSchema.attributes?.["*"] || []), ...SVG_ATTRS],
  },
};

// 与 Markdown.jsx 一致的 rescueMermaid（裸图整体捕获版）
const MERMAID_FIRST_LINE =
  /^\s*(flowchart|sequenceDiagram|classDiagram|stateDiagram(-v2)?|erDiagram|journey|gantt|pie|mindmap|timeline|quadrantChart|quadrant|requirementDiagram|gitGraph|graph\s+(TB|TD|BT|RL|LR)\b|C4(Context|Container|Component|Dynamic|Deployment)\b|sankey(-beta)?|xychart(-beta)?|block(-beta)?|zenuml)\b/;
const FENCE_RE = /^\s{0,3}(`{3,}|~{3,})/;
// 图体行特征：生命周期关键字或消息箭头（->>、-->>、-)、--)、-->、-.-、==）
const DIAGRAM_LINE =
  /^\s*(autonumber\b|participant\s|actor\s|note\s|alt\b|else\b|opt\b|loop\b|par\b|and\b|critical\b|option\b|break\b|rect\b|title\s|legend\s|end\b)|->>|-->>|-\)|--\)|-->|-\.|==/;
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
        if (j < lines.length && FENCE_RE.test(lines[j]) && j === i + (j - i)) {
          // 从 i 起只有空行就到了围栏 → 合并
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

// 测试样本
const samples = {
  "A 标准 mermaid 围栏": "给你画一个时序图：\n\n```mermaid\nsequenceDiagram\nautonumber\nactor U as 用户\nparticipant S as 服务\nU->>S: 请求\nS-->>U: 响应\n```\n",
  "B 无语言围栏(自动识别)": "图如下：\n\n```\nsequenceDiagram\nU->>S: 请求\n```\n",
  "C 头部裸段落+无语言围栏(rescue合并)": "时序图：\n\nsequenceDiagram\nautonumber\nactor U as 用户\nparticipant S as 服务\n\n```\nU->>S: 请求\nS-->>U: 响应\nalt ok\nA->>B: x\nelse bad\nA->>B: y\nend\n```\n",
  "D 流式中未闭合围栏": "画图中：\n\n```mermaid\nsequenceDiagram\nU->>S: 请求\n",
  "E 真实坏消息(整段裸图,来自sessions.json)": '给你画一个通用的示例时序图（用户登录 + 鉴权流程），直接可渲染：\n\nsequenceDiagram\n  autonumber\n  participant U as 用户\n  participant C as 客户端\n  participant G as 网关/API\n  participant A as 认证服务\n  participant D as 数据库\n\n  U->>C: 输入账号密码\n  C->>G: POST /login {user, pwd}\n  G->>A: 转发登录请求\n  A->>D: 查询用户记录\n  D-->>A: 返回 salt + hash\n  A->>A: 校验密码哈希\n\n  alt 校验通过\n    A-->>G: 签发 token (JWT)\n    G-->>C: 200 OK + token\n  else 校验失败\n    A-->>G: 401 认证失败\n  end\n\n如果你要的是**特定业务**的时序图，把参与者名称告诉我。',
  "F 普通散文不能误伤": "这个 graph TB 的话题很有意思，我们聊聊别的吧。\n\n今天天气不错。",
};

function render(name, src) {
  const rescued = rescueMermaid(src);
  console.log(`\n===== ${name} =====`);
  console.log("--- rescueMermaid 后的 markdown ---");
  console.log(rescued);
  try {
    const html = renderToStaticMarkup(
      React.createElement(
        ReactMarkdown,
        {
          remarkPlugins: [remarkGfm, remarkMath, remarkBreaks],
          rehypePlugins: [rehypeRaw, [rehypeSanitize, sanitizeSchema], [rehypeKatex, { throwOnError: false }]],
          components: {
            code: (props) => {
              const { children, ...rest } = props;
              const desc = Array.isArray(children)
                ? children.map((c) => (typeof c === "string" ? `str(${c.length})` : typeof c + ":" + JSON.stringify(c)?.slice(0, 60))).join(", ")
                : typeof children === "string" ? `str(${children.length})` : typeof children;
              console.log(`[code] className=${JSON.stringify(rest.className)} children=[${desc}]`);
              return React.createElement("code", null, Array.isArray(children) ? children.map(String).join("") : String(children));
            },
          },
        },
        rescued,
      ),
    );
    console.log("--- 渲染产物（前 300 字符）---");
    console.log(html.slice(0, 300));
  } catch (e) {
    console.log("!!! 渲染抛异常:", e.message);
  }
}

for (const [name, src] of Object.entries(samples)) render(name, src);
console.log("\n全部样本通过（无异常 = 管线本身不会产生 [object Object]）");
