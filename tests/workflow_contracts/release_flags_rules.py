"""Hold the release builds' platform linker, and the lint commands' warning denial.

A release ships from the platform linker, without the build standard's nightly-only
flags, and an assigned `RUSTFLAGS` displaces every `rustflags` table in
`.cargo/config.toml`. So every release build says so itself:

* a build on a GitHub-hosted runner assigns `RUSTFLAGS: -D warnings` in its own
  `env:` block, which is the value the toolchain action exports anyway; and
* a build inside a BSD VM, whose packaged Rust is stable, assigns an empty
  `RUSTFLAGS` on the command line itself.

The reading is over parsed documents, so a step is judged by its own mapping and
a sibling step's `env:` neither lends nor hides a value.
"""

from __future__ import annotations

import re
import typing as typ

from workflow_reader import Document, Step

#: The release workflow, by file name.
RELEASE: typ.Final[str] = "release.yml"
#: A command that builds a release, in either spelling the workflow uses.
BUILD: typ.Final[re.Pattern[str]] = re.compile(
    r"\b(?:cargo|cross \+stable) build --release\b"
)
#: The value a hosted-runner build must assign.
HOSTED_VALUE: typ.Final[str] = "-D warnings"
#: How many release builds the workflow holds: three hosted, two in a VM.
EXPECTED_BUILDS: typ.Final[int] = 5
#: An empty assignment at the head of a command line.
EMPTY_ASSIGNMENT: typ.Final[re.Pattern[str]] = re.compile(r"""^RUSTFLAGS=(?:''|"")\s""")


def _steps(document: Document) -> list[tuple[str, Step]]:
    """Return every step of every job as `(job id, step)`."""
    jobs = document.get("jobs")
    if not isinstance(jobs, dict):
        return []
    return [
        (str(job_id), step)
        for job_id, job in jobs.items()
        if isinstance(job, dict)
        for step in job.get("steps", [])
        if isinstance(step, dict)
    ]


def _commands(step: Step) -> list[tuple[str, bool]]:
    """Return the step's command lines as `(line, runs in a VM)`.

    A VM action carries its script in `with.run`; a plain step in `run`.
    """
    plain = step.get("run")
    with_block = step.get("with")
    in_vm = with_block.get("run") if isinstance(with_block, dict) else None
    return [(line.strip(), False) for line in str(plain or "").splitlines()] + [
        (line.strip(), True) for line in str(in_vm or "").splitlines()
    ]


def release_build_violations(documents: dict[str, Document]) -> list[str]:
    """List every way the release workflow's builds miss the platform linker.

    Parameters
    ----------
    documents : dict[str, Document]
        The parsed workflows by file name.

    Returns
    -------
    list[str]
        One message per breach; empty when every release build assigns its own
        value as above and exactly `EXPECTED_BUILDS` builds were found.

    """
    document = documents.get(RELEASE)
    if document is None:
        return [f"{RELEASE} is not among the workflows"]
    problems: list[str] = []
    found = 0
    for job_id, step in _steps(document):
        for line, in_vm in _commands(step):
            if not BUILD.search(line):
                continue
            found += 1
            where = f"{RELEASE}: job {job_id!r}, step {step.get('name', '<unnamed>')!r}"
            if in_vm:
                if not EMPTY_ASSIGNMENT.match(line):
                    problems.append(
                        f"{where} builds in a VM without an empty RUSTFLAGS: {line}"
                    )
                continue
            env = step.get("env")
            value = env.get("RUSTFLAGS") if isinstance(env, dict) else None
            if value != HOSTED_VALUE:
                problems.append(
                    f"{where} assigns RUSTFLAGS={value!r}, not {HOSTED_VALUE!r}"
                )
    if found != EXPECTED_BUILDS:
        problems.append(
            f"{RELEASE}: expected {EXPECTED_BUILDS} release builds, found {found}"
        )
    return problems


def lint_violations(printed: str) -> list[str]:
    """List every lint command that does not keep warnings denied.

    Parameters
    ----------
    printed : str
        What `make -n lint` printed.

    Returns
    -------
    list[str]
        One message per `cargo clippy` or Whitaker command without `-D warnings`
        on its line, and one when no such command was found.

    """
    commands = [
        line
        for line in printed.replace("\\\n", " ").splitlines()
        if re.search(r"\bcargo clippy\b|\bwhitaker\b", line)
        and not line.lstrip().startswith("echo")
    ]
    problems = [
        f"a lint command does not deny warnings: {line.strip()}"
        for line in commands
        if "-D warnings" not in line
    ]
    if not commands:
        problems.append("`make -n lint` prints no clippy or Whitaker command")
    return problems
