# Implementation Plan: Mixtape MVP

**Branch**: `001-mixtape-mvp` | **Date**: 2026-10-17 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/001-mixtape-mvp/spec.md`

## Summary

Личный Windows-плеер: Rust-ядро (звук, библиотека, загрузка, YouTube Music) в оболочке
Tauri 2 с UI на Svelte 5. Библиотека выкачивается из YouTube Music один раз и играет офлайн.
Миксы и каталог стримятся через тот же кэш с предзагрузкой. Каталог, библиотека и радио
читаются через RustyPipe (форк на зафиксированном rev) + сырые InnerTube-запросы для лайков,
главной и чипов. Аудиопотоки идут через sidecar `yt-dlp`, байты качает Rust. Детали решений —
[research.md](research.md).

## Technical Context

**Language/Version**: Rust 1.97 (edition 2024), TypeScript 7 / Svelte 5.57

**Primary Dependencies**:
- Rust: tauri 2.12, rustypipe 0.11 (форк), reqwest + tokio, symphonia 0.6.1 +
  symphonia-adapter-libopus 0.3, cpal 0.18.2, rtrb 0.4, rubato 5.0, ebur128 0.1.10,
  rusqlite 0.40 (bundled) + rusqlite_migration 2.6, fast_image_resize 6.1 + image 0.25,
  kmeans_colors 0.7, windows 0.62.
- Frontend: vite 8, svelte 5.57, tailwindcss 4.3, virtua 0.53, unplugin-icons 24.
- Sidecar: yt-dlp 2026.08.19 + yt-dlp-ejs + deno.

**Storage**: SQLite (WAL) + файлы аудио и обложек в `%LOCALAPPDATA%\Mixtape\`
(`library\`, `cache\`, `covers\`, `mixtape.db`); модель в [data-model.md](data-model.md).

**Testing**:
- `cargo test`: модели очереди и микса, БД и поиск, синхронизация на фикстурах JSON ответов
  YouTube, `SparseCacheFile`, декодер на коротких тестовых webm/m4a.
- `vitest`: чистая логика UI (интерполяция позиции, раскладка).
- Ручные сценарии: [quickstart.md](quickstart.md).

**Target Platform**: Windows 10/11 x64, WebView2 Evergreen

**Project Type**: desktop-app (Tauri)

**Performance Goals**:
- старт ≤ 1,5 с;
- переключение трека ≤ 150 мс;
- поиск ≤ 50 мс на 20k треков;
- 60 fps скролл и анимации.

**Constraints**:
- ≤ 150 МБ RAM суммарно, включая процессы WebView2;
- офлайн для скачанного;
- ≤ 2 параллельных загрузки с паузами (анти-бот YouTube);
- cookie шифруется DPAPI.

**Scale/Scope**: до 20 000 треков, ~10 экранов и модалок, один пользователь.

## Constitution Check

*GATE: проверено до Phase 0 и повторно после Phase 1.*

| Принцип | Статус | Как соблюдается |
|---|---|---|
| I. Производительность | ✅ | Звук в Rust (cpal), позиция в UI ≤ 10 Гц с интерполяцией, виртуальные списки, страничная выдача данных из Rust, обложки заранее нарезаны. Бюджеты проверяются в quickstart (RAM, старт, поиск). |
| II. Интерфейс как продукт | ✅ | Отдельные задачи на состояния загрузки, пустые экраны и ошибки; горячие клавиши; тексты песен скрыты за кнопкой. |
| III. Local-first | ✅ | SQLite + файлы — источник правды; кэш стрима с LRU; без сети работает всё скачанное. |
| IV. YouTube за одной границей | ✅ | Крейт `yt` с трейтом `YtMusic` и доменными типами; RustyPipe, InnerTube и yt-dlp не видны снаружи. Лимит параллельности и паузы при загрузке. |
| V. Простота | ⚠️ обосн. | Sidecar yt-dlp + deno — внешняя зависимость; без неё сейчас нельзя получить полный аудиопоток (R2). Изолирован за `StreamResolver` в `yt`. |
| VI. Clean-room | ✅ | Liner — только референс раскладки; код, шейдеры и ассеты не используются. |

**Отклонение от спеки**: SC-004 («первый звук микса ≤ 2 с») достижим, когда стартовый трек
локальный или уже в кэше. Для нескачанного трека разрешение URL через yt-dlp занимает
2–4 с, поэтому порог для него — ≤ 5 с. Spec SC-004 уточнён соответственно.

## Project Structure

### Documentation (this feature)

```text
specs/001-mixtape-mvp/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/ipc.md
└── tasks.md            # /speckit-tasks
```

### Source Code (repository root)

```text
Cargo.toml                 # [workspace] resolver=3, members = crates/*, src-tauri
crates/
├── core/                  # доменные типы (Track, Album, Artist, Playlist, ids), ошибки
├── library/               # SQLite: миграции, репозитории, поток-писатель, поиск (FTS5 + раскладка), views
├── artwork/               # загрузка и нарезка обложек 64/300/1024, палитра, blur-миниатюра
├── yt/                    # трейт YtMusic + реализация: rustypipe (форк), raw InnerTube (лайки, главная, чипы),
│                          #   StreamResolver (yt-dlp sidecar), cookie-хранилище DPAPI, rate limiter
├── audio/                 # движок: symphonia+libopus → rubato → rtrb → cpal; gapless, seek, громкость,
│                          #   SparseCacheFile + Range-загрузчик, ebur128, спектр
├── player/                # очередь, MixSession, предзагрузка, восстановление состояния, play_stats/автосохранение
├── sync/                  # download_jobs, синхронизация источников, импорт по ссылке, like_outbox
└── win/                   # SMTC, кнопки миниатюры панели задач
src-tauri/                 # тонкий слой: команды/каналы/события (contracts/ipc.md), cover:// протокол,
│                          #   окно (Mica, frame), sidecars (binaries/yt-dlp, deno), конфиг
└── binaries/              # yt-dlp-x86_64-pc-windows-msvc.exe, deno-… (не в git, скачиваются скриптом)
ui/
├── index.html
├── vite.config.ts
└── src/
    ├── app.css            # Tailwind 4 @theme: цвета, радиусы, шрифт
    ├── main.ts / App.svelte
    ├── lib/ipc/           # обёртки команд, типы (сгенерированные), каналы
    ├── lib/stores/        # player.svelte.ts, queue, library, downloads, account, router
    ├── lib/ui/            # Button, Menu, Slider, Modal, Toast, Cover, VirtualTrackList, Chips
    └── screens/           # Home, Library (tracks/albums/artists/playlists/liked), Album, Artist,
                           #   Playlist, Search, Downloads, Settings, Onboarding, FullscreenPlayer
