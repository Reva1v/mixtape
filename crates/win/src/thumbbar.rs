//! Taskbar thumbnail toolbar: previous / play-pause / next under the window preview.
//!
//! The buttons live in `ITaskbarList3` and report clicks as `WM_COMMAND` to the window,
//! so the window is subclassed (`SetWindowSubclass`). All toolbar state is owned by the
//! subclass and touched only on the UI thread; the public handle talks to it with
//! posted messages.

use crate::glyph::{self, Glyph};
use crate::{Error, MediaCommand};
use mixtape_core::tr;
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ITaskbarList3, RemoveWindowSubclass, SetWindowSubclass, THB_FLAGS, THB_ICON,
    THB_TOOLTIP, THBF_ENABLED, THBN_CLICKED, THUMBBUTTON, TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateIconIndirect, DestroyIcon, HICON, ICONINFO, MSGFLT_ALLOW,
    PostMessageW, RegisterWindowMessageW, SM_CXSMICON, WM_COMMAND, WM_NCDESTROY,
};
use windows::core::{PCWSTR, w};

const SUBCLASS_ID: usize = 0x4D58_5442;
const ID_PREVIOUS: u32 = 1;
const ID_TOGGLE: u32 = 2;
const ID_NEXT: u32 = 3;

/// Handle to a window's thumbnail toolbar.
///
/// [`ThumbBar::install`] must run on the window's UI thread (in Tauri:
/// `run_on_main_thread`). The handle itself is `Send + Sync`: `set_playing` and `Drop`
/// post messages to that thread. `on_command` runs on the UI thread.
pub struct ThumbBar {
    hwnd: isize,
    messages: Messages,
    /// Address of the subclass state; identifies this install in the uninstall message.
    state: usize,
}

#[derive(Clone, Copy)]
struct Messages {
    button_created: u32,
    set_playing: u32,
    uninstall: u32,
}

impl ThumbBar {
    /// Subclasses `hwnd` and adds the buttons, now if the taskbar button already exists
    /// or when `TaskbarButtonCreated` arrives (also after an Explorer restart).
    /// One toolbar per window.
    pub fn install(
        hwnd: isize,
        on_command: impl Fn(MediaCommand) + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let hwnd = HWND(hwnd as *mut _);
        unsafe {
            // Tauri's UI thread already is an STA; then this is a no-op (S_FALSE).
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let taskbar: ITaskbarList3 =
                CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)?;
            taskbar.HrInit()?;
            let messages = Messages {
                button_created: register(w!("TaskbarButtonCreated"))?,
                set_playing: register(w!("Mixtape.ThumbBar.SetPlaying"))?,
                uninstall: register(w!("Mixtape.ThumbBar.Uninstall"))?,
            };
            // Lets Explorer's message through when the app runs elevated.
            let _ = ChangeWindowMessageFilterEx(hwnd, messages.button_created, MSGFLT_ALLOW, None);
            let icons = Icons::new(icon_size(hwnd))?;
            let state = Box::into_raw(Box::new(State {
                taskbar,
                hwnd,
                messages,
                icons,
                playing: Cell::new(false),
                on_command: Box::new(on_command),
            }));
            if !SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, state as usize).as_bool()
            {
                drop(Box::from_raw(state));
                return Err(windows::core::Error::from_thread().into());
            }
            (*state).add_buttons();
            Ok(Self {
                hwnd: hwnd.0 as isize,
                messages,
                state: state as usize,
            })
        }
    }

    /// Swaps the middle button between play and pause.
    pub fn set_playing(&self, playing: bool) -> Result<(), Error> {
        Ok(self.post(self.messages.set_playing, playing as usize)?)
    }

    fn post(&self, msg: u32, wparam: usize) -> windows::core::Result<()> {
        unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                msg,
                WPARAM(wparam),
                LPARAM(0),
            )
        }
    }
}

impl Drop for ThumbBar {
    fn drop(&mut self) {
        // Fails only if the window is gone, and then WM_NCDESTROY already cleaned up.
        let _ = self.post(self.messages.uninstall, self.state);
    }
}

struct State {
    taskbar: ITaskbarList3,
    hwnd: HWND,
    messages: Messages,
    icons: Icons,
    playing: Cell<bool>,
    on_command: Box<dyn Fn(MediaCommand)>,
}

impl State {
    fn buttons(&self) -> [THUMBBUTTON; 3] {
        let i = &self.icons;
        let (toggle_icon, toggle_tip) = if self.playing.get() {
            (i.pause, tr("Pause", "Пауза", "Пауза"))
        } else {
            (i.play, tr("Play", "Відтворити", "Играть"))
        };
        [
            button(ID_PREVIOUS, i.previous, tr("Previous", "Попередній", "Предыдущий")),
            button(ID_TOGGLE, toggle_icon, toggle_tip),
            button(ID_NEXT, i.next, tr("Next", "Наступний", "Следующий")),
        ]
    }

    fn add_buttons(&self) {
        let buttons = self.buttons();
        // Add works once per taskbar button; a repeat (Explorer restart) falls back to update.
        let result = unsafe {
            self.taskbar
                .ThumbBarAddButtons(self.hwnd, &buttons)
                .or_else(|_| self.taskbar.ThumbBarUpdateButtons(self.hwnd, &buttons))
        };
        if let Err(e) = result {
            tracing::debug!("thumbbar: buttons not added yet: {e}");
        }
    }

