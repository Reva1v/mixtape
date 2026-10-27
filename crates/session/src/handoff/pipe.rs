//! The transport: a named pipe per side (`\\.\pipe\mixtape-<SID>-app`, `…-island`), locked
//! to the current user, one client at a time. A [`Link`] is one handshaken connection.

use super::msg::{self, ErrorCode, FrameError, MAX_FRAME, Msg, PROTO, Side};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// How long a side waits for an answer (the peer's `hello`, `ack`, `handoff`).
pub const TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    /// Nobody serves the pipe: the other app is not running (or has no channel).
    #[error("the other side is not running")]
    NoPeer,
    #[error("the other side already has a connection")]
    Busy,
    /// The peer speaks another `proto` (its number).
    #[error("the other side speaks protocol {0}, this one {PROTO}")]
    Incompatible(u16),
    #[error("no answer in time")]
    Timeout,
    #[error("the connection closed")]
    Closed,
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The peer sent `error` and closed.
    #[error("the other side reported {0:?}")]
    Peer(ErrorCode),
    #[error("{0}")]
    Io(String),
}

/// What the peer said about itself in its `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub side: Side,
    pub pid: u32,
    pub version: String,
}

type Writer = Arc<Mutex<Box<dyn AsyncWrite + Send + Sync + Unpin>>>;
type Reader = BufReader<Box<dyn AsyncRead + Send + Sync + Unpin>>;

/// A handshaken connection. [`Link::split`] gives a sender (clonable) and a receiver, so
/// one task can wait for messages while others send.
pub struct Link {
    peer: Peer,
    opened_by: Side,
    tx: LinkTx,
    rx: LinkRx,
}

#[derive(Clone)]
pub struct LinkTx {
    writer: Writer,
    /// Held while the link (either half) lives: the server answers `busy` to other
    /// clients meanwhile.
    _busy: Option<Arc<()>>,
}

pub struct LinkRx {
    reader: Reader,
    writer: Writer,
    _busy: Option<Arc<()>>,
}

impl Link {
    /// Handshakes over any byte stream: sends this side's `hello`, then waits up to
    /// [`TIMEOUT`] for the peer's. `opened_by` is the side that connected (the client).
    pub async fn over<S>(stream: S, me: Side, opened_by: Side) -> Result<Link, LinkError>
    where
        S: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
    {
        let (r, w) = tokio::io::split(stream);
        let writer: Writer = Arc::new(Mutex::new(Box::new(w)));
        let mut reader: Reader = BufReader::new(Box::new(r));
        write(&writer, &Msg::hello(me)).await?;
        let line = match tokio::time::timeout(TIMEOUT, read_frame(&mut reader)).await {
            Err(_) => return Err(LinkError::Timeout),
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => return Err(LinkError::Closed),
            Ok(Err(e)) => {
                let _ = write(&writer, &Msg::Error { code: e.code(), proto: None }).await;
                return Err(e.into());
            }
        };
        if let Some(p) = msg::hello_proto(&line)
            && p != PROTO
        {
            let _ = write(&writer, &Msg::Error { code: ErrorCode::Incompatible, proto: Some(PROTO) }).await;
            return Err(LinkError::Incompatible(p));
        }
        let peer = match msg::decode(&line) {
            Ok(Msg::Hello { app, pid, version, .. }) => Peer { side: app, pid, version },
            Ok(Msg::Error { code: ErrorCode::Busy, .. }) => return Err(LinkError::Busy),
            Ok(Msg::Error { code: ErrorCode::Incompatible, proto }) => {
                return Err(LinkError::Incompatible(proto.unwrap_or(0)));
            }
            Ok(Msg::Error { code, .. }) => return Err(LinkError::Peer(code)),
            Ok(_) => {
                let _ = write(&writer, &Msg::Error { code: ErrorCode::BadFrame, proto: None }).await;
                return Err(FrameError::Bad("the first line is not hello".into()).into());
            }
            Err(e) => {
                let _ = write(&writer, &Msg::Error { code: e.code(), proto: None }).await;
                return Err(e.into());
            }
        };
        Ok(Link {
            peer,
            opened_by,
            tx: LinkTx { writer: writer.clone(), _busy: None },
            rx: LinkRx { reader, writer, _busy: None },
        })
    }

    /// Connects to the pipe `name` as a client.
    #[cfg(windows)]
    pub async fn connect(name: &str, me: Side) -> Result<Link, LinkError> {
        use tokio::net::windows::named_pipe::ClientOptions;
        const ERROR_FILE_NOT_FOUND: i32 = 2;
        const ERROR_PIPE_BUSY: i32 = 231;
        let client = match ClientOptions::new().open(name) {
            Ok(c) => c,
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => return Err(LinkError::NoPeer),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => return Err(LinkError::Busy),
            Err(e) => return Err(LinkError::Io(e.to_string())),
        };
        Link::over(client, me, me).await
    }

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// The side that connected; see [`keep_new`].
    pub fn opened_by(&self) -> Side {
        self.opened_by
    }

