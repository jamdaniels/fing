const PACKAGE_JSON_PATH = "package.json";
const CARGO_TOML_PATH = "src-tauri/Cargo.toml";
const CARGO_LOCK_PATH = "src-tauri/Cargo.lock";
const TAURI_CONFIG_PATH = "src-tauri/tauri.conf.json";
const CHANGELOG_PATH = "CHANGELOG.md";

const VERSION_FILES = [
  CHANGELOG_PATH,
  PACKAGE_JSON_PATH,
  CARGO_TOML_PATH,
  CARGO_LOCK_PATH,
  TAURI_CONFIG_PATH,
];

const CARGO_VERSION_LINE_PATTERN = /^version = ".*"$/m;
const CARGO_VERSION_PATTERN = /^version = "(.*)"$/m;
const CARGO_LOCK_VERSION_PATTERN =
  /(\[\[package\]\]\nname = "fing"\nversion = )"[^"]*"/;
const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
const BASE_VERSION_PATTERN = /^\d+\.\d+\.\d+$/;
const RC_SUFFIX_PATTERN = /-rc(\d+)$/;

// Commits that describe the release process itself, not the product.
const SKIPPED_COMMIT_PATTERNS = [
  /^(?:chore|docs|ci|test|build|style|refactor)(?:\(.*\))?:/i,
  /^(?:prepare|release) v\d/i,
  /^update (?:readme|changelog|agents|claude)/i,
  /^merge /i,
];

type Command = "bump" | "notes" | "release" | "verify-tag";

type ReleaseOptions = {
  assumeYes: boolean;
  dryRun: boolean;
  isRc: boolean;
  skipRcCheck: boolean;
  skipVerify: boolean;
};

function fail(message: string): never {
  console.error(`\n${message}`);
  process.exit(1);
}

function heading(message: string): void {
  console.log(`\n\x1b[1m${message}\x1b[0m`);
}

function normalizeVersion(input: string): string {
  const version = input.startsWith("v") ? input.slice(1) : input;

  if (!VERSION_PATTERN.test(version)) {
    fail(`Invalid version: ${input}`);
  }

  return version;
}

/** `1.2.3-rc2` -> `1.2.3`. The changelog is keyed by the base version. */
function baseVersion(version: string): string {
  return version.replace(RC_SUFFIX_PATTERN, "");
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

/**
 * Rewrite the `fing` entry in Cargo.lock directly. Running `cargo check` would
 * do the same thing but has to build the whole whisper tree to get there.
 */
async function updateCargoLockVersion(version: string): Promise<void> {
  const cargoLock = await Bun.file(CARGO_LOCK_PATH).text();
  const updated = cargoLock.replace(
    CARGO_LOCK_VERSION_PATTERN,
    `$1"${version}"`
  );

  if (updated === cargoLock && !cargoLock.includes(`version = "${version}"`)) {
    fail(`Could not find the fing package entry in ${CARGO_LOCK_PATH}`);
  }

  await Bun.write(CARGO_LOCK_PATH, updated);
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
  const heading = `## ${baseVersion(version)} - `;
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
    updateCargoLockVersion(version),
  ]);

  console.log(`Updated version files to ${version}`);
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

async function git(args: string[]): Promise<string> {
  const proc = Bun.spawn(["git", ...args], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);

  if (exitCode !== 0) {
    fail(`git ${args.join(" ")} failed:\n${stderr.trim()}`);
  }

  return stdout.trim();
}

/** Same as `git`, but returns null instead of exiting when the command fails. */
async function gitOrNull(args: string[]): Promise<string | null> {
  const proc = Bun.spawn(["git", ...args], {
    stdout: "pipe",
    stderr: "ignore",
  });
  const stdout = await new Response(proc.stdout).text();

  return (await proc.exited) === 0 ? stdout.trim() || null : null;
}

async function run(command: string[]): Promise<void> {
  const proc = Bun.spawn(command, {
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });

  if ((await proc.exited) !== 0) {
    fail(`\`${command.join(" ")}\` failed.`);
  }
}

async function assertReleasableTree(): Promise<void> {
  const branch = await git(["rev-parse", "--abbrev-ref", "HEAD"]);

  if (branch !== "main") {
    fail(`Releases must be cut from main. You are on ${branch}.`);
  }

  if (await git(["status", "--porcelain"])) {
    fail("The working tree has uncommitted changes. Commit or stash first.");
  }

  await git(["fetch", "--tags", "--force", "--quiet"]);

  const behind = await gitOrNull(["rev-list", "--count", "HEAD..@{upstream}"]);

  if (behind && behind !== "0") {
    fail(`main is ${behind} commit(s) behind its upstream. Pull first.`);
  }
}

