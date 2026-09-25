"""Prove the pull-request lane half of CV-005: it ratchets like main.

Each test mutates a copy of the workflows in the way a later edit could and
asserts that the clause meant to catch that edit does: the lane's coverage
step ratchets against main's baseline at the publisher's pin and selection,
and its contract step runs unconditionally.
"""

from __future__ import annotations

import typing as typ

import pytest
from codescene_contract_support import (
    EXPECTED_SELECTION,
    LANE,
    Documents,
    assert_reports,
    coverage_step,
    find_publisher,
    first_job,
    job_steps,
    replace_triggers,
)
from codescene_coverage_rules import contract_invocations, coverage_violations


@pytest.mark.parametrize(
    ("key", "value", "expected"),
    [
        ("with-ratchet", "false", "with-ratchet 'true'"),
        ("publish-artefact", "true", "publish-artefact 'false'"),
        ("python-source", "./tests", "selection differs"),
    ],
)
def test_pull_request_coverage_ratchets_like_main(
    documents: Documents, key: str, value: str, expected: str
) -> None:
    """The PR lane ratchets against main's baseline and publishes nothing."""
    typ.cast("dict[str, object]", coverage_step(documents[LANE])["with"])[key] = value
    assert_reports(coverage_violations, documents, expected)


@pytest.mark.parametrize("guard", ["false", "github.event_name == 'push'", None])
def test_pull_request_coverage_cannot_be_switched_off(
    documents: Documents, guard: object
) -> None:
    """Any condition keeps the step while the ratchet may never run."""
    coverage_step(documents[LANE])["if"] = guard
    assert_reports(coverage_violations, documents, "must run unconditionally")


def test_pull_request_coverage_job_runs_unconditionally(documents: Documents) -> None:
    """A job-level `if: false` skips the ratchet with the step intact."""
    first_job(documents[LANE])["if"] = "false"
    assert_reports(coverage_violations, documents, "must run unconditionally")


def test_pull_request_coverage_pin_matches_the_publisher(documents: Documents) -> None:
    """A lane at another pin measures with different code from the baseline's."""
    step = coverage_step(documents[LANE])
    step["uses"] = str(step["uses"]).partition("@")[0] + "@" + "0" * 40
    assert_reports(coverage_violations, documents, "pin differs from the publisher's")


def test_repository_selection_is_pinned(documents: Documents) -> None:
    """Both lanes changing their selection together would pass parity alone."""
    publisher, _ = find_publisher(documents)
    found = coverage_step(publisher).get("with")
    assert found == EXPECTED_SELECTION, f"coverage selection is {found}"


def test_pull_request_lane_cannot_answer_a_push(documents: Documents) -> None:
    """On main's push the lane's coverage would write a second baseline."""
    lane_triggers = {"pull_request": None, "push": {"branches": ["main"]}}
    replace_triggers(documents[LANE], lane_triggers)
    assert_reports(coverage_violations, documents, "can run on a push outside")


@pytest.mark.parametrize(
    "change",
    [
        {"run": "echo make workflow-contracts"},
        {"run": "false && make workflow-contracts"},
        {"if": "false"},
        {"continue-on-error": True},
        {"shell": "true {0}"},
    ],
)
def test_pull_request_lane_runs_the_contract(
    documents: Documents, change: dict[str, object]
) -> None:
    """A contract CI never runs protects nothing, whatever it asserts."""
    step = next(
        s
        for s in job_steps(documents[LANE])
        if s.get("run") == "make workflow-contracts"
    )
    step.update(change)
    assert_reports(contract_invocations, documents, "workflow-contracts")


@pytest.mark.parametrize(
    ("scope", "change"),
    [
        ("job", {"if": "false"}),
        ("job", {"continue-on-error": True}),
        ("job", {"defaults": {"run": {"shell": "true {0}"}}}),
        ("workflow", {"defaults": {"run": {"shell": "true {0}"}}}),
    ],
)
def test_contract_step_scopes_cannot_skip_it(
    documents: Documents, scope: str, change: dict[str, object]
) -> None:
    """A job or workflow can skip the contract, or swap its shell, step intact."""
    target = first_job(documents[LANE]) if scope == "job" else documents[LANE]
    target.update(change)
    assert_reports(contract_invocations, documents, "workflow-contracts")


def test_pull_request_coverage_must_exist(documents: Documents) -> None:
    """Deleting the PR coverage step deletes the ratchet."""
    steps = job_steps(documents[LANE])
    steps.remove(coverage_step(documents[LANE]))
    assert_reports(
        coverage_violations,
        documents,
        "no pull-request lane generates coverage for the ratchet",
    )
