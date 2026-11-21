"""Served asset usage (MEDULLA-UNIFIED-DATA-PLANE-R764.3.1).

The server records which datasets a served query referenced, per verified
principal, in the `asset_usage.reads` and `asset_usage.queries` series. The count
comes only from the server's own dispatch: a usage claim sent by the client is
not a field of any method and changes nothing.
"""

from __future__ import annotations

import os
import socket
import subprocess
import time
import uuid
from typing import Any

import msgpack
import pytest
from conftest import TEST_AGENT_ID, request_context


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


# The served engine's outbound allow-list is read from the process environment at start,
# so the loopback source named here is allowed before the session engine starts.
_PG_PORT = _free_port()
os.environ["MEDULLA_FEDERATION_ALLOW"] = ",".join(
    part
    for part in (
        os.environ.get("MEDULLA_FEDERATION_ALLOW", ""),
        f"127.0.0.1:{_PG_PORT}",
    )
    if part
)


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _sql(client: Any, graph: str, query: str) -> Any:
    return _decode(client._send("Sql", {"query": query, "params_msgpack": b""}, graph))


def _usage_points(client: Any, graph: str, asset: str) -> int:
    """The served reads of `asset` by the test principal.

    The usage ledger is graph-scoped and shared by every principal, so it is read
    through the served `CatalogUsage` answer (MEDULLA-UNIFIED-DATA-PLANE-R764.8),
    not through `TsRange`, which reads only the caller's own private series.
    """
    try:
        answer = _decode(
            client._send("CatalogUsage", {"asset": asset, "top_n": None}, graph)
        )
    except Exception as refused:
        if "CATALOG_ASSET_NOT_FOUND" in str(refused):
            return 0
        raise
    return sum(
        int(user["reads"])
        for user in answer["frequent_users"]
        if user["principal"] == TEST_AGENT_ID
    )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.3.1")
