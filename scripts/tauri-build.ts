const env = { ...process.env };
const args = [process.execPath, "x", "tauri", "build"];
const extraArgs = process.argv.slice(2);
const targetFlagIndex = extraArgs.indexOf("--target");
const target =
  targetFlagIndex === -1 ? undefined : extraArgs[targetFlagIndex + 1];

if (process.platform === "darwin") {
  const pathEntries = (env.PATH ?? "").split(":");
  const rustPathEntries = [
    `${env.HOME ?? ""}/.cargo/bin`,
    "/opt/homebrew/opt/rustup/bin",
    "/usr/local/opt/rustup/bin",
  ].filter(
    (pathEntry) =>
      pathEntry !== "/.cargo/bin" && !pathEntries.includes(pathEntry)
  );

  if (rustPathEntries.length > 0) {
    env.PATH = [...rustPathEntries, ...pathEntries].join(":");
  }

  // whisper-sys's build.rs sets the whisper.cpp CMake options per target
  // architecture itself (no ccache, no native CPU tuning, Metal on arm64).
  if (target === "universal-apple-darwin") {
    env.CARGO_BUILD_JOBS ??= "1";
    env.NUM_JOBS ??= "1";
    env.CMAKE_BUILD_PARALLEL_LEVEL ??= "1";
  }
}

if (process.platform === "win32") {
  env.CARGO_TARGET_DIR ??= "C:\\t";
}

if (!env.TAURI_SIGNING_PRIVATE_KEY) {
  args.push("--config", "src-tauri/tauri.local.conf.json", "--no-sign");
}

args.push(...extraArgs);

const proc = Bun.spawn(args, {
  cwd: process.cwd(),
  env,
  stdin: "inherit",
  stdout: "inherit",
  stderr: "inherit",
});

process.exit(await proc.exited);
