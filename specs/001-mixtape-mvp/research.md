# Research: Mixtape MVP

Проверено 2026-10-17 по crates.io/npm/docs.rs, исходникам и живыми запросами к YouTube.
Помечено **[spike]** — то, что подтверждается только на реальном аккаунте в фазе Spike.

## R1. Доступ к YouTube Music: данные

- **Decision**: RustyPipe 0.11.4 (GPL-3.0, feature `userdata`) для чтения каталога и библиотеки,
  подключённый как git-зависимость на зафиксированный rev собственного форка (с фиксом PR #87
  `#[serde(default)]` на `GridRenderer.items`). То, чего в RustyPipe нет, — через
  `RustyPipeQuery::raw(ClientType::DesktopMusic, endpoint, body)` + свой serde-разбор:
  - лайки: `like/like`, `like/removelike` с `{"target":{"videoId"}}`;
  - главная: `browse {"browseId":"FEmusic_home"}` (шелфы «Quick picks», миксы);
  - чипы микса: `next` → `musicQueueRenderer.subHeaderChipCloud.chipCloudRenderer.chips[]`,
    у чипа `queueUpdateCommand.fetchContentsCommand.watchEndpoint{playlistId, params}`;
    повторный `next` с этими `playlistId/params` отдаёт новую очередь с
    `nextRadioContinuationData`, но без чипов → список чипов храним из первого ответа.
- **Используемые вызовы RustyPipe** (проверены по исходникам, каталог — живьём):
  `music_search_tracks/_albums/_artists`, `music_search_suggestion`, `music_artist(id, true)`,
  `music_album`, `music_playlist`, `music_liked_tracks`, `music_saved_playlists`,
  `music_saved_albums`, `music_saved_artists`, `music_radio_track` (+ `Paginator::extend`),
  `user_auth_set_cookie_txt`, `user_auth_check_cookie`.
- **Rationale**: парсеры каталога в RustyPipe рабочие и большие, переписывать их дорого.
  Проект почти не поддерживается (последний релиз 2025-04, фиксы висят в ветках), поэтому
  всё за трейтом `YtMusic` в крейте `yt` — замена реализации не трогает остальной код.
- **Alternatives**: свой InnerTube-клиент целиком (больше кода сейчас, позже — возможно);
  youtubei.js сайдкаром (Node/Deno-процесс съедает 40–60 МБ из бюджета 150 МБ).
- **Риски [spike]**: персонализация радио и главной с cookies; какие чипы видны залогиненному
  («Знакомое», «Вечеринка»); `music_saved_playlists` на большом аккаунте.

## R2. Доступ к YouTube Music: аудиопотоки

- **Decision**: аудио получаем через `yt-dlp.exe` (последний релиз 2026.08.19) как sidecar:
  `yt-dlp --cookies <file> -f 251/140 -J <url>` → берём `url`, `filesize`, `itag` из JSON;
  сами байты качает Rust (reqwest, чанки ~10 МБ через `&range=`). Для решения JS-челленджей
  yt-dlp нужен `yt-dlp-ejs` + JS-рантайм (deno) — кладём рядом как sidecar.
  URL живут ~6 ч, кэшируются в памяти по `video_id`.
- **Rationale**: в RustyPipe 0.11.4 deobfuscation сломан для Desktop/Tv/Android, а iOS-URL
  без PO-token отдаёт только первые ~1 МБ (403 дальше) — полный трек получить нельзя.
  yt-dlp чинится в течение дней после поломок YouTube.
- **Цена**: запуск yt-dlp ≈ 2–4 с на трек. Скрывается предразрешением URL для следующих
  треков очереди. Для первого трека микса, которого нет локально, первый звук может прийти
  позже 2 с — см. уточнение SC-004 в plan.md.
- **Alternatives**: RustyPipe streams (сломаны, перепроверять при релизах upstream);
  ветка `feat/deobf-extractor` (только SABR без URL); свой SABR/UMP-клиент (слишком дорого).
- **Наблюдение**: при исследовании IP получил 429 / «Sign in to confirm you're not a bot»
  примерно после 30 анонимных запросов → обязательны кэш, лимит параллельности (2),
  паузы между загрузками и запросы с cookies.

## R3. Подключение аккаунта