def test_usage_counts_come_from_served_queries(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.3.1: two served reads count two."""
    graph = f"usage-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        assert _usage_points(client, graph, "nodes") == 0
        _sql(client, graph, "SELECT count(*) AS n FROM nodes")
        _sql(client, graph, "SELECT count(*) AS n FROM nodes")
        assert _usage_points(client, graph, "nodes") == 2

        # A failed statement records nothing.
        with pytest.raises(Exception, match="."):
            _sql(client, graph, "SELECT nope FROM nodes WHERE")
        assert _usage_points(client, graph, "nodes") == 2

        # A client-supplied usage claim is not a field of any method. Whether the
        # server drops the extra field or refuses the request, it counts at most the
        # one read it served itself, never the claimed 1000.
        try:
            client._send(
                "Sql",
                {
                    "query": "SELECT count(*) AS n FROM nodes",
                    "params_msgpack": b"",
                    "asset_usage": {"asset": "nodes", "reads": 1000},
                },
                graph,
            )
        except Exception:
            pass
        assert _usage_points(client, graph, "nodes") in (2, 3)
    finally:
        client.tenants.delete(graph)
        client.close()


_ENV_DSN = "MEDULLA_TEST_PG_DSN"


@pytest.fixture(scope="module")
def pg_log_source():
    """A PostgreSQL server with `pg_stat_statements` and a way to run SQL as a role.

    The nightly lane names one in MEDULLA_TEST_PG_DSN; otherwise a disposable container
    preloads the extension. Skips when neither exists.
    """
    psycopg = pytest.importorskip("psycopg")
    container = None
    dsn = os.environ.get(_ENV_DSN)
    if not dsn:
        if subprocess.run(["docker", "version"], capture_output=True).returncode != 0:
            pytest.skip(f"{_ENV_DSN} is not set and docker is unavailable")
        container = f"medulla-qlog-pg-{uuid.uuid4().hex[:8]}"
        subprocess.run(
            [
                "docker",
                "run",
                "-d",
                "--rm",
                "--name",
                container,
                "-e",
                "POSTGRES_PASSWORD=qlog-test",
                "-p",
                f"127.0.0.1:{_PG_PORT}:5432",
                "postgres:16-alpine",
                "-c",
                "shared_preload_libraries=pg_stat_statements",
            ],
            check=True,
            capture_output=True,
        )
        dsn = f"postgres://postgres:qlog-test@127.0.0.1:{_PG_PORT}/postgres"  # sanitizer:ignore - test placeholder DSN  # noqa: E501
    try:
        deadline = time.monotonic() + 60
        while True:
            try:
                with psycopg.connect(dsn, autocommit=True, connect_timeout=2) as conn:
                    conn.execute("CREATE EXTENSION IF NOT EXISTS pg_stat_statements")
                    conn.execute("SELECT 1 FROM pg_stat_statements LIMIT 1")
                break
            except Exception:
                if time.monotonic() > deadline:
                    pytest.skip("the PostgreSQL server did not become ready")
                time.sleep(0.5)

        def run_as(role: str | None, statements: list[str]) -> None:
            with psycopg.connect(dsn, autocommit=True) as conn:
                if role:
                    conn.execute(f'SET SESSION AUTHORIZATION "{role}"')
                for statement in statements:
                    conn.execute(statement)

        yield dsn, run_as
    finally:
        if container:
            subprocess.run(["docker", "rm", "-f", container], capture_output=True)


def _wait_job(client: Any, job_id: str) -> dict:
    status: dict = {}
    for _ in range(300):
        status = client.jobs.status(job_id)
        state = status["state"]
        name = state if isinstance(state, str) else next(iter(state))
        if name in ("Succeeded", "Failed", "Cancelled"):
            assert name == "Succeeded", f"query log job ended {state!r}"
            return status
        time.sleep(0.1)
    raise AssertionError(f"query log job did not finish: {status!r}")


def _ingest(client: Any, graph: str, source: str) -> dict:
    job = client.jobs.submit(graph, {"SourceQueryLogIngest": {"source": source}})
    return _wait_job(client, job["job_id"])


class _Log:
    """One attached Postgres source whose log holds statements of two roles."""

    def __init__(self, client: Any, graph: str, dsn: str, run_as: Any) -> None:
        tag = uuid.uuid4().hex[:8]
        self.client, self.graph, self.run_as = client, graph, run_as
        self.source = f"qlog-{tag}"
        self.orders, self.customers = f"orders_{tag}", f"customers_{tag}"
        self.alice, self.bob = f"qa_{tag}", f"qb_{tag}"
        run_as(
            None,
            [
                f"CREATE TABLE {self.orders} "
                "(id int, customer_id int, status text, total int)",
                f"CREATE TABLE {self.customers} (id int, region text)",
                f'CREATE ROLE "{self.alice}" LOGIN SUPERUSER',
                f'CREATE ROLE "{self.bob}" LOGIN SUPERUSER',
            ],
        )
        client.query.register_foreign_source(
            self.source,
            {"Sql": {"dsn": dsn, "query": "SELECT 1 AS id", "id_field": "id"}},
        )

    def alice_joins(self, times: int) -> None:
        sql = (
            f"SELECT o.id FROM {self.orders} o JOIN {self.customers} c "
            f"ON c.id = o.customer_id WHERE o.status = 'open'"
        )
        for _ in range(times):
            self.run_as(self.alice, [sql])

    def bob_reads(self, times: int) -> None:
        for _ in range(times):
            self.run_as(self.bob, [f"SELECT id FROM {self.orders} WHERE total > 10"])

    def reads(self, user: str, asset: str) -> int:
        """The reads of `asset` by the source user, from the served usage answer."""
        principal = f"source:{self.source}:{user}"
        return sum(
            int(row["reads"])
            for row in self._usage(asset)["frequent_users"]
            if row["principal"] == principal
        )

    def suggestion(self, kind: str, target: str, asset: str) -> int:
        """The served count of one join or filter suggestion of `asset`, else 0."""
        return sum(
            int(row["count"])
            for row in self._usage(asset)["suggestions"]
            if (row["kind"], row["target"]) == (kind, target)
        )

    def _usage(self, asset: str) -> dict:
        return _decode(
            self.client._send("CatalogUsage", {"asset": asset, "top_n": 50}, self.graph)
        )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.7")
def test_source_query_log_feeds_usage(start_medulla_server, pg_log_source) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.7: reads, joins, watermark, replay."""
    dsn, run_as = pg_log_source
    graph = f"qlog-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        log = _Log(client, graph, dsn, run_as)
        log.alice_joins(3)
        log.bob_reads(2)
        first = _ingest(client, graph, log.source)
        row = first["output"]["rows"][0]
        assert row["watermark_ms"] > 0 and row["statements"] >= 5

        # Usage is read through the served CatalogUsage answer: reads per principal,
        # joins and filters summed over principals and listed from the minimum of 3.
        assert log.reads(log.alice, log.orders) == 3
        assert log.reads(log.alice, log.customers) == 3
        assert log.reads(log.bob, log.orders) == 2
        assert log.suggestion("join", log.customers, log.orders) == 3
        assert log.suggestion("filter", "status", log.orders) == 3
        # Two uses is under the minimum of 3.
        assert log.suggestion("filter", "total", log.orders) == 0

        # A replay of the same log state adds nothing.
        _ingest(client, graph, log.source)
        assert log.reads(log.alice, log.orders) == 3
        assert log.reads(log.bob, log.orders) == 2

        # Only the growth since the watermark is counted.
        log.alice_joins(1)
        _ingest(client, graph, log.source)
        assert log.reads(log.alice, log.orders) == 4
        assert log.suggestion("join", log.customers, log.orders) == 4

        # A source kind with no log refuses with its code.
        client.query.register_foreign_source(
            "no-log",
            {
                "HttpJson": {
                    "url": "https://x.example.test/r",
                    "json_path": "",
                    "field_map": {"id": "id"},
                }
            },
        )
        job = client.jobs.submit(graph, {"SourceQueryLogIngest": {"source": "no-log"}})
        for _ in range(300):
            state = client.jobs.status(job["job_id"])["state"]
            if isinstance(state, dict) and "Failed" in state:
                assert "QUERY_LOG_UNSUPPORTED" in str(state)
                break
            time.sleep(0.1)
        else:
            raise AssertionError("an unsupported source did not fail its job")
    finally:
        client.tenants.delete(graph)
        client.close()


def _usage(client: Any, graph: str, asset: str, top_n: int | None = None) -> dict:
    return _decode(
        client._send("CatalogUsage", {"asset": asset, "top_n": top_n}, graph)
    )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.8")
def test_frequent_users_and_join_suggestions(
    start_medulla_server, pg_log_source
) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.8: users, suggestions, scope."""
    from medulla.client import SyncMedullaClient

    dsn, run_as = pg_log_source
    graph = f"usage-top-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        log = _Log(client, graph, dsn, run_as)
        log.alice_joins(4)
        log.bob_reads(2)
        _ingest(client, graph, log.source)

        answer = _usage(client, graph, log.orders)
        by_principal = {u["principal"]: u["reads"] for u in answer["frequent_users"]}
        assert by_principal[f"source:{log.source}:{log.alice}"] == 4
        assert by_principal[f"source:{log.source}:{log.bob}"] == 2
        assert answer["frequent_users"][0]["reads"] == 4
        kinds = {(s["kind"], s["target"]): s["count"] for s in answer["suggestions"]}
        assert kinds[("join", log.customers)] == 4
        assert kinds[("filter", "status")] == 4
        assert ("filter", "total") not in kinds  # 2 uses is under the minimum of 3

        assert len(_usage(client, graph, log.orders, top_n=1)["frequent_users"]) == 1
        with pytest.raises(Exception, match="CATALOG_LIMIT_INVALID"):
            _usage(client, graph, log.orders, top_n=0)
        with pytest.raises(Exception, match="CATALOG_ASSET_NOT_FOUND"):
            _usage(client, graph, "no_such_table")

        # Without the scope the counts stay and the identities go.
        plain = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(scopes=["kg:read", "query:unified"]),
        )
        hidden = _usage(plain, graph, log.orders)
        assert [u["principal"] for u in hidden["frequent_users"]] == [None, None]
        assert [u["reads"] for u in hidden["frequent_users"]] == [4, 2]
        plain.close()

        # Another tenant sees no trace of the asset.
        other = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context={**request_context(), "tenant": "tenant:other"},
        )
        with pytest.raises(Exception, match="."):
            _usage(other, graph, log.orders)
        other.close()
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R805.7")
def test_usage_cluster_yields_product_proposal(
    start_medulla_server, pg_log_source
) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R805.7.

    Three roles each read the orders and customers tables, so a UsageCluster job
    proposes one data product over both, names the glossary terms the caller
    bound, offers the principal with the most reads as owner candidate and holds
    the proposal in an open change request. A graph with no usage proposes
    nothing.
    """
    dsn, run_as = pg_log_source
    graph = f"qclu-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        log = _Log(client, graph, dsn, run_as)
        carol = f"qc_{uuid.uuid4().hex[:8]}"
        run_as(None, [f'CREATE ROLE "{carol}" LOGIN SUPERUSER'])
        sql = (
            f"SELECT o.id FROM {log.orders} o JOIN {log.customers} c "
            "ON c.id = o.customer_id"
        )
        for _ in range(3):
            run_as(log.alice, [sql])
        for _ in range(2):
            run_as(log.bob, [sql])
        run_as(carol, [sql])

        def cluster(bindings: list[dict]) -> dict:
            job = client.jobs.submit(
                graph, {"UsageCluster": {"term_bindings": bindings}}
            )
            return _wait_job(client, job["job_id"])

        _ingest(client, graph, log.source)
        status = cluster(
            [
                {"asset": log.orders, "term": "term:sale"},
                {"asset": log.customers, "term": "term:party"},
            ]
        )
        rows = status["output"]["rows"]
        assert len(rows) == 1, rows
        row = rows[0]
        assert row["datasets"] == sorted([log.orders, log.customers])
        assert row["terms"] == ["term:party", "term:sale"]
        principals = sorted(
            f"source:{log.source}:{u}" for u in (log.alice, log.bob, carol)
        )
        assert row["principals"] == principals
        assert row["owner_candidate"] == f"source:{log.source}:{log.alice}"
        assert row["change_request_state"] == "open"
        assert row["change_request_reviewers"] == [row["owner_candidate"]]

        # A graph with no usage proposes nothing.
        empty = f"qclu-{uuid.uuid4().hex[:8]}"
        client.tenants.create(empty)
        try:
            job = client.jobs.submit(empty, {"UsageCluster": {"term_bindings": []}})
            assert _wait_job(client, job["job_id"])["output"]["rows"] == []
        finally:
            client.tenants.delete(empty)
    finally:
        client.tenants.delete(graph)
        client.close()


_CATALOG = "https://knuckles-team.github.io/ontology/catalog#"
_COMPONENTS = (
    f"PREFIX cat: <{_CATALOG}>\n"
    "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n"
    "SELECT ?asset ?components ?score WHERE {\n"
    "  ?asset skos:prefLabel ?name .\n"
    '  BIND(cat:rankComponents(?asset, "nodes") AS ?components)\n'
    '  BIND(cat:relevance(?asset, "nodes") AS ?score)\n'
    "  FILTER(?score > 0)\n"
    "}"
)


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.5")
def test_hit_components_listed(start_medulla_server) -> None:
    """Every hit lists its text, vector and usage components; they sum to the score."""
    import json
    from medulla.client import SyncMedullaClient

    graph = f"components-{uuid.uuid4().hex[:8]}"
    admin = _client()
    admin.tenants.create(graph, graph_type="Global")
    client = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(),
    )
    try:
        client.rdf.add_triples(
            turtle=(
                f"@prefix cat: <{_CATALOG}> .\n"
                "@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n"
                '<urn:medulla:asset:table:nodes> a cat:DataResource ; skos:prefLabel "nodes" .\n'
                '<urn:medulla:asset:table:unused> a cat:DataResource ; '
                'skos:prefLabel "nodes archive" .\n'
            )
        )
        # The reads of the asset named `nodes` come only from the server's own dispatch.
        for _ in range(5):
            _sql(client, graph, "SELECT count(*) AS n FROM nodes")

        rows = client.rdf.sparql(_COMPONENTS)
        by_asset = {row["asset"].strip("<>"): row for row in rows}
        assert set(by_asset) == {
            "urn:medulla:asset:table:nodes",
            "urn:medulla:asset:table:unused",
        }
        for row in by_asset.values():
            parts = json.loads(row["components"])
            total = parts["text"] + parts["vector"] + parts["usage"]
            # The components add up to the score before the trust ratio.
            assert abs(total * parts["trust"] - float(row["score"])) < 1e-9
            assert abs(parts["score"] - float(row["score"])) < 1e-9
            # The usage prior never exceeds its cap of the fused score.
            assert parts["usage"] <= 0.20 * total + 1e-9

        used = json.loads(by_asset["urn:medulla:asset:table:nodes"]["components"])
        unused = json.loads(by_asset["urn:medulla:asset:table:unused"]["components"])
        assert used["usage"] > 0
        assert unused["usage"] == 0
    finally:
        client.close()
        admin.tenants.delete(graph)
        admin.close()


