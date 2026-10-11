// HTTPS: the TLS layer between the TCP socket and hyper. In Node.js this is the node:https
// module, which comes with the runtime. Here it's built on rustls.
//
// rustls does the TLS itself: it turns the encrypted data we read from the socket into the
// plain requests that hyper parses, and the plain responses that hyper writes into the
// encrypted data we write to the socket. What this file decides is when to read and write:
//
// - One read takes everything the client sent off the socket. rustls on its own asks for
//   4 KB at a time, so a request with a 10 KB body would cost three system calls instead of one.
// - Nothing is written to the socket while hyper is at work. Whatever there is to send after
//   one of hyper's turns (the responses, and the TLS handshake too) goes out with one write,
//   exactly like on our plain connections (see HeldBack in server.rs).

use std::cell::RefCell;
use std::io::{self, BufRead, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use hyper::rt::{ReadBuf, ReadBufCursor};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};

use crate::internal::types::HttpsOptions;

// What an HTTPS server keeps of its options: the certificate and the key, read once
pub(crate) type ResolvedHttpsConfig = Arc<ServerConfig>;

pub(crate) fn resolve_https_options(options: HttpsOptions) -> ResolvedHttpsConfig {
    read_https_options(&options)
        .unwrap_or_else(|error| panic!("[cpeak] Invalid https options: {error}"))
}

fn read_https_options(options: &HttpsOptions) -> Result<ResolvedHttpsConfig, String> {
    let certificates = CertificateDer::pem_slice_iter(options.cert.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cert can't be read ({error})"))?;
    if certificates.is_empty() {
        return Err("cert has no certificate in it (it has to be in the PEM format)".to_string());
    }
    let key = PrivateKeyDer::from_pem_slice(options.key.as_bytes())
        .map_err(|error| format!("key can't be read ({error}). It has to be in the PEM format."))?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| error.to_string())?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|error| format!("cert and key can't be used ({error})"))?;
    // What we speak once the connection is encrypted
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    Ok(Arc::new(config))
}

// The most that one read takes off a socket: two of the largest TLS records there are
const INCOMING_SIZE: usize = 2 * (16 * 1024 + 256 + 5);

thread_local! {
    // Where the encrypted data lands when it comes off a socket, before rustls takes it in.
    // It's only in use during one read, so one per worker thread is all we need.
    static INCOMING: RefCell<Box<[u8]>> = RefCell::new(vec![0; INCOMING_SIZE].into_boxed_slice());
}

// A connection over TLS, as hyper sees it: what hyper reads from it is decrypted, and what
// hyper writes to it gets encrypted
pub(crate) struct TlsSocket<T> {
    socket: T,
    tls: ServerConnection,
    // Encrypted data that came off the socket and that rustls hasn't taken in yet. rustls only
    // holds on to so much decrypted data, and takes in more once hyper has read what it has.
    not_taken_in: Vec<u8>,
    // The client is done sending
    ended: bool,
    said_goodbye: bool,
}

impl<T: hyper::rt::Read + hyper::rt::Write + Unpin> TlsSocket<T> {
    pub(crate) fn new(socket: T, config: &ResolvedHttpsConfig) -> io::Result<Self> {
        let tls = ServerConnection::new(config.clone()).map_err(io::Error::other)?;
        Ok(Self {
            socket,
            tls,
            not_taken_in: Vec::new(),
            ended: false,
            said_goodbye: false,
        })
    }

    // Hand hyper the decrypted data that rustls has. False when there is none right now.
    fn read_decrypted(&mut self, buf: &mut ReadBufCursor<'_>) -> io::Result<bool> {
        let mut read = false;
        while buf.remaining() > 0 {
            let mut reader = self.tls.reader();
            let decrypted = match reader.fill_buf() {
                Ok(decrypted) => decrypted,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            };
            if decrypted.is_empty() {
                // The client ended the TLS session
                self.ended = true;
                break;
            }
            let size = decrypted.len().min(buf.remaining());
            buf.put_slice(&decrypted[..size]);
            reader.consume(size);
            read = true;
        }
        Ok(read)
    }

