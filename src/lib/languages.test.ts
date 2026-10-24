import { describe, expect, it } from "bun:test";

import {
  languagesOutsideParakeet,
  PARAKEET_LANGUAGE_CODES,
  WHISPER_LANGUAGES,
} from "./languages";

describe("Whisper language catalog", () => {
  it("contains the 99 languages supported by every bundled model", () => {
    expect(WHISPER_LANGUAGES.length).toBe(99);
    expect(WHISPER_LANGUAGES.some(({ code }) => code === "yue")).toBe(false);
    expect(WHISPER_LANGUAGES.some(({ name }) => name === "Cantonese")).toBe(
      false
    );
  });

  it("uses unique language codes", () => {
    const codes = WHISPER_LANGUAGES.map(({ code }) => code);
    expect(new Set(codes).size).toBe(codes.length);
  });
});

describe("Parakeet languages", () => {
  it("are 25 languages from the Whisper catalog", () => {
    expect(PARAKEET_LANGUAGE_CODES.size).toBe(25);
    const whisperCodes = new Set(WHISPER_LANGUAGES.map(({ code }) => code));
    for (const code of PARAKEET_LANGUAGE_CODES) {
      expect(whisperCodes.has(code)).toBe(true);
    }
  });

  it("lists the selected languages Parakeet can't transcribe", () => {
    expect(languagesOutsideParakeet(["de", "en"])).toEqual([]);
    expect(languagesOutsideParakeet(["en", "ja", "tr"])).toEqual(["ja", "tr"]);
  });
});
