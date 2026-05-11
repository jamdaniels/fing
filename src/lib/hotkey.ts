const DEFAULT_HOTKEY = "F9";
const FUNCTION_KEY_REGEX = /^F(?:[1-9]|1\d|2[0-4])$/;
const KEY_TOKEN_REGEX = /^Key[A-Z]$/;
const DIGIT_TOKEN_REGEX = /^Digit\d$/;
const NUMPAD_TOKEN_REGEX = /^Numpad\d$/;

const EVENT_CODE_ALIASES: Record<string, string> = {
  AltLeft: "Alt",
  AltRight: "AltGr",
  ArrowDown: "DownArrow",
  ArrowLeft: "LeftArrow",
  ArrowRight: "RightArrow",
  ArrowUp: "UpArrow",
  AudioVolumeDown: "VolumeDown",
  AudioVolumeMute: "VolumeMute",
  AudioVolumeUp: "VolumeUp",
  Backquote: "BackQuote",
  BracketLeft: "LeftBracket",
  BracketRight: "RightBracket",
  ContextMenu: "Apps",
  Digit0: "Num0",
  Digit1: "Num1",
  Digit2: "Num2",
  Digit3: "Num3",
  Digit4: "Num4",
  Digit5: "Num5",
  Digit6: "Num6",
  Digit7: "Num7",
  Digit8: "Num8",
  Digit9: "Num9",
  Enter: "Return",
  Fn: "Function",
  IntlBackslash: "IntlBackslash",
  IntlRo: "IntlRo",
  IntlYen: "IntlYen",
  KanaMode: "KanaMode",
  Numpad0: "Kp0",
  Numpad1: "Kp1",
  Numpad2: "Kp2",
  Numpad3: "Kp3",
  Numpad4: "Kp4",
  Numpad5: "Kp5",
  Numpad6: "Kp6",
  Numpad7: "Kp7",
  Numpad8: "Kp8",
  Numpad9: "Kp9",
  NumpadAdd: "KpPlus",
  NumpadComma: "KpComma",
  NumpadDecimal: "KpDecimal",
  NumpadDivide: "KpDivide",
  NumpadEnter: "KpReturn",
  NumpadEqual: "KpEqual",
  NumpadMultiply: "KpMultiply",
  NumpadSubtract: "KpMinus",
  Period: "Dot",
  Semicolon: "SemiColon",
};

const BASE_TOKENS = [
  "Alt",
  "AltGr",
  "Apps",
  "Backslash",
  "Backspace",
  "BackQuote",
  "Cancel",
  "CapsLock",
  "Clear",
  "Comma",
  "ControlLeft",
  "ControlRight",
  "Delete",
  "Dot",
  "DownArrow",
  "End",
  "Equal",
  "Execute",
  "Final",
  "Function",
  "Hangul",
  "Hanja",
  "Hanji",
  "Help",
  "Home",
  "Insert",
  "IntlBackslash",
  "IntlRo",
  "IntlYen",
  "Junja",
  "Kana",
  "KanaMode",
  "KpComma",
  "KpDecimal",
  "KpDivide",
  "KpEqual",
  "KpMinus",
  "KpMultiply",
  "KpPlus",
  "KpReturn",
  "Lang1",
  "Lang2",
  "Lang3",
  "Lang4",
  "Lang5",
  "LeftArrow",
  "LeftBracket",
  "MetaLeft",
  "MetaRight",
  "Minus",
  "NumLock",
  "PageDown",
  "PageUp",
  "Pause",
  "Print",
  "PrintScreen",
  "Quote",
  "Return",
  "RightArrow",
  "RightBracket",
  "ScrollLock",
  "Select",
  "Separator",
  "SemiColon",
  "ShiftLeft",
  "ShiftRight",
  "Slash",
  "Sleep",
  "Space",
  "Tab",
  "UpArrow",
  "VolumeDown",
  "VolumeMute",
  "VolumeUp",
];

const VALID_HOTKEY_TOKENS = new Set([
  ...BASE_TOKENS,
  ...Array.from({ length: 24 }, (_, index) => `F${index + 1}`),
  ...Array.from(
    { length: 26 },
    (_, index) => `Key${String.fromCharCode(65 + index)}`
  ),
  ...Array.from({ length: 10 }, (_, index) => `Num${index}`),
  ...Array.from({ length: 10 }, (_, index) => `Kp${index}`),
]);

