//! Frames, handshake and encryption of the peer protocol.
//!
//! Every frame is a big-endian `u32` length and that many bytes. The two `Hello`s go in the
//! clear (JSON); after them both sides derive two keys,
//! `HKDF-SHA256(psk, salt = nonce_c ‖ nonce_s, info = "mixtape-peer-1")` → 64 bytes, the first
//! half for client → server and the second for server → client. Every later frame is
//! `nonce (12) ‖ ChaCha20-Poly1305 ciphertext` of a JSON [`Msg`]; the nonce is a per-direction
//! counter, so a replayed, dropped or reordered frame fails like a wrong key does.

use super::PeerError;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use mixtape_library::peer_store::{Cursors, PeerEvent};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Protocol version in `Hello`.
pub const VERSION: u32 = 1;
/// The largest frame accepted: a batch of 1 000 events is ~300 KB.
pub const MAX_FRAME: usize = 8 << 20;
const INFO: &[u8] = b"mixtape-peer-1";

/// What a stream needs to carry a session.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// The handshake frames, in the clear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Clear {
    Hello {
        v: u32,
        /// The sender's `settings.device_id`.
        device: String,
        /// 32 random bytes, base64url.
        nonce: String,
        /// The client pairs with the one-time code key instead of exchanging.
        #[serde(default)]
        pair: bool,
    },
    /// The server's answer instead of `Hello` when it won't talk.
    Refused { reason: Refusal },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Refusal {
    /// Not paired with this device (it was unpaired on the PC).
    UnknownDevice,
    /// A pairing attempt while no code is shown.
    NoCode,
    /// A pairing attempt with a code past its time.
    Expired,
    /// Another protocol version.
    Version,
}

/// The encrypted messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Msg {
    /// Client → server while pairing: who is asking.
    Pair { device: String, name: String },
    /// Server → client: the pairing is stored.
    Paired { device: String, name: String },
    /// Client → server: what the client has.
    Cursors { cursors: Cursors },
    /// Up to [`mixtape_library::peer_store::BATCH`] plays.
    Events { events: Vec<PeerEvent> },
    /// The sender has sent all it had. The server's carries its cursors, for the client's half.
    Done {
        #[serde(default)]
        cursors: Option<Cursors>,
    },
    /// Server → client: the client's plays are stored (or the pairing removed).
    Ack,
    /// Client → server: forget the pairing.
    Unpair,
}

/// An encrypted connection after the handshake.
pub struct Session<S> {
    stream: S,
    send: ChaCha20Poly1305,
    recv: ChaCha20Poly1305,
    sent: u64,
    received: u64,
    /// The other side's device id, from its `Hello`.
    pub peer_device: String,
}

impl<S: Stream> Session<S> {
    /// Opens a session from the client side: sends `Hello`, reads the answer.
    pub async fn client(mut stream: S, device: &str, psk: &[u8; 32], pair: bool) -> Result<Self, PeerError> {
        let nonce_c = random32()?;
        write_clear(&mut stream, &hello(device, &nonce_c, pair)).await?;
        match read_clear(&mut stream).await? {
            Clear::Hello { v, device: peer, nonce, .. } => {
                if v != VERSION {
                    return Err(PeerError::Refused(Refusal::Version));
                }
                let nonce_s = decode_nonce(&nonce)?;
                let (c2s, s2c) = derive(psk, &nonce_c, &nonce_s);
                Ok(Self::new(stream, c2s, s2c, peer))
            }
            Clear::Refused { reason } => Err(PeerError::Refused(reason)),
        }
    }

    /// Answers the client's `hello` (read with [`read_hello`]) and opens the session.
    pub async fn server(mut stream: S, hello_c: &Hello, device: &str, psk: &[u8; 32]) -> Result<Self, PeerError> {
        let nonce_s = random32()?;
        write_clear(&mut stream, &hello(device, &nonce_s, hello_c.pair)).await?;
        let (c2s, s2c) = derive(psk, &hello_c.nonce, &nonce_s);
        Ok(Self::new(stream, s2c, c2s, hello_c.device.clone()))
    }

    fn new(stream: S, send: [u8; 32], recv: [u8; 32], peer_device: String) -> Self {
        Self {
            stream,
            send: ChaCha20Poly1305::new(Key::from_slice(&send)),
            recv: ChaCha20Poly1305::new(Key::from_slice(&recv)),
            sent: 0,
            received: 0,
            peer_device,
        }
    }

    pub async fn send(&mut self, msg: &Msg) -> Result<(), PeerError> {
        let plain = serde_json::to_vec(msg).map_err(|e| PeerError::Protocol(e.to_string()))?;
        let nonce = counter_nonce(self.sent);
        self.sent += 1;
        let sealed = self
            .send
            .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
            .map_err(|_| PeerError::Protocol("encryption failed".into()))?;
        let mut frame = Vec::with_capacity(12 + sealed.len());
        frame.extend_from_slice(&nonce);
        frame.extend_from_slice(&sealed);
        write_frame(&mut self.stream, &frame).await?;
        Ok(())
    }

