// E2E 模拟上游 AI（OpenAI 兼容）：模仿 BIT 的上游提供方，驱动全功能工具调用测试
// 用法：node .e2e-mock-ai.cjs  (监听 127.0.0.1:9901)
const http = require("http");

const PORT = 9901;

function contentText(m) {
  // 多模态：content 可能是 [{type:"text",text}, {type:"image_url",...}] 数组
  if (typeof m.content === "string") return m.content;
  if (Array.isArray(m.content)) return m.content.map((p) => (p.type === "text" ? p.text : "")).join("\n");
  return "";
}

// 检测消息里是否带图片（OpenAI image_url / Claude source / Gemini inline_data）
function imageCount(messages) {
  let n = 0;
  for (const m of messages) {
    if (!Array.isArray(m.content)) continue;
    for (const p of m.content) {
      if (p.type === "image_url" && p.image_url?.url) n++;
      else if (p.type === "image" && p.source?.type === "base64") n++;
      else if (p.type === "image_url" && p.source) n++;
      else if (p.type === "image" && (p.inline_data || p.inlineData)) n++;
    }
  }
  return n;
}

function pickLastUser(messages) {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") return contentText(messages[i]);
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
  const chars = messages.reduce((n, m) => n + contentText(m).length, 0);
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
  // GET /v1/models：模拟 OpenAI 兼容模型列表（供 list_provider_models 集成测试）
  if (req.method === "GET" && req.url.startsWith("/v1/models")) {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(
      JSON.stringify({
        object: "list",
        data: [
          { id: "mock-model-a", object: "model", owned_by: "mock" },
          { id: "mock-model-b", object: "model", owned_by: "mock" },
        ],
      })
    );
  }
  if (req.method !== "POST") {
    res.writeHead(404);
    return res.end();
  }
  let body = "";
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    // Buffer 拼接后再转 utf8：直接 += 会在 chunk 边界切断多字节字符产生 U+FFFD
    body = Buffer.concat(chunks).toString("utf8");
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
    const all = messages.map((m) => contentText(m)).join("\n");
    const isFeedback = rounds > 0;

    // ── 图片场景：多模态消息到达即确认看见（在工具轮判断之前，图片消息无工具反馈） ──
    const imgs = imageCount(messages);
    if (imgs > 0) return respond(res, `E2E-IMAGE-SEEN count=${imgs}`, sse);

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

      // E2E-CMD-RETOOL: 轮0 注册工具 → 轮1 同名覆盖更新（改成三倍） → 轮2 调用 → 轮3 最终
      if (all.includes("E2E-CMD-RETOOL")) {
        if (rounds === 1) {
          const code2 =
            "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>{const p=JSON.parse(d||'{}');console.log(JSON.stringify({tripled:(p.a||0)*3}))});";
          return respond(
            res,
            `发现之前实现有误，覆盖更新同名工具：[{"tool":"add_tool","params":{"name":"e2e-doubler","description":"E2E 覆盖更新为三倍","runtime":"node","code":"${code2.replace(/"/g, '\\"')}"}}]`,
            sse
          );
        }
        if (rounds === 2)
          return respond(res, '覆盖成功，调用验证：[{"tool":"e2e-doubler","params":{"a":5}}]', sse);
        const tripled = (fb.match(/"tripled"\s*:\s*(\d+)/) || [])[1];
        if (tripled !== undefined) return respond(res, `E2E-FINAL-RETOOL tripled=${tripled}`, sse);
        return respond(res, "E2E-FINAL-RETOOL failed: 覆盖后调用无有效结果", sse);
      }

      // E2E-CMD-SEND: 轮0 write_file 生成文件 → 轮1 send_file 发送 → 轮2 最终
      if (all.includes("E2E-CMD-SEND")) {
        if (rounds === 1)
          return respond(res, '文件已生成，发送给你：[{"tool":"send_file","params":{"path":"./.e2e-send.txt","note":"E2E 交付文件"}}]', sse);
        const sent = /"sent"\s*:\s*true/.test(fb);
        if (sent) return respond(res, "E2E-FINAL-SEND: 文件已发送", sse);
        return respond(res, "E2E-FINAL-SEND failed: send_file 无有效结果", sse);
      }

      // E2E-CMD-DELTOOL: 轮0 add_tool 自建工具 → 轮1 删内置工具（应被拒） → 轮2 删自建工具 → 轮3 最终
      if (all.includes("E2E-CMD-DELTOOL")) {
        if (rounds === 1)
          return respond(res, '先试试删内置工具：[{"tool":"delete_tool","params":{"name":"shell"}}]', sse);
        if (rounds === 2) {
          const blocked = /不允许删除/.test(fb) && /"ok"\s*:\s*false/.test(fb);
          return respond(
            res,
            `内置删除被拒=${blocked}，接着删自建工具：[{"tool":"delete_tool","params":{"name":"e2e-temp-tool"}}]`,
            sse
          );
        }
        const deleted = /"deleted"\s*:\s*"e2e-temp-tool"/.test(fb);
        return respond(res, `E2E-FINAL-DELTOOL builtin-blocked=true deleted=${deleted}`, sse);
      }

      // E2E-CMD-COMPACT: 轮0 compact_history 压缩 → 轮1 最终
      if (all.includes("E2E-CMD-COMPACT")) {
        const compacted = /"compacted"\s*:\s*true/.test(fb);
        return respond(res, `E2E-FINAL-COMPACT compacted=${compacted}`, sse);
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

    // 压测-上行完整性：回显收到的最后一条用户消息的统计（验证 BIT 完整转发大文本）
    if (last.includes("E2E-CMD-ECHO")) {
      try { require("fs").writeFileSync("/tmp/mock-echo-received.txt", last); } catch {}
      const payload = JSON.stringify({ len: last.length, head: last.slice(0, 24), tail: last.slice(-24) });
      return respond(res, `E2E-ECHO-STATS ${payload}`, sse);
    }

    // 压测-下行长文本：分块流式输出 50KB 确定性文本（可校验 head/tail 完整性）
    if (last.includes("E2E-CMD-LONG")) {
      const total = 50;
      const mk = (i) => `L${String(i).padStart(4, "0")}:` + "x".repeat(1000 - 6) + "\n";
      const full = Array.from({ length: total }, (_, i) => mk(i)).join("");
      if (sse) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        const usage = usageFor(messages);
        for (let i = 0; i < total; i += 5) {
          const chunk = { id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content: mk(i) + mk(i + 1) + mk(i + 2) + mk(i + 3) + mk(i + 4) } }] };
          res.write(`data: ${JSON.stringify(chunk)}\n\n`);
        }
        res.write(`data: ${JSON.stringify({ id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: {} }], usage })}\n\n`);
        res.write("data: [DONE]\n\n");
        return res.end();
      }
      return respond(res, full, sse);
    }

    // 压测-流中断：发两块内容后直接断开（不发 [DONE]），验证 BIT 不再把半截当完整回复
    if (last.includes("E2E-CMD-DROP")) {
      if (!sse) return respond(res, "E2E-DROP-需要流式请求", sse);
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write(`data: ${JSON.stringify({ id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content: "第一段内容。" } }] })}\n\n`);
      res.write(`data: ${JSON.stringify({ id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content: "第二段内容，马上就要断了" } }] })}\n\n`);
      setTimeout(() => res.destroy(), 50);
      return;
    }

    // 压测-max_tokens 截断：finish_reason=length（验证 BIT 显式标注截断而不是默默断句）
    if (last.includes("E2E-CMD-LENGTH")) {
      const content = "回答的前半部分，然后长度到上限了";
      if (sse) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write(`data: ${JSON.stringify({ id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content } }] })}\n\n`);
        res.write(`data: ${JSON.stringify({ id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: "length" }] })}\n\n`);
        res.write("data: [DONE]\n\n");
        return res.end();
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({
        id: "mock", object: "chat.completion",
        choices: [{ index: 0, message: { role: "assistant", content }, finish_reason: "length" }],
        usage: usageFor(messages),
      }));
    }

    // ── 模糊格式识别：不严格遵循 OpenAI 协议的变体响应（BIT 应动态识别而非报错） ──

    // content 为内容块数组（网关转换常见）：流式 delta.content 同样给数组
    if (last.includes("E2E-FMT-ARRAY")) {
      const blocks = [{ type: "text", text: "E2E-FMT-ARRAY" }, { type: "text", text: "-OK" }];
      if (sse) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write(`data: ${JSON.stringify({ id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content: blocks } }] })}\n\n`);
        res.write("data: [DONE]\n\n");
        return res.end();
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({ id: "mock", object: "chat.completion", choices: [{ index: 0, message: { role: "assistant", content: blocks } }], usage: usageFor(messages) }));
    }

    // 旧版 completions：choices[0].text（无 message）
    if (last.includes("E2E-FMT-LEGACY")) {
      if (sse) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write(`data: ${JSON.stringify({ id: "mock", object: "text_completion.chunk", choices: [{ index: 0, text: "E2E-FMT-LEGACY-OK" }] })}\n\n`);
        res.write("data: [DONE]\n\n");
        return res.end();
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({ id: "mock", object: "text_completion", choices: [{ index: 0, text: "E2E-FMT-LEGACY-OK" }], usage: usageFor(messages) }));
    }

    // Responses API 风格：顶层 output_text
    if (last.includes("E2E-FMT-OUTTEXT")) {
      if (sse) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write(`data: ${JSON.stringify({ id: "mock", type: "response.output_text.delta", output_text: "E2E-FMT-OUTTEXT-OK" })}\n\n`);
        res.write("data: [DONE]\n\n");
        return res.end();
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({ id: "mock", object: "response", output_text: "E2E-FMT-OUTTEXT-OK", usage: usageFor(messages) }));
    }

    // finish_reason 大写变体 MAX_TOKENS：应被归一化识别并显式标注截断
    if (last.includes("E2E-FMT-MAXTOK")) {
      const content = "E2E-FMT-MAXTOK-OK 回答到这里被截断";
      if (sse) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write(`data: ${JSON.stringify({ id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content } }] })}\n\n`);
        res.write(`data: ${JSON.stringify({ id: "mock", object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: "MAX_TOKENS" }] })}\n\n`);
        res.write("data: [DONE]\n\n");
        return res.end();
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({
        id: "mock", object: "chat.completion",
        choices: [{ index: 0, message: { role: "assistant", content }, finish_reason: "MAX_TOKENS" }],
        usage: usageFor(messages),
      }));
    }

    // 完全不可解析的响应体：BIT 必须报错而不是崩溃或吞掉
    if (last.includes("E2E-FMT-GARBAGE")) {
      if (sse) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write("data: not-json-garbage{{{\n\n");
        res.write("data: [DONE]\n\n");
        return res.end();
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end("not-json-garbage{{{");
    }

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

    // 覆盖更新场景：先注册翻倍工具，反馈轮里同名覆盖为三倍
    if (last.includes("E2E-CMD-RETOOL")) {
      const code =
        "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>{const p=JSON.parse(d||'{}');console.log(JSON.stringify({doubled:(p.a||0)*2}))});";
      const calls = JSON.stringify([
        { tool: "add_tool", params: { name: "e2e-doubler", description: "E2E 测试：把数字翻倍", runtime: "node", code } },
      ]);
      return respond(res, `我先创建一个翻倍工具。\n${calls}`, sse);
    }

    // 发送文件场景：先 write_file 生成，反馈轮里 send_file 发给用户
    if (last.includes("E2E-CMD-SEND"))
      return respond(
        res,
        '生成一个交付文件：\n[{"tool":"write_file","params":{"path":"./.e2e-send.txt","content":"hello from BIT e2e"}}]',
        sse
      );

    // 删除工具场景：先 add_tool 自建，反馈轮里先删内置（被拒）再删自建（成功）
    if (last.includes("E2E-CMD-DELTOOL")) {
      const code =
        "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>{const p=JSON.parse(d||'{}');console.log(JSON.stringify({echo:p}))});";
      return respond(
        res,
        `我先创建一个临时工具。\n${JSON.stringify([
          { tool: "add_tool", params: { name: "e2e-temp-tool", description: "E2E 临时工具", runtime: "node", code } },
        ])}`,
        sse
      );
    }

    // 看图场景：让 BIT 调 view_image，第二轮请求应带上注入的图片（imgs>0 分支回 IMAGE-SEEN）
    if (last.includes("E2E-CMD-VIEWIMG"))
      return respond(
        res,
        '我来看一下这张图：\n[{"tool":"view_image","params":{"path":"./.e2e-view.png"}}]',
        sse
      );

    // 压缩历史场景：让 BIT 调 compact_history，用摘要替换全部历史
    if (last.includes("E2E-CMD-COMPACT"))
      return respond(
        res,
        '我先把历史压缩成摘要。\n[{"tool":"compact_history","params":{"summary":"E2E-SUMMARY-MARK 用户要求压缩历史；关键结论：E2E 压缩测试"}}]',
        sse
      );

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