async function listTags(pattern: string): Promise<string[]> {
  const output = await git(["tag", "--list", pattern]);

  return output ? output.split("\n") : [];
}

/** The next unused `-rcN` for this version, so the number is never typed. */
async function resolveRcTag(version: string): Promise<string> {
  const tags = await listTags(`v${version}-rc*`);
  const highest = tags.reduce((max, tag) => {
    const match = tag.match(RC_SUFFIX_PATTERN);
    const value = match ? Number(match[1]) : 0;

    return value > max ? value : max;
  }, 0);

  return `v${version}-rc${highest + 1}`;
}

/**
 * A final release should ship what an rc already proved. Compare against the
 * newest rc, ignoring the version files a `Prepare` commit always touches.
 */
async function warnAboutUntestedChanges(version: string): Promise<string[]> {
  const tags = await listTags(`v${version}-rc*`);

  if (tags.length === 0) {
    return [];
  }

  const newest = tags.sort((left, right) => {
    const leftValue = Number(left.match(RC_SUFFIX_PATTERN)?.[1] ?? 0);
    const rightValue = Number(right.match(RC_SUFFIX_PATTERN)?.[1] ?? 0);

    return rightValue - leftValue;
  })[0];

  const changed = await git([
    "diff",
    "--name-only",
    `${newest}..HEAD`,
    "--",
    ".",
    ...VERSION_FILES.map((path) => `:!${path}`),
  ]);

  if (!changed) {
    return [];
  }

  return [
    `Code changed since ${newest}, the last release candidate you tested:`,
    ...changed.split("\n").map((path) => `  ${path}`),
  ];
}

async function draftedEntries(sinceTag: string | null): Promise<string[]> {
  const range = sinceTag ? `${sinceTag}..HEAD` : "HEAD";
  const output = await git(["log", "--no-merges", "--format=%s", range]);

  if (!output) {
    return [];
  }

  return output
    .split("\n")
    .filter(
      (subject) =>
        !SKIPPED_COMMIT_PATTERNS.some((pattern) => pattern.test(subject))
    )
    .map((subject) => `- ${subject}`);
}

async function mostRecentTag(): Promise<string | null> {
  return await gitOrNull(["describe", "--tags", "--abbrev=0"]);
}

function today(): string {
  return new Date().toISOString().slice(0, 10);
}

/**
 * Seed or extend the section for this version. The heading always carries the
 * base version, so promoting an rc to a release never rewrites it.
 */
async function updateChangelog(version: string): Promise<boolean> {
  const base = baseVersion(version);
  const changelog = await Bun.file(CHANGELOG_PATH).text();
  const lines = changelog.split("\n");
  const headingIndex = lines.findIndex((line) =>
    line.startsWith(`## ${base} - `)
  );
  const entries = await draftedEntries(await mostRecentTag());

  if (headingIndex === -1) {
    const firstSectionIndex = lines.findIndex((line) => line.startsWith("## "));
    const insertIndex =
      firstSectionIndex === -1 ? lines.length : firstSectionIndex;
    const section = [`## ${base} - ${today()}`, "", ...entries, ""];
    lines.splice(insertIndex, 0, ...section);
    await Bun.write(CHANGELOG_PATH, lines.join("\n"));

    return true;
  }

  lines[headingIndex] = `## ${base} - ${today()}`;

  if (entries.length === 0) {
    await Bun.write(CHANGELOG_PATH, lines.join("\n"));

    return false;
  }

  let endIndex = headingIndex + 1;

  while (endIndex < lines.length && !lines[endIndex]?.startsWith("## ")) {
    endIndex += 1;
  }

  while (endIndex > headingIndex + 1 && !lines[endIndex - 1]?.trim()) {
    endIndex -= 1;
  }

  lines.splice(endIndex, 0, ...entries);
  await Bun.write(CHANGELOG_PATH, lines.join("\n"));

  return true;
}

function confirm(question: string): boolean {
  if (!process.stdin.isTTY) {
    fail("Not a terminal. Re-run interactively, or pass --yes.");
  }

  const answer = prompt(`${question} [y/N]`);

  return answer?.trim().toLowerCase() === "y";
}

