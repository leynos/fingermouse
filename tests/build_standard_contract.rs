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
//! host through `BUILD_HOST_OS`, because mold is added on Linux alone. Both
//! files are read as text, so the contract needs no parser dependency; the
//! readers are driven against fixtures first, because a rule exercised only
//! over this repository's own compliant files would pass whether or not it
//! detects anything.

use std::process::Command;

const CONFIG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/.cargo/config.toml"));
const TOOLCHAIN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/rust-toolchain.toml"));

/// The parallel-frontend flag every `rustflags` source carries on a nightly pin.
const THREADS_FLAG: &str = "-Zthreads=8";
/// The linker flag the Linux source adds, normalized to one token.
const MOLD_FLAG: &str = "-Clink-arg=-fuse-ld=mold";
/// Makefile targets that build for development. A command in one either
/// assigns `RUSTFLAGS` with the standard flags or assigns none and so takes the
/// configuration's.
const DEVELOPMENT_TARGETS: [&str; 4] = ["test", "typecheck", "lint", "build"];
/// Makefile targets that measure or ship, so every command assigns `RUSTFLAGS`
/// and none carries a standard flag.
const HELD_OUT_TARGETS: [&str; 2] = ["coverage", "release"];

/// Joins `-C value` pairs into `-Cvalue`, so both spellings compare equal.
fn normalized(flags: &[String]) -> Vec<String> {
    let mut joined: Vec<String> = Vec::new();
    for flag in flags {
        match joined.last_mut() {
            Some(last) if last == "-C" => *last = format!("-C{flag}"),
            _ => joined.push(flag.clone()),
        }
    }
    joined
}

/// Returns whether a flag list names one flag.
fn names(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|candidate| candidate == flag)
}

/// Returns whether the pinned channel is a nightly one.
fn pins_nightly(toolchain: &str) -> bool {
    toolchain
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("channel"))
        .any(|line| line.contains("\"nightly"))
}

/// Returns the quoted strings in one line, in order.
fn quoted(line: &str) -> Vec<String> {
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// Returns every `rustflags` source in a Cargo configuration, by table name.
///
/// A table header is a line opening with `[`, and its `rustflags` entry is a
/// one-line array of strings, which is the shape the standard prescribes. An
/// entry spread over several lines is refused rather than half read.
fn sources(config: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let mut table = String::new();
    let mut found = Vec::new();
    for line in config.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            line.trim_matches(|c| c == '[' || c == ']')
                .clone_into(&mut table);
        } else if let Some(value) = line.strip_prefix("rustflags") {
            if !value.contains(']') {
                return Err(format!(
                    "`rustflags` in [{table}] spans lines; keep it on one"
                ));
            }
            found.push((table.clone(), normalized(&quoted(value))));
        }
    }
    Ok(found)
}

/// Returns whether a table applies on Linux alone.
fn is_linux_table(table: &str) -> bool {
    table.starts_with("target.") && table.contains("linux")
}

/// Returns every complaint about the configuration sources.
fn config_problems(config: &str, nightly: bool) -> Result<Vec<String>, String> {
    let found = sources(config)?;
    let mut problems = Vec::new();
    if found.is_empty() {
        problems.push("no rustflags source".to_owned());
    }
    if !found.iter().any(|(table, _)| is_linux_table(table)) {
        problems.push("no Linux target table carries rustflags".to_owned());
    }
    if nightly && !found.iter().any(|(table, _)| table == "build") {
        problems.push("no [build] rustflags for non-Linux hosts".to_owned());
    }
    for (table, flags) in &found {
        if names(flags, THREADS_FLAG) != nightly {
            problems.push(format!(
                "[{table}] {} {THREADS_FLAG}",
                if nightly { "drops" } else { "names" }
            ));
        }
        if names(flags, MOLD_FLAG) != is_linux_table(table) {
            problems.push(format!("[{table}] gets mold wrong: {flags:?}"));
        }
    }
    let mut stripped: Vec<Vec<&String>> = found
        .iter()
        .map(|(_, flags)| flags.iter().filter(|flag| *flag != MOLD_FLAG).collect())
        .collect();
    stripped.dedup();
    if stripped.len() > 1 {
        problems.push(format!("sources differ beyond the linker: {stripped:?}"));
    }
    Ok(problems)
}

