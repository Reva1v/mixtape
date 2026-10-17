# Tasks: Mixtape MVP

**Input**: `specs/001-mixtape-mvp/` — plan.md, spec.md, research.md, data-model.md, contracts/ipc.md, quickstart.md

**Tests**: по конституции ядро покрывается `cargo test` без сети (фикстуры JSON ответов YouTube,
короткие тестовые аудиофайлы). UI-тесты — только для чистой логики.

**Format**: `- [ ] [ID] [P?] [Story?] Описание с путём`

## Phase 1: Setup

- [x] T001 Создать cargo workspace (`resolver = "3"`, `[workspace.dependencies]` с версиями из plan.md) в `Cargo.toml`, пустые крейты `crates/{core,library,artwork,yt,audio,player,sync,win}` с `lib.rs`
- [x] T002 Создать Tauri 2.12 приложение в `src-tauri/` (`tauri.conf.json`: identifier `dev.mixtape.app`, `decorations:false`, `transparent:true`, `shadow:true`, `visible:false`, `windowEffects: mica`, `backgroundColor` тёмный, devUrl 5173, frontendDist `../ui/dist`)
- [x] T003 [P] Создать UI-проект `ui/` (Vite 8 + Svelte 5.57 + TS + `@tailwindcss/vite` 4.3 + `virtua` + `unplugin-icons` с `@iconify-json/lucide` и `@iconify-json/ph`), корневой `package.json` со скриптами `tauri dev/build`
- [x] T004 [P] Скрипт `scripts/fetch-sidecars.ps1`: скачать `yt-dlp.exe` и `deno.exe` в `src-tauri/binaries/` с суффиксом `-x86_64-pc-windows-msvc`, прописать `externalBin` в `src-tauri/tauri.conf.json`
- [x] T005 [P] Линт и формат: `rustfmt.toml`, `clippy` в `Cargo.toml` (`[workspace.lints]`), `svelte-check` + `prettier` в `ui/package.json`
- [x] T006 [P] Подключить генерацию TS-типов из Rust (`specta` + `tauri-specta`) → `ui/src/lib/ipc/bindings.ts`

## Phase 2: Foundational (блокирует все истории)

**Ядро данных**

- [x] T007 Доменные типы и ID (`TrackId`, `AlbumId`, `ArtistId`, `PlaylistId`, `TrackState = remote|cached|downloaded|broken`, `Track`, `Album`, `Artist`, `Playlist`, `CatalogItem`, `Page<T>`, ошибки) в `crates/core/src/lib.rs`
- [x] T008 Миграция v1 по data-model.md (все таблицы, `search_fts` trigram + триггеры, индексы) в `crates/library/migrations/001_init.sql`, применение `rusqlite_migration` в `crates/library/src/db.rs` (WAL, `synchronous=NORMAL`)
- [x] T009 Поток-писатель (очередь команд, батчи в транзакциях) + пул из 3 read-only соединений в `crates/library/src/conn.rs`
- [x] T010 [P] Нормализация `search_text` (lower, ё→е, NFKD без латинских диакритик) и конвертер раскладки ЙЦУКЕН↔QWERTY в `crates/library/src/normalize.rs` + unit-тесты
- [x] T011 Пути данных `%LOCALAPPDATA%\Mixtape\{library,cache,covers}` и `mixtape.db`, настройки (`settings` key/value JSON) в `crates/library/src/paths.rs`, `crates/library/src/settings.rs`
- [x] T012 [P] Хранилище обложек: загрузка, нарезка 64/300/1024 (`fast_image_resize`), blur-мини 64 px, палитра 4–5 цветов (`kmeans_colors`) в `crates/artwork/src/lib.rs` + тест на образце

**Оболочка и UI-каркас**

