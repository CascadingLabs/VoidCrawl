"""Validation tests for the explicit Python interrupt API."""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from voidcrawl import InterruptRequest


def test_interrupt_request_defaults_to_bounded_ttl() -> None:
    request = InterruptRequest(code="policy.operator_review", summary="Review the page")

    assert request.ttl_seconds == 600


@pytest.mark.parametrize(
    ("code", "summary"),
    [("", "Review"), (" ", "Review"), ("policy.review", "  ")],
)
def test_interrupt_request_rejects_blank_redacted_metadata(
    code: str, summary: str
) -> None:
    with pytest.raises(ValidationError):
        InterruptRequest(code=code, summary=summary)


def test_interrupt_request_rejects_unbounded_ttl() -> None:
    with pytest.raises(ValidationError):
        InterruptRequest(code="policy.review", summary="Review", ttl_seconds=3601)
