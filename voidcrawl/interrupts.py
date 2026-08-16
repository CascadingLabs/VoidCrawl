"""Explicit, AI-free browser interrupt models.

Callers decide when a page requires operator review. VoidCrawl only keeps the
same tab alive and enforces the resulting lifecycle.
"""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, Field, field_validator


class InterruptRequest(BaseModel, frozen=True):
    """Redacted metadata for an explicit operator-review interruption."""

    code: str = Field(min_length=1, max_length=128, pattern=r"^[a-z0-9][a-z0-9._-]*$")
    summary: str = Field(min_length=1, max_length=512)
    ttl_seconds: int = Field(default=600, ge=1, le=3600)

    @field_validator("code", "summary")
    @classmethod
    def _not_blank(cls, value: str) -> str:
        value = value.strip()
        if not value:
            raise ValueError("must not be blank")
        return value


class InterruptRef(BaseModel, frozen=True):
    """Redacted lifecycle state for one interrupted browser target."""

    interrupt_id: str
    target_id: str
    code: str
    summary: str
    state: Literal["interrupted", "resumed", "released", "expired"]
    expires_in_ms: int = Field(ge=0)
