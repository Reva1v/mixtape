# Contract: IPC между UI (Svelte) и ядром (Rust)

Все команды — `async`, ошибки — `{ kind: string, message: string }` (сообщение на русском).
Типы — TypeScript-нотация; Rust-структуры с `#[serde(rename_all = "camelCase")]`.
TS-типы генерируются из Rust (`specta`/`ts-rs`) — ручного дублирования нет.

## Общие типы

```ts
type TrackId = string; type AlbumId = string; type ArtistId = string; type PlaylistId = string;
type TrackState = "remote" | "cached" | "downloaded" | "broken";
interface TrackRow { id: TrackId; title: string; artists: {id?: ArtistId; name: string}[];
  album?: {id: AlbumId; title: string}; durationMs: number; coverKey?: string;
  state: TrackState; liked: boolean }
interface AlbumCard { id: AlbumId; title: string; artists: string; year?: number; coverKey?: string }
interface ArtistCard { id: ArtistId; name: string; imageKey?: string }
interface PlaylistCard { id: PlaylistId; title: string; origin: "ytm"|"local"; trackCount: number; coverKey?: string }
```
Обложки: `<img src="http://cover.localhost/{coverKey}/{64|300|1024}">`.

## Библиотека

| команда | вход | выход |
|---|---|---|
| `library_view` | `{ kind: "tracks"\|"albums"\|"artists"\|"playlists"\|"liked", sort, filter? }` | `{ viewId, count }` |
| `library_rows` | `{ viewId, offset, limit }` (limit ≤ 200) | `TrackRow[] \| AlbumCard[] \| ...` |
| `library_search` | `{ query, limit }` | `{ tracks, albums, artists, playlists }` (top-N каждого) |
| `album_get` | `{ id }` | `{ album, tracks: TrackRow[] }` (локально, иначе из каталога) |
| `artist_get` | `{ id }` | `{ artist, top: TrackRow[], albums, singles }` |
| `playlist_get` / `playlist_create` / `playlist_rename` / `playlist_delete` / `playlist_move_item` / `playlist_add` / `playlist_remove` | … | … |
| `track_set_liked` | `{ id, liked }` | `void` (лайк → загрузка + outbox в YTM) |
| `track_download` / `track_remove` | `{ id }` | `void` |
| `home_get` | — | `{ likedCount, recent: Album[], added: Album[], top: Track[] }` (только БД, мгновенно) |
| `home_shelves` | `{ force? }` | `HomeShelf[]` — шелфы YTM `{ title, items: ShelfItem[] }`, кэш в памяти 30 мин; `[]` без сети/аккаунта |
| `mix_start_playlist` | `{ playlistId }` | `void` — карточка микса (`RD…`) с главной заменяет очередь |

`ShelfItem` = `{kind:"track", track: Track}` (сохранён как `remote`) \| `{kind:"album", id, title, artists, year?, coverKey?}`
\| `{kind:"playlist", id, title, subtitle?, coverKey?}` \| `{kind:"artist", id, name, coverKey?}`
\| `{kind:"mix", playlistId, title, subtitle?, coverKey?}`; обложки карточек лежат в CoverStore под ключом = id.

## Каталог YouTube Music

| команда | вход | выход |
|---|---|---|
| `catalog_search` | `{ query, kind: "tracks"\|"albums"\|"artists" }` | `{ items, nextToken? }` |
| `catalog_more` | `{ nextToken }` | `{ items, nextToken? }` |
| `catalog_suggest` | `{ query }` | `string[]` |

## Воспроизведение и очередь

| команда | вход |
|---|---|
| `play` | `{ source: {kind:"album"\|"playlist"\|"library"\|"liked"\|"tracks", id?, trackIds?}, startIndex? }` |
| `mix_start` | `{ trackId }` |
| `mix_select_chip` | `{ chipId }` |
| `toggle` / `next` / `prev` | — |
| `seek` | `{ positionMs }` |
| `set_volume` | `{ volume: 0..1 }` |
| `set_repeat` / `set_shuffle` | `{ mode }` / `{ on }` |
| `queue_get` | → `{ items: QueueRow[], current, mix?: { chips: Chip[] } }` |
| `queue_add` | `{ trackIds, position: "next"\|"end" }` |
| `queue_remove` / `queue_move` / `queue_clear` | `{ uid }` / `{ uid, toIndex }` / — |
| `player_subscribe` | `{ channel: Channel<PlaybackState> }` — позиция/статус 10 Гц и на изменениях |
| `spectrum_subscribe` | `{ channel: Channel<number[]> }` — 16 полос 0..1 (40 Гц–16 кГц, лог-шкала, не зависят от громкости), ~30 Гц, пока что-то играет, затем один кадр нулей; подписывается только открытый полноэкранный плеер |
| `spectrum_unsubscribe` | — останавливает поток спектра |

## Синхронизация и загрузки

| команда | вход / выход |
|---|---|
| `account_status` | → `{ connected, name?, avatar?, lastSyncAt? }` |
| `account_connect_cookies` | `{ cookiesTxt }` → `account_status` |
| `account_connect_webview` | открывает окно входа [spike] → `account_status` |
| `account_disconnect` | — |
| `sync_preview` | → `{ liked: n, playlists: PlaylistCard[], albums: n, artists: n }` |
| `sync_start` | `{ liked, playlistIds[], albums, artists }` |
| `import_url` | `{ url }` → `{ kind, title, trackCount }` (ставит в загрузку) |
| `downloads_pause` / `downloads_resume` / `download_retry` | — / — / `{ jobId \| "all-failed" }` |
| `downloads_subscribe` | `{ channel: Channel<DownloadsProgress> }` — сводка ≤4 Гц |

```ts
interface DownloadsProgress { running: boolean; paused: boolean; done: number; total: number;
  failed: number; current?: { trackId: TrackId; title: string; bytesDone: number; bytesTotal?: number };
  bytesPerSec: number }
```

## События (редкие, `emit`)

| событие | payload |
|---|---|
| `library:changed` | `{ kinds: ("tracks"\|"albums"\|"artists"\|"playlists")[] }` — UI инвалидирует view |
| `track:changed` | `{ track: TrackRow, palette?: string[] }` |
| `queue:changed` | `{}` — UI перезапрашивает `queue_get` |
| `account:expired` | `{}` — показать переподключение |
| `net:status` | `{ online: boolean }` |
| `toast` | `{ level: "info"\|"error", text }` |

## Окно

`tauri-plugin-frame` + команды `window_minimize/toggle_maximize/close`; `app_ready` из UI после
первого кадра → Rust показывает окно.
