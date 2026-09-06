use std::process::ExitCode;

use spotify_tui::playback::{MprisPlaybackSource, PlaybackError, PlaybackSnapshot, PlaybackSource};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let source = match MprisPlaybackSource::connect().await {
        Ok(source) => source,
        Err(error) => return report_error(&error),
    };

    match source.snapshot().await {
        Ok(snapshot) => {
            print_snapshot(&snapshot);
            ExitCode::SUCCESS
        }
        Err(PlaybackError::Disconnected) => {
            println!("connection: disconnected");
            ExitCode::SUCCESS
        }
        Err(error) => report_error(&error),
    }
}

fn print_snapshot(snapshot: &PlaybackSnapshot) {
    println!("connection: connected");
    println!("status: {}", snapshot.status);
    println!("title: {}", optional_text(snapshot.track.title.as_deref()));
    println!("artists: {}", list_text(&snapshot.track.artists));
    println!("album: {}", optional_text(snapshot.track.album.as_deref()));
    println!("position_us: {}", snapshot.position.as_micros());
    println!(
        "duration_us: {}",
        snapshot.track.duration.map_or_else(
            || "unknown".to_owned(),
            |value| value.as_micros().to_string()
        )
    );
    println!("volume: {:.3}", snapshot.volume);
    println!(
        "art_url: {}",
        optional_text(snapshot.track.art_url.as_deref())
    );
}

fn optional_text(value: Option<&str>) -> &str {
    value.unwrap_or("unknown")
}

fn list_text(values: &[String]) -> String {
    if values.is_empty() {
        "unknown".to_owned()
    } else {
        values.join(", ")
    }
}

fn report_error(error: &PlaybackError) -> ExitCode {
    eprintln!("error: {error}");
    ExitCode::FAILURE
}
