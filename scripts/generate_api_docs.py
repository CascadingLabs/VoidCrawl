"""Generate API reference markdown from the voidcrawl public API.

Uses griffe for static analysis -- no need to import the package or build
the native extension.  Parses Google-style docstrings into structured
sections.  Only symbols listed in ``__all__`` are included (unless passed
via ``--exclude``).

Each symbol heading includes a linked GitHub source icon pointing to the
exact line in the repository.

Usage:
    # Single combined file:
    uv run python scripts/generate_api_docs.py --output api-reference.md

    # Split into per-category files:
    uv run python scripts/generate_api_docs.py --output-dir docs/reference
"""

from __future__ import annotations

import argparse
import re
from pathlib import Path
from typing import TYPE_CHECKING, cast

import griffe

if TYPE_CHECKING:
    from griffe import Class, Function, Object

# ---------------------------------------------------------------------------
# GitHub source link
# ---------------------------------------------------------------------------

_GITHUB_ICON = (
    '<svg aria-hidden="true" height="14" viewBox="0 0 16 16"'
    ' version="1.1" width="14"'
    ' xmlns="http://www.w3.org/2000/svg"'
    ' style="vertical-align:-2px;display:inline-block">'
    '<path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53'
    " 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49"
    "-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13"
    "-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72"
    " 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2"
    "-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2"
    "-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27"
    " 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82"
    ".44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0"
    " 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0"
    " 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013"
    ' 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/>'
    "</svg>"
)

_REPO_ROOT = Path(__file__).parent.parent


def _gh_link(obj: Object, repo_url: str, ref: str) -> str:
    """Return an inline HTML GitHub source link, or '' if unavailable."""
    lineno = getattr(obj, "lineno", None)
    filepath = getattr(obj, "filepath", None)
    if not lineno or not filepath:
        return ""
    try:
        rel = Path(filepath).relative_to(_REPO_ROOT)
    except ValueError:
        return ""
    url = f"{repo_url}/blob/{ref}/{rel.as_posix()}#L{lineno}"
    return (
        f' <a href="{url}" target="_blank" rel="noopener noreferrer"'
        f' title="View source on GitHub">{_GITHUB_ICON}</a>'
    )


# ---------------------------------------------------------------------------
# Docstring rendering
# ---------------------------------------------------------------------------


_RST_ROLE_RE = re.compile(r":(?:class|meth|func|attr|mod|obj|exc|data):`~?([^`]+)`")


def _strip_rst_roles(text: str) -> str:
    """Convert RST cross-reference roles to plain markdown backtick refs.

    ``:class:`Page``` → ``Page``,  ``:meth:`~Flow.run``` → ``run``.
    """

    def _replace(m: re.Match[str]) -> str:
        target = m.group(1)
        # :meth:`~Foo.bar` → show only the last component
        if target.startswith("~"):
            target = target[1:].rsplit(".", 1)[-1]
        return f"`{target}`"

    return _RST_ROLE_RE.sub(_replace, text)


