# Tauri v2 Best Practices

> Comprehensive guide for building Tauri v2 applications. Verified against official documentation at tauri.app (January 2026).

---

## 1. Project Structure

```
my-tauri-app/
├── src-tauri/
│   ├── src/
│   │   ├── lib.rs               # App setup (with mobile_entry_point)
│   │   ├── main.rs              # Desktop entry (calls lib.rs)
│   │   ├── commands/            # Command modules
│   │   │   ├── mod.rs
│   │   │   └── files.rs
│   │   ├── state.rs             # Application state
│   │   └── error.rs             # Custom error types
│   ├── capabilities/            # Permission configurations
│   │   └── default.json
│   ├── icons/
│   ├── Cargo.toml
│   └── tauri.conf.json
├── src/                         # Frontend source
│   ├── lib/
│   │   └── tauri.ts             # Typed API wrappers
│   └── main.tsx
├── package.json
└── vite.config.ts
```

---

## 2. Initial Setup

```bash
# Create new project
npm create tauri-app@latest

# Or add to existing project
npm install -D @tauri-apps/cli@latest
npm run tauri init
```

### Cargo.toml

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2021"

[lib]
name = "my_app_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-shell = "2"
tauri-plugin-dialog = "2"
tauri-plugin-fs = "2"
tauri-plugin-os = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
thiserror = "2"

[profile.release]
strip = true
lto = true
codegen-units = 1
panic = "abort"
```

### tauri.conf.json

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "My App",
  "version": "0.1.0",
  "identifier": "com.mycompany.myapp",
  "build": {
    "beforeDevCommand": "npm run dev",
    "devUrl": "http://localhost:5173",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      {
        "label": "main",
        "title": "My App",
        "width": 1200,
        "height": 800,
        "resizable": true,
        "center": true
      }
    ],
    "security": {
      "csp": {
        "default-src": "'self' customprotocol: asset:",
        "script-src": "'self'",
        "style-src": "'self' 'unsafe-inline'",
        "img-src": "'self' asset: http://asset.localhost blob: data:",
        "connect-src": "'self' ipc: http://ipc.localhost"
      },
      "assetProtocol": {
        "enable": true,
        "scope": ["$APPDATA/**", "$RESOURCE/**"]
      }
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

**CSP Notes:**
- `customprotocol:` and `asset:` in default-src are required for bundled assets in release builds
- Add `'wasm-unsafe-eval'` to script-src only if using WebAssembly (Rust/Yew/Leptos frontends)
- Tauri auto-appends nonces/hashes at compile time
- `assetProtocol.enable: true` is required to use `convertFileSrc()` for local file URLs

---

## 3. Rust Backend

### Entry Points

```rust
// src-tauri/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    my_app_lib::run();
}
```

```rust
// src-tauri/src/lib.rs
mod commands;
mod error;
mod state;

