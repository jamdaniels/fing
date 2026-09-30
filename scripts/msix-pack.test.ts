import { describe, expect, test } from "bun:test";
import {
  bundleResourceTargets,
  renderManifest,
  toMsixVersion,
} from "./msix-pack";

describe("toMsixVersion", () => {
  test("appends the Store-reserved fourth part", () => {
    expect(toMsixVersion("1.2.3")).toBe("1.2.3.0");
  });

  test("drops prerelease suffixes", () => {
    expect(toMsixVersion("1.3.0-rc1")).toBe("1.3.0.0");
  });

  test("rejects non-semver input", () => {
    expect(() => toMsixVersion("1.2")).toThrow();
  });
});

describe("renderManifest", () => {
  test("fills every placeholder in the template", async () => {
    const template = await Bun.file(
      "src-tauri/msix/AppxManifest.template.xml"
    ).text();
    const rendered = renderManifest(template, {
      DESCRIPTION: "Fast & private",
      DISPLAY_NAME: "Fing Local Dictation",
      IDENTITY_NAME: "jamdaniels.FingLocalDictation",
      PRODUCT_NAME: "Fing",
      PUBLISHER: "CN=F15B9FD1-5558-46A1-A558-BEDDB9894FD6",
      PUBLISHER_DISPLAY_NAME: "jamdaniels",
      VERSION: "1.2.3.0",
    });

    expect(rendered).not.toMatch(/\{\{[A-Z_]+\}\}/);
    expect(rendered).toContain('Version="1.2.3.0"');
    expect(rendered).toContain("Fast &amp; private");
    expect(rendered).toContain('TaskId="FingStartup"');
  });

  test("fails on a placeholder without a value", () => {
    expect(() => renderManifest("<a>{{MISSING}}</a>", {})).toThrow("MISSING");
  });
});

describe("bundleResourceTargets", () => {
  test("uses map values and strips trailing slashes", () => {
    const targets = bundleResourceTargets({
      bundle: {
        resources: {
          "sounds/": "sounds/",
          "locales/macos/en.lproj/InfoPlist.strings":
            "en.lproj/InfoPlist.strings",
        },
      },
      productName: "Fing",
      version: "1.0.0",
    });
    expect(targets).toEqual(["sounds", "en.lproj/InfoPlist.strings"]);
  });

  test("accepts the list form", () => {
    const targets = bundleResourceTargets({
      bundle: { resources: ["sounds/"] },
      productName: "Fing",
      version: "1.0.0",
    });
    expect(targets).toEqual(["sounds"]);
  });
});
