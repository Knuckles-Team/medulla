"""Client-side placement authority consumer (MEDULLA-PLACEMENT-R611).

The engine is the only placement authority. This module never computes a
route: it asks the engine for the member list (``ClusterMembers``) and for the
full route of a graph (``PlacementRoute``) through the generated client, keeps
the newest snapshot it has accepted, and refuses anything older than its
routing epoch with the stable code ``PLACEMENT_STALE`` (then refetches once).

It also owns the three client mechanics every production caller shares:

* :func:`select_transport` -- the one function that maps an endpoint to a
  local-socket, TLS or mTLS transport. :class:`~medulla.pool.ConnectionPool`
  uses it; no other caller parses endpoint schemes.
* :class:`Breaker` -- a per-endpoint circuit breaker that opens after
  consecutive failures and half-opens on a timer.
* :class:`TenantPool` -- a bounded LRU of per-tenant clients.

:class:`PlacementMetrics` is the evidence surface: breaker state, pool size,
evictions and stale rejects.
"""

from __future__ import annotations

import re
import time
from collections import OrderedDict
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Generic, Literal, TypeVar

if TYPE_CHECKING:
    from .client import MedullaClient

PLACEMENT_STALE = "PLACEMENT_STALE"
PLACEMENT_NOT_GRANTED = "PLACEMENT_NOT_GRANTED"

# The engine's canonical resource-id alphabet and size (``ResourceId``).
MAX_TENANT_ID_BYTES = 1_024
_CANONICAL_ID = re.compile(r"[A-Za-z0-9._:/@#-]+")

TransportKind = Literal["unix", "tcp", "tls", "mtls"]
BreakerState = Literal["closed", "open", "half_open"]


class PlacementStaleError(ValueError):
    """A snapshot or route older than the client's routing epoch."""

    code = PLACEMENT_STALE

    def __init__(self, detail: str) -> None:
        super().__init__(f"{PLACEMENT_STALE}: {detail}")
        self.detail = detail


class PlacementNotGrantedError(PermissionError):
    """A node holds no placement route for the tenant (MEDULLA-PLACEMENT-R611.7).

    Raised before any segment of the tenant graph is fetched from the shared root.
    """

    code = PLACEMENT_NOT_GRANTED

    def __init__(self, tenant: str, partition: str) -> None:
        super().__init__(
            f"{PLACEMENT_NOT_GRANTED}: no placement route for tenant {tenant!r} "
            f"partition {partition!r}"
        )
        self.tenant = tenant
        self.partition = partition


@dataclass(frozen=True)
class Transport:
    """The transport an endpoint selects."""

    kind: TransportKind
    address: str


def select_transport(endpoint: str, *, client_cert: bool = False) -> Transport:
    """Map an endpoint to its transport. The only endpoint-scheme parser.

    ``unix://path`` or a bare path selects the local socket; ``tcp://host:port``
    selects plain TCP; ``tls://host:port`` selects TLS, upgraded to mTLS when
    the caller presents a client certificate.
    """
    if not isinstance(endpoint, str) or not endpoint.strip():
        raise ValueError("endpoint must be a non-empty string")
    if endpoint.startswith("tls://"):
        address = endpoint[len("tls://") :]
        kind: TransportKind = "mtls" if client_cert else "tls"
    elif endpoint.startswith("tcp://"):
        address = endpoint[len("tcp://") :]
        kind = "tcp"
    elif endpoint.startswith("unix://"):
        address = endpoint[len("unix://") :]
        kind = "unix"
    elif "://" in endpoint:
        raise ValueError(f"unsupported endpoint scheme: {endpoint.split('://')[0]}")
    else:
        address = endpoint
        kind = "unix"
    if not address:
        raise ValueError("endpoint has no address")
    return Transport(kind=kind, address=address)


