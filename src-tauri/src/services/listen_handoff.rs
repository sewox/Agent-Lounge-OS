//! Hand a pre-bound listening socket to a child process (test helpers).
//!
//! Avoids reserve → free → rebind TOCTOU races on macOS ephemeral ports.
//! Unix: clear FD_CLOEXEC and pass `--listen-fd=N` (caller drops after spawn).
//! Windows: pass the SOCKET as the child's stdin (`--listen-stdin`) so
//! CreateProcess inherits it via STARTF_USESTDHANDLES.

use std::net::TcpListener;
use std::process::Command;
#[cfg(windows)]
use std::process::Stdio;

/// Convert a reserved `std` listener into a tokio listener without rebinding.
pub fn std_listener_to_tokio(listener: TcpListener) -> std::io::Result<tokio::net::TcpListener> {
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}

/// Attach a reserved listener for child inheritance.
///
/// Returns `Some(listener)` when the caller must keep it alive until after
/// `Command::spawn` (Unix). Returns `None` when ownership was moved into the
/// command (Windows stdin).
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
        use std::os::windows::io::{FromRawHandle, IntoRawSocket, RawHandle};
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};

        let socket = listener.into_raw_socket();
        let ok = unsafe {
            SetHandleInformation(socket as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SOCKET as stdin — reliable under STARTF_USESTDHANDLES.
        // SAFETY: socket is a live inheritable HANDLE; Stdio takes ownership.
        command.stdin(unsafe { Stdio::from_raw_handle(socket as RawHandle) });
        command.arg("--listen-stdin");
        Ok(None)
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
