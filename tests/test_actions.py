"""Unit tests for the actions framework.

Uses a mock tab -- no browser needed.
"""

from __future__ import annotations

import json
from typing import Any

import pytest

from voidcrawl.actions import (
    ActionNode,
    CdpClick,
    CdpClickAndHold,
    CdpHover,
    CdpScroll,
    CdpScrollDown,
    CdpScrollLeft,
    CdpScrollRight,
    CdpScrollUp,
    CdpTypeText,
    ClearInput,
    ClickAt,
    ClickElement,
    CollectNetworkRequests,
    Flow,
    GetAttribute,
    GetText,
    Hover,
    InstallNetworkObserver,
    JsActionNode,
    JsTab,
    ScrollBy,
    ScrollTo,
    SelectOption,
    SetAttribute,
    SetInputValue,
    Tab,
    WaitForSelector,
    WaitForTimeout,
    inline_js,
)

# ── Mock tabs ─────────────────────────────────────────────────────────────


class MockTab:
    """Records calls to evaluate_js / dispatch_* for assertions."""

    def __init__(self, js_return: object = None) -> None:
        self.js_calls: list[str] = []
        self.mouse_calls: list[dict[str, Any]] = []
        self.key_calls: list[dict[str, Any]] = []
        self.js_return = js_return

    async def evaluate_js(self, expression: str) -> object:
        self.js_calls.append(expression)
        return self.js_return

    async def dispatch_mouse_event(
        self,
        event_type: str,
        x: float,
        y: float,
        button: str = "left",
        click_count: int = 1,
        delta_x: float | None = None,
        delta_y: float | None = None,
        modifiers: int | None = None,
    ) -> None:
        self.mouse_calls.append(
            {
                "event_type": event_type,
                "x": x,
                "y": y,
                "button": button,
                "click_count": click_count,
                "delta_x": delta_x,
                "delta_y": delta_y,
            }
        )

    async def dispatch_key_event(
        self,
        event_type: str,
        key: str | None = None,
        code: str | None = None,
        text: str | None = None,
        modifiers: int | None = None,
    ) -> None:
        self.key_calls.append(
            {"event_type": event_type, "key": key, "code": code, "text": text}
        )


class JsOnlyTab:
    """Minimal mock that only has evaluate_js — satisfies JsTab but not Tab."""

    def __init__(self, js_return: object = None) -> None:
        self.js_calls: list[str] = []
        self.js_return = js_return

    async def evaluate_js(self, expression: str) -> object:
        self.js_calls.append(expression)
        return self.js_return


# ── JS expression helpers ────────────────────────────────────────────────


def _extract_params(expression: str) -> dict[str, Any]:
    """Pull the __params JSON out of an IIFE expression."""
    prefix = "const __params = "
    start = expression.index(prefix) + len(prefix)
    end = expression.index("; ", start)
    result: dict[str, Any] = json.loads(expression[start:end])
    return result


# ── Protocol tests ────────────────────────────────────────────────────────


class TestProtocol:
    def test_mock_tab_satisfies_tab(self) -> None:
        assert isinstance(MockTab(), Tab)

    def test_mock_tab_satisfies_js_tab(self) -> None:
        assert isinstance(MockTab(), JsTab)

    def test_js_only_tab_satisfies_js_tab(self) -> None:
        assert isinstance(JsOnlyTab(), JsTab)

    def test_js_only_tab_does_not_satisfy_full_tab(self) -> None:
        assert not isinstance(JsOnlyTab(), Tab)


# ── Base framework tests ─────────────────────────────────────────────────


