import type { IconNode } from "lucide";

const DEFAULT_ICON_ATTRIBUTES: Record<string, string> = {
  xmlns: "http://www.w3.org/2000/svg",
  width: "24",
  height: "24",
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  "stroke-width": "2",
  "stroke-linecap": "round",
  "stroke-linejoin": "round",
};

function renderAttributes(attrs: Record<string, string | number | undefined>): string {
  return Object.entries(attrs)
    .filter(([, value]) => value !== undefined)
    .map(([key, value]) => `${key}="${String(value)}"`)
    .join(" ");
}

/**
 * Escape HTML special characters to prevent XSS
 * Use this for any user or backend data interpolated into innerHTML
 */
export function escapeHtml(unsafe: string): string {
  return unsafe
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

export function createIcon(iconData: IconNode): string {
  const childrenStr = iconData
    .map(([tag, attrs]) => `<${tag} ${renderAttributes(attrs)}/>`)
    .join("");
  return `<svg ${renderAttributes(DEFAULT_ICON_ATTRIBUTES)}>${childrenStr}</svg>`;
}
