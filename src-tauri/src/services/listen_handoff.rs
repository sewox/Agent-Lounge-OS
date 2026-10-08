//! Hand a pre-bound listening socket to a child process (test helpers).
//!
//! Avoids reserve → free → rebind TOCTOU races on macOS ephemeral ports.
//!
//! Unix: pass `--listen-fd=N`; clear `FD_CLOEXEC` only in the child via
//! `pre_exec` so concurrent spawns cannot inherit the LISTEN socket.
//!
//! Windows: parent keeps an **exclusive** bind; hand off via
//! `WSADuplicateSocketW` + `--listen-proto-stdin` (child `WSASocketW`), then
//! drop parent after `listen-adopted`. Do **not** use child `--reuse-bind`
//! over an exclusive parent — same-user specific+specific SO_REUSEADDR fails
//! with WSAEACCES (10013) on modern Windows. Never free-then-rebind.
//! Do **not** use `CREATE_NO_WINDOW` on handoff spawns (breaks piped stdio).

use std::net::TcpListener;
use std::process::{Child, Command};
#[cfg(windows)]
use std::time::Duration;

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
        use std::os::unix::process::CommandExt;
        let fd = listener.as_raw_fd();
        // Keep FD_CLOEXEC in the parent. Clear it only in this child after fork
        // so a concurrent sibling spawn cannot keep the LISTEN socket open.
        // SAFETY: pre_exec runs in the forked child before exec; `fd` is the
        // inherited copy of our live listener.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
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
/// Unix: drop parent fd (child inherited it via pre_exec CLOEXEC clear).
/// Windows: `WSADuplicateSocketW` → write protocol info on stdin → wait for
/// `listen-adopted` (bounded) → drop parent so the child alone owns the port.
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
        use std::io::Write;
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

        let stderr = child.stderr.take().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "listen handoff: child stderr pipe missing",
            )
        })?;
        wait_listen_adopted_line(stderr, pid, Duration::from_secs(5))?;
        // Child has its own SOCKET; release parent so accept / listen_pids
        // attach to the child alone.
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

/// Wait for `listen-adopted ... pid=P` with a real timeout (reader thread + channel).
#[cfg(windows)]
fn wait_listen_adopted_line(
    mut stderr: impl std::io::Read + Send + 'static,
    expected_pid: u32,
    timeout: Duration,
) -> std::io::Result<()> {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<std::io::Result<()>>();
    std::thread::spawn(move || {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        let mut seen = String::new();
        let result = loop {
            match stderr.read(&mut byte) {
                Ok(0) => {
                    break Err(std::io::Error::new(
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
                            // Drain leftover stderr so the child is not killed by
                            // a full pipe / ERROR_BROKEN_PIPE on further writes.
                            let mut buf = [0u8; 512];
                            loop {
                                match stderr.read(&mut buf) {
                                    Ok(0) | Err(_) => break,
                                    Ok(_) => {}
                                }
                            }
                            break Ok(());
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
                Err(err) => break Err(err),
            }
        };
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("timed out waiting for listen-adopted from pid {expected_pid}"),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            format!("listen-adopted waiter disconnected for pid {expected_pid}"),
        )),
    }
}
