//! Readers for the build standard's contract: the Cargo configuration sources
//! and the commands `make -n` prints, judged against a toolchain pin and a host.
//!
//! They live apart from the tests so the tests read as scenarios, and each
//! reader is small enough to be driven against fixtures. Everything here is
//! read as text, so the contract needs no parser dependency.

use std::process::Command;

pub const CONFIG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/.cargo/config.toml"));
pub const TOOLCHAIN: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/rust-toolchain.toml"));

/// The parallel-frontend flag every `rustflags` source carries on a nightly pin.
pub const THREADS_FLAG: &str = "-Zthreads=8";
/// The linker flag the Linux source adds, normalized to one token.
pub const MOLD_FLAG: &str = "-Clink-arg=-fuse-ld=mold";
/// Makefile targets that build for development. A command in one either
/// assigns `RUSTFLAGS` with the standard flags or assigns none and so takes the
/// configuration's.
const DEVELOPMENT_TARGETS: [&str; 4] = ["test", "typecheck", "lint", "build"];
/// Makefile targets that measure or ship, so every command assigns `RUSTFLAGS`
/// and none carries a standard flag.
const HELD_OUT_TARGETS: [&str; 2] = ["coverage", "release"];

/// A list of complaints about the repository.
pub type Problems = Vec<String>;

/// The channel the toolchain file pins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pin {
    Nightly,
    Stable,
}

impl Pin {
    /// Reads the pin from a `rust-toolchain.toml`.
    pub fn read(toolchain: &str) -> Self {
        let is_nightly = toolchain
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("channel"))
            .any(|line| line.contains("\"nightly"));
        if is_nightly {
            Self::Nightly
        } else {
            Self::Stable
        }
    }

    /// `-Zthreads` is a nightly flag, so only a nightly pin takes it.
    const fn takes_threads(self) -> bool {
        matches!(self, Self::Nightly)
    }
}

/// The host `make` is told it runs on, through `BUILD_HOST_OS`.
#[derive(Clone, Copy)]
pub enum Host {
    Linux,
    Darwin,
}

impl Host {
    /// The value `uname -s` reports for the host.
    const fn make_value(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::Darwin => "Darwin",
        }
    }

    /// mold ships for Linux alone.
    const fn takes_mold(self) -> bool {
        matches!(self, Self::Linux)
    }
}

/// A list of compiler flags, with `-C value` pairs joined into `-Cvalue` so
/// both spellings compare equal.
#[derive(Debug, PartialEq, Eq)]
pub struct Flags(Vec<String>);

impl Flags {
    /// Reads a flag list from its words.
    pub fn from_words<'a>(words: impl IntoIterator<Item = &'a str>) -> Self {
        let mut joined: Vec<String> = Vec::new();
        for word in words {
            match joined.last_mut() {
                Some(last) if last == "-C" => *last = format!("-C{word}"),
                _ => joined.push(word.to_owned()),
            }
        }
        Self(joined)
    }

    /// Returns whether the list names one flag.
    fn names(&self, flag: &str) -> bool {
        self.0.iter().any(|candidate| candidate == flag)
    }

    /// The list without the linker flag, which is the one that may differ.
    fn without_mold(&self) -> Vec<&String> {
        self.0.iter().filter(|flag| *flag != MOLD_FLAG).collect()
    }

    /// Returns whether the list carries the frontend and linker flags a pin and
    /// a host call for.
    fn meets(&self, pin: Pin, takes_mold: bool) -> Result<(), String> {
        if self.names(THREADS_FLAG) != pin.takes_threads() {
            return Err(format!("gets {THREADS_FLAG} wrong: {:?}", self.0));
        }
        if self.names(MOLD_FLAG) != takes_mold {
            return Err(format!("gets mold wrong: {:?}", self.0));
        }
        Ok(())
    }
}

/// One `rustflags` source in a Cargo configuration.
struct Source {
    table: String,
    flags: Flags,
}

