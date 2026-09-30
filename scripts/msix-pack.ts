// Packages the Windows build into an MSIX for Microsoft Store submission.
//
// The Store re-signs the package after certification, so the default output
// is unsigned and is NOT installable by end users. Pass --pfx to sign with a
// self-signed dev certificate for local Add-AppxPackage testing (subject
// must equal the manifest Publisher).
//
// Usage (Windows only):
//   bun run scripts/msix-pack.ts [--exe <path>] [--out <dir>]
//                                [--pfx <path>] [--version <x.y.z[-rcN]>]

import { existsSync, readdirSync, statSync } from "node:fs";
import { cp, mkdir, rm } from "node:fs/promises";
import { join, resolve } from "node:path";

const TAURI_CONFIG_PATH = "src-tauri/tauri.conf.json";
const MSIX_CONFIG_PATH = "src-tauri/msix/msix.config.json";
const MANIFEST_TEMPLATE_PATH = "src-tauri/msix/AppxManifest.template.xml";
const ICONS_DIR = "src-tauri/icons";
// Referenced from AppxManifest.template.xml under Assets\. Without the
// unplated/lightunplated (dark/light theme) target-size variants Windows
// draws the taskbar icon on an accent-colored plate.
const UNPLATED_TARGET_SIZES = [16, 20, 24, 30, 32, 36, 40, 48, 64, 256];
const ASSET_FILES = [
  "StoreLogo.png",
  "Square44x44Logo.png",
  "Square150x150Logo.png",
  ...UNPLATED_TARGET_SIZES.flatMap((size) => [
    `Square44x44Logo.targetsize-${size}_altform-unplated.png`,
    `Square44x44Logo.targetsize-${size}_altform-lightunplated.png`,
  ]),
];
const WINDOWS_KITS_BIN = "C:\\Program Files (x86)\\Windows Kits\\10\\bin";
const SEMVER_PATTERN = /^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?$/;

type MsixConfig = {
  description: string;
  displayName: string;
  identityName: string;
  publisher: string;
  publisherDisplayName: string;
};

type TauriConfig = {
  bundle: { resources: Record<string, string> | string[] };
  productName: string;
  version: string;
};

const FLAGS = {
  "--exe": "exe",
  "--out": "out",
  "--pfx": "pfx",
  "--version": "version",
} as const;

type Options = Partial<Record<(typeof FLAGS)[keyof typeof FLAGS], string>>;

function parseArgs(argv: string[]): Options {
  const options: Options = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = FLAGS[argv[i] as keyof typeof FLAGS];
    const value = argv[i + 1];
    if (!(key && value)) {
      throw new Error(`Unknown or incomplete argument: ${argv[i]}`);
    }
    options[key] = value;
  }
  return options;
}

// Store packages need a four-part version with the last part 0. Prerelease
// suffixes cannot be expressed, so an rc package carries the base version.
export function toMsixVersion(version: string): string {
  const match = version.match(SEMVER_PATTERN);
  if (!match) {
    throw new Error(`Version is not semver: ${version}`);
  }
  return `${match[1]}.${match[2]}.${match[3]}.0`;
}

function escapeXml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function renderManifest(
  template: string,
  values: Record<string, string>
): string {
  return template.replace(/\{\{([A-Z_]+)\}\}/g, (_match, key: string) => {
    const value = values[key];
    if (value === undefined) {
      throw new Error(`Manifest placeholder without value: ${key}`);
    }
    return escapeXml(value);
  });
}

function newestSdkTool(name: string): string {
  if (!existsSync(WINDOWS_KITS_BIN)) {
    throw new Error(`Windows SDK not found at ${WINDOWS_KITS_BIN}`);
  }

  const candidates = readdirSync(WINDOWS_KITS_BIN)
    .filter((entry) => /^10\.\d+\.\d+\.\d+$/.test(entry))
    .sort((a, b) =>
      b.localeCompare(a, undefined, { numeric: true, sensitivity: "base" })
    )
    .map((entry) => join(WINDOWS_KITS_BIN, entry, "x64", `${name}.exe`))
    .filter((path) => existsSync(path));

  const tool = candidates[0];
  if (!tool) {
    throw new Error(`${name}.exe not found under ${WINDOWS_KITS_BIN}`);
  }
  return tool;
}

