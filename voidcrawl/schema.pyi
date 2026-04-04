"""Type stubs for voidcrawl.schema."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from pydantic import BaseModel

__all__ = [
    "Attr",
    "Schema",
    "Text",
    "safe_url",
    "strip_tags",
]

def safe_url(value: str | None) -> str | None:
    """Return ``None`` for ``javascript:``, ``data:``, and ``vbscript:`` URLs."""
    ...

def strip_tags(value: str | None) -> str | None:
    """Strip HTML tag-like substrings from *value*."""
    ...

def Text(  # noqa: N802
    css: str,
    *,
    sanitize: Callable[[str | None], str | None] | None = None,
) -> Any:
    """Declare a field extracted from an element's ``textContent``.

    Args:
        css: CSS sub-selector whose ``textContent`` (trimmed) is used as the
            field value.  Pass ``""`` to target the root element itself.
        sanitize: Optional callable applied to the extracted value before
            Pydantic validation.
    """
    ...

def Attr(  # noqa: N802
    css: str,
    attr: str,
    *,
    sanitize: Callable[[str | None], str | None] | None = None,
) -> Any:
    """Declare a field extracted from an element attribute.

    Args:
        css: CSS sub-selector relative to the root element.  Pass ``""`` to
            target the root element itself.
        attr: HTML attribute name (e.g. ``"href"``, ``"src"``).
        sanitize: Optional callable applied to the extracted value before
            Pydantic validation.
    """
    ...

class Schema(BaseModel):
    """Base class for declarative DOM extraction models.

    Subclass and annotate fields with :func:`Text` or :func:`Attr` to
    declare how each field is extracted from the DOM.

    Example::

        class Article(Schema):
            headline: str = Text("h2")
            url: str | None = Attr("a", "href", sanitize=safe_url)
            excerpt: str | None = Text(".summary", sanitize=strip_tags)
    """

    @classmethod
    def _vc_fields_spec(cls) -> dict[str, str | tuple[str, str]]:
        """Build the ``fields`` dict expected by QueryAll."""
        ...
