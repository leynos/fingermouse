//! Contract tests for the Rust build standard.
//!
//! The standard makes the parallel `rustc` frontend the default for every
//! development build on a nightly pin, and mold the default linker on Linux.
//! Cargo reads both from `.cargo/config.toml`, but it applies a single
//! `rustflags` source rather than merging them, and an assigned `RUSTFLAGS`
//! replaces every source. So the flags must be repeated in each configuration
//! source, restated wherever the Makefile assigns `RUSTFLAGS` for a development
//! target, and kept out of the coverage and release recipes, which measure or
//! ship and so stay on the default flags. A stable pin takes mold alone,
//! because `-Zthreads` is a nightly flag.
//!
//! The Makefile clauses run `make -n` and read the commands it would run,
//! rather than the Makefile's text, so a flag lost through a variable or a
//! recipe edit fails here. They run once as a Linux host and once as a macOS
//! host through `BUILD_HOST_OS`, because mold is added on Linux alone. The
//! readers are driven against fixtures first, because a rule exercised only
//! over this repository's own compliant files would pass whether or not it
//! detects anything.

#[path = "build_standard_support/readers.rs"]
mod readers;
use readers::{
    Assignment, CONFIG, Flags, Host, Pin, Problems, THREADS_FLAG, TOOLCHAIN, assigned_rustflags,
    commands_from, config_problems, development_problems, held_out_problems,
};

/// Turns a list of complaints into a test result.
fn none_of(problems: &Problems) -> Result<(), String> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{problems:#?}"))
    }
}

const LINUX_TABLE: &str = "[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\n";
const NIGHTLY: &str = "[toolchain]\nchannel = \"nightly-2026-05-28\"\n";
const STABLE: &str = "[toolchain]\nchannel = \"1.94.0\"\n";

/// Counts the problems the reader finds in a fixture configuration.
fn problem_count(config: &str, pin: Pin) -> Result<usize, String> {
    config_problems(config, pin).map(|problems| problems.len())
}

/// Scenario: configurations of each shape, read on a nightly and a stable pin.
///
/// Invariant: a compliant nightly file passes, and each way of losing the
/// frontend flag, losing mold, naming mold beyond Linux, or letting a source
/// drift is reported; a stable pin refuses the frontend flag it cannot take.
#[test]
fn the_configuration_reader_reports_each_defect() -> Result<(), String> {
    let nightly_ok = format!(
        "[build]\nrustflags = [\"-Zthreads=8\"]\n{LINUX_TABLE}rustflags = [\"-Zthreads=8\", \"-Clink-arg=-fuse-ld=mold\"]\n"
    );
    let spelled_apart = nightly_ok.replace(
        "\"-Clink-arg=-fuse-ld=mold\"",
        "\"-C\", \"link-arg=-fuse-ld=mold\"",
    );
    let stable_ok = format!("{LINUX_TABLE}rustflags = [\"-Clink-arg=-fuse-ld=mold\"]\n");
    let mold_in_build = nightly_ok.replace(
        "[build]\nrustflags = [\"-Zthreads=8\"]",
        "[build]\nrustflags = [\"-Zthreads=8\", \"-Clink-arg=-fuse-ld=mold\"]",
    );
    let linux_without_mold = nightly_ok.replace(
        "[\"-Zthreads=8\", \"-Clink-arg=-fuse-ld=mold\"]",
        "[\"-Zthreads=8\"]",
    );
    let cases = [
        (&nightly_ok, Pin::Nightly, 0),
        (&spelled_apart, Pin::Nightly, 0),
        (&stable_ok, Pin::Stable, 0),
        (
            &nightly_ok.replace("[\"-Zthreads=8\"]", "[\"-Dwarnings\"]"),
            Pin::Nightly,
            2,
        ),
        (&linux_without_mold, Pin::Nightly, 1),
        (&mold_in_build, Pin::Nightly, 1),
        (
            &nightly_ok.replace("[build]\nrustflags = [\"-Zthreads=8\"]\n", ""),
            Pin::Nightly,
            1,
        ),
        (
            &stable_ok.replace("[\"-Clink", "[\"-Zthreads=8\", \"-Clink"),
            Pin::Stable,
            1,
        ),
        (&String::new(), Pin::Nightly, 3),
    ];
    for (config, pin, expected) in cases {
        let found = problem_count(config, pin)?;
        if found != expected {
            return Err(format!("{config:?}: {found} problems, not {expected}"));
        }
    }
    if config_problems(
        "[build]\nrustflags = [\n  \"-Zthreads=8\",\n]\n",
        Pin::Nightly,
    )
    .is_ok()
    {
        return Err("a rustflags array spread over lines was read".to_owned());
    }
    if Pin::read(NIGHTLY) != Pin::Nightly || Pin::read(STABLE) != Pin::Stable {
        return Err("the pin reader misjudged a channel".to_owned());
    }
    Ok(())
}

/// Scenario: `make -n` output lines in each spelling of an assignment.
///
/// Invariant: a quoted assignment is read, a line assigning none reads as
/// unassigned, and an unquoted assignment fails rather than passing.
#[test]
fn the_command_reader_reads_only_what_it_understands() -> Result<(), String> {
    let flags = |words: &[&str]| Assignment::Flags(Flags::from_words(words.iter().copied()));
    let cases = [
        (
            "RUSTFLAGS=\"-D warnings -Zthreads=8\" cargo test",
            flags(&["-D", "warnings", THREADS_FLAG]),
        ),
        (
            "RUSTFLAGS=\"${RUSTFLAGS:+$RUSTFLAGS }-Zthreads=8\" cargo check",
            flags(&[THREADS_FLAG]),
        ),
        (
            "RUSTFLAGS=\"${RUSTFLAGS-}\" cargo build --release",
            flags(&[]),
        ),
        ("cargo clippy --all-targets", Assignment::Unassigned),
    ];
    for (line, expected) in cases {
        if assigned_rustflags(line)? != expected {
            return Err(format!("`{line}` was read wrongly"));
        }
    }
    for refused in [
        "RUSTFLAGS=-Zthreads=8 cargo test",
        "RUSTFLAGS=\"-Zthreads=8 cargo test",
    ] {
        if assigned_rustflags(refused).is_ok() {
            return Err(format!("`{refused}` was read, not refused"));
        }
    }
    let joined = commands_from("RUSTFLAGS=\"-A\" \\\ncargo test\necho cargo test\nmake other\n")?;
    if joined != vec![flags(&["-A"])] {
        return Err(format!("a continued command was read wrongly: {joined:?}"));
    }
    Ok(())
}

#[test]
fn every_rustflags_source_is_consistent_with_the_pin() -> Result<(), String> {
    none_of(&config_problems(CONFIG, Pin::read(TOOLCHAIN))?)
}

#[test]
fn development_targets_restate_the_flags_on_linux() -> Result<(), String> {
    let (problems, read) = development_problems(Host::Linux, Pin::read(TOOLCHAIN))?;
    none_of(&problems)?;
    if read == 0 {
        return Err(
            "no development target assigns RUSTFLAGS, so the check proves nothing".to_owned(),
        );
    }
    Ok(())
}

#[test]
fn development_targets_keep_the_frontend_but_not_mold_elsewhere() -> Result<(), String> {
    none_of(&development_problems(Host::Darwin, Pin::read(TOOLCHAIN))?.0)
}

/// Coverage measures and release ships, so both stay on the default flags.
#[test]
fn coverage_and_release_take_neither_flag() -> Result<(), String> {
    none_of(&held_out_problems()?)
}
