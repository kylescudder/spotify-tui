use std::{
    env,
    ffi::{OsStr, OsString},
    fmt, io,
    process::{Command, Stdio},
};

use thiserror::Error;

pub const SPOTIFYD_PROGRAM_ENV: &str = "SPOTIFY_TUI_SPOTIFYD";
pub const SPOTIFYD_CONFIG_ENV: &str = "SPOTIFY_TUI_SPOTIFYD_CONFIG";
pub const SPOTIFYD_SERVICE_ENV: &str = "SPOTIFY_TUI_SPOTIFYD_SERVICE";
pub const SYSTEMCTL_PROGRAM_ENV: &str = "SPOTIFY_TUI_SYSTEMCTL";
pub const SYSTEMD_RUN_PROGRAM_ENV: &str = "SPOTIFY_TUI_SYSTEMD_RUN";
pub const LAUNCHCTL_PROGRAM_ENV: &str = "SPOTIFY_TUI_LAUNCHCTL";
pub const BREW_PROGRAM_ENV: &str = "SPOTIFY_TUI_BREW";

#[cfg(any(target_os = "linux", test))]
const TRANSIENT_LINUX_SERVICE: &str = "spotify-tui-spotifyd.service";
#[cfg(any(target_os = "linux", test))]
const DEFAULT_LINUX_SERVICE: &str = "spotifyd.service";
#[cfg(target_os = "macos")]
const DEFAULT_MACOS_SERVICE: &str = "io.github.kylescudder.spotifyd";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleAction {
    Start,
    Restart,
}

impl LifecycleAction {
    const fn systemd_verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Restart => "restart",
        }
    }

    #[cfg(any(target_os = "macos", test))]
    const fn brew_verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Restart => "restart",
        }
    }
}

impl fmt::Display for LifecycleAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Start => "start",
            Self::Restart => "restart",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandSpec {
    program: OsString,
    arguments: Vec<OsString>,
}

impl CommandSpec {
    fn new(program: impl Into<OsString>, arguments: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            program: program.into(),
            arguments: arguments.into_iter().collect(),
        }
    }

    fn checked(&self) -> Result<(), String> {
        let output = Command::new(&self.program)
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("{}: {error}", self.display()))?;
        if output.status.success() {
            return Ok(());
        }

        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if detail.is_empty() {
            Err(format!("{} exited with {}", self.display(), output.status))
        } else {
            Err(format!("{}: {detail}", self.display()))
        }
    }

    fn spawn_detached(&self) -> Result<(), String> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_detached_process(&mut command);
        command
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("{}: {error}", self.display()))
    }

    fn display(&self) -> String {
        let mut command = self.program.to_string_lossy().into_owned();
        for argument in &self.arguments {
            command.push(' ');
            command.push_str(&argument.to_string_lossy());
        }
        command
    }
}

#[cfg(windows)]
fn configure_detached_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
}

#[cfg(not(windows))]
fn configure_detached_process(_command: &mut Command) {}

#[derive(Debug, Clone)]
struct LifecycleConfig {
    spotifyd_program: OsString,
    spotifyd_config: Option<OsString>,
    #[cfg(any(target_os = "linux", target_os = "macos", test))]
    service: OsString,
    #[cfg(any(target_os = "linux", test))]
    systemctl_program: OsString,
    #[cfg(any(target_os = "linux", test))]
    systemd_run_program: OsString,
    #[cfg(target_os = "macos")]
    launchctl_program: OsString,
    #[cfg(target_os = "macos")]
    brew_program: OsString,
}

