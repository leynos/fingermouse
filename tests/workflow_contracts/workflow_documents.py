"""The workflows a contract reads, one private copy per test."""

from __future__ import annotations

import typing as typ
from pathlib import Path

from workflow_reader import Document, read_workflows

type Documents = dict[str, Document]

WORKFLOWS: typ.Final[Path] = (
    Path(__file__).resolve().parents[2] / ".github" / "workflows"
)


def fresh_documents(directory: Path = WORKFLOWS) -> Documents:
    """Read a private copy of the workflows for one test to mutate.

    Read afresh each time rather than cached for the process, so no test can
    see another's mutation and a read failure surfaces in the test that met
    it, as the reader's `WorkflowError`.
    """
    return read_workflows(directory)