async function run(command: string[]): Promise<void> {
  const proc = Bun.spawn(command, {
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  const code = await proc.exited;
  if (code !== 0) {
    throw new Error(`${command[0]} exited with code ${code}`);
  }
}

// Install-relative paths of bundle.resources (map values, or the entries of
// the list form), without trailing slashes.
export function bundleResourceTargets(config: TauriConfig): string[] {
  const resources = config.bundle.resources;
  const targets = Array.isArray(resources)
    ? resources
    : Object.values(resources);
  return targets.map((target) => target.replace(/\/+$/, ""));
}

function resolveExe(explicit: string | undefined): string {
  // Same default target dir as scripts/tauri-build.ts on Windows.
  const targetDir = process.env.CARGO_TARGET_DIR ?? "C:\\t";
  const candidates = explicit
    ? [explicit]
    : [
        join(targetDir, "release", "fing.exe"),
        "src-tauri/target/release/fing.exe",
      ];
  const found = candidates.find(
    (path) => existsSync(path) && statSync(path).isFile()
  );
  if (!found) {
    throw new Error(
      `Built executable not found. Run the Windows build first or pass --exe. Tried: ${candidates.join(", ")}`
    );
  }
  return found;
}

async function main(): Promise<void> {
  if (process.platform !== "win32") {
    throw new Error("MSIX packaging requires Windows (makeappx.exe)");
  }

  const options = parseArgs(process.argv.slice(2));
  const tauriConfig = (await Bun.file(TAURI_CONFIG_PATH).json()) as TauriConfig;
  const msixConfig = (await Bun.file(MSIX_CONFIG_PATH).json()) as MsixConfig;
  const template = await Bun.file(MANIFEST_TEMPLATE_PATH).text();

  const version = options.version ?? tauriConfig.version;
  const msixVersion = toMsixVersion(version);
  const exePath = resolveExe(options.exe);
  const exeDir = resolve(exePath, "..");

  const outDir = resolve(options.out ?? "src-tauri/target/release/bundle/msix");
  const stagingDir = join(outDir, "staging");
  const packagePath = join(outDir, `Fing_${version}_x64_store.msix`);

  await rm(stagingDir, { recursive: true, force: true });
  await mkdir(join(stagingDir, "Assets"), { recursive: true });

  // Same layout as the NSIS install: Fing.exe next to its bundle resources,
  // which tauri-build has already copied next to the built executable.
  await cp(exePath, join(stagingDir, "Fing.exe"));
  for (const target of bundleResourceTargets(tauriConfig)) {
    await cp(join(exeDir, target), join(stagingDir, target), {
      recursive: true,
    });
  }
  for (const asset of ASSET_FILES) {
    await cp(join(ICONS_DIR, asset), join(stagingDir, "Assets", asset));
  }

  const manifest = renderManifest(template, {
    DESCRIPTION: msixConfig.description,
    DISPLAY_NAME: msixConfig.displayName,
    IDENTITY_NAME: msixConfig.identityName,
    PRODUCT_NAME: tauriConfig.productName,
    PUBLISHER: msixConfig.publisher,
    PUBLISHER_DISPLAY_NAME: msixConfig.publisherDisplayName,
    VERSION: msixVersion,
  });
  await Bun.write(join(stagingDir, "AppxManifest.xml"), manifest);

  // Qualified asset names (targetsize-*, altform-*) are only resolved through
  // the package resource index, so build resources.pri from the staging dir.
  const priConfigPath = join(outDir, "priconfig.xml");
  const makepri = newestSdkTool("makepri");
  await run([
    makepri,
    "createconfig",
    "/o",
    "/cf",
    priConfigPath,
    "/dq",
    "en-US",
  ]);
  await run([
    makepri,
    "new",
    "/o",
    "/pr",
    stagingDir,
    "/cf",
    priConfigPath,
    "/mn",
    join(stagingDir, "AppxManifest.xml"),
    "/of",
    join(stagingDir, "resources.pri"),
  ]);

  await rm(packagePath, { force: true });
  await run([
    newestSdkTool("makeappx"),
    "pack",
    "/o",
    "/d",
    stagingDir,
    "/p",
    packagePath,
  ]);

  if (options.pfx) {
    const password = process.env.MSIX_PFX_PASSWORD;
    await run([
      newestSdkTool("signtool"),
      "sign",
      "/fd",
      "SHA256",
      "/f",
      options.pfx,
      ...(password ? ["/p", password] : []),
      packagePath,
    ]);
  }

  console.log(`MSIX package: ${packagePath} (version ${msixVersion})`);
  console.log(
    options.pfx
      ? "Signed with the given certificate for local testing."
      : "Unsigned: for Partner Center upload only, not installable directly."
  );
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
