use std::path::Path;

fn main() {
    secrets();
    tauri_build::build()
}

/// Last.fm and Discord keys are not in the repository. They come from the environment at
/// compile time (`MIXTAPE_LASTFM_KEY`, `MIXTAPE_LASTFM_SECRET`, `MIXTAPE_DISCORD_APP_ID`)
/// or, when those are unset, from the gitignored `.secrets/lastfm.json`
/// (`{"key": "...", "secret": "..."}`) and `.secrets/discord.json` (`{"appId": "..."}`).
/// Without them the features are hidden.
fn secrets() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.secrets");
    from_file(
        &dir.join("lastfm.json"),
        &[
            ("key", "MIXTAPE_LASTFM_KEY"),
            ("secret", "MIXTAPE_LASTFM_SECRET"),
        ],
    );
    from_file(
        &dir.join("discord.json"),
        &[("appId", "MIXTAPE_DISCORD_APP_ID")],
    );
    // Watch only what exists: a missing path would rerun this script on every build.
    // The folder's listing changes when a file appears in it; without the folder, the
    // workspace root's does when it is created.
    let watch = [
        dir.join("lastfm.json"),
        dir.join("discord.json"),
        dir.clone(),
    ];
    let existing: Vec<_> = watch.iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        println!(
            "cargo:rerun-if-changed={}",
            dir.parent().unwrap_or(&dir).display()
        );
    }
    for p in existing {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}

fn from_file(path: &Path, fields: &[(&str, &str)]) {
    for (_, var) in fields {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        println!("cargo:warning={} is not valid JSON", path.display());
        return;
    };
    for (field, var) in fields {
        if std::env::var_os(var).is_some() {
            continue;
        }
        if let Some(v) = json
            .get(field)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            println!("cargo:rustc-env={var}={v}");
        }
    }
}
