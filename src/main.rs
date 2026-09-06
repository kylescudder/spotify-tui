use std::{env, error::Error, ffi::OsString, io, process::ExitCode, time::Duration};

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
    input, ui,
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

    loop {
        match run_tui_session(&mut app, config.theme())? {
            SessionOutcome::Quit => return Ok(()),
            SessionOutcome::Authenticate => {
                println!("Starting Spotifyd authentication…");
                match auth::authenticate(&[]) {
                    Ok(outcome) => apply_auth_outcome(&mut app, &outcome),
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

fn run_tui_session(app: &mut AppState, theme: &Theme) -> io::Result<SessionOutcome> {
    let mut terminal = start_terminal()?;
    let result = run(&mut terminal, app, theme);
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

fn run(terminal: &mut Tui, app: &mut AppState, theme: &Theme) -> io::Result<SessionOutcome> {
    loop {
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
                Command::RetryConnection => app.reduce(AppEvent::ConnectionPending),
                Command::TogglePlayback
                | Command::PreviousTrack
                | Command::NextTrack
                | Command::Seek { .. }
                | Command::SetVolume(_) => {}
            }
        }
    }
}
