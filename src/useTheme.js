import { useEffect, useState } from "react";

const KEY = "bit.theme.v1";

function apply(mode) {
  const isDark =
    mode === "dark" ||
    (mode === "auto" && window.matchMedia?.("(prefers-color-scheme: dark)").matches);
  const root = document.documentElement;
  root.classList.toggle("dark", isDark);
  root.style.colorScheme = isDark ? "dark" : "light";
  return isDark;
}

/** 主题：light / dark / auto，持久化到 localStorage，auto 跟随系统 */
export function useTheme() {
  const [mode, setMode] = useState(() => localStorage.getItem(KEY) || "light");
  const [isDark, setIsDark] = useState(() => apply(localStorage.getItem(KEY) || "light"));

  useEffect(() => {
    localStorage.setItem(KEY, mode);
    setIsDark(apply(mode));
    if (mode !== "auto") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setIsDark(apply("auto"));
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [mode]);

  // 亮 → 暗 → 亮，简单二态切换（长按/右键可扩展 auto）
  const toggle = () => setMode((m) => (isDark ? "light" : "dark"));

  return { mode, isDark, setMode, toggle };
}
