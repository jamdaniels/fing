use rodio::{Decoder, OutputStream, Sink};
use std::io::Cursor;

// Embed sound files at compile time
const START_SOUND: &[u8] = include_bytes!("../sounds/recording-start.wav");

fn play_sound(data: &'static [u8]) {
    std::thread::spawn(move || {
        let Ok((_stream, stream_handle)) = OutputStream::try_default() else {
            tracing::warn!("Failed to get audio output stream for sound playback");
            return;
        };

        let Ok(sink) = Sink::try_new(&stream_handle) else {
            tracing::warn!("Failed to create audio sink for sound playback");
            return;
        };

        let cursor = Cursor::new(data);
        let Ok(source) = Decoder::new(cursor) else {
            tracing::warn!("Failed to decode sound file");
            return;
        };

        sink.append(source);
        sink.sleep_until_end();
    });
}

/// Play the recording start sound on a background thread
pub fn play_start() {
    play_sound(START_SOUND);
}
