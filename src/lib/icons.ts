import type { IconNode } from "lucide";

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