def validate_tenant_id(tenant: object) -> str:
    """Return ``tenant`` when it is a canonical engine resource id."""
    if (
        not isinstance(tenant, str)
        or not tenant
        or len(tenant.encode("utf-8")) > MAX_TENANT_ID_BYTES
        or not tenant.isascii()
        or _CANONICAL_ID.fullmatch(tenant) is None
    ):
        raise ValueError(
            f"tenant must be a canonical resource id of 1..={MAX_TENANT_ID_BYTES} bytes"
        )
    return tenant


def _bounded(name: str, value: object, low: int, high: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"{name} must be an integer")
    if not low <= value <= high:
        raise ValueError(f"{name} must be within [{low}, {high}], got {value}")
    return value


@dataclass(frozen=True)
class PlacementConfig:
    """The client config keys of the placement pool and breaker.

    ``placement.pool_size`` (clients), ``placement.breaker_failures``
    (consecutive failures) and ``placement.breaker_half_open_ms`` (ms). Each
    value is bounds-checked at construction; an out-of-range value raises.
    """

    pool_size: int = 64
    breaker_failures: int = 5
    breaker_half_open_ms: int = 10_000

    POOL_SIZE_BOUNDS = (1, 1_024)
    BREAKER_FAILURES_BOUNDS = (1, 100)
    BREAKER_HALF_OPEN_MS_BOUNDS = (100, 600_000)

    def __post_init__(self) -> None:
        _bounded("placement.pool_size", self.pool_size, *self.POOL_SIZE_BOUNDS)
        _bounded(
            "placement.breaker_failures",
            self.breaker_failures,
            *self.BREAKER_FAILURES_BOUNDS,
        )
        _bounded(
            "placement.breaker_half_open_ms",
            self.breaker_half_open_ms,
            *self.BREAKER_HALF_OPEN_MS_BOUNDS,
        )

    @classmethod
    def from_mapping(cls, config: Mapping[str, object]) -> PlacementConfig:
        """Build from dotted client config keys; unknown ``placement.`` keys
        are refused so a misspelt key never silently keeps a default."""
        known = {
            "placement.pool_size": "pool_size",
            "placement.breaker_failures": "breaker_failures",
            "placement.breaker_half_open_ms": "breaker_half_open_ms",
        }
        values: dict[str, Any] = {}
        for key, value in config.items():
            if not key.startswith("placement."):
                continue
            field_name = known.get(key)
            if field_name is None:
                raise ValueError(f"unknown placement config key: {key}")
            values[field_name] = value
        return cls(**values)

    def breaker(self, clock: Callable[[], float] = time.monotonic) -> Breaker:
        return Breaker(
            failure_threshold=self.breaker_failures,
            reset_after_s=self.breaker_half_open_ms / 1_000,
            clock=clock,
        )


DEFAULT_PLACEMENT_CONFIG = PlacementConfig()


