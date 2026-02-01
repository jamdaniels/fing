use tauri::AppHandle;

/// Escape a string for use in AppleScript double-quoted strings
#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Escape a string for use in PowerShell single-quoted strings
#[cfg(target_os = "windows")]
fn escape_powershell(s: &str) -> String {
    // In PowerShell single-quoted strings, only ' needs escaping (doubled)
    // Also escape backticks and dollar signs for extra safety
    s.replace('\'', "''").replace('`', "``").replace('$', "`$")
}

/// Show error notification
pub fn show_error(app: &AppHandle, title: &str, message: &str) {
    #[cfg(target_os = "macos")]
    {
        let escaped_title = escape_applescript(title);
        let escaped_message = escape_applescript(message);
        let script = format!(
            "display notification \"{escaped_message}\" with title \"{escaped_title}\""
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let escaped_title = escape_powershell(title);
        let escaped_message = escape_powershell(message);
        let ps_script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $template = [Windows.UI.Notifications.ToastTemplateType]::ToastText02; \
             $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent($template); \
             $text = $xml.GetElementsByTagName('text'); \
             $text[0].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $text[1].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Fing').Show($toast);",
            escaped_title,
            escaped_message
        );
        let _ = std::process::Command::new("powershell")
            .args(["-Command", &ps_script])
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args(["-u", "critical", title, message])
            .spawn();
    }

    let _ = app;
    tracing::error!("Error notification: {} - {}", title, message);
}

/// Show informational notification
pub fn show_info(app: &AppHandle, title: &str, message: &str) {
    #[cfg(target_os = "macos")]
    {
        let escaped_title = escape_applescript(title);
        let escaped_message = escape_applescript(message);
        let script = format!(
            "display notification \"{escaped_message}\" with title \"{escaped_title}\""
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let escaped_title = escape_powershell(title);
        let escaped_message = escape_powershell(message);
        let ps_script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $template = [Windows.UI.Notifications.ToastTemplateType]::ToastText02; \
             $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent($template); \
             $text = $xml.GetElementsByTagName('text'); \
             $text[0].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $text[1].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Fing').Show($toast);",
            escaped_title,
            escaped_message
        );
        let _ = std::process::Command::new("powershell")
            .args(["-Command", &ps_script])
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args([title, message])
            .spawn();
    }

    let _ = app;
    tracing::info!("Info notification: {} - {}", title, message);
}

#[tauri::command]
pub fn notify_error(app: AppHandle, title: String, message: String) {
    show_error(&app, &title, &message);
}
