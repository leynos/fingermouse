//! The recipes run against the repository with stand-in tools.
//!
//! The stand-ins are shell one-liners handed to Make through the tool
//! variables, so the tests touch no filesystem API: the shell writes the log,
//! and `cat` reads it back.

use std::{
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// A stand-in tool: a shell command that logs its arguments and exits.
struct FakeTool {
    command: String,
    log: String,
}

impl FakeTool {
    /// A tool that exits with `exit_code`, logging to a path unique to this
    /// process and call under the system temporary directory.
    fn new(exit_code: i32) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let log_path = std::env::temp_dir().join(format!(
            "yaml-lint-boundary-{}-{sequence}.log",
            std::process::id()
        ));
        let log = log_path.to_string_lossy().into_owned();
        // `$$@` reaches the shell as `$@`: Make would expand a bare `$@`.
        let command = format!(
            "sh -c 'for a in \"$$@\"; do printf \"%s\\n\" \"$$a\"; done > {log}; exit {exit_code}' sh"
        );
        Self { command, log }
    }

    /// The arguments the tool logged, one per line.
    fn logged(&self) -> String {
        let read = Command::new("cat").arg(&self.log).output();
        read.map_or_else(
            |error| format!("could not read {}: {error}", self.log),
            |printed| String::from_utf8_lossy(&printed.stdout).into_owned(),
        )
    }
}

fn make(target: &str, variable: &str, tool: &FakeTool) -> Output {
    match Command::new("make")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg(target)
        .arg(format!("{variable}={}", tool.command))
        .env_remove("MAKEFLAGS")
        .env_remove("MAKELEVEL")
        .env_remove("MFLAGS")
        .env_remove("MAKEOVERRIDES")
        .output()
    {
        Ok(output) => output,
        Err(error) => panic!("could not run make {target}: {error}"),
    }
}

fn tracked_yaml() -> Vec<String> {
    let output = match Command::new("git")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["ls-files", "-z", "*.yml", "*.yaml"])
        .output()
    {
        Ok(output) => output,
        Err(error) => panic!("could not run git: {error}"),
    };
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect();
    files.sort_unstable();
    files
}

#[test]
fn make_yamllint_hands_the_tool_strict_mode_and_the_tracked_yaml() {
    let tool = FakeTool::new(0);

    let output = make("yamllint", "YAMLLINT", &tool);

    assert!(output.status.success(), "{output:?}");
    let logged = tool.logged();
    let mut arguments = logged.lines();
    assert_eq!(arguments.next(), Some("--strict"));
    assert_eq!(arguments.next(), Some("--"));
    let mut given: Vec<&str> = arguments.collect();
    given.sort_unstable();
    assert_eq!(given, tracked_yaml());
}

#[test]
fn a_failing_yamllint_fails_the_make_target() {
    let tool = FakeTool::new(1);

    assert!(!make("yamllint", "YAMLLINT", &tool).status.success());
}

#[test]
fn a_failing_actionlint_fails_the_make_target() {
    let tool = FakeTool::new(1);

    assert!(!make("actionlint", "ACTIONLINT", &tool).status.success());
}

#[test]
fn a_dry_run_of_lint_reaches_both_tools() {
    let dry_run = Command::new("make")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["-n", "lint"])
        .env_remove("MAKEFLAGS")
        .env_remove("MAKELEVEL")
        .env_remove("MFLAGS")
        .env_remove("MAKEOVERRIDES")
        .output();
    let Ok(finished) = dry_run else {
        panic!("could not run make -n lint");
    };

    let plan = String::from_utf8_lossy(&finished.stdout);
    assert!(plan.contains("yamllint --strict"), "{plan}");
    assert!(
        plan.lines().any(|line| line.trim() == "actionlint"),
        "{plan}"
    );
}
