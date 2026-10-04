use rodio::{Decoder, DeviceSinkBuilder, Player};
use std::io::Cursor;
use std::sync::mpsc;

// Embed sound files at compile time
const START_SOUND: &[u8] = include_bytes!("../sounds/recording-start.wav");

/// A start sound whose output device is already being opened. Call
/// [`StartCue::play`] once recording is live; dropping it stays silent.
pub struct StartCue(mpsc::Sender<()>);

impl StartCue {
    pub fn play(self) {
        let _ = self.0.send(());
    }
}

/// Open the output and decode the start sound on a background thread, so that
/// work overlaps microphone startup instead of following it.
pub fn prepare_start() -> StartCue {
    let (play_tx, play_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok(stream) = DeviceSinkBuilder::open_default_sink() else {
            tracing::warn!("Failed to get audio output stream for sound playback");
            return;
        };

        let cursor = Cursor::new(START_SOUND);
        let Ok(source) = Decoder::try_from(cursor) else {
            tracing::warn!("Failed to decode sound file");
            return;
        };

        // The cue was dropped without playing (recording failed to start).
        if play_rx.recv().is_err() {
            return;
        }

        let player = Player::connect_new(stream.mixer());
        player.append(source);
        player.sleep_until_end();
    });
    StartCue(play_tx)
}
