"""Type stubs for voidcrawl.actions.builtin."""

from __future__ import annotations

from voidcrawl.actions.builtin.click import (
    CdpClick as CdpClick,
)
from voidcrawl.actions.builtin.click import (
    CdpClickAndHold as CdpClickAndHold,
)
from voidcrawl.actions.builtin.click import (
    ClickAt as ClickAt,
)
from voidcrawl.actions.builtin.click import (
    ClickElement as ClickElement,
)
from voidcrawl.actions.builtin.dom import (
    GetAttribute as GetAttribute,
)
from voidcrawl.actions.builtin.dom import (
    GetText as GetText,
)
from voidcrawl.actions.builtin.dom import (
    QueryAll as QueryAll,
)
from voidcrawl.actions.builtin.dom import (
    SetAttribute as SetAttribute,
)
from voidcrawl.actions.builtin.hover import CdpHover as CdpHover
from voidcrawl.actions.builtin.hover import Hover as Hover
from voidcrawl.actions.builtin.input import (
    CdpTypeText as CdpTypeText,
)
from voidcrawl.actions.builtin.input import (
    ClearInput as ClearInput,
)
from voidcrawl.actions.builtin.input import (
    SelectOption as SelectOption,
)
from voidcrawl.actions.builtin.input import (
    SetInputValue as SetInputValue,
)
from voidcrawl.actions.builtin.network import (
    CollectNetworkRequests as CollectNetworkRequests,
)
from voidcrawl.actions.builtin.network import (
    InstallNetworkObserver as InstallNetworkObserver,
)
from voidcrawl.actions.builtin.scroll import (
    CdpScroll as CdpScroll,
)
from voidcrawl.actions.builtin.scroll import (
    CdpScrollDown as CdpScrollDown,
)
from voidcrawl.actions.builtin.scroll import (
    CdpScrollLeft as CdpScrollLeft,
)
from voidcrawl.actions.builtin.scroll import (
    CdpScrollRight as CdpScrollRight,
)
from voidcrawl.actions.builtin.scroll import (
    CdpScrollUp as CdpScrollUp,
)
from voidcrawl.actions.builtin.scroll import (
    ScrollBy as ScrollBy,
)
from voidcrawl.actions.builtin.scroll import (
    ScrollTo as ScrollTo,
)
from voidcrawl.actions.builtin.wait import (
    WaitForSelector as WaitForSelector,
)
from voidcrawl.actions.builtin.wait import (
    WaitForTimeout as WaitForTimeout,
)

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
