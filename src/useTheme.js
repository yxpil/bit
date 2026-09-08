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
    root.style.removeProperty("--look-card-radius");
    root.style.removeProperty("--look-bg-color-light");
    root.style.removeProperty("--look-bg-color-dark");
    root.style.removeProperty("--look-bg-opacity");
    root.classList.remove("no-border", "no-shadow", "has-bg-image", "has-transparency");
    return;
  }
  // 背景色（light/dark 各自生效）
  if (look.bgColorLight) root.style.setProperty("--look-bg-color-light", look.bgColorLight);
  else root.style.removeProperty("--look-bg-color-light");
  if (look.bgColorDark) root.style.setProperty("--look-bg-color-dark", look.bgColorDark);
  else root.style.removeProperty("--look-bg-color-dark");

  // 透明度：背景 + 卡片/pill/chip 也跟着半透明（不透明度 = 背景透明度 + 15%，保证内容可读）
  const op = look.bgOpacity != null ? look.bgOpacity : 100;
  root.style.setProperty("--look-bg-opacity", op + "%");
  // 卡片/pill 的 alpha：背景越透明，卡片也相应透出，但始终比背景高 15% 保证可读性
  const cardAlpha = Math.min(1, Math.max(0.3, (op + 15) / 100));
  root.style.setProperty("--look-card-alpha", cardAlpha.toFixed(2));
  if (look.bgImage) {
    root.style.setProperty("--app-bg-image", `url("${look.bgImage}")`);
    root.classList.add("has-bg-image");
  } else {
    root.style.removeProperty("--app-bg-image");
    root.classList.remove("has-bg-image");
  }

  // 圆角
  root.style.setProperty("--look-card-radius", (look.cardRadius ?? 24) + "px");

  // 边框 & 阴影开关
  root.classList.toggle("no-border", !look.borderOn);
  root.classList.toggle("no-shadow", !look.shadowOn);
  // 透明度联动：背景 < 100% 时给卡片/按钮也加半透明
  root.classList.toggle("has-transparency", op < 100);
}

// 默认即「启用外观定制」：开箱即带主题色、背景色、透明度，不再回退到黑白胶囊风
const DEFAULT_LOOK = {
  enabled: true,
  bgColorLight: "#ffffff",
  bgColorDark: "#18181b",
  bgOpacity: 90,
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
  // 默认主题色：橙色 #ea580c（与外观定制页推荐配置一致，避免开箱是彩虹无主题态）
  const [accent, setAccentState] = useState(() => localStorage.getItem(ACCENT_KEY) || "#ea580c");
  const [look, setLookState] = useState(() => {
    try {
      const raw = localStorage.getItem(LOOK_KEY);
      if (raw) return { ...DEFAULT_LOOK, ...JSON.parse(raw) };
    } catch { /* ignore bad JSON */ }
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
