"""Type stubs for voidcrawl.actions.builtin.network."""

from __future__ import annotations

from voidcrawl.actions._base import JsActionNode

class InstallNetworkObserver(JsActionNode):
    def __init__(self) -> None: ...
    def params(self) -> dict[str, object]: ...

class CollectNetworkRequests(JsActionNode):
    clear: bool
    def __init__(self, *, clear: bool = False) -> None: ...