- [x] T013 AppState в `src-tauri/src/state.rs` (БД, артворк, позже player/sync/yt), регистрация команд и плагинов (`window-state`, `single-instance`) в `src-tauri/src/lib.rs`
- [x] T014 Async URI-протокол `cover://` → `http://cover.localhost/{key}/{size}` с `Cache-Control: immutable` и LRU в памяти в `src-tauri/src/cover_protocol.rs`
- [x] T015 Окно: `tauri-plugin-frame` (snap overlay на кнопке «развернуть»), команды `window_minimize/toggle_maximize/close`, команда `app_ready` → `show()` в `src-tauri/src/window.rs`
- [x] T016 Дизайн-токены Tailwind 4 `@theme` (тёмная палитра, акцент из обложки через CSS-переменную, радиусы, тени, Inter variable woff2 с кириллицей) в `ui/src/app.css`, шрифт в `ui/src/assets/fonts/`
- [x] T017 Каркас приложения: титлбар с drag-region и кнопками окна, узкий сайдбар, основная область, правая выдвижная панель, место под мини-плеер в `ui/src/App.svelte`, `ui/src/lib/ui/TitleBar.svelte`, `ui/src/lib/ui/Sidebar.svelte`
- [x] T018 [P] Роутер на `$state` (экраны + история назад/вперёд, Alt+←/→) в `ui/src/lib/stores/router.svelte.ts`
- [x] T019 [P] Базовые компоненты: Button, IconButton, Menu (контекстное), Slider, Modal, Toast, Cover (img с размером), Skeleton в `ui/src/lib/ui/`
- [x] T020 [P] Строки интерфейса на русском в `ui/src/lib/i18n/ru.ts`
- [x] T021 IPC-обёртки и подписка на события в `ui/src/lib/ipc/index.ts`
- [x] T022 Замер: релизная сборка пустого каркаса — RAM (Private WS mixtape + msedgewebview2) и время до `app_ready`; записать в `specs/001-mixtape-mvp/research.md` (spike S2)

**Checkpoint**: окно открывается за ≤ 1,5 с, каркас рисуется, БД создаётся.

## Phase 3: User Story 1 — локальная библиотека и плеер (P1) 🎯 MVP

**Goal**: играть скачанную музыку офлайн, без пауз между треками, с поиском и очередью.
**Independent Test**: библиотека из тестовых файлов (импорт локальных webm/m4a через dev-команду), сценарии Q4–Q7 из quickstart.md.

**Аудиодвижок**

- [x] T023 [US1] Декодер: symphonia 0.6.1 (mkv, isomp4, aac) + регистрация `symphonia-adapter-libopus`, применение `trim_start/trim_end`, точный seek (grubый seek + досчёт, pre-roll 80 мс для Opus) в `crates/audio/src/decoder.rs` + тесты на `crates/audio/tests/fixtures/*.webm|*.m4a`
- [x] T024 [US1] Вывод: cpal 0.18 WASAPI shared, `rtrb` кольцо ~300 мс f32, колбэк без аллокаций и локов, атомарный счётчик фреймов, тишина при underrun в `crates/audio/src/output.rs`
- [x] T025 [US1] Поток движка: команды `Load/QueueNext/Play/Pause/Seek/SetVolume/SetGainDb`, открытие следующего трека за 10 с до конца, PCM подряд в одно кольцо (gapless), маркеры `(frame, track_id)` для позиции и смены трека, fade 10 мс на паузу/seek в `crates/audio/src/engine.rs`
- [x] T026 [US1] Ресемплинг `rubato` только при частоте устройства ≠ источнику; пересоздание потока при ошибке cpal / смене устройства по умолчанию с сохранением позиции в `crates/audio/src/device.rs`
- [x] T027 [P] [US1] Громкость: `gain_dB` из `media_files.lufs` (цель −14 LUFS, clamp −20…+6), мягкий лимитер при усилении в `crates/audio/src/gain.rs` + тест
- [x] T028 [P] [US1] Замер EBU R128 (`ebur128`) для файла в фоне → `media_files.lufs/true_peak` в `crates/audio/src/loudness.rs`

**Очередь и плеер**

- [x] T029 [US1] Модель `Queue` (data-model.md): «следующим», «в конец блока User», remove, move, clear, repeat Off/All/One, shuffle только будущих с `original_order` + unit-тесты в `crates/player/src/queue.rs`
- [x] T030 [US1] `Player`: связка Queue ↔ Engine, переходы next/prev (prev всегда предыдущий трек, без перезапуска текущего — решение пользователя), `play_stats` (полное прослушивание = ≥ 90 % без перемотки за последние 10 %) в `crates/player/src/player.rs`
- [x] T031 [US1] Снимок очереди/позиции/громкости в `settings.queue_snapshot` (дебаунс 2 с + при выходе), восстановление на паузе при старте в `crates/player/src/persist.rs` + тест

**Библиотека**