    /// The next message. A frame that doesn't open with the expected key and counter is
    /// [`PeerError::BadKey`].
    pub async fn recv(&mut self) -> Result<Msg, PeerError> {
        let frame = read_frame(&mut self.stream).await?;
        let expected = counter_nonce(self.received);
        if frame.len() < 12 + 16 || frame[..12] != expected {
            return Err(PeerError::BadKey);
        }
        let plain = self
            .recv
            .decrypt(Nonce::from_slice(&expected), &frame[12..])
            .map_err(|_| PeerError::BadKey)?;
        self.received += 1;
        serde_json::from_slice(&plain).map_err(|e| PeerError::Protocol(e.to_string()))
    }
}

/// A client's `Hello` as the server sees it.
#[derive(Debug, Clone)]
pub struct Hello {
    pub device: String,
    pub nonce: [u8; 32],
    pub pair: bool,
}

/// Reads the client's `Hello`; the server then picks the key and calls [`Session::server`]
/// or [`refuse`].
pub async fn read_hello<S: Stream>(stream: &mut S) -> Result<Hello, PeerError> {
    match read_clear(stream).await? {
        Clear::Hello { v, device, nonce, pair } => {
            if v != VERSION {
                refuse(stream, Refusal::Version).await?;
                return Err(PeerError::Refused(Refusal::Version));
            }
            Ok(Hello { device, nonce: decode_nonce(&nonce)?, pair })
        }
        Clear::Refused { .. } => Err(PeerError::Protocol("a refusal from the client".into())),
    }
}

/// Answers `Refused` and flushes.
pub async fn refuse<S: Stream>(stream: &mut S, reason: Refusal) -> Result<(), PeerError> {
    write_clear(stream, &Clear::Refused { reason }).await
}

fn hello(device: &str, nonce: &[u8; 32], pair: bool) -> Clear {
    Clear::Hello {
        v: VERSION,
        device: device.to_owned(),
        nonce: URL_SAFE_NO_PAD.encode(nonce),
        pair,
    }
}

/// The client → server and server → client keys.
fn derive(psk: &[u8; 32], nonce_c: &[u8; 32], nonce_s: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(nonce_c);
    salt[32..].copy_from_slice(nonce_s);
    let mut okm = [0u8; 64];
    Hkdf::<Sha256>::new(Some(&salt), psk)
        .expand(INFO, &mut okm)
        .expect("64 bytes is a valid HKDF-SHA256 length");
    let (mut c2s, mut s2c) = ([0u8; 32], [0u8; 32]);
    c2s.copy_from_slice(&okm[..32]);
    s2c.copy_from_slice(&okm[32..]);
    (c2s, s2c)
}

fn counter_nonce(n: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&n.to_be_bytes());
    nonce
}

/// 32 bytes from the system's random source.
pub fn random32() -> Result<[u8; 32], PeerError> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| PeerError::Io(std::io::Error::other(e.to_string())))?;
    Ok(b)
}

fn decode_nonce(s: &str) -> Result<[u8; 32], PeerError> {
    URL_SAFE_NO_PAD
        .decode(s)
        .ok()
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
        .ok_or_else(|| PeerError::Protocol("bad handshake nonce".into()))
}

async fn write_clear<S: Stream>(stream: &mut S, msg: &Clear) -> Result<(), PeerError> {
    let bytes = serde_json::to_vec(msg).map_err(|e| PeerError::Protocol(e.to_string()))?;
    write_frame(stream, &bytes).await
}

async fn read_clear<S: Stream>(stream: &mut S) -> Result<Clear, PeerError> {
    let frame = read_frame(stream).await?;
    serde_json::from_slice(&frame).map_err(|e| PeerError::Protocol(e.to_string()))
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) -> Result<(), PeerError> {
    if bytes.len() > MAX_FRAME {
        return Err(PeerError::Protocol("frame too large".into()));
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(bytes).await?;
    w.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<u8>, PeerError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(PeerError::Protocol("frame too large".into()));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn messages_roundtrip_with_a_key_per_direction() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let psk = [7u8; 32];
        let server = async move {
            let mut b = b;
            let h = read_hello(&mut b).await?;
            assert_eq!(h.device, "phone");
            let mut s = Session::server(b, &h, "pc", &psk).await?;
            let m = s.recv().await?;
            s.send(&Msg::Ack).await?;
            Ok::<_, PeerError>((m, s.peer_device))
        };
        let client = async move {
            let mut c = Session::client(a, "phone", &psk, false).await?;
            c.send(&Msg::Done { cursors: None }).await?;
            let m = c.recv().await?;
            Ok::<_, PeerError>((m, c.peer_device))
        };
        let (s, c) = tokio::join!(server, client);
        assert_eq!(s.unwrap(), (Msg::Done { cursors: None }, "phone".to_owned()));
        assert_eq!(c.unwrap(), (Msg::Ack, "pc".to_owned()));

        // Different directions use different keys.
        let (c2s, s2c) = derive(&psk, &[1; 32], &[2; 32]);
        assert_ne!(c2s, s2c);
        assert_ne!(derive(&psk, &[1; 32], &[3; 32]).0, c2s);
    }
}
