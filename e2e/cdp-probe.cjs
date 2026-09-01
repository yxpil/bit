// CDP 诊断：连接 WebView2 调试端口，检查页面加载状态与 console 错误（Node >=22 原生 WebSocket）
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

async function main() {
  const pages = (await getList()).filter((t) => t.type === "page");
  if (!pages.length) return console.log("NO_PAGE");
  const ws = new WebSocket(pages[0].webSocketDebuggerUrl);
  let id = 0;
  const pending = new Map();
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      const mid = ++id;
      pending.set(mid, resolve);
      ws.send(JSON.stringify({ id: mid, method, params }));
    });
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    }
    if (msg.method === "Runtime.exceptionThrown")
      console.log("[EXCEPTION]", JSON.stringify(msg.params.exceptionDetails).slice(0, 800));
    if (msg.method === "Log.entryAdded")
      console.log("[LOG]", msg.params.entry.level, String(msg.params.entry.text).slice(0, 300));
    if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error")
      console.log("[CONSOLE.ERROR]", JSON.stringify(msg.params.args).slice(0, 500));
    if (msg.method === "Page.frameNavigated")
      console.log("[NAV]", JSON.stringify(msg.params.frame.url ?? ""));
    if (msg.method === "Page.frameStoppedLoading")
      console.log("[LOADED]", JSON.stringify(msg.params ?? ""));
  };
  await new Promise((r) => (ws.onopen = r));
  await send("Runtime.enable");
  await send("Log.enable");
  await send("Page.enable");
  await send("Page.reload");
  await new Promise((r) => setTimeout(r, 6000));
  const evalRes = await send("Runtime.evaluate", {
    expression:
      "JSON.stringify({readyState: document.readyState, rootChildren: document.getElementById('root')?.children.length ?? -1, bodyLen: document.body?.innerHTML?.length ?? -1, href: location.href})",
    returnByValue: true,
  });
  console.log("[STATE]", evalRes.result?.result?.value ?? JSON.stringify(evalRes));
  process.exit(0);
}

main().catch((e) => {
  console.error("PROBE_ERROR", e.message);
  process.exit(1);
});