use state::AppState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_os::init())
        .manage(Mutex::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::files::read_file,
            commands::files::write_file,
        ])
        .setup(|app| {
            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

### State Management

**Use `std::sync::Mutex`** - Tauri handles `Arc` internally. Only use `tokio::sync::Mutex` when holding locks across `.await` points.

```rust
// src-tauri/src/state.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct AppState {
    pub theme: String,
    pub counter: u32,
}

// Type alias prevents State<T> type mismatches (causes runtime panics, not compile errors)
pub type AppStateHandle = std::sync::Mutex<AppState>;
```

```rust
// Accessing state in commands
use tauri::State;
use crate::state::AppStateHandle;

#[tauri::command]
pub fn get_counter(state: State<'_, AppStateHandle>) -> u32 {
    state.lock().unwrap().counter
}

#[tauri::command]
pub fn increment(state: State<'_, AppStateHandle>) -> u32 {
    let mut s = state.lock().unwrap();
    s.counter += 1;
    s.counter
}
```

### Error Handling

```rust
// src-tauri/src/error.rs
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("File error: {0}")]
    File(#[from] std::io::Error),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Access denied: {0}")]
    AccessDenied(String),
}

// Required: serialize errors for frontend
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
```

### Commands with Scope Validation

**Important:** Plugin permissions (like `fs:scope`) only apply to plugin commands, NOT custom commands. You must enforce your own path validation in custom commands.

```rust
// src-tauri/src/commands/files.rs
use crate::error::AppError;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Validates that a path is within allowed directories.
/// Custom commands MUST validate paths - plugin scopes don't apply here.
fn validate_path(app: &AppHandle, user_path: &str) -> Result<PathBuf, AppError> {
    let path = PathBuf::from(user_path);

    // Get allowed base directories
    let app_data = app.path().app_data_dir()
        .map_err(|e| AppError::Validation(e.to_string()))?;
    let documents = dirs::document_dir()
        .ok_or_else(|| AppError::Validation("Cannot resolve documents dir".into()))?;

    let allowed_bases = [app_data, documents];

    // Canonicalize to resolve symlinks and ..
    let canonical = path.canonicalize()
        .map_err(|_| AppError::NotFound(user_path.to_string()))?;

    // Check if path is within allowed directories
    let is_allowed = allowed_bases.iter().any(|base| canonical.starts_with(base));

    if !is_allowed {
        return Err(AppError::AccessDenied(format!(
            "Path '{}' is outside allowed directories",
            user_path
        )));
    }

    Ok(canonical)
}

#[tauri::command]
pub async fn read_file(app: AppHandle, path: String) -> Result<String, AppError> {
    let validated_path = validate_path(&app, &path)?;
    tokio::fs::read_to_string(&validated_path)
        .await
        .map_err(AppError::File)
}

#[tauri::command]
pub async fn write_file(app: AppHandle, path: String, content: String) -> Result<(), AppError> {
    let validated_path = validate_path(&app, &path)?;
    tokio::fs::write(&validated_path, &content)
        .await
        .map_err(AppError::File)
}
```

**Alternative:** Use the fs plugin APIs directly from frontend instead of custom commands - plugin scopes will then apply automatically.

---

## 4. Capabilities & Permissions

All files in `src-tauri/capabilities/` are automatically enabled. Custom commands via `invoke_handler` are allowed by default for all windows.

```json
// src-tauri/capabilities/default.json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Default capabilities for main window",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "shell:allow-open",
    "dialog:allow-open",
    "dialog:allow-save",
    "fs:default",
    "fs:allow-read-text-file",
    "fs:allow-write-text-file",
    "fs:allow-exists",
    "fs:allow-mkdir",
    {
      "identifier": "fs:scope",
      "allow": [
        { "path": "$APPDATA" },
        { "path": "$APPDATA/**" },
        { "path": "$DOCUMENT" },
        { "path": "$DOCUMENT/**" }
      ],
      "deny": [
        { "path": "$HOME/.ssh" },
        { "path": "$HOME/.ssh/**" }
      ]
    }
  ]
}
```

**Notes:**
- Schema path varies by target: `desktop-schema.json`, `mobile-schema.json`, etc.
- Run `npm run tauri build` once to generate schemas in `src-tauri/gen/schemas/`

**Permission naming:**
- Command-specific: `fs:allow-read-text-file`, `fs:deny-write-file`
- Grouped: `fs:read-all`, `fs:write-all`
- Directory-specific: `fs:allow-appdata-read-recursive`
- Scope definition uses path objects: `{ "path": "$APPDATA/**" }`

### Platform-Specific Capabilities

```json
// src-tauri/capabilities/mobile.json
{
  "identifier": "mobile",
  "platforms": ["android", "iOS"],
  "windows": ["main"],
  "permissions": [
    "core:default",
    "haptics:allow-vibrate"
  ]
}
```

---

## 5. IPC Patterns

### Commands (Request/Response)

```typescript
// Frontend
import { invoke } from '@tauri-apps/api/core';

const content = await invoke<string>('read_file', { path: '/path/to/file' });
```

### Events (Push Notifications)

Use for small payloads, multi-consumer patterns. Not type-safe, always JSON serialized.

```rust
// Backend - use tauri::async_runtime::spawn for portability
use tauri::{AppHandle, Emitter};

#[tauri::command]
pub async fn start_task(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn(async move {
        for i in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = app.emit("progress", i);
        }
    });
    Ok(())
}
```

```typescript
// Frontend
import { listen } from '@tauri-apps/api/event';

const unlisten = await listen<number>('progress', (event) => {
  console.log(`Progress: ${event.payload}%`);
});

// Cleanup
unlisten();
```

### Channels (Streaming Data)

Use for large payloads, ordered delivery, high throughput. Faster than events.

```rust
use tauri::ipc::Channel;

#[tauri::command]
pub async fn stream_file(path: String, channel: Channel<Vec<u8>>) -> Result<(), String> {
    let data = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;

    for chunk in data.chunks(4096) {
        channel.send(chunk.to_vec()).map_err(|e| e.to_string())?;
    }
    Ok(())
}
```

```typescript
import { invoke, Channel } from '@tauri-apps/api/core';

const channel = new Channel<number[]>();
channel.onmessage = (chunk) => {
  console.log(`Received ${chunk.length} bytes`);
};

await invoke('stream_file', { path: '/path/to/file', channel });
```

---

## 6. Frontend Integration

### Typed API Wrapper

```typescript
// src/lib/tauri.ts
import { invoke } from '@tauri-apps/api/core';

export const api = {
  files: {
    read: (path: string) => invoke<string>('read_file', { path }),
    write: (path: string, content: string) => invoke<void>('write_file', { path, content }),
  },
};
```

### Using FS Plugin Directly (Recommended)

Plugin APIs respect capability scopes automatically:

```typescript
import { open, save } from '@tauri-apps/plugin-dialog';
import { readTextFile, writeTextFile } from '@tauri-apps/plugin-fs';

async function openFile() {
  const path = await open({
    filters: [{ name: 'Text', extensions: ['txt', 'md'] }],
  });
  if (path) {
    // fs:scope applies here automatically
    return await readTextFile(path);
  }
}

async function saveFile(path: string, content: string) {
  // fs:scope applies here automatically
  await writeTextFile(path, content);
}
```

---

## 7. Multi-Window Management

### Configuration

```json
{
  "app": {
    "windows": [
      { "label": "main", "title": "My App", "width": 1200, "height": 800 },
      { "label": "settings", "title": "Settings", "width": 600, "height": 400, "visible": false, "url": "/settings" }
    ]
  }
}
```

### Programmatic Creation

**Always use async commands for window creation** - sync commands can deadlock on Windows.

```rust
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
pub async fn open_settings(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("settings") {
        window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Settings")
        .inner_size(600.0, 400.0)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(())
}
```

---

## 8. Security

### Key Points

1. **Plugin scopes only apply to plugin commands** - custom commands need manual validation
2. **Use `tauri::async_runtime::spawn`** - more portable than `tokio::spawn`
3. **CSP must include `customprotocol:` and `asset:`** in default-src for release builds
4. **Enable `assetProtocol`** in config if using `convertFileSrc()`

### Checklist

- [ ] Capabilities follow least-privilege principle
- [ ] CSP includes `customprotocol:` and `asset:` in default-src
- [ ] Custom commands validate paths (plugin scopes don't apply)
- [ ] All user input validated on Rust side
- [ ] No remote scripts loaded from CDNs
- [ ] Debug features disabled in production
- [ ] Run `cargo audit` and `npm audit`

---

## 9. Common Gotchas

### Async Commands Cannot Use Borrowed Types

```rust
// Won't compile
#[tauri::command]
pub async fn bad(name: &str) -> String { ... }

// Use owned types
#[tauri::command]
pub async fn good(name: String) -> String { ... }

// State in async requires Result return
#[tauri::command]
pub async fn with_state(state: State<'_, AppStateHandle>) -> Result<String, ()> {
    Ok(state.lock().unwrap().theme.clone())
}
```

### Use tauri::async_runtime::spawn

```rust
// Works but less portable - breaks if runtime setup changes
tokio::spawn(async { ... });

// Preferred - integrates with Tauri's runtime
tauri::async_runtime::spawn(async { ... });
```

### Don't Block Async Runtime

```rust
// Bad - blocks runtime
std::thread::sleep(Duration::from_secs(1));

// Good - non-blocking
tokio::time::sleep(Duration::from_secs(1)).await;
```

### State Type Mismatch = Runtime Panic

```rust
// If registered as: app.manage(Mutex::new(AppState::default()))
// Must use: State<'_, Mutex<AppState>>
// NOT: State<'_, AppState> // PANICS!
```

### Plugin Scopes Don't Apply to Custom Commands

```rust
// This command can access ANY path - fs:scope doesn't protect it!
#[tauri::command]
pub async fn read_file(path: String) -> Result<String, String> {
    tokio::fs::read_to_string(&path).await.map_err(|e| e.to_string())
}

// Solution: validate paths manually or use fs plugin APIs from frontend
```

### WebView Context Isolation

Each window has its own JS context. Use events or backend to sync state between windows.

### Use Tauri Path APIs

```typescript
// Bad
const path = '/Users/me/.config/myapp/config.json';

// Good
import { appDataDir, join } from '@tauri-apps/api/path';
const appData = await appDataDir();
const path = await join(appData, 'config.json');
```

---

## 10. Build & Distribution

```bash
# Development
npm run tauri dev

# Production
npm run tauri build

# Specific target
npm run tauri build -- --target x86_64-apple-darwin
```

### Updater Setup

```bash
# Generate signing keys
tauri signer generate -w ~/.tauri/myapp.key
```

```json
// tauri.conf.json
{
  "plugins": {
    "updater": {
      "pubkey": "YOUR_PUBLIC_KEY",
      "endpoints": ["https://releases.example.com/{{target}}/{{arch}}/{{current_version}}"]
    }
  },
  "bundle": {
    "createUpdaterArtifacts": true
  }
}
```

```typescript
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

const update = await check();
if (update) {
  await update.downloadAndInstall();
  await relaunch();
}
```

---

## 11. Common Plugins

```toml
tauri-plugin-dialog = "2"      # File dialogs
tauri-plugin-fs = "2"          # File system access
tauri-plugin-shell = "2"       # Shell/process spawning
tauri-plugin-store = "2"       # Key-value storage
tauri-plugin-updater = "2"     # Auto-updates
tauri-plugin-notification = "2" # System notifications
tauri-plugin-clipboard-manager = "2"
tauri-plugin-http = "2"        # HTTP client
tauri-plugin-os = "2"          # OS information
tauri-plugin-process = "2"     # Process management
```

---

## Quick Reference

| Task | Command |
|------|---------|
| Dev server | `npm run tauri dev` |
| Build | `npm run tauri build` |
| Add plugin | `npm run tauri add <plugin>` |
| Generate icons | `npm run tauri icon /path/to/icon.png` |
| Security audit | `cargo audit && npm audit` |

**Links:**
- [Tauri Docs](https://tauri.app/)
- [Plugin Directory](https://tauri.app/plugin/)
- [API Reference](https://docs.rs/tauri/latest/tauri/)

---

*Verified against Tauri v2.x official documentation - January 2026*
