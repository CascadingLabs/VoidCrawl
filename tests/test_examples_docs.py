"""Static checks for the curated public examples and their documentation."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).parents[1]
EXAMPLES = ROOT / "examples"
INDEX = EXAMPLES / "README.md"
DOCUMENTS = [ROOT / "README.md", INDEX]
LINK_RE = re.compile(r"\[[^]]*]\((?P<target>[^)\s]+)(?:\s+[^)]*)?\)")
FLAT_EXAMPLE_PATH_RE = re.compile(r"examples/[a-z_]+\.py")


def _local_links(document: Path) -> list[Path]:
    links: list[Path] = []
    for match in LINK_RE.finditer(document.read_text()):
        target = match["target"].strip("<>")
        if target.startswith(("#", "http://", "https://", "mailto:")):
            continue
        target = target.split("#", maxsplit=1)[0]
        if target:
            links.append(document.parent / target)
    return links


def test_every_public_example_is_indexed() -> None:
    public_examples = {
        path.relative_to(EXAMPLES)
        for path in EXAMPLES.rglob("*.py")
        if "development" not in path.parts
    }
    indexed_examples = {
        Path(match["target"])
        for match in LINK_RE.finditer(INDEX.read_text())
        if match["target"].endswith(".py")
    }

    assert public_examples
    assert public_examples == indexed_examples


def test_documentation_local_links_resolve() -> None:
    missing = [
        f"{document.relative_to(ROOT)} -> {link}"
        for document in DOCUMENTS
        for link in _local_links(document)
        if not link.exists()
    ]

    assert not missing, "Broken documentation links:\n" + "\n".join(missing)


def test_docs_use_the_public_python_package_name() -> None:
    stale_imports = [
        str(document.relative_to(ROOT))
        for document in DOCUMENTS
        if "from void_crawl import" in document.read_text()
        or "import void_crawl" in document.read_text()
    ]

    assert not stale_imports, "Stale void_crawl imports: " + ", ".join(stale_imports)


def test_api_reference_is_published_from_one_canonical_location() -> None:
    assert not (ROOT / "api-reference.md").exists()
    assert not (ROOT / "docs").exists()


def test_docs_use_current_pool_and_session_constructors() -> None:
    stale_snippets = [
        str(document.relative_to(ROOT))
        for document in DOCUMENTS
        if "BrowserPool.from_env(" in document.read_text()
        or "BrowserSession(headless=" in document.read_text()
    ]

    assert not stale_snippets, "Stale constructor snippets: " + ", ".join(
        stale_snippets
    )


def test_examples_do_not_self_reference_flat_paths() -> None:
    source_files = [ROOT / "README.md", *EXAMPLES.rglob("*.py")]
    stale_paths = [
        str(path.relative_to(ROOT))
        for path in source_files
        if FLAT_EXAMPLE_PATH_RE.search(path.read_text())
    ]

    assert not stale_paths, "Stale example paths: " + ", ".join(stale_paths)


def test_development_examples_are_local_only() -> None:
    assert "/examples/development/" in (ROOT / ".gitignore").read_text()
    pyproject = (ROOT / "pyproject.toml").read_text()
    assert 'extend-exclude = [ "examples/development" ]' in pyproject
