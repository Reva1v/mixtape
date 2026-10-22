//! Phone ↔ PC exchange of the play log (specs 007): pairing by a one-time code shown as a QR
//! code, then an encrypted TCP session that sends each side the plays it lacks.
//!
//! The PC listens on [`PORT`] and announces itself by mDNS ([`mdns`]); the phone dials it.
//! The functions here take any byte stream ([`wire::Stream`]), so the apps own the sockets,
//! timeouts and retries. The stored side (pairing, cursors, merging) is
//! `mixtape_library::peer_store`.

pub mod code;
pub mod exchange;
pub mod mdns;
pub mod wire;

pub use code::PairCode;
pub use exchange::{Report, Served, exchange, pair, serve, unpair};
pub use mixtape_library::peer_store::Pairing;
pub use wire::{Refusal, Session};

use mixtape_core::{AppError, ErrorKind, tr, trf};

/// The PC's TCP port.
pub const PORT: u16 = 47821;

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("{}: {}", tr("network error", "помилка мережі", "ошибка сети"), .0)]
    Io(std::io::Error),
    #[error("{}", tr("the other device closed the connection", "інший пристрій розірвав з'єднання", "другое устройство разорвало соединение"))]
    Closed,
    #[error("{}", tr(
        "the devices' keys don't match: pair them again",
        "ключі пристроїв не збігаються: зв'яжіть їх знову",
        "ключи устройств не совпадают: свяжите их заново"
    ))]
    BadKey,
    #[error("{}", tr(
        "the PC doesn't know this device: pair them again",
        "ПК не знає цього пристрою: зв'яжіть їх знову",
        "ПК не знает это устройство: свяжите их заново"
    ))]
    NotPaired,
    #[error("{}", tr("The code has expired, show a new one", "Код застарів, покажіть новий", "Код устарел, покажите новый"))]
    Expired,
    #[error("{}", tr("This is not a Mixtape pairing code", "Це не код зв'язування Mixtape", "Это не код связки Mixtape"))]
    BadCode,
    #[error("{}", refusal_text(*.0))]
    Refused(Refusal),
    #[error("{}: {}", tr("unexpected answer from the other device", "неочікувана відповідь іншого пристрою", "неожиданный ответ другого устройства"), .0)]
    Protocol(String),
    #[error("{0}")]
    Library(#[from] mixtape_library::Error),
}

fn refusal_text(r: Refusal) -> &'static str {
    match r {
        Refusal::UnknownDevice => tr(
            "the PC doesn't know this device: pair them again",
            "ПК не знає цього пристрою: зв'яжіть їх знову",
            "ПК не знает это устройство: свяжите их заново",
        ),
        Refusal::NoCode => tr(
            "the PC is not waiting for a pairing: show the code again",
            "ПК не чекає зв'язування: покажіть код ще раз",
            "ПК не ждёт связки: покажите код ещё раз",
        ),
        Refusal::Expired => tr("The code has expired, show a new one", "Код застарів, покажіть новий", "Код устарел, покажите новый"),
        Refusal::Version => tr(
            "the other device runs an incompatible Mixtape version",
            "на іншому пристрої несумісна версія Mixtape",
            "на другом устройстве несовместимая версия Mixtape",
        ),
    }
}

impl From<std::io::Error> for PeerError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        match e.kind() {
            K::UnexpectedEof | K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe => Self::Closed,
            _ => Self::Io(e),
        }
    }
}

impl PeerError {
    /// The pairing is gone or broken on the other side: only pairing again helps.
    pub fn needs_pairing(&self) -> bool {
        matches!(self, Self::BadKey | Self::NotPaired | Self::Refused(Refusal::UnknownDevice))
    }
}

impl From<PeerError> for AppError {
    fn from(e: PeerError) -> Self {
        let kind = match e {
            PeerError::Io(_) | PeerError::Closed => ErrorKind::Network,
            PeerError::BadKey | PeerError::NotPaired | PeerError::Refused(_) => ErrorKind::Auth,
            PeerError::Expired | PeerError::BadCode => ErrorKind::Invalid,
            PeerError::Protocol(_) => ErrorKind::Internal,
            PeerError::Library(_) => ErrorKind::Storage,
        };
        if kind == ErrorKind::Invalid {
            // About the scanned code: the message stands on its own.
            return AppError::new(kind, e.to_string());
        }
        AppError::new(
            kind,
            trf!("Sync with the other device failed: {e}", "Синхронізація з іншим пристроєм не вдалася: {e}", "Синхронизация с другим устройством не удалась: {e}"),
        )
    }
}

/// Now, Unix seconds.
pub(crate) fn now_s() -> i64 {
    mixtape_library::repo::now_ms().div_euclid(1000)
}
