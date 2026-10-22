//! The pairing code the PC shows as a QR code:
//! `mixtape://pair?v=1&k=<32 bytes base64url>&n=<PC name>&p=<port>&a=<addr1,addr2,…>&e=<expiry, Unix s>`.
//! `k` is a one-time key: the pairing handshake runs under it and it becomes the pair's key.

use super::{PeerError, wire};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// How long a code is good for, in seconds.
pub const CODE_TTL_S: i64 = 300;

#[derive(Clone, PartialEq, Eq)]
pub struct PairCode {
    pub key: [u8; 32],
    /// The PC's name.
    pub name: String,
    pub port: u16,
    /// The PC's addresses (IPs or host names, no port), best first.
    pub addresses: Vec<String>,
    /// Unix seconds.
    pub expires_at: i64,
}

impl std::fmt::Debug for PairCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairCode")
            .field("name", &self.name)
            .field("port", &self.port)
            .field("addresses", &self.addresses)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl PairCode {
    /// A fresh code with a random key, good for [`CODE_TTL_S`] from `now_s`.
    pub fn new(name: impl Into<String>, port: u16, addresses: Vec<String>, now_s: i64) -> Result<Self, PeerError> {
        Ok(Self {
            key: wire::random32()?,
            name: name.into(),
            port,
            addresses,
            expires_at: now_s + CODE_TTL_S,
        })
    }

    pub fn to_uri(&self) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("v", "1")
            .append_pair("k", &URL_SAFE_NO_PAD.encode(self.key))
            .append_pair("n", &self.name)
            .append_pair("p", &self.port.to_string())
            .append_pair("a", &self.addresses.join(","))
            .append_pair("e", &self.expires_at.to_string())
            .finish();
        format!("mixtape://pair?{query}")
    }

    /// Reads a scanned code. Doesn't look at the time: see [`check`](Self::check).
    pub fn parse(uri: &str) -> Result<Self, PeerError> {
        let url = url::Url::parse(uri.trim()).map_err(|_| PeerError::BadCode)?;
        if url.scheme() != "mixtape" || url.host_str() != Some("pair") {
            return Err(PeerError::BadCode);
        }
        let (mut v, mut key, mut name, mut port, mut addresses, mut expires_at) = (None, None, None, None, None, None);
        for (k, val) in url.query_pairs() {
            match &*k {
                "v" => v = Some(val.into_owned()),
                "k" => key = URL_SAFE_NO_PAD.decode(val.as_bytes()).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()),
                "n" => name = Some(val.into_owned()),
                "p" => port = val.parse::<u16>().ok(),
                "a" => {
                    addresses = Some(val.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect())
                }
                "e" => expires_at = val.parse::<i64>().ok(),
                _ => {}
            }
        }
        if v.as_deref() != Some("1") {
            return Err(PeerError::BadCode);
        }
        match (key, port, expires_at) {
            (Some(key), Some(port), Some(expires_at)) if port != 0 => Ok(Self {
                key,
                name: name.unwrap_or_default(),
                port,
                addresses: addresses.unwrap_or_default(),
                expires_at,
            }),
            _ => Err(PeerError::BadCode),
        }
    }

    pub fn is_expired(&self, now_s: i64) -> bool {
        now_s >= self.expires_at
    }

    /// [`PeerError::Expired`] past the code's time.
    pub fn check(&self, now_s: i64) -> Result<(), PeerError> {
        if self.is_expired(now_s) { Err(PeerError::Expired) } else { Ok(()) }
    }

    /// The addresses as `host:port`, to dial and to store in the pairing.
    pub fn socket_addrs(&self) -> Vec<String> {
        self.addresses
            .iter()
            .map(|a| {
                if a.contains(':') && !a.starts_with('[') {
                    format!("[{a}]:{}", self.port)
                } else {
                    format!("{a}:{}", self.port)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_rejections() {
        let code = PairCode::new(
            "Desktop & Co",
            47821,
            vec!["192.168.1.5".into(), "100.101.102.103".into(), "pc.tail1234.ts.net".into(), "fe80::1".into()],
            1_000,
        )
        .unwrap();
        let uri = code.to_uri();
        assert!(uri.starts_with("mixtape://pair?v=1&k="), "{uri}");
        let back = PairCode::parse(&uri).unwrap();
        assert_eq!(back, code);
        assert_eq!(back.expires_at, 1_300);
        assert_eq!(
            back.socket_addrs(),
            ["192.168.1.5:47821", "100.101.102.103:47821", "pc.tail1234.ts.net:47821", "[fe80::1]:47821"]
        );
        assert!(back.check(1_299).is_ok());
        assert!(matches!(back.check(1_300), Err(PeerError::Expired)));
        assert!(!format!("{code:?}").contains(&URL_SAFE_NO_PAD.encode(code.key)));

        for bad in [
            "https://example.com/pair?v=1",
            "mixtape://pair?v=2&k=AAAA&p=1&e=1",
            "mixtape://pair?v=1&k=AAAA&p=47821&e=1",
            &uri.replace("&p=47821", ""),
            "not a uri",
        ] {
            assert!(matches!(PairCode::parse(bad), Err(PeerError::BadCode)), "{bad}");
        }
    }
}