scripts/
└── fetch-sidecars.ps1     # скачивание yt-dlp/deno в src-tauri/binaries
```

**Structure Decision**: cargo workspace с доменными крейтами. Тяжёлую логику можно тестировать
без Tauri, а `src-tauri` остаётся тонким адаптером. UI — отдельное Vite-приложение в `ui/`,
без SvelteKit.

## Ключевые интерфейсы ядра

```rust
// crates/yt
trait YtMusic: Send + Sync {
    async fn search(&self, q: &str, kind: SearchKind) -> Result<Page<CatalogItem>>;
    async fn album(&self, id: &AlbumId) -> Result<AlbumFull>;
    async fn artist(&self, id: &ArtistId) -> Result<ArtistFull>;          // all_albums = true
    async fn playlist(&self, id: &PlaylistId) -> Result<PlaylistFull>;
    async fn library(&self, part: LibraryPart) -> Result<Vec<LibraryEntry>>; // liked|playlists|albums|artists
    async fn radio(&self, seed: RadioSeed) -> Result<RadioPage>;           // RadioPage { tracks, chips, continuation }
    async fn radio_more(&self, c: &Continuation) -> Result<RadioPage>;
    async fn home(&self) -> Result<Vec<Shelf>>;
    async fn set_like(&self, id: &TrackId, liked: bool) -> Result<()>;
}
trait StreamResolver: Send + Sync {
    async fn resolve(&self, id: &TrackId) -> Result<StreamInfo>; // url, itag, container, size, loudness_db, expires_at
}

// crates/audio
struct Engine; // команды: Load(Source), QueueNext(Source), Play, Pause, Seek(ms), SetVolume(f32), SetGainDb(f32)
enum Source { File(PathBuf), Streaming(Arc<SparseCacheFile>) }
// события движка: Position{ms}, TrackStarted{id}, TrackEnded{id, full}, Buffering(bool), Error

// crates/player
struct Player; // владеет Queue, MixSession, Engine; решает, что предзагрузить (2–3 трека вперёд)
```

## Правила

- Модуль `yt` — единственный, кто знает про RustyPipe, InnerTube и yt-dlp. Все ответы
  YouTube маппятся в типы `core`. Лимит: ≤ 2 одновременных запроса потоков и случайная пауза
  1–3 с между загрузками.
- В cpal-колбэке нет аллокаций, блокировок и логов.
- UI не держит больших коллекций: только страницы из `library_rows`, хранимые в `$state.raw`.
- Каждое изображение запрашивается в размере отображения (64/300/1024).
- Все строки интерфейса на русском, в одном модуле `ui/src/lib/i18n/ru.ts`.
- Ошибки сети и YouTube никогда не роняют воспроизведение локального.

## Порядок работ

0. **Spike** (на реальном аккаунте пользователя), проверить до основной разработки:
   - yt-dlp с cookies отдаёт полный поток itag 251 на этом IP;
   - радио и главная с cookies персонализированы, какие чипы видны;
   - вход через встроенное окно WebView2;
   - tauri-plugin-frame snap layouts;
   - базовый RAM пустого Tauri+Svelte окна.
1. **Фундамент**: workspace, Tauri-окно (frameless, Mica), UI-каркас и дизайн-токены, БД + миграции.
2. **US1**: аудиодвижок → очередь → библиотека/поиск → экраны → SMTC/миниатюры → восстановление.
3. **US2**: аккаунт → синк-превью → загрузчик (jobs, докачка, лимиты) → ссылки → неудавшиеся.
4. **US3**: StreamResolver + SparseCacheFile → MixSession + чипы → предзагрузка → автосохранение, лайки → outbox.
5. **US4**: поиск по каталогу, страницы артиста/альбома из каталога, «скачать дискографию».
6. **Главная** (локальные секции + шелфы YTM).
7. **US5**: тексты по кнопке.
8. **Полировка и замеры** бюджетов из quickstart.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Sidecar yt-dlp + deno | Единственный сейчас рабочий способ получить полный аудиопоток | RustyPipe streams сломаны (deobf, PO-token); свой SABR-клиент — недели работы |
| Форк RustyPipe | Фикс пагинации #87 и возможность быстро чинить парсеры | Upstream не выпускает релизы с 2025-04 |
| Свой `SparseCacheFile` | Стрим, seek и предзагрузка в постоянный кэш одним компонентом | `stream-download` не умеет сохранять в путь кэша |
