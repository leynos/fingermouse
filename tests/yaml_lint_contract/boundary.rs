//! The recipes run against the repository with stand-in tools.
//!
//! The stand-ins are shell one-liners handed to Make through the tool
//! variables, so the tests touch no filesystem API: the shell writes the log,
//! and `cat` reads it back. Every process boundary returns a `Result`, and the
//! tests decide what a failure means.

use std::{
    error::Error,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

type Outcome<T> = Result<T, Box<dyn Error>>;

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// Variables an outer `make test` exports, which change what a nested `make`
/// prints and which jobserver it joins.
const OUTER_MAKE: [&str; 4] = ["MAKEFLAGS", "MAKELEVEL", "MFLAGS", "MAKEOVERRIDES"];

/// A `make` invocation in the repository, in a clean Make environment.
fn make_command() -> Command {
    let mut command = Command::new("make");
    command.current_dir(env!("CARGO_MANIFEST_DIR"));
    for variable in OUTER_MAKE {
        command.env_remove(variable);
    }
    command
}

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
    fn logged(&self) -> Outcome<String> {
        let printed = Command::new("cat").arg(&self.log).output()?;
        if !printed.status.success() {
            return Err(format!("the stand-in logged nothing to {}", self.log).into());
        }
        Ok(String::from_utf8_lossy(&printed.stdout).into_owned())
    }
}

/// Runs one Make target with the tool variable pointed at the stand-in.
fn make(target: &str, variable: &str, tool: &FakeTool) -> Outcome<Output> {
    Ok(make_command()
        .arg(target)
        .arg(format!("{variable}={}", tool.command))
        .output()?)
}

/// The recipes `make -n lint` would run, as text.
fn dry_run_of_lint() -> Outcome<String> {
    let finished = make_command().args(["-n", "lint"]).output()?;
    if !finished.status.success() {
        return Err(format!(
            "make -n lint failed: {}",
            String::from_utf8_lossy(&finished.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&finished.stdout).into_owned())
}

/// The Git-tracked YAML files, sorted; a Git failure is an error, not an
/// empty list.
fn tracked_yaml() -> Outcome<Vec<String>> {
    let listed = Command::new("git")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["ls-files", "-z", "*.yml", "*.yaml"])
        .output()?;
    if !listed.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&listed.stderr)
        )
        .into());
    }
    let mut files: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect();
    files.sort_unstable();
    Ok(files)
}

#[test]
fn make_yamllint_hands_the_tool_strict_mode_and_the_tracked_yaml() {
    let tool = FakeTool::new(0);

    let output = make("yamllint", "YAMLLINT", &tool).expect("run make yamllint");

    assert!(output.status.success(), "{output:?}");
    let logged = tool.logged().expect("read the stand-in's log");
    let mut arguments = logged.lines();
    assert_eq!(arguments.next(), Some("--strict"));
    assert_eq!(arguments.next(), Some("--"));
    let mut given: Vec<&str> = arguments.collect();
    given.sort_unstable();
    assert_eq!(given, tracked_yaml().expect("list the tracked YAML"));
}

#[test]
fn a_failing_yamllint_fails_the_make_target() {
    let tool = FakeTool::new(1);

    let output = make("yamllint", "YAMLLINT", &tool).expect("run make yamllint");

    assert!(!output.status.success());
}

#[test]
fn a_failing_actionlint_fails_the_make_target() {
    let tool = FakeTool::new(1);

    let output = make("actionlint", "ACTIONLINT", &tool).expect("run make actionlint");

    assert!(!output.status.success());
}

#[test]
fn a_dry_run_of_lint_reaches_both_tools() {
    let plan = dry_run_of_lint().expect("plan make lint");

    assert!(plan.contains("yamllint --strict"), "{plan}");
    assert!(
        plan.lines().any(|line| line.trim() == "actionlint"),
        "{plan}"
    );
}
