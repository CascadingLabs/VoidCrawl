"""Interactive step debugger for voidcrawl actions.

Provides :class:`DebugSession` for stepping through actions with
breakpoints, history inspection, and back/forward navigation.
Use :func:`vc_breakpoint` to mark action classes that should
automatically pause execution.

Example:
    Minimal usage::

        from voidcrawl.debug import DebugSession

        async with BrowserSession(BrowserConfig(headless=False)) as browser:
            page = await browser.new_page(url)
            dbg = DebugSession(page, start_url=url)
            dbg.add(SetInputValue("#name", "World"))
            dbg.add(ClickElement("#greet"))
            dbg.add(GetText("#title"))
            await dbg.start()

    With breakpoints::

        @vc_breakpoint
        class MyClick(JsActionNode): ...


        dbg = DebugSession(page, start_url=url, stepping=False)
        dbg.add(MyClick("#btn"))  # will pause here
        await dbg.start()
"""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

try:
    import click
    from rich.console import Console
    from rich.panel import Panel
    from rich.table import Table
except ImportError as _err:
    raise ImportError(
        "voidcrawl.debug requires the 'debug' extra: uv add 'voidcrawl[debug]'"
    ) from _err

from voidcrawl.actions._flow import FlowResult

if TYPE_CHECKING:
    from voidcrawl.actions._base import ActionNode
    from voidcrawl.actions._flow import Flow
    from voidcrawl.actions._protocol import Tab

__all__ = [
    "DebugSession",
    "vc_breakpoint",
]

_BREAKPOINT_ATTR = "_vc_breakpoint"
_console = Console()


def vc_breakpoint(cls: type) -> type:
    """Mark an action class as a debugger breakpoint.

    When a :class:`DebugSession` encounters an action whose class is
    marked with this decorator, it pauses execution regardless of
    whether stepping mode is active.

    Args:
        cls: The action class to mark.

    Returns:
        The same class, with an internal marker attribute set.

    Example:
        >>> @vc_breakpoint
        ... class ImportantClick(JsActionNode):
        ...     js = inline_js("document.querySelector(__params.s).click();")
        ...
        ...     def __init__(self, s: str) -> None:
        ...         self.s = s
    """
    setattr(cls, _BREAKPOINT_ATTR, True)
    return cls


def _is_breakpoint(action: ActionNode) -> bool:
    """Return True if *action*'s class is decorated with :func:`vc_breakpoint`."""
    return getattr(type(action), _BREAKPOINT_ATTR, False)


def _has_selector(action: ActionNode) -> str | None:
    """Return the selector attribute if present, else None."""
    return getattr(action, "selector", None)


async def _highlight(tab: Tab, selector: str) -> None:
    """Flash a red outline on *selector* for 400 ms."""
    js = f"""\
(async () => {{
    const el = document.querySelector({selector!r});
    if (!el) return;
    const prev = el.style.outline;
    const prevOff = el.style.outlineOffset;
    el.style.outline = '3px solid red';
    el.style.outlineOffset = '2px';
    await new Promise(r => setTimeout(r, 400));
    el.style.outline = prev;
    el.style.outlineOffset = prevOff;
}})()"""
    await tab.evaluate_js(js)


async def _async_key(prompt: str = "") -> str:
    """Print *prompt*, then read one keypress without blocking the loop."""
    _console.print(prompt, end="")
    loop = asyncio.get_running_loop()
    ch: str = await loop.run_in_executor(None, click.getchar)
    _console.print(ch)
    return ch


class _HistoryEntry:
    """One executed action and its result."""

    __slots__ = ("action", "result")

    def __init__(self, action: ActionNode, result: object) -> None:
        self.action = action
        self.result = result