/// Returns the `RUSTFLAGS` a `make -n` output line assigns, if it runs Cargo or
/// Whitaker: `None` when it assigns none, an error when the form is unreadable.
fn assigned_rustflags(line: &str) -> Result<Option<Vec<String>>, String> {
    if !(line.contains("cargo") || line.contains("whitaker")) {
        return Ok(None);
    }
    if let Some((_, rest)) = line.split_once("RUSTFLAGS=\"") {
        let (assigned, _) = rest
            .split_once('"')
            .ok_or_else(|| format!("unterminated RUSTFLAGS in `{line}`"))?;
        // The recipes prepend the caller's own flags with these expansions; they
        // are not standard flags, and glued to the next word they would hide it.
        let own_flags = assigned
            .replace("${RUSTFLAGS:+$RUSTFLAGS }", " ")
            .replace("${RUSTFLAGS-}", "");
        let words: Vec<String> = own_flags.split_whitespace().map(str::to_owned).collect();
        return Ok(Some(normalized(&words)));
    }
    // Any other spelling still replaces the configuration's sources, so a form
    // this reader cannot parse fails rather than passing.
    if line.contains("RUSTFLAGS=") {
        return Err(format!("unreadable RUSTFLAGS assignment in `{line}`"));
    }
    Ok(None)
}

/// Returns the assignment of each cargo or whitaker command `make -n` printed,
/// `None` where a command assigns none.
fn commands_from(stdout: &str) -> Result<Vec<Option<Vec<String>>>, String> {
    // A recipe continued with a trailing backslash is one command.
    let joined = stdout.replace("\\\n", " ");
    joined
        .lines()
        .filter(|line| !line.trim_start().starts_with("echo"))
        .filter(|line| line.contains("cargo") || line.contains("whitaker"))
        .map(assigned_rustflags)
        .collect()
}

/// Runs `make -n` for a target on the named host and returns its commands, or
/// `None` when the Makefile defines no such target.
fn make_rustflags(target: &str, host: &str) -> Result<Option<Vec<Option<Vec<String>>>>, String> {
    let output = Command::new("make")
        .args(["-n", "-B", &format!("BUILD_HOST_OS={host}"), target])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|error| format!("running make: {error}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return if stderr.contains("No rule to make target") {
            Ok(None)
        } else {
            Err(format!("`make -n {target}` failed: {stderr}"))
        };
    }
    commands_from(&String::from_utf8_lossy(&output.stdout)).map(Some)
}

/// Returns every complaint about the development targets on one host: an
/// assigned `RUSTFLAGS` carries the frontend flag on a nightly pin, and carries
/// mold exactly when the host is Linux. Also returns how many assignments it
/// read, so a test can refuse to pass over nothing.
fn development_problems(host: &str, nightly: bool) -> Result<(Vec<String>, usize), String> {
    let mut problems = Vec::new();
    let mut read = 0;
    for target in DEVELOPMENT_TARGETS {
        for flags in make_rustflags(target, host)?
            .into_iter()
            .flatten()
            .flatten()
        {
            read += 1;
            if names(&flags, THREADS_FLAG) != nightly {
                problems.push(format!(
                    "`make {target}` on {host} gets {THREADS_FLAG} wrong: {flags:?}"
                ));
            }
            if names(&flags, MOLD_FLAG) != (host == "Linux") {
                problems.push(format!(
                    "`make {target}` on {host} gets mold wrong: {flags:?}"
                ));
            }
        }
    }
    Ok((problems, read))
}

/// Returns every complaint about the held-out targets: each command assigns
/// `RUSTFLAGS`, since only an assignment displaces the configuration's sources,
/// and none names a standard flag.
fn held_out_problems() -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    for target in HELD_OUT_TARGETS {
        for assigned in make_rustflags(target, "Linux")?.into_iter().flatten() {
            problems.extend(held_out_command_problems(target, assigned.as_deref()));
        }
    }
    Ok(problems)
}

/// Returns every complaint about one held-out command: it assigns nothing, so
/// it takes the configuration's flags, or the assignment names a standard flag.
fn held_out_command_problems(target: &str, assigned: Option<&[String]>) -> Vec<String> {
    let Some(flags) = assigned else {
        return vec![format!(
            "`make {target}` runs a command that takes the configuration's flags"
        )];
    };
    [THREADS_FLAG, MOLD_FLAG]
        .into_iter()
        .filter(|flag| names(flags, flag))
        .map(|flag| format!("`make {target}` takes {flag}"))
        .collect()
}

const LINUX_TABLE: &str = "[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\n";
const NIGHTLY: &str = "[toolchain]\nchannel = \"nightly-2026-05-28\"\n";
const STABLE: &str = "[toolchain]\nchannel = \"1.94.0\"\n";