impl Source {
    /// Returns whether the table applies on Linux alone.
    fn is_linux(&self) -> bool {
        self.table.starts_with("target.") && self.table.contains("linux")
    }

    /// Returns what is wrong with the source's flags for a pin: the frontend flag
    /// on a nightly pin only, and mold in a Linux table only.
    fn problem(&self, pin: Pin) -> Option<String> {
        let reason = self.flags.meets(pin, self.is_linux()).err()?;
        Some(format!("[{}] {reason}", self.table))
    }
}

/// One line of a Cargo configuration, as far as the standard reads it.
enum Line {
    Table(String),
    Rustflags(Flags),
    Other,
}

/// Returns the quoted strings in one line, in order.
fn quoted(line: &str) -> Vec<&str> {
    line.split('"').skip(1).step_by(2).collect()
}

/// Reads one configuration line. A `rustflags` entry is a one-line array of
/// strings, which is the shape the standard prescribes; an entry spread over
/// several lines is refused rather than half read.
fn read_line(line: &str) -> Result<Line, String> {
    if line.starts_with('[') {
        return Ok(Line::Table(
            line.trim_matches(|c| c == '[' || c == ']').to_owned(),
        ));
    }
    let Some(value) = line.strip_prefix("rustflags") else {
        return Ok(Line::Other);
    };
    if !value.contains(']') {
        return Err("a `rustflags` array spans lines; keep it on one".to_owned());
    }
    Ok(Line::Rustflags(Flags::from_words(quoted(value))))
}

/// Returns every `rustflags` source in a Cargo configuration.
fn sources(config: &str) -> Result<Vec<Source>, String> {
    let mut table = String::new();
    let mut found = Vec::new();
    for line in config
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
    {
        match read_line(line)? {
            Line::Table(name) => table = name,
            Line::Rustflags(flags) => found.push(Source {
                table: table.clone(),
                flags,
            }),
            Line::Other => {}
        }
    }
    Ok(found)
}

/// Returns the complaints about which sources exist: there must be a Linux
/// table, and a nightly pin needs a `[build]` source for the other hosts.
fn shape_problems(found: &[Source], pin: Pin) -> Problems {
    let checks = [
        (found.is_empty(), "no rustflags source"),
        (
            !found.iter().any(Source::is_linux),
            "no Linux target table carries rustflags",
        ),
        (
            pin.takes_threads() && !found.iter().any(|source| source.table == "build"),
            "no [build] rustflags for non-Linux hosts",
        ),
    ];
    checks
        .into_iter()
        .filter(|(failed, _)| *failed)
        .map(|(_, text)| text.to_owned())
        .collect()
}

/// Returns a complaint when the sources differ in anything but the linker, since
/// Cargo applies one source rather than merging them.
fn drift_problem(found: &[Source]) -> Option<String> {
    let mut stripped: Vec<Vec<&String>> = found
        .iter()
        .map(|source| source.flags.without_mold())
        .collect();
    stripped.dedup();
    (stripped.len() > 1).then(|| format!("sources differ beyond the linker: {stripped:?}"))
}

/// Returns every complaint about the configuration sources.
pub fn config_problems(config: &str, pin: Pin) -> Result<Problems, String> {
    let found = sources(config)?;
    let mut problems = shape_problems(&found, pin);
    problems.extend(found.iter().filter_map(|source| source.problem(pin)));
    problems.extend(drift_problem(&found));
    Ok(problems)
}

/// What one `make -n` command assigns to `RUSTFLAGS`.
#[derive(Debug, PartialEq, Eq)]
pub enum Assignment {
    Unassigned,
    Flags(Flags),
}

