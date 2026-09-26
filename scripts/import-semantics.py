#!/usr/bin/env python3
"""Import a maintained webspec-semantics checkout as a runtime artifact.

Only effects/*.yaml and state/*.yaml are copied. Acceptance HTML/JSON is intentionally
left out of data/semantics so installed consumers do not ship the corpus.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile

import yaml


class StrictLoader(yaml.SafeLoader):
    """Safe YAML loader that rejects duplicate keys and custom tags."""


def _mapping(loader: StrictLoader, node: yaml.MappingNode, deep: bool = False):
    result = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in result:
            raise ValueError(f"duplicate YAML key {key!r}")
        result[key] = loader.construct_object(value_node, deep=deep)
    return result


StrictLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _mapping
)


def _reject_tag(loader: StrictLoader, node: yaml.Node):
    raise ValueError(f"custom YAML tag {node.tag!r} is not supported")


StrictLoader.add_constructor(None, _reject_tag)


def runtime_files(root: Path) -> list[tuple[Path, str, bytes]]:
    effects = root / "effects"
    if not effects.is_dir():
        raise ValueError(f"catalog has no effects directory: {effects}")
    effects_paths = sorted(
        path for path in effects.rglob("*") if path.is_file() and path.suffix in {".yaml", ".yml"}
    )
    if not effects_paths:
        raise ValueError(f"catalog has no runtime YAML files: {effects}")
    state = root / "state"
    state_paths = sorted(
        path for path in state.rglob("*") if path.is_file() and path.suffix in {".yaml", ".yml"}
    ) if state.is_dir() else []
    files = effects_paths + state_paths
    package = None
    entries = []
    for path in files:
        try:
            doc = yaml.load(path.read_text(encoding="utf-8"), Loader=StrictLoader)
        except Exception as exc:  # add the path to PyYAML's otherwise terse errors
            raise ValueError(f"invalid YAML in {path}: {exc}") from exc
        if not isinstance(doc, dict) or doc.get("schema") != 1:
            raise ValueError(f"{path}: runtime YAML must have schema: 1")
        current = doc.get("package")
        if not isinstance(current, str):
            raise ValueError(f"{path}: package is required")
        if package is None:
            package = current
        elif package != current:
            raise ValueError(f"{path}: package {current!r} differs from {package!r}")
        rel = path.relative_to(root).as_posix()
        entries.append((path, rel, path.read_bytes()))
    return entries


def content_digest(entries: list[tuple[Path, str, bytes]]) -> str:
    digest = hashlib.sha256()
    for _path, rel, content in entries:
        digest.update(rel.encode("utf-8"))
        digest.update(b"\0")
        digest.update(content)
        digest.update(b"\0")
    return digest.hexdigest()


def import_catalog(root: Path, destination: Path, source_repository: str, source_revision: str) -> str:
    entries = runtime_files(root)
    package = yaml.safe_load((entries[0][0]).read_text(encoding="utf-8"))["package"]
    digest = content_digest(entries)

    destination.mkdir(parents=True, exist_ok=True)
    # Stage the complete result in a sibling temporary directory. A failed copy
    # therefore cannot leave an apparently complete but mixed catalog behind.
    with tempfile.TemporaryDirectory(prefix="semantics-import-", dir=destination.parent) as staging:
        staged = Path(staging)
        for _path, rel, content in entries:
            output = staged / rel
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(content)
        lock = {
            "schema": 1,
            "package": package,
            "source_repository": source_repository,
            "source_revision": source_revision,
            "content_sha256": digest,
        }
        (staged / "catalog-lock.json").write_text(
            json.dumps(lock, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        (staged / "embedded.json").write_text(
            json.dumps([[rel, content.decode("utf-8")] for _, rel, content in entries],
                       ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8"
        )
        # Remove stale generated YAML first (for example after deleting a rule
        # file), then replace only generated runtime paths. Acceptance data is
        # never copied or removed.
        for old in destination.rglob("*"):
            if old.is_file() and old.suffix in {".yaml", ".yml"}:
                old.unlink()
        for source in sorted(staged.rglob("*")):
            if source.is_file():
                target = destination / source.relative_to(staged)
                target.parent.mkdir(parents=True, exist_ok=True)
                os.replace(source, target)
    return digest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path, help="maintained webspec-semantics checkout")
    parser.add_argument("--destination", type=Path, default=Path("data/semantics"))
    parser.add_argument("--source-repository", required=True)
    parser.add_argument("--source-revision", required=True)
    args = parser.parse_args()
    digest = import_catalog(
        args.package.resolve(),
        args.destination.resolve(),
        args.source_repository,
        args.source_revision,
    )
    print(f"imported runtime catalog {digest} into {args.destination}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
