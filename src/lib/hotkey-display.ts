import {
  ArrowBigUp,
  ArrowBigUpDash,
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowRightToLine,
  ArrowUp,
  ChevronUp,
  Command,
  CornerDownLeft,
  Delete,
  type IconNode,
  Option,
  Space,
} from "lucide";
import { formatHotkeyForDisplay, parseHotkeyString } from "./hotkey";
import { createIcon } from "./icons";

const TOKEN_ICONS: Record<string, IconNode> = {
  ControlLeft: ChevronUp,
  ControlRight: ChevronUp,
  MetaLeft: Command,
  MetaRight: Command,
  Alt: Option,
  AltGr: Option,
  ShiftLeft: ArrowBigUp,
  ShiftRight: ArrowBigUp,
  CapsLock: ArrowBigUpDash,
  Tab: ArrowRightToLine,
  Return: CornerDownLeft,
  Backspace: Delete,
  Space,
  UpArrow: ArrowUp,
  DownArrow: ArrowDown,
  LeftArrow: ArrowLeft,
  RightArrow: ArrowRight,
};

const LEFT_SIDE_TOKENS = new Set([
  "ControlLeft",
  "MetaLeft",
  "Alt",
  "ShiftLeft",
]);
const RIGHT_SIDE_TOKENS = new Set([
  "ControlRight",
  "MetaRight",
  "AltGr",
  "ShiftRight",
]);

const WIDE_TOKENS_2U = new Set(["Backspace"]);
const WIDE_TOKENS_175U = new Set(["ShiftLeft", "ShiftRight"]);
const WIDE_TOKENS_15U = new Set([
  "ControlLeft",
  "ControlRight",
  "MetaLeft",
  "MetaRight",
  "Alt",
  "AltGr",
  "CapsLock",
  "Tab",
  "Return",
]);
const WIDE_TOKENS_25U = new Set(["Space"]);

function widthClass(token: string): string {
  if (WIDE_TOKENS_2U.has(token)) {
    return "is-w-2";
  }
  if (WIDE_TOKENS_175U.has(token)) {
    return "is-w-175";
  }
  if (WIDE_TOKENS_15U.has(token)) {
    return "is-w-15";
  }
  if (WIDE_TOKENS_25U.has(token)) {
    return "is-w-25";
  }
  return "";
}

function sideClass(token: string): string {
  if (LEFT_SIDE_TOKENS.has(token)) {
    return "is-side-left";
  }
  if (RIGHT_SIDE_TOKENS.has(token)) {
    return "is-side-right";
  }
  return "";
}

function chipContent(token: string): string {
  const icon = TOKEN_ICONS[token];
  if (icon) {
    return createIcon(icon);
  }
  return formatHotkeyForDisplay(token);
}

export interface RenderHotkeyChipsOptions {
  chipClass:
    | "hotkey-key"
    | "hotkey-modal-key"
    | "instruction-key"
    | "hotkey-key-inline";
  extraChipClass?: string;
  separator?: "plus";
}

export function renderHotkeyChips(
  hotkey: string,
  options: RenderHotkeyChipsOptions
): string {
  const parsed = parseHotkeyString(hotkey);
  const tokens = parsed?.keys ?? [hotkey];
  const extra = options.extraChipClass ? ` ${options.extraChipClass}` : "";
  const chips = tokens.map((token) => {
    const classes = [options.chipClass, widthClass(token), sideClass(token)]
      .filter(Boolean)
      .join(" ");
    return `<span class="${classes}${extra}">${chipContent(token)}</span>`;
  });
  const joined =
    options.separator === "plus"
      ? chips.join('<span class="hotkey-chip-sep">+</span>')
      : chips.join("");
  return `<span class="hotkey-chips">${joined}</span>`;
}