/// Scenario: configurations of each shape, read on a nightly and a stable pin.
///
/// Invariant: a compliant nightly file passes, and each way of losing the
/// frontend flag, losing mold, naming mold beyond Linux, or letting a source
/// drift is reported; a stable pin refuses the frontend flag it cannot take.
#[test]
fn the_configuration_reader_reports_each_defect() {
    let nightly_ok = format!(
        "[build]\nrustflags = [\"-Zthreads=8\"]\n{LINUX_TABLE}rustflags = [\"-Zthreads=8\", \"-Clink-arg=-fuse-ld=mold\"]\n"
    );
    let spelled_apart = nightly_ok.replace(
        "\"-Clink-arg=-fuse-ld=mold\"",
        "\"-C\", \"link-arg=-fuse-ld=mold\"",
    );
    let stable_ok = format!("{LINUX_TABLE}rustflags = [\"-Clink-arg=-fuse-ld=mold\"]\n");
    let cases: [(&str, bool, usize); 9] = [
        (&nightly_ok, true, 0),
        (&spelled_apart, true, 0),
        (&stable_ok, false, 0),
        (
            &nightly_ok.replace("[\"-Zthreads=8\"]", "[\"-Dwarnings\"]"),
            true,
            2,
        ),
        (
            &nightly_ok.replace(
                "[\"-Zthreads=8\", \"-Clink-arg=-fuse-ld=mold\"]",
                "[\"-Zthreads=8\"]",
            ),
            true,
            1,
        ),
        (
            &nightly_ok.replace(
                "[build]\nrustflags = [\"-Zthreads=8\"]",
                "[build]\nrustflags = [\"-Zthreads=8\", \"-Clink-arg=-fuse-ld=mold\"]",
            ),
            true,
            1,
        ),
        (
            &nightly_ok.replace("[build]\nrustflags = [\"-Zthreads=8\"]\n", ""),
            true,
            1,
        ),
        (
            &stable_ok.replace("[\"-Clink", "[\"-Zthreads=8\", \"-Clink"),
            false,
            1,
        ),
        ("", true, 3),
    ];
    for (config, nightly, expected) in cases {
        let found = config_problems(config, nightly).expect("the fixture reads");
        assert_eq!(
            found.len(),
            expected,
            "{config:?} on nightly={nightly}: {found:?}"
        );
    }
    assert!(config_problems("[build]\nrustflags = [\n  \"-Zthreads=8\",\n]\n", true).is_err());
    assert!(pins_nightly(NIGHTLY) && !pins_nightly(STABLE));
}

/// Scenario: `make -n` output lines in each spelling of an assignment.
///
/// Invariant: a quoted assignment is read, a line assigning none reads as
/// `None`, a command that is not Cargo is ignored, and an unquoted assignment
/// fails rather than passing.
#[test]
fn the_command_reader_reads_only_what_it_understands() {
    let read = |line| assigned_rustflags(line).expect("the fixture reads");
    assert_eq!(
        read("RUSTFLAGS=\"-D warnings -Zthreads=8\" cargo test"),
        Some(vec!["-D".into(), "warnings".into(), THREADS_FLAG.into()])
    );
    assert_eq!(
        read("RUSTFLAGS=\"${RUSTFLAGS:+$RUSTFLAGS }-Zthreads=8\" cargo check"),
        Some(vec![THREADS_FLAG.into()])
    );
    assert_eq!(
        read("RUSTFLAGS=\"${RUSTFLAGS-}\" cargo build --release"),
        Some(vec![])
    );
    assert_eq!(read("cargo clippy --all-targets"), None);
    assert_eq!(read("echo RUSTFLAGS=-Zthreads=8"), None);
    assert!(assigned_rustflags("RUSTFLAGS=-Zthreads=8 cargo test").is_err());
    assert!(assigned_rustflags("RUSTFLAGS=\"-Zthreads=8 cargo test").is_err());
    let joined =
        commands_from("RUSTFLAGS=\"-A\" \\\ncargo test\nmake other\n").expect("the fixture reads");
    assert_eq!(joined, vec![Some(vec!["-A".to_owned()])]);
}

#[test]
fn every_rustflags_source_is_consistent_with_the_pin() {
    let problems =
        config_problems(CONFIG, pins_nightly(TOOLCHAIN)).expect("read the configuration sources");
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn development_targets_restate_the_flags_on_linux() {
    let (problems, read) =
        development_problems("Linux", pins_nightly(TOOLCHAIN)).expect("read `make -n` output");
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(
        read > 0,
        "no development target assigns RUSTFLAGS, so the check above proves nothing"
    );
}

#[test]
fn development_targets_keep_the_frontend_but_not_mold_elsewhere() {
    let (problems, _) =
        development_problems("Darwin", pins_nightly(TOOLCHAIN)).expect("read `make -n` output");
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Coverage measures and release ships, so both stay on the default flags.
#[test]
fn coverage_and_release_take_neither_flag() {
    let problems = held_out_problems().expect("read `make -n` output");
    assert!(problems.is_empty(), "{problems:#?}");
}
