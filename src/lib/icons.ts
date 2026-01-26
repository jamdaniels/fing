import type { IconNode } from "lucide";

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
  const [tag, attrs, children] = iconData as [
    string,
    Record<string, string>,
    [string, Record<string, string>][]?,
  ];
  const attrStr = Object.entries(attrs)
    .map(([k, v]) => `${k}="${v}"`)
    .join(" ");
  const childrenStr = (children || [])
    .map((child) => {
      const [cTag, cAttrs] = child;
      const cAttrStr = Object.entries(cAttrs)
        .map(([k, v]) => `${k}="${v}"`)
        .join(" ");
      return `<${cTag} ${cAttrStr}/>`;
    })
    .join("");
  return `<${tag} ${attrStr}>${childrenStr}</${tag}>`;
}
