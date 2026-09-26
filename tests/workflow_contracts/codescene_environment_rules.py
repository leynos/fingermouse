"""Hold the CodeScene token's environment to the uploading jobs (CV-005).

The token lives in the `codescene` environment, whose deployment policy admits
`main` alone. So every job that calls the uploader declares that environment,
no other job does, and no workflow a pull request can start declares it in any
job: a declaration there would let branch code ask for the token.
"""

from __future__ import annotations

import typing as typ

from codescene_publisher_rules import upload_steps
from codescene_pull_request_rules import pull_request_closure
from codescene_workflow_reader import Document, holding_job, jobs

ENVIRONMENT: typ.Final[str] = "codescene"
MISSING: typ.Final[str] = f"the uploading job must declare `environment: {ENVIRONMENT}`"
STRAY: typ.Final[str] = f"declares `{ENVIRONMENT}` but uploads nothing"
REACHABLE: typ.Final[str] = (
    f"is reachable from a pull request and declares `{ENVIRONMENT}`"
)


def environment_name(job: dict[str, object]) -> str | None:
    """Return the environment a job declares, from either accepted form.

    Examples
    --------
    >>> environment_name({"environment": "codescene"})
    'codescene'
    >>> environment_name({"environment": {"name": "codescene", "url": "x"}})
    'codescene'
    >>> environment_name({}) is None
    True

    """
    match job.get("environment"):
        case str() as name:
            return name
        case {"name": str() as name}:
            return name
        case _:
            return None


def _placed(
    documents: dict[str, Document], names: typ.Iterable[str]
) -> list[tuple[str, dict[str, object]]]:
    """Return ("workflow:job", job) for every job in the named workflows.

    Returns
    -------
    list of tuple of (str, dict)
        Each job with its location.

    """
    return [
        (f"{name}:{job_id}", job)
        for name in sorted(names)
        for job_id, job in jobs(name, documents[name]).items()
    ]


def environment_violations(documents: dict[str, Document]) -> list[str]:
    """Report every departure from the `codescene` environment placement.

    Returns
    -------
    list of str
        One message per violation; empty when the placement holds.

    """
    uploads = upload_steps(documents)
    if not uploads:
        return ["no workflow job calls the CodeScene uploader"]
    holders = [holding_job(name, documents[name], step) for name, step in uploads]
    held = {id(job) for job in holders}
    problems = [
        f"{name}: {MISSING}"
        for (name, _), job in zip(uploads, holders, strict=True)
        if environment_name(job) != ENVIRONMENT
    ]
    problems.extend(
        f"{where} {STRAY}"
        for where, job in _placed(documents, documents)
        if id(job) not in held and environment_name(job) == ENVIRONMENT
    )
    problems.extend(
        f"{where} {REACHABLE}"
        for where, job in _placed(documents, pull_request_closure(documents))
        if environment_name(job) == ENVIRONMENT
    )
    return problems
