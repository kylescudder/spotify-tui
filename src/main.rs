use std::{
    cell::RefCell,
    env,
    error::Error,
    ffi::OsString,
    io,
    process::ExitCode,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use spotify_tui::{
    app::{AppEvent, AppState, ArtworkState, Command},
    artwork::{ArtworkEvent, ArtworkRenderer, ArtworkRuntime, ArtworkTarget},
    auth::{self, AuthOutcome},
    browser::{BrowserCommand, BrowserEffect, BrowserMode},
    catalog::{CatalogEvent, CatalogRuntime},
    catalog_auth,
    cli::{self, LaunchMode},
    config::{Config, Theme},
    input,
    onboarding::{OnboardingAction, OnboardingState},
    playback::PlaybackCommand,
    playback_runtime::{PlaybackEvent, PlaybackRuntime},
    spotifyd_lifecycle::SpotifydLifecycle,
    ui,
};

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const ARTWORK_POLL_INTERVAL: Duration = Duration::from_millis(16);

fn main() -> ExitCode {
    match cli::parse_args(env::args_os().skip(1)) {
        Ok(LaunchMode::Tui) => report_result(run_tui_app()),
        Ok(LaunchMode::Authenticate { spotifyd_arguments }) => {
            report_result(run_auth_command(&spotifyd_arguments))
        }
        Ok(LaunchMode::CatalogAuthenticate) => report_result(run_catalog_auth_command()),
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

fn run_catalog_auth_command() -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;
    let spotify_api = config.spotify_api().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Spotify catalogue is not configured; add [spotify_api] client_id to config.toml",
        )
    })?;
    println!("Starting Spotify catalogue authentication…");
    let path = catalog_auth::authenticate(spotify_api)?;
    println!(
        "Spotify catalogue authentication complete. Token saved to {}.",
        path.display()
    );
    Ok(())
}

fn run_tui_app() -> Result<(), Box<dyn Error>> {
    let Some(config) = prepare_config()? else {
        return Ok(());
    };
    let mut app = AppState::default();
    let lifecycle = SpotifydLifecycle::from_environment()?;
    let lifecycle_error = lifecycle
        .ensure_running()
        .err()
        .map(|error| error.to_string());
    let playback = PlaybackRuntime::start(config.startup_uri().map(str::to_owned))?;
    let artwork = ArtworkRuntime::start()?;
    let catalog = CatalogRuntime::start(
        config
            .spotify_api()
            .cloned()
            .expect("onboarding guarantees Spotify API configuration"),
    )?;
    let services = RuntimeServices {
        playback: &playback,
        artwork: &artwork,
        catalog: &catalog,
        lifecycle: &lifecycle,
        lifecycle_error: RefCell::new(lifecycle_error),
    };

    loop {
        match run_tui_session(&mut app, config.theme(), &services)? {
            SessionOutcome::Quit => return Ok(()),
            SessionOutcome::Authenticate => {
                println!("Starting Spotifyd authentication…");
                match auth::authenticate(&[]) {
                    Ok(outcome) => {
                        apply_auth_outcome(&mut app, &outcome);
                        reconnect(&mut app, &services);
                    }
                    Err(error) => app.reduce(AppEvent::PlaybackFailed(format!(
                        "authentication failed: {error}"
                    ))),
                }
            }
        }
    }
}