async function release(
  rawVersion: string,
  options: ReleaseOptions
): Promise<void> {
  const base = normalizeVersion(rawVersion);

  if (!BASE_VERSION_PATTERN.test(base)) {
    fail(
      `Pass the plain version, without a suffix: ${baseVersion(base)}\n` +
        "Add --rc to cut the next release candidate for it."
    );
  }

  await assertReleasableTree();

  const tag = options.isRc ? await resolveRcTag(base) : `v${base}`;
  const version = tag.slice(1);

  if ((await listTags(tag)).length > 0) {
    fail(`Tag ${tag} already exists.`);
  }

  if (!(options.isRc || options.skipRcCheck)) {
    if ((await listTags(`v${base}-rc*`)).length === 0) {
      fail(
        `No release candidate was ever cut for ${base}.\n` +
          `Run \`bun run release ${base} --rc\` first, or pass --no-rc.`
      );
    }
  }

  heading(`Preparing ${tag}`);

  const drafted = await updateChangelog(version);
  await bumpVersion(version);

  if (drafted) {
    console.log(`Drafted new ${CHANGELOG_PATH} entries from commit subjects.`);
  }

  if (options.skipVerify) {
    console.log("Skipping `bun run verify`.");
  } else {
    heading("Verifying");
    await run(["bun", "run", "verify"]);
  }

  await verifyTag(tag);

  const notes = extractReleaseNotes(
    await Bun.file(CHANGELOG_PATH).text(),
    version
  );

  heading(`Release notes for ${tag}`);
  console.log(`${notes}\n`);
  console.log("These are published to the GitHub release verbatim.");

  const warnings = options.isRc ? [] : await warnAboutUntestedChanges(base);

  if (warnings.length > 0) {
    heading("Warning");
    console.log(warnings.join("\n"));
  }

  heading("Changes to commit");
  await run(["git", "--no-pager", "diff", "--stat", "--", ...VERSION_FILES]);

  if (options.dryRun) {
    console.log(
      `\nDry run. ${CHANGELOG_PATH} and the version files were edited but ` +
        "nothing was committed."
    );

    return;
  }

  const summary = options.isRc
    ? `Tag ${tag} and push a prerelease?`
    : `Tag ${tag} and push a PUBLIC release?`;

  if (!(options.assumeYes || confirm(`\n${summary}`))) {
    console.log(
      `\nStopped. Edit ${CHANGELOG_PATH} and re-run the same command, or ` +
        `run \`git checkout -- ${VERSION_FILES.join(" ")}\` to undo.`
    );

    return;
  }

  const message = options.isRc ? `Prepare ${tag}` : `Release ${tag}`;

  await git(["add", "--", ...VERSION_FILES]);
  await git(["commit", "-m", message]);
  await git(["tag", tag]);
  await git(["push", "origin", "main"]);
  await git(["push", "origin", tag]);

  heading(`Pushed ${tag}`);
  console.log("The release workflow builds, signs, and publishes it.");
}

function parseReleaseOptions(args: string[]): ReleaseOptions {
  const known = new Set([
    "--dry-run",
    "--no-rc",
    "--rc",
    "--skip-verify",
    "--yes",
  ]);

  for (const arg of args) {
    if (!known.has(arg)) {
      fail(`Unknown option: ${arg}`);
    }
  }

  return {
    assumeYes: args.includes("--yes"),
    dryRun: args.includes("--dry-run"),
    isRc: args.includes("--rc"),
    skipRcCheck: args.includes("--no-rc"),
    skipVerify: args.includes("--skip-verify"),
  };
}

async function main(): Promise<void> {
  const [command, value, ...rest] = process.argv.slice(2) as [
    Command | undefined,
    string | undefined,
    ...string[],
  ];

  if (!(command && value)) {
    fail(
      "Usage: bun run scripts/release.ts <release|bump|verify-tag|notes> " +
        "<version-or-tag> [options]"
    );
  }

  switch (command) {
    case "bump":
      await bumpVersion(value);
      return;
    case "notes":
      await printReleaseNotes(value);
      return;
    case "release":
      await release(value, parseReleaseOptions(rest));
      return;
    case "verify-tag":
      await verifyTag(value);
      return;
    default:
      fail(`Unknown command: ${command}`);
  }
}

await main();
