//! Start with Windows: a value under the per-user `Run` key
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`). Task Manager's «Startup apps»
//! can switch such an entry off without deleting it (`StartupApproved\Run`); a switched-off
//! entry reads as absent, and writing the entry again switches it back on.

use crate::Error;
use std::path::Path;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// The command line stored in the `Run` value: the quoted program path, then `args`.
pub fn command_line(exe: &Path, args: &[&str]) -> String {
    let mut cmd = format!("\"{}\"", exe.display());
    for a in args {
        cmd.push(' ');
        cmd.push_str(a);
    }
    cmd
}

/// The arguments of a `Run` command line, without the program path (quoted or not).
pub fn command_args(cmd: &str) -> Vec<&str> {
    let cmd = cmd.trim_start();
    let rest = match cmd.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').map_or("", |(_, rest)| rest),
        None => cmd
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest),
    };
    rest.split_whitespace().collect()
}

/// Whether a `StartupApproved` record says the entry is switched off: the low bit of its
/// first byte (02/06 = on, 03/07 = off).
pub fn approval_disabled(record: &[u8]) -> bool {
    record.first().is_some_and(|b| b & 1 == 1)
}

/// The command line Windows runs at sign-in for `name`, or `None` when there is no entry
/// or it is switched off in Task Manager.
pub fn get(name: &str) -> Result<Option<String>, Error> {
    #[cfg(windows)]
    {
        let Some(cmd) = imp::read_string(RUN_KEY, name)? else {
            return Ok(None);
        };
        if imp::read_binary(APPROVED_KEY, name)?.is_some_and(|r| approval_disabled(&r)) {
            return Ok(None);
        }
        Ok(Some(cmd))
    }
    #[cfg(not(windows))]
    {
        let _ = (name, RUN_KEY, APPROVED_KEY);
        Ok(None)
    }
}

/// Writes (`Some`) or removes (`None`) the entry. Writing also clears a Task Manager
/// «disabled» mark, so the entry really runs.
pub fn set(name: &str, command: Option<&str>) -> Result<(), Error> {
    #[cfg(windows)]
    {
        imp::delete(APPROVED_KEY, name)?;
        match command {
            Some(cmd) => imp::write_string(RUN_KEY, name, cmd),
            None => imp::delete(RUN_KEY, name),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (name, command);
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use crate::Error;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_ROUTINE_FLAGS, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows::core::HSTRING;

    fn check(err: WIN32_ERROR) -> Result<bool, Error> {
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        err.ok()?;
        Ok(true)
    }

    /// Raw value bytes, `None` when the key or the value is missing.
    fn read(key: &str, name: &str, flags: REG_ROUTINE_FLAGS) -> Result<Option<Vec<u8>>, Error> {
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        let mut size = 0u32;
        // SAFETY: a size query with no buffer.
        if !check(unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                flags,
                None,
                None,
                Some(&mut size),
            )
        })? {
            return Ok(None);
        }
        let mut buf = vec![0u8; size as usize];
        // SAFETY: `buf` holds `size` bytes, as the call is told.
        let found = check(unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                flags,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        })?;
        buf.truncate(size as usize);
        Ok(found.then_some(buf))
    }

    pub fn read_string(key: &str, name: &str) -> Result<Option<String>, Error> {
        Ok(read(key, name, RRF_RT_REG_SZ)?.map(|b| {
            let wide: Vec<u16> = b
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&wide)
                .trim_end_matches('\0')
                .to_string()
        }))
    }

    pub fn read_binary(key: &str, name: &str) -> Result<Option<Vec<u8>>, Error> {
        read(key, name, RRF_RT_REG_BINARY)
    }

    pub fn write_string(key: &str, name: &str, value: &str) -> Result<(), Error> {
        let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        // SAFETY: `wide` is a NUL-terminated UTF-16 string of the length passed.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                (wide.len() * 2) as u32,
            )
        }
        .ok()?;
        Ok(())
    }

    /// Removes a value; a missing one is not an error.
    pub fn delete(key: &str, name: &str) -> Result<(), Error> {
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        // SAFETY: plain registry call with owned strings.
        check(unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name) })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_parses_command_lines() {
        let exe = Path::new(r"C:\Program Files\Mixtape\mixtape.exe");
        let cmd = command_line(exe, &["--autostart", "--hidden"]);
        assert_eq!(
            cmd,
            r#""C:\Program Files\Mixtape\mixtape.exe" --autostart --hidden"#
        );
        assert_eq!(command_args(&cmd), ["--autostart", "--hidden"]);
        assert_eq!(command_args(&command_line(exe, &[])), Vec::<&str>::new());
        assert_eq!(command_args(r"C:\mixtape.exe --autostart"), ["--autostart"]);
        assert_eq!(command_args(r"C:\mixtape.exe"), Vec::<&str>::new());
        assert_eq!(command_args(r#""C:\unterminated"#), Vec::<&str>::new());
    }

    #[test]
    fn reads_task_manager_approval() {
        assert!(!approval_disabled(&[2, 0, 0, 0]));
        assert!(!approval_disabled(&[6, 0, 0, 0]));
        assert!(approval_disabled(&[3, 0, 0, 0, 1, 2]));
        assert!(!approval_disabled(&[]));
    }

    /// Writes and removes a value under a name only this test uses.
    #[cfg(windows)]
    #[test]
    fn registry_round_trip() {
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = set(&self.0, None);
            }
        }
        let name = format!("MixtapeAutostartTest{}", std::process::id());
        let _cleanup = Cleanup(name.clone());
        assert_eq!(get(&name).unwrap(), None);
        let cmd = command_line(Path::new(r"C:\Тест\mixtape.exe"), &["--autostart"]);
        set(&name, Some(&cmd)).unwrap();
        assert_eq!(get(&name).unwrap().as_deref(), Some(cmd.as_str()));
        set(&name, None).unwrap();
        assert_eq!(get(&name).unwrap(), None);
        set(&name, None).unwrap();
    }
}
