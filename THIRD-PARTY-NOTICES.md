# Third-party notices

Mixtape is licensed under the GNU General Public License v3.0 or later (see [`LICENSE`](LICENSE)).
It ships with, or is built from, the third-party components below. Each remains under its own
license; this file is installed next to the app.

## Bundled programs

| Component | Version | License | Source |
|---|---|---|---|
| yt-dlp (`yt-dlp.exe`, sidecar) | 2026.08.19 | [Unlicense](https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE) | https://github.com/yt-dlp/yt-dlp |
| Deno (`deno.exe`, sidecar, used by yt-dlp) | 2.9.7 | [MIT](https://github.com/denoland/deno/blob/main/LICENSE.md) | https://github.com/denoland/deno |

The official `yt-dlp.exe` is a PyInstaller build that also contains Python and yt-dlp's Python
dependencies, each under its own license; yt-dlp lists them in
[`THIRD_PARTY_LICENSES.txt`](https://github.com/yt-dlp/yt-dlp/blob/master/THIRD_PARTY_LICENSES.txt)
attached to each release. Deno likewise lists its dependencies in its repository.

## Built into the app

| Component | License | Notes |
|---|---|---|
| libopus (via `opusic-sys`) | BSD-3-Clause | © Xiph.Org Foundation, Skype Limited, Octasic, Jean-Marc Valin, Timothy B. Terriberry, CSIRO, Gregory Maxwell, Mark Borgerding, Erik de Castro Lopo, Mozilla, Amazon. https://opus-codec.org/license/ |
| Inter (font, `@fontsource-variable/inter`) | SIL Open Font License 1.1 | © The Inter Project Authors (Rasmus Andersson). https://github.com/rsms/inter |
| Lucide icons (`@iconify-json/lucide`) | ISC | © Lucide Contributors; parts © Cole Bemis (Feather, MIT). https://lucide.dev/license |
| Phosphor icons (`@iconify-json/ph`) | MIT | © Phosphor Icons. https://github.com/phosphor-icons/core |
| Solar icons (`@iconify-json/solar`) | CC BY 4.0 | **Attribution:** "Solar Icons Set" by 480 Design (https://www.figma.com/community/file/1166831539721848736), licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Icons are used unchanged apart from size and colour. |
| Svelte, motion, virtua, Tailwind CSS, `@tauri-apps/api` | MIT (`@tauri-apps/api`: MIT or Apache-2.0) | JavaScript bundled into the interface. |

### BSD-3-Clause (libopus)

```
Copyright 2001-2023 Xiph.Org, Skype Limited, Octasic,
                    Jean-Marc Valin, Timothy B. Terriberry,
                    CSIRO, Gregory Maxwell, Mark Borgerding,
                    Erik de Castro Lopo, Mozilla, Amazon

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

- Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

- Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

- Neither the name of Internet Society, IETF or IETF Trust, nor the
names of specific contributors, may be used to endorse or promote
products derived from this software without specific prior written
permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER
OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### ISC (Lucide)

```
ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT).
All other copyright (c) for Lucide are held by Lucide Contributors 2022.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### MIT (Phosphor, Deno and the JavaScript libraries above)

```
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The SIL Open Font License 1.1 (Inter) is at https://openfontlicense.org; CC BY 4.0 (Solar)
at https://creativecommons.org/licenses/by/4.0/legalcode.

## Rust crates

The app is built from many Rust crates (Tauri, symphonia, cpal, rusqlite with bundled SQLite,
reqwest, image, tokio, serde and their dependencies), under permissive licenses — mostly MIT,
Apache-2.0, BSD, ISC, Zlib, Unicode-3.0 and MPL-2.0 — all compatible with the GPL-3.0. The full
list with license texts is generated from `Cargo.lock`:

```powershell
cargo install cargo-about --locked
cargo about init                                  # writes about.toml and about.hbs once
cargo about generate about.hbs -o rust-licenses.html
```

Release maintainers should regenerate it for each release and attach it to the release, or
paste it below this section.
