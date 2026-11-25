//! `mixtape://` links on the command line: a browser extension or the OS protocol handler starts
//! `mixtape-player.exe "mixtape://…"`, and a second launch hands its arguments to the running app.

/// The arguments that are links: those starting with `mixtape:` in any case. Everything else
/// (`--autostart`, `--hidden`, a stray path) is not a link and behaves as before.
pub fn links_in(args: impl Iterator<Item = String>) -> Vec<String> {
    args.filter(|a| {
        a.get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("mixtape:"))
    })
    .collect()
}

/// Links of a second launch that came before the session was up.
static EARLY: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Hands the links to the session (they wait there for the interface); before the session
/// exists they wait here.
pub fn hand_over_all(app: &tauri::AppHandle, links: Vec<String>) {
    use tauri::Manager;
    match app.try_state::<std::sync::Arc<mixtape_session::Session>>() {
        Some(st) => {
            for l in links {
                mixtape_session::commands::link::hand_over(st.inner(), l);
            }
        }
        None => EARLY.lock().expect("early links poisoned").extend(links),
    }
}

/// From `setup`, once the session is up: the links that arrived before.
pub fn hand_over_early(st: &std::sync::Arc<mixtape_session::Session>) {
    let early = std::mem::take(&mut *EARLY.lock().expect("early links poisoned"));
    for l in early {
        mixtape_session::commands::link::hand_over(st, l);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(args: &[&str]) -> Vec<String> {
        links_in(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn no_arguments_no_links() {
        assert!(links(&[]).is_empty());
    }

    #[test]
    fn one_link() {
        assert_eq!(links(&["mixtape://open?url=x"]), ["mixtape://open?url=x"]);
    }

    #[test]
    fn hidden_start_is_not_a_link() {
        assert!(links(&["--autostart", "--hidden"]).is_empty());
        assert_eq!(
            links(&["--autostart", "--hidden", "mixtape://play?ids=a"]),
            ["mixtape://play?ids=a"]
        );
    }

    #[test]
    fn scheme_is_case_insensitive() {
        assert_eq!(
            links(&["MIXTAPE://open?url=x", "Mixtape:open"]),
            ["MIXTAPE://open?url=x", "Mixtape:open"]
        );
    }

    #[test]
    fn paths_and_look_alikes_are_not_links() {
        assert!(
            links(&[
                "C:\\Music\\mixtape://x.mp3",
                "mixtapes://x",
                "mixtape",
                "",
                "é",
                "mixtap\u{e9}:x"
            ])
            .is_empty()
        );
    }

    #[test]
    fn several_links_keep_their_order() {
        assert_eq!(
            links(&["mixtape://a", "--x", "mixtape://b"]),
            ["mixtape://a", "mixtape://b"]
        );
    }
}