/// Reads the `RUSTFLAGS` a `make -n` output line assigns, if it runs Cargo or
/// Whitaker. An unreadable form is an error, because it still replaces the
/// configuration's sources and so must not pass.
pub fn assigned_rustflags(line: &str) -> Result<Assignment, String> {
    let Some((_, rest)) = line.split_once("RUSTFLAGS=\"") else {
        if line.contains("RUSTFLAGS=") {
            return Err(format!("unreadable RUSTFLAGS assignment in `{line}`"));
        }
        return Ok(Assignment::Unassigned);
    };
    let (assigned, _) = rest
        .split_once('"')
        .ok_or_else(|| format!("unterminated RUSTFLAGS in `{line}`"))?;
    // The recipes prepend the caller's own flags with these expansions; they are
    // not standard flags, and glued to the next word they would hide it.
    let own = assigned
        .replace("${RUSTFLAGS:+$RUSTFLAGS }", " ")
        .replace("${RUSTFLAGS-}", "");
    Ok(Assignment::Flags(Flags::from_words(own.split_whitespace())))
}

/// Reads the assignment of each cargo or whitaker command `make -n` printed.
pub fn commands_from(stdout: &str) -> Result<Vec<Assignment>, String> {
    // A recipe continued with a trailing backslash is one command.
    let joined = stdout.replace("\\\n", " ");
    joined
        .lines()
        .filter(|line| !line.trim_start().starts_with("echo"))
        .filter(|line| line.contains("cargo") || line.contains("whitaker"))
        .map(assigned_rustflags)
        .collect()
}

/// Runs `make -n` for a target on a host and reads its commands, or returns
/// `None` when the Makefile defines no such target.
fn make_commands(target: &str, host: Host) -> Result<Option<Vec<Assignment>>, String> {
    let output = Command::new("make")
        .args([
            "-n",
            "-B",
            &format!("BUILD_HOST_OS={}", host.make_value()),
            target,
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|error| format!("running make: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let undefined = stderr.contains("No rule to make target");
        return if undefined {
            Ok(None)
        } else {
            Err(format!("`make -n {target}` failed: {stderr}"))
        };
    }
    commands_from(&String::from_utf8_lossy(&output.stdout)).map(Some)
}

/// Returns the complaint about one development command, if any: an assigned
/// `RUSTFLAGS` restates the frontend flag on a nightly pin, and mold on Linux.
fn development_problem(
    target: &str,
    host: Host,
    pin: Pin,
    assignment: &Assignment,
) -> Option<String> {
    let Assignment::Flags(flags) = assignment else {
        return None;
    };
    let reason = flags.meets(pin, host.takes_mold()).err()?;
    Some(format!("`make {target}` on {} {reason}", host.make_value()))
}

/// Returns every complaint about the development targets on one host, and how
/// many assignments it read, so a test can refuse to pass over nothing.
pub fn development_problems(host: Host, pin: Pin) -> Result<(Problems, usize), String> {
    let mut problems = Vec::new();
    let mut read = 0;
    for target in DEVELOPMENT_TARGETS {
        let commands = make_commands(target, host)?.unwrap_or_default();
        read += commands
            .iter()
            .filter(|command| **command != Assignment::Unassigned)
            .count();
        problems.extend(
            commands
                .iter()
                .filter_map(|command| development_problem(target, host, pin, command)),
        );
    }
    Ok((problems, read))
}

/// Returns every complaint about one held-out command: it assigns nothing, so it
/// takes the configuration's flags, or the assignment names a standard flag.
fn held_out_command_problems(target: &str, assignment: &Assignment) -> Problems {
    let Assignment::Flags(flags) = assignment else {
        return vec![format!(
            "`make {target}` runs a command that takes the configuration's flags"
        )];
    };
    [THREADS_FLAG, MOLD_FLAG]
        .into_iter()
        .filter(|flag| flags.names(flag))
        .map(|flag| format!("`make {target}` takes {flag}"))
        .collect()
}

/// Returns every complaint about the held-out targets: each command assigns
/// `RUSTFLAGS`, since only an assignment displaces the configuration's sources.
pub fn held_out_problems() -> Result<Problems, String> {
    let mut problems = Vec::new();
    for target in HELD_OUT_TARGETS {
        let commands = make_commands(target, Host::Linux)?.unwrap_or_default();
        problems.extend(
            commands
                .iter()
                .flat_map(|command| held_out_command_problems(target, command)),
        );
    }
    Ok(problems)
}