def _render_docstring(obj: Object) -> str:
    """Render a griffe docstring as markdown (Google-style sections)."""
    if not obj.docstring:
        return ""

    parsed = obj.docstring.parse("google")
    parts: list[str] = []

    for section in parsed:
        kind = section.kind.value

        if kind == "text":
            parts.append(_strip_rst_roles(str(section.value).strip()))

        elif kind == "parameters":
            parts.append("**Args:**\n")
            for param in section.value:
                ann = f"`{param.annotation}`" if param.annotation else ""
                desc = (
                    _strip_rst_roles(param.description.strip())
                    if param.description
                    else ""
                )
                parts.append(f"- `{param.name}` {ann} — {desc}")
            parts.append("")

        elif kind == "attributes":
            parts.append("**Attributes:**\n")
            for attr in section.value:
                ann = f"`{attr.annotation}`" if attr.annotation else ""
                desc = (
                    _strip_rst_roles(attr.description.strip())
                    if attr.description
                    else ""
                )
                parts.append(f"- `{attr.name}` {ann} — {desc}")
            parts.append("")

        elif kind in ("returns", "yields"):
            label = "Returns" if kind == "returns" else "Yields"
            items = (
                section.value if isinstance(section.value, list) else [section.value]
            )
            descs = [
                (f"`{i.annotation}` — " if i.annotation else "")
                + _strip_rst_roles(i.description or "")
                for i in items
            ]
            parts.append(f"**{label}:** {' '.join(descs)}".strip())
            parts.append("")

        elif kind == "raises":
            parts.append("**Raises:**\n")
            for exc in section.value:
                desc = (
                    _strip_rst_roles(exc.description.strip()) if exc.description else ""
                )
                parts.append(f"- `{exc.annotation}` — {desc}")
            parts.append("")

        elif kind == "examples":
            parts.append("**Example:**\n")
            parts.append(str(section.value).strip())
            parts.append("")

    return "\n".join(parts).strip()


# ---------------------------------------------------------------------------
# Signature rendering
# ---------------------------------------------------------------------------


def _render_params(fn: Function) -> str:
    """Render function parameters, dropping self/cls."""
    params = [p for p in fn.parameters if p.name not in ("self", "cls")]
    parts: list[str] = []
    for p in params:
        ann = f": {p.annotation}" if p.annotation else ""
        default = f" = {p.default}" if p.default is not None else ""
        parts.append(f"{p.name}{ann}{default}")
    ret = f" -> {fn.returns}" if fn.returns else ""
    return f"({', '.join(parts)}){ret}"


# ---------------------------------------------------------------------------
# Class / function formatters
# ---------------------------------------------------------------------------


def _format_function(name: str, obj: Function, link: str) -> list[str]:
    """Format a top-level function as markdown."""
    sig = _render_params(obj)
    lines = [f"## `{name}`{link}\n", f"`{name}{sig}`\n"]
    doc = _render_docstring(obj)
    if doc:
        lines.append(doc)
        lines.append("")
    return lines


def _format_class(
    name: str,
    obj: Class,
    exclude: set[str],
    link: str,
    repo_url: str,
    ref: str,
) -> list[str]:
    """Format a class and its public methods as markdown."""
    lines = [f"## `{name}`{link}\n"]
    doc = _render_docstring(obj)
    if doc:
        lines.append(doc)
        lines.append("")

    # Only methods defined directly on this class (not inherited)
    for mname, member in sorted(obj.members.items()):
        if mname.startswith("_") or mname in exclude:
            continue
        if member.is_alias:
            continue
        if not isinstance(member, griffe.Function):
            continue
        mlink = _gh_link(member, repo_url, ref)
        sig = _render_params(member)
        lines.append(f"### `{mname}`{mlink}\n")
        lines.append(f"`{mname}{sig}`\n")
        mdoc = _render_docstring(member)
        if mdoc:
            lines.append(mdoc)
            lines.append("")

    return lines


# ---------------------------------------------------------------------------
# Symbol classification
# ---------------------------------------------------------------------------

# Group names → symbols that belong to them
_CONFIG_TYPES = {"BrowserConfig", "PoolConfig"}
_SESSION_TYPES = {"BrowserSession", "BrowserPool", "Page", "PooledTab"}
_ACTION_FRAMEWORK = {
    "ActionNode",
    "JsActionNode",
    "JsSource",
    "JsTab",
    "Tab",
    "Flow",
    "FlowResult",
    "inline_js",
    "load_js",
}
# Everything else from actions.builtin is a "Builtin Action"

_DEBUG_TYPES = {"DebugSession", "vc_breakpoint"}

