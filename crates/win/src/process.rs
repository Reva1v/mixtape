//! Finding a running program of this Windows session by its file name (Droplet Island, for
//! handing it `mixtape://` links). Read-only: nothing is opened beyond "query limited information".

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Win32 call and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn session_of(pid: u32) -> Option<u32> {
    let mut id = 0u32;
    // SAFETY: `id` is a valid out pointer.
    unsafe { ProcessIdToSessionId(pid, &mut id) }.ok()?;
    Some(id)
}

fn path_of(pid: u32) -> Option<PathBuf> {
    // SAFETY: plain Win32 calls; the buffer outlives the call and `len` holds its size.
    unsafe {
        let h = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
            .ok()?;
        Some(PathBuf::from(OsString::from_wide(&buf[..len as usize])))
    }
}

fn exe_name(e: &PROCESSENTRY32W) -> String {
    let n = e
        .szExeFile
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(e.szExeFile.len());
    String::from_utf16_lossy(&e.szExeFile[..n])
}

/// The full path of the first process named `exe_name` (case ignored) in this session, if its
/// path can be read. Any failure is `None`: callers fall back to doing the work themselves.
pub fn find_in_session(exe_name_wanted: &str) -> Option<PathBuf> {
    if exe_name_wanted.is_empty() {
        return None;
    }
    // SAFETY: no arguments besides the current pid.
    let mine = session_of(unsafe { GetCurrentProcessId() })?;
    // SAFETY: the snapshot handle is closed by `Handle`; the entry has its size set as required.
    unsafe {
        let snap = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?);
        let mut e = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snap.0, &mut e).is_ok();
        while more {
            if exe_name(&e).eq_ignore_ascii_case(exe_name_wanted)
                && session_of(e.th32ProcessID) == Some(mine)
                && let Some(p) = path_of(e.th32ProcessID)
            {
                return Some(p);
            }
            more = Process32NextW(snap.0, &mut e).is_ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> (PathBuf, String) {
        let p = std::env::current_exe().unwrap();
        let n = p.file_name().unwrap().to_string_lossy().into_owned();
        (p, n)
    }

    #[test]
    fn finds_this_test_binary() {
        let (path, name) = me();
        let found = find_in_session(&name).expect("the test process itself");
        assert_eq!(found.file_name(), path.file_name());
    }

    #[test]
    fn name_ignores_case() {
        let (_, name) = me();
        assert!(find_in_session(&name.to_uppercase()).is_some());
    }

    #[test]
    fn unknown_or_empty_name_is_none() {
        assert!(find_in_session("no-such-program-5f3a9c.exe").is_none());
        assert!(find_in_session("").is_none());
    }

    #[test]
    fn a_prefix_is_not_a_match() {
        let (_, name) = me();
        assert!(find_in_session(&name[..name.len() - 4]).is_none());
    }
}
