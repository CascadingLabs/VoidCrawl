"""Regression tests for generated API source links."""

# ruff: noqa: I001

from __future__ import annotations

import re
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts import generate_api_docs


REPO_URL = "https://github.com/CascadingLabs/VoidCrawl"


def _current_ref() -> str:
    return generate_api_docs._current_git_ref()


def test_validate_source_links_rejects_existing_file_with_bad_line() -> None:
    ref = _current_ref()
    content = (
        f'## `GetAttribute` <a href="{REPO_URL}/blob/{ref}/'
        'voidcrawl/actions/builtin/dom.py#L1">bad</a>'
    )

    with pytest.raises(SystemExit, match="Source link line validation failed"):
        generate_api_docs._validate_source_links(content, REPO_URL, ref)


def test_validate_source_links_rejects_wrong_symbol_on_declaration_line() -> None:
    ref = _current_ref()
    content = (
        f'## `WrongName` <a href="{REPO_URL}/blob/{ref}/'
        'voidcrawl/actions/builtin/dom.py#L21">bad</a>'
    )

    with pytest.raises(SystemExit, match="Source link line validation failed"):
        generate_api_docs._validate_source_links(content, REPO_URL, ref)


def test_generated_source_links_target_declaration_lines() -> None:
    ref = _current_ref()
    content = generate_api_docs.generate("test", set(), REPO_URL, ref)

    generate_api_docs._validate_source_links(content, REPO_URL, ref)
    assert re.search(r"/voidcrawl/actions/builtin/dom\.py#L21\"", content)
    assert re.search(r"/voidcrawl/actions/builtin/dom\.py#L63\"", content)
