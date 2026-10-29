//! TTY backends.
//!
//! [`Tty`] abstracts the connection the terminal emulation talks to: it
//! provides the byte streams, resizing, and the child process handle. The
//! default backend [`LocalTty`] spawns a shell on the local machine; the
//! trait leaves room for future ssh or serial backends.

mod local;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use portable_pty::Child;

pub use self::local::LocalTty;

/// Reader half of a TTY.
pub type TtyReader = Box<dyn io::Read + Send>;

/// Writer half of a TTY.
pub type TtyWriter = Box<dyn io::Write + Send>;

/// Child process of a TTY.
pub type TtyChild = Box<dyn Child + Send + Sync>;

/// Size of a TTY.
///
/// Mirrors `portable_pty::PtySize` without leaking the dependency through
/// the whole API surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TtySize {
    /// Number of rows.
    pub rows: u16,
    /// Number of columns.
    pub cols: u16,
    /// Width of a cell in pixels.
    pub pixel_width: u16,
    /// Height of a cell in pixels.
    pub pixel_height: u16,
}

impl From<TtySize> for portable_pty::PtySize {
    fn from(size: TtySize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        }
    }
}

/// A pseudoterminal connection.
pub trait Tty: Send {
    /// Get a new reader for the TTY output.
    fn reader(&mut self) -> io::Result<TtyReader>;

    /// Take the writer for the TTY input.
    ///
    /// There is only one writer; taking it twice returns an error.
    fn writer(&mut self) -> io::Result<TtyWriter>;

    /// Resize the TTY.
    fn resize(&mut self, size: TtySize) -> io::Result<()>;

    /// Access the child process spawned inside the TTY.
    fn child(&mut self) -> &mut (dyn Child + Send + Sync);
}

/// Configuration for spawning a [`Tty`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Shell options.
    ///
    /// [`None`] will use the default shell.
    pub shell: Option<Shell>,

    /// Shell startup directory.
    pub working_directory: Option<PathBuf>,

    /// Extra environment variables.
    pub env: HashMap<String, String>,
}

/// Shell options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    /// Path to a shell program to run on startup.
    pub program: String,

    /// Arguments passed to the shell.
    pub args: Vec<String>,
}

impl Shell {
    /// Create a new shell configuration.
    pub fn new(program: String, args: Vec<String>) -> Self {
        Self { program, args }
    }
}
