"""Built-in browser actions shipped with void_crawl.

Re-exports every action from the ``click``, ``dom``, ``hover``,
``input``, ``scroll``, and ``wait`` modules for convenient import::

    from void_crawl.actions import ClickAt, WaitForSelector
"""

from void_crawl.actions.builtin.click import (
    CdpClick,
    CdpClickAndHold,
    ClickAt,
    ClickElement,
)
from void_crawl.actions.builtin.dom import GetAttribute, GetText, SetAttribute
from void_crawl.actions.builtin.hover import CdpHover, Hover
from void_crawl.actions.builtin.input import (
    CdpTypeText,
    ClearInput,
    SelectOption,
    SetInputValue,
)
from void_crawl.actions.builtin.scroll import (
    CdpScroll,
    CdpScrollDown,
    CdpScrollLeft,
    CdpScrollRight,
    CdpScrollUp,
    ScrollBy,
    ScrollTo,
)
from void_crawl.actions.builtin.wait import WaitForSelector, WaitForTimeout

__all__ = [
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
    # click
    "ClickAt",
    "ClickElement",
    # dom
    "GetAttribute",
    "GetText",
    # hover
    "Hover",
    "ScrollBy",
    # scroll
    "ScrollTo",
    "SelectOption",
    "SetAttribute",
    # input
    "SetInputValue",
    # wait
    "WaitForSelector",
    "WaitForTimeout",
]
