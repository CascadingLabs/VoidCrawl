"""Declarative scrape result models backed by Pydantic.

Define a :class:`Schema` subclass with :func:`Text` and :func:`Attr`
field declarations, then pass the class to
:class:`~voidcrawl.actions.QueryAll` to get back typed model instances
instead of raw dicts.

Built-in sanitizers (:func:`safe_url`, :func:`strip_tags`) can be attached
per-field via the ``sanitize=`` keyword argument.

Example::

    import voidcrawl as vc
    from voidcrawl.actions import QueryAll


    class Article(vc.Schema):
        headline: str = vc.Text("h2")
        url: str | None = vc.Attr("a", "href", sanitize=vc.safe_url)
        date: str | None = vc.Text(".byline", sanitize=vc.strip_tags)


    articles = await QueryAll(".article", Article).run(page)
    # articles: list[Article]
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Callable

from pydantic import BaseModel, model_validator
from pydantic.fields import FieldInfo

# ── Selector validation ──────────────────────────────────────────────────

_DANGEROUS_SELECTOR = re.compile(r"[<>\x00]")


def _validate_css(css: str, origin: str) -> None:
    """Raise :exc:`ValueError` if *css* looks like HTML rather than a selector.

    Fires at class-definition time so bad selectors are caught on import,
    not at scrape time.
    """
    if _DANGEROUS_SELECTOR.search(css):
        raise ValueError(
            f"{origin}: selector {css!r} contains HTML-like or null-byte characters. "
            "Pass a valid CSS selector string."
        )


# ── Sanitizer metadata ───────────────────────────────────────────────────


@dataclass(frozen=True)
class _SanitizeMeta:
    """Marker stored in Pydantic ``FieldInfo.metadata`` to carry a sanitizer fn."""

    fn: Callable[[str | None], str | None]


# ── Built-in sanitizers ──────────────────────────────────────────────────

_DANGEROUS_URL = re.compile(r"^\s*(javascript|data|vbscript)\s*:", re.IGNORECASE)
_HTML_TAG = re.compile(r"<[^>]+>")


def safe_url(value: str | None) -> str | None:
    """Return ``None`` for ``javascript:``, ``data:``, and ``vbscript:`` URLs.

    Use this on any :func:`Attr` field that extracts a URL (``href``,
    ``src``, ``action``, etc.) to prevent unsafe schemes from propagating.

    Args:
        value: Raw attribute value from the DOM, or ``None``.

    Returns:
        The original value, or ``None`` if a dangerous scheme was detected.
    """
    if value and _DANGEROUS_URL.match(value):
        return None
    return value


def strip_tags(value: str | None) -> str | None:
    """Strip HTML tag-like substrings from *value*.

    Useful when a field may contain inline markup — removes anything that
    looks like ``<tag>`` or ``</tag>`` using a simple regex.  Not a full
    HTML sanitiser; use on plain-text fields that shouldn't contain markup.

    Args:
        value: Raw text value from the DOM, or ``None``.

    Returns:
        The value with tag-like substrings removed, or ``None`` unchanged.
    """
    if value is None:
        return None
    return _HTML_TAG.sub("", value)


# ── Field constructors ───────────────────────────────────────────────────


def Text(  # noqa: N802
    css: str,
    *,
    sanitize: Callable[[str | None], str | None] | None = None,
) -> Any:
    """Declare a field extracted from an element's ``textContent``.

    The *css* string is a sub-selector relative to the root element
    matched by :class:`~voidcrawl.actions.QueryAll`'s ``selector``
    argument.  Pass ``""`` to target the root element itself.

    CSS selectors are validated at class-definition time; strings
    containing ``<``, ``>``, or null bytes raise :exc:`ValueError`.

    Args:
        css: CSS sub-selector whose ``textContent`` (trimmed) is used
            as the field value.  ``None`` is returned when no element
            matches.
        sanitize: Optional callable applied to the extracted value
            before Pydantic validation.  Use :func:`safe_url` or
            :func:`strip_tags`, or supply your own
            ``(str | None) -> str | None`` function.

    Returns:
        A Pydantic ``FieldInfo`` carrying the selector and sanitizer
        metadata.
    """
    _validate_css(css, "Text")
    fi = FieldInfo(default=None, json_schema_extra={"_vc_selector": css})
    if sanitize:
        fi.metadata.append(_SanitizeMeta(sanitize))
    return fi


def Attr(  # noqa: N802
    css: str,
    attr: str,
    *,
    sanitize: Callable[[str | None], str | None] | None = None,
) -> Any:
    """Declare a field extracted from an element attribute.

    CSS selectors are validated at class-definition time; strings
    containing ``<``, ``>``, or null bytes raise :exc:`ValueError`.

    Args:
        css: CSS sub-selector relative to the root element.  Pass
            ``""`` to target the root element itself.
        attr: HTML attribute name (e.g. ``"href"``, ``"src"``).
        sanitize: Optional callable applied to the extracted value
            before Pydantic validation.  Use :func:`safe_url` to block
            dangerous URL schemes on link/image fields.

    Returns:
        A Pydantic ``FieldInfo`` carrying the selector, attribute, and
        sanitizer metadata.
    """
    _validate_css(css, "Attr")
    fi = FieldInfo(default=None, json_schema_extra={"_vc_attr": [css, attr]})
    if sanitize:
        fi.metadata.append(_SanitizeMeta(sanitize))
    return fi


# ── Schema base class ────────────────────────────────────────────────────


class Schema(BaseModel):
    """Base class for declarative DOM extraction models.

    Subclass this and annotate fields with :func:`Text` or :func:`Attr`
    to declare how each field is extracted from the DOM.  Pass the
    subclass to :class:`~voidcrawl.actions.QueryAll` to receive typed
    instances instead of raw dicts.

    Example::

        class Article(Schema):
            headline: str = Text("h2")
            url: str | None = Attr("a", "href", sanitize=safe_url)
            excerpt: str | None = Text(".summary", sanitize=strip_tags)
    """

    @classmethod
    def _vc_fields_spec(cls) -> dict[str, str | tuple[str, str]]:
        """Build the ``fields`` dict expected by :class:`~voidcrawl.actions.QueryAll`.

        Introspects Pydantic ``model_fields`` for :func:`Text` /
        :func:`Attr` metadata and returns a mapping of field name →
        sub-selector string or ``(sub_selector, attr)`` tuple.

        Returns:
            A dict suitable for passing as the ``fields`` argument of
            :class:`~voidcrawl.actions.QueryAll`.
        """
        spec: dict[str, str | tuple[str, str]] = {}
        for name, fi in cls.model_fields.items():
            extra: dict[str, Any] = fi.json_schema_extra or {}  # type: ignore[assignment]
            if "_vc_selector" in extra:
                spec[name] = extra["_vc_selector"]
            elif "_vc_attr" in extra:
                spec[name] = (extra["_vc_attr"][0], extra["_vc_attr"][1])
        return spec

    def __init_subclass__(cls, **kwargs: Any) -> None:
        super().__init_subclass__(**kwargs)
        # Detect required fields that have no Text()/Attr() declaration at
        # class-definition time so errors surface immediately rather than as
        # an opaque Pydantic "field required" ValidationError at scrape time.
        for name, fi in cls.model_fields.items():
            extra: dict[str, Any] = fi.json_schema_extra or {}  # type: ignore[assignment]
            has_vc_meta = "_vc_selector" in extra or "_vc_attr" in extra
            if not has_vc_meta and fi.is_required():
                raise TypeError(
                    f"{cls.__name__}.{name}: required Schema field must declare "
                    f"how it is extracted — use Text({name!r}) or Attr(...) "
                    "as the field default."
                )

    @model_validator(mode="before")
    @classmethod
    def _vc_sanitize(cls, data: Any) -> Any:
        if not isinstance(data, dict):
            return data
        out = dict(data)
        for name, fi in cls.model_fields.items():
            for m in fi.metadata:
                if isinstance(m, _SanitizeMeta):
                    out[name] = m.fn(out.get(name))
        return out
