//! DPAPI (`CryptProtectData`): secrets encrypted for the current Windows user, so the
//! stored YouTube cookies are unreadable from another account or another machine.

use crate::Error;
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

pub fn protect(data: &[u8]) -> Result<Vec<u8>, Error> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` borrows `data` for the call; `out` is allocated by the system and freed below.
    unsafe {
        CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)?;
        Ok(take(out))
    }
}

pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, Error> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)?;
        Ok(take(out))
    }
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

/// Copies a system-allocated blob out and frees it.
unsafe fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: the caller passes a blob just filled by CryptProtectData / CryptUnprotectData.
    unsafe {
        let bytes = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(blob.pbData.cast())));
        bytes
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let secret = b"SAPISID=value; SID=other";
        let sealed = super::protect(secret).unwrap();
        assert_ne!(sealed.as_slice(), secret);
        assert_eq!(super::unprotect(&sealed).unwrap(), secret);
    }
}
