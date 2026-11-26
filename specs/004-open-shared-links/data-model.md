# Data model: Open shared YouTube Music links

Nothing is stored in the database or prefs, except the extension's own two switches in
`chrome.storage.local`.

## Link (existing, `mixtape_sync::url_import::Link`)

`Track(TrackId) | Album(AlbumId) | Playlist(PlaylistId) | Artist(ArtistId)`. The rules are in the module
docs. A playlist id is kept without `VL`; `OLAK5uy_…` lists are albums' audio playlists and open as a
playlist page.

## SharedLink (new, IPC, `ts-rs` export)

What the UI gets from `link_parse`:

| field | type | note |
|---|---|---|
| kind | `"track" \| "album" \| "playlist" \| "artist"` | |
| id | string | the id in the form the UI route for that kind expects |

## MixtapeLink (new, `mixtape_sync::mixtape_link`)

`Play { ids: Vec<TrackId>, at_ms: u64 } | Open(Link)`. It's parsed from `mixtape://play?…` /
`mixtape://open?url=…` (research R2). Invalid ids are dropped; `Play` with no valid id gives `None`.

## Pending links (new, PC, in memory)

A FIFO of raw URL strings in `src-tauri`, filled from argv before the UI is ready and drained on
`app_ready`. After that, links act on arrival. States: queued → acted on (or rejected with a toast).

## Extension settings

`{ catchYoutube: boolean = false, paused: boolean = false, promptAllowed: boolean = false }`.
