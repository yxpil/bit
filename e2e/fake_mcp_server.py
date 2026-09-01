#!/usr/bin/env python3
"""
Fake MCP Server（Streamable HTTP / JSON-RPC 2.0）— 用于测试 BIT 的 MCP 客户端
实现: initialize 握手 → notifications/initialized → tools/list → tools/call
工具做真实计算: echo(原样回显) / add(加法) / now(当前时间)
响应故意混合两种格式: initialize+tools/call 用 application/json, tools/list 用 text/event-stream
"""
import json
import time
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SESSION = "fake-mcp-session-0001"
PORT = 9801

TOOLS = [
    {
        "name": "echo",
        "description": "原样回显输入的 message，用于连通性测试",
        "inputSchema": {
            "type": "object",
            "properties": {"message": {"type": "string", "description": "要回显的文本"}},
            "required": ["message"],
        },
    },
    {
        "name": "add",
        "description": "计算两个数字之和",
        "inputSchema": {
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"],
        },
    },
    {
        "name": "now",
        "description": "返回服务器当前时间",
        "inputSchema": {"type": "object", "properties": {}},
    },
]

stats = {"initialize": 0, "notifications/initialized": 0, "tools/list": 0, "tools/call": 0}


def handle_rpc(req):
    method = req.get("method", "")
    params = req.get("params") or {}
    stats[method] = stats.get(method, 0) + 1

    if method == "initialize":
        return (
            200,
            "application/json",
            {
                "jsonrpc": "2.0",
                "id": req.get("id"),
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "Fake MCP Toolkit", "version": "0.1.0"},
                },
            },
            {"Mcp-Session-Id": SESSION},
        )

    if method == "notifications/initialized":
        return 202, "text/plain", "", None

    if method == "tools/list":
        # SSE 格式响应，测试 BIT 客户端对 event-stream 的解析
        sse = (
            "event: message\r\ndata: "
            + json.dumps(
                {"jsonrpc": "2.0", "id": req.get("id"), "result": {"tools": TOOLS}},
                ensure_ascii=False,
            )
            + "\r\n\r\n"
        )
        return 200, "text/event-stream", sse, None

    if method == "tools/call":
        name = params.get("name")
        args = params.get("arguments") or {}
        if name == "echo":
            text = f"ECHO<{args.get('message', '')}>"
        elif name == "add":
            try:
                s = float(args.get("a", 0)) + float(args.get("b", 0))
                text = f"SUM={int(s) if s.is_integer() else s}"
            except (TypeError, ValueError):
                return (
                    200,
                    "application/json",
                    {"jsonrpc": "2.0", "id": req.get("id"),
                     "result": {"content": [{"type": "text", "text": "参数 a/b 必须是数字"}], "isError": True}},
                    None,
                )
        elif name == "now":
            text = f"NOW={datetime.now().strftime('%Y-%m-%d %H:%M:%S')}"
        else:
            return (
                200,
                "application/json",
                {"jsonrpc": "2.0", "id": req.get("id"),
                 "result": {"content": [{"type": "text", "text": f"未知工具 {name}"}], "isError": True}},
                None,
            )
        return (
            200,
            "application/json",
            {"jsonrpc": "2.0", "id": req.get("id"),
             "result": {"content": [{"type": "text", "text": text}], "isError": False}},
            None,
        )

    return (
        200,
        "application/json",
        {"jsonrpc": "2.0", "id": req.get("id"),
         "error": {"code": -32601, "message": f"method not found: {method}"}},
        None,
    )


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path.rstrip("/") != "/mcp":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", 0))
        try:
            req = json.loads(self.rfile.read(length))
        except json.JSONDecodeError:
            self.send_error(400)
            return
        status, ct, payload, extra = handle_rpc(req)
        body = payload if isinstance(payload, str) else json.dumps(payload, ensure_ascii=False)
        self.send_response(status)
        self.send_header("Content-Type", ct)
        self.send_header("Content-Length", str(len(body.encode("utf-8"))))
        if extra:
            for k, v in extra.items():
                self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body.encode("utf-8"))

    def do_GET(self):
        if self.path == "/stats":
            body = json.dumps({"stats": stats, "started": time.ctime()}, ensure_ascii=False)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body.encode("utf-8"))))
            self.end_headers()
            self.wfile.write(body.encode("utf-8"))
        else:
            self.send_error(404)

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    print(f"Fake MCP server listening on http://127.0.0.1:{PORT}/mcp")
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
