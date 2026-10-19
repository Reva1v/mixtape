# Security policy

## Reporting a vulnerability

Please report security problems **privately** through GitHub:
[Security → Report a vulnerability](https://github.com/Reva1v/mixtape/security/advisories/new)
(a GitHub Security Advisory). Do not open a public issue for them.

Include what you found, how to reproduce it and the version of Mixtape (shown in Settings, or in the
installer name). You should get an answer within a week. Once a fix is released, the advisory is
published with credit to you unless you prefer otherwise.

## Supported versions

Only the latest release gets security fixes; the app updates itself from GitHub Releases.

## Scope

In scope: the Mixtape app and installer, its handling of your YouTube cookies and local data, and
anything it does with data from the network (YouTube, LRCLIB, Spotify, SoundCloud, Bandcamp,
Last.fm, the update feed).

Out of scope: bugs in yt-dlp, Deno, WebView2 or the services themselves — please report those to
their projects — unless Mixtape uses them in an unsafe way.