impl LifecycleConfig {
    fn from_environment() -> Result<Self, SpotifydLifecycleError> {
        Ok(Self {
            spotifyd_program: environment_value(SPOTIFYD_PROGRAM_ENV)?
                .unwrap_or_else(|| OsString::from("spotifyd")),
            spotifyd_config: environment_value(SPOTIFYD_CONFIG_ENV)?,
            #[cfg(any(target_os = "linux", target_os = "macos", test))]
            service: environment_value(SPOTIFYD_SERVICE_ENV)?.unwrap_or_else(default_service),
            #[cfg(any(target_os = "linux", test))]
            systemctl_program: environment_value(SYSTEMCTL_PROGRAM_ENV)?
                .unwrap_or_else(|| OsString::from("systemctl")),
            #[cfg(any(target_os = "linux", test))]
            systemd_run_program: environment_value(SYSTEMD_RUN_PROGRAM_ENV)?
                .unwrap_or_else(|| OsString::from("systemd-run")),
            #[cfg(target_os = "macos")]
            launchctl_program: environment_value(LAUNCHCTL_PROGRAM_ENV)?
                .unwrap_or_else(|| OsString::from("launchctl")),
            #[cfg(target_os = "macos")]
            brew_program: environment_value(BREW_PROGRAM_ENV)?
                .unwrap_or_else(|| OsString::from("brew")),
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn default_service() -> OsString {
    #[cfg(target_os = "macos")]
    {
        return OsString::from(DEFAULT_MACOS_SERVICE);
    }
    #[cfg(not(target_os = "macos"))]
    {
        OsString::from(DEFAULT_LINUX_SERVICE)
    }
}

fn environment_value(name: &'static str) -> Result<Option<OsString>, SpotifydLifecycleError> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(SpotifydLifecycleError::EmptyEnvironmentValue(name));
    }
    Ok(Some(value))
}

#[derive(Debug, Clone)]
pub struct SpotifydLifecycle {
    config: LifecycleConfig,
}

impl SpotifydLifecycle {
    pub fn from_environment() -> Result<Self, SpotifydLifecycleError> {
        Ok(Self {
            config: LifecycleConfig::from_environment()?,
        })
    }

    pub fn ensure_running(&self) -> Result<(), SpotifydLifecycleError> {
        self.manage(LifecycleAction::Start)
    }

    pub fn restart(&self) -> Result<(), SpotifydLifecycleError> {
        self.manage(LifecycleAction::Restart)
    }

    fn manage(&self, action: LifecycleAction) -> Result<(), SpotifydLifecycleError> {
        #[cfg(target_os = "linux")]
        {
            return self.manage_linux(action);
        }
        #[cfg(target_os = "macos")]
        {
            return self.manage_macos(action);
        }
        #[cfg(windows)]
        {
            return self.manage_windows(action);
        }
        #[allow(unreachable_code)]
        Err(SpotifydLifecycleError::UnsupportedPlatform(
            env::consts::OS.to_owned(),
        ))
    }

    #[cfg(target_os = "linux")]
    fn manage_linux(&self, action: LifecycleAction) -> Result<(), SpotifydLifecycleError> {
        let mut failures = Vec::new();
        for service in [&self.config.service, OsStr::new(TRANSIENT_LINUX_SERVICE)] {
            let command = systemd_service_command(&self.config.systemctl_program, action, service);
            match command.checked() {
                Ok(()) => return Ok(()),
                Err(error) => failures.push(error),
            }
        }

        let transient = linux_transient_command(&self.config);
        match transient.checked() {
            Ok(()) => return Ok(()),
            Err(error) => failures.push(error),
        }

        if action == LifecycleAction::Restart {
            let _ = CommandSpec::new("pkill", [OsString::from("-x"), OsString::from("spotifyd")])
                .checked();
        }
        self.spawn_fallback(action, failures)
    }

    #[cfg(target_os = "macos")]
    fn manage_macos(&self, action: LifecycleAction) -> Result<(), SpotifydLifecycleError> {
        let mut failures = Vec::new();
        match macos_launchctl_command(&self.config, action).and_then(|command| command.checked()) {
            Ok(()) => return Ok(()),
            Err(error) => failures.push(error),
        }
        let brew = brew_service_command(&self.config.brew_program, action);
        match brew.checked() {
            Ok(()) => return Ok(()),
            Err(error) => failures.push(error),
        }
        if action == LifecycleAction::Restart {
            let _ = CommandSpec::new("pkill", [OsString::from("-x"), OsString::from("spotifyd")])
                .checked();
        }
        self.spawn_fallback(action, failures)
    }

    #[cfg(windows)]
    fn manage_windows(&self, action: LifecycleAction) -> Result<(), SpotifydLifecycleError> {
        if action == LifecycleAction::Start {
            let tasklist = CommandSpec::new(
                "tasklist",
                [
                    OsString::from("/FI"),
                    OsString::from("IMAGENAME eq spotifyd.exe"),
                    OsString::from("/NH"),
                ],
            );
            if command_output_contains(&tasklist, "spotifyd.exe") {
                return Ok(());
            }
        } else {
            let _ = CommandSpec::new(
                "taskkill",
                [
                    OsString::from("/IM"),
                    OsString::from("spotifyd.exe"),
                    OsString::from("/F"),
                ],
            )
            .checked();
        }
        self.spawn_fallback(action, Vec::new())
    }

    fn spawn_fallback(
        &self,
        action: LifecycleAction,
        mut failures: Vec<String>,
    ) -> Result<(), SpotifydLifecycleError> {
        let command = spotifyd_command(&self.config);
        match command.spawn_detached() {
            Ok(()) => Ok(()),
            Err(error) => {
                failures.push(error);
                Err(SpotifydLifecycleError::AutomaticLifecycle {
                    action: action.systemd_verb(),
                    detail: failures.join("; "),
                })
            }
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn systemd_service_command(
    program: &OsStr,
    action: LifecycleAction,
    service: &OsStr,
) -> CommandSpec {
    CommandSpec::new(
        program,
        [
            OsString::from("--user"),
            OsString::from(action.systemd_verb()),
            service.to_owned(),
        ],
    )
}

#[cfg(any(target_os = "linux", test))]
fn linux_transient_command(config: &LifecycleConfig) -> CommandSpec {
    let mut arguments = vec![
        OsString::from("--user"),
        OsString::from(format!("--unit={TRANSIENT_LINUX_SERVICE}")),
        OsString::from("--collect"),
        OsString::from("--property=Restart=on-failure"),
        OsString::from("--property=RestartSec=5"),
        OsString::from("--"),
    ];
    arguments.push(config.spotifyd_program.clone());
    arguments.extend(spotifyd_arguments(config));
    CommandSpec::new(&config.systemd_run_program, arguments)
}

#[cfg(target_os = "macos")]
fn macos_launchctl_command(
    config: &LifecycleConfig,
    action: LifecycleAction,
) -> Result<CommandSpec, String> {
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|error| format!("id -u: {error}"))?;
    if !uid.status.success() {
        return Err(format!("id -u exited with {}", uid.status));
    }
    let uid = String::from_utf8_lossy(&uid.stdout).trim().to_owned();
    let mut arguments = vec![OsString::from("kickstart")];
    if action == LifecycleAction::Restart {
        arguments.push(OsString::from("-k"));
    }
    arguments.push(OsString::from(format!(
        "gui/{uid}/{}",
        config.service.to_string_lossy()
    )));
    Ok(CommandSpec::new(&config.launchctl_program, arguments))
}

#[cfg(any(target_os = "macos", test))]
fn brew_service_command(program: &OsStr, action: LifecycleAction) -> CommandSpec {
    CommandSpec::new(
        program,
        [
            OsString::from("services"),
            OsString::from(action.brew_verb()),
            OsString::from("spotify-tui"),
        ],
    )
}

fn spotifyd_command(config: &LifecycleConfig) -> CommandSpec {
    CommandSpec::new(&config.spotifyd_program, spotifyd_arguments(config))
}

fn spotifyd_arguments(config: &LifecycleConfig) -> Vec<OsString> {
    let mut arguments = Vec::with_capacity(3);
    if let Some(path) = &config.spotifyd_config {
        arguments.push(OsString::from("--config-path"));
        arguments.push(path.clone());
    }
    arguments.push(OsString::from("--no-daemon"));
    arguments
}

#[cfg(windows)]
fn command_output_contains(command: &CommandSpec, needle: &str) -> bool {
    Command::new(&command.program)
        .args(&command.arguments)
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
        })
}

#[derive(Debug, Error)]
pub enum SpotifydLifecycleError {
    #[error("{0} is set but empty")]
    EmptyEnvironmentValue(&'static str),
    #[error("could not {action} Spotifyd automatically: {detail}")]
    AutomaticLifecycle {
        action: &'static str,
        detail: String,
    },
    #[error("automatic Spotifyd lifecycle is not supported on {0}")]
    UnsupportedPlatform(String),
}

impl From<SpotifydLifecycleError> for io::Error {
    fn from(error: SpotifydLifecycleError) -> Self {
        Self::other(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> LifecycleConfig {
        LifecycleConfig {
            spotifyd_program: OsString::from("/opt/spotify-tui/spotifyd"),
            spotifyd_config: Some(OsString::from("/tmp/spotifyd.conf")),
            service: OsString::from("spotifyd.service"),
            systemctl_program: OsString::from("systemctl"),
            systemd_run_program: OsString::from("systemd-run"),
            #[cfg(target_os = "macos")]
            launchctl_program: OsString::from("launchctl"),
            #[cfg(target_os = "macos")]
            brew_program: OsString::from("brew"),
        }
    }

    #[test]
    fn linux_service_start_is_idempotent() {
        assert_eq!(
            systemd_service_command(
                OsStr::new("systemctl"),
                LifecycleAction::Start,
                OsStr::new("spotifyd.service")
            ),
            CommandSpec::new(
                "systemctl",
                [
                    OsString::from("--user"),
                    OsString::from("start"),
                    OsString::from("spotifyd.service")
                ]
            )
        );
    }

    #[test]
    fn raw_nix_fallback_runs_spotifyd_as_a_transient_user_service() {
        assert_eq!(
            linux_transient_command(&config()),
            CommandSpec::new(
                "systemd-run",
                [
                    OsString::from("--user"),
                    OsString::from("--unit=spotify-tui-spotifyd.service"),
                    OsString::from("--collect"),
                    OsString::from("--property=Restart=on-failure"),
                    OsString::from("--property=RestartSec=5"),
                    OsString::from("--"),
                    OsString::from("/opt/spotify-tui/spotifyd"),
                    OsString::from("--config-path"),
                    OsString::from("/tmp/spotifyd.conf"),
                    OsString::from("--no-daemon")
                ]
            )
        );
    }

    #[test]
    fn fallback_process_uses_the_configured_spotifyd() {
        assert_eq!(
            spotifyd_command(&config()),
            CommandSpec::new(
                "/opt/spotify-tui/spotifyd",
                [
                    OsString::from("--config-path"),
                    OsString::from("/tmp/spotifyd.conf"),
                    OsString::from("--no-daemon")
                ]
            )
        );
    }

    #[test]
    fn homebrew_lifecycle_uses_the_formula_service() {
        assert_eq!(
            brew_service_command(OsStr::new("brew"), LifecycleAction::Start),
            CommandSpec::new(
                "brew",
                [
                    OsString::from("services"),
                    OsString::from("start"),
                    OsString::from("spotify-tui")
                ]
            )
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lifecycle_starts_the_installed_user_service() {
        use std::{
            fs,
            os::unix::fs::PermissionsExt,
            sync::atomic::{AtomicU64, Ordering},
        };

        static NEXT_TEST: AtomicU64 = AtomicU64::new(0);
        let root = env::temp_dir().join(format!(
            "spotify-tui-lifecycle-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("test directory should be created");
        let calls = root.join("calls");
        let systemctl = root.join("systemctl");
        fs::write(
            &systemctl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                calls.display()
            ),
        )
        .expect("fake systemctl should be written");
        let mut permissions = fs::metadata(&systemctl)
            .expect("fake systemctl should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&systemctl, permissions).expect("fake systemctl should be executable");

        let mut lifecycle_config = config();
        lifecycle_config.systemctl_program = systemctl.into_os_string();
        SpotifydLifecycle {
            config: lifecycle_config,
        }
        .ensure_running()
        .expect("installed user service should start");

        assert_eq!(
            fs::read_to_string(&calls).expect("service call should be recorded"),
            "--user start spotifyd.service\n"
        );
        fs::remove_dir_all(root).expect("test directory should be removed");
    }
}
