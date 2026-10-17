# Data Model: Mixtape MVP

SQLite (WAL), миграции `rusqlite_migration`. Идентификаторы — нативные ID YouTube Music:
трек = `videoId` (11 символов), альбом = `MPRE…`/`OLAK…`, артист = `UC…`, плейлист = `VL…/PL…`.
Локальные плейлисты получают ID `local:<uuid>`. Время — unix ms (INTEGER).

## Таблицы

### artists
| поле | тип | примечание |
|---|---|---|
| id | TEXT PK | `UC…` |
| name | TEXT NOT NULL | |
| image_key | TEXT NULL | ключ в хранилище обложек |
| in_library | INTEGER 0/1 | артист из библиотеки YTM (дискография синхронизируется) |
| synced_at | INTEGER NULL | |

### albums
| поле | тип | примечание |
|---|---|---|
| id | TEXT PK | |
| title | TEXT NOT NULL | |
| year | INTEGER NULL | |
| album_type | TEXT | `album/ep/single` |
| cover_key | TEXT NULL | |
| palette | TEXT NULL | JSON-массив 4–5 hex-цветов |
| in_library | INTEGER 0/1 | сохранён в библиотеке YTM |
| track_count | INTEGER NULL | |

`album_artists(album_id, artist_id, position)` — PK (album_id, position).

### tracks
| поле | тип | примечание |
|---|---|---|
| id | TEXT PK | videoId |
| title | TEXT NOT NULL | |
| album_id | TEXT NULL FK albums | |
| track_no | INTEGER NULL | |
| duration_ms | INTEGER NOT NULL | |
| cover_key | TEXT NULL | своя обложка (если трек без альбома) |
| state | TEXT NOT NULL | `remote` / `cached` / `downloaded` / `broken` |
| liked | INTEGER 0/1 | |
| liked_at | INTEGER NULL | |
| added_at | INTEGER NULL | когда попал в библиотеку (`downloaded`) |
| search_text | TEXT NOT NULL | нормализованные title + artists + album |

`track_artists(track_id, artist_id NULL, name, position)` — артист может быть без ID.

**Состояния трека** (`state`):
```
remote ──stream/prefetch──▶ cached ──like | «Скачать» | автосохранение | sync──▶ downloaded
   └──────────────────── download (sync / ссылка) ───────────────────────────────▶ downloaded
cached ──LRU-вытеснение──▶ remote
downloaded ──файл пропал/повреждён──▶ broken ──перекачка──▶ downloaded
```
Правило: в библиотеке показываются `downloaded` и `broken`; `cached`/`remote` — только в
поиске каталога, миксах и на главной.

### media_files
| поле | тип | примечание |
|---|---|---|
| track_id | TEXT PK FK tracks | |
| path | TEXT NOT NULL | относительно каталога данных |
| container | TEXT | `webm` / `m4a` |
| itag | INTEGER | 251 / 140 |
| size_bytes | INTEGER | |
| sample_rate | INTEGER | |
| loudness_db | REAL NULL | из ответа плеера YouTube |
| lufs | REAL NULL | замер ebur128 после загрузки |
| true_peak | REAL NULL | |
| last_access | INTEGER | для LRU кэша |
| location | TEXT | `cache` / `library` |

Инвариант: `tracks.state='downloaded'` ⇔ есть запись с `location='library'`.
Сумма `size_bytes` по `location='cache'` ≤ лимита кэша (по умолчанию 5 ГБ).

### playlists / playlist_items
| поле | тип | примечание |
|---|---|---|
| id | TEXT PK | `VL…` или `local:<uuid>` |
| title | TEXT NOT NULL | |
| origin | TEXT | `ytm` / `local` |
| cover_key | TEXT NULL | |
| synced_at | INTEGER NULL | |

`playlist_items(playlist_id, position, track_id)` — PK (playlist_id, position).
Плейлисты `ytm` при синке перезаписываются составом из YTM; правки пользователя в них
превращают плейлист в `local`-копию (YTM-версия остаётся отдельной).

### play_stats
`track_id PK, play_count, full_play_count, last_played_at` — «Продолжить», «Часто слушаю»,
правило автосохранения: `full_play_count ≥ N` (настройка, по умолчанию 3) → загрузка в библиотеку.
Полное прослушивание = доиграл ≥ 90 % длительности без перемотки вперёд за последние 10 %.

### sync_sources
`kind PK (liked|playlists|albums|artists|url:<id>), enabled, last_synced_at, last_error`.

### download_jobs
| поле | тип | примечание |
|---|---|---|
| id | INTEGER PK | |
| track_id | TEXT FK | |
| source | TEXT | откуда поставлено (sync-источник, `url`, `autosave`, `user`) |
| status | TEXT | `queued` / `running` / `paused` / `done` / `failed` |
| bytes_done | INTEGER | |
| bytes_total | INTEGER NULL | |
| attempts | INTEGER | |
| error | TEXT NULL | человекочитаемая причина |
| updated_at | INTEGER | |

Уникальность: одна активная задача на `track_id`. Переходы:
`queued → running → done | failed`; `running ⇄ paused`; `failed → queued` (повтор).
После перезапуска `running` → `queued` (докачка по скачанным интервалам).

### like_outbox
`id, track_id, action (like|unlike), created_at, attempts` — отправка лайков в YTM;
обрабатывается при наличии сети и аккаунта, схлопывается по track_id (последнее действие).

### settings
`key PK, value JSON` — громкость, режимы повтора/перемешивания, лимит кэша, N автосохранения,
сохранённая очередь (`queue_snapshot`), состояние окна, язык (ru).

### lyrics_cache
`track_id PK, synced INTEGER, body TEXT NULL, fetched_at` (`body NULL` = не найдено).

### search_fts
`CREATE VIRTUAL TABLE search_fts USING fts5(search_text, tokenize='trigram',
content='tracks', content_rowid='rowid')` + триггеры синхронизации; аналогичные лёгкие
индексы для альбомов/артистов/плейлистов (или общий FTS с колонкой `kind`).

## Модели в памяти (не в БД)

### Queue
```
Queue {
  items: Vec<QueueItem{ uid, track_id, origin: User|Context|Mix }>,
  current: Option<usize>,
  repeat: Off|All|One, shuffle: bool, original_order: Option<Vec<uid>>,
  context: Album(id)|Playlist(id)|Library|Mix(MixSession)|Search,
}
```
- «Включить следующим» вставляет после текущего, «В очередь» — в конец блока `User`.
- Перемешивание затрагивает только будущие элементы; `original_order` сохраняется
  в `queue_snapshot` для корректного выключения после перезапуска.

### MixSession
```
MixSession {
  seed_track_id, playlist_id ("RDAMVM<id>" или из чипа), params: Option<String>,
  chips: Vec<Chip{ id, title, playlist_id, params, selected }>,
  continuation: Option<Ctoken>, seen: HashSet<track_id>,
}
```
- Подгрузка продолжения, когда до конца очереди ≤ 5 треков; дубликаты (`seen`) отбрасываются.
- Выбор чипа: будущая часть очереди после текущего заменяется ответом `next` по чипу,
  `continuation` — из этого ответа, `chips` не меняются.
- Без сети: если продолжение недоступно — предложение «продолжить из библиотеки»
  (локальные треки тех же артистов, затем случайные).

### PlaybackState (поток в UI через Channel: на изменениях и раз в ~5 с во время игры)
`{ track_id, status: Idle|Loading|Buffering|Playing|Paused|Error, position_ms, duration_ms,
   volume, timestamp_ms }` — UI интерполирует позицию сам.
