const env = { ...process.env };
const args = [process.execPath, "x", "tauri", "build"];

if (process.platform === "win32" && !env.CARGO_TARGET_DIR) {
  // ggml-vulkan's generated CMake/MSBuild paths exceed Windows limits under the repo-local target dir.
  env.CARGO_TARGET_DIR = "C:\\t";
}

if (!env.TAURI_SIGNING_PRIVATE_KEY) {
  args.push("--config", "src-tauri/tauri.local.conf.json", "--no-sign");
}

const proc = Bun.spawn(args, {
  cwd: process.cwd(),
  env,
  stdin: "inherit",
  stdout: "inherit",
  stderr: "inherit",
});

process.exit(await proc.exited);