# Section key → (output filename, page title, description template)
_SECTION_FILES = {
    "Configuration": (
        "configuration.md",
        "Configuration",
        "Configuration reference for voidcrawl {version}",
    ),
    "Sessions & Pools": (
        "sessions.md",
        "Sessions & Pools",
        "Session and pool reference for voidcrawl {version}",
    ),
    "Action Framework": (
        "action-framework.md",
        "Action Framework",
        "Action framework reference for voidcrawl {version}",
    ),
    "Builtin Actions": (
        "builtin-actions.md",
        "Built-in Actions",
        "Built-in action reference for voidcrawl {version}",
    ),
    "Debug": (
        "debug.md",
        "Debug",
        "Debug and replay reference for voidcrawl {version}",
    ),
}


def _add_to_section(
    section: list[str],
    name: str,
    target: Object,
    exclude: set[str],
    link: str,
    repo_url: str,
    ref: str,
) -> None:
    if isinstance(target, griffe.Class):
        section.extend(_format_class(name, target, exclude, link, repo_url, ref))
    elif isinstance(target, griffe.Function):
        section.extend(_format_function(name, target, link))


def _classify(
    name: str,
    target: Object,
    exclude: set[str],
    sections: dict[str, list[str]],
    repo_url: str,
    ref: str,
) -> None:
    """Route a public symbol into the correct section."""
    link = _gh_link(target, repo_url, ref)

    if name in _CONFIG_TYPES:
        if isinstance(target, griffe.Class):
            sections["Configuration"].extend(
                _format_class(name, target, exclude, link, repo_url, ref)
            )
        else:
            sections["Configuration"].extend(
                _format_function(name, target, link)  # type: ignore[arg-type]
            )
    elif name in _SESSION_TYPES:
        if isinstance(target, griffe.Class):
            sections["Sessions & Pools"].extend(
                _format_class(name, target, exclude, link, repo_url, ref)
            )
    elif name in _ACTION_FRAMEWORK:
        _add_to_section(
            sections["Action Framework"], name, target, exclude, link, repo_url, ref
        )
    elif name in _DEBUG_TYPES:
        _add_to_section(sections["Debug"], name, target, exclude, link, repo_url, ref)
    else:
        # Remaining symbols are builtin actions
        _add_to_section(
            sections["Builtin Actions"], name, target, exclude, link, repo_url, ref
        )


# ---------------------------------------------------------------------------
# Collect public symbols from __all__ across packages
# ---------------------------------------------------------------------------


def _extract_all_names(pkg: griffe.Module) -> list[str]:
    """Extract names from __all__ via regex on the AST value."""
    if "__all__" in pkg.members:
        all_obj = pkg.members["__all__"]
        raw = str(all_obj.value) if hasattr(all_obj, "value") else ""
        names = re.findall(r"'([^']+)'", raw)
        if names:
            return names
    return [n for n in pkg.members if not n.startswith("_")]


def _build_sections(exclude: set[str], repo_url: str, ref: str) -> dict[str, list[str]]:
    """Load the package and populate per-section content lists."""
    pkg = cast("griffe.Module", griffe.load("voidcrawl"))

    # Merge __all__ from voidcrawl, voidcrawl.actions, voidcrawl.debug
    top_names = _extract_all_names(pkg)

    actions_mod = pkg.members.get("actions")
    action_names: list[str] = []
    if actions_mod and isinstance(actions_mod, griffe.Module):
        action_names = _extract_all_names(actions_mod)

    debug_mod = pkg.members.get("debug")
    debug_names: list[str] = []
    if debug_mod and isinstance(debug_mod, griffe.Module):
        debug_names = _extract_all_names(debug_mod)

    # Deduplicate, preserving order
    seen: set[str] = set()
    all_names: list[str] = []
    for n in top_names + action_names + debug_names:
        if n not in seen:
            seen.add(n)
            all_names.append(n)

    sections: dict[str, list[str]] = {k: [] for k in _SECTION_FILES}

    for name in sorted(all_names):
        if name in exclude:
            continue
        # Resolve from the right module
        obj = None
        for mod in (pkg, actions_mod, debug_mod):
            if mod and name in mod.members:
                obj = mod.members[name]
                break
        if obj is None:
            continue
        target = obj.final_target if isinstance(obj, griffe.Alias) else obj
        _classify(name, cast("Object", target), exclude, sections, repo_url, ref)

    return sections


