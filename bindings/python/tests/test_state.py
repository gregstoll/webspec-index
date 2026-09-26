"""Offline tests for the state and state_coverage bindings."""

import os
import tempfile

import pytest

import webspec_index as wsi


def test_state_without_any_state_model_raises(monkeypatch):
    with tempfile.TemporaryDirectory() as tmp:
        monkeypatch.setenv("SPEC_INDEX_TEST_DB", os.path.join(tmp, "index.db"))
        with pytest.raises(wsi.WebspecError, match="state model"):
            wsi.state("Document")


def test_state_signature_defaults():
    import inspect
    sig = inspect.signature(wsi.state)
    assert sig.parameters["include_inits"].default is True
    assert sig.parameters["limit"].default is None
