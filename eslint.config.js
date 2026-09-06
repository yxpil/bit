import js from "@eslint/js";
import globals from "globals";

export default [
  {
    ignores: ["dist/**", "node_modules/**", "src-tauri/**", "e2e/**", "installer/**"],
  },
  js.configs.recommended,
  {
    files: ["src/**/*.{js,jsx}"],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "module",
      parserOptions: { ecmaFeatures: { jsx: true } },
      globals: { ...globals.browser },
    },
    rules: {
      // v0.5.14 黑屏事故回归闸门：漏声明变量在渲染期抛 ReferenceError，React 整树卸载 → 黑屏。
      // 此规则让同类错误在构建前（而非用户安装后）暴露。
      "no-undef": "error",
      "no-unused-vars": "warn",
    },
  },
];
