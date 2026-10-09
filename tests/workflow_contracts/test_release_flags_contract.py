"""Prove the release and lint clauses hold, and that each part of them bites.

The real workflow and Makefile must comply, and each clause must reject the
edit it exists to catch. The workflow mutations edit a private copy of the
parsed documents; the Makefile cases feed canned `make -n` text to the reader.
"""

from __future__ import annotations

import copy
import subprocess
from pathlib import Path

import pytest
from release_flags_rules import (
    EXPECTED_BUILDS,
    RELEASE,
    lint_violations,
    release_build_violations,
)
from workflow_documents import WORKFLOWS, fresh_documents

ROOT = WORKFLOWS.parents[1]


def _build_steps(document: dict[object, object]) -> list[dict[str, object]]:
    """Return the steps whose command builds a release."""
    steps = [
        step
        for job in document["jobs"].values()
        for step in job["steps"]
        if "build --release" in str(step.get("run", ""))
        or "build --release" in str((step.get("with") or {}).get("run", ""))
    ]
    assert len(steps) == EXPECTED_BUILDS
    return steps


def _hosted(document: dict[object, object]) -> list[dict[str, object]]:
    """Return the release builds that run on a hosted runner."""
    return [step for step in _build_steps(document) if "run" in step]


def _vm(document: dict[object, object]) -> list[dict[str, object]]:
    """Return the release builds that run inside a VM."""
    return [step for step in _build_steps(document) if "run" not in step]


def test_the_repository_release_workflow_holds_the_clause() -> None:
    """The shipped release builds each assign their own value."""
    assert release_build_violations(fresh_documents()) == []


def test_each_hosted_build_that_loses_its_assignment_is_refused() -> None:
    """Dropping the `env:` block lets the configuration's nightly flags through."""
    for index in range(len(_hosted(fresh_documents()[RELEASE]))):
        documents = fresh_documents()
        del _hosted(documents[RELEASE])[index]["env"]

        assert any("RUSTFLAGS=None" in p for p in release_build_violations(documents))


@pytest.mark.parametrize(
    "value", ["", "-Zthreads=8", "-D warnings -Clink-arg=-fuse-ld=mold"]
)
def test_a_hosted_build_with_another_value_is_refused(value: str) -> None:
    """Only the warning deny is the right value: nothing more, nothing less."""
    documents = fresh_documents()
    _hosted(documents[RELEASE])[0]["env"]["RUSTFLAGS"] = value

    assert any("not '-D warnings'" in p for p in release_build_violations(documents))


def test_each_vm_build_that_loses_its_empty_assignment_is_refused() -> None:
    """A BSD VM's packaged Rust is stable, so the nightly flags must not reach it."""
    for index in range(len(_vm(fresh_documents()[RELEASE]))):
        documents = fresh_documents()
        step = _vm(documents[RELEASE])[index]
        step["with"]["run"] = step["with"]["run"].replace("RUSTFLAGS='' ", "")

        assert any(
            "without an empty RUSTFLAGS" in p
            for p in release_build_violations(documents)
        )


def test_a_sibling_steps_environment_does_not_count() -> None:
    """The assignment is judged on the build step itself."""
    documents = fresh_documents()
    step = _hosted(documents[RELEASE])[0]
    lent = copy.deepcopy(step["env"])
    del step["env"]
    step_list = next(
        job["steps"]
        for job in documents[RELEASE]["jobs"].values()
        if step in job["steps"]
    )
    step_list.insert(step_list.index(step), {"name": "Neighbour", "env": lent})

    assert any("RUSTFLAGS=None" in p for p in release_build_violations(documents))


def test_a_removed_build_is_a_count_mismatch() -> None:
    """A workflow that stops building a release cannot read as compliant."""
    documents = fresh_documents()
    for job in documents[RELEASE]["jobs"].values():
        job["steps"] = [
            s for s in job["steps"] if "build --release" not in str(s.get("run", ""))
        ]

    assert any(
        "release builds, found" in p for p in release_build_violations(documents)
    )


def test_a_missing_release_workflow_is_refused() -> None:
    """Reading no workflow must not read as success."""
    assert release_build_violations({}) != []


CLIPPY = (
    'RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Zthreads=8" cargo clippy --all-targets '
    "--all-features -- -D warnings\n"
)
WHITAKER = (
    'RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-D warnings -Zthreads=8" whitaker --all -- '
    "--all-targets --all-features\n"
)


def test_the_repository_lint_recipe_denies_warnings(tmp_path: Path) -> None:
    """`make -n lint` prints commands that each keep `-D warnings`."""
    printed = subprocess.run(  # noqa: S603 - a fixed command
        ["make", "-n", "-B", "lint"],  # noqa: S607
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout

    assert lint_violations(printed) == []


def test_a_compliant_lint_recipe_is_accepted() -> None:
    """Both commands as written pass."""
    assert lint_violations(CLIPPY + WHITAKER) == []


@pytest.mark.parametrize(
    "printed",
    [
        CLIPPY.replace(" -- -D warnings", ""),
        WHITAKER.replace("-D warnings ", ""),
        CLIPPY.replace("-D warnings", "-A warnings") + WHITAKER,
        "echo nothing to lint\n",
    ],
    ids=[
        "clippy loses it",
        "whitaker loses it",
        "clippy allows warnings",
        "no command",
    ],
)
def test_a_lint_recipe_that_stops_denying_warnings_is_refused(printed: str) -> None:
    """Each way of losing the deny, or of reading nothing, draws a complaint."""
    assert lint_violations(printed)