- **Decision**: импорт `cookies.txt` (Netscape) из инкогнито-сессии + проверка
  `user_auth_check_cookie()`. Cookie хранится через свой `CacheStorage`, зашифрованный DPAPI
  (`CryptProtectData`), а не в plaintext `rustypipe_cache.json`. Тот же файл cookies
  (временно расшифрованный) отдаётся yt-dlp.
- **[spike]** Вход через встроенное окно Tauri на music.youtube.com с чтением cookies из
  WebView2 — удобнее, но Google может блокировать вход во встроенных браузерах. Если spike
  успешен — это основной путь, cookies.txt остаётся запасным.

## R4. Аудиодвижок

- **Decision**: свой движок на `cpal` 0.18.2 (WASAPI shared), без rodio.
  - Декодирование: `symphonia` 0.6.1 (mkv/webm, isomp4, aac) + `symphonia-adapter-libopus`
    0.3.0 (bundled libopus; pre-skip обрабатывает адаптер).
  - Поток декодера → SPSC-кольцо `rtrb` 0.4 (~300 мс f32) → cpal-колбэк (без аллокаций и
    локов, только pop + атомарный счётчик сыгранных фреймов).
  - Ресемплинг `rubato` 5.0.1 только если частота устройства ≠ частоте источника (Opus = 48 кГц).
  - Gapless: следующий трек открывается за ~10 с до конца, PCM подаётся в то же кольцо без
    перезапуска потока; применяются `trim_start/trim_end` пакетов. Хвостовой padding WebM
    (<20 мс) в v1 допускается.
  - Seek: грубый seek symphonia + досчитывание до точной позиции; для Opus pre-roll 80 мс.
  - Смена устройства: пересоздание потока по ошибке cpal / смене default-устройства,
    позиция сохраняется.
- **Громкость**: `gain_dB = clamp(-loudnessDb, -20, +6)`, где `loudnessDb` из ответа плеера
  YouTube (yt-dlp JSON); после загрузки — фоновый замер `ebur128` 0.1.10 (LUFS + true peak),
  дальше используется он. Рампа 10 мс, мягкий лимитер при усилении.
- **Alternatives**: rodio 0.23 (тянет symphonia 0.5, нет trim для mp4/mkv); воспроизведение в
  WebView (`<audio>`) — нарушает принцип I конституции.

## R5. Стрим с кэшем и предзагрузкой

- **Decision**: собственный `SparseCacheFile`: файл заранее выделен под `content_length`,
  карта скачанных интервалов под `Mutex + Condvar`, `MediaSource` для symphonia с блокирующим
  `read` (таймаут + отмена). Seek за пределы скачанного → загрузчик перенацеливается на новый
  Range. Полностью скачанный файл переименовывается в кэш/библиотеку. Тот же компонент
  предзагружает 2–3 следующих трека очереди.
- **Формат хранения**: как скачано — `.webm` (itag 251, Opus) приоритетно, `.m4a` (itag 140)
  запасной. Без ремукса и без ffmpeg. Метаданные — только в SQLite.
- **Alternatives**: `stream-download` 0.24 (нет постоянного хранилища в путь кэша).

## R6. Оболочка, IPC, UI

- **Decision**: Tauri 2.12 + Vite 8 + Svelte 5.57 (без SvelteKit: роутинг — `$state`-enum
  экранов, 6–7 экранов этого достаточно) + TypeScript + Tailwind 4.3 (`@tailwindcss/vite`).
  - IPC: `tauri::ipc::Channel` для частых данных (позиция 10 Гц, прогресс загрузок, спектр
    ≤30 Гц); события — только для редких уведомлений; команды `async`.
  - Большие списки не передаются целиком: Rust держит отсортированный/отфильтрованный view,
    UI запрашивает окна строк `get_rows(view_id, offset, limit)` под виртуальный список.
  - Виртуализация: `virtua` 0.53 (`VList`, `VGrid`, нативные компоненты Svelte 5).
  - Обложки: свой async URI-протокол `cover://` (`http://cover.localhost/{id}/{size}`),
    заранее нарезанные 64/300/1024 px через `fast_image_resize` 6.1, `Cache-Control: immutable`.
  - Окно: `decorations:false`, `shadow:true`, Mica (`windowEffects`, требует `transparent:true`),
    snap layouts через `tauri-plugin-frame` 1.1.8 **[spike]**, старт со `visible:false` и
    показ после первого кадра, `tauri-plugin-window-state`, `tauri-plugin-single-instance`.
  - Иконки: `unplugin-icons` 24 + `@iconify-json/lucide` и `ph` (filled play/pause).
    Шрифт — локальный variable woff2 с кириллицей (Inter).
