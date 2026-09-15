"""Offline contract tests for the native possible-effects adapter."""

import json
import sqlite3

import pytest

import webspec_index as wsi


@pytest.fixture()
def effects_db(tmp_path, monkeypatch):
    database = tmp_path / "effects.db"
    monkeypatch.setenv("SPEC_INDEX_TEST_DB", str(database))

    # Opening any native API initializes the real schema and seeded registry.
    wsi.specs()
    with sqlite3.connect(database) as connection:
        spec_id = connection.execute(
            "SELECT id FROM specs WHERE name = 'WEBIDL'"
        ).fetchone()[0]
        snapshot_id = connection.execute(
            """INSERT INTO snapshots
               (spec_id, sha, commit_date, indexed_at, is_latest, index_version)
               VALUES (?, 'hash:python-effects', '2026-09-13', '2026-09-13', 1, ?)
               RETURNING id""",
            (spec_id, wsi.__version__),
        ).fetchone()[0]
        connection.execute(
            """INSERT INTO sections
               (snapshot_id, anchor, title, content_text, section_type, depth)
               VALUES (?, 'wait-for-all', 'Wait for all',
                       'The algorithm uses PerformPromiseThen.', 'algorithm', 1)""",
            (snapshot_id,),
        )
        connection.commit()
    return database


def request():
    return {
        "schema_version": 1,
        "subject": {"spec": "WEBIDL", "anchor": "wait-for-all"},
    }


def test_summary_and_explanation_round_trip_through_native_module(effects_db):
    summary = wsi.get_effect_summary(request())
    assert summary["schema_version"] == 1
    assert summary["subject"]["snapshot_sha"] == "hash:python-effects"
    assert summary["effects_status"]["state"] == "ready"
    assert summary["effects_status"]["semantics"] == "may"
    assert [effect["kind"] for effect in summary["effects"]] == [
        "scheduling.promise-continuation"
    ]
    location = summary["effects"][0]["location"]
    assert location["spec"] == "WEBIDL"
    assert location["anchor"] == "wait-for-all"
    assert "step_path" not in location  # Declaration has no parsed numbered step.
    assert json.loads(json.dumps(summary)) == summary

    explanation = wsi.explain_effects(
        {**request(), "explanation": {"limit": 1, "max_depth": 8, "max_states": 100}}
    )
    assert explanation["subject"] == summary["subject"]
    assert explanation["effects"] == summary["effects"]
    assert explanation["explanations"][0]["effect_id"] == summary["effects"][0]["id"]
    assert explanation["explanations"][0]["witnesses"]
    assert json.loads(json.dumps(explanation)) == explanation


def test_recompute_returns_versioned_json_compatible_result(effects_db):
    result = wsi.recompute_effects(
        {
            "schema_version": 1,
            "scope": {"kind": "subject", "subject": request()["subject"]},
        }
    )
    assert result["schema_version"] == 1
    assert result["analysis_id"].startswith("an_")
    assert result["processed_subjects"]
    assert json.loads(json.dumps(result)) == result


def test_native_request_validation_distinguishes_wire_and_contract_errors(effects_db):
    with pytest.raises(ValueError, match="schema_version"):
        wsi.get_effect_summary({"subject": request()["subject"]})

    invalid = request()
    invalid["subject"] = {
        **invalid["subject"],
        "step_path": [1],
        "body_id": "body-also-selected",
    }
    with pytest.raises(wsi.WebspecError, match="at most one"):
        wsi.get_effect_summary(invalid)