- [x] T032 [US1] Репозитории треков/альбомов/артистов/плейлистов (upsert, связи, правило «в библиотеке = downloaded|broken») в `crates/library/src/repo/*.rs`
- [x] T033 [US1] Views: `library_view(kind, sort, filter) → {viewId, count}`, `library_rows(viewId, offset, limit≤200)` с кэшем упорядоченных ID в `crates/library/src/views.rs`
- [x] T034 [US1] Поиск: FTS5 trigram, <3 символов — префиксный скан, OR с конвертированной раскладкой (оригинал выше) + бенчмарк 20k строк ≤ 50 мс в `crates/library/src/search.rs`, `crates/library/benches/search.rs`
- [x] T035 [US1] Локальные плейлисты: create/rename/delete/add/remove/move (правка `ytm`-плейлиста создаёт `local`-копию) в `crates/library/src/repo/playlists.rs` + тесты
- [x] T036 [US1] Проверка целостности при старте (файл пропал/битый → `broken`) в `crates/library/src/integrity.rs`
- [x] T037 [P] [US1] Dev-команда импорта локальной папки webm/m4a (для теста истории без YouTube) в `src-tauri/src/commands/dev.rs`

**Команды и интеграция с Windows**

- [x] T038 [US1] Команды библиотеки и плеера из contracts/ipc.md (library_*, album_get/artist_get/playlist_* локально, play, toggle, next, prev, seek, set_volume, set_repeat, set_shuffle, queue_*) в `src-tauri/src/commands/{library,player}.rs`
- [x] T039 [US1] Канал `player_subscribe` (`PlaybackState` 10 Гц + на изменениях), события `track:changed` (с палитрой), `queue:changed` в `src-tauri/src/commands/player.rs`
- [x] T040 [P] [US1] SMTC через `windows` 0.62 (`GetForWindow`, кнопки, метаданные + обложка 300 px, таймлайн каждые 5 с, Min/MaxSeekTime) в `crates/win/src/smtc.rs`
- [x] T041 [P] [US1] Кнопки миниатюры (prev/play-pause/next) через `ITaskbarList3` + `SetWindowSubclass` в `crates/win/src/thumbbar.rs`

**UI**

- [x] T042 [US1] Стор плеера `player.svelte.ts` (состояние из канала, интерполяция позиции в rAF от `{pos, ts}`) + vitest на интерполяцию в `ui/src/lib/stores/player.svelte.ts`
- [x] T043 [US1] `VirtualTrackList` (virtua `VList`, строки 52 px, страницы через `library_rows` с LRU в `$state.raw`, hover/active, двойной клик — play, меню трека) в `ui/src/lib/ui/VirtualTrackList.svelte`
- [x] T044 [US1] Меню трека (US1, сценарий 3: «Включить микс», «Следующим», «В очередь», «В плейлист», «Лайк», «Открыть альбом», «К исполнителю», «Удалить из библиотеки») в `ui/src/lib/ui/TrackMenu.svelte`
- [x] T045 [US1] Экран «Библиотека» с вкладками Треки / Альбомы (`VGrid`) / Артисты / Плейлисты / Понравившиеся, сортировки, пустые состояния в `ui/src/screens/Library.svelte`
- [x] T046 [P] [US1] Экраны «Альбом», «Артист», «Плейлист» (шапка с обложкой и палитрой, play/shuffle/микс) в `ui/src/screens/{Album,Artist,Playlist}.svelte`
- [x] T047 [US1] Поиск: поле в титлбаре (`/` или Ctrl+K), мгновенные результаты по секциям, клавиатурная навигация в `ui/src/screens/Search.svelte`
- [x] T048 [US1] Мини-плеер: обложка, название/артист (ссылки), prev/play/next, таймлайн со скрабом, громкость, repeat/shuffle, кнопка очереди и полноэкранного режима в `ui/src/lib/ui/MiniPlayer.svelte`
- [x] T049 [US1] Правая панель «Очередь» (текущий, «Далее», drag-n-drop перестановка, удаление, очистка) в `ui/src/lib/ui/QueuePanel.svelte`
- [x] T050 [US1] Полноэкранный плеер по референсу пользователя (Liner): компактная колонка по центру — обложка ~280 px с кнопкой «+» в углу, название/исполнитель, тонкий таймлайн, минималистичный транспорт без кругов, громкость; тёмный фон со свечением цвета обложки (слои blur-мини, только transform/opacity), круглая «свернуть» слева сверху, Esc/F в `ui/src/screens/FullscreenPlayer.svelte`
- [x] T051 [US1] Горячие клавиши (Space, ←/→ seek 5 с, Shift+←/→ трек, ↑/↓ громкость, M, Q, F, /, Esc) в `ui/src/lib/stores/hotkeys.ts`