    pub async fn send(&self, msg: &Msg) -> Result<(), LinkError> {
        self.tx.send(msg).await
    }

    pub async fn recv(&mut self) -> Result<Msg, LinkError> {
        self.rx.recv().await
    }

    pub async fn recv_timeout(&mut self, wait: Duration) -> Result<Msg, LinkError> {
        self.rx.recv_timeout(wait).await
    }

    /// Sends `msg` and waits up to [`TIMEOUT`] for the next message (the answer).
    pub async fn request(&mut self, msg: &Msg) -> Result<Msg, LinkError> {
        self.send(msg).await?;
        self.recv_timeout(TIMEOUT).await
    }

    pub fn split(self) -> (LinkTx, LinkRx) {
        (self.tx, self.rx)
    }
}

impl LinkTx {
    pub async fn send(&self, msg: &Msg) -> Result<(), LinkError> {
        write(&self.writer, msg).await
    }

    /// Closes the write side: the peer reads the end of the stream.
    pub async fn close(&self) {
        let _ = self.writer.lock().await.shutdown().await;
    }
}

impl LinkRx {
    /// The next message. A peer's `error` ends the link as `Peer(code)`; a broken frame is
    /// answered with `error` and ends it too.
    pub async fn recv(&mut self) -> Result<Msg, LinkError> {
        match read_frame(&mut self.reader).await {
            Ok(None) => Err(LinkError::Closed),
            Ok(Some(line)) => match msg::decode(&line) {
                Ok(Msg::Error { code, .. }) => Err(LinkError::Peer(code)),
                Ok(m) => Ok(m),
                Err(e) => {
                    let _ = write(&self.writer, &Msg::Error { code: e.code(), proto: None }).await;
                    Err(e.into())
                }
            },
            Err(e) => {
                let _ = write(&self.writer, &Msg::Error { code: e.code(), proto: None }).await;
                Err(e.into())
            }
        }
    }

    pub async fn recv_timeout(&mut self, wait: Duration) -> Result<Msg, LinkError> {
        tokio::time::timeout(wait, self.recv()).await.unwrap_or(Err(LinkError::Timeout))
    }
}

/// Two links at once (both apps started together and each reached the other's server):
/// the one the app opened stays. `true` when a new link opened by `new` replaces the one
/// opened by `current`.
pub fn keep_new(current: Side, new: Side) -> bool {
    new == Side::App && current != Side::App
}

async fn write(writer: &Writer, msg: &Msg) -> Result<(), LinkError> {
    let frame = msg::encode(msg)?;
    let mut w = writer.lock().await;
    w.write_all(&frame).await.map_err(io_err)?;
    w.flush().await.map_err(io_err)
}

fn io_err(e: std::io::Error) -> LinkError {
    use std::io::ErrorKind::*;
    match e.kind() {
        BrokenPipe | ConnectionReset | ConnectionAborted | UnexpectedEof | NotConnected => LinkError::Closed,
        _ if e.raw_os_error() == Some(109) || e.raw_os_error() == Some(232) => LinkError::Closed,
        _ => LinkError::Io(e.to_string()),
    }
}

/// One line, `\n` included; `None` at a clean end of the stream.
async fn read_frame<R: AsyncBufRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut buf = Vec::new();
    let n = match (&mut *r).take(MAX_FRAME as u64 + 1).read_until(b'\n', &mut buf).await {
        Ok(n) => n,
        // A pipe whose other end went away reads as an error, not as the end.
        Err(_) if buf.is_empty() => return Ok(None),
        Err(e) => return Err(FrameError::Bad(e.to_string())),
    };
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        return Err(if buf.len() > MAX_FRAME {
            FrameError::TooLarge
        } else {
            FrameError::Bad("the line was cut off".into())
        });
    }
    Ok(Some(buf))
}