class TestJsActionNode:
    @pytest.mark.asyncio
    async def test_iife_wrapping(self) -> None:
        action = ClickAt(10, 20)
        tab = MockTab(js_return="div")
        result = await action.run(tab)
        assert result == "div"
        assert len(tab.js_calls) == 1
        expr = tab.js_calls[0]
        assert expr.startswith("(async () => {")
        assert expr.endswith("})()")

    @pytest.mark.asyncio
    async def test_params_injection(self) -> None:
        action = ClickAt(42, 99)
        tab = MockTab()
        await action.run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"x": 42, "y": 99}

    @pytest.mark.asyncio
    async def test_special_chars_escaped(self) -> None:
        action = ClickElement('div[data-id="foo<bar>"]')
        tab = MockTab()
        await action.run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"selector": 'div[data-id="foo<bar>"]'}

    def test_repr(self) -> None:
        action = ClickAt(1, 2)
        assert "ClickAt" in repr(action)
        assert "x=1" in repr(action)

    @pytest.mark.asyncio
    async def test_default_params_via_vars(self) -> None:
        """params() returns vars(self) by default — no override needed."""

        class Simple(JsActionNode):
            js = inline_js("return __params.val;")

            def __init__(self, val: int) -> None:
                self.val = val

        action = Simple(42)
        assert action.params() == {"val": 42}

        tab = MockTab(js_return=42)
        result = await action.run(tab)
        assert result == 42
        params = _extract_params(tab.js_calls[0])
        assert params == {"val": 42}

    @pytest.mark.asyncio
    async def test_works_with_js_only_tab(self) -> None:
        """JsActionNode should work with a JsTab (no CDP methods)."""
        tab = JsOnlyTab(js_return="ok")
        result = await ClickAt(1, 2).run(tab)
        assert result == "ok"
        assert len(tab.js_calls) == 1


class TestCdpActions:
    @pytest.mark.asyncio
    async def test_cdp_click(self) -> None:
        action = CdpClick(100.0, 200.0)
        tab = MockTab()
        await action.run(tab)
        assert len(tab.mouse_calls) == 2
        assert tab.mouse_calls[0]["event_type"] == "mousePressed"
        assert tab.mouse_calls[1]["event_type"] == "mouseReleased"
        assert tab.mouse_calls[0]["x"] == 100.0
        assert tab.mouse_calls[0]["y"] == 200.0

    @pytest.mark.asyncio
    async def test_cdp_click_and_hold(self) -> None:
        action = CdpClickAndHold(50.0, 60.0, duration_ms=10)
        tab = MockTab()
        await action.run(tab)
        assert len(tab.mouse_calls) == 2
        assert tab.mouse_calls[0]["event_type"] == "mousePressed"
        assert tab.mouse_calls[1]["event_type"] == "mouseReleased"

    @pytest.mark.asyncio
    async def test_cdp_hover(self) -> None:
        action = CdpHover(30.0, 40.0)
        tab = MockTab()
        await action.run(tab)
        assert len(tab.mouse_calls) == 1
        assert tab.mouse_calls[0]["event_type"] == "mouseMoved"

    @pytest.mark.asyncio
    async def test_cdp_scroll(self) -> None:
        action = CdpScroll(x=100.0, y=200.0, delta_x=0, delta_y=300.0)
        tab = MockTab()
        await action.run(tab)
        assert len(tab.mouse_calls) == 1
        call = tab.mouse_calls[0]
        assert call["event_type"] == "mouseWheel"
        assert call["x"] == 100.0
        assert call["y"] == 200.0
        assert call["delta_y"] == 300.0

    @pytest.mark.asyncio
    async def test_cdp_scroll_down(self) -> None:
        action = CdpScrollDown(pixels=200, x=50.0, y=50.0)
        tab = MockTab()
        await action.run(tab)
        assert len(tab.mouse_calls) == 1
        call = tab.mouse_calls[0]
        assert call["event_type"] == "mouseWheel"
        assert call["delta_y"] == 200.0
        assert call["delta_x"] == 0

    @pytest.mark.asyncio
    async def test_cdp_scroll_up(self) -> None:
        action = CdpScrollUp(pixels=200)
        tab = MockTab()
        await action.run(tab)
        call = tab.mouse_calls[0]
        assert call["event_type"] == "mouseWheel"
        assert call["delta_y"] == -200.0

    @pytest.mark.asyncio
    async def test_cdp_scroll_right(self) -> None:
        action = CdpScrollRight(pixels=150)
        tab = MockTab()
        await action.run(tab)
        call = tab.mouse_calls[0]
        assert call["event_type"] == "mouseWheel"
        assert call["delta_x"] == 150.0
        assert call["delta_y"] == 0

    @pytest.mark.asyncio
    async def test_cdp_scroll_left(self) -> None:
        action = CdpScrollLeft(pixels=150)
        tab = MockTab()
        await action.run(tab)
        call = tab.mouse_calls[0]
        assert call["event_type"] == "mouseWheel"
        assert call["delta_x"] == -150.0

    @pytest.mark.asyncio
    async def test_cdp_type_text(self) -> None:
        action = CdpTypeText("ab")
        tab = MockTab()
        await action.run(tab)
        assert len(tab.key_calls) == 4  # keyDown+keyUp for each char
        assert tab.key_calls[0] == {
            "event_type": "keyDown",
            "key": "a",
            "code": None,
            "text": "a",
        }
        assert tab.key_calls[1] == {
            "event_type": "keyUp",
            "key": "a",
            "code": None,
            "text": None,
        }


