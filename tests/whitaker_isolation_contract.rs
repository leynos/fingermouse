//! Contract for the Whitaker isolation the development build standard asks for.
//!
//! Whitaker runs under the toolchain its installer manages, so `make lint` assigns it the literal
//! `RUSTFLAGS="-D warnings"`, which displaces every `rustflags` source (no parallel frontend flag,
//! no `mold`) and inherits nothing from the caller. These tests read the real `make -n lint`
//! output, so a recipe that drops the assignment fails here.

use std::process::Command;

/// Returns the `make -n lint` line that runs Whitaker.
fn whitaker_line() -> Result<String, String> {
    let output = Command::new("make")
        .args(["-n", "-B", "lint"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|error| format!("running make: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`make -n lint` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.contains("whitaker"))
        .map(str::to_owned)
        .ok_or_else(|| "`make -n lint` runs no Whitaker command".to_owned())
}

/// Returns the complaint about a Whitaker line that does not assign the literal warning-only
/// `RUSTFLAGS`, which displaces every `rustflags` source and inherits nothing from the caller.
fn isolation_problems(line: &str) -> Vec<String> {
    let literal = "RUSTFLAGS=\"-D warnings\" ";
    let runs_literal = line.split("&&").map(str::trim).any(|command| {
        command
            .strip_prefix(literal)
            .is_some_and(|rest| rest.contains("whitaker"))
    });
    if runs_literal {
        Vec::new()
    } else {
        vec![format!(
            "the Whitaker command must assign the literal `RUSTFLAGS=\"-D warnings\"`: {line}"
        )]
    }
}

/// Scenario: the real `make -n lint` output.
///
/// Invariant: Whitaker assigns the literal `RUSTFLAGS`, so it takes none of the development
/// standard and inherits nothing from the caller.
#[test]
fn make_lint_isolates_whitaker_with_the_literal_rustflags() -> Result<(), String> {
    let problems = isolation_problems(&whitaker_line()?);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Scenario: Whitaker commands that assign no, inherited or development `RUSTFLAGS`.
///
/// Invariant: only the literal warning-only assignment passes.
#[test]
fn a_whitaker_command_without_the_literal_rustflags_is_reported() -> Result<(), String> {
    let accepted = ["RUSTFLAGS=\"-D warnings\" whitaker --all -- --all-targets"];
    let rejected = [
        "whitaker --all",
        "RUSTFLAGS=\"-D warnings -Zthreads=8\" whitaker --all",
        "RUSTFLAGS=\"$RUSTFLAGS -D warnings\" whitaker --all",
    ];
    for line in accepted {
        if !isolation_problems(line).is_empty() {
            return Err(format!("must pass: {line}"));
        }
    }
    for line in rejected {
        if isolation_problems(line).is_empty() {
            return Err(format!("must be reported: {line}"));
        }
    }
    Ok(())
}
