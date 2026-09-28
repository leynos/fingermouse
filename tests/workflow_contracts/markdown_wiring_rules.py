"""Hold the Markdown formatting wiring to the estate baseline.

`make check-fmt` must run `mdtablefix --check` over the Git-selected Markdown
set with its exit status reaching Make; the CI job that runs `make check-fmt`
must install mdtablefix in an earlier step; and every markdownlint-cli2-action
step must lint `**/*.md`. The rules read the commands, not target or step
names, so renaming a step or a variable cannot satisfy them by accident.
"""

from __future__ import annotations

import re
import shlex
import typing as typ

from codescene_workflow_reader import Document, calls, jobs

if typ.TYPE_CHECKING:
    import collections.abc as cabc

type Steps = list[dict[str, object]]

INSTALL_ACTION: typ.Final[str] = (
    "leynos/shared-actions/.github/actions/install-mdtablefix"
)
LINT_ACTION: typ.Final[str] = "DavidAnson/markdownlint-cli2-action"
SELECT_FLAGS: typ.Final[frozenset[str]] = frozenset(
    {"--check", "--git", "--include-untracked"}
)
_ASSIGNMENT = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\s*(?::=|\?=|=)\s*(.*)$")
_REFERENCE = re.compile(r"\$\(([A-Za-z_][A-Za-z0-9_]*)\)")
_CONDITIONAL_OPEN = re.compile(r"^(ifeq|ifneq|ifdef|ifndef)\b")
_RUNS_CHECK_FMT = re.compile(r"(^|\s)make\s+(\S+\s+)*check-fmt(\s|$)")
_MASKS_STATUS = re.compile(r"\|\||;|\|")


def _unconditional_assignments(makefile: str) -> cabc.Iterator[tuple[str, str]]:
    """Yield each `NAME = value` assignment outside a conditional block."""
    depth = 0
    for line in makefile.splitlines():
        if _CONDITIONAL_OPEN.match(line):
            depth += 1
        elif line.startswith("endif"):
            depth -= 1
        elif depth == 0 and (match := _ASSIGNMENT.match(line)):
            yield match.group(1), match.group(2).strip()


def variables(makefile: str) -> dict[str, str]:
    """Return each variable with exactly one unconditional assignment.

    A variable assigned twice, or inside a conditional, is left out, so a
    reference to it stays unexpanded and fails the rule rather than being
    guessed.
    """
    seen: dict[str, list[str]] = {}
    for name, value in _unconditional_assignments(makefile):
        seen.setdefault(name, []).append(value)
    return {name: values[0] for name, values in seen.items() if len(values) == 1}


def expand(text: str, known: dict[str, str]) -> str:
    """Expand `$(NAME)` references, three passes deep."""
    for _ in range(3):
        text = _REFERENCE.sub(lambda m: known.get(m.group(1), m.group(0)), text)
    return text


def _recipe_start(lines: list[str], target: str) -> int | None:
    """Return the index of the target's rule line when it is defined once."""
    pattern = re.compile(rf"^{re.escape(target)}\s*:(?!=)")
    starts = [index for index, line in enumerate(lines) if pattern.match(line)]
    return starts[0] if len(starts) == 1 else None


def recipe(makefile: str, target: str) -> list[str]:
    """Return a target's recipe lines, with continuations joined."""
    lines = makefile.splitlines()
    start = _recipe_start(lines, target)
    if start is None:
        return []
    joined: list[str] = []
    for line in lines[start + 1 :]:
        if not line.startswith("\t"):
            break
        if joined and joined[-1].endswith("\\"):
            joined[-1] = joined[-1][:-1] + " " + line.strip()
        else:
            joined.append(line[1:])
    return joined


def _binds_status(command: str) -> bool:
    """Return whether a recipe line's exit status reaches Make.

    A `-` prefix ignores the status, and a later `;`, `|` or `||` hands the
    line's status to another command.
    """
    stripped = command.lstrip("@+ ")
    return not stripped.startswith("-") and not _MASKS_STATUS.search(stripped)


def _is_check_invocation(segment: str) -> bool:
    """Return whether one `&&` segment runs `mdtablefix` with the flags."""
    words = shlex.split(segment.strip().lstrip("@+"), posix=True)
    if not words or words[0].rsplit("/", 1)[-1] != "mdtablefix":
        return False
    return SELECT_FLAGS <= set(words[1:])


def runs_mdtablefix_check(makefile: str) -> bool:
    """Return whether `check-fmt` runs `mdtablefix` with the select flags."""
    known = variables(makefile)
    expanded = (expand(line, known) for line in recipe(makefile, "check-fmt"))
    return any(
        _binds_status(line) and any(map(_is_check_invocation, line.split("&&")))
        for line in expanded
    )


def _job_steps(documents: dict[str, Document]) -> cabc.Iterator[tuple[str, Steps]]:
    """Yield each job as `workflow:job` with its steps."""
    for name, document in documents.items():
        for job_id, job in jobs(name, document).items():
            yield f"{name}:{job_id}", typ.cast("Steps", job.get("steps", []))


def _checks_before_install(steps: Steps) -> bool:
    """Return whether a step runs `make check-fmt` before any install step."""
    for step in steps:
        if calls(step, INSTALL_ACTION):
            return False
        if _RUNS_CHECK_FMT.search(str(step.get("run", ""))):
            return True
    return False


def install_precedes_check_fmt(documents: dict[str, Document]) -> list[str]:
    """Return each job that runs `make check-fmt` without an earlier install."""
    return [
        label for label, steps in _job_steps(documents) if _checks_before_install(steps)
    ]


def _lint_steps(documents: dict[str, Document]) -> cabc.Iterator[tuple[str, dict]]:
    """Yield each markdownlint-cli2-action step with its job label."""
    for label, steps in _job_steps(documents):
        yield from ((label, step) for step in steps if calls(step, LINT_ACTION))


def lint_action_globs(documents: dict[str, Document]) -> list[str]:
    """Return each markdownlint-cli2-action step not linting `**/*.md`.

    An empty list with no action step at all is not compliance, so the caller
    also asserts that at least one step exists.
    """
    return [
        label
        for label, step in _lint_steps(documents)
        if typ.cast("dict[str, object]", step.get("with") or {}).get("globs")
        != "**/*.md"
    ]


def lint_action_steps(documents: dict[str, Document]) -> int:
    """Return how many steps call the markdownlint-cli2-action."""
    return sum(1 for _ in _lint_steps(documents))
