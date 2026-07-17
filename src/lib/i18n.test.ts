import { describe, expect, it } from "bun:test";
import {
  formatNumber,
  getCatalogKeys,
  getCatalogsForTest,
  resolveTranslationForTest,
  setUiLanguage,
  t,
  tp,
} from "./i18n";

function flattenStrings(
  value: unknown,
  prefix = "",
  output = new Map<string, string>()
): Map<string, string> {
  if (typeof value !== "object" || value === null) {
    return output;
  }
  for (const [key, child] of Object.entries(value)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof child === "string") {
      output.set(path, child);
    } else {
      flattenStrings(child, path, output);
    }
  }
  return output;
}

function placeholders(value: string): string[] {
  return [...value.matchAll(/\{([A-Za-z][A-Za-z0-9]*)\}/g)]
    .map((match) => match[1])
    .sort();
}

describe("i18n catalogs", () => {
  it("English and German have identical keys and placeholders", () => {
    const catalogs = getCatalogsForTest();
    expect(getCatalogKeys(catalogs.de)).toEqual(getCatalogKeys(catalogs.en));

    const en = flattenStrings(catalogs.en);
    const de = flattenStrings(catalogs.de);
    for (const [key, value] of en) {
      expect(placeholders(de.get(key) ?? "")).toEqual(placeholders(value));
    }
  });

  it("interpolates and pluralizes in both languages", () => {
    setUiLanguage("en");
    expect(t("dictionary.full", { count: 100 })).toBe(
      "Dictionary is full (100 terms)"
    );
    expect(tp("common.wordOne", "common.wordOther", 1)).toBe("1 word");
    expect(tp("common.wordOne", "common.wordOther", 2)).toBe("2 words");

    setUiLanguage("de");
    expect(tp("common.wordOne", "common.wordOther", 1)).toBe("1 Wort");
    expect(tp("common.wordOne", "common.wordOther", 2)).toBe("2 Wörter");
  });

  it("falls back to English when a selected catalog key is missing", () => {
    expect(
      resolveTranslationForTest(
        {},
        { settings: { title: "Settings" } },
        "settings.title"
      )
    ).toBe("Settings");
  });

  it("formats numbers with the selected locale", () => {
    setUiLanguage("en");
    expect(/1[,.]234/.test(formatNumber(1234))).toBe(true);
    setUiLanguage("de");
    expect(/1\.234|1 234/.test(formatNumber(1234))).toBe(true);
  });
});
