//! Distribution channel of the running binary. One binary serves every
//! channel; the channel is detected at runtime so store builds can leave
//! updates to the store and use its autostart mechanism.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Distribution {
    /// Downloaded installer (DMG / NSIS). Self-updates via the Tauri updater.
    Direct,
    /// Installed from the Microsoft Store (MSIX). The Store handles updates.
    MicrosoftStore,
}

pub fn current() -> Distribution {
    if crate::platform::is_packaged() {
        Distribution::MicrosoftStore
    } else {
        Distribution::Direct
    }
}

/// Store builds must not self-update: the install directory is read-only
/// and the store delivers updates itself.
pub fn is_store_build() -> bool {
    current() == Distribution::MicrosoftStore
}
