use tauri::AppHandle;

/// Show notification that text was copied to clipboard
pub fn show_clipboard_fallback(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("osascript")
            .args([
                "-e",
                "display notification \"Text copied to clipboard\" with title \"Fing\"",
            ])
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        // Windows toast notification via PowerShell
        let _ = std::process::Command::new("powershell")
            .args([
                "-Command",
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
                 $template = [Windows.UI.Notifications.ToastTemplateType]::ToastText02; \
                 $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent($template); \
                 $text = $xml.GetElementsByTagName('text'); \
                 $text[0].AppendChild($xml.CreateTextNode('Fing')) | Out-Null; \
                 $text[1].AppendChild($xml.CreateTextNode('Text copied to clipboard')) | Out-Null; \
                 $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Fing').Show($toast);"
            ])
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args(["Fing", "Text copied to clipboard"])
            .spawn();
    }

    let _ = app; // Suppress unused warning when not needed
    tracing::info!("Clipboard fallback notification shown");
}

/// Show error notification
pub fn show_error(app: &AppHandle, title: &str, message: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            message.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let ps_script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $template = [Windows.UI.Notifications.ToastTemplateType]::ToastText02; \
             $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent($template); \
             $text = $xml.GetElementsByTagName('text'); \
             $text[0].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $text[1].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Fing').Show($toast);",
            title.replace('\'', "''"),
            message.replace('\'', "''")
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
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            message.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let ps_script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $template = [Windows.UI.Notifications.ToastTemplateType]::ToastText02; \
             $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent($template); \
             $text = $xml.GetElementsByTagName('text'); \
             $text[0].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $text[1].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Fing').Show($toast);",
            title.replace('\'', "''"),
            message.replace('\'', "''")
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

// Tauri commands for notifications
#[tauri::command]
pub fn notify_clipboard_fallback(app: AppHandle) {
    show_clipboard_fallback(&app);
}

#[tauri::command]
pub fn notify_error(app: AppHandle, title: String, message: String) {
    show_error(&app, &title, &message);
}
