//! What really happens when a cancel arrives while packages are still downloading.
use std::path::PathBuf;
use std::time::Duration;

use store_protocol::{EXIT_CANCELLED, ErrorCode, Event};

use crate::Aborter;

/// Writes one line straight to a file descriptor, giving up after `timeout`. It does not go
/// through Rust's stdout lock, which the event writer may hold while blocked on a client that
/// stopped reading: the abort must never wait for that.
pub(crate) fn write_line_raw(fd: i32, text: &str, timeout: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: poll gets one valid pollfd that lives for the whole call.
    let ready = unsafe { libc::poll(&mut poll, 1, millis) };
    if ready <= 0 || poll.revents & libc::POLLOUT == 0 {
        return false;
    }
    let bytes = text.as_bytes();
    // SAFETY: `bytes` is valid for `bytes.len()` bytes. A pipe that polled writable accepts a
    // write of up to PIPE_BUF bytes without blocking, and these lines are shorter.
    let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    written == bytes.len() as isize
}

/// Removes the helper's own database lock, reports the cancellation and exits. It is only called
/// inside the cancel gate's window, i.e. after this process took the lock and before it changed
/// any package, so nothing is left half applied.
pub struct ProcessAborter {
    lock_path: PathBuf,
}

impl ProcessAborter {
    pub fn new(lock_path: PathBuf) -> ProcessAborter {
        ProcessAborter { lock_path }
    }

    /// Everything the abort does except reporting and exiting. Touches only the lock file.
    pub(crate) fn cleanup_only(&self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

impl Aborter for ProcessAborter {
    fn abort(&self) {
        self.cleanup_only();
        let line = Event::Error {
            code: ErrorCode::Cancelled,
            message: "cancelled while downloading; nothing was changed".to_string(),
        }
        .to_line();
        // Best effort: a client that is not reading must not keep the helper alive.
        write_line_raw(libc::STDOUT_FILENO, &line, Duration::from_millis(500));
        std::process::exit(EXIT_CANCELLED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_aborter_removes_only_its_own_lock() {
        let dir = tempfile::tempdir().unwrap();
        let ours = dir.path().join("db.lck");
        let other = dir.path().join("other.lck");
        std::fs::write(&ours, b"").unwrap();
        std::fs::write(&other, b"").unwrap();
        ProcessAborter::new(ours.clone()).cleanup_only();
        assert!(!ours.exists(), "its own lock must go");
        assert!(other.exists(), "no other file may be touched");
        ProcessAborter::new(ours).cleanup_only(); // an already missing lock is fine
    }

    #[test]
    fn raw_write_delivers_to_a_reader_and_gives_up_on_a_full_pipe() {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        use std::time::{Duration, Instant};

        let (mut reader, writer) = std::io::pipe().unwrap();
        assert!(write_line_raw(
            writer.as_raw_fd(),
            "hello\n",
            Duration::from_millis(500)
        ));
        let mut buf = [0u8; 6];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello\n");

        // Fill the pipe until nothing more fits, the way a client that stopped reading does.
        let fd = writer.as_raw_fd();
        // SAFETY: plain fcntl/write calls on a pipe this test owns.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            let chunk = [b'x'; 4096];
            while libc::write(fd, chunk.as_ptr().cast(), chunk.len()) > 0 {}
        }
        let started = Instant::now();
        assert!(!write_line_raw(
            fd,
            "never delivered\n",
            Duration::from_millis(100)
        ));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "it must not block: {:?}",
            started.elapsed()
        );
    }
}
