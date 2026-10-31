//! Contract for the Whitaker isolation the development build standard asks for.
//!
//! Whitaker runs under the toolchain its installer manages, so `make lint` assigns it the literal
//! `RUSTFLAGS="-D warnings"`, which displaces every `rustflags` source (no parallel frontend flag,
//! no `mold`) and inherits nothing from the caller. These tests read the real `make -n lint`
//! output, so a recipe that drops the assignment fails here.

use std::process::Command;

use rstest::rstest;

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

/// Returns the complaints about a Whitaker line that is not isolated: it must clear an inherited
/// `CARGO_ENCODED_RUSTFLAGS`, which outranks `RUSTFLAGS`, and then assign the literal warning-only
/// `RUSTFLAGS`, which displaces every `rustflags` source and inherits nothing from the caller.
fn isolation_problems(line: &str) -> Vec<String> {
    let literal = "RUSTFLAGS=\"-D warnings\" ";
    let commands: Vec<&str> = line.split("&&").map(str::trim).collect();
    let clears_encoded = commands.contains(&"unset CARGO_ENCODED_RUSTFLAGS");
    let runs_literal = commands.iter().any(|command| {
        command
            .strip_prefix(literal)
            .is_some_and(|rest| rest.contains("whitaker"))
    });
    let mut problems = Vec::new();
    if !clears_encoded {
        problems.push(format!(
            "the Whitaker command must clear `CARGO_ENCODED_RUSTFLAGS` first: {line}"
        ));
    }
    if !runs_literal {
        problems.push(format!(
            "the Whitaker command must assign the literal `RUSTFLAGS=\"-D warnings\"`: {line}"
        ));
    }
    problems
}

/// Scenario: the real `make -n lint` output.
///
/// Invariant: Whitaker clears the encoded flags and assigns the literal `RUSTFLAGS`, so it takes none of
/// the development standard and inherits nothing from the caller.
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
/// Invariant: only the cleared, literal warning-only assignment passes.
#[rstest]
#[case::isolated(
    "unset CARGO_ENCODED_RUSTFLAGS && RUSTFLAGS=\"-D warnings\" whitaker --all -- --all-targets",
    true
)]
#[case::encoded_flags_not_cleared(
    "RUSTFLAGS=\"-D warnings\" whitaker --all -- --all-targets",
    false
)]
#[case::none("whitaker --all", false)]
#[case::development_flags("RUSTFLAGS=\"-D warnings -Zthreads=8\" whitaker --all", false)]
#[case::inherited("RUSTFLAGS=\"$RUSTFLAGS -D warnings\" whitaker --all", false)]
fn only_the_literal_rustflags_isolate_whitaker(
    #[case] line: &str,
    #[case] isolated: bool,
) -> Result<(), String> {
    if isolation_problems(line).is_empty() == isolated {
        Ok(())
    } else {
        Err(format!("isolated should be {isolated}: {line}"))
    }
}