fn prepare_config() -> Result<Option<Config>, Box<dyn Error>> {
    let mut config = Config::load()?;
    let config_path = Config::config_path()?;
    let mut configuration_error = None;
    let mut authentication_error = None;
    let mut editing_client_id = false;

    loop {
        if config.spotify_api().is_none() || editing_client_id {
            let mut state = config.spotify_api().map_or_else(
                || OnboardingState::configure(config_path.clone()),
                |spotify_api| {
                    OnboardingState::edit_client_id(
                        config_path.clone(),
                        spotify_api.client_id().to_owned(),
                        spotify_api.redirect_uri().to_owned(),
                    )
                },
            );
            if let Some(error) = configuration_error.take() {
                state.set_error(error);
            }
            match run_onboarding_session(state, config.theme())? {
                OnboardingAction::SaveClientId(client_id) => {
                    match Config::save_spotify_api_client_id(&client_id) {
                        Ok(_) => {
                            config = Config::load()?;
                            editing_client_id = false;
                            authentication_error = None;
                        }
                        Err(error) => configuration_error = Some(error.to_string()),
                    }
                }
                OnboardingAction::Quit => return Ok(None),
                OnboardingAction::None
                | OnboardingAction::Authenticate
                | OnboardingAction::EditClientId => {}
            }
            continue;
        }

        let spotify_api = config
            .spotify_api()
            .expect("Spotify API configuration was checked above");
        if catalog_auth::is_authenticated(spotify_api)? {
            return Ok(Some(config));
        }

        let state = OnboardingState::authorize(
            config_path.clone(),
            spotify_api.client_id().to_owned(),
            spotify_api.redirect_uri().to_owned(),
            authentication_error.take(),
        );
        match run_onboarding_session(state, config.theme())? {
            OnboardingAction::Authenticate => {
                println!("Opening Spotify authorization in your browser…");
                match catalog_auth::authenticate(spotify_api) {
                    Ok(path) => {
                        println!("Spotify authorization saved to {}.", path.display());
                    }
                    Err(error) => authentication_error = Some(error.to_string()),
                }
            }
            OnboardingAction::EditClientId => editing_client_id = true,
            OnboardingAction::Quit => return Ok(None),
            OnboardingAction::None | OnboardingAction::SaveClientId(_) => {}
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
        println!("Spotifyd is running with the new authentication.");
    }
}

struct RuntimeServices<'a> {
    playback: &'a PlaybackRuntime,
    artwork: &'a ArtworkRuntime,
    catalog: &'a CatalogRuntime,
    lifecycle: &'a SpotifydLifecycle,
    lifecycle_error: RefCell<Option<String>>,
}

fn run_tui_session(
    app: &mut AppState,
    theme: &Theme,
    services: &RuntimeServices<'_>,
) -> io::Result<SessionOutcome> {
    let mut terminal = start_terminal()?;
    let mut artwork_renderer = match ArtworkRenderer::detect(theme) {
        Ok(renderer) => renderer,
        Err(error) => {
            let _ = restore_terminal(&mut terminal);
            return Err(error);
        }
    };
    let result = run(&mut terminal, app, theme, services, &mut artwork_renderer);
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
    if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen);
        return Err(error);
    }

    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let _ = disable_raw_mode();
            let mut stdout = io::stdout();
            let _ = execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen);
            Err(error)
        }
    }
}

fn restore_terminal(terminal: &mut Tui) -> io::Result<()> {
    let raw_mode_result = disable_raw_mode();
    let screen_result = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let cursor_result = terminal.show_cursor();

    raw_mode_result?;
    screen_result?;
    cursor_result
}

fn run_onboarding_session(
    mut state: OnboardingState,
    theme: &Theme,
) -> io::Result<OnboardingAction> {
    let mut terminal = start_terminal()?;
    let result = drive_onboarding_session(&mut terminal, &mut state, theme);
    let restore_result = restore_terminal(&mut terminal);

    match result {
        Ok(action) => {
            restore_result?;
            Ok(action)
        }
        Err(error) => {
            let _ = restore_result;
            Err(error)
        }
    }
}