- **Фон полноэкранного плеера**: палитра (4–5 цветов, `kmeans_colors` 0.7) и размытая
  мини-обложка 64 px считаются в Rust при импорте; на экране — 2–3 слоя этой картинки с
  анимацией только `transform/opacity` (композитор, почти 0 CPU). WebGL-градиент — опционально
  позже, ¼ разрешения, ≤30 fps, стоп при скрытии.

## R7. Хранилище и поиск

- **Decision**: `rusqlite` 0.40 (bundled SQLite 3.53, FTS5) + `rusqlite_migration` 2.6.
  WAL, `synchronous=NORMAL`. Один поток-писатель с очередью команд + 2–3 read-only соединения.
- **Поиск**: колонка `search_text` (lower, ё→е, без латинских диакритик; title + artists +
  album), FTS5 `tokenize='trigram'`; запросы <3 символов — префиксный скан. Неверная раскладка:
  запрос дополнительно конвертируется ЙЦУКЕН↔QWERTY и ищется через `OR`, оригинал ранжируется
  выше. Бенчмарк на 20k строк в тестах; если FTS-краевые случаи мешают — скан
  `memchr::memmem` по массиву строк в памяти (~2–3 МБ, ~1–3 мс).

## R8. Интеграция с Windows

- **Decision**: напрямую через `windows` 0.62 (та же версия, что у Tauri):
  - SMTC: `ISystemMediaTransportControlsInterop::GetForWindow(hwnd)`, кнопки, метаданные,
    обложка, таймлайн (каждые ~5 с и на изменениях), `MinSeekTime/MaxSeekTime`.
    Медиаклавиши Windows маршрутизирует в активную SMTC-сессию.
  - Кнопки на миниатюре: `ITaskbarList3::ThumbBarAddButtons` после `TaskbarButtonCreated`,
    `SetWindowSubclass` на HWND окна для `THBN_CLICKED`.
- **Alternatives**: `souvlaki` 0.8.3 (тянет `windows` 0.44 — второй копией).

## R9. Тексты (P3)

- **Decision**: lrclib.net (`/api/get` по artist/title/album/duration) → LRC, свой простой
  парсер синхронных строк. Только по кнопке, кэш в SQLite.

## Spike S1 — результаты на аккаунте пользователя (2026-10-17)

- **Поток**: yt-dlp 2026.08.19 + deno 2.9.7 (`--js-runtimes deno:<path>`) с cookies отдаёт
  itag 251 (webm/opus, 129 kbps). Полный файл 3,4 МБ скачан за 0,12 с через `&range=0-N`;
  HTTP `Range` тоже работает (206). Разрешение URL: 5,1 с на один трек, 9,5 с на три в одном
  вызове → предразрешение пачками в фоне. `loudness_db` в JSON yt-dlp нет → громкость
  только через ebur128 после загрузки (до замера gain = 0 dB).
- **yt-dlp перезаписывает файл cookies при выходе** → ему всегда передаётся временная копия.
- **Авторизованный InnerTube** (WEB_REMIX, `Authorization: SAPISIDHASH … SAPISID1PHASH …
  SAPISID3PHASH …`, `X-Goog-AuthUser: 0`, Origin music.youtube.com): `logged_in=1`.
  - `next` RDAMVM: ~48 треков (+ continuation). Элементы бывают `playlistPanelVideoWrapperRenderer`
    (primary/counterpart = аудио/клип) → брать аудио-версию.
  - Чипы (hl=ru): «Сохранить» (действие, не фильтр), «Все», «Знакомое», «Популярное»,
    «Рекомендованное», «Редкие треки», «Вечеринка» + жанровые. «Знакомое» явно персонализировано.
  - `browse FEmusic_home`: первая страница — «Послушать ещё раз», «Видеоклипы для вас»,
    сезонная подборка; остальные шелфы — через continuation.
  - `browse VLLM` (лайки): 100 треков на первой странице.
- **Не проверено** (переносится в фазу Foundation): вход через окно WebView2, snap layouts
  `tauri-plugin-frame`, базовый RAM.
