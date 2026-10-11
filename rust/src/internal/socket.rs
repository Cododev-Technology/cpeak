// The TCP socket that hyper reads the requests from and writes the responses to.
//
// tokio's TcpStream asks the operating system for an event whenever the socket can be read
// from, and also whenever it can be written to. But every response we send makes the socket
// writable again a moment later, so that second kind of event wakes the worker thread up for
// nothing, once per request. Under load, that alone costs about 15% of the throughput.
//
// So like Drogon and nginx, we only ask for the "readable" events. Responses are written to the
// socket right away, and only when the client is too slow to take them (the write would block)
// do we also ask for the "writable" events, until the client catches up.

#[cfg(unix)]
pub(crate) use self::unix::{Listener, Socket};

#[cfg(not(unix))]
pub(crate) use self::other::{Listener, Socket};

#[cfg(unix)]
mod unix {
    use std::io::{self, Read, Write};
    use std::mem::MaybeUninit;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::pin::Pin;
    use std::task::{ready, Context, Poll};

    use tokio::io::unix::AsyncFd;
    use tokio::io::Interest;

    // The socket the server listens on. The acceptor thread takes the new connections from it.
    //
    // tokio's own TcpListener hands every new connection to its event loop first (a system
    // call), and we'd have to take it out again (another one) before a worker can have it.
    // So we take the connections off the listening socket ourselves, and the only event loop
    // a connection ever joins is its worker's.
    pub(crate) struct Listener {
        socket: AsyncFd<socket2::Socket>,
    }

    impl Listener {
        pub(crate) fn new(listener: TcpListener) -> io::Result<Self> {
            let socket = socket2::Socket::from(listener);
            socket.set_nonblocking(true)?;
            // Responses go out right away instead of waiting for more to send. On these
            // systems a connection gets the setting from the listening socket, so we don't
            // have to set it again on every one of them.
            #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
            let _ = socket.set_tcp_nodelay(true);
            Ok(Self {
                socket: AsyncFd::with_interest(socket, Interest::READABLE)?,
            })
        }

