use rodio::{Decoder, DeviceSinkBuilder, Player};
use std::io::Cursor;

// Embed sound files at compile time
const START_SOUND: &[u8] = include_bytes!("../sounds/recording-start.wav");

fn play_sound(data: &'static [u8]) {
    std::thread::spawn(move || {
        let Ok(stream) = DeviceSinkBuilder::open_default_sink() else {
            tracing::warn!("Failed to get audio output stream for sound playback");
            return;
        };

        let cursor = Cursor::new(data);
        let Ok(source) = Decoder::try_from(cursor) else {
            tracing::warn!("Failed to decode sound file");
            return;
        };

        let player = Player::connect_new(stream.mixer());
        player.append(source);
        player.sleep_until_end();
    });
}

/// Play the recording start sound on a background thread
pub fn play_start() {
    play_sound(START_SOUND);
}