/// A running server; dropping it stops accepting (open links stay).
pub struct Server {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(windows)]
impl Server {
    /// Serves the pipe `name` on `rt`: each client is handshaken and given to `on_link`;
    /// while a link lives, other clients get `error{busy}` and are closed. Fails when the
    /// pipe already exists (another process serves it).
    pub fn serve<F>(rt: &tokio::runtime::Handle, name: &str, me: Side, on_link: F) -> std::io::Result<Server>
    where
        F: Fn(Link) + Send + Sync + 'static,
    {
        let dacl = Arc::new(win::UserOnly::new()?);
        let first = {
            let _guard = rt.enter();
            win::create(name, &dacl, true)?
        };
        let name = name.to_owned();
        let on_link = Arc::new(on_link);
        let task = rt.spawn(async move {
            let active: Arc<StdMutex<Weak<()>>> = Arc::new(StdMutex::new(Weak::new()));
            let mut server = first;
            loop {
                if let Err(e) = server.connect().await {
                    tracing::warn!(error = %e, "hand-off pipe: accept failed");
                }
                let next = match win::create(&name, &dacl, false) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "hand-off pipe: no next instance");
                        return;
                    }
                };
                let conn = std::mem::replace(&mut server, next);
                let busy = {
                    let mut slot = active.lock().expect("hand-off slot poisoned");
                    if slot.strong_count() > 0 {
                        None
                    } else {
                        let token = Arc::new(());
                        *slot = Arc::downgrade(&token);
                        Some(token)
                    }
                };
                let on_link = on_link.clone();
                tokio::spawn(async move {
                    let Some(token) = busy else {
                        refuse_busy(conn).await;
                        return;
                    };
                    match Link::over(conn, me, me.other()).await {
                        Ok(mut link) => {
                            link.rx._busy = Some(token.clone());
                            link.tx._busy = Some(token);
                            on_link(link);
                        }
                        Err(e) => tracing::info!(error = %e, "hand-off pipe: handshake failed"),
                    }
                });
            }
        });
        Ok(Server { task })
    }
}

/// Answers a client the server has no room for. Its `hello` is read first and the end of
/// the stream awaited after `error{busy}`: a pipe closed with unread data, or before the
/// client wrote, would read on the other end as a plain close instead.
async fn refuse_busy<S>(conn: S)
where
    S: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    let (r, w) = tokio::io::split(conn);
    let mut reader: Reader = BufReader::new(Box::new(r));
    let w: Writer = Arc::new(Mutex::new(Box::new(w)));
    let _ = tokio::time::timeout(TIMEOUT, read_frame(&mut reader)).await;
    let _ = write(&w, &Msg::Error { code: ErrorCode::Busy, proto: None }).await;
    let _ = tokio::time::timeout(TIMEOUT, async { while let Ok(Some(_)) = read_frame(&mut reader).await {} }).await;
}

/// `\\.\pipe\mixtape-<SID>-app` or `…-island`: per user, so two users of one PC never meet.
#[cfg(windows)]
pub fn pipe_name(side: Side) -> std::io::Result<String> {
    Ok(format!(r"\\.\pipe\mixtape-{}-{}", win::user_sid()?, side.suffix()))
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::{HSTRING, PWSTR};

    fn os(e: windows::core::Error) -> std::io::Error {
        std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF)
    }

    /// The current user's SID as text (`S-1-5-21-…`).
    pub fn user_sid() -> std::io::Result<String> {
        // SAFETY: plain Win32 calls; every buffer is sized by the first call and freed below.
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(os)?;
            let mut len = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len);
            let _ = CloseHandle(token);
            got.map_err(os)?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text).map_err(os)?;
            let sid = text.to_string().map_err(|e| std::io::Error::other(e.to_string()));
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            sid
        }
    }

    /// Security attributes whose DACL grants the current user alone.
    pub struct UserOnly {
        sd: PSECURITY_DESCRIPTOR,
        sa: SECURITY_ATTRIBUTES,
    }

    // SAFETY: the descriptor is immutable after creation and freed only in `drop`.
    unsafe impl Send for UserOnly {}
    unsafe impl Sync for UserOnly {}

    impl UserOnly {
        pub fn new() -> std::io::Result<Self> {
            let sddl = HSTRING::from(format!("D:P(A;;GA;;;{})", user_sid()?));
            let mut sd = PSECURITY_DESCRIPTOR::default();
            // SAFETY: `sd` receives a LocalAlloc'd descriptor, freed in `drop`.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut sd, None)
                    .map_err(os)?;
            }
            let sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            Ok(Self { sd, sa })
        }
    }

    impl Drop for UserOnly {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.sd.0)));
            }
        }
    }

    /// One pipe instance. Needs a Tokio runtime context.
    pub fn create(name: &str, dacl: &UserOnly, first: bool) -> std::io::Result<NamedPipeServer> {
        let mut opts = ServerOptions::new();
        opts.first_pipe_instance(first).reject_remote_clients(true).pipe_mode(PipeMode::Byte);
        // SAFETY: `dacl` outlives the call; the system copies the descriptor.
        unsafe { opts.create_with_security_attributes_raw(name, &dacl.sa as *const SECURITY_ATTRIBUTES as *mut c_void) }
    }
}