        // The next connection, non-blocking and ready for a worker
        pub(crate) async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
            loop {
                let mut guard = self.socket.readable().await?;
                match guard.try_io(|socket| accept(socket.get_ref())) {
                    Ok(accepted) => return accepted,
                    Err(_would_block) => continue,
                }
            }
        }
    }

    // On Linux, one system call takes the connection and makes it non-blocking
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn accept(listener: &socket2::Socket) -> io::Result<(TcpStream, SocketAddr)> {
        let (socket, address) = listener.accept4(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC)?;
        Ok((socket.into(), socket_address(&address)?))
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn accept(listener: &socket2::Socket) -> io::Result<(TcpStream, SocketAddr)> {
        let (socket, address) = listener.accept()?;
        socket.set_nonblocking(true)?;
        #[cfg(not(target_vendor = "apple"))]
        let _ = socket.set_tcp_nodelay(true);
        Ok((socket.into(), socket_address(&address)?))
    }

    fn socket_address(address: &socket2::SockAddr) -> io::Result<SocketAddr> {
        address
            .as_socket()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an IP address"))
    }

    pub(crate) struct Socket {
        // Registered for the readable events only
        readable: AsyncFd<TcpStream>,
        // A second handle on the same socket, registered for the writable events. We only have
        // it while the client is slower than us.
        writable: Option<AsyncFd<TcpStream>>,
    }

    impl Socket {
        // The stream has to be non-blocking already (the Listener's are)
        pub(crate) fn new(stream: TcpStream) -> io::Result<Self> {
            Ok(Self {
                readable: AsyncFd::with_interest(stream, Interest::READABLE)?,
                writable: None,
            })
        }

        fn poll_write_with(
            &mut self,
            cx: &mut Context<'_>,
            wanted: usize,
            mut write: impl FnMut(&TcpStream) -> io::Result<usize>,
        ) -> Poll<io::Result<usize>> {
            loop {
                if let Some(writable) = &self.writable {
                    let mut guard = ready!(writable.poll_write_ready(cx))?;
                    match guard.try_io(|inner| write(inner.get_ref())) {
                        Ok(result) => {
                            // Everything went out, so the client has caught up with us
                            if matches!(result, Ok(written) if written == wanted) {
                                self.writable = None;
                            }
                            return Poll::Ready(result);
                        }
                        Err(_would_block) => continue,
                    }
                }

                match write(self.readable.get_ref()) {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        // The client is slower than us: wait until it takes some more
                        let handle = self.readable.get_ref().try_clone()?;
                        self.writable = Some(AsyncFd::with_interest(handle, Interest::WRITABLE)?);
                    }
                    result => return Poll::Ready(result),
                }
            }
        }
    }

    impl hyper::rt::Read for Socket {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            mut buf: hyper::rt::ReadBufCursor<'_>,
        ) -> Poll<io::Result<()>> {
            loop {
                let mut guard = ready!(self.readable.poll_read_ready(cx))?;
                // SAFETY: read() only writes to the buffer, and we only advance the buffer by
                // what it wrote (this is what tokio's own TcpStream does too)
                let unfilled = unsafe { buf.as_mut() };
                let wanted = unfilled.len();
                let result = guard.try_io(|inner| {
                    let unfilled =
                        unsafe { &mut *(unfilled as *mut [MaybeUninit<u8>] as *mut [u8]) };
                    (&*inner.get_ref()).read(unfilled)
                });
                match result {
                    Ok(Ok(read)) => {
                        unsafe { buf.advance(read) };
                        // Reading less than we asked for means we took everything the socket
                        // had. Anything new will come with a new event, so we don't need one
                        // more read() just to find out that there's nothing left.
                        if read > 0 && read < wanted {
                            guard.clear_ready();
                        }
                        return Poll::Ready(Ok(()));
                    }
                    Ok(Err(error)) => return Poll::Ready(Err(error)),
                    Err(_would_block) => continue,
                }
            }
        }
    }

    impl hyper::rt::Write for Socket {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.get_mut()
                .poll_write_with(cx, buf.len(), |mut stream| stream.write(buf))
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[io::IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            let wanted = bufs.iter().map(|buf| buf.len()).sum();
            self.get_mut()
                .poll_write_with(cx, wanted, |mut stream| stream.write_vectored(bufs))
        }

        fn is_write_vectored(&self) -> bool {
            true
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        // hyper asks for this right before it lets go of the connection, and the socket is
        // closed as soon as it does. Closing sends everything that's still on its way and
        // then ends the connection, so a shutdown() first would only be one more system call.
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}

// Elsewhere (Windows), we use tokio's TcpStream as it is
#[cfg(not(unix))]
mod other {
    use std::io;
    use std::net::{SocketAddr, TcpListener, TcpStream};

    // The socket the server listens on, the way tokio has it
    pub(crate) struct Listener {
        listener: tokio::net::TcpListener,
    }

    impl Listener {
        pub(crate) fn new(listener: TcpListener) -> io::Result<Self> {
            listener.set_nonblocking(true)?;
            Ok(Self {
                listener: tokio::net::TcpListener::from_std(listener)?,
            })
        }

        // The next connection. Its worker registers it with its own event loop.
        pub(crate) async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
            let (stream, address) = self.listener.accept().await?;
            let _ = stream.set_nodelay(true);
            Ok((stream.into_std()?, address))
        }
    }

    pub(crate) struct Socket;

    impl Socket {
        pub(crate) fn new(
            stream: TcpStream,
        ) -> io::Result<hyper_util::rt::TokioIo<tokio::net::TcpStream>> {
            stream.set_nonblocking(true)?;
            Ok(hyper_util::rt::TokioIo::new(
                tokio::net::TcpStream::from_std(stream)?,
            ))
        }
    }
}