fn drive_onboarding_session(
    terminal: &mut Tui,
    state: &mut OnboardingState,
    theme: &Theme,
) -> io::Result<OnboardingAction> {
    loop {
        terminal.draw(|frame| ui::render_onboarding(frame, state, theme))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let action = state.handle_key(key);
                if action != OnboardingAction::None {
                    return Ok(action);
                }
            }
            Event::Paste(text) => state.paste(&text),
            _ => {}
        }
    }
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
    services: &RuntimeServices<'_>,
    artwork_renderer: &mut ArtworkRenderer,
) -> io::Result<SessionOutcome> {
    loop {
        drain_playback_events(app, services);
        drain_catalog_events(app, services);
        sync_catalog_artwork(app, services.artwork);
        drain_artwork_events(app, services.artwork);
        if app.browser().mode() == BrowserMode::Closed {
            artwork_renderer.sync(app.track_revision(), app.artwork());
        } else {
            artwork_renderer.sync(app.catalog_revision(), app.catalog_artwork());
        }
        terminal.draw(|frame| ui::render(frame, app, theme, artwork_renderer))?;

        if !event::poll(input_poll_interval(app, artwork_renderer))? {
            continue;
        }
        let input_event = event::read()?;
        if let Event::Paste(text) = input_event {
            app.browser_mut().apply(BrowserCommand::Paste(text));
            continue;
        }
        let Event::Key(key) = input_event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        let browser_mode = app.browser().mode();
        if let Some(command) = input::browser_command_for_key(key, browser_mode) {
            match app.browser_mut().apply(command) {
                BrowserEffect::None => {}
                BrowserEffect::Quit => {
                    app.reduce(AppEvent::QuitRequested);
                    return Ok(SessionOutcome::Quit);
                }
                BrowserEffect::Play(uri) => {
                    dispatch(app, services.playback, PlaybackCommand::OpenUri(uri));
                }
                BrowserEffect::PlayTrack(target) => {
                    if let Err(error) = services.catalog.play(target) {
                        app.reduce(AppEvent::PlaybackFailed(error.to_string()));
                    }
                }
                BrowserEffect::Fetch {
                    request_id,
                    request,
                } => {
                    if let Err(error) = services.catalog.fetch(request_id, request) {
                        app.browser_mut().resolve(CatalogEvent::Failed {
                            request_id,
                            message: error.to_string(),
                        });
                    }
                }
            }
            continue;
        }

        if browser_mode != BrowserMode::Closed {
            continue;
        }

        let Some(command) = input::command_for_key(key) else {
            continue;
        };
        match command {
            Command::Quit => {
                app.reduce(AppEvent::QuitRequested);
                return Ok(SessionOutcome::Quit);
            }
            Command::Authenticate => return Ok(SessionOutcome::Authenticate),
            Command::RetryConnection => reconnect(app, services),
            Command::TogglePlayback => {
                dispatch(app, services.playback, PlaybackCommand::Toggle);
            }
            Command::PreviousTrack => {
                dispatch(app, services.playback, PlaybackCommand::Previous);
            }
            Command::NextTrack => {
                dispatch(app, services.playback, PlaybackCommand::Next);
            }
            Command::Seek { direction, amount } => {
                let amount = i64::try_from(amount.as_micros()).unwrap_or(i64::MAX);
                let offset = match direction {
                    spotify_tui::app::SeekDirection::Backward => -amount,
                    spotify_tui::app::SeekDirection::Forward => amount,
                };
                dispatch(app, services.playback, PlaybackCommand::SeekBy(offset));
            }
            Command::AdjustVolume(amount) => {
                if let Some(current) = app.playback().map(|state| state.volume()) {
                    dispatch(
                        app,
                        services.playback,
                        PlaybackCommand::SetVolume((current + amount).clamp(0.0, 1.0)),
                    );
                }
            }
        }
    }
}

fn input_poll_interval(app: &AppState, artwork_renderer: &ArtworkRenderer) -> Duration {
    let visible_artwork = if app.browser().mode() == BrowserMode::Closed {
        app.artwork()
    } else {
        app.catalog_artwork()
    };
    if matches!(visible_artwork, ArtworkState::Loading) || artwork_renderer.is_pending() {
        ARTWORK_POLL_INTERVAL
    } else {
        IDLE_POLL_INTERVAL
    }
}

