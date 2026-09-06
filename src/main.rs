use std::{
    env,
    error::Error,
    ffi::OsString,
    io,
    process::ExitCode,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use spotify_tui::{
    app::{AppEvent, AppState, Command},
    auth::{self, AuthOutcome},
    cli::{self, LaunchMode},
    config::{Config, Theme},
    input,
    playback::PlaybackCommand,
    playback_runtime::{PlaybackEvent, PlaybackRuntime},
    ui,
};

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

fn main() -> ExitCode {
    match cli::parse_args(env::args_os().skip(1)) {
        Ok(LaunchMode::Tui) => report_result(run_tui_app()),
        Ok(LaunchMode::Authenticate { spotifyd_arguments }) => {
            report_result(run_auth_command(&spotifyd_arguments))
        }
        Ok(LaunchMode::Help) => {
            print!("{}", cli::HELP);
            ExitCode::SUCCESS
        }
        Ok(LaunchMode::Version) => {
            println!("spotify-tui {}", cli::VERSION);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("spotify-tui: {error}");
            ExitCode::from(2)
        }
    }
}

fn report_result(result: Result<(), Box<dyn Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spotify-tui: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_auth_command(arguments: &[OsString]) -> Result<(), Box<dyn Error>> {
    println!("Starting Spotifyd authentication…");
    let outcome = auth::authenticate(arguments)?;
    println!("Spotifyd authentication complete.");
    report_restart_warning(&outcome);
    Ok(())
}

fn run_tui_app() -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;
    let mut app = AppState::default();
    let playback = PlaybackRuntime::start(config.startup_uri().map(str::to_owned))?;

    loop {
        match run_tui_session(&mut app, config.theme(), &playback)? {
            SessionOutcome::Quit => return Ok(()),
            SessionOutcome::Authenticate => {
                println!("Starting Spotifyd authentication…");
                match auth::authenticate(&[]) {
                    Ok(outcome) => {
                        apply_auth_outcome(&mut app, &outcome);
                        reconnect(&mut app, &playback);
                    }
                    Err(error) => app.reduce(AppEvent::PlaybackFailed(format!(
                        "authentication failed: {error}"
                    ))),
                }
            }
        }
    }
}

fn apply_auth_outcome(app: &mut AppState, outcome: &AuthOutcome) {
    if let Some(warning) = outcome.restart_warning() {
        app.reduce(AppEvent::PlaybackFailed(format!(
            "signed in, but spotifyd did not restart: {warning}"
        )));
    } else {
        app.reduce(AppEvent::ConnectionPending);
    }
}

fn report_restart_warning(outcome: &AuthOutcome) {
    if let Some(warning) = outcome.restart_warning() {
        eprintln!("warning: signed in, but spotifyd did not restart: {warning}");
    } else {
        println!("Restarted the spotifyd user service.");
    }
}

fn run_tui_session(
    app: &mut AppState,
    theme: &Theme,
    playback: &PlaybackRuntime,
) -> io::Result<SessionOutcome> {
    let mut terminal = start_terminal()?;
    let result = run(&mut terminal, app, theme, playback);
    let restore_result = restore_terminal(&mut terminal);

    match result {
        Ok(outcome) => {
            restore_result?;
            Ok(outcome)
        }
        Err(error) => Err(error),
    }
}

fn start_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;

    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        disable_raw_mode()?;
        return Err(error);
    }

    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut Tui) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionOutcome {
    Quit,
    Authenticate,
}

fn run(
    terminal: &mut Tui,
    app: &mut AppState,
    theme: &Theme,
    playback: &PlaybackRuntime,
) -> io::Result<SessionOutcome> {
    loop {
        drain_playback_events(app, playback);
        terminal.draw(|frame| ui::render(frame, app, theme))?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && let Some(command) = input::command_for_key(key)
        {
            match command {
                Command::Quit => {
                    app.reduce(AppEvent::QuitRequested);
                    return Ok(SessionOutcome::Quit);
                }
                Command::Authenticate => return Ok(SessionOutcome::Authenticate),
                Command::RetryConnection => reconnect(app, playback),
                Command::TogglePlayback => {
                    dispatch(app, playback, PlaybackCommand::Toggle);
                }
                Command::PreviousTrack => {
                    dispatch(app, playback, PlaybackCommand::Previous);
                }
                Command::NextTrack => {
                    dispatch(app, playback, PlaybackCommand::Next);
                }
                Command::Seek { direction, amount } => {
                    let amount = i64::try_from(amount.as_micros()).unwrap_or(i64::MAX);
                    let offset = match direction {
                        spotify_tui::app::SeekDirection::Backward => -amount,
                        spotify_tui::app::SeekDirection::Forward => amount,
                    };
                    dispatch(app, playback, PlaybackCommand::SeekBy(offset));
                }
                Command::AdjustVolume(amount) => {
                    if let Some(current) = app.playback().map(|state| state.volume()) {
                        dispatch(
                            app,
                            playback,
                            PlaybackCommand::SetVolume((current + amount).clamp(0.0, 1.0)),
                        );
                    }
                }
            }
        }
    }
}

fn drain_playback_events(app: &mut AppState, playback: &PlaybackRuntime) {
    while let Some(event) = playback.try_event() {
        let event = match event {
            PlaybackEvent::Connecting => AppEvent::ConnectionPending,
            PlaybackEvent::Updated(snapshot) => AppEvent::PlaybackUpdated {
                snapshot,
                observed_at: Instant::now(),
            },
            PlaybackEvent::Disconnected => AppEvent::PlaybackDisconnected,
            PlaybackEvent::Failed(message) => AppEvent::PlaybackFailed(message),
        };
        app.reduce(event);
    }
}

fn reconnect(app: &mut AppState, playback: &PlaybackRuntime) {
    app.reduce(AppEvent::ConnectionPending);
    if let Err(error) = playback.reconnect() {
        app.reduce(AppEvent::PlaybackFailed(error.to_string()));
    }
}

fn dispatch(app: &mut AppState, playback: &PlaybackRuntime, command: PlaybackCommand) {
    if let Err(error) = playback.dispatch(command) {
        app.reduce(AppEvent::PlaybackFailed(error.to_string()));
    }
}
