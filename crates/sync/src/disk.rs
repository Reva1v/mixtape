//! Free disk space checks: downloads pause when the data drive has less than
//! [`MIN_FREE`] left.

use std::path::Path;

/// Downloads pause below this much free space (2 GiB).
pub const MIN_FREE: u64 = 2 * 1024 * 1024 * 1024;

/// Bytes available to the current user on the volume holding `path`. The path itself
/// may not exist yet; its nearest existing ancestor is queried.
pub fn free_bytes(path: &Path) -> std::io::Result<u64> {
    let existing = path
        .ancestors()
        .find(|p| !p.as_os_str().is_empty() && p.exists())
        .ok_or_else(|| std::io::Error::new(
                std::io::ErrorKind::NotFound,
                mixtape_core::tr("path not found", "шлях не знайдено", "путь не найден"),
            ))?;
    imp::free_bytes(existing)
}

/// True when the volume holding `path` has less than [`MIN_FREE`] available. Errors
/// count as "enough space" so a failed check never blocks downloads.
pub fn low_space(path: &Path) -> bool {
    free_bytes(path).is_ok_and(|n| n < MIN_FREE)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;

    pub fn free_bytes(path: &Path) -> std::io::Result<u64> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut available = 0u64;
        // SAFETY: `wide` is NUL-terminated and outlives the call; the out pointer is valid.
        unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None) }
            .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
        Ok(available)
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    pub fn free_bytes(_path: &Path) -> std::io::Result<u64> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            mixtape_core::tr("Windows only", "лише Windows", "только Windows"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn temp_dir_has_free_space() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_bytes(dir.path()).unwrap() > 0);
        // A missing subfolder resolves to its existing parent.
        assert!(free_bytes(&dir.path().join("a/b/c.webm")).unwrap() > 0);
    }

    #[test]
    fn min_free_is_two_gib() {
        assert_eq!(MIN_FREE, 2_147_483_648);
    }
}
