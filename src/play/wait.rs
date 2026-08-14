//! A pause step between English and Russian playback in a [`super::play`] sequence.

use std::io::{self, Read};
use std::os::unix::io::RawFd;
use std::thread::sleep;
use std::time::Duration;

use super::{PlayError, PlayFn};

/// Return a function that prints a blank line then sleeps for `seconds`.
pub fn play_wait(seconds: u64) -> PlayFn {
    Box::new(move || {
        println!();
        sleep(Duration::from_secs(seconds));
        Ok(())
    })
}

fn get_termios(fd: RawFd) -> io::Result<libc::termios> {
    // SAFETY: `term` is fully written by tcgetattr before any field is read;
    // termios has no invariants beyond being a plain C struct.
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut term) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(term)
    }
}

fn set_termios(fd: RawFd, term: &libc::termios) -> io::Result<()> {
    // SAFETY: `term` is a valid, fully-initialized termios (sourced from a
    // prior get_termios call), and tcsetattr only reads through the pointer.
    let result = unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, term) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Put `fd` into raw mode (no line buffering, no echo, one byte at a time),
/// mirroring Python's `tty.setraw`.
fn set_raw_mode(fd: RawFd, original: &libc::termios) -> io::Result<()> {
    let mut raw = *original;
    // SAFETY: `raw` is a valid termios; cfmakeraw only mutates its flags.
    unsafe { libc::cfmakeraw(&mut raw) };
    set_termios(fd, &raw)
}

fn read_one_char() -> io::Result<char> {
    let mut byte = [0u8; 1];
    io::stdin().read_exact(&mut byte)?;
    Ok(byte[0] as char)
}

/// Return a function that prints which keys are waited on, then blocks until
/// the space bar is pressed.
///
/// If `repeat_key` is given, pressing it calls `repeat_fn` and resumes
/// waiting instead of returning; any other key is ignored. Terminal mode is
/// restored around the `repeat_fn` call so its output isn't staircased by
/// raw mode's disabled CR-on-LF.
pub fn play_wait_key(repeat_key: Option<char>, repeat_fn: Option<PlayFn>) -> PlayFn {
    Box::new(move || {
        match repeat_key {
            Some(key) => println!("[space] continue, [{key}] replay"),
            None => println!("[space] continue"),
        }

        let fd: RawFd = 0; // stdin
        let original = get_termios(fd).map_err(PlayError::Termios)?;
        set_raw_mode(fd, &original).map_err(PlayError::Termios)?;

        let result = (|| -> Result<(), PlayError> {
            loop {
                let ch = read_one_char().map_err(PlayError::Termios)?;
                if ch == ' ' {
                    return Ok(());
                }
                if repeat_key == Some(ch)
                    && let Some(repeat_fn) = &repeat_fn
                {
                    set_termios(fd, &original).map_err(PlayError::Termios)?;
                    repeat_fn()?;
                    set_raw_mode(fd, &original).map_err(PlayError::Termios)?;
                }
            }
        })();

        set_termios(fd, &original).map_err(PlayError::Termios)?;
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn play_wait_returns_ok_immediately_for_zero_seconds() {
        assert!(play_wait(0)().is_ok());
    }

    // play_wait_key requires a real controlling terminal on fd 0 (tcgetattr
    // fails otherwise), so it isn't exercised by the test suite the way
    // play_wait is; it's covered by manual/interactive testing instead.

    #[test]
    fn get_termios_errors_without_a_terminal_on_the_given_fd() {
        // fd 999 is never a valid open fd, let alone a terminal, so this
        // exercises the error path deterministically.
        assert!(get_termios(999).is_err());
    }

    #[test]
    fn repeat_flag_type_is_usable_as_a_play_fn() {
        // Sanity check that a PlayFn closure can be constructed and invoked
        // through the same signature play_wait_key expects for repeat_fn,
        // without needing a real terminal.
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();
        let repeat_fn: PlayFn = Box::new(move || {
            called_clone.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert!(repeat_fn().is_ok());
        assert!(called.load(Ordering::SeqCst));
    }
}