# ── Builtin action params tests ──────────────────────────────────────────


class TestBuiltinParams:
    @pytest.mark.asyncio
    async def test_scroll_to(self) -> None:
        tab = MockTab()
        await ScrollTo(0, 500).run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"x": 0, "y": 500}

    @pytest.mark.asyncio
    async def test_scroll_by(self) -> None:
        tab = MockTab()
        await ScrollBy(dx=100, dy=-50).run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"dx": 100, "dy": -50}

    @pytest.mark.asyncio
    async def test_hover(self) -> None:
        tab = MockTab()
        await Hover("#btn").run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"selector": "#btn"}

    @pytest.mark.asyncio
    async def test_set_input_value(self) -> None:
        tab = MockTab()
        await SetInputValue("#input", "hello").run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"selector": "#input", "text": "hello"}

    @pytest.mark.asyncio
    async def test_clear_input(self) -> None:
        tab = MockTab()
        await ClearInput("#field").run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"selector": "#field"}

    @pytest.mark.asyncio
    async def test_select_option(self) -> None:
        tab = MockTab()
        await SelectOption("select#color", "red").run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"selector": "select#color", "value": "red"}

    @pytest.mark.asyncio
    async def test_wait_for_selector(self) -> None:
        tab = MockTab()
        await WaitForSelector(".loaded", timeout=3.0).run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"selector": ".loaded", "timeout": 3.0}

    @pytest.mark.asyncio
    async def test_wait_for_timeout(self) -> None:
        tab = MockTab()
        await WaitForTimeout(500).run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"ms": 500}

    @pytest.mark.asyncio
    async def test_get_attribute(self) -> None:
        tab = MockTab(js_return="bar")
        result = await GetAttribute("#el", "data-foo").run(tab)
        assert result == "bar"
        params = _extract_params(tab.js_calls[0])
        assert params == {"selector": "#el", "attr": "data-foo"}

    @pytest.mark.asyncio
    async def test_get_text(self) -> None:
        tab = MockTab(js_return="hello")
        result = await GetText("h1").run(tab)
        assert result == "hello"

    @pytest.mark.asyncio
    async def test_set_attribute(self) -> None:
        tab = MockTab()
        await SetAttribute("#el", "class", "active").run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"selector": "#el", "attr": "class", "value": "active"}


# ── Flow tests ────────────────────────────────────────────────────────────


class TestFlow:
    @pytest.mark.asyncio
    async def test_sequential_execution(self) -> None:
        tab = MockTab()
        flow = Flow([ScrollTo(0, 100), ClickAt(50, 50)])
        result = await flow.run(tab)
        assert len(result.results) == 2
        assert len(tab.js_calls) == 2

    @pytest.mark.asyncio
    async def test_mixed_js_and_cdp(self) -> None:
        tab = MockTab()
        flow = Flow([CdpClick(10.0, 20.0), ClickAt(30, 40)])
        result = await flow.run(tab)
        assert len(result.results) == 2
        assert len(tab.mouse_calls) == 2  # from CdpClick
        assert len(tab.js_calls) == 1  # from ClickAt

    @pytest.mark.asyncio
    async def test_empty_flow(self) -> None:
        tab = MockTab()
        result = await Flow().run(tab)
        assert result.results == []
        assert result.last is None

    @pytest.mark.asyncio
    async def test_last_property(self) -> None:
        tab = MockTab(js_return="result")
        result = await Flow([ClickAt(1, 2)]).run(tab)
        assert result.last == "result"

    def test_add_chaining(self) -> None:
        flow = Flow()
        returned = flow.add(ClickAt(1, 2)).add(ScrollTo(0, 0))
        assert returned is flow
        assert len(flow) == 2

    @pytest.mark.asyncio
    async def test_error_stops_flow(self) -> None:
        class FailTab(MockTab):
            async def evaluate_js(self, expression: str) -> object:
                raise RuntimeError("boom")

        tab = FailTab()
        flow = Flow([ClickAt(1, 2), ClickAt(3, 4)])
        with pytest.raises(RuntimeError, match="boom"):
            await flow.run(tab)

    def test_repr(self) -> None:
        flow = Flow([ClickAt(1, 2)])
        assert "Flow" in repr(flow)
        assert "ClickAt" in repr(flow)


