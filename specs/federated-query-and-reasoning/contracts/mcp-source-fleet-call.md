# Seam contract: operation-bound MCP source served through the fleet

Governing spec: [spec.md](../spec.md).

Provider: arbora ARBORA-FLEET-R015.4 (child of ARBORA-FLEET-R015, in the arbora spec dir `fleet-catalog-and-tools`; it binds this seam to the source executor `ARBORA-FLEET-R043.7`)
Consumer: medulla MEDULLA-FEDERATED-QUERY-R073.3

## Provider

Repository: arbora. Arbora's source executor (`ARBORA-FLEET-R015.4`, over `ARBORA-FLEET-R043.7`) calls the registered MCP server and tool through the governed fleet gateway, under the caller's identity and the shared outbound guard, and returns the rows of the call over the connector row contract of `contracts/connector-source-transport.md`. Medulla never calls MCP.

## Consumer

Repository: medulla. Row `MEDULLA-FEDERATED-QUERY-R073.3` keeps `ForeignSourceSpec::Mcp` and the federation planning (capabilities, pushed key filter, exact local residual). At execution, `McpRemote::fetch` hands a `ConnectorRowRequest` to the host-registered `ConnectorRowSource` port and receives rows; it holds no MCP client and opens no connection.

## Wire shape

| Item | Shape |
|---|---|
| Request | `ConnectorRowRequest { source_label, server, tool, arguments, pushed_filter, budget }` (budget: `max_rows` 1 to 10000, `timeout_ms` 100 to 60000); `arguments` are checked against the declared tool schema before the request is sent |
| Reply | `{source_label, rows, truncated, missing_source}`: rows in the connector row contract, each with its source label; a partial reply names the missing source |

## Refusal codes

- `SOURCE_UNREACHABLE`: the call failed, timed out or returned a result that is not a row list; the source abstains and the label is named.
- `UNSUPPORTED_SEARCH_ARGUMENT`: arguments outside the declared tool schema.
- `OUTBOUND_TARGET_DENIED` and `FOREIGN_SOURCE_UNGRANTED`: as in the connector source transport contract.

## Authoritative types

- `ForeignSourceSpec::Mcp`: `crates/medulla-types/src/wire` in medulla.
- `ConnectorRowSource` and `ConnectorRowRequest`: `crates/medulla-federation/src/federation_opt/mcp.rs` in medulla.

## Test vectors

- `crates/medulla-federation/src/federation_opt/tests.rs::mcp_remote_delegates_to_connector_row_source` in medulla.
- `tests/federation/test_web_metasearch_served.py::test_mcp_source_resolves` in medulla.
