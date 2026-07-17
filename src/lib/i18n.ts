import de from "../locales/de.json";
import en from "../locales/en.json";
import type { UiLanguage } from "./types";

type Catalog = typeof en;
type Join<Prefix extends string, Key extends string> = Prefix extends ""
  ? Key
  : `${Prefix}.${Key}`;
type NestedKey<T, Prefix extends string = ""> = {
  [Key in keyof T & string]: T[Key] extends string
    ? Join<Prefix, Key>
    : T[Key] extends Record<string, unknown>
      ? NestedKey<T[Key], Join<Prefix, Key>>
      : never;
}[keyof T & string];

export type TranslationKey = NestedKey<Catalog>;
export type TranslationValues = Record<string, string | number>;

const catalogs: Record<UiLanguage, Catalog> = {
  en,
  de,
};

let currentLanguage: UiLanguage = "en";

function lookup(catalog: unknown, key: string): string | undefined {
  let current: unknown = catalog;
  for (const segment of key.split(".")) {
    if (
      typeof current !== "object" ||
      current === null ||
      !(segment in current)
    ) {
      return undefined;
    }
    current = (current as Record<string, unknown>)[segment];
  }
  return typeof current === "string" ? current : undefined;
}

function resolveTranslation(
  primaryCatalog: unknown,
  fallbackCatalog: unknown,
  key: string
): string {
  return lookup(primaryCatalog, key) ?? lookup(fallbackCatalog, key) ?? key;
}

function interpolate(template: string, values: TranslationValues): string {
  return template.replace(/\{([A-Za-z][A-Za-z0-9]*)\}/g, (match, name) => {
    const value = values[name];
    return value === undefined ? match : String(value);
  });
}

export function setUiLanguage(language: UiLanguage): void {
  currentLanguage = language === "de" ? "de" : "en";
  if (typeof document !== "undefined") {
    document.documentElement.lang = currentLanguage;
  }
}

export function getUiLanguage(): UiLanguage {
  return currentLanguage;
}

export function t(key: TranslationKey, values: TranslationValues = {}): string {
  const template = resolveTranslation(
    catalogs[currentLanguage],
    catalogs.en,
    key
  );
  return interpolate(template, values);
}

export function resolveTranslationForTest(
  primaryCatalog: unknown,
  fallbackCatalog: unknown,
  key: string
): string {
  return resolveTranslation(primaryCatalog, fallbackCatalog, key);
}

export function tp(
  oneKey: TranslationKey,
  otherKey: TranslationKey,
  count: number,
  values: TranslationValues = {}
): string {
  const category = new Intl.PluralRules(currentLanguage).select(count);
  return t(category === "one" ? oneKey : otherKey, { count, ...values });
}

export function formatNumber(value: number): string {
  return new Intl.NumberFormat(currentLanguage).format(value);
}

export function formatDateTime(
  value: Date,
  options: Intl.DateTimeFormatOptions
): string {
  return new Intl.DateTimeFormat(currentLanguage, options).format(value);
}

export function getCatalogKeys(catalog: unknown, prefix = ""): string[] {
  if (typeof catalog !== "object" || catalog === null) {
    return [];
  }

  const keys: string[] = [];
  for (const [key, value] of Object.entries(catalog)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") {
      keys.push(path);
    } else {
      keys.push(...getCatalogKeys(value, path));
    }
  }
  return keys.sort();
}

export function getCatalogsForTest(): {
  de: unknown;
  en: unknown;
} {
  return { de, en };
}
