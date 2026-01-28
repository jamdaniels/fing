# Fing - Future Improvements

## Missing from PRD (Settings UI)

- [ ] **pasteEnabled toggle** - Auto-paste on/off option in Settings
- [ ] **historyLimit dropdown** - Max transcripts to keep (100, 500, 1000, 5000, unlimited)
- [ ] **Model path display** - Show current model path with "Change..." button

## Minor TODOs in Code

- [ ] **Capture active app context** - hotkey.rs:360 - Use `NSWorkspace` (macOS) / `GetForegroundWindow` (Windows) to record which app was focused

## Quick Wins

- [ ] **History export** - CSV/JSON export button in History view
- [ ] **Notification preferences** - Let user disable "copied to clipboard" notification
- [ ] **Dark/light mode toggle** - Manual override instead of only system preference

## Medium Effort

- [ ] **Toggle mode** - Press-to-start/press-to-stop alternative to hold mode
- [ ] **Transcript editing** - Edit text after transcription before paste
- [ ] **Keyboard shortcut for history** - Global shortcut to open history/search
- [ ] **Recording duration display** - Show elapsed time on indicator during recording
- [ ] **Undo last transcription** - Quick undo if paste was wrong
- [ ] **Tray icon animation** - Animate tray icon during recording/processing

## Larger Features

- [ ] **Multi-language support** - Use ggml-base.bin or larger models for non-English
- [ ] **Voice commands** - "delete that", "new paragraph", "period" etc.
- [ ] **Per-app hotkey customization** - Different hotkey in different apps
- [ ] **Audio playback** - Store audio alongside transcript, allow replay
- [ ] **Silence detection/VAD** - Trim silence before transcription for speed
- [ ] **Cloud sync** - Optional sync transcripts across devices
- [ ] **Text corrections/replacements** - Auto-replace common errors or expand abbreviations
- [ ] **Moonshine engine** - Swappable engine for 2-3x speed (engine.rs trait exists)
- [ ] **Multiple models** - Let user choose between tiny/base/small models
- [ ] **Linux support** - X11 first, Wayland later

## UX Polish

- [ ] **Transcription confidence** - Show if Whisper was uncertain
- [ ] **Statistics charts** - Visual graphs in Home dashboard
- [ ] **Search highlighting** - Highlight matched terms in History search results
- [ ] **Transcript timestamps** - Show word-level timestamps for longer transcripts