    // Give rustls more of what the client sent: what we still have of it, or else what the
    // socket has
    fn poll_take_in(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.not_taken_in.is_empty() {
            let data = std::mem::take(&mut self.not_taken_in);
            return Poll::Ready(self.take_in(&data));
        }

        INCOMING.with(|incoming| {
            let mut incoming = incoming.borrow_mut();
            let mut incoming = ReadBuf::new(&mut incoming);
            ready!(Pin::new(&mut self.socket).poll_read(cx, incoming.unfilled()))?;
            if incoming.filled().is_empty() {
                self.ended = true;
                return Poll::Ready(Ok(()));
            }
            Poll::Ready(self.take_in(incoming.filled()))
        })
    }

    fn take_in(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            match self.tls.read_tls(&mut data) {
                // Whatever comes after the end of the TLS session isn't for us
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::InvalidData => return Err(error),
                // rustls holds all the decrypted data it wants to. The rest waits until
                // hyper has read some of that.
                Err(_) => {
                    self.not_taken_in.extend_from_slice(data);
                    break;
                }
            }
            // What rustls has to say about it (its part of the handshake, or an alert when
            // this fails) is sent at the end of hyper's turn, like everything else
            self.tls
                .process_new_packets()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        }
        Ok(())
    }

    // Hand data to rustls, which encrypts it and keeps it until poll_send. It only keeps so
    // much: when it takes nothing, some of it has to go out first.
    fn poll_encrypt(
        &mut self,
        cx: &mut Context<'_>,
        has_data: bool,
        mut encrypt: impl FnMut(&mut ServerConnection) -> io::Result<usize>,
    ) -> Poll<io::Result<usize>> {
        loop {
            match encrypt(&mut self.tls) {
                Ok(0) if has_data => {
                    if !self.tls.wants_write() {
                        return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                    }
                    ready!(self.poll_send(cx))?;
                }
                result => return Poll::Ready(result),
            }
        }
    }

    // Send what rustls has for the client. If the client can't take it all right now, the
    // socket wakes the connection up once it can.
    fn poll_send(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.tls.wants_write() {
            let mut socket = Sending {
                socket: &mut self.socket,
                cx,
            };
            match self.tls.write_tls(&mut socket) {
                Ok(0) => return Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Poll::Pending,
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<T: hyper::rt::Read + hyper::rt::Write + Unpin> hyper::rt::Read for TlsSocket<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            // Reading nothing is how hyper learns that the client is done
            if this.read_decrypted(&mut buf)? || this.ended || buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            ready!(this.poll_take_in(cx))?;
        }
    }
}

impl<T: hyper::rt::Read + hyper::rt::Write + Unpin> hyper::rt::Write for TlsSocket<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut()
            .poll_encrypt(cx, !buf.is_empty(), |tls| tls.writer().write(buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let has_data = bufs.iter().any(|buf| !buf.is_empty());
        self.get_mut()
            .poll_encrypt(cx, has_data, |tls| tls.writer().write_vectored(bufs))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().poll_send(cx)
    }

    // Tell the client that this is the end (close_notify), so it knows that what it got is all
    // there was, and not a response that was cut short
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.said_goodbye {
            this.said_goodbye = true;
            this.tls.send_close_notify();
        }
        ready!(this.poll_send(cx))?;
        Pin::new(&mut this.socket).poll_shutdown(cx)
    }
}

// The socket as rustls writes to it. rustls doesn't know about wakers: when the client can't
// take more right now, it gets a "would block", and poll_send takes it from there.
struct Sending<'a, 'b, T> {
    socket: &'a mut T,
    cx: &'a mut Context<'b>,
}

impl<T: hyper::rt::Write + Unpin> Write for Sending<'_, '_, T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match Pin::new(&mut *self.socket).poll_write(self.cx, buf) {
            Poll::Ready(result) => result,
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        match Pin::new(&mut *self.socket).poll_write_vectored(self.cx, bufs) {
            Poll::Ready(result) => result,
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
