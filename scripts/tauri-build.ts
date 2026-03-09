const env = { ...process.env };
const args = [process.execPath, "x", "tauri", "build"];
const extraArgs = process.argv.slice(2);
const targetFlagIndex = extraArgs.indexOf("--target");
const target =
  targetFlagIndex === -1 ? undefined : extraArgs[targetFlagIndex + 1];

if (process.platform === "darwin") {
  env.WHISPER_CCACHE ??= "OFF";
  env.CCACHE_DISABLE ??= "1";

  if (target === "universal-apple-darwin") {
    env.WHISPER_NATIVE ??= "OFF";
    env.CMAKE_CROSSCOMPILING ??= "ON";
    env.CARGO_BUILD_JOBS ??= "1";
    env.NUM_JOBS ??= "1";
    env.CMAKE_BUILD_PARALLEL_LEVEL ??= "1";
  }
}

if (process.platform === "win32" && !env.CMAKE_GENERATOR) {
  const ninjaPath = Bun.which("ninja") ?? Bun.which("ninja.exe");
  if (!ninjaPath) {
    console.error(
      "Windows Vulkan builds require Ninja in PATH or an explicit CMAKE_GENERATOR. Install Ninja and retry."
    );
    process.exit(1);
  }

  env.CMAKE_GENERATOR = "Ninja Multi-Config";
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
