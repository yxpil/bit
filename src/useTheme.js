import { useEffect, useState } from "react";

const KEY = "bit.theme.v1";
const ACCENT_KEY = "bit.accent.v1";
const LOOK_KEY = "bit.look.v1";

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

// 外观配置：背景色/透明度/图片/圆角，统一走 CSS 变量
// look 结构：{ enabled, bgColorLight, bgColorDark, bgOpacity, bgImage, borderOn, borderRadius, cardRadius, shadowOn }
function applyLook(look) {
  const root = document.documentElement;
  if (!look || !look.enabled) {
    // 恢复默认
    root.style.removeProperty("--app-bg-image");
    root.style.removeProperty("--look-border-radius");
    root.style.removeProperty("--look-card-radius");
    root.style.removeProperty("--look-shadow");
    root.style.removeProperty("--look-bg-color-light");
    root.style.removeProperty("--look-bg-color-dark");
    root.style.removeProperty("--look-bg-opacity");
    root.classList.remove("no-border", "no-shadow", "has-bg-image");
    return;
  }
  // 背景色（light/dark 各自生效）
  if (look.bgColorLight) root.style.setProperty("--look-bg-color-light", look.bgColorLight);
  else root.style.removeProperty("--look-bg-color-light");
  if (look.bgColorDark) root.style.setProperty("--look-bg-color-dark", look.bgColorDark);
  else root.style.removeProperty("--look-bg-color-dark");

  // 透明度 + 背景图
  const op = look.bgOpacity != null ? look.bgOpacity : 100;
  root.style.setProperty("--look-bg-opacity", op + "%");
  if (look.bgImage) {
    root.style.setProperty("--app-bg-image", `url("${look.bgImage}")`);
    root.classList.add("has-bg-image");
  } else {
    root.style.removeProperty("--app-bg-image");
    root.classList.remove("has-bg-image");
  }

  // 圆角
  root.style.setProperty("--look-border-radius", (look.borderRadius ?? 0) + "px");
  root.style.setProperty("--look-card-radius", (look.cardRadius ?? 24) + "px");

  // 边框 & 阴影开关
  root.classList.toggle("no-border", !look.borderOn);
  root.classList.toggle("no-shadow", !look.shadowOn);
}

const DEFAULT_LOOK = {
  enabled: false,
  bgColorLight: "#ffffff",
  bgColorDark: "#0a0a0a",
  bgOpacity: 100,
  bgImage: "",
  borderOn: true,
  borderRadius: 0,
  cardRadius: 24,
  shadowOn: true,
};

/** 主题：light / dark / auto + 自定义强调色（--accent），均持久化到 localStorage */
export function useTheme() {
  const [mode, setMode] = useState(() => localStorage.getItem(KEY) || "light");
  const [isDark, setIsDark] = useState(() => apply(localStorage.getItem(KEY) || "light"));
  const [accent, setAccentState] = useState(() => localStorage.getItem(ACCENT_KEY) || "");
  const [look, setLookState] = useState(() => {
    try {
      const raw = localStorage.getItem(LOOK_KEY);
      if (raw) return { ...DEFAULT_LOOK, ...JSON.parse(raw) };
    } catch {}
    return { ...DEFAULT_LOOK };
  });

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

  // 外观配置变化立即生效
  useEffect(() => {
    localStorage.setItem(LOOK_KEY, JSON.stringify(look));
    applyLook(look);
  }, [look]);

  // 亮 → 暗 → 亮，简单二态切换（长按/右键可扩展 auto）
  const toggle = () => setMode((m) => (isDark ? "light" : "dark"));

  return { mode, isDark, setMode, toggle, accent, setAccent: setAccentState, look, setLook: setLookState };
}

export { DEFAULT_LOOK };