# ── Custom action tests ───────────────────────────────────────────────────


class TestCustomAction:
    @pytest.mark.asyncio
    async def test_user_defined_js_action(self) -> None:
        class MyAction(JsActionNode):
            js = inline_js("return __params.msg;")

            def __init__(self, msg: str) -> None:
                self.msg = msg

        tab = MockTab(js_return="hello")
        result = await MyAction("hello").run(tab)
        assert result == "hello"

    @pytest.mark.asyncio
    async def test_user_defined_cdp_action(self) -> None:
        class DragTo(ActionNode):
            def __init__(self, fx: float, fy: float, tx: float, ty: float) -> None:
                self.fx, self.fy, self.tx, self.ty = fx, fy, tx, ty

            async def run(self, tab: Tab) -> None:
                await tab.dispatch_mouse_event("mousePressed", self.fx, self.fy)
                await tab.dispatch_mouse_event("mouseMoved", self.tx, self.ty)
                await tab.dispatch_mouse_event("mouseReleased", self.tx, self.ty)

        tab = MockTab()
        await DragTo(0, 0, 100, 100).run(tab)
        assert len(tab.mouse_calls) == 3
        assert tab.mouse_calls[0]["event_type"] == "mousePressed"
        assert tab.mouse_calls[1]["event_type"] == "mouseMoved"
        assert tab.mouse_calls[2]["event_type"] == "mouseReleased"

    @pytest.mark.asyncio
    async def test_custom_params_override(self) -> None:
        """Verify that params() can still be overridden when needed."""

        class Filtered(JsActionNode):
            js = inline_js("return __params.key;")

            def __init__(self, key: str, _internal: int = 0) -> None:
                self.key = key
                self._internal = _internal

            def params(self) -> dict[str, Any]:
                return {"key": self.key}  # exclude _internal

        action = Filtered("val", _internal=99)
        tab = MockTab(js_return="val")
        await action.run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"key": "val"}
        assert "_internal" not in params


# ── Network observer action tests ────────────────────────────────────────


class TestNetworkObserver:
    @pytest.mark.asyncio
    async def test_install_observer_sends_js(self) -> None:
        tab = MockTab()
        await InstallNetworkObserver().run(tab)
        assert len(tab.js_calls) == 1
        expr = tab.js_calls[0]
        assert "__vc_network_log" in expr
        assert "PerformanceObserver" in expr

    @pytest.mark.asyncio
    async def test_install_observer_empty_params(self) -> None:
        action = InstallNetworkObserver()
        assert action.params() == {}

    @pytest.mark.asyncio
    async def test_collect_default_no_clear(self) -> None:
        tab = MockTab()
        await CollectNetworkRequests().run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"clear": False}

    @pytest.mark.asyncio
    async def test_collect_with_clear(self) -> None:
        tab = MockTab()
        await CollectNetworkRequests(clear=True).run(tab)
        params = _extract_params(tab.js_calls[0])
        assert params == {"clear": True}

    @pytest.mark.asyncio
    async def test_collect_returns_js_result(self) -> None:
        entries = [
            {
                "name": "https://example.com/app.js",
                "type": "script",
                "duration": 50,
                "size": 1024,
            },
            {
                "name": "https://example.com/style.css",
                "type": "link",
                "duration": 30,
                "size": 512,
            },
        ]
        tab = MockTab(js_return=entries)
        result = await CollectNetworkRequests().run(tab)
        assert result == entries
        assert len(result) == 2

    @pytest.mark.asyncio
    async def test_install_then_collect_flow(self) -> None:
        """InstallNetworkObserver + CollectNetworkRequests in a Flow."""
        entries = [
            {
                "name": "https://example.com/a.js",
                "type": "script",
                "duration": 10,
                "size": 100,
            },
        ]
        tab = MockTab(js_return=entries)
        flow = Flow([InstallNetworkObserver(), CollectNetworkRequests()])
        result = await flow.run(tab)
        assert len(result.results) == 2
        assert result.last == entries

    def test_install_repr(self) -> None:
        assert "InstallNetworkObserver" in repr(InstallNetworkObserver())

    def test_collect_repr(self) -> None:
        r = repr(CollectNetworkRequests(clear=True))
        assert "CollectNetworkRequests" in r
        assert "clear=True" in r
