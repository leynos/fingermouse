"""Check the Markdown formatting wiring against the estate baseline.

The real Makefile and workflows must comply, and each clause must reject the
edit it exists to catch while accepting legitimate variants of the same
command, so that the rule is proved narrow as well as sufficient.
"""

from __future__ import annotations

from pathlib import Path

import pytest
from workflow_documents import WORKFLOWS, fresh_documents
from markdown_wiring_rules import (
    INSTALL_ACTION,
    install_precedes_check_fmt,
    lint_action_globs,
    lint_action_steps,
    runs_mdtablefix_check,
)

MAKEFILE = WORKFLOWS.parents[1] / "Makefile"
CHECK = "\t$(MDTABLEFIX) --check $(MDTABLEFIX_SELECT) $(MDTABLEFIX_RULES)"
VARIABLES = (
    "MDTABLEFIX ?= mdtablefix\n"
    "MDTABLEFIX_SELECT = --git --include-untracked\n"
    "MDTABLEFIX_RULES = --wrap --renumber --breaks --ellipsis --fences\n"
)


def _makefile(recipe_line: str, variables: str = VARIABLES) -> str:
    """Return a minimal Makefile whose `check-fmt` runs one line."""
    return f"{variables}\ncheck-fmt: ## Verify formatting\n{recipe_line}\n"


def test_the_repository_makefile_runs_the_check() -> None:
    """`make check-fmt` here runs `mdtablefix --check` with the select flags."""
    assert runs_mdtablefix_check(Path(MAKEFILE).read_text(encoding="utf-8"))


def test_the_repository_workflows_install_and_lint() -> None:
    """CI installs mdtablefix before check-fmt and lints every Markdown file."""
    documents = fresh_documents()
    assert install_precedes_check_fmt(documents) == []
    assert lint_action_globs(documents) == []
    assert lint_action_steps(documents) > 0


@pytest.mark.parametrize(
    "line",
    [
        CHECK.replace(" --check", ""),
        CHECK.replace(" $(MDTABLEFIX_SELECT)", ""),
        "\t-" + CHECK.lstrip("\t"),
        CHECK + " || true",
        CHECK + "; true",
        "\techo " + CHECK.lstrip("\t"),
    ],
    ids=["no_check", "no_select", "ignored", "or_true", "sequenced", "echoed"],
)
def test_a_weakened_check_is_refused(line: str) -> None:
    """Each weakening of the check-fmt command is caught."""
    assert not runs_mdtablefix_check(_makefile(line))


@pytest.mark.parametrize(
    ("line", "variables"),
    [
        (CHECK, VARIABLES),
        (
            "\tmdtablefix --include-untracked --check --git --wrap",
            "",
        ),
        ("\t@$(MDTABLEFIX) --check $(MDTABLEFIX_SELECT)", VARIABLES),
        ("\tcargo fmt --check && mdtablefix --check --git --include-untracked", ""),
        ("\t/usr/local/bin/mdtablefix --check --git --include-untracked", ""),
    ],
    ids=["reference", "reordered_literal", "silenced", "chained", "qualified"],
)
def test_a_legitimate_check_is_accepted(line: str, variables: str) -> None:
    """Reordered flags, a literal command or an `&&` chain still comply."""
    assert runs_mdtablefix_check(_makefile(line, variables))


def _all_steps(documents: dict[str, object]) -> list[list[dict[str, object]]]:
    """Return every job's step list, for the mutation cases to edit in place."""
    return [
        job.get("steps", [])
        for document in documents.values()
        for job in document.get("jobs", {}).values()
    ]


def test_an_install_after_check_fmt_is_refused() -> None:
    """Moving the install step below check-fmt is caught."""
    documents = fresh_documents()
    for steps in _all_steps(documents):
        installs = [s for s in steps if INSTALL_ACTION in str(s.get("uses", ""))]
        steps[:] = [s for s in steps if s not in installs] + installs
    assert install_precedes_check_fmt(documents) != []


def test_narrowed_lint_globs_are_refused() -> None:
    """A markdownlint-cli2-action step linting less than `**/*.md` is caught."""
    documents = fresh_documents()
    lint_steps = [
        step
        for steps in _all_steps(documents)
        for step in steps
        if "markdownlint-cli2-action" in str(step.get("uses", ""))
    ]
    for step in lint_steps:
        step["with"] = {"globs": "docs/**/*.md"}
    assert lint_action_globs(documents) != []