    fn set_playing(&self, playing: bool) {
        self.playing.set(playing);
        let toggle = self.buttons()[1];
        if let Err(e) = unsafe { self.taskbar.ThumbBarUpdateButtons(self.hwnd, &[toggle]) } {
            tracing::debug!("thumbbar: update failed: {e}");
        }
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let ptr = data as *mut State;
    // SAFETY: `data` is the box leaked in `install`, freed only below after the subclass
    // is removed, so it's alive for every message this proc sees.
    let state = unsafe { &*ptr };
    let m = state.messages;
    if msg == WM_COMMAND && hiword(wparam.0) == THBN_CLICKED {
        if let Some(cmd) = button_command(loword(wparam.0)) {
            (state.on_command)(cmd);
            return LRESULT(0);
        }
    } else if msg == m.button_created {
        state.add_buttons();
    } else if msg == m.set_playing {
        state.set_playing(wparam.0 != 0);
        return LRESULT(0);
    } else if (msg == m.uninstall && wparam.0 == data) || msg == WM_NCDESTROY {
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            drop(Box::from_raw(ptr));
        }
        if msg == m.uninstall {
            return LRESULT(0);
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn button_command(id: u32) -> Option<MediaCommand> {
    match id {
        ID_PREVIOUS => Some(MediaCommand::Previous),
        ID_TOGGLE => Some(MediaCommand::Toggle),
        ID_NEXT => Some(MediaCommand::Next),
        _ => None,
    }
}

fn hiword(v: usize) -> u32 {
    (v >> 16) as u32 & 0xffff
}

fn loword(v: usize) -> u32 {
    v as u32 & 0xffff
}

fn button(id: u32, icon: HICON, tip: &str) -> THUMBBUTTON {
    let mut b = THUMBBUTTON {
        dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
        iId: id,
        hIcon: icon,
        dwFlags: THBF_ENABLED,
        ..Default::default()
    };
    // Leave the last slot as the terminating NUL.
    let max = b.szTip.len() - 1;
    for (dst, src) in b.szTip[..max].iter_mut().zip(tip.encode_utf16()) {
        *dst = src;
    }
    b
}

fn register(name: PCWSTR) -> windows::core::Result<u32> {
    match unsafe { RegisterWindowMessageW(name) } {
        0 => Err(windows::core::Error::from_thread()),
        id => Ok(id),
    }
}

/// Small-icon size at the window's DPI (16 px at 100 %).
fn icon_size(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let size = if dpi == 0 {
        0
    } else {
        unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }
    };
    if size > 0 { size } else { 16 }
}

struct Icons {
    previous: HICON,
    play: HICON,
    pause: HICON,
    next: HICON,
}

impl Icons {
    fn new(size: i32) -> windows::core::Result<Self> {
        let mut icons = Self {
            previous: HICON::default(),
            play: HICON::default(),
            pause: HICON::default(),
            next: HICON::default(),
        };
        // Filled one by one so `Drop` frees the already created ones on error.
        icons.previous = create_icon(Glyph::Previous, size)?;
        icons.play = create_icon(Glyph::Play, size)?;
        icons.pause = create_icon(Glyph::Pause, size)?;
        icons.next = create_icon(Glyph::Next, size)?;
        Ok(icons)
    }
}

impl Drop for Icons {
    fn drop(&mut self) {
        for icon in [self.previous, self.play, self.pause, self.next] {
            if !icon.is_invalid() {
                let _ = unsafe { DestroyIcon(icon) };
            }
        }
    }
}

/// 32-bit alpha icon from a rendered glyph; the 1-bit mask is all zeros (alpha wins).
fn create_icon(glyph: Glyph, size: i32) -> windows::core::Result<HICON> {
    let pixels = glyph::render(glyph, size as u32);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mask_bits = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
    unsafe {
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_ids_map_to_commands() {
        assert_eq!(button_command(ID_PREVIOUS), Some(MediaCommand::Previous));
        assert_eq!(button_command(ID_TOGGLE), Some(MediaCommand::Toggle));
        assert_eq!(button_command(ID_NEXT), Some(MediaCommand::Next));
        assert_eq!(button_command(0), None);
    }

    #[test]
    fn wparam_words_split() {
        let wparam = ((THBN_CLICKED as usize) << 16) | ID_NEXT as usize;
        assert_eq!(hiword(wparam), THBN_CLICKED);
        assert_eq!(loword(wparam), ID_NEXT);
    }

    #[test]
    fn tooltip_is_nul_terminated_utf16() {
        let b = button(ID_TOGGLE, HICON::default(), "Пауза");
        let len = b.szTip.iter().position(|&c| c == 0).unwrap();
        assert_eq!(String::from_utf16(&b.szTip[..len]).unwrap(), "Пауза");
    }

    #[test]
    fn icons_are_created_without_a_window() {
        for size in [16, 20, 24, 32] {
            drop(Icons::new(size).unwrap());
        }
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn handle_is_send_sync() {
        assert_send_sync::<ThumbBar>();
    }
}
