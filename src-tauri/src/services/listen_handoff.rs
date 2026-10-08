//! Hand a pre-bound listening socket to a child process (test helpers).
//!
//! Avoids reserve → free → rebind TOCTOU races on macOS ephemeral ports.
//! Unix: clear FD_CLOEXEC and pass `--listen-fd=N`; drop parent copy after spawn.
//! Windows: keep the parent listener alive, spawn the child with
//! `--reuse-bind --port=N` (SO_REUSEADDR concurrent bind), wait for
//! `listen-adopted`, then drop the parent descriptor. Never free-then-rebind.
//! Do **not** use `CREATE_NO_WINDOW` on handoff spawns (breaks piped stdio).

use std::net::TcpListener;
use std::process::{Child, Command};
#[cfg(windows)]
use std::time::{Duration, Instant};

/// Convert a reserved `std` listener into a tokio listener without rebinding.
pub fn std_listener_to_tokio(listener: TcpListener) -> std::io::Result<tokio::net::TcpListener> {
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}

/// Bind `127.0.0.1:port` (`0` = ephemeral) with `SO_REUSEADDR` so a Windows child
/// can `--reuse-bind` the same port while this listener is still held.
pub fn bind_loopback_reuseaddr(port: u16) -> std::io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    Ok(socket.into())
}

/// Token returned by [`attach_inherited_listener_owned`]; finish with
/// [`complete_listen_handoff`] after `Command::spawn`.
pub struct PendingListenHandoff {
    listener: TcpListener,
    #[cfg(windows)]
    port: u16,
}

/// Usually empty after [`complete_listen_handoff`] (parent socket already dropped).
#[must_use]
pub struct ListenHandoffGuard {
    _listener: Option<TcpListener>,
}

impl ListenHandoffGuard {
    fn none() -> Self {
        Self { _listener: None }
    }
}

/// Prepare `command` so the child can adopt `listener` without a free→rebind gap.
pub fn attach_inherited_listener_owned(
    command: &mut Command,
    listener: TcpListener,
) -> std::io::Result<PendingListenHandoff> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = listener.as_raw_fd();
        // SAFETY: fcntl on a live socket fd owned by `listener`.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        command.arg(format!("--listen-fd={fd}"));
        Ok(PendingListenHandoff { listener })
    }

    #[cfg(windows)]
    {
        use std::process::Stdio;
        let port = listener.local_addr()?.port();
        // Concurrent bind while parent still holds — no free window for thieves.
        command.arg("--reuse-bind");
        command.arg(format!("--port={port}"));
        command.stdin(Stdio::null());
        Ok(PendingListenHandoff { listener, port })
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (command, listener);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "listen handoff unsupported on this platform",
        ))
    }
}

/// Finish handoff after spawn.
///
/// Unix: drop parent fd (child inherited it).
/// Windows: wait for `listen-adopted`, then drop parent so the child alone owns
/// the port for accept / `listen_pids`.
pub fn complete_listen_handoff(
    pending: PendingListenHandoff,
    child: &mut Child,
) -> std::io::Result<ListenHandoffGuard> {
    #[cfg(unix)]
    {
        let _ = child;
        drop(pending.listener);
        Ok(ListenHandoffGuard::none())
    }

    #[cfg(windows)]
    {
        let _ = pending.port;
        let pid = child.id();
        let stderr = child.stderr.as_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "listen handoff: child stderr pipe missing",
            )
        })?;
        wait_listen_adopted_line(stderr, pid, Duration::from_secs(5))?;
        drop(pending.listener);
        Ok(ListenHandoffGuard::none())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pending, child);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "listen handoff unsupported on this platform",
        ))
    }
}

/// Read stderr one byte at a time until `listen-adopted ... pid=P`.
#[cfg(windows)]
fn wait_listen_adopted_line(
    stderr: &mut impl std::io::Read,
    expected_pid: u32,
    timeout: Duration,
) -> std::io::Result<()> {
    let start = Instant::now();
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let mut seen = String::new();
    loop {
        if start.elapsed() > timeout {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for listen-adopted from pid {expected_pid}; stderr={seen}"
                ),
            ));
        }
        match stderr.read(&mut byte) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("stderr EOF before listen-adopted; stderr={seen}"),
                ));
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    let text = String::from_utf8_lossy(&line);
                    let trimmed = text.trim().trim_end_matches('\r');
                    if !seen.is_empty() {
                        seen.push('\n');
                    }
                    seen.push_str(trimmed);
                    if trimmed.starts_with("listen-adopted ")
                        && trimmed.contains(&format!("pid={expected_pid}"))
                    {
                        return Ok(());
                    }
                    line.clear();
                } else {
                    line.push(byte[0]);
                    if line.len() > 512 {
                        line.clear();
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => return Err(err),
        }
    }
}
