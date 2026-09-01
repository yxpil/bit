// 纯 SVG 图标库（黑白线性风格，无 emoji）
const I = ({ children, size = 18, ...rest }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.8"
    strokeLinecap="round"
    strokeLinejoin="round"
    {...rest}
  >
    {children}
  </svg>
);

export const IconPlay = (p) => (
  <I {...p}>
    <circle cx="12" cy="12" r="9" />
    <polygon points="10,8 16,12 10,16" fill="currentColor" stroke="none" />
  </I>
);

export const IconPause = (p) => (
  <I {...p}>
    <circle cx="12" cy="12" r="9" />
    <line x1="10" y1="9" x2="10" y2="15" />
    <line x1="14" y1="9" x2="14" y2="15" />
  </I>
);

// 裸图标（无外圈），用于放进实心圆形按钮内，复刻 PLocalSwitch 的启用/暂停圆钮
export const IconPauseBars = (p) => (
  <I {...p}>
    <line x1="9" y1="7" x2="9" y2="17" />
    <line x1="15" y1="7" x2="15" y2="17" />
  </I>
);
export const IconPlayTri = (p) => (
  <I {...p}>
    <polygon points="8,6 18,12 8,18" fill="currentColor" stroke="none" />
  </I>
);

export const IconChat = (p) => (
  <I {...p}>
    <path d="M21 12a8 8 0 0 1-8 8H4l2.3-2.9A8 8 0 1 1 21 12z" />
    <circle cx="9" cy="12" r="0.5" fill="currentColor" />
    <circle cx="12.5" cy="12" r="0.5" fill="currentColor" />
    <circle cx="16" cy="12" r="0.5" fill="currentColor" />
  </I>
);

export const IconTool = (p) => (
  <I {...p}>
    <path d="M14.7 6.3a4.5 4.5 0 0 0-6 5.6L3 17.6V21h3.4l5.7-5.7a4.5 4.5 0 0 0 5.6-6L14.5 12l-2.5-2.5 2.7-3.2z" />
  </I>
);

export const IconMemory = (p) => (
  <I {...p}>
    <rect x="5" y="5" width="14" height="14" rx="3" />
    <circle cx="9.5" cy="10" r="0.5" fill="currentColor" />
    <circle cx="14.5" cy="10" r="0.5" fill="currentColor" />
    <circle cx="9.5" cy="14" r="0.5" fill="currentColor" />
    <circle cx="14.5" cy="14" r="0.5" fill="currentColor" />
  </I>
);

export const IconSkill = (p) => (
  <I {...p}>
    <polygon points="12,3 14.5,8.5 20,9.3 16,13.2 17,19 12,16 7,19 8,13.2 4,9.3 9.5,8.5" />
  </I>
);

export const IconAudit = (p) => (
  <I {...p}>
    <path d="M6 3h12v18l-6-3-6 3z" />
    <line x1="9" y1="8" x2="15" y2="8" />
    <line x1="9" y1="12" x2="15" y2="12" />
  </I>
);

export const IconGlobe = (p) => (
  <I {...p}>
    <circle cx="12" cy="12" r="9" />
    <path d="M3 12h18M12 3c2.5 2.5 3.5 5.7 3.5 9s-1 6.5-3.5 9c-2.5-2.5-3.5-5.7-3.5-9s1-6.5 3.5-9z" />
  </I>
);

