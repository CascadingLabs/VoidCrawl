"""Built-in browser actions shipped with voidcrawl.

Re-exports every action from the ``click``, ``dom``, ``hover``,
``input``, ``network``, ``scroll``, and ``wait`` modules for convenient
import::

    from voidcrawl.actions import ClickAt, WaitForSelector
"""

from voidcrawl.actions.builtin.click import (
    CdpClick,
    CdpClickAndHold,
    ClickAt,
    ClickElement,
)
from voidcrawl.actions.builtin.dom import GetAttribute, GetText, QueryAll, SetAttribute
from voidcrawl.actions.builtin.hover import CdpHover, Hover
from voidcrawl.actions.builtin.input import (
    CdpTypeText,
    ClearInput,
    SelectOption,
    SetInputValue,
)
from voidcrawl.actions.builtin.network import (
    CollectNetworkRequests,
    InstallNetworkObserver,
)
from voidcrawl.actions.builtin.scroll import (
    CdpScroll,
    CdpScrollDown,
    CdpScrollLeft,
    CdpScrollRight,
    CdpScrollUp,
    ScrollBy,
    ScrollTo,
)
from voidcrawl.actions.builtin.wait import WaitForSelector, WaitForTimeout

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
    "ClickAt",
    "ClickElement",
    "CollectNetworkRequests",
    "GetAttribute",
    "GetText",
    "Hover",
    "InstallNetworkObserver",
    "QueryAll",
    "ScrollBy",
    "ScrollTo",
    "SelectOption",
    "SetAttribute",
    "SetInputValue",
    "WaitForSelector",
    "WaitForTimeout",
]