_SKOS_LABEL = "http://www.w3.org/2004/02/skos/core#prefLabel"


def _relevance_rows(client: Any, text: str) -> list[dict]:
    return client.rdf.sparql(
        f"PREFIX cat: <{_CATALOG}>\n"
        "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n"
        "SELECT ?asset ?score WHERE {\n"
        "  ?asset skos:prefLabel ?name .\n"
        f'  BIND(cat:relevance(?asset, "{text}") AS ?score)\n'
        "  FILTER(?score > 0)\n"
        "}"
    )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.4")
def test_search_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.4: three refusals, and no hit for a hidden asset."""
    from conftest import register_commons_agents
    from medulla.client import SyncMedullaClient

    tag = uuid.uuid4().hex[:8]
    owner_id, reader_id = f"catalog-owner-{tag}", f"catalog-reader-{tag}"
    register_commons_agents(owner_id, reader_id)

    def connect(agent: str) -> Any:
        return SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(agent_id=agent),
        )

    owner, reader = connect(owner_id), connect(reader_id)
    try:
        # Text over the knob and blank text refuse with their own codes.
        with pytest.raises(Exception, match="CATALOG_QUERY_TOO_LONG"):
            _relevance_rows(reader, "x" * 1001)
        with pytest.raises(Exception, match="CATALOG_QUERY_EMPTY"):
            _relevance_rows(reader, "   ")
        # A limit outside the knob's bounds refuses.
        for bad_limit in (0, 1_000):
            with pytest.raises(Exception, match="CATALOG_LIMIT_INVALID"):
                _decode(
                    reader._send(
                        "CatalogUsage",
                        {"asset": "nodes", "top_n": bad_limit},
                        "__commons__",
                    )
                )

        # An asset the caller cannot read is no hit, and the caller's total does not move.
        needle = f"hiddenasset{tag}"
        before = len(_relevance_rows(reader, needle))
        owner.nodes.add(f"urn:medulla:asset:hidden:{tag}", {_SKOS_LABEL: needle})
        assert len(_relevance_rows(owner, needle)) == 1
        assert len(_relevance_rows(reader, needle)) == before == 0
    finally:
        owner.close()
        reader.close()


_ASSET_COUNT = 10_000
_PAGE = 100


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.6")
def test_search_over_ten_thousand_assets(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.6: ranking order and a stable page cursor
    over a 10,000-asset catalog, and the refusal of text over the knob."""
    from medulla.client import SyncMedullaClient

    graph = f"search10k-{uuid.uuid4().hex[:8]}"
    admin = _client()
    admin.tenants.create(graph, graph_type="Global")
    client = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(),
    )
    ranked = (
        f"PREFIX cat: <{_CATALOG}>\n"
        "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n"
        "SELECT ?asset ?score WHERE {\n"
        "  ?asset skos:prefLabel ?name .\n"
        '  BIND(cat:relevance(?asset, "ledgerfact") AS ?score)\n'
        "  FILTER(?score > 0)\n"
        "} ORDER BY DESC(?score) ?asset LIMIT %d OFFSET %d"
    )
    try:
        # Every tenth asset matches by name, every fiftieth also by description; the
        # rest never match.
        for start in range(0, _ASSET_COUNT, 1_000):
            lines = [
                f"@prefix cat: <{_CATALOG}> .\n",
                "@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n",
                "@prefix dct: <http://purl.org/dc/terms/> .\n",
            ]
            for index in range(start, start + 1_000):
                name = f"ledgerfact table {index}" if index % 10 == 0 else f"asset {index}"
                lines.append(
                    f'<urn:medulla:asset:table:{index:05d}> a cat:DataResource ; '
                    f'skos:prefLabel "{name}"'
                )
                if index % 50 == 0:
                    lines.append(f' ; dct:description "ledgerfact detail {index}"')
                lines.append(" .\n")
            client.rdf.add_triples(turtle="".join(lines))

        # Order: the best score comes first, and the description match ranks above a
        # name-only match.
        first = client.rdf.sparql(ranked % (_PAGE, 0))
        assert len(first) == _PAGE
        scores = [float(row["score"]) for row in first]
        assert scores == sorted(scores, reverse=True)
        assert scores[0] > scores[-1]
        best = {row["asset"].strip("<>") for row in first[:10]}
        assert all(int(asset.rsplit(":", 1)[1]) % 50 == 0 for asset in best)

        # Cursor: the same request returns the same page, and consecutive pages share no
        # asset and together hold every matching asset exactly once.
        assert client.rdf.sparql(ranked % (_PAGE, 0)) == first
        seen: list[str] = []
        offset = 0
        while True:
            page = client.rdf.sparql(ranked % (_PAGE, offset))
            if not page:
                break
            seen.extend(row["asset"].strip("<>") for row in page)
            offset += _PAGE
        assert len(seen) == len(set(seen)) == _ASSET_COUNT // 10
        assert seen[:_PAGE] == [row["asset"].strip("<>") for row in first]

        # Text over the knob refuses.
        with pytest.raises(Exception, match="CATALOG_QUERY_TOO_LONG"):
            _relevance_rows(client, "x" * 1001)
    finally:
        client.close()
        admin.tenants.delete(graph)
        admin.close()