**Checkpoint**: сценарии Q4–Q7 проходят офлайн.

## Phase 4: User Story 2 — выкачка из YouTube Music (P1)

**Goal**: подключить аккаунт и один раз скачать лайки, плейлисты, альбомы, артистов; ссылки вручную.
**Independent Test**: сценарии Q1–Q3.

- [x] T052 [US2] Трейт `YtMusic` и доменные типы ответов (research R1) в `crates/yt/src/api.rs`
- [x] T053 [US2] Cookie-хранилище: импорт `cookies.txt` и JSON (формат EditThisCookie), шифрование DPAPI, `CacheStorage` для RustyPipe, временная копия файла для yt-dlp в `crates/yt/src/auth.rs` + тест разбора обоих форматов
- [x] T054 [US2] ~~Форк RustyPipe~~ → свой InnerTube-клиент (`innertube.rs`, `parse.rs`, `api.rs`), см. research R1 «Пересмотр». Было: Форк RustyPipe (фикс #87) как git-зависимость на rev; клиент с нашим storage; маппинг `music_liked_tracks`, `music_saved_playlists/albums/artists`, `music_album`, `music_artist(id,true)`, `music_playlist` в `crates/yt/src/rustypipe_impl.rs`
- [x] T055 [US2] Обложки YouTube: перевод URL в `=w1200-h1200-l90-rj` и скачивание в artwork в `crates/yt/src/thumbs.rs`
- [x] T056 [US2] `StreamResolver` через sidecar yt-dlp (`--cookies <копия> --js-runtimes deno:<path> -f 251/140 -J`, пакетное разрешение до 5 ID, кэш URL до `expires_at`, таймаут 30 с, разбор ошибок в человекочитаемые причины) в `crates/yt/src/resolver.rs` + тест разбора JSON-фикстуры
- [x] T057 [US2] Rate limiter: ≤ 2 загрузки параллельно, пауза 1–3 с между задачами, backoff при 429 / «not a bot» в `crates/yt/src/limits.rs`
- [x] T058 [US2] Загрузчик файла: чанки `&range=` ~10 МБ, запись в `.part`, докачка по уже записанным байтам, переименование в `library/` + `media_files` в `crates/sync/src/fetch.rs`
- [x] T059 [US2] `download_jobs`: очередь, статусы `queued/running/paused/done/failed`, «одна активная задача на track_id», `running → queued` после перезапуска, пауза/продолжение, повтор неудавшихся в `crates/sync/src/jobs.rs` + тесты переходов
- [x] T060 [US2] Синхронизация источников (`sync_sources`): лайки, выбранные плейлисты (перезапись состава `ytm`-плейлистов), альбомы, артисты (дискография) — только новые треки в загрузку, предпочтение аудио-версии перед клипом в `crates/sync/src/sources.rs` + тест на фикстурах
- [x] T061 [US2] Импорт по ссылке (трек / альбом / плейлист music.youtube.com и youtube.com) в `crates/sync/src/url_import.rs` + тест разбора URL
- [x] T062 [US2] Проверка места на диске (пауза при < 2 ГБ + предупреждение) в `crates/sync/src/disk.rs`
- [x] T063 [US2] Команды `account_*`, `sync_preview`, `sync_start`, `import_url`, `downloads_*`, канал `downloads_subscribe`, событие `account:expired` в `src-tauri/src/commands/sync.rs`
- [x] T064 [US2] Вход через окно WebView2 на music.youtube.com с чтением cookies (spike: если Google блокирует — скрыть кнопку, оставить импорт файла) в `src-tauri/src/login_window.rs`
- [x] T065 [US2] Онбординг/подключение аккаунта (вход в окне или вставка cookies, инструкция) в `ui/src/screens/Onboarding.svelte`
- [x] T066 [US2] Экран «Синхронизация»: превью с количествами, выбор источников, общий прогресс, текущий трек, скорость, пауза, список неудавшихся с причиной и «Повторить» в `ui/src/screens/Downloads.svelte`
- [x] T067 [P] [US2] Модалка «Добавить по ссылке» (Ctrl+V в любом месте с распознанной ссылкой — предложение скачать) в `ui/src/lib/ui/ImportUrlModal.svelte`
- [x] T068 [P] [US2] Индикатор загрузок в сайдбаре в `ui/src/lib/ui/Sidebar.svelte`

**Checkpoint**: Q1–Q3 проходят; скачанное сразу доступно в US1.

## Phase 5: User Story 3 — «Включить микс» (P1)

**Goal**: бесконечный персональный микс от трека с чипами и мгновенными переходами.
**Independent Test**: сценарии Q8–Q9.

- [x] T069 [US3] Сырые InnerTube-запросы с SAPISIDHASH (`SAPISIDHASH`, `SAPISID1PHASH`, `SAPISID3PHASH`, `X-Goog-AuthUser`) через `RustyPipeQuery::raw` или свой reqwest-клиент в `crates/yt/src/innertube.rs`
- [x] T070 [US3] `radio(seed)` / `radio_more(continuation)` / `radio_by_chip`: разбор `playlistPanelVideoRenderer` и `…WrapperRenderer` (брать аудио-версию), чипов `chipCloudChipRenderer` (пропуск действия «Сохранить»), `nextRadioContinuationData` в `crates/yt/src/radio.rs` + тест на сохранённой фикстуре ответа
- [x] T071 [US3] `SparseCacheFile`: предвыделенный файл, интервалы под `Mutex+Condvar`, блокирующий `read` с таймаутом/отменой, `MediaSource` для symphonia, перенацеливание загрузки при seek, финализация в `cache/` + тесты (чтение до/после докачки, seek за границу) в `crates/audio/src/sparse.rs`
- [x] T072 [US3] `Source::Streaming` в движке + статус `Buffering` при underrun в `crates/audio/src/engine.rs`
- [x] T073 [US3] `MixSession` (data-model.md): старт от трека, `seen`-дедупликация, подгрузка продолжения при ≤ 5 оставшихся, выбор чипа заменяет будущую часть очереди без прерывания текущего + unit-тесты в `crates/player/src/mix.rs`
- [x] T074 [US3] Предзагрузчик: пакетное предразрешение URL для следующих 5 треков, докачка 2–3 следующих в кэш, приоритет текущего, отмена при смене очереди в `crates/player/src/prefetch.rs`
- [x] T075 [US3] LRU кэша стрима (лимит из настроек, по умолчанию 5 ГБ; `cached → remote` при вытеснении) в `crates/library/src/cache_lru.rs` + тест
- [x] T076 [US3] Автосохранение: `full_play_count ≥ N` (по умолчанию 3, выключаемо) → перенос из кэша в библиотеку без повторной загрузки; лайк/«Скачать» → то же в `crates/player/src/autosave.rs` + тест
- [x] T077 [US3] `like_outbox`: лайк/снятие → `like/like` / `like/removelike`, отложенная отправка при появлении сети, схлопывание по track_id в `crates/sync/src/likes.rs` + тест
- [x] T078 [US3] Офлайн-режим микса: при недоступности продолжения — предложение «продолжить из библиотеки» (локальные треки тех же артистов, затем случайные) в `crates/player/src/mix.rs`
- [x] T079 [US3] Команды `mix_start`, `mix_select_chip`, `track_set_liked`, `track_download`, `queue_get` с чипами в `src-tauri/src/commands/mix.rs`
- [x] T080 [US3] UI: чипы над очередью в `QueuePanel` (горизонтальный скролл, выбранный подсвечен), значок «стрим/в кэше/скачан» у трека, состояние «буферизация» в мини-плеере в `ui/src/lib/ui/{QueuePanel,Chips,MiniPlayer}.svelte`

**Checkpoint**: Q8–Q9 проходят: 20 треков без пауз, смена чипа, автосохранение, лайк виден в YouTube Music.

## Phase 6: User Story 4 — каталог YouTube Music (P2)

**Goal**: искать вне библиотеки, слушать стримом, скачивать.
**Independent Test**: сценарий Q10.

- [x] T081 [US4] `search(q, kind)`, `suggest(q)`, страницы каталога с токеном продолжения в `crates/yt/src/rustypipe_impl.rs`
- [ ] T082 [US4] Сохранение элементов каталога как `remote` (без загрузки) для открытия и проигрывания в `crates/library/src/repo/catalog.rs`
- [ ] T083 [US4] Команды `catalog_search`, `catalog_more`, `catalog_suggest`; `album_get`/`artist_get` с фолбэком на каталог в `src-tauri/src/commands/catalog.rs`
- [ ] T084 [US4] Вкладка «Каталог» в поиске (метки «скачано»), страницы альбома/артиста из каталога с кнопками «Скачать альбом» / «Скачать дискографию» в `ui/src/screens/{Search,Album,Artist}.svelte`

## Phase 7: Главная (FR-024, обслуживает US1/US3)

- [x] T085 Разбор `browse FEmusic_home` + continuation в шелфы (заголовок, элементы: трек/альбом/плейлист/микс) в `crates/yt/src/home.rs` + тест на фикстуре
- [ ] T086 Команда `home_get`: «Продолжить», «Часто слушаю», «Недавно добавлено» из БД + шелфы YTM (кэш 30 мин, скрыть без сети) в `src-tauri/src/commands/home.rs`
- [ ] T087 Экран «Главная» с горизонтальными каруселями и быстрым запуском микса в `ui/src/screens/Home.svelte`

## Phase 8: User Story 5 — тексты по кнопке (P3)

**Independent Test**: сценарий Q11.

- [x] T088 [P] [US5] Клиент lrclib (`/api/get` по artist/title/album/duration), парсер LRC, кэш `lyrics_cache` (в т.ч. «не найдено») в `crates/sync/src/lyrics.rs` + тест парсера
- [x] T089 [US5] Команда `lyrics_get` в `src-tauri/src/commands/lyrics.rs`
- [x] T090 [US5] Кнопка текста в мини- и полноэкранном плеере. В полноэкранном: колонка плеера уезжает в левую половину, справа крупные жирные строки (~30 px), текущая — белая, остальные приглушены, плавная автопрокрутка к текущей; сверху плашка выбора провайдера и сдвиг ±0.1 с. Подсветка — бинарный поиск по времени в rAF, клик по строке — seek, «Текст не найден» в `ui/src/lib/ui/LyricsPanel.svelte`

## Phase 9: Polish & Cross-Cutting

- [x] T091 [P] Настройки: лимит кэша, N автосохранения (вкл/выкл), каталог данных, аккаунт, очистка кэша в `ui/src/screens/Settings.svelte` + команды в `src-tauri/src/commands/settings.rs`
- [x] T092 [P] Тосты и обработка ошибок сети/YouTube (локальное воспроизведение никогда не падает), событие `net:status` в `src-tauri/src/net.rs`, `ui/src/lib/ui/Toast.svelte`
- [ ] T093 [P] Канал спектра (16 полос, ≤ 30 Гц, только при открытом полноэкранном плеере) и его визуализация в фоне в `crates/audio/src/spectrum.rs`, `ui/src/screens/FullscreenPlayer.svelte`
- [ ] T094 Микро-анимации и переходы экранов (только transform/opacity), проверка 60 fps в `ui/src/app.css` и экранах
- [x] T095 Замер всех бюджетов по quickstart.md на релизной сборке с синтетической библиотекой 20k треков; результаты в `specs/001-mixtape-mvp/quickstart.md`
- [x] T096 Сборка NSIS-инсталлятора с sidecar'ами, иконка приложения в `src-tauri/icons/`

## Dependencies & Execution Order

- Setup (T001–T006) → Foundational (T007–T022) → истории.
- **US1** (T023–T051) — независима от сети; первая к реализации.
- **US2** (T052–T068) — зависит от Foundational; использует библиотеку US1 для показа, но тестируется сама (Q1–Q3 + проверка файлов).
- **US3** (T069–T080) — зависит от `YtMusic`/`StreamResolver`/auth из US2 (T052–T057) и движка/очереди из US1.
- **US4** — зависит от T052–T057, T071–T072. **Главная** — от T069 и T030. **US5** — только от плеера US1.
- Внутри истории: ядро (крейты) → команды `src-tauri` → UI.

## Parallel Opportunities

- Setup: T003, T004, T005, T006 параллельно после T001–T002.
- Foundational: T010, T012, T018, T019, T020 параллельно.
- US1: аудио (T023–T028) ∥ библиотека (T032–T036) ∥ Windows (T040–T041); UI-экраны T046 параллельно после T043.
- US2: T053, T055, T057 параллельно после T052.
- US5 (T088) можно делать в любой момент после US1.

## Implementation Strategy

1. **MVP-1**: Setup + Foundational + US1 → красивый офлайн-плеер (тестовая библиотека из dev-импорта).
2. **MVP-2**: + US2 → своя музыка из YouTube Music.
3. **MVP-3**: + US3 → миксы — полноценная замена YouTube Music для ежедневного использования.
4. Далее US4 → Главная → US5 → Polish. После каждой ступени — коммит и проверка сценариев quickstart.
