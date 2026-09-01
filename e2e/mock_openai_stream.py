#!/usr/bin/env python3
"""模拟 OpenAI 兼容流式 API（SSE），用于测试 BIT 的流式输出与中断。

用法：
    python3 e2e/mock_openai_stream.py [端口]

BIT 中添加提供方（或用「AI 设置」预设后改地址）：
    协议: OpenAI   Base URL: http://127.0.0.1:9802/v1
    API Key: 任意（如 mock）   模型: mock-stream

行为：
    - 流式请求：把回复逐字分块下发（每块 60ms），便于观察打字机效果；
      若请求最后一轮的用户消息包含"慢"，每块延迟加到 500ms，便于测试会话中断
    - 模型输出最后一轮若包含"调用"，返回 tool_calls（shell echo mock-echo），
      用于验证流式端点下的原生工具调用
    - 非流式请求（stream!=true）：一次性返回完整回复
"""

import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 9802
REPLY = "这是来自本地模拟流式 API 的回复：你好，世界。" * 3


def sse(obj):
    return f"data: {json.dumps(obj, ensure_ascii=False)}\n\n".encode()


def user_text(body):
    msgs = body.get("messages") or []
    for m in reversed(msgs):
        if m.get("role") == "user":
            c = m.get("content")
            if isinstance(c, str):
                return c
            if isinstance(c, list):
                return "".join(p.get("text", "") for p in c if isinstance(p, dict))
    return ""


def tool_call_id():
    return f"mock-{time.time_ns()}"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            body = {}

        if "tool" in user_text(body).lower() and body.get("tools"):
            self._send_tool_call(body)
        elif body.get("stream"):
            self._send_stream(body)
        else:
            self._send_plain(body)

    def _reply_headers(self, ctype):
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Connection", "close")
        self.end_headers()

    def _send_plain(self, body):
        payload = json.dumps(
            {"id": "mock", "object": "chat.completion", "model": body.get("model", "mock"),
             "choices": [{"index": 0, "finish_reason": "stop",
                          "message": {"role": "assistant", "content": REPLY}}]},
            ensure_ascii=False).encode()
        self._reply_headers("application/json")
        self.wfile.write(f"Content-Length: {len(payload)}\r\n\r\n".encode())
        self.wfile.write(payload)

    def _send_stream(self, body):
        self._reply_headers("text/event-stream")
        text = user_text(body)
        delay = 0.5 if "慢" in text else 0.06
        try:
            for ch in REPLY:
                self.wfile.write(sse({"id": "mock", "object": "chat.completion.chunk",
                                      "model": body.get("model", "mock"),
                                      "choices": [{"index": 0, "finish_reason": None,
                                                   "delta": {"content": ch}}]}))
                self.wfile.flush()
                time.sleep(delay)
            self.wfile.write(sse({"id": "mock", "object": "chat.completion.chunk",
                                  "model": body.get("model", "mock"),
                                  "choices": [{"index": 0, "finish_reason": "stop",
                                               "delta": {}}]}))
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass  # 客户端中断断开连接属预期行为

    def _send_tool_call(self, body):
        cid = tool_call_id()
        args = json.dumps({"command": "echo mock-echo"}, ensure_ascii=False)
        self._reply_headers("application/json")
        payload = json.dumps(
            {"id": "mock", "object": "chat.completion", "model": body.get("model", "mock"),
             "choices": [{"index": 0, "finish_reason": "tool_calls",
                          "message": {"role": "assistant", "content": "",
                                      "tool_calls": [{"id": cid, "type": "function",
                                                      "function": {"name": "shell", "arguments": args}}]}}]},
            ensure_ascii=False).encode()
        self.wfile.write(f"Content-Length: {len(payload)}\r\n\r\n".encode())
        self.wfile.write(payload)


if __name__ == "__main__":
    print(f"mock openai stream api: http://127.0.0.1:{PORT}/v1/chat/completions")
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
