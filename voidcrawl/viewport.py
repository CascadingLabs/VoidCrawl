"""Variable CDP viewport: named device presets (phones, tablets, desktop
sizes) and custom dimensions, in the spirit of Chrome DevTools' device
toolbar.

The raw ``_ext`` binding is dumb on purpose — it takes a preset name string
or bare width/height/dsf/mobile values and validates almost nothing. This
module is where a Python caller gets real validation: :class:`Viewport` is
a pydantic model that rejects an invalid combination (preset + custom size,
custom size missing a dimension, neither given) before it ever crosses into
Rust.

``Page``/``PooledTab`` are the raw PyO3 classes and take flat
``viewport_preset=``/``viewport_width=``/… kwargs directly (see
``Page.screenshot``'s docstring) rather than a ``Viewport`` object — build a
validated one here, then unpack it with :meth:`Viewport.as_kwargs`::

    from voidcrawl import BrowserSession
    from voidcrawl.viewport import Viewport

    async with BrowserSession() as browser:
        page = await browser.new_page("https://example.com")
        vp = Viewport(preset="iPhone 16 Pro Max")
        result = await page.screenshot(**vp.as_kwargs(prefix="viewport_"))

        # Persistent — stays active across subsequent calls on this page.
        persistent = Viewport(width=1920, height=1080)
        await page.set_viewport(**persistent.as_kwargs())
"""

from __future__ import annotations

from pydantic import BaseModel, model_validator

from voidcrawl._ext import list_device_presets as _list_device_presets


class Viewport(BaseModel):
    """A device/viewport override: a named preset OR a custom size.

    Exactly one of ``preset`` or (``width`` and ``height``) must be set.

    Attributes:
        preset: Named device — see :func:`list_device_presets` for valid
            names (e.g. ``"iPhone 16 Pro Max"``, ``"iPad Pro 11"``,
            ``"Desktop 1080p"``). Mutually exclusive with ``width``/``height``.
        width: Custom viewport width in CSS pixels. Requires ``height``.
        height: Custom viewport height in CSS pixels. Requires ``width``.
        device_scale_factor: Device pixel ratio for a custom viewport
            (default ``1.0``). Ignored with ``preset``.
        mobile: Emulate a mobile viewport for a custom size — also enables
            touch (default ``False``). Ignored with ``preset``.

    Example:
        >>> Viewport(preset="Pixel 7")
        >>> Viewport(width=1280, height=800)
        >>> Viewport(width=390, height=844, device_scale_factor=3.0, mobile=True)
    """

    preset: str | None = None
    width: int | None = None
    height: int | None = None
    device_scale_factor: float | None = None
    mobile: bool | None = None

    @model_validator(mode="after")
    def _check_exclusive(self) -> Viewport:
        has_preset = self.preset is not None
        has_custom = self.width is not None or self.height is not None
        if has_preset and has_custom:
            raise ValueError("preset is mutually exclusive with width/height")
        if has_custom and (self.width is None or self.height is None):
            raise ValueError("width and height must both be set together")
        if not has_preset and not has_custom:
            raise ValueError("pass either preset= or width=+height=")
        if has_preset:
            known = {name for name, *_ in _list_device_presets()}
            if self.preset not in known:
                raise ValueError(f"unknown device preset {self.preset!r}")
        return self

    def as_kwargs(self, *, prefix: str = "") -> dict[str, str | int | float | bool]:
        """Render as ``_ext``-compatible kwargs, optionally prefixed
        (``screenshot()`` needs ``viewport_width``, ``viewport_mobile``, …;
        ``set_viewport()`` takes them bare).
        """
        fields = {
            "preset": self.preset,
            "width": self.width,
            "height": self.height,
            "device_scale_factor": self.device_scale_factor,
            "mobile": self.mobile,
        }
        return {f"{prefix}{k}": v for k, v in fields.items() if v is not None}


class DevicePreset(BaseModel):
    """One named entry from :func:`list_device_presets`."""

    name: str
    width: int
    height: int
    device_scale_factor: float
    mobile: bool


def list_device_presets() -> list[DevicePreset]:
    """List named device presets (phones, tablets, desktop sizes) available
    to :class:`Viewport` — Chrome DevTools' device-toolbar dropdown, as data.

    Example:
        >>> [p.name for p in list_device_presets()]
        ['iPhone 16 Pro Max', 'iPhone 16', ..., 'Desktop 1080p', ...]
    """
    return [
        DevicePreset(
            name=name,
            width=width,
            height=height,
            device_scale_factor=dsf,
            mobile=mobile,
        )
        for name, width, height, dsf, mobile in _list_device_presets()
    ]