export const IconSettings = (p) => (
  <I {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19 12a7 7 0 0 0-.1-1.2l2-1.6-2-3.4-2.4 1a7 7 0 0 0-2-1.2L14 3h-4l-.4 2.6a7 7 0 0 0-2 1.2l-2.5-1-2 3.4 2 1.6A7 7 0 0 0 5 12c0 .4 0 .8.1 1.2l-2 1.6 2 3.4 2.4-1a7 7 0 0 0 2 1.2L10 21h4l.4-2.6a7 7 0 0 0 2-1.2l2.5 1 2-3.4-2-1.6c.1-.4.1-.8.1-1.2z" />
  </I>
);

export const IconPlus = (p) => (
  <I {...p}>
    <line x1="12" y1="5" x2="12" y2="19" />
    <line x1="5" y1="12" x2="19" y2="12" />
  </I>
);

export const IconTrash = (p) => (
  <I {...p}>
    <path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" />
  </I>
);

export const IconSend = (p) => (
  <I {...p}>
    <path d="M22 2 11 13M22 2 15 22l-4-9-9-4 20-7z" />
  </I>
);

export const IconBolt = (p) => (
  <I {...p}>
    <polygon points="13,2 4,14 11,14 10,22 20,9 13,9" />
  </I>
);

export const IconRefresh = (p) => (
  <I {...p}>
    <path d="M21 12a9 9 0 1 1-2.6-6.3M21 3v6h-6" />
  </I>
);

export const IconStop = (p) => (
  <I {...p}>
    <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" stroke="none" />
  </I>
);

export const IconEye = (p) => (
  <I {...p}>
    <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z" />
    <circle cx="12" cy="12" r="3" />
  </I>
);

export const IconShield = (p) => (
  <I {...p}>
    <path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6l8-3z" />
  </I>
);

export const IconQueue = (p) => (
  <I {...p}>
    <line x1="4" y1="6" x2="20" y2="6" />
    <line x1="4" y1="12" x2="20" y2="12" />
    <line x1="4" y1="18" x2="14" y2="18" />
    <path d="M18 15l3 3-3 3" />
  </I>
);

export const IconTarget = (p) => (
  <I {...p}>
    <circle cx="12" cy="12" r="9" />
    <circle cx="12" cy="12" r="5" />
    <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" />
  </I>
);

export const IconCheck = (p) => (
  <I {...p}>
    <path d="M4 12.5 9.5 18 20 6" />
  </I>
);

export const IconX = (p) => (
  <I {...p}>
    <line x1="5" y1="5" x2="19" y2="19" />
    <line x1="19" y1="5" x2="5" y2="19" />
  </I>
);

/* ---- 标题栏 / 主题 / 解释器相关 ---- */

export const IconMinus = (p) => (
  <I {...p}>
    <line x1="5" y1="12" x2="19" y2="12" />
  </I>
);

export const IconSquare = (p) => (
  <I {...p}>
    <rect x="5" y="5" width="14" height="14" rx="2" />
  </I>
);

export const IconSun = (p) => (
  <I {...p}>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
  </I>
);

export const IconMoon = (p) => (
  <I {...p}>
    <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
  </I>
);

export const IconShirt = (p) => (
  <I {...p}>
    <path d="M20.38 3.46 16 2a4 4 0 0 1-8 0L3.62 3.46a2 2 0 0 0-1.34 2.23l.58 3.47a1 1 0 0 0 .99.84H6v10c0 1.1.9 2 2 2h8a2 2 0 0 0 2-2V10h2.15a1 1 0 0 0 .99-.84l.58-3.47a2 2 0 0 0-1.34-2.23z" />
  </I>
);

export const IconServer = (p) => (
  <I {...p}>
    <rect x="3" y="4" width="18" height="7" rx="2" />
    <rect x="3" y="13" width="18" height="7" rx="2" />
    <line x1="7" y1="7.5" x2="7.01" y2="7.5" />
    <line x1="7" y1="16.5" x2="7.01" y2="16.5" />
  </I>
);

export const IconTerminal = (p) => (
  <I {...p}>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M7 9l3 3-3 3M13 15h4" />
  </I>
);

export const IconCode = (p) => (
  <I {...p}>
    <path d="M8 6l-5 6 5 6M16 6l5 6-5 6" />
  </I>
);

export const IconInfo = (p) => (
  <I {...p}>
    <circle cx="12" cy="12" r="9" />
    <line x1="12" y1="11" x2="12" y2="16" />
    <circle cx="12" cy="7.5" r="0.6" fill="currentColor" stroke="none" />
  </I>
);

export const IconEdit = (p) => (
  <I {...p}>
    <path d="M12 20h9" />
    <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" />
  </I>
);

export const IconChevronDown = (p) => (
  <I {...p}>
    <path d="M6 9l6 6 6-6" />
  </I>
);

export const IconChevronRight = (p) => (
  <I {...p}>
    <path d="M9 6l6 6-6 6" />
  </I>
);

/* ---- 附件：图片 / 文件 / 链接 ---- */

export const IconImage = (p) => (
  <I {...p}>
    <rect x="3" y="3" width="18" height="18" rx="2" />
    <circle cx="8.5" cy="8.5" r="1.5" />
    <path d="M21 15l-5-5L5 21" />
  </I>
);

export const IconFile = (p) => (
  <I {...p}>
    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
    <path d="M14 2v6h6" />
  </I>
);

export const IconLink = (p) => (
  <I {...p}>
    <path d="M10 13a5 5 0 0 0 7.07 0l2-2a5 5 0 0 0-7.07-7.07l-1.5 1.5" />
    <path d="M14 11a5 5 0 0 0-7.07 0l-2 2a5 5 0 0 0 7.07 7.07l1.5-1.5" />
  </I>
);
