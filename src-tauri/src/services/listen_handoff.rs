//! Hand a pre-bound listening socket to a child process (test helpers).
//!
//! Avoids reserve → free → rebind TOCTOU races on macOS ephemeral ports.
//! Unix: clear FD_CLOEXEC and pass `--listen-fd=N`; drop parent copy after spawn.
//! Windows: spawn with piped stdin + `--listen-proto-stdin`, then
//! `WSADuplicateSocketW` → write `WSAPROTOCOL_INFOW` → wait for child's
//! `listen-adopted` line → drop parent descriptor (so accept/ownership work).
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

/// Token returned by [`attach_inherited_listener_owned`]; finish with
/// [`complete_listen_handoff`] after `Command::spawn`.
pub struct PendingListenHandoff {
    listener: TcpListener,
}

/// Kept for API symmetry; Windows handoff now drops the parent socket inside
/// [`complete_listen_handoff`] after `listen-adopted`, so this is usually empty.
#[must_use]
pub struct ListenHandoffGuard {
    _listener: Option<TcpListener>,
}

impl ListenHandoffGuard {
    fn none() -> Self {
        Self { _listener: None }
    }
}

/// Prepare `command` so the child can adopt `listener` without rebinding.
///
/// On Windows this sets `stdin` to a pipe and adds `--listen-proto-stdin`.
/// Call [`complete_listen_handoff`] immediately after a successful spawn.
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
        let _ = &listener;
        command.stdin(Stdio::piped());
        command.arg("--listen-proto-stdin");
        Ok(PendingListenHandoff { listener })
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
/// Unix: drops the parent fd immediately (child already inherited it).
/// Windows: writes protocol info, waits for `listen-adopted` on stderr, then
/// drops the parent LISTEN socket so the child alone owns accept/PID.
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
        use std::io::{Read, Write};
        use std::mem::{size_of, MaybeUninit};
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            WSADuplicateSocketW, SOCKET, SOCKET_ERROR, WSAPROTOCOL_INFOW,
        };

        let mut stdin = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "listen handoff: child stdin pipe missing",
            )
        })?;
        let socket = pending.listener.as_raw_socket() as SOCKET;
        let pid = child.id();
        let mut info = MaybeUninit::<WSAPROTOCOL_INFOW>::uninit();
        let rc = unsafe { WSADuplicateSocketW(socket, pid, info.as_mut_ptr()) };
        if rc == SOCKET_ERROR {
            return Err(std::io::Error::last_os_error());
        }
        let info = unsafe { info.assume_init() };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&info as *const WSAPROTOCOL_INFOW).cast::<u8>(),
                size_of::<WSAPROTOCOL_INFOW>(),
            )
        };
        stdin.write_all(bytes)?;
        stdin.flush()?;
        drop(stdin);

        let stderr = child.stderr.as_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "listen handoff: child stderr pipe missing",
            )
        })?;
        wait_listen_adopted_line(stderr, pid, Duration::from_secs(5))?;
        // Child has its own SOCKET descriptor now; release parent so ownership
        // and accepts attach to the child alone.
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

/// Read stderr one byte at a time until `listen-adopted ... pid=P` (no buffering ahead).
#[cfg(windows)]
fn wait_listen_adopted_line(
    stderr: &mut impl Read,
    expected_pid: u32,
    timeout: Duration,
) -> std::io::Result<()> {
    let start = Instant::now();
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if start.elapsed() > timeout {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out waiting for listen-adopted from pid {expected_pid}"),
            ));
        }
        match stderr.read(&mut byte) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "stderr EOF before listen-adopted",
                ));
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    let text = String::from_utf8_lossy(&line);
                    let trimmed = text.trim().trim_end_matches('\r');
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
