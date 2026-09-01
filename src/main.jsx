import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App.jsx";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);

// 首帧绘制完成后显示窗口（启动期间窗口隐藏，避免 WebView 加载期白屏）；
// 兜底定时器防止渲染异常时窗口永不显示
const win = getCurrentWindow();
const showWindow = () => win.show().catch(() => {});
requestAnimationFrame(() => requestAnimationFrame(showWindow));
setTimeout(showWindow, 3000);
