"""CI-safe output checks for the policy-interrupt operator walkthrough."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any
from urllib.parse import unquote

import pytest

EXAMPLE = Path(__file__).parents[1] / "examples" / "policy_interrupt_mock.py"


def load_example() -> Any:
    spec = importlib.util.spec_from_file_location("policy_interrupt_mock", EXAMPLE)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class FakeInterruptedError(Exception):
    """The interrupted error emitted by the fake page mutation."""


class FakePage:
    async def url(self) -> str:
        return "data:text/html,fixture"

    async def target_id(self) -> str:
        return "target-1"

    async def title(self) -> str:
        return "Mock reCAPTCHA review"

    async def navigate(self, _url: str) -> None:
        raise FakeInterruptedError("target is parked")


class FakeBrowser:
    def __init__(self, *_args: object) -> None:
        self.page = FakePage()

    async def __aenter__(self) -> FakeBrowser:
        return self

    async def __aexit__(self, *_args: object) -> None:
        return None

    async def new_page(self, _url: str) -> FakePage:
        return self.page

    async def interrupt(self, _page: FakePage, request: Any) -> SimpleNamespace:
        assert request.code in {"policy.captcha.operator", "policy.login.credentials"}
        return SimpleNamespace(interrupt_id="interrupt-1", target_id="target-1")

    async def resume(self, interrupt_id: str) -> SimpleNamespace:
        assert interrupt_id == "interrupt-1"
        return SimpleNamespace(state="resumed", target_id="target-1")

    async def release(self, interrupt_id: str) -> SimpleNamespace:
        assert interrupt_id == "interrupt-1"
        return SimpleNamespace(state="released", target_id="target-1")


@pytest.mark.asyncio
async def test_recaptcha_mock_is_visible_and_reports_resumed_lifecycle(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    example = load_example()
    monkeypatch.setattr(example, "BrowserSession", FakeBrowser)
    monkeypatch.setattr(example, "SessionInterrupted", FakeInterruptedError)

    async def resolved() -> bool:
        return True

    monkeypatch.setattr(example, "prompt", resolved)
    assert "I'm not a robot" in unquote(example.scenario_url("recaptcha"))

    await example.main("recaptcha")

    output = capsys.readouterr().out
    assert "Parked recaptcha review: target=target-1 interrupt=interrupt-1" in output
    assert "navigation attempt was rejected" in output
    assert "Resumed the original target without replaying" in output
    assert "Interrupt is now resumed." in output


@pytest.mark.asyncio
async def test_mock_release_reports_its_terminal_outcome(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    example = load_example()
    monkeypatch.setattr(example, "BrowserSession", FakeBrowser)
    monkeypatch.setattr(example, "SessionInterrupted", FakeInterruptedError)

    async def released() -> bool:
        return False

    monkeypatch.setattr(example, "prompt", released)

    await example.main("login")

    output = capsys.readouterr().out
    assert "Released the interrupt without replaying" in output
    assert "Interrupt is now released." in output
