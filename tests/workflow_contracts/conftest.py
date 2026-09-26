"""Shared fixtures for the CV-005 workflow contract tests."""

from __future__ import annotations

import pytest
from codescene_contract_support import Documents, fresh_documents


@pytest.fixture
def documents() -> Documents:
    """Give each CV-005 contract test its own copy of the workflows to mutate."""
    return fresh_documents()