# ---------------------------------------------------------------------------
# Output generators
# ---------------------------------------------------------------------------


def generate_split(
    version: str, exclude: set[str], repo_url: str, ref: str
) -> dict[str, str]:
    """Return ``{filename: content}`` for each reference page."""
    sections = _build_sections(exclude, repo_url, ref)
    result: dict[str, str] = {}

    for key, (filename, title, desc_tmpl) in _SECTION_FILES.items():
        description = desc_tmpl.format(version=version)
        content_lines = sections[key]
        if not content_lines:
            continue
        parts: list[str] = [
            "---",
            f"title: {title}",
            f"description: {description}",
            "---",
            "",
            f"> Generated from voidcrawl `{version}`."
            " Only symbols in `__all__` are listed.",
            "",
        ]
        parts.extend(content_lines)
        result[filename] = "\n".join(parts) + "\n"

    return result


def generate(version: str, exclude: set[str], repo_url: str, ref: str) -> str:
    """Build the full combined API reference markdown."""
    sections = _build_sections(exclude, repo_url, ref)

    parts: list[str] = [
        "---",
        "title: API Reference",
        f"description: Full API reference for voidcrawl {version}",
        f"version: {version}",
        "---",
        "",
        "# API Reference",
        "",
        f"> Generated from voidcrawl `{version}`."
        " Only symbols in `__all__` are listed.",
        "",
    ]

    for section_key, (_, title, _) in _SECTION_FILES.items():
        content = sections[section_key]
        if not content:
            continue
        parts.append(f"# {title}\n")
        parts.extend(content)

    return "\n".join(parts) + "\n"


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    """CLI entry point."""
    parser = argparse.ArgumentParser(
        description="Generate voidcrawl API reference markdown."
    )
    output_group = parser.add_mutually_exclusive_group()
    output_group.add_argument("--output", default="", help="Single output file path")
    output_group.add_argument(
        "--output-dir",
        default="",
        help="Directory to write split reference files into",
    )
    parser.add_argument("--version", default="", help="Version string (e.g. v0.1.0)")
    parser.add_argument(
        "--exclude",
        default="",
        help="Comma-separated list of symbol names to exclude",
    )
    parser.add_argument(
        "--github-repo",
        default="https://github.com/CascadingLabs/VoidCrawl",
        help="GitHub repository base URL",
    )
    parser.add_argument(
        "--ref",
        default="",
        help="Git ref (tag/branch/commit) for source links",
    )
    args = parser.parse_args()

    exclude: set[str] = {s.strip() for s in args.exclude.split(",") if s.strip()}

    if not args.version:
        toml_path = Path(__file__).parent.parent / "pyproject.toml"
        if toml_path.exists():
            text = toml_path.read_text()
            m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
            version = f"v{m.group(1)}" if m else "unknown"
        else:
            version = "unknown"
    else:
        version = args.version

    ref = args.ref or version.lstrip("v")

    if args.output_dir:
        files = generate_split(version, exclude, args.github_repo, ref)
        out_dir = Path(args.output_dir)
        out_dir.mkdir(parents=True, exist_ok=True)
        total = 0
        for filename, content in files.items():
            dest = out_dir / filename
            dest.write_text(content)
            total += len(content)
            print(f"  Wrote {len(content):,} bytes -> {dest}")
        print(f"Done. {total:,} bytes across {len(files)} files.")
    else:
        out_path = args.output or "api-reference.md"
        content = generate(version, exclude, args.github_repo, ref)
        out = Path(out_path)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(content)
        print(f"Wrote {len(content):,} bytes to {out}")


if __name__ == "__main__":
    main()
