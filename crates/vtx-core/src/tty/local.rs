//! Local TTY backend.

use std::io;

use portable_pty::{CommandBuilder, MasterPty, native_pty_system};

use super::{Options, Shell, Tty, TtyChild, TtyReader, TtySize, TtyWriter};

/// Local TTY: a shell process on the machine running the terminal, hosted
/// in a pseudoterminal through `portable-pty`.
pub struct LocalTty {
    /// PTY master; owns the reader/writer/resize endpoints.
    master: Box<dyn MasterPty + Send>,

    /// Child process spawned inside the PTY.
    ///
    /// Held until [`Tty::take_child`] moves it out to the reaper.
    child: Option<TtyChild>,
}

impl LocalTty {
    /// Spawn the configured shell inside a new pseudoterminal.
    pub fn new(options: &Options, size: TtySize) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size.into()).map_err(io_error)?;

        let child = pair
            .slave
            .spawn_command(build_command(options))
            .map_err(io_error)?;

        // Drop the slave so the reader observes EOF once the child exits.
        drop(pair.slave);

        Ok(Self {
            master: pair.master,
            child: Some(child),
        })
    }
}

impl Tty for LocalTty {
    fn reader(&mut self) -> io::Result<TtyReader> {
        self.master.try_clone_reader().map_err(io_error)
    }

    fn writer(&mut self) -> io::Result<TtyWriter> {
        self.master.take_writer().map_err(io_error)
    }

    fn resize(&mut self, size: TtySize) -> io::Result<()> {
        self.master.resize(size.into()).map_err(io_error)
    }

    fn take_child(&mut self) -> io::Result<TtyChild> {
        self.child
            .take()
            .ok_or_else(|| io::Error::other("the child process was already taken from this TTY"))
    }
}

/// Translate a `portable-pty` error into an [`io::Error`].
fn io_error<E: std::fmt::Display>(error: E) -> io::Error {
    io::Error::other(format!("{error:#}"))
}

/// Build the command for the configured shell.
fn build_command(options: &Options) -> CommandBuilder {
    let mut cmd = match &options.shell {
        Some(shell) => shell_command(shell),
        None => default_shell(),
    };

    if let Some(cwd) = &options.working_directory {
        cmd.cwd(cwd);
    }
    for (key, value) in &options.env {
        cmd.env(key, value);
    }

    // Advertise terminal capabilities. M1 always reports `xterm-256color`;
    // the config layer will make this user-overridable.
    //
    // Unlike upstream, which mutates the global process environment before
    // forking, the variables are injected per command and leave the host
    // process untouched.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    cmd
}

/// Build a command for an explicit shell.
fn shell_command(shell: &Shell) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(&shell.program);
    cmd.args(&shell.args);
    cmd
}

/// The platform default shell.
fn default_shell() -> CommandBuilder {
    #[cfg(windows)]
    {
        // Alacritty's default: PowerShell ships with every Windows install.
        CommandBuilder::new("powershell")
    }

    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        CommandBuilder::new(shell)
    }
}
