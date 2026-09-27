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

INSTALL_ACTION: typ.Final[str] = (
    "leynos/shared-actions/.github/actions/install-mdtablefix"
)
LINT_ACTION: typ.Final[str] = "DavidAnson/markdownlint-cli2-action"
SELECT_FLAGS: typ.Final[frozenset[str]] = frozenset(
    {"--check", "--git", "--include-untracked"}
)
_ASSIGNMENT = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\s*(?::=|\?=|=)\s*(.*)$")
_REFERENCE = re.compile(r"\$\(([A-Za-z_][A-Za-z0-9_]*)\)")


def variables(makefile: str) -> dict[str, str]:
    """Return each variable with exactly one unconditional assignment.

    A variable assigned twice, or inside a conditional, is left out, so a
    reference to it stays unexpanded and fails the rule rather than being
    guessed.
    """
    seen: dict[str, list[str]] = {}
    depth = 0
    for line in makefile.splitlines():
        if re.match(r"^(ifeq|ifneq|ifdef|ifndef)\b", line):
            depth += 1
        elif line.startswith("endif"):
            depth -= 1
        elif depth == 0 and (match := _ASSIGNMENT.match(line)):
            seen.setdefault(match.group(1), []).append(match.group(2).strip())
    return {name: values[0] for name, values in seen.items() if len(values) == 1}


def expand(text: str, known: dict[str, str]) -> str:
    """Expand `$(NAME)` references, three passes deep."""
    for _ in range(3):
        text = _REFERENCE.sub(lambda m: known.get(m.group(1), m.group(0)), text)
    return text


def recipe(makefile: str, target: str) -> list[str]:
    """Return a target's recipe lines, with continuations joined."""
    lines = makefile.splitlines()
    starts = [
        index
        for index, line in enumerate(lines)
        if re.match(rf"^{re.escape(target)}\s*:(?!=)", line)
    ]
    if len(starts) != 1:
        return []
    joined: list[str] = []
    for line in lines[starts[0] + 1 :]:
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
    return not stripped.startswith("-") and not re.search(r"\|\||;|\|", stripped)


def runs_mdtablefix_check(makefile: str) -> bool:
    """Return whether `check-fmt` runs `mdtablefix` with the select flags."""
    known = variables(makefile)
    for line in recipe(makefile, "check-fmt"):
        expanded = expand(line, known)
        for segment in expanded.split("&&"):
            words = shlex.split(segment.strip().lstrip("@+"), posix=True)
            if (
                words
                and words[0].rsplit("/", 1)[-1] == "mdtablefix"
                and SELECT_FLAGS <= set(words[1:])
                and _binds_status(expanded)
            ):
                return True
    return False


def install_precedes_check_fmt(documents: dict[str, Document]) -> list[str]:
    """Return each job that runs `make check-fmt` without an earlier install."""
    missing = []
    for name, document in documents.items():
        for job_id, job in jobs(name, document).items():
            installed = False
            for step in typ.cast("list[dict[str, object]]", job.get("steps", [])):
                if calls(step, INSTALL_ACTION):
                    installed = True
                run = str(step.get("run", ""))
                if (
                    re.search(r"(^|\s)make\s+(\S+\s+)*check-fmt(\s|$)", run)
                    and not installed
                ):
                    missing.append(f"{name}:{job_id}")
    return missing


def lint_action_globs(documents: dict[str, Document]) -> list[str]:
    """Return each markdownlint-cli2-action step not linting `**/*.md`.

    An empty list with no action step at all is not compliance, so the caller
    also asserts that at least one step exists.
    """
    wrong = []
    for name, document in documents.items():
        for job_id, job in jobs(name, document).items():
            for step in typ.cast("list[dict[str, object]]", job.get("steps", [])):
                inputs = typ.cast("dict[str, object]", step.get("with") or {})
                if calls(step, LINT_ACTION) and inputs.get("globs") != "**/*.md":
                    wrong.append(f"{name}:{job_id}")
    return wrong


def lint_action_steps(documents: dict[str, Document]) -> int:
    """Return how many steps call the markdownlint-cli2-action."""
    return sum(
        calls(step, LINT_ACTION)
        for name, document in documents.items()
        for job in jobs(name, document).values()
        for step in typ.cast("list[dict[str, object]]", job.get("steps", []))
    )
