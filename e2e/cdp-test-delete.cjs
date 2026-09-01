// E2E：在真实 WebView 里验证记忆/技能的批量删除命令（添加测试数据→删除→确认消失）
const http = require("http");

function getList() {
  return new Promise((resolve, reject) => {
    http
      .get("http://127.0.0.1:9333/json/list", (res) => {
        let d = "";
        res.on("data", (c) => (d += c));
        res.on("end", () => resolve(JSON.parse(d)));
      })
      .on("error", reject);
  });
}

const EXPR = `
(async () => {
  const inv = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
  const out = {};
  // 记忆：加 2 条 → 删 1 条 → 确认
  const m1 = await inv('add_memory', { content: '__E2E_DEL_TEST_A__' });
  const m2 = await inv('add_memory', { content: '__E2E_DEL_TEST_B__' });
  const beforeM = (await inv('list_memories')).memories.length;
  const delM = await inv('delete_memories', { ids: [m1.memory.id, m2.memory.id] });
  const afterM = (await inv('list_memories')).memories;
  out.memory = { beforeM, removed: delM.removed, afterM, residual: afterM.filter(m => m.content.includes('__E2E_DEL_TEST')).length };
  // 技能：加 1 条 → 批量删（连同原有 ai-self 一并保留，只删测试条）→ 确认
  const s1 = await inv('add_skill', { name: '__e2e_del_test__', summary: 'temp' });
  const delS = await inv('delete_skills', { ids: [s1.skill.id] });
  const afterS = (await inv('list_skills')).skills;
  out.skill = { removed: delS.removed, residual: afterS.filter(s => s.name === '__e2e_del_test__').length };
  return JSON.stringify(out);
})()
`;

async function main() {
  const pages = (await getList()).filter((t) => t.type === "page");
  if (!pages.length) return console.log("NO_PAGE");
  const ws = new WebSocket(pages[0].webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  const result = await new Promise((resolve) => {
    ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id === 1) resolve(msg);
    };
    ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate", params: { expression: EXPR, returnByValue: true, awaitPromise: true } }));
  });
  console.log("[RESULT]", result.result?.result?.value ?? JSON.stringify(result.result));
  process.exit(0);
}

main().catch((e) => {
  console.error("PROBE_ERROR", e.message);
  process.exit(1);
});
