import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App.jsx";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);

// 首帧绘制完成后显示窗口（启动期间窗口隐藏，避免 WebView 加载期白屏）；
// 兜底定时器防止渲染异常时窗口永不显示。
// BIT_HEADLESS=1（E2E/专项测试）：窗口保持隐藏，不弹到用户屏幕
const showWhenVisible = () => {
  invoke("is_headless")
    .then((headless) => {
      if (headless) return;
      const win = getCurrentWindow();
      const showWindow = () => win.show().catch(() => {});
      requestAnimationFrame(() => requestAnimationFrame(showWindow));
      setTimeout(showWindow, 3000);
    })
    .catch(() => {});
};
showWhenVisible();