fn drain_playback_events(app: &mut AppState, services: &RuntimeServices<'_>) {
    while let Some(event) = services.playback.try_event() {
        match event {
            PlaybackEvent::Connecting => app.reduce(AppEvent::ConnectionPending),
            PlaybackEvent::Updated(snapshot) => {
                services.lifecycle_error.borrow_mut().take();
                let previous_revision = app.track_revision();
                let art_url = snapshot.track.art_url.clone();
                app.reduce(AppEvent::PlaybackUpdated {
                    snapshot,
                    observed_at: Instant::now(),
                });
                if app.track_revision() != previous_revision
                    && let Some(url) = art_url
                    && let Err(error) = services
                        .artwork
                        .load(ArtworkTarget::Playback(app.track_revision()), url)
                {
                    app.reduce(AppEvent::ArtworkFailed {
                        track_revision: app.track_revision(),
                        message: error.to_string(),
                    });
                }
            }
            PlaybackEvent::Disconnected => {
                if let Some(message) = services.lifecycle_error.borrow().clone() {
                    app.reduce(AppEvent::PlaybackFailed(message));
                } else {
                    app.reduce(AppEvent::PlaybackDisconnected);
                }
            }
            PlaybackEvent::Failed(message) => app.reduce(AppEvent::PlaybackFailed(message)),
        }
    }
}

fn drain_artwork_events(app: &mut AppState, artwork: &ArtworkRuntime) {
    while let Some(event) = artwork.try_event() {
        app.reduce(match event {
            ArtworkEvent::Loaded {
                target: ArtworkTarget::Playback(track_revision),
                artwork,
            } => AppEvent::ArtworkLoaded {
                track_revision,
                artwork,
            },
            ArtworkEvent::Failed {
                target: ArtworkTarget::Playback(track_revision),
                message,
            } => AppEvent::ArtworkFailed {
                track_revision,
                message,
            },
            ArtworkEvent::Loaded {
                target: ArtworkTarget::Catalog(catalog_revision),
                artwork,
            } => AppEvent::CatalogArtworkLoaded {
                catalog_revision,
                artwork,
            },
            ArtworkEvent::Failed {
                target: ArtworkTarget::Catalog(catalog_revision),
                message,
            } => AppEvent::CatalogArtworkFailed {
                catalog_revision,
                message,
            },
        });
    }
}

fn sync_catalog_artwork(app: &mut AppState, artwork: &ArtworkRuntime) {
    let url = app.browser().artwork_url().map(str::to_owned);
    let prefetch_urls = app
        .browser()
        .prefetch_artwork_urls()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some((catalog_revision, url)) = app.sync_catalog_artwork(url.as_deref())
        && let Err(error) = artwork.load(ArtworkTarget::Catalog(catalog_revision), url)
    {
        app.reduce(AppEvent::CatalogArtworkFailed {
            catalog_revision,
            message: error.to_string(),
        });
    }
    let _ = artwork.prefetch(prefetch_urls);
}

fn drain_catalog_events(app: &mut AppState, services: &RuntimeServices<'_>) {
    while let Some(event) = services.catalog.try_event() {
        match event {
            CatalogEvent::PlaybackFailed { message } => {
                app.reduce(AppEvent::PlaybackFailed(message));
            }
            CatalogEvent::PlaybackFallback { playback } => {
                dispatch(
                    app,
                    services.playback,
                    PlaybackCommand::OpenUri(playback.uri().to_owned()),
                );
            }
            event @ (CatalogEvent::Loaded { .. } | CatalogEvent::Failed { .. }) => {
                app.browser_mut().resolve(event);
            }
        }
    }
}

fn reconnect(app: &mut AppState, services: &RuntimeServices<'_>) {
    app.reduce(AppEvent::ConnectionPending);
    match services.lifecycle.ensure_running() {
        Ok(()) => {
            services.lifecycle_error.borrow_mut().take();
        }
        Err(error) => {
            let message = error.to_string();
            services.lifecycle_error.replace(Some(message.clone()));
            app.reduce(AppEvent::PlaybackFailed(message));
            return;
        }
    }
    if let Err(error) = services.playback.reconnect() {
        app.reduce(AppEvent::PlaybackFailed(error.to_string()));
    }
}

fn dispatch(app: &mut AppState, playback: &PlaybackRuntime, command: PlaybackCommand) {
    if let Err(error) = playback.dispatch(command) {
        app.reduce(AppEvent::PlaybackFailed(error.to_string()));
    }
}