const TOKEN_DISPLAY_LABELS: Record<string, string> = {
  Alt: "Option",
  AltGr: "Option Right",
  Apps: "Menu",
  BackQuote: "`",
  Backslash: "\\",
  CapsLock: "Caps Lock",
  Comma: ",",
  ControlLeft: "Ctrl Left",
  ControlRight: "Ctrl Right",
  Delete: "Delete",
  Dot: ".",
  DownArrow: "Down",
  Equal: "=",
  Function: "fn",
  KpComma: "Numpad ,",
  KpDecimal: "Numpad .",
  KpDivide: "Numpad /",
  KpEqual: "Numpad =",
  KpMinus: "Numpad -",
  KpMultiply: "Numpad *",
  KpPlus: "Numpad +",
  KpReturn: "Numpad Enter",
  LeftArrow: "Left",
  LeftBracket: "[",
  MetaLeft: "Cmd Left",
  MetaRight: "Cmd Right",
  Minus: "-",
  NumLock: "Num Lock",
  PageDown: "Page Down",
  PageUp: "Page Up",
  PrintScreen: "Print Screen",
  Quote: "'",
  Return: "Enter",
  RightArrow: "Right",
  RightBracket: "]",
  ScrollLock: "Scroll Lock",
  SemiColon: ";",
  ShiftLeft: "Shift Left",
  ShiftRight: "Shift Right",
  Slash: "/",
  UpArrow: "Up",
  VolumeDown: "Volume Down",
  VolumeMute: "Volume Mute",
  VolumeUp: "Volume Up",
};

export interface ParsedHotkeyConfig {
  keySet: ReadonlySet<string>;
  keys: string[];
}

function normalizeEventCode(code: string): string | null {
  if (!code || code === "Escape") {
    return null;
  }
  if (EVENT_CODE_ALIASES[code]) {
    return EVENT_CODE_ALIASES[code];
  }
  if (FUNCTION_KEY_REGEX.test(code)) {
    return code;
  }
  if (KEY_TOKEN_REGEX.test(code)) {
    return code;
  }
  if (DIGIT_TOKEN_REGEX.test(code)) {
    return `Num${code.slice(-1)}`;
  }
  if (NUMPAD_TOKEN_REGEX.test(code)) {
    return `Kp${code.slice(-1)}`;
  }
  if (VALID_HOTKEY_TOKENS.has(code)) {
    return code;
  }

  return null;
}

function normalizeEventKey(key: string): string | null {
  if (!key || key === "Escape") {
    return null;
  }
  if (key === " ") {
    return "Space";
  }
  if (key === "Control") {
    return "ControlLeft";
  }
  if (key === "Alt") {
    return "Alt";
  }
  if (key === "Shift") {
    return "ShiftLeft";
  }
  if (key === "Meta") {
    return "MetaLeft";
  }
  if (key === "Fn" || key === "Function") {
    return "Function";
  }
  if (FUNCTION_KEY_REGEX.test(key)) {
    return key;
  }

  return null;
}

function sameHotkeySet(
  pressedTokens: ReadonlySet<string>,
  config: ParsedHotkeyConfig
): boolean {
  if (pressedTokens.size !== config.keySet.size) {
    return false;
  }

  for (const key of config.keySet) {
    if (!pressedTokens.has(key)) {
      return false;
    }
  }

  return true;
}

export function eventToHotkeyToken(e: KeyboardEvent): string | null {
  const fromCode = normalizeEventCode(e.code);
  if (fromCode) {
    return fromCode;
  }

  return normalizeEventKey(e.key);
}

export function formatHotkeyForDisplay(hotkey: string): string {
  return hotkey
    .split("+")
    .map((part) => {
      if (KEY_TOKEN_REGEX.test(part)) {
        return part.slice(3);
      }
      if (part.startsWith("Num") && part.length === 4) {
        return part.slice(3);
      }
      if (part.startsWith("Kp") && part.length === 3) {
        return `Numpad ${part.slice(2)}`;
      }

      return TOKEN_DISPLAY_LABELS[part] ?? part;
    })
    .join(" + ");
}

export function matchesHotkey(
  pressedTokens: ReadonlySet<string>,
  config: ParsedHotkeyConfig
): boolean {
  return sameHotkeySet(pressedTokens, config);
}

export function matchesHotkeyRelease(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  const token = eventToHotkeyToken(e);
  return token !== null && config.keySet.has(token);
}

export function normalizeStoredHotkey(hotkey?: string | null): string {
  if (hotkey) {
    const parsed = parseHotkeyString(hotkey);
    if (parsed) {
      return parsed.keys.join("+");
    }
  }

  return DEFAULT_HOTKEY;
}

export function parseHotkeyString(hotkey: string): ParsedHotkeyConfig | null {
  const trimmed = hotkey.trim();
  if (!trimmed) {
    return null;
  }

  const keys = trimmed.split("+");
  if (keys.length === 0) {
    return null;
  }

  const keySet = new Set<string>();
  for (const key of keys) {
    const token = key.trim();
    if (!VALID_HOTKEY_TOKENS.has(token) || keySet.has(token)) {
      return null;
    }
    keySet.add(token);
  }

  return { keys: Array.from(keySet), keySet };
}
