import { useEffect, useState } from "react";

const KEY = "bit.theme.v1";
const ACCENT_KEY = "bit.accent.v1";

function apply(mode) {
  const isDark =
    mode === "dark" ||
    (mode === "auto" && window.matchMedia?.("(prefers-color-scheme: dark)").matches);
  const root = document.documentElement;
  root.classList.toggle("dark", isDark);
  root.style.colorScheme = isDark ? "dark" : "light";
  return isDark;
}

// 按亮度自动决定强调色上的前景色（深色字 / 白字）
function applyAccent(color) {
  const root = document.documentElement;
  if (!color) {
    root.style.removeProperty("--accent");
    root.style.removeProperty("--accent-fg");
    return;
  }
  root.style.setProperty("--accent", color);
  const m = color.replace("#", "");
  const r = parseInt(m.slice(0, 2), 16);
  const g = parseInt(m.slice(2, 4), 16);
  const b = parseInt(m.slice(4, 6), 16);
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  root.style.setProperty("--accent-fg", lum > 0.6 ? "#171717" : "#ffffff");
}

/** 主题：light / dark / auto + 自定义强调色（--accent），均持久化到 localStorage */
export function useTheme() {
  const [mode, setMode] = useState(() => localStorage.getItem(KEY) || "light");
  const [isDark, setIsDark] = useState(() => apply(localStorage.getItem(KEY) || "light"));
  const [accent, setAccentState] = useState(() => localStorage.getItem(ACCENT_KEY) || "");

  useEffect(() => {
    localStorage.setItem(KEY, mode);
    setIsDark(apply(mode));
    if (mode !== "auto") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setIsDark(apply("auto"));
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [mode]);

  // 强调色变化立即生效（首次启动也恢复上次选择）
  useEffect(() => {
    localStorage.setItem(ACCENT_KEY, accent);
    applyAccent(accent);
  }, [accent]);

  // 亮 → 暗 → 亮，简单二态切换（长按/右键可扩展 auto）
  const toggle = () => setMode((m) => (isDark ? "light" : "dark"));

  return { mode, isDark, setMode, toggle, accent, setAccent: setAccentState };
}
