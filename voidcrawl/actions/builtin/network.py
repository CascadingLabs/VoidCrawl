"""Network observation actions (JS-tier).

Provides :class:`InstallNetworkObserver` and :class:`CollectNetworkRequests`
for intercepting and logging network request URLs during page load.
"""

from __future__ import annotations

from voidcrawl.actions._base import JsActionNode, inline_js


class InstallNetworkObserver(JsActionNode):
    """Install a ``PerformanceObserver`` that records network requests.

    Call this **after** navigation completes (e.g. after
    ``wait_for_network_idle``). The observer uses ``buffered: true``
    to retroactively capture all resource entries from the current
    navigation, plus any future requests.

    Use :class:`CollectNetworkRequests` to retrieve the captured entries.

    The observer stores entries on ``window.__vc_network_log``.
    """

    js = inline_js("""\
window.__vc_network_log = [];
const obs = new PerformanceObserver(list => {
    for (const entry of list.getEntries()) {
        window.__vc_network_log.push({
            name: entry.name,
            type: entry.initiatorType,
            duration: Math.round(entry.duration),
            size: entry.transferSize ?? 0,
        });
    }
});
obs.observe({ type: 'resource', buffered: true });
return null;
""")

    def __init__(self) -> None:
        pass

    def params(self) -> dict[str, object]:
        return {}


class CollectNetworkRequests(JsActionNode):
    """Retrieve all network entries captured by :class:`InstallNetworkObserver`.

    Returns a list of dicts, each containing:

    - ``name`` — the full URL of the request
    - ``type`` — the initiator type (``"script"``, ``"img"``, ``"fetch"``, etc.)
    - ``duration`` — round-trip time in milliseconds
    - ``size`` — transfer size in bytes (0 if unavailable)

    Pass ``clear=True`` to reset the log after collection.
    """

    js = inline_js("""\
const log = window.__vc_network_log ?? [];
if (__params.clear) window.__vc_network_log = [];
return log;
""")

    def __init__(self, *, clear: bool = False) -> None:
        self.clear = clear
