const PACKAGE_JSON_PATH = "package.json";
const CARGO_TOML_PATH = "src-tauri/Cargo.toml";
const TAURI_CONFIG_PATH = "src-tauri/tauri.conf.json";
const CHANGELOG_PATH = "CHANGELOG.md";

const CARGO_VERSION_LINE_PATTERN = /^version = ".*"$/m;
const CARGO_VERSION_PATTERN = /^version = "(.*)"$/m;
const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

type Command = "bump" | "notes" | "verify-tag";

function fail(message: string): never {
  console.error(message);
  process.exit(1);
}

function normalizeVersion(input: string): string {
  const version = input.startsWith("v") ? input.slice(1) : input;

  if (!VERSION_PATTERN.test(version)) {
    fail(`Invalid version: ${input}`);
  }

  return version;
}

async function readJsonFile<T>(path: string): Promise<T> {
  const file = await Bun.file(path).text();

  return JSON.parse(file) as T;
}

async function writeJsonFile(path: string, value: unknown): Promise<void> {
  await Bun.write(path, `${JSON.stringify(value, null, 2)}\n`);
}

async function updatePackageVersion(version: string): Promise<void> {
  const packageJson =
    await readJsonFile<Record<string, unknown>>(PACKAGE_JSON_PATH);
  packageJson.version = version;
  await writeJsonFile(PACKAGE_JSON_PATH, packageJson);
}

async function updateTauriConfigVersion(version: string): Promise<void> {
  const tauriConfig =
    await readJsonFile<Record<string, unknown>>(TAURI_CONFIG_PATH);
  tauriConfig.version = version;
  await writeJsonFile(TAURI_CONFIG_PATH, tauriConfig);
}

function replaceCargoVersion(file: string, version: string): string {
  const updated = file.replace(
    CARGO_VERSION_LINE_PATTERN,
    `version = "${version}"`
  );

  if (updated === file) {
    fail(`Could not find package version in ${CARGO_TOML_PATH}`);
  }

  return updated;
}

async function updateCargoVersion(version: string): Promise<void> {
  const cargoToml = await Bun.file(CARGO_TOML_PATH).text();
  await Bun.write(CARGO_TOML_PATH, replaceCargoVersion(cargoToml, version));
}

async function readVersions(): Promise<{
  cargoVersion: string;
  packageVersion: string;
  tauriVersion: string;
}> {
  const [packageJson, tauriConfig, cargoToml] = await Promise.all([
    readJsonFile<{ version?: string }>(PACKAGE_JSON_PATH),
    readJsonFile<{ version?: string }>(TAURI_CONFIG_PATH),
    Bun.file(CARGO_TOML_PATH).text(),
  ]);

  const cargoVersionMatch = cargoToml.match(CARGO_VERSION_PATTERN);
  const cargoVersion = cargoVersionMatch?.[1];
  const packageVersion = packageJson.version;
  const tauriVersion = tauriConfig.version;

  if (!(cargoVersion && packageVersion && tauriVersion)) {
    fail("Could not read all version values.");
  }

  return {
    cargoVersion,
    packageVersion,
    tauriVersion,
  };
}

function extractReleaseNotes(changelog: string, version: string): string {
  const lines = changelog.split("\n");
  const heading = `## ${version} - `;
  const startIndex = lines.findIndex((line) => line.startsWith(heading));

  if (startIndex === -1) {
    fail(`Could not find changelog notes for ${version} in ${CHANGELOG_PATH}`);
  }

  const bodyLines: string[] = [];

  for (const line of lines.slice(startIndex + 1)) {
    if (line.startsWith("## ")) {
      break;
    }

    bodyLines.push(line);
  }

  const notes = bodyLines.join("\n").trim();

  if (!notes) {
    fail(`Could not find changelog notes for ${version} in ${CHANGELOG_PATH}`);
  }

  return notes;
}

async function bumpVersion(rawVersion: string): Promise<void> {
  const version = normalizeVersion(rawVersion);

  await Promise.all([
    updatePackageVersion(version),
    updateTauriConfigVersion(version),
    updateCargoVersion(version),
  ]);

  console.log(`Updated version files to ${version}`);
  console.log(
    "Run `cargo check --manifest-path src-tauri/Cargo.toml` before tagging."
  );
}

async function printReleaseNotes(rawTag: string): Promise<void> {
  const version = normalizeVersion(rawTag);
  const changelog = await Bun.file(CHANGELOG_PATH).text();
  process.stdout.write(`${extractReleaseNotes(changelog, version)}\n`);
}

async function verifyTag(rawTag: string): Promise<void> {
  const expectedVersion = normalizeVersion(rawTag);
  const { cargoVersion, packageVersion, tauriVersion } = await readVersions();

  const mismatches = [
    ["package.json", packageVersion],
    ["src-tauri/Cargo.toml", cargoVersion],
    ["src-tauri/tauri.conf.json", tauriVersion],
  ].filter(([, version]) => version !== expectedVersion);

  if (mismatches.length > 0) {
    const details = mismatches
      .map(([file, version]) => `${file}=${version}`)
      .join(", ");
    fail(`Tag v${expectedVersion} does not match version files: ${details}`);
  }

  const changelog = await Bun.file(CHANGELOG_PATH).text();
  extractReleaseNotes(changelog, expectedVersion);

  console.log(`Verified tag v${expectedVersion}`);
}

async function main(): Promise<void> {
  const [command, value] = process.argv.slice(2) as [
    Command | undefined,
    string | undefined,
  ];

  if (!(command && value)) {
    fail(
      "Usage: bun run scripts/release.ts <bump|verify-tag|notes> <version-or-tag>"
    );
  }

  switch (command) {
    case "bump":
      await bumpVersion(value);
      return;
    case "notes":
      await printReleaseNotes(value);
      return;
    case "verify-tag":
      await verifyTag(value);
      return;
    default:
      fail(`Unknown command: ${command}`);
  }
}

await main();