class DebugSession:
    """Interactive step debugger for browser actions.

    Queue actions via :meth:`add` (or :meth:`add_flow`), then call
    :meth:`start` to execute them with an interactive debug control.

    Args:
        tab: The page or pooled tab to run actions against.
        start_url: URL to navigate to when rewinding.  Required for
            back/restart — if omitted those commands are disabled.
        stepping: If ``True`` (default), pause before every action.
            If ``False``, run freely and only pause at breakpoints.
        step_delay: Seconds to wait after each action in non-stepping
            mode (ignored when paused at a prompt). Defaults to ``0.3``.
        highlight: Flash a CSS outline on selector-targeted elements
            before executing the action. Defaults to ``True``.
        nav_settle_secs: Seconds to wait after triggering a rewind
            navigation before replaying actions.  Increase if pages
            are slow to load. Defaults to ``0.5``.

    Example:
        >>> dbg = DebugSession(page, start_url="https://example.com")
        >>> dbg.add(ClickElement("#link"))
        >>> dbg.add(GetText("h1"))
        >>> await dbg.start()
    """

    def __init__(
        self,
        tab: Tab,
        *,
        start_url: str | None = None,
        stepping: bool = True,
        step_delay: float = 0.3,
        highlight: bool = True,
        nav_settle_secs: float = 0.5,
    ) -> None:
        self._tab = tab
        self._start_url = start_url
        self._stepping = stepping
        self._step_delay = step_delay
        self._highlight = highlight
        self._nav_settle_secs = nav_settle_secs

        self._queue: list[ActionNode] = []
        self._history: list[_HistoryEntry] = []
        self._pos = 0

    # ── Queue management ─────────────────────────────────────────────

    def add(self, action: ActionNode) -> DebugSession:
        """Append a single action to the execution queue.

        Args:
            action: The action to enqueue.

        Returns:
            This session, for chaining.
        """
        self._queue.append(action)
        return self

    def add_flow(self, flow: Flow) -> DebugSession:
        """Append every action from a :class:`Flow` to the queue.

        Args:
            flow: The flow whose actions to enqueue.

        Returns:
            This session, for chaining.
        """
        self._queue.extend(flow)
        return self

    # ── Execution ────────────────────────────────────────────────────

    async def start(self) -> FlowResult:
        """Run the queued actions with interactive debug control.

        Prints a command prompt before each action (or only at
        breakpoints, depending on *stepping*).  The user can type:

        - **n** / **Enter** — execute the current action and advance
        - **c** — continue running until the next breakpoint
        - **b** — rewind one step (re-navigates and replays)
        - **r** — restart from the beginning
        - **l** — list all queued actions with position marker
        - **h** — show history of executed actions and results
        - **q** — quit the session early

        Returns:
            Aggregated results collected so far.
        """
        self._pos = 0
        self._history.clear()
        self._print_banner()

        while self._pos < len(self._queue):
            action = self._queue[self._pos]
            is_bp = _is_breakpoint(action)
            should_pause = self._stepping or is_bp

            if should_pause:
                cmd = await self._prompt(action, is_bp)
                if cmd == "q":
                    break
                if cmd == "c":
                    self._stepping = False
                    # Fall through to execute
                elif cmd == "b":
                    await self._rewind(self._pos - 1)
                    continue
                elif cmd == "r":
                    await self._rewind(0)
                    continue
                elif cmd == "l":
                    self._print_queue()
                    continue
                elif cmd == "h":
                    self._print_history()
                    continue
                # "n" or Enter — fall through to execute

            await self._exec_action(action)
            self._pos += 1

            if not should_pause:
                await asyncio.sleep(self._step_delay)

        self._print_footer()
        return FlowResult(results=[e.result for e in self._history])

    # ── Internal ─────────────────────────────────────────────────────

    async def _exec_action(self, action: ActionNode) -> object:
        """Execute one action with optional highlighting and logging."""
        selector = _has_selector(action) if self._highlight else None
        if selector:
            await _highlight(self._tab, selector)

        tag = f"[dim]{self._pos + 1}/{len(self._queue)}[/]"
        _console.print(f"[bold green]\u25b6[/]  {tag}  [cyan]{action!r}[/]")

        result = await action.run(self._tab)

        if result is not None:
            _console.print(f"   [dim]\u2192[/] [yellow]{result!r}[/]")

        self._history.append(_HistoryEntry(action, result))
        return result

    async def _rewind(self, target: int) -> None:
        """Re-navigate and replay actions 0..target-1, setting _pos to target."""
        if self._start_url is None:
            _console.print(
                "   [bold red]\u2717[/] Cannot rewind: no start_url provided"
            )
            return
        target = max(target, 0)

        _console.print(
            f"\n[bold magenta]\u21ba[/]  Rewinding to step [bold]{target + 1}[/]  "
            f"[dim](replaying {target} action{'s' if target != 1 else ''})[/]"
        )
        await self._tab.evaluate_js(f"window.location.href = {self._start_url!r}")
        await asyncio.sleep(self._nav_settle_secs)

        self._history.clear()
        for i in range(target):
            action = self._queue[i]
            result = await action.run(self._tab)
            self._history.append(_HistoryEntry(action, result))

        self._pos = target
        self._stepping = True
        _console.print(f"   [bold green]\u2714[/] Ready at step [bold]{target + 1}[/]")

    async def _prompt(self, action: ActionNode, is_bp: bool) -> str:
        """Show the interactive prompt and return the command character."""
        bp_tag = " [bold red]\u25cf BP[/]" if is_bp else ""
        tag = f"[dim]{self._pos + 1}/{len(self._queue)}[/]"
        _console.print(f"\n[bold yellow]\u23f8[/]  {tag}{bp_tag}  [cyan]{action!r}[/]")

        prompt_text = (
            "   [bold]n[/] next  [bold]c[/] cont  [bold]b[/] back"
            "  [bold]r[/] restart  [bold]l[/] list  [bold]h[/] hist"
            "  [bold]q[/] quit \u203a "
        )
        ch = await _async_key(prompt_text)
        cmd = ch.strip().lower()
        if cmd in ("", "n", "\r", "\n"):
            return "n"
        if cmd in ("c", "b", "r", "l", "h", "q"):
            return cmd
        _console.print(f"   [dim]Unknown key:[/] [red]{cmd!r}[/]")
        return await self._prompt(action, is_bp)

    def _print_banner(self) -> None:
        mode = "stepping" if self._stepping else "breakpoints only"
        keys = Table.grid(padding=(0, 3))
        keys.add_column()
        keys.add_column()
        keys.add_column()
        keys.add_column()
        keys.add_row(
            "[bold]n[/] next",
            "[bold]c[/] continue",
            "[bold]b[/] back",
            "[bold]r[/] restart",
        )
        keys.add_row(
            "[bold]l[/] list",
            "[bold]h[/] history",
            "[bold]q[/] quit",
            "",
        )
        title = (
            f"[bold]VoidCrawl Debugger[/]  "
            f"[cyan]{len(self._queue)}[/] actions  [dim]({mode})[/]"
        )
        banner = Panel(
            keys,
            title=title,
            border_style="blue",
            padding=(0, 2),
        )
        _console.print(banner)

    def _print_footer(self) -> None:
        done = len(self._history)
        total = len(self._queue)
        style = "green" if done == total else "yellow"
        footer = Panel(
            f"[{style}]{done}/{total}[/] actions executed",
            title="[bold]Session ended[/]",
            border_style="blue",
            padding=(0, 1),
        )
        _console.print(footer)

    def _print_queue(self) -> None:
        table = Table(
            show_header=True,
            header_style="bold",
            border_style="dim",
            padding=(0, 1),
        )
        table.add_column("#", justify="right", style="dim", width=4)
        table.add_column("State", width=7)
        table.add_column("Action")
        table.add_column("BP", justify="center", width=4)

        for i, action in enumerate(self._queue):
            if i < len(self._history):
                state = "[green]\u2714 done[/]"
            elif i == self._pos:
                state = "[bold yellow]\u25b6 here[/]"
            else:
                state = "[dim]\u00b7 wait[/]"
            bp_marker = "[bold red]\u25cf[/]" if _is_breakpoint(action) else ""
            table.add_row(str(i + 1), state, f"[cyan]{action!r}[/]", bp_marker)

        _console.print(table)

    def _print_history(self) -> None:
        if not self._history:
            _console.print("   [dim](no actions executed yet)[/]")
            return

        table = Table(
            show_header=True,
            header_style="bold",
            border_style="dim",
            padding=(0, 1),
        )
        table.add_column("#", justify="right", style="dim", width=4)
        table.add_column("Action")
        table.add_column("Result")

        for i, entry in enumerate(self._history):
            result_str = (
                f"[yellow]{entry.result!r}[/]" if entry.result is not None else ""
            )
            table.add_row(str(i + 1), f"[cyan]{entry.action!r}[/]", result_str)

        _console.print(table)
