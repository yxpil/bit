// E2E 模拟上游 AI（OpenAI 兼容）：模仿 BIT 的上游提供方，驱动全功能工具调用测试
// 用法：node .e2e-mock-ai.cjs  (监听 127.0.0.1:9901)
const http = require("http");

const PORT = 9901;

function pickLastUser(messages) {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") return messages[i].content || "";
  }
  return "";
}

function toolResultCount(messages) {
  // 文本协议：user 消息以「工具调用结果」开头；原生 function calling：role="tool" 消息
  return messages.filter(
    (m) =>
      (m.role === "user" && String(m.content || "").startsWith("工具调用结果")) ||
      m.role === "tool"
  ).length;
}

// 从工具反馈里提取结果文本（用于在最终回复中回显断言特征）
function feedbackText(messages) {
  const lastTool = [...messages].reverse().find((m) => m.role === "tool");
  if (lastTool) return String(lastTool.content || "");
  const last = [...messages].reverse().find((m) => m.role === "user" && String(m.content || "").startsWith("工具调用结果"));
  return last ? String(last.content) : "";
}

// 模拟 token 用量：输入随历史增长；工具反馈轮之后命中缓存（前缀一致）→ cached_tokens 约 80%
function usageFor(messages) {
  const chars = messages.reduce((n, m) => n + String(m.content || "").length, 0);
  const prompt = Math.max(120, Math.floor(chars / 4));
  const cached = toolResultCount(messages) > 0 ? Math.floor(prompt * 0.8) : 0;
  return {
    prompt_tokens: prompt,
    completion_tokens: 42,
    prompt_tokens_details: { cached_tokens: cached },
  };
}

function respondMsg(res, payload, sse, messages) {
  const usage = usageFor(messages || []);
  if (sse) {
    res.writeHead(200, { "Content-Type": "text/event-stream" });
    const chunk = { id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content: payload } }], usage };
    res.write(`data: ${JSON.stringify(chunk)}\n\n`);
    res.write("data: [DONE]\n\n");
    res.end();
  } else {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ id: "mock", object: "chat.completion", choices: [{ index: 0, message: { role: "assistant", content: payload } }], usage }));
  }
}

