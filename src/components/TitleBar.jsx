import { getCurrentWindow } from "@tauri-apps/api/window";
import logoUrl from "../assets/logo-64.png";
import { IconMinus, IconSquare, IconX } from "./Icons.jsx";
import { useLang } from "../i18n.js";

// 自定义无边框标题栏（替代原生菜单栏）：左侧品牌，右侧窗口按钮，中段可拖动
export default function TitleBar() {
  const { t } = useLang();
  const win = () => getCurrentWindow();
  const minimize = () => win().minimize().catch(() => {});
  const toggleMax = () => win().toggleMaximize().catch(() => {});
  // 关闭走 CloseRequested → 隐藏到托盘（后端已拦截），此处直接调用即可
  const close = () => win().close().catch(() => {});

  return (
    <div
      data-tauri-drag-region
      className="flex h-9 shrink-0 select-none items-center gap-2 border-b border-neutral-200/70 bg-white/80 px-3 backdrop-blur-md dark:border-neutral-800/70 dark:bg-black/50"
    >
      {/* 品牌区（也可拖动） */}
      <div data-tauri-drag-region className="flex items-center gap-2">
        <img src={logoUrl} alt="BIT" className="h-4 w-4 rounded-full" />
        <span className="text-xs font-semibold tracking-wide text-neutral-700 dark:text-neutral-300">
          BIT
        </span>
      </div>

      {/* 弹性拖动区 */}
      <div data-tauri-drag-region className="flex-1" />

      {/* 窗口按钮 */}
      <button
        onClick={minimize}
        aria-label={t("title.minimize")}
        className="flex h-7 w-7 items-center justify-center rounded-full text-neutral-500 transition-colors hover:bg-neutral-200 hover:text-neutral-900 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
      >
        <IconMinus size={13} />
      </button>
      <button
        onClick={toggleMax}
        aria-label={t("title.maximize")}
        className="flex h-7 w-7 items-center justify-center rounded-full text-neutral-500 transition-colors hover:bg-neutral-200 hover:text-neutral-900 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
      >
        <IconSquare size={12} />
      </button>
      <button
        onClick={close}
        aria-label={t("common.close")}
        className="flex h-7 w-7 items-center justify-center rounded-full text-neutral-500 transition-colors hover:bg-red-600 hover:text-white"
      >
        <IconX size={14} />
      </button>
    </div>
  );
}
