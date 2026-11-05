# Seam: clarity rage-click and dead-click metrics

Provider: clarity-api CONN-CLARITY-API-DATA-PLATFORM-OPERATIONS-R001
Consumer: medulla MEDULLA-DATAPLAT-VOCAB-R006.7

## Wire shape

`clarity_insights` action `get_data_export` names and surfaces the rage-click and dead-click metrics by URL dimension. Medulla maps the export to `dp:BehaviorMetric` (`dp:metricName`, `dp:metricValue`, `dp:windowDays`, `dp:dimensionName`, `dp:dimensionValue`) through `crates/medulla-core/ontology/fixtures/data_platform/clarity.ttl`, virtual for windows up to 3 days and materialised beyond with a stated reason.

## Refusal codes

A window longer than 3 days is served from the declared snapshot only; a request for a metric the export does not name abstains typed.

## Authoritative types

`dp:AnalyticsSite`, `dp:BehaviorMetric` in `crates/medulla-core/ontology/data_platform-v1.ttl`.

## Test vectors

`crates/medulla-core/tests/data_platform_vocab.rs::mapping_fixture_clarity_validates`.
