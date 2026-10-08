//! Hand a pre-bound listening socket to a child process (test helpers).
//!
//! Avoids reserve → free → rebind TOCTOU races on macOS ephemeral ports.
//! Unix: clear FD_CLOEXEC and pass `--listen-fd=N` (caller drops after spawn).
//! Windows: mark the SOCKET inheritable, pass `LOUNGE_TEST_LISTEN_SOCKET`, and
//! keep the parent listener alive until after `Command::spawn` (do **not** use
//! `CREATE_NO_WINDOW` on handoff spawns — it breaks handle inheritance).

use std::net::TcpListener;
use std::process::Command;

/// Env var consumed by `lounge-test-helper` on Windows.
#[cfg(windows)]
pub const LISTEN_SOCKET_ENV: &str = "LOUNGE_TEST_LISTEN_SOCKET";

/// Convert a reserved `std` listener into a tokio listener without rebinding.
pub fn std_listener_to_tokio(listener: TcpListener) -> std::io::Result<tokio::net::TcpListener> {
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}

/// Attach a reserved listener for child inheritance.
///
/// Returns `Some(listener)` when the caller must keep it alive until after
/// `Command::spawn`, then drop it so only the child holds the LISTEN socket.
pub fn attach_inherited_listener_owned(
    command: &mut Command,
    listener: TcpListener,
) -> std::io::Result<Option<TcpListener>> {
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
        Ok(Some(listener))
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};

        let socket = listener.as_raw_socket();
        let ok = unsafe {
            SetHandleInformation(socket as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        command.env(LISTEN_SOCKET_ENV, socket.to_string());
        // Keep parent copy until after spawn (child inherits the HANDLE).
        Ok(Some(listener))
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
