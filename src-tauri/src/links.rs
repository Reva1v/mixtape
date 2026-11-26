//! `mixtape://` links on the command line: a browser extension or the OS protocol handler starts
//! `mixtape-player.exe "mixtape://…"`, and a second launch hands its arguments to the running app.
//! When Droplet Island runs, the links go to it instead (spec 005).

/// The island takes links with this flag followed by the link; Mixtape accepts the same form.
pub const LINK_FLAG: &str = "--mixtape-link";
/// The island sends a link back with this flag when its music module is off: never hand it back.
pub const FROM_ISLAND: &str = "--from-island";
/// The island's program, looked for in this Windows session.
pub const ISLAND_EXE: &str = "droplet-island.exe";
/// Longest link handed to the island (the island refuses longer ones too).
pub const MAX_LINK: usize = 4096;

fn is_link(a: &str) -> bool {
    a.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("mixtape:"))
}

/// What a start's arguments carry: its links in order, and whether they came from the island.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Start {
    pub links: Vec<String>,
    pub from_island: bool,
}

/// Bare `mixtape:` arguments and the link after [`LINK_FLAG`] (a missing or non-link value is
/// ignored), in order; [`FROM_ISLAND`] anywhere.
pub fn parse(args: impl IntoIterator<Item = String>) -> Start {
    let mut start = Start::default();
    let mut args = args.into_iter().peekable();
    while let Some(a) = args.next() {
        if a == FROM_ISLAND {
            start.from_island = true;
        } else if a == LINK_FLAG {
            if args.peek().is_some_and(|v| is_link(v)) {
                start.links.extend(args.next());
            }
        } else if is_link(&a) {
            start.links.push(a);
        }
    }
    start
}

/// Whether a link may go to the island: a Mixtape link no longer than [`MAX_LINK`] bytes.
pub fn handable(link: &str) -> bool {
    is_link(link) && link.len() <= MAX_LINK
}

/// Hands the start's links to the island and returns those that stay with Mixtape, in order.
/// Nothing is handed when they came from the island or there is no island. A link that can't be
/// handed stays; when starting the island fails, that link and all later ones stay.
pub fn route(
    start: &Start,
    island: Option<&std::path::Path>,
    mut spawn: impl FnMut(&std::path::Path, &str) -> bool,
) -> Vec<String> {
    let Some(island) = island.filter(|_| !start.from_island) else {
        return start.links.clone();
    };
    let mut stay = Vec::new();
    let mut failed = false;
    for l in &start.links {
        if !failed && handable(l) {
            if spawn(island, l) {
                continue;
            }
            failed = true;
        }
        stay.push(l.clone());
    }
    stay
}

/// Starts the island with one link. True when the program started.
pub fn spawn_island(island: &std::path::Path, link: &str) -> bool {
    match std::process::Command::new(island).args([LINK_FLAG, link]).spawn() {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "could not hand a link to the island");
            false
        }
    }
}

/// The links of this start that stayed with Mixtape (set once by `run`, read by `setup`).
pub static START: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

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
        start(args).links
    }


    fn start(args: &[&str]) -> Start {
        parse(args.iter().map(|s| (*s).to_owned()))
    }

    fn owned(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parse_bare_and_flagged_links_in_order() {
        assert_eq!(
            start(&["mixtape://a", "--mixtape-link", "mixtape://b", "--hidden", "MIXTAPE://c"]),
            Start { links: owned(&["mixtape://a", "mixtape://b", "MIXTAPE://c"]), from_island: false }
        );
    }

    #[test]
    fn parse_from_island_anywhere() {
        assert!(start(&["--from-island", "--mixtape-link", "mixtape://a"]).from_island);
        assert!(start(&["--mixtape-link", "mixtape://a", "--from-island"]).from_island);
        assert!(!start(&["--mixtape-link", "mixtape://a"]).from_island);
        assert!(!start(&["--from-islands", "mixtape://a"]).from_island);
    }

    #[test]
    fn parse_flag_without_a_link_value() {
        assert!(start(&["--mixtape-link"]).links.is_empty());
        assert_eq!(start(&["--mixtape-link", "--from-island"]).links, Vec::<String>::new());
        assert!(start(&["--mixtape-link", "--from-island"]).from_island);
        assert!(start(&["--mixtape-link", "https://music.youtube.com/watch?v=a"]).links.is_empty());
        // the value that isn't a link is not swallowed as a flag either
        assert_eq!(start(&["--mixtape-link", "--mixtape-link", "mixtape://a"]).links, ["mixtape://a"]);
    }

    #[test]
    fn parse_plain_starts_have_no_links() {
        assert_eq!(start(&["--autostart", "--hidden"]), Start::default());
        assert_eq!(start(&[]), Start::default());
    }

    #[test]
    fn handable_boundaries() {
        let base = "mixtape://open?url=";
        let at_max = format!("{base}{}", "a".repeat(MAX_LINK - base.len()));
        assert!(handable(&at_max));
        assert!(!handable(&format!("{at_max}a")));
        assert!(handable("Mixtape:open"));
        assert!(!handable("https://music.youtube.com/watch?v=a"));
        assert!(!handable(""));
    }

    fn run_route(s: &Start, island: bool, fail_on: Option<&str>) -> (Vec<String>, Vec<String>) {
        let path = std::path::PathBuf::from("C:/Island/droplet-island.exe");
        let mut spawned = Vec::new();
        let stay = route(s, island.then_some(path.as_path()), |p, l| {
            assert_eq!(p, path.as_path());
            spawned.push(l.to_owned());
            Some(l) != fail_on
        });
        (stay, spawned)
    }

    #[test]
    fn route_without_island_keeps_all() {
        let s = start(&["mixtape://a", "mixtape://b"]);
        assert_eq!(run_route(&s, false, None), (owned(&["mixtape://a", "mixtape://b"]), vec![]));
    }

    #[test]
    fn route_with_island_hands_all_in_order() {
        let s = start(&["mixtape://a", "--mixtape-link", "mixtape://b"]);
        assert_eq!(run_route(&s, true, None), (vec![], owned(&["mixtape://a", "mixtape://b"])));
    }

    #[test]
    fn route_from_island_never_hands_back() {
        let s = start(&["--from-island", "--mixtape-link", "mixtape://a"]);
        assert_eq!(run_route(&s, true, None), (owned(&["mixtape://a"]), vec![]));
    }

    #[test]
    fn route_keeps_an_unhandable_link_and_hands_the_rest() {
        let long = format!("mixtape://open?url={}", "a".repeat(MAX_LINK));
        let s = Start { links: vec!["mixtape://a".into(), long.clone(), "mixtape://b".into()], from_island: false };
        assert_eq!(run_route(&s, true, None), (vec![long], owned(&["mixtape://a", "mixtape://b"])));
    }

    #[test]
    fn route_failed_spawn_keeps_that_link_and_later_ones() {
        let s = start(&["mixtape://a", "mixtape://b", "mixtape://c"]);
        let (stay, spawned) = run_route(&s, true, Some("mixtape://b"));
        assert_eq!(stay, owned(&["mixtape://b", "mixtape://c"]));
        assert_eq!(spawned, owned(&["mixtape://a", "mixtape://b"]));
    }

    #[test]
    fn route_nothing_to_do_without_links() {
        assert_eq!(run_route(&Start::default(), true, None), (vec![], vec![]));
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
