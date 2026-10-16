//! Manual check of SMTC and the thumbnail toolbar: opens a small window for 20 s,
//! publishes a fake track and prints every command received.
//!
//! `cargo run -p mixtape-win --example smtc_demo [-- path\to\cover.jpg]`
//! Then press the media keys, use the volume flyout / Win+A media card, or hover the
//! taskbar button for the prev / play-pause / next toolbar.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    demo::run()
}

#[cfg(windows)]
mod demo {
    use mixtape_win::{MediaCommand, Smtc, ThumbBar};
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Instant;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetMessageW, MSG, PostQuitMessage, RegisterClassW, SW_SHOW, SetTimer, ShowWindow,
        TranslateMessage, WINDOW_EX_STYLE, WM_DESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    const RUN_FOR_MS: u64 = 20_000;
    const DURATION_MS: u64 = 180_000;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let hwnd = create_window()?;
        let (tx, rx) = channel();
        let tx2 = tx.clone();
        let thumbbar = ThumbBar::install(hwnd.0 as isize, move |cmd| {
            println!("thumbbar: {cmd:?}");
            let _ = tx.send(cmd);
        })?;
        let smtc = Smtc::new(hwnd.0 as isize, move |cmd| {
            println!("smtc:     {cmd:?}");
            let _ = tx2.send(cmd);
        })?;
        let cover = std::env::args_os().nth(1).map(PathBuf::from);
        smtc.set_metadata("Demo Track", "Mixtape", "Smoke Test", cover.as_deref())?;
        smtc.set_playback(true)?;
        smtc.set_timeline(0, DURATION_MS)?;
        thumbbar.set_playing(true)?;
        println!(
            "running for {} s: press media keys / thumbnail buttons",
            RUN_FOR_MS / 1000
        );

        let mut player = Player {
            playing: true,
            position_ms: 0,
            last: Instant::now(),
            rx,
        };
        let started = Instant::now();
        unsafe { SetTimer(Some(hwnd), 1, 250, None) };
        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            player.tick(&smtc, &thumbbar)?;
            if started.elapsed().as_millis() as u64 >= RUN_FOR_MS {
                drop(thumbbar);
                smtc.clear()?;
                unsafe { DestroyWindow(hwnd)? };
                // Drain until WM_QUIT so the uninstall message and WM_NCDESTROY run.
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                    unsafe { DispatchMessageW(&msg) };
                }
                break;
            }
        }
        println!("done");
        Ok(())
    }

    struct Player {
        playing: bool,
        position_ms: u64,
        last: Instant,
        rx: Receiver<MediaCommand>,
    }

    impl Player {
        fn tick(&mut self, smtc: &Smtc, thumbbar: &ThumbBar) -> Result<(), mixtape_win::Error> {
            if self.playing {
                self.position_ms =
                    (self.position_ms + self.last.elapsed().as_millis() as u64).min(DURATION_MS);
            }
            self.last = Instant::now();
            let mut changed = false;
            while let Ok(cmd) = self.rx.try_recv() {
                changed = true;
                match cmd {
                    MediaCommand::Play => self.playing = true,
                    MediaCommand::Pause | MediaCommand::Stop => self.playing = false,
                    MediaCommand::Toggle => self.playing = !self.playing,
                    MediaCommand::Seek { position_ms } => self.position_ms = position_ms,
                    MediaCommand::Next | MediaCommand::Previous => self.position_ms = 0,
                }
            }
            if changed {
                smtc.set_playback(self.playing)?;
                smtc.set_timeline(self.position_ms, DURATION_MS)?;
                thumbbar.set_playing(self.playing)?;
            }
            Ok(())
        }
    }

    fn create_window() -> windows::core::Result<HWND> {
        unsafe {
            let instance = GetModuleHandleW(None)?.into();
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: w!("MixtapeSmtcDemo"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("MixtapeSmtcDemo"),
                w!("Mixtape SMTC demo"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                420,
                200,
                None,
                None,
                Some(instance),
                None,
            )?;
            let _ = ShowWindow(hwnd, SW_SHOW);
            Ok(hwnd)
        }
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_DESTROY {
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}
