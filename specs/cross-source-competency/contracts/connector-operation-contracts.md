# Seam: connector operation contracts for the mapping fixtures

Provider: uptime-kuma-agent CONN-UPTIME-KUMA-AGENT-CONNECTOR-CONTENT-R003
Consumer: medulla MEDULLA-CQ-XSRC-R003.4

## Wire shape

Each connector serves a read tool and an SDK operation contract: the uptime connector lists monitors (`CONN-UPTIME-KUMA-AGENT-CONNECTOR-CONTENT-R003`), the procurement connector resolves its supplier class (`CONN-ERPNEXT-AGENT-CONNECTOR-CONTENT-R002`) and the container-manager connector declares access contracts for pods, containers and images (`CONN-CONTAINER-MANAGER-MCP-CONNECTOR-CONTENT-R003`). Medulla maps each recorded response through an approved `vg:VirtualMapping` (`MEDULLA-CQ-XSRC-R002.7`, `MEDULLA-CQ-XSRC-R003.4`, `MEDULLA-CQ-XSRC-R003.5`).

## Refusal codes

A mapping without approval is not effective (`MEDULLA-SRC-AUTH-R003.4`). A contract kind that disagrees with the connection kind abstains with `ContractMismatch`.

## Authoritative types

The mapping vocabulary is `vg:VirtualMapping` in `crates/medulla-core/ontology/virtual_graph-v1.ttl`; the operation-bound reader is `MEDULLA-FEDERATED-QUERY-R073`.

## Test vectors

The recorded responses are `tests/federated/fixtures/xsrc_recorded/` (planned); the shape tests are in `crates/medulla-core/tests/supply_chain_vocab.rs`.
