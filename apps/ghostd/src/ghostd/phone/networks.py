"""Which network a phone address is on: the home network or the operator's tunnel (ADR 0020).

Deterministic and unconfigured: the interface that holds the address decides. An interface
the kernel calls WireGuard (``DEVTYPE=wireguard`` in ``/sys/class/net/<name>/uevent``), or
any other point-to-point tunnel device (link type ``ARPHRD_NONE``, as ``wireguard-go`` and
OpenVPN make), is the tunnel; any other interface is the home network. Nothing is guessed from
the order of ``GHOST_PHONE_LISTEN``.

An address that was on an interface once and is gone now (the tunnel went down) keeps the
network it had. ``None`` means Ghost can't tell: a wildcard address (on every interface), a
loopback one, or one no interface of this computer has held since the daemon started.
"""

from __future__ import annotations

import ipaddress
import time
from collections.abc import Callable
from pathlib import Path
from typing import Literal

Network = Literal["home", "tunnel"]

ARPHRD_NONE = 65534  # the link type of tun devices: no hardware header
REFRESH_SECS = 30.0  # how long the address-to-interface map is trusted
SYS_NET = Path("/sys/class/net")


def _normal(host: str) -> str | None:
    """An IP in one spelling (``fe80::1%wg0`` and ``[fe80::1]`` as ``fe80::1``); None for a
    host name or anything else that isn't an IP."""
    host = host.strip().removeprefix("[").removesuffix("]").split("%", 1)[0]
    try:
        return ipaddress.ip_address(host).compressed
    except ValueError:
        return None


def system_addresses() -> dict[str, str]:
    """Every IP on this computer and the interface that holds it, from ``getifaddrs``.
    Loopback is left out: a phone never reaches it."""
    import ifaddr

    out: dict[str, str] = {}
    for adapter in ifaddr.get_adapters():
        for ip in adapter.ips:
            raw = ip.ip[0] if isinstance(ip.ip, tuple) else ip.ip
            ip_text = _normal(str(raw))
            if ip_text and not ipaddress.ip_address(ip_text).is_loopback:
                out.setdefault(ip_text, adapter.name)
    return out


def is_tunnel_interface(name: str, sys_net: Path = SYS_NET) -> bool:
    """WireGuard by the kernel's own word, or any tun device."""
    base = sys_net / name
    try:
        uevent = (base / "uevent").read_text()
    except OSError:
        uevent = ""
    if any(line.strip() == "DEVTYPE=wireguard" for line in uevent.splitlines()):
        return True
    try:
        return int((base / "type").read_text().strip()) == ARPHRD_NONE
    except (OSError, ValueError):
        return False


class Networks:
    """Classifies an IP. The address map is re-read at most every ``REFRESH_SECS``, so a
    phone polling ``/hello`` costs a dict lookup, not a ``getifaddrs`` call."""

    def __init__(
        self,
        addresses: Callable[[], dict[str, str]] = system_addresses,
        is_tunnel: Callable[[str], bool] = is_tunnel_interface,
        clock: Callable[[], float] = time.monotonic,
        refresh: float = REFRESH_SECS,
    ) -> None:
        self._addresses = addresses
        self._is_tunnel = is_tunnel
        self._clock = clock
        self._refresh = refresh
        self._map: dict[str, Network] = {}
        self._known: dict[str, Network] = {}  # every IP ever seen, for one that went away
        self._read_at: float | None = None

    def _current(self) -> dict[str, Network]:
        now = self._clock()
        if self._read_at is None or now - self._read_at >= self._refresh:
            try:
                found = self._addresses()
            except OSError:
                found = {}
            self._map = {
                ip: "tunnel" if self._is_tunnel(name) else "home" for ip, name in found.items()
            }
            self._known.update(self._map)
            self._read_at = now
        return self._map

    def of(self, host: str | None) -> Network | None:
        ip = _normal(host or "")
        if ip is None:
            return None
        if ipaddress.ip_address(ip).is_unspecified:
            return None
        return self._current().get(ip) or self._known.get(ip)


__all__ = ["Network", "Networks", "is_tunnel_interface", "system_addresses"]
