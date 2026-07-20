const env = { ...process.env };

if (process.platform === "win32") {
  // Keep whisper-rs-sys's nested Vulkan helper build below Windows' path limit.
  env.CARGO_TARGET_DIR ??= "C:\\ft";
  env.GGML_CCACHE ??= "OFF";
}

const proc = Bun.spawn(
  [
    process.execPath,
    "x",
    "tauri",
    "dev",
    "--config",
    "src-tauri/tauri.dev.conf.json",
    ...process.argv.slice(2),
  ],
  {
    cwd: process.cwd(),
    env,
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  }
);

process.exit(await proc.exited);
