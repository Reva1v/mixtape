//! «Start with Windows»: the per-user `Run` entry (`mixtape_win::autostart`) pointing at
//! this executable. The registry is the only record (not `Prefs`), so the setting always
//! shows what Windows will really do, including a switch-off in Task Manager.

use mixtape_core::{AppError, ErrorKind, trf};
use mixtape_win::autostart;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Name of the value under the `Run` key.
const VALUE_NAME: &str = "Mixtape";
/// Marks a launch by Windows at sign-in.
pub const ARG_AUTOSTART: &str = "--autostart";
/// With [`ARG_AUTOSTART`]: stay in the tray, do not show the window.
pub const ARG_HIDDEN: &str = "--hidden";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Autostart {
    #[default]
    Off,
    /// Start with Windows and show the window.
    On,
    /// Start with Windows, only the tray icon.
    Hidden,
}

impl Autostart {
    fn args(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Off => None,
            Self::On => Some(&[ARG_AUTOSTART]),
            Self::Hidden => Some(&[ARG_AUTOSTART, ARG_HIDDEN]),
        }
    }

    /// The mode a `Run` command line stands for (`None` = no entry).
    fn from_command(cmd: Option<&str>) -> Self {
        match cmd {
            None => Self::Off,
            Some(c) if autostart::command_args(c).contains(&ARG_HIDDEN) => Self::Hidden,
            Some(_) => Self::On,
        }
    }

    fn command(self, exe: &std::path::Path) -> Option<String> {
        self.args().map(|args| autostart::command_line(exe, args))
    }
}

/// What Windows does at sign-in right now; `Off` when the registry cannot be read.
pub fn current() -> Autostart {
    match autostart::get(VALUE_NAME) {
        Ok(cmd) => Autostart::from_command(cmd.as_deref()),
        Err(e) => {
            tracing::warn!(error = %e, "autostart entry not read");
            Autostart::Off
        }
    }
}

/// Writes the entry for `mode` with this executable's path.
pub fn set(mode: Autostart) -> Result<(), AppError> {
    let fail = |e: &dyn std::fmt::Display| {
        AppError::new(
            ErrorKind::Internal,
            trf!(
                "Couldn't change start with Windows: {e}",
                "Не вдалося змінити автозапуск: {e}",
                "Не удалось изменить автозапуск: {e}"
            ),
        )
    };
    let exe = std::env::current_exe().map_err(|e| fail(&e))?;
    autostart::set(VALUE_NAME, mode.command(&exe).as_deref()).map_err(|e| fail(&e))?;
    tracing::info!(?mode, "autostart set");
    Ok(())
}

/// Points an existing entry at this executable if the app has moved (release builds only:
/// a dev build must not take over the installed app's entry).
pub fn refresh() {
    if cfg!(debug_assertions) {
        return;
    }
    let Ok(Some(cmd)) = autostart::get(VALUE_NAME) else {
        return;
    };
    let mode = Autostart::from_command(Some(&cmd));
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if mode.command(&exe).is_some_and(|want| want != cmd)
        && let Err(e) = set(mode)
    {
        tracing::warn!(error = %e, "autostart entry not refreshed");
    }
}

#[tauri::command]
pub async fn autostart_get() -> Autostart {
    current()
}

/// Sets the mode and answers with what the registry says now.
#[tauri::command]
pub async fn autostart_set(mode: Autostart) -> Result<Autostart, AppError> {
    set(mode)?;
    Ok(current())
}

/// This process was started at sign-in in the tray-only mode.
pub fn launched_hidden() -> bool {
    is_hidden_launch(std::env::args().skip(1))
}

fn is_hidden_launch(args: impl Iterator<Item = String>) -> bool {
    let args: Vec<String> = args.collect();
    args.iter().any(|a| a == ARG_AUTOSTART) && args.iter().any(|a| a == ARG_HIDDEN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn modes_round_trip_through_the_command_line() {
        let exe = Path::new(r"C:\Users\me\AppData\Local\Mixtape\mixtape.exe");
        assert_eq!(Autostart::Off.command(exe), None);
        for mode in [Autostart::On, Autostart::Hidden] {
            let cmd = mode.command(exe).unwrap();
            assert!(
                cmd.starts_with(r#""C:\Users\me\AppData\Local\Mixtape\mixtape.exe" --autostart"#)
            );
            assert_eq!(Autostart::from_command(Some(&cmd)), mode);
        }
        assert_eq!(Autostart::from_command(None), Autostart::Off);
        // An entry written by hand without our flags still starts the app.
        assert_eq!(
            Autostart::from_command(Some(r"C:\Mixtape\mixtape.exe")),
            Autostart::On
        );
    }

    #[test]
    fn hidden_launch_needs_both_flags() {
        let args = |a: &[&str]| {
            a.iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
        };
        assert!(is_hidden_launch(args(&["--autostart", "--hidden"])));
        assert!(!is_hidden_launch(args(&["--autostart"])));
        assert!(!is_hidden_launch(args(&["--hidden"])));
        assert!(!is_hidden_launch(args(&[])));
    }
}
