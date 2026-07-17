import { describe, expect, it } from "bun:test";

import { WHISPER_LANGUAGES } from "./languages";

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