class Breaker:
    """Closed -> open after ``failure_threshold`` consecutive failures;
    open -> half-open once ``reset_after_s`` has elapsed; a half-open trial
    success closes it, a failure re-opens it."""

    def __init__(
        self,
        *,
        failure_threshold: int = PlacementConfig.breaker_failures,
        reset_after_s: float = PlacementConfig.breaker_half_open_ms / 1_000,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        _bounded(
            "failure_threshold",
            failure_threshold,
            *PlacementConfig.BREAKER_FAILURES_BOUNDS,
        )
        low, high = PlacementConfig.BREAKER_HALF_OPEN_MS_BOUNDS
        if (
            isinstance(reset_after_s, bool)
            or not isinstance(reset_after_s, int | float)
            or not low <= reset_after_s * 1_000 <= high
        ):
            raise ValueError(
                f"reset_after_s must be within [{low / 1_000}, {high / 1_000}] s"
            )
        self._threshold = failure_threshold
        self._reset_after = reset_after_s
        self._clock = clock
        self._failures = 0
        self._opened_at: float | None = None

    @property
    def state(self) -> BreakerState:
        if self._opened_at is None:
            return "closed"
        if self._clock() - self._opened_at >= self._reset_after:
            return "half_open"
        return "open"

    def allow(self) -> bool:
        return self.state != "open"

    def record_success(self) -> None:
        self._failures = 0
        self._opened_at = None

    def record_failure(self) -> None:
        if self.state == "half_open":
            self._opened_at = self._clock()
            return
        self._failures += 1
        if self._failures >= self._threshold:
            self._opened_at = self._clock()


ClientT = TypeVar("ClientT")


class TenantPool(Generic[ClientT]):
    """A bounded LRU of per-tenant clients. The least recently used client is
    closed and evicted when a new tenant would exceed ``capacity``."""

    def __init__(
        self,
        factory: Callable[[str], Awaitable[ClientT]],
        *,
        capacity: int = PlacementConfig.pool_size,
        close: Callable[[ClientT], Awaitable[None]] | None = None,
    ) -> None:
        _bounded("capacity", capacity, *PlacementConfig.POOL_SIZE_BOUNDS)
        self._factory = factory
        self._close = close
        self._capacity = capacity
        self._clients: OrderedDict[str, ClientT] = OrderedDict()
        self.evictions = 0

    def __len__(self) -> int:
        return len(self._clients)

    async def acquire(self, tenant: str) -> ClientT:
        validate_tenant_id(tenant)
        client = self._clients.get(tenant)
        if client is not None:
            self._clients.move_to_end(tenant)
            return client
        client = await self._factory(tenant)
        self._clients[tenant] = client
        while len(self._clients) > self._capacity:
            _, evicted = self._clients.popitem(last=False)
            self.evictions += 1
            if self._close is not None:
                await self._close(evicted)
        return client

    def tenants(self) -> list[str]:
        return list(self._clients)

    async def open_routed(
        self, directory: PlacementDirectory, tenant: str, partition: str
    ) -> ClientT:
        """Open a tenant graph from the shared root only with a placement route
        (MEDULLA-PLACEMENT-R611.7). A tenant without a placed route with at
        least one endpoint refuses ``PLACEMENT_NOT_GRANTED`` before the factory
        (which fetches segments) runs."""
        validate_tenant_id(tenant)
        route = await directory.route(tenant, partition)
        if not route.get("placed") or not route.get("endpoints"):
            raise PlacementNotGrantedError(tenant, partition)
        return await self.acquire(tenant)


@dataclass
class PlacementMetrics:
    """Evidence: breaker state per endpoint, pool size, evictions, stale rejects."""

    breaker_state: dict[str, BreakerState] = field(default_factory=dict)
    pool_size: int = 0
    evictions: int = 0
    stale_rejects: int = 0
    snapshots_accepted: int = 0
    routes_accepted: int = 0


class PlacementDirectory:
    """The engine-authoritative topology and route view one client holds."""

    def __init__(
        self,
        client: MedullaClient,
        *,
        breaker_factory: Callable[[], Breaker] | None = None,
        tenant_pool: TenantPool[Any] | None = None,
        config: PlacementConfig = DEFAULT_PLACEMENT_CONFIG,
    ) -> None:
        self._client = client
        self._config = config
        self._breaker_factory = breaker_factory or config.breaker
        self._breakers: dict[str, Breaker] = {}
        self._tenant_pool = tenant_pool
        self._snapshot: dict[str, Any] | None = None
        self._routes: dict[tuple[str, str], dict[str, Any]] = {}
        self._routing_epoch = 0
        self._stale_rejects = 0
        self._snapshots_accepted = 0
        self._routes_accepted = 0

    @property
    def routing_epoch(self) -> int:
        return self._routing_epoch

    def breaker(self, endpoint: str) -> Breaker:
        breaker = self._breakers.get(endpoint)
        if breaker is None:
            breaker = self._breaker_factory()
            self._breakers[endpoint] = breaker
        return breaker

    async def _fetch_snapshot(self) -> dict[str, Any]:
        try:
            snapshot = await self._client.cluster_topology.members(
                min_placement_epoch=self._routing_epoch
            )
        except ValueError as exc:
            if "stale" in str(exc) or "backwards" in str(exc):
                raise PlacementStaleError(str(exc)) from exc
            raise
        placement_epoch = snapshot["placement_epoch"]
        if placement_epoch < self._routing_epoch:
            raise PlacementStaleError(
                f"snapshot epoch {placement_epoch} < routing epoch "
                f"{self._routing_epoch}"
            )
        return snapshot

    async def refresh(self) -> dict[str, Any]:
        """Fetch the member list; a stale answer is rejected and refetched once."""
        try:
            snapshot = await self._fetch_snapshot()
        except PlacementStaleError:
            self._stale_rejects += 1
            snapshot = await self._fetch_snapshot()
        self._snapshot = snapshot
        self._routing_epoch = max(self._routing_epoch, snapshot["placement_epoch"])
        self._snapshots_accepted += 1
        return snapshot

    async def _fetch_route(self, tenant: str, partition: str) -> dict[str, Any]:
        route = await self._client.placement.route(
            tenant, partition, client_epoch=self._routing_epoch
        )
        if route.get("stale") or route["epoch"] < self._routing_epoch:
            raise PlacementStaleError(
                f"route epoch {route['epoch']} < routing epoch {self._routing_epoch}"
            )
        return route

    async def route(self, tenant: str, partition: str) -> dict[str, Any]:
        """The engine's full route for ``(tenant, partition)``. Never computed
        here. A stale route is rejected and refetched once."""
        validate_tenant_id(tenant)
        try:
            route = await self._fetch_route(tenant, partition)
        except PlacementStaleError:
            self._stale_rejects += 1
            route = await self._fetch_route(tenant, partition)
        self._routing_epoch = max(self._routing_epoch, route["epoch"])
        self._routes_accepted += 1
        self._routes[(tenant, partition)] = route
        return route

    def cached_route(self, tenant: str, partition: str) -> dict[str, Any] | None:
        """The last accepted engine route for ``(tenant, partition)``, or
        ``None`` when none is held or its epoch is below the routing epoch.
        A local dict lookup: it never contacts the engine and never computes a
        route (MEDULLA-PLACEMENT-R611.6 hot path)."""
        route = self._routes.get((tenant, partition))
        if route is None or route["epoch"] < self._routing_epoch:
            return None
        return route

    async def shard_topology(
        self, *, local_endpoint: str | None = None
    ) -> dict[str, Any]:
        """Shard topology for operator views: mode, endpoints with health and
        breaker state, and the epochs of the snapshot it came from."""
        snapshot = await self.refresh()
        endpoints: list[dict[str, Any]] = []
        seen: set[str] = set()
        for group in snapshot.get("groups", []):
            for member in group.get("members", []):
                endpoint = member["client_endpoint"]
                if endpoint in seen:
                    continue
                seen.add(endpoint)
                endpoints.append(
                    {
                        "endpoint": endpoint,
                        "node_id": str(member["node_id"]),
                        "local": endpoint == local_endpoint,
                        "reachable": member.get("health") == "healthy",
                        "breaker": self.breaker(endpoint).state,
                    }
                )
        groups = len(snapshot.get("groups", []))
        return {
            "mode": "sharded" if groups > 1 or len(endpoints) > 1 else "single",
            "cluster_id": snapshot["cluster_id"],
            "membership_epoch": snapshot["membership_epoch"],
            "placement_epoch": snapshot["placement_epoch"],
            "groups": groups,
            "endpoints": endpoints,
        }

    def metrics(self) -> PlacementMetrics:
        pool = self._tenant_pool
        return PlacementMetrics(
            breaker_state={ep: b.state for ep, b in self._breakers.items()},
            pool_size=len(pool) if pool is not None else 0,
            evictions=pool.evictions if pool is not None else 0,
            stale_rejects=self._stale_rejects,
            snapshots_accepted=self._snapshots_accepted,
            routes_accepted=self._routes_accepted,
        )