const server = http.createServer((req, res) => {
  if (req.method !== "POST") {
    res.writeHead(404);
    return res.end();
  }
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    let parsed;
    try {
      parsed = JSON.parse(body);
    } catch {
      res.writeHead(400);
      return res.end("{}");
    }
    const messages = parsed.messages || [];
    // 带上本次请求历史，便于模拟用量统计
    const respond = (r, p, s) => respondMsg(r, p, s, messages);
    const sse = !!parsed.stream;
    const last = pickLastUser(messages);
    const rounds = toolResultCount(messages);
    const fb = feedbackText(messages);
    // 场景标记可能出现在任意轮的用户消息里，用全历史匹配
    const all = messages.map((m) => String(m.content || "")).join("\n");
    const isFeedback = rounds > 0;

    // ── 工具反馈轮：按场景与轮次决定继续调用还是给最终答案 ──
    if (isFeedback) {
      // 记忆/技能沉淀等后台请求：直接给个普通文本，避免触发更多工具
      if (last.includes("沉淀") || last.includes("总结")) return respond(res, "已完成后台整理。", sse);

      // E2E-CMD-FILES: 轮0 写文件 → 轮1 编辑文件 → 轮2 最终
      if (all.includes("E2E-CMD-FILES")) {
        if (rounds === 1)
          return respond(
            res,
            '写入成功，接着编辑它：[{"tool":"edit","params":{"path":"./.e2e-tmp.txt","old_string":"alpha","new_string":"alpha-beta"}}]',
            sse
          );
        return respond(res, "E2E-FINAL-FILES: 文件写入与编辑完成", sse);
      }

      // E2E-CMD-SKILL: 轮0 保存技能 → 轮1 搜索技能 → 轮2 最终
      if (all.includes("E2E-CMD-SKILL")) {
        if (rounds === 1)
          return respond(
            res,
            '保存成功，再搜一下：[{"tool":"skill","params":{"action":"search","query":"e2e"}}]',
            sse
          );
        return respond(res, "E2E-FINAL-SKILL: 技能保存与搜索完成", sse);
      }

      // E2E-CMD-ADDTOOL: 轮0 AI 自建工具（add_tool 注册 node 脚本）→ 轮1 立即调用新工具 → 轮2 最终
      if (all.includes("E2E-CMD-ADDTOOL")) {
        if (rounds === 1)
          return respond(res, '工具注册成功，立即调用它：[{"tool":"e2e-doubler","params":{"a":21}}]', sse);
        const doubled = (fb.match(/"doubled"\s*:\s*(\d+)/) || [])[1];
        if (doubled !== undefined) return respond(res, `E2E-FINAL-ADDTOOL doubled=${doubled}`, sse);
        return respond(res, "E2E-FINAL-ADDTOOL failed: 新工具调用无有效结果", sse);
      }

      // 其余场景（shell / markup / multi / plan）一轮工具即完成；回显所有工具的 stdout（单轮多工具场景）
      const stdouts = messages
        .filter((m) => m.role === "tool")
        .map((m) => (String(m.content || "").match(/"stdout"\s*:\s*"([^"]*)"/) || [])[1] || "")
        .filter(Boolean)
        .join(" ");
      const echo = stdouts || (fb.match(/"stdout"\s*:\s*"([^"]*)"/) || [])[1] || "";
      return respond(res, `E2E-FINAL-OK stdout=「${echo}」`, sse);
    }

    // ── 用户轮：按场景标记返回工具调用（含 BIT 文本协议的各种变体） ──
    if (last.includes("E2E-CMD-SHELL"))
      return respond(res, '好的，我来执行命令。\n[{"tool":"shell","params":{"command":"echo e2e-shell-ok"}}]', sse);

    // AI 自我扩展：注册一个 node 脚本工具（读 stdin 的 params，输出 JSON）
    if (last.includes("E2E-CMD-ADDTOOL")) {
      const code =
        "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>{const p=JSON.parse(d||'{}');console.log(JSON.stringify({doubled:(p.a||0)*2}))});";
      const calls = JSON.stringify([
        {
          tool: "add_tool",
          params: {
            name: "e2e-doubler",
            description: "E2E 测试：把数字翻倍",
            runtime: "node",
            code,
          },
        },
      ]);
      return respond(res, `我来给自己创建一个翻倍工具。\n${calls}`, sse);
    }

    if (last.includes("E2E-CMD-MARKUP"))
      // v0.1.9 兼容场景：自创标记 + 裸对象（非数组）
      return respond(res, '<dots_function_call> {"tool":"shell","params":{"command":"echo e2e-markup-ok"}}', sse);

    if (last.includes("E2E-CMD-MULTI"))
      return respond(
        res,
        '连续执行两个命令：\n[{"tool":"shell","params":{"command":"echo e2e-multi-a"}},{"tool":"shell","params":{"command":"echo e2e-multi-b"}}]',
        sse
      );

    if (last.includes("E2E-CMD-FILES"))
      return respond(
        res,
        '先写入文件：\n[{"tool":"write_file","params":{"path":"./.e2e-tmp.txt","content":"alpha"}}]',
        sse
      );

    if (last.includes("E2E-CMD-PLAN"))
      return respond(
        res,
        '记录一个待办：\n[{"tool":"plan","params":{"goal":"E2E 待办","steps":["验证 plan 工具"]}}]',
        sse
      );

    if (last.includes("E2E-CMD-SKILL"))
      return respond(
        res,
        '保存一条技能：\n[{"tool":"skill","params":{"action":"save","name":"e2e-test-skill","description":"端到端测试技能","content":"console.log(1)"}}]',
        sse
      );

    // 普通对话
    if (last.includes("E2E-PLAIN")) return respond(res, "E2E-FINAL-PLAIN: 你好，普通对话正常。", sse);

    // 未知请求（autopilot 等后台调用兜底）
    respond(res, "好的。", sse);
  });
});

server.listen(PORT, "127.0.0.1", () => console.log(`mock upstream AI listening on http://127.0.0.1:${PORT}/v1`));
