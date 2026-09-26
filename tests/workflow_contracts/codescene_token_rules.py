"""Hold where the publisher may name CS_ACCESS_TOKEN (CV-005).

The uploader is a composite action. A token bound in its step's `env` would
reach its nested `upload-artifact` and cache steps too, and the action binds the
token itself from `inputs.access-token`. So no `env` anywhere in the publisher
carries the token. A check step reports whether the secret is set with one
exact command whose expression Actions evaluates before the shell runs, and the
upload receives the token only as its `access-token` input.

A guard on `env.CS_ACCESS_TOKEN != ''` is simply false when the binding is
deleted or moved, so the upload would skip forever with nothing failing. The
check step and its command are therefore asserted positively, not inferred
from the guard.
"""

from __future__ import annotations

import copy
import typing as typ

from codescene_workflow_reader import (
    Document,
    Step,
    continues_on_error,
    folded,
    holding_job,
    jobs,
    scalars,
)

#: The check step's id, which the upload guard reads.
CHECK_ID: typ.Final[str] = "codescene-token"

#: The check step's whole command. Actions evaluates the expression to `true`
#: or `false` before the shell runs, so there is no shell conditional to
#: neutralize and the token is bound in no step's `env`. A fork without the
#: secret writes `available=false` and skips the upload rather than failing.
CHECK_COMMAND: typ.Final[str] = (
    'echo "available=${{ secrets.CS_ACCESS_TOKEN != \'\' }}" >> "$GITHUB_OUTPUT"'
)

#: What the upload passes as `access-token`.
CREDENTIAL_INPUT: typ.Final[str] = "${{ secrets.CS_ACCESS_TOKEN }}"

#: Keys a check step may carry. Anything else, such as `if`, `uses`, `shell`,
#: `env` or `continue-on-error`, could skip it, run other code, bind the token
#: or turn its failure green.
CHECK_KEYS: typ.Final[frozenset[str]] = frozenset({"name", "id", "run"})


def expression(value: object) -> str:
    """Return an expression with its inner whitespace normalized.

    Parameters
    ----------
    value : object
        A scalar from a parsed workflow.

    Returns
    -------
    str
        The text with `${{` and `}}` spaced and runs of whitespace collapsed.

    Examples
    --------
    >>> expression("${{secrets.X}}")
    '${{ secrets.X }}'

    """
    return " ".join(str(value).replace("${{", "${{ ").replace("}}", " }}").split())


def _check_step(name: str, check: Step, upload_index: int, index: int) -> list[str]:
    """Report a check step that could skip, fail green or run other code."""
    return [
        f"{name} `{CHECK_ID}` step {problem}"
        for problem, failed in (
            ("must run before the upload", index > upload_index),
            (f"may carry only {sorted(CHECK_KEYS)}", not check.keys() <= CHECK_KEYS),
            ("must not continue on error", continues_on_error(check)),
            (
                f"must run exactly `{CHECK_COMMAND}`",
                expression(check.get("run", "")) != CHECK_COMMAND,
            ),
        )
        if failed
    ]


def _outside_steps(name: str, document: Document, kept: list[Step]) -> list[str]:
    """Report the token anywhere in the publisher but the named steps."""
    rest = copy.deepcopy(document)
    for job in jobs(name, rest).values():
        job["steps"] = [
            step
            for step in typ.cast("list[Step]", job.get("steps", []))
            if step not in kept
        ]
    if any("cs_access_token" in folded(text) for text in scalars(rest)):
        return [f"{name} puts CS_ACCESS_TOKEN in reach outside its two steps"]
    return []


def _upload_env(name: str, upload: Step) -> list[str]:
    """Report a token in the upload step's `env`, which nested steps inherit."""
    if any("cs_access_token" in folded(text) for text in scalars(upload.get("env"))):
        return [f"{name} upload step must not bind CS_ACCESS_TOKEN in its env"]
    return []


def token_violations(name: str, document: Document, upload: Step) -> list[str]:
    """Report a token placed anywhere but the check command and the upload input.

    Parameters
    ----------
    name : str
        The publisher's file name, for messages.
    document : Document
        The parsed publisher.
    upload : Step
        The publisher's upload step.

    Returns
    -------
    list of str
        One message per violation; empty when the token is placed as required.

    """
    held = typ.cast("list[Step]", holding_job(name, document, upload).get("steps", []))
    checks = [index for index, step in enumerate(held) if step.get("id") == CHECK_ID]
    if len(checks) != 1:
        return [f"{name} needs one `{CHECK_ID}` step in the upload job"]
    check = held[checks[0]]
    found = _check_step(name, check, held.index(upload), checks[0])
    inputs = upload.get("with")
    inputs = inputs if isinstance(inputs, dict) else {}
    if expression(inputs.get("access-token")) != CREDENTIAL_INPUT:
        found.append(f"{name} upload must pass access-token {CREDENTIAL_INPUT}")
    return [
        *found,
        *_upload_env(name, upload),
        *_outside_steps(name, document, [check, upload]),
    ]
