import { IconPauseBars, IconPlayTri } from "./Icons.jsx";

// 复刻 PLocalSwitch 的启用/暂停圆钮：
// 启用 = 黑色实心圆（暗色白底）+ 暂停竖条；暂停 = 灰圆 + 播放三角。
// 点击即在启用 / 暂停间切换。
export default function PillSwitch({ checked, onChange, disabled, size = "md", title }) {
  const box = size === "sm" ? "h-7 w-7" : "h-9 w-9";
  const icon = size === "sm" ? 12 : 15;

  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      title={title}
      onClick={() => !disabled && onChange?.(!checked)}
      className={`flex ${box} shrink-0 items-center justify-center rounded-full transition-all duration-200 ${
        checked
          ? "bg-neutral-900 text-white hover:bg-neutral-700 dark:bg-white dark:text-black dark:hover:bg-neutral-200"
          : "bg-neutral-100 text-neutral-500 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700"
      } ${disabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"}`}
    >
      {checked ? <IconPauseBars size={icon} /> : <IconPlayTri size={icon} />}
    </button>
  );
}
