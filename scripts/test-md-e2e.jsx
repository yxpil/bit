// E2E：用真实 Markdown.jsx（vite SSR 构建）渲染真实事故内容
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import Markdown from "../src/components/Markdown.jsx";

const realCase =
  "给你画一个通用的示例时序图（用户登录 + 鉴权流程），直接可渲染：\n\n" +
  "sequenceDiagram\n  autonumber\n  participant U as 用户\n  participant C as 客户端\n  participant G as 网关/API\n  participant A as 认证服务\n  participant D as 数据库\n\n" +
  "  U->>C: 输入账号密码\n  C->>G: POST /login {user, pwd}\n  G->>A: 转发登录请求\n  A->>D: 查询用户记录\n  D-->>A: 返回 salt + hash\n  A->>A: 校验密码哈希\n\n" +
  "  alt 校验通过\n    A-->>G: 签发 token (JWT)\n    G-->>C: 200 OK + token\n  else 校验失败\n    A-->>G: 401 认证失败\n  end\n\n" +
  "如果你要的是**特定业务**的时序图，把参与者名称告诉我。";

const cases = {
  "真实事故(整段裸图)": realCase,
  "标准mermaid围栏": "```mermaid\nsequenceDiagram\nU->>S: hi\n```\n",
  "无语言围栏(自动识别)": "```\nsequenceDiagram\nU->>S: hi\n```\n",
  "普通代码块不受影响": "```python\nprint('hello')\n```\n",
  "散文不动": "这个 graph TB 的话题很有意思。\n\n今天天气不错。",
  "真实ER事故(裸文本+实体块)": "先给你画一个**通用电商系统**的 ER 图作示例：\n\nerDiagram\n    USER      ||--o{ ADDRESS    : \"收货地址\"\n    USER      ||--o{ ORDER      : \"下单\"\n    CATEGORY  |o--o{ CATEGORY   : \"父分类\"\n    ORDER     ||--|{ ORDER_ITEM : \"包含明细\"\n\n    USER {\n        bigint   id PK\n        string   username\n    }\n\nN）；`|o--o{` = 右侧可关联。要不要我按你的实际业务改一版？",
};

let fail = 0;
for (const [name, src] of Object.entries(cases)) {
  try {
    const html = renderToStaticMarkup(React.createElement(Markdown, null, src));
    const hasDiagram = html.includes("mermaid 渲染中") || html.includes("<svg");
    const hasObject = html.includes("[object Object]");
    const summary = html.length + " chars, diagram=" + hasDiagram + ", objectLiteral=" + hasObject;
    console.log(`[${name}] ${summary}`);
    if (hasObject) { console.log("  !!! 出现 [object Object]"); fail++; }
    if (name !== "普通代码块不受影响" && name !== "散文不动" && !hasDiagram) { console.log("  !!! 应渲染成图却没有"); fail++; }
    if (name === "普通代码块不受影响" && hasDiagram) { console.log("  !!! 普通代码被误判为图"); fail++; }
    if (name === "散文不动" && (hasDiagram || html.includes("```"))) { console.log("  !!! 散文被误改"); fail++; }
  } catch (e) {
    console.log(`[${name}] !!! 渲染异常: ${e.message}`);
    fail++;
  }
}
console.log(fail === 0 ? "\nE2E 全部通过" : `\nE2E 失败 ${fail} 项`);
process.exit(fail === 0 ? 0 : 1);
