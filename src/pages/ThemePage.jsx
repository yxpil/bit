import { useRef } from "react";
import { useTheme, DEFAULT_LOOK } from "../useTheme.js";
import { useLang } from "../i18n.js";
import PillSwitch from "../components/PillSwitch.jsx";
import { IconImage, IconTrash, IconCheck } from "../components/Icons.jsx";

/**
 * ThemePage —— 主题定制页
 *  - 背景色（亮色/暗色各自一套）
 *  - 背景透明度滑块
 *  - 背景图片上传（本地文件 → data URL）
 *  - 边框 / 阴影开关
 *  - 卡片圆角滑块
 *  - 一键恢复默认
 */
export default function ThemePage() {
  const { isDark, look, setLook } = useTheme();
  const { t } = useLang();
  const fileRef = useRef(null);

  // 浅拷贝避免直接修改原对象
  const patch = (partial) => setLook((l) => ({ ...l, ...partial, enabled: true }));

  const reset = () => setLook({ ...DEFAULT_LOOK });

  const onPickFile = async (e) => {
    const f = e.target.files?.[0];
    if (!f) return;
    // 限制 5MB
    if (f.size > 5 * 1024 * 1024) {
      alert("图片不能超过 5MB");
      return;
    }
    const reader = new FileReader();
    reader.onload = () => patch({ bgImage: reader.result });
    reader.readAsDataURL(f);
    e.target.value = "";
  };

  const PRESET_COLORS = [
    "#ffffff", "#fafafa", "#f5f5f5", "#faf5f0", "#f0f7ff", "#fffbeb",
    "#0a0a0a", "#171717", "#1c1917", "#0f172a", "#1e1b4b", "#18181b",
  ];

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <h2 className="text-xl font-semibold">{t("theme.title", "外观主题")}</h2>
      <p className="text-sm text-neutral-500 dark:text-neutral-400">
        {t("theme.desc", "自定义背景、透明度、圆角和边框。所有改动即时生效并自动保存。")}
      </p>

      {/* 开关总控：启用外观定制 */}
      <div className="card flex items-center justify-between">
        <div>
          <div className="font-medium">{t("theme.enable", "启用外观定制")}</div>
          <div className="text-xs text-neutral-500 dark:text-neutral-400">
            {t("theme.enableHint", "关闭后恢复默认黑白胶囊风格")}
          </div>
        </div>
        <PillSwitch checked={look.enabled} onChange={(v) => setLook((l) => ({ ...l, enabled: v }))} />
      </div>

      <div className={`space-y-5 ${!look.enabled ? "opacity-40 pointer-events-none" : ""}`}>
        {/* ==== 背景色 ==== */}
        <div className="card space-y-4">
          <div className="font-medium">{t("theme.bgColor", "背景颜色")}</div>

          {/* 亮色背景 */}
          <div>
            <label className="mb-2 flex items-center justify-between text-xs text-neutral-500 dark:text-neutral-400">
              <span>{t("theme.bgLight", "亮色背景")}</span>
              <span className="font-mono">{look.bgColorLight}</span>
            </label>
            <ColorInput
              value={look.bgColorLight}
              onChange={(v) => patch({ bgColorLight: v })}
              presets={PRESET_COLORS.slice(0, 6)}
            />
          </div>

          {/* 暗色背景 */}
          <div>
            <label className="mb-2 flex items-center justify-between text-xs text-neutral-500 dark:text-neutral-400">
              <span>{t("theme.bgDark", "暗色背景")}</span>
              <span className="font-mono">{look.bgColorDark}</span>
            </label>
            <ColorInput
              value={look.bgColorDark}
              onChange={(v) => patch({ bgColorDark: v })}
              presets={PRESET_COLORS.slice(6)}
            />
          </div>
        </div>

        {/* ==== 背景透明度 ==== */}
        <div className="card">
          <label className="mb-3 flex items-center justify-between">
            <span className="font-medium">{t("theme.bgOpacity", "背景透明度")}</span>
            <span className="font-mono text-sm text-neutral-500 dark:text-neutral-400">
              {look.bgOpacity}%
            </span>
          </label>
          <input
            type="range"
            min="10"
            max="100"
            step="5"
            value={look.bgOpacity}
            onChange={(e) => patch({ bgOpacity: Number(e.target.value) })}
            className="w-full accent-[var(--accent)]"
          />
          <div className="mt-1 flex justify-between text-[11px] text-neutral-400">
            <span>{t("theme.bgTransparent", "更透明")}</span>
            <span>{t("theme.bgOpaque", "完全不透明")}</span>
          </div>
        </div>

        {/* ==== 背景图片 ==== */}
        <div className="card">
          <div className="mb-3 flex items-center justify-between">
            <div>
              <div className="font-medium">{t("theme.bgImage", "背景图片")}</div>
              <div className="text-xs text-neutral-500 dark:text-neutral-400">
                {t("theme.bgImageHint", "支持 JPG / PNG / WEBP，≤5MB")}
              </div>
            </div>
            {look.bgImage && (
              <button
                onClick={() => patch({ bgImage: "" })}
                className="icon-btn"
                title={t("theme.removeImage", "移除图片")}
              >
                <IconTrash size={14} />
              </button>
            )}
          </div>

          {look.bgImage ? (
            <div className="relative h-40 w-full overflow-hidden rounded-xl border border-neutral-200 dark:border-neutral-800">
              <img src={look.bgImage} alt="" className="h-full w-full object-cover" />
              <div className="absolute bottom-2 right-2 rounded-full bg-black/50 px-2 py-0.5 text-[11px] text-white">
                {t("theme.imageApplied", "已应用")}
              </div>
            </div>
          ) : (
            <button
              onClick={() => fileRef.current?.click()}
              className="flex h-32 w-full items-center justify-center gap-2 rounded-xl border-2 border-dashed border-neutral-300 text-neutral-500 transition-colors hover:border-neutral-500 hover:text-neutral-700 dark:border-neutral-700 dark:hover:border-neutral-400 dark:hover:text-neutral-200"
            >
              <IconImage size={20} />
              <span>{t("theme.uploadImage", "点击上传背景图片")}</span>
            </button>
          )}
          <input
            ref={fileRef}
            type="file"
            accept="image/*"
            onChange={onPickFile}
            className="hidden"
          />
        </div>

        {/* ==== 外观细节开关 ==== */}
        <div className="card space-y-4">
          <div className="font-medium">{t("theme.details", "外观细节")}</div>

          <div className="flex items-center justify-between">
            <div>
              <div className="text-sm">{t("theme.border", "显示边框")}</div>
              <div className="text-xs text-neutral-500 dark:text-neutral-400">
                {t("theme.borderHint", "去除卡片 / 按钮的描边")}
              </div>
            </div>
            <PillSwitch
              checked={look.borderOn}
              onChange={(v) => patch({ borderOn: v })}
            />
          </div>

          <div className="flex items-center justify-between">
            <div>
              <div className="text-sm">{t("theme.shadow", "显示阴影")}</div>
              <div className="text-xs text-neutral-500 dark:text-neutral-400">
                {t("theme.shadowHint", "去除卡片投影")}
              </div>
            </div>
            <PillSwitch
              checked={look.shadowOn}
              onChange={(v) => patch({ shadowOn: v })}
            />
          </div>

          <div>
            <label className="mb-2 flex items-center justify-between text-sm">
              <span>{t("theme.cardRadius", "卡片圆角")}</span>
              <span className="font-mono text-neutral-500 dark:text-neutral-400">
                {look.cardRadius}px
              </span>
            </label>
            <input
              type="range"
              min="0"
              max="48"
              step="4"
              value={look.cardRadius}
              onChange={(e) => patch({ cardRadius: Number(e.target.value) })}
              className="w-full accent-[var(--accent)]"
            />
            <div className="mt-1 flex justify-between text-[11px] text-neutral-400">
              <span>{t("theme.square", "直角")}</span>
              <span>{t("theme.rounded", "圆润")}</span>
            </div>
          </div>
        </div>
      </div>

      {/* 恢复默认 */}
      <div className="flex justify-end">
        <button
          onClick={reset}
          className="pill"
          title={t("theme.reset", "恢复默认外观")}
        >
          <IconCheck size={12} />
          {t("theme.reset", "恢复默认")}
        </button>
      </div>
    </div>
  );
}

/** 颜色输入：preset 色点 + 原生 color picker */
function ColorInput({ value, onChange, presets }) {
  return (
    <div className="flex items-center gap-3">
      <input
        type="color"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="h-9 w-9 cursor-pointer rounded-lg border border-neutral-200 bg-transparent p-0.5 dark:border-neutral-700"
      />
      <div className="flex flex-wrap gap-1.5">
        {presets.map((c) => (
          <button
            key={c}
            onClick={() => onChange(c)}
            className={`h-6 w-6 rounded-md border transition-transform hover:scale-110 ${
              value.toLowerCase() === c.toLowerCase()
                ? "border-[var(--accent)] ring-2 ring-[var(--accent)]/30"
                : "border-neutral-200 dark:border-neutral-700"
            }`}
            style={{ backgroundColor: c }}
            title={c}
          />
        ))}
      </div>
    </div>
  );
}
