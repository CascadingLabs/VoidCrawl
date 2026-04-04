"""Extensible browser actions backed by JavaScript or CDP commands."""

from void_crawl.actions._base import (
    ActionNode,
    JsActionNode,
    JsSource,
    inline_js,
    load_js,
)
from void_crawl.actions._flow import Flow, FlowResult
from void_crawl.actions._protocol import JsTab, Tab
from void_crawl.actions.builtin import (
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
    GetAttribute,
    GetText,
    Hover,
    ScrollBy,
    ScrollTo,
    SelectOption,
    SetAttribute,
    SetInputValue,
    WaitForSelector,
    WaitForTimeout,
)

__all__ = [
    # framework
    "ActionNode",
    "CdpClick",
    "CdpClickAndHold",
    "CdpHover",
    "CdpScroll",
    "CdpScrollDown",
    "CdpScrollLeft",
    "CdpScrollRight",
    "CdpScrollUp",
    "CdpTypeText",
    "ClearInput",
    # builtin - click
    "ClickAt",
    "ClickElement",
    "Flow",
    "FlowResult",
    # builtin - dom
    "GetAttribute",
    "GetText",
    # builtin - hover
    "Hover",
    "JsActionNode",
    "JsSource",
    "JsTab",
    "ScrollBy",
    # builtin - scroll
    "ScrollTo",
    "SelectOption",
    "SetAttribute",
    # builtin - input
    "SetInputValue",
    "Tab",
    # builtin - wait
    "WaitForSelector",
    "WaitForTimeout",
    "inline_js",
    "load_js",
]
