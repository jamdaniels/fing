const FUNCTION_KEY_REGEX = /^F\d+$/i;
const SINGLE_KEY_REGEX = /^[A-Z0-9]$/;
const MODIFIER_EVENT_KEYS = new Set(["Control", "Alt", "Shift", "Meta"]);
const SPACE_EVENT_KEYS = new Set([" ", "Space", "Spacebar"]);

type HotkeyModifier = "alt" | "ctrl" | "meta" | "shift";

interface ParsedHotkeyPart {
  key: string | null;
  modifier: HotkeyModifier | null;
}

export interface ParsedHotkeyConfig {
  alt: boolean;
  ctrl: boolean;
  key: string | null;
  meta: boolean;
  shift: boolean;
}

function getHotkeyModifiers(e: KeyboardEvent): string[] {
  const modifiers: string[] = [];

  if (e.ctrlKey) {
    modifiers.push("Ctrl");
  }
  if (e.altKey) {
    modifiers.push("Option");
  }
  if (e.shiftKey) {
    modifiers.push("Shift");
  }
  if (e.metaKey) {
    modifiers.push("Cmd");
  }

  return modifiers;
}

function normalizeHotkeyBase(key: string): string | null {
  if (FUNCTION_KEY_REGEX.test(key)) {
    return key.toUpperCase();
  }
  if (key.length === 1 && SINGLE_KEY_REGEX.test(key.toUpperCase())) {
    return key.toUpperCase();
  }

  return null;
}

function getParsedModifierCount(config: ParsedHotkeyConfig): number {
  return (
    Number(config.ctrl) +
    Number(config.alt) +
    Number(config.shift) +
    Number(config.meta)
  );
}

function matchesModifierFlags(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  return (
    e.ctrlKey === config.ctrl &&
    e.altKey === config.alt &&
    e.shiftKey === config.shift &&
    e.metaKey === config.meta
  );
}

function isConfiguredModifierKey(
  key: string,
  config: ParsedHotkeyConfig
): boolean {
  if (key === "Alt") {
    return config.alt;
  }
  if (key === "Control") {
    return config.ctrl;
  }
  if (key === "Meta") {
    return config.meta;
  }
  if (key === "Shift") {
    return config.shift;
  }

  return false;
}

function parseHotkeyPart(part: string): ParsedHotkeyPart | null {
  const trimmed = part.trim();

  if (!trimmed) {
    return null;
  }

  const lower = trimmed.toLowerCase();

  if (lower === "alt" || lower === "option") {
    return { key: null, modifier: "alt" };
  }
  if (lower === "ctrl" || lower === "control") {
    return { key: null, modifier: "ctrl" };
  }
  if (lower === "meta" || lower === "cmd" || lower === "command") {
    return { key: null, modifier: "meta" };
  }
  if (lower === "shift") {
    return { key: null, modifier: "shift" };
  }
  if (lower === "space") {
    return { key: "Space", modifier: null };
  }
  if (lower === "fn") {
    return { key: "Fn", modifier: null };
  }

  const key = normalizeHotkeyBase(trimmed);

  if (!key) {
    return null;
  }

  return { key, modifier: null };
}

function isValidParsedHotkey(
  config: ParsedHotkeyConfig,
  partCount: number
): boolean {
  const modifierCount = getParsedModifierCount(config);

  if (partCount === 1) {
    return config.key !== null && config.key !== "Space" && modifierCount === 0;
  }
  if (config.key === null) {
    return modifierCount === 2;
  }

  return config.key === "Space" && modifierCount === 1;
}

function matchesConfiguredKey(
  e: KeyboardEvent,
  configuredKey: string | null
): boolean {
  if (configuredKey === null) {
    return false;
  }
  if (configuredKey === "Space") {
    return isSpaceKeyEvent(e);
  }

  return e.key.toLowerCase() === configuredKey.toLowerCase();
}

export function isSpaceKeyEvent(e: KeyboardEvent): boolean {
  return e.code === "Space" || SPACE_EVENT_KEYS.has(e.key);
}

export function keyEventToHotkey(e: KeyboardEvent): string | null {
  if (e.key === "Escape") {
    return null;
  }

  const modifiers = getHotkeyModifiers(e);

  if (MODIFIER_EVENT_KEYS.has(e.key)) {
    return modifiers.length === 2 ? modifiers.join("+") : null;
  }
  if (isSpaceKeyEvent(e)) {
    return modifiers.length === 1 ? [...modifiers, "Space"].join("+") : null;
  }

  const key = normalizeHotkeyBase(e.key);

  if (!key || modifiers.length !== 0) {
    return null;
  }

  return key;
}

export function matchesHotkey(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  if (!matchesModifierFlags(e, config)) {
    return false;
  }
  if (config.key === null) {
    return isConfiguredModifierKey(e.key, config);
  }

  return matchesConfiguredKey(e, config.key);
}

export function matchesHotkeyRelease(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  if (!matchesModifierFlags(e, config)) {
    return true;
  }
  if (config.key === null) {
    return false;
  }

  return matchesConfiguredKey(e, config.key);
}

export function normalizeStoredHotkey(hotkey?: string | null): string {
  if (hotkey) {
    const parsed = parseHotkeyString(hotkey);

    if (parsed) {
      return parsed.key === "Fn" ? "Fn" : hotkey;
    }
  }

  return "F9";
}

export function parseHotkeyString(hotkey: string): ParsedHotkeyConfig | null {
  const parts = hotkey.split("+");

  if (parts.length === 0 || parts.length > 2) {
    return null;
  }

  const config: ParsedHotkeyConfig = {
    alt: false,
    ctrl: false,
    key: null,
    meta: false,
    shift: false,
  };

  for (const part of parts) {
    const parsedPart = parseHotkeyPart(part);

    if (!parsedPart) {
      return null;
    }
    if (parsedPart.modifier) {
      config[parsedPart.modifier] = true;
      continue;
    }
    if (config.key !== null) {
      return null;
    }

    config.key = parsedPart.key;
  }

  return isValidParsedHotkey(config, parts.length) ? config : null;
}

export function shouldReleaseOnKeydown(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  if (!matchesModifierFlags(e, config)) {
    return true;
  }
  if (config.key === null) {
    return !isConfiguredModifierKey(e.key, config);
  }

  return false;
}
