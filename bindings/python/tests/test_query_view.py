import inspect

import pytest

import webspec_index as wsi


def test_query_view_signature_defaults():
    sig = inspect.signature(wsi.query_view)
    assert sig.parameters["links"].default == "short"
    assert sig.parameters["no_notes"].default is False
    assert sig.parameters["involving"].default is None


def test_selector_errors_raise_before_any_lookup():
    with pytest.raises(wsi.WebspecError, match="slice_invalid_selector"):
        wsi.query_view("HTML#navigate", involving="url", steps=["1"])
    with pytest.raises(wsi.WebspecError, match="slice_invalid_selector"):
        wsi.query_view("HTML#navigate", feeding="x.y")
    with pytest.raises(wsi.WebspecError, match="slice_invalid_selector"):
        wsi.query_view("HTML#navigate")


def test_links_mode_is_validated():
    with pytest.raises(ValueError):
        wsi.query_view("HTML#navigate", depth=1, links="long")
