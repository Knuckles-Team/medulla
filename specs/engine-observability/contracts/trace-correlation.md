# Seam contract: trace correlation

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-OBSERVE-R002.2
Consumer: arbora ARBORA-HOST-R008.1007.1

## Provider

Repository: medulla. Rows: `MEDULLA-OBSERVE-R002.2` for the field, `MEDULLA-OBSERVE-R002.3` for the request span and `MEDULLA-OBSERVE-R002.5` for the client method.

## Consumer

Repository: arbora, row `ARBORA-HOST-R008.1007.1`. Its correlation context (`ARBORA-HOST-R008.1007`) carries one run id across processes. It sends that id to the engine as a W3C `traceparent` on every engine call.

## Wire shape

| Carrier | Field | Rule |
|---|---|---|
| Native request | `Request.trace_id` | Optional text, at most 55 characters, format `00-<32 hex>-<16 hex>-<2 hex>`; outside the signed envelope; omitted when none |
| HTTP-shaped listener | header `traceparent` | Same value |

## Behavior

- The server parses the value. A malformed value is dropped with a DEBUG line and the request runs.
- Every engine log line of the request shows `trace_id` and `request_id`. Every task the request spawns shows the same ids.
- The server generates its own `request_id` for each request. The caller does not choose it.
- The trace id carries no authority. The server never uses it for authorization or routing.

## Refusal codes

None. A malformed trace context is dropped, never refused. `ARBORA-OPS-R018` serves the introspection that shows the ids; arbora adds no handler for it.

## Version and compatibility

An old client omits the field. An old server ignores the field only if it tolerates unknown optional keys; this release adds the field on both sides, so the consumer sends it only to a server whose `Health` reports `readiness`.
