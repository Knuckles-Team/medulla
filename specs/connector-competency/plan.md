# MEDULLA-CQ-CONN — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The engine answers one single-source and one cross-source question per live connector through its existing federated query and unified data plane, reading the source at query time through the connector's declared access contract. This plan adds no planner: it adds the ontology terms the questions read, one table-driven served harness, and the proof per row. The complete question and vocabulary inventory is in the Design section below.

```text
caller (arbora ask / Uql / NlQuery)
  -> Method::Uql / UnifiedQuery   crates/medulla-protocol-types/src/protocol/method/method_05.rs
  -> dispatch_sql_query           crates/medulla-h-query/src/query/dispatch.rs
  -> foreign_scan                 crates/medulla-federation/src/federation_opt/run.rs
  -> ForeignSourceRegistry        crates/medulla-federation/src/federation.rs
  -> connector contract (MCP, A2A, API, GraphQL, virtual graph) at query time
```

Materialisation is an explicit optimisation with a stated reason, never a prerequisite. The `MEDULLA-CQ-CONN-R067` children define acceptance for each contract kind and declared materialisation.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain for ontology and engine code; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `crates/medulla-h-query`, `crates/medulla-federation`, `crates/medulla-h-admin`, `crates/medulla-graph-core`, `crates/medulla-core` (ontology modules) and the standard library. No new third-party dependency.
- **Storage**: None added. Source rows stay at the source; the engine stores only pack and catalog nodes (`MEDULLA-TYPED-PACKS-R106`).
- **Testing**: Rust audit tests under `crates/medulla-core/tests/connector_competency_*.rs`; served Python tests under `tests/connector_competency/` and `tests/typed_packs/`; recorded fixtures, with no live credentials or network in the pull-request gate.
- **Target Platform**: Linux x86_64 and aarch64.
- **Project Type**: Ontology and served-test additions to the existing engine crates.
- **Performance Goals**: A question over recorded fixtures finishes within the budget of `MEDULLA-FEDERATED-QUERY-R722`; the hot-path proof is `MEDULLA-CQ-CONN-R084.6`.
- **Constraints**: A wrong answer is a failure, not an abstention. An unavailable required source abstains by name. Each answer row carries source, record id and as-of time. No version suffix in a name.
- **Scale/Scope**: 287 leaf rows over 5 user stories, 8 `FR-` labels and 5 `SC-` labels; 66 connectors.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Questions run in the engine; external stores stay virtual-graph sources.
- **R-II. Every wait has a deadline**: pass. Each foreign read carries the deadline of the federated query contract.
- **R-III. Every row sets budgets and names a benchmark**: pass. `MEDULLA-CQ-CONN-R084.6` holds the hot-path proof.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name the post-split crates and add no handler-to-handler call.
- **Organization standards**: pass. Rows follow the spec standard, the ecosystem boundaries and the wording standard.

## Project Structure

```text
crates/medulla-core/ontology/                  existing: registered ontology modules the vocabulary rows extend
crates/medulla-core/tests/                     new files: connector_competency_<module>.rs audit tests
crates/medulla-graph-core/src/graph/schema_sources.rs  existing: GraphSchemaSources::default() registers the modules
crates/medulla-h-query/src/query/              existing: Uql, NlQuery and UnifiedQuery dispatch
crates/medulla-federation/src/federation_opt/  existing: foreign_scan and the source registry
crates/medulla-h-admin/src/admin/              existing: ConnectorPack import and projection
tests/connector_competency/                    new: case files, recordings and the table-driven served test
tests/typed_packs/                             existing: pack served tests; new onboarding test
tests/federated/                               new: virtual-graph federation served test
```

New files that rows add are marked `new` in their own `Shape:` sentence.

## Phase 0: Research

No unknowns that block planning; no research.md. The vocabulary terms that no module declares are reused or minted as listed in [data-model.md](data-model.md).

## Phase 1: Design

The rules below bind every row.

1. **Reuse, never a second planner.** Cross-source answers use `specs/federated-query-and-reasoning` and `specs/unified-data-plane`; the engine never gets a connector-specific planner.
2. **Virtual first.** The engine ingests the ontology and the access contract and fetches data at query time. A copy needs a stated reason and a declared row (`MEDULLA-CQ-CONN-R067.6`).
3. **One harness.** The table-driven served test of `MEDULLA-CQ-CONN-R067.9` replaces per-connector acceptance tests; the `.3` rows are retired.
4. **Provenance on every row.** Source, record id and as-of time travel with each answer row; an unavailable source abstains by name.
5. **Vocabulary first.** A question row depends on the producer row that declares its terms; filename-based vocabulary assignment is a proposal until checked against the registered modules.
6. **Reuse before minting.** A term that no module declares is reused from an open vocabulary when an equivalent exists; otherwise it is minted in the owning core module with a `skos:exactMatch` or `skos:closeMatch` mapping and a core superclass. Each decision is listed in [data-model.md](data-model.md).
7. **Planned means planned.** `Method::SourceCoverage` is planned under `MEDULLA-SRC-AUTH-R004.6.1`; rows cite it as planned until that row lands.

The sections below hold the question inventory and the term assignment. Data entities are in [data-model.md](data-model.md); the shortest served proof is in [quickstart.md](quickstart.md); each seam has a document in `contracts/`.

### Design

#### Method

Each live connector gets one rollup with three children: a single-source competency question, a cross-source question that needs at least one other connector, and an acceptance test. Questions are specific to what the system holds. The vocabulary column was decided from the single listing of `crates/medulla-core/ontology/` by file name only (files were not opened); MISSING means no file obviously matches, so a vocabulary file must be added before the question can be answered.

Mechanism is existing engine functionality, not new: cross-source answers use federated query and foreign sources (`specs/federated-query-and-reasoning`, `specs/unified-data-plane`); source authority and connector mapping use the native source connector vocabulary (`native_source_connector-v1.ttl`). Typed abstention names the unconnected source when a linked connector has no data. Rows here only add the question, the vocabulary assignment and the acceptance test.

#### Questions

| Row | Connector | Single-source question | Cross-source question (other connectors; link) | Vocabulary |
|---|---|---|---|---|
| R001 | ansible-tower | Which job templates failed in their last run, and against which inventory hosts? | Which hosts that failed their last Tower run also have an open ServiceNow change or incident? (servicenow, systems-manager; managed host (configuration item)) | infrastructure-v1 |
| R002 | archimate | Which application components serve a given business process, and through which relationships? | Which ArchiMate application components have no matching LeanIX fact sheet or ServiceNow CI? (leanix, servicenow; application / configuration item) | archimate-v1 |
| R003 | archivebox | Which archived snapshots of a given domain failed one or more extractors, and when? | Which archived pages were first seen as FreshRSS articles but never captured by ArchiveBox? (freshrss, searxng; URL / web resource) | documentation-v1 |
| R004 | aris | Which EPC functions in a process model have no outgoing control-flow connection? | Which ARIS-modelled processes have a deployed Camunda definition, and which do not? (camunda, archimate; business process) | process_intelligence-v1 |
| R005 | arr | Which monitored movies and episodes are wanted but have no file, and which indexer last searched them? | Which wanted Radarr titles are currently downloading in qBittorrent and not yet visible in Jellyfin? (qbittorrent, jellyfin; media item (title)) | media-v1 |
| R006 | atlassian | Which Jira issues in a project are blocked and unassigned, and by which linked issue? | Which open Jira issues reference a GitHub pull request that is already merged? (github, plane; work item / pull request) | sdlc_lifecycle-v1 |
| R007 | audiobookshelf | Which audiobook series have a book with listening progress stopped partway, and for which user? | Which audiobooks exist both in Audiobookshelf and as a downloaded file from media-downloader, and which are only one side? (jellyfin, media-downloader; media item (title)) | media-v1 |
| R008 | audio-transcriber | Which recordings were transcribed with a low-confidence segment, and what text was affected? | Which transcribed recordings mention a topic that also appears in PulseLink social or video transcripts? (pulselink, paperless-ngx; document / transcript) | media-v1 / personal-v1 |
| R009 | caddy | Which routes proxy to a given upstream, and which hostnames are served by each site block? | Which Caddy hostnames have no DNS record in Technitium or no Uptime Kuma monitor? (technitium-dns, uptime; hostname) | infrastructure-v1 |
| R010 | camunda | Which process instances have an open incident, and at which activity did they stop? | Which running Camunda process definitions have an open ServiceNow incident raised against their service? (aris, servicenow; business process) | process_intelligence-v1 |
| R011 | ciso-assistant | Which controls in an applied framework are non-compliant and which risk scenarios do they mitigate? | Which CISO Assistant risks map to a ServiceNow CMDB service with an open incident? (onetrust, servicenow; control / risk) | government-v1 |
| R012 | clarity | Which pages have the highest rage-click or dead-click rate over the last week? | Which pages with a spike in rage clicks were changed by a pull request merged in the previous week? (plane, github; web page / release) | MISSING |
| R013 | container-manager | Which containers or pods are restarting repeatedly, and from which image? | Which running images have a newer tag on Docker Hub than the one deployed? (dockerhub, portainer; container image) | infrastructure-v1 |
| R014 | data-science | Which trained models have validation metrics below their baseline run, and on which dataset? | Which fine-tuned models have no evaluation dataset trace in the trace store and no stored artifact in the object store? (langfuse, objectstore; model / dataset) | MISSING |
| R015 | dockerhub | Which repositories in an organization have not been pushed to in a year, and what are their pull counts? | Which Docker Hub repositories correspond to a GitHub repository with recent commits but no recent image push? (container-manager, github; container image / repository) | infrastructure-v1 + software-v1 |
| R016 | documentdb | Which collections have the most documents and which fields are missing in more than half of them? | Which documentdb collections duplicate a table in the SQL database, and which side is newer? (sql, objectstore; dataset / collection) | MISSING |
| R017 | egeria | Which glossary terms are linked to a data asset with a confidential classification? | Which Egeria-cataloged assets correspond to a live SQL table, and which are cataloged but missing? (sql, jena; data asset) | core-foundation-v1 |
| R018 | emerald-exchange | Which open orders on an exchange are partially filled and what is their remaining quantity? | Which exchange fills correspond to a deposit or withdrawal recorded in the Firefly III ledger? (firefly-iii, home-assistant; account / currency) | finance-v1 |
| R019 | erpnext | Which open purchase orders are late, and from which supplier? | Which ERPNext suppliers with late orders are also marked as active accounts in the Twenty CRM or paid in Firefly III? (twenty, firefly-iii; party (supplier / customer)) | commercial-v1 |
| R020 | fan-manager | Which fans ran above 80 percent duty while the host temperature stayed above its threshold? | Which hosts had fan duty at maximum during a window in which Grafana alerted on temperature? (systems-manager, lgtm; host) | infrastructure-v1 |
| R021 | firefly-iii | Which budgets were exceeded this month, and by which transactions? | Which Firefly III expenses match an ERPNext purchase invoice by amount and date? (erpnext, emerald-exchange; account / transaction) | finance-v1 |
| R022 | freshrss | Which unread articles from a given feed were published in the last 48 hours? | Which FreshRSS articles also appear in SearXNG results for the same query, and which are new? (archivebox, searxng; URL / article) | content-v1 |
| R023 | github | Which open pull requests have failing checks and no reviewer assigned? | Which GitHub pull requests close a Jira issue that is still in an open state? (gitlab, atlassian; repository / change request) | sdlc_lifecycle-v1 + software-v1 |
| R024 | gitlab | Which pipelines on the default branch failed in the last day, and at which job? | Which repositories exist in GitLab and GitHub with diverging default-branch heads? (github, repository-manager; repository / pipeline) | sdlc_lifecycle-v1 + software-v1 |
| R025 | gramps | Which people were born in a given place before a given year, and who are their parents? | Which Gramps people have a photo or document reference that exists as a file in Nextcloud? (nextcloud; person / media) | personal-v1 |
| R026 | home-assistant | Which sensors reported a value outside their normal range in the last 24 hours? | Which Home Assistant devices went unavailable at the same time as an Uptime Kuma monitor failure? (fan-manager, uptime; device / host) | infrastructure-v1 |
| R027 | jellyfin | Which movies in a library have never been played and were added more than a year ago? | Which Jellyfin items have no matching Radarr or Sonarr entry, and so cannot be re-fetched? (arr, qbittorrent; media item (title)) | media-v1 |
| R028 | jena | Which named graphs contain the most triples and which have no recent update? | Which Jena named graphs describe the same entities as an Egeria-cataloged asset? (egeria, sql; graph / dataset) | virtual_graph-v1 |
| R029 | kafka | Which consumer groups lag by more than a threshold on which topic partitions? | Which Kafka consumer groups with sustained lag belong to a service that Grafana is alerting on? (langfuse, lgtm; topic / service) | MISSING |
| R030 | keycloak | Which clients can a given user sign in to, and through which role? | Which people have access to a service that has an open security incident? (okta, servicenow; person / service) | identity-v1 |
| R031 | langfuse | Which traces had the highest token cost and which prompt version produced them? | Which prompt versions with falling evaluation scores were last changed by a merged GitHub pull request? (data-science, github; prompt / model) | orchestration-v1 |
| R032 | leanix | Which fact sheets of type application have lifecycle phase end-of-life but still have active users? | Which end-of-life LeanIX applications still have an active ServiceNow CI and open incidents? (archimate, servicenow; application) | enterprise-v1 |
| R033 | legal-peripherals | Which contracts expire within 90 days and have no renewal action recorded? | Which expiring contracts have a scanned copy in Paperless-ngx and a matching OneTrust vendor assessment? (paperless-ngx, onetrust; contract / document) | core-foundation-v1 |
| R034 | lgtm | Which alerts are firing right now, with what severity and for how long? | Which firing Grafana alerts have no open ServiceNow incident, and which incidents have no alert? (uptime, servicenow; service / alert) | infrastructure-v1 |
| R035 | listmonk | Which campaigns had an open rate below the list average, and to which lists were they sent? | Which subscribers on a Listmonk list have no matching Twenty contact, and which contacts are unsubscribed? (twenty, postiz; contact / audience) | media-v1 / content-v1 |
| R036 | mattermost | Which channels have had no posts in 90 days but still have more than ten members? | Which Mattermost members have no active identity-provider account, and which people lack a Mattermost account? (keycloak, okta; person) | media-v1 / content-v1 |
| R037 | mealie | Which recipes use an ingredient that is also on the current shopping list? | Which planned Mealie meals exceed the daily calorie target set in wger, and what did the groceries cost in Firefly III? (wger, firefly-iii; food / ingredient) | nutrition-v1 |
| R038 | media-downloader | Which download jobs failed in the last week, with what error, for which source URL? | Which completed downloads were never added to Jellyfin or Audiobookshelf? (audiobookshelf, jellyfin; media item / URL) | media-v1 |
| R039 | microsoft | Which meetings tomorrow overlap, and which attendees are double-booked? | Which attendees of an upcoming meeting have no identity-provider account or Mattermost presence? (okta, mattermost; person) | calendar-v1 |
| R040 | nextcloud | Which shared folders are accessible by external links and when do those shares expire? | Which publicly shared Nextcloud files also exist as Paperless-ngx documents tagged confidential? (keycloak, paperless-ngx; file / person) | personal-v1 |
| R041 | objectstore | Which buckets hold the most data and which objects have not been read in a year? | Which object-store artifacts are not referenced by any model or dataset in the data-science connector? (data-science, documentdb; dataset / artifact) | MISSING |
| R042 | okta | Which users hold a given application assignment through which group? | Which Okta users were deactivated but still hold a Keycloak role or an open ServiceNow assignment? (keycloak, servicenow; person / role) | identity-v1 |
| R043 | onetrust | Which privacy assessments are overdue and which processing activities do they cover? | Which vendors with an overdue OneTrust assessment have open ERPNext purchase orders? (ciso-assistant, erpnext; processing activity / vendor) | government-v1 |
| R044 | openbao | Which secret paths have not been rotated in 180 days, and which policies grant read on them? | Which services running in container-manager read a secret path that has no recent rotation? (keycloak, container-manager; secret / service) | MISSING |
| R045 | owncast | Which streams had the highest peak viewer count and how long did they run? | Which Owncast broadcasts were announced through a Postiz post, and what was the audience versus the post reach? (postiz, mattermost; broadcast / audience) | media-v1 / content-v1 |
| R046 | paperless-ngx | Which documents tagged as invoice have no correspondent or no due date? | Which Paperless-ngx invoices have no matching ERPNext purchase invoice or Firefly III expense? (erpnext, firefly-iii; document / invoice) | personal-v1 |
| R047 | plane | Which work items in the active cycle are overdue and which module do they belong to? | Which Plane work items link to a GitHub pull request that has merged but are not marked done? (github, atlassian; work item) | sdlc_lifecycle-v1 |
| R048 | portainer | Which stacks have services with fewer running replicas than desired, and on which endpoint? | Which Portainer stacks deploy from a Git repository whose latest GitLab pipeline failed? (container-manager, gitlab; stack / repository) | infrastructure-v1 |
| R049 | postiz | Which scheduled posts failed to publish and on which channel? | Which Postiz posts promote a campaign that Listmonk also sent, and which got the better engagement? (owncast, listmonk; campaign / channel) | media-v1 / content-v1 |
| R050 | pulselink | Which videos on a topic had transcripts mentioning a given term this week? | Which topics appear in PulseLink video transcripts and SearXNG news but not in any archived page? (searxng, audio-transcriber; topic / transcript) | media-v1 / content-v1 |
| R051 | qbittorrent | Which torrents are stalled, with what ratio and how long since last activity? | Which stalled torrents belong to a wanted Radarr or Sonarr title, and so block it from import? (arr, jellyfin; media item (torrent)) | media-v1 |
| R052 | repository-manager | Which worktrees are stale or dangling relative to their base branch? | Which local worktrees track a branch whose pull request has already been merged on GitHub or GitLab? (github, gitlab; repository / branch) | sdlc_lifecycle-v1 + software-v1 |
| R053 | rom-manager | Which ROMs in a platform collection fail a checksum match against the reference set? | Which ROMs were obtained by a media-downloader job and which are still unconverted? (media-downloader, jellyfin; game / file) | media-v1 |
| R054 | salesforce | Which open opportunities close this quarter, with what amount and owner? | Which Salesforce accounts have an open ServiceNow incident and an opportunity closing this quarter? (twenty, servicenow; account / customer) | commercial-v1 |
| R055 | scholarx | Which papers cite a given paper and were published in the last two years? | Which papers on a topic are in ScholarX but were never surfaced by a SearXNG query for that topic? (searxng, langfuse; paper / topic) | media-v1 / content-v1 |
| R056 | searxng | Which engines returned no results for a given query, and which returned the most? | Which URLs from a SearXNG query are not yet archived in ArchiveBox? (freshrss, archivebox; URL / query) | media-v1 / content-v1 |
| R057 | servicenow | Which incidents are open on a given configuration item and who is assigned? | Which open incidents on a CI also show a Grafana alert and a failing Uptime Kuma monitor? (okta, lgtm; configuration item / service) | archimate-v1 |
| R058 | sql | Which tables have no primary key, and which columns reference tables that do not exist? | Which SQL tables are not cataloged in Egeria, and which cataloged tables no longer exist? (egeria, documentdb; table / data asset) | MISSING |
| R059 | stirlingpdf | Which operations failed on which PDFs and with what error? | Which PDFs processed by Stirling-PDF were stored in Paperless-ngx or Nextcloud, and which were not? (paperless-ngx, nextcloud; document) | action-v1 |
| R060 | systems-manager | Which hosts have pending OS updates and what is their uptime? | Which hosts with pending updates have no Ansible Tower patch job scheduled and no ServiceNow change? (ansible-tower, servicenow; host (configuration item)) | infrastructure-v1 |
| R061 | technitium-dns | Which zones contain records pointing to an address that no host answers on? | Which DNS A records have no Caddy route and no Uptime Kuma monitor? (caddy, uptime; hostname) | infrastructure-v1 |
| R062 | tunnel-manager | Which SSH hosts in the inventory failed their last connection, and with what error? | Which hosts that tunnel-manager cannot reach are still listed as managed in Ansible Tower? (ansible-tower, systems-manager; host) | infrastructure-v1 |
| R063 | twenty | Which people at a company have no logged activity in the last 60 days while a deal is open? | Which Twenty contacts also exist as Salesforce contacts with different email addresses? (salesforce, listmonk; person / company) | commercial-v1 |
| R064 | uptime | Which monitored services were down in the last 24 hours and for how long? | Which services Uptime Kuma saw down had a Grafana alert or a Caddy route error at the same time? (lgtm, caddy; service / hostname) | infrastructure-v1 |
| R065 | vector | Which collections have the most vectors and which have an embedding dimension that mismatches the model? | Which Paperless-ngx documents have no embedding stored in the vector database? (data-science, paperless-ngx; embedding / document) | MISSING |
| R066 | wger | Which workout routines were not completed in the last two weeks and which exercises were skipped? | Which weeks of low workout completion coincide with a Mealie meal plan above the calorie target? (mealie, home-assistant; person / nutrition plan) | nutrition-v1 |

#### Coverage

| Connector | Domain | Vocabulary |
|---|---|---|
| ansible-tower | IT operations | infrastructure-v1.ttl |
| archimate | engineering | archimate-v1.ttl |
| archivebox | knowledge | documentation-v1.ttl |
| aris | governance | process_intelligence-v1.ttl |
| arr | media | MISSING |
| atlassian | engineering | sdlc_lifecycle-v1.ttl |
| audiobookshelf | media | MISSING |
| audio-transcriber | media | MISSING |
| caddy | IT operations | infrastructure-v1.ttl |
| camunda | business | process_intelligence-v1.ttl |
| ciso-assistant | governance | government-v1.ttl |
| clarity | data | MISSING |
| container-manager | IT operations | infrastructure-v1.ttl |
| data-science | data | MISSING |
| dockerhub | engineering | infrastructure-v1.ttl + software-v1.ttl |
| documentdb | data | MISSING |
| egeria | governance | core-foundation-v1.ttl |
| emerald-exchange | finance | finance-v1.ttl |
| erpnext | business | commercial-v1.ttl |
| fan-manager | home | infrastructure-v1.ttl |
| firefly-iii | finance | finance-v1.ttl |
| freshrss | knowledge | MISSING |
| github | engineering | sdlc_lifecycle-v1.ttl + software-v1.ttl |
| gitlab | engineering | sdlc_lifecycle-v1.ttl + software-v1.ttl |
| gramps | knowledge | personal-v1.ttl |
| home-assistant | home | infrastructure-v1.ttl |
| jellyfin | media | MISSING |
| jena | data | virtual_graph-v1.ttl |
| kafka | data | MISSING |
| keycloak | identity | identity-v1.ttl |
| langfuse | engineering | orchestration-v1.ttl |
| leanix | business | enterprise-v1.ttl |
| legal-peripherals | governance | core-foundation-v1.ttl |
| lgtm | IT operations | infrastructure-v1.ttl |
| listmonk | business | MISSING |
| mattermost | business | MISSING |
| mealie | home | nutrition-v1.ttl |
| media-downloader | media | MISSING |
| microsoft | business | calendar-v1.ttl |
| nextcloud | knowledge | personal-v1.ttl |
| objectstore | data | MISSING |
| okta | identity | identity-v1.ttl |
| onetrust | governance | government-v1.ttl |
| openbao | IT operations | MISSING |
| owncast | media | MISSING |
| paperless-ngx | knowledge | personal-v1.ttl |
| plane | engineering | sdlc_lifecycle-v1.ttl |
| portainer | IT operations | infrastructure-v1.ttl |
| postiz | business | MISSING |
| pulselink | knowledge | MISSING |
| qbittorrent | media | MISSING |
| repository-manager | engineering | sdlc_lifecycle-v1.ttl + software-v1.ttl |
| rom-manager | media | MISSING |
| salesforce | business | commercial-v1.ttl |
| scholarx | knowledge | MISSING |
| searxng | knowledge | MISSING |
| servicenow | IT operations | archimate-v1.ttl |
| sql | data | MISSING |
| stirlingpdf | knowledge | action-v1.ttl |
| systems-manager | IT operations | infrastructure-v1.ttl |
| technitium-dns | IT operations | infrastructure-v1.ttl |
| tunnel-manager | IT operations | infrastructure-v1.ttl |
| twenty | business | commercial-v1.ttl |
| uptime | IT operations | infrastructure-v1.ttl |
| vector | data | MISSING |
| wger | home | nutrition-v1.ttl |

Connectors with no matching vocabulary: **23** of 66 (arr, audiobookshelf, audio-transcriber, clarity, data-science, documentdb, freshrss, jellyfin, kafka, listmonk, mattermost, media-downloader, objectstore, openbao, owncast, postiz, pulselink, qbittorrent, rom-manager, scholarx, searxng, sql, vector).

### Vocabulary design

##### Architecture

This is a vocabulary correction. No new mechanism: answers still come from the engine's federated query over each connector's access contract (`specs/federated-query-and-reasoning`, `specs/unified-data-plane`), source authority from `native_source_connector-v1.ttl`, and virtual mappings from `virtual_graph-v1.ttl`. The requirements name the ontology terms that each competency question uses.

```
competency question -> vocabulary file (the coverage table) -> classes/properties
   -> virtual mapping (vg:mapsOntologyClass) -> connector access contract (MCP/API/GraphQL)
   -> fetch at query time -> provenance-carrying answer or typed abstention
```

Owners: ontology files and `schema_sources.rs` registration are Medulla (`crates/medulla-core`); connector-side mappings stay with the connector package (cross-repo, not in this spec).

##### Vocabulary

Terms to add, by file. Each is one child row (a 5-minute unit: one class or property, plus its SHACL shape where the constraint is implied). Prefixed names use the file's default prefix `:`; domains, ranges and shapes are in the child rows of `requirements.md`.

| File | Terms | Names |
|---|---|---|
| `infrastructure-v1.ttl` | 25 | `:AutomationJob`, `:AutomationJobTemplate`, `:jobStatus`, `:ranAgainstHost`, `:routeHostname`, `:upstreamAddress`, `:Alert`, `:alertSeverity`, `:alertState`, `:firingSince`, `:alertsOn`, `:lastPushedAt`, `:pullCount`, `:entityState`, `:unavailableSince`, `:desiredReplicas`, `:runningReplicas`, `:stackSourceRepository`, `:ManagementEnvironment`, `:pendingUpdateCount`, `:uptimeSeconds`, `:recordType`, `:recordValue`, `:lastConnectionStatus`, `:lastConnectionError` |
| `sdlc_lifecycle-v1.ttl` | 20 | `:branchOf`, `:headCommit`, `:isDefaultBranch`, `:blockedBy`, `:assignedTo`, `:ticketStatus`, `:closesTicket`, `:proposalState`, `:requestedReviewer`, `:PipelineJob`, `:pipelineJobOf`, `:IterationCycle`, `:inCycle`, `:WorkModule`, `:inModule`, `:ticketDueDate`, `:Worktree`, `:worktreeState`, `:tracksBranch`, `:proposalBranch` |
| `software-v1.ttl` | 2 | Existing canonical `:Repository`, `:Branch` classes reused by connector competency mappings; no duplicate declarations. |
| `archimate-v1.ttl` | 7 | `:ApplicationComponent`, `:ArchimateRelationship`, `:relationshipType`, `:ConfigurationItem`, `:affectsConfigurationItem`, `:incidentState`, `:incidentAssignee` |
| `documentation-v1.ttl` | 5 | `:WebSnapshot`, `:ArchiveResult`, `:hasArchiveResult`, `:extractorName`, `:resultStatus` |
| `process_intelligence-v1.ttl` | 7 | `:EpcFunction`, `:EpcRuleOperator`, `:flowsTo`, `:ProcessInstance`, `:instanceOfProcessModel`, `:stoppedAtActivity`, `:raisedIncident` |
| `government-v1.ttl` | 10 | `:RiskScenario`, `:mitigatesRisk`, `:inFramework`, `:riskAffectsService`, `:PrivacyAssessment`, `:ProcessingActivity`, `:assessmentCovers`, `:assessmentDueDate`, `:assessedVendor`, `:assessmentStatus` |
| `core-foundation-v1.ttl` | 5 | `:GlossaryTerm`, `:termAssignedTo`, `:counterparty`, `:agreementExpiresAt`, `:hasRenewalAction` |
| `finance-v1.ttl` | 6 | `:orderQty`, `:Budget`, `:budgetLimit`, `:LedgerTransaction`, `:chargedToBudget`, `:transactionAmount` |
| `personal-v1.ttl` | 9 | `:birthPlace`, `:birthYear`, `:documentType`, `:dueDate`, `:totalAmount`, `:FileShare`, `:sharesFile`, `:isPublicLink`, `:shareExpiresAt` |
| `virtual_graph-v1.ttl` | 3 | `:NamedGraph`, `:tripleCount`, `:lastModifiedAt` |
| `identity-v1.ttl` | 4 | `:Application`, `:OidcClient`, `:roleGrantsAccessTo`, `:accountStatus` |
| `orchestration-v1.ttl` | 4 | `:promptVersion`, `:tokenCost`, `:tracePrompt`, `:evaluationScore` |
| `enterprise-v1.ttl` | 3 | `:factSheetType`, `:lifecyclePhase`, `:activeUserCount` |
| `nutrition-v1.ttl` | 8 | `:Recipe`, `:usesIngredient`, `:ShoppingListItem`, `:MealPlanEntry`, `:WorkoutRoutine`, `:Exercise`, `:includesExercise`, `:WorkoutSession` |
| `action-v1.ttl` | 1 | `:failureReason` |
| `commercial-v1.ttl` | See `MEDULLA-SUPPLY-VOCAB-R003` | Commercial declarations belong to the supply-chain vocabulary. |

Only `finance-v1` has a `.shapes.ttl` companion today. Other files register
`shapes: None`. Each constrained file rollup has a first child. That child
creates and registers its companion. Shape children therefore have a home.

##### Wiring

- Ontology files: `crates/medulla-core/ontology/<file>.ttl`; registration: `CoreSpec` entries in `crates/medulla-graph-core/src/graph/schema_sources.rs` (precedent `governance-shapes`, `personal`, `government`).
- `catalog-v1.ttl` is generated; it is never hand-edited. The `commercial` module appears in it by regeneration.
- Connector mappings to the new terms are `vg:VirtualMapping` rows (`virtual_graph-v1.ttl`: `mapsOntologyClass`, `mapsEntity`, `hasPredicateFieldPair`); they belong to the connector lanes, not here.
- Reassignment rows change the Coverage table and `Vocabulary:` text in `specs/connector-competency` after their producer terms land. They carry exact row-level assertions in both documents.
- Scopes: none added; reads use the scopes the engine contract already lists.

##### Worked example

Question (home-assistant, single-source): "Which sensors reported a value outside their normal range in the last 24 hours?"

```sparql
PREFIX : <https://knuckles-team.github.io/ontology/infrastructure#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
SELECT ?entity ?signal ?observed ?p95 WHERE {
  VALUES ?cutoff { "2026-01-02T00:00:00Z"^^xsd:dateTime }
  ?entity a :Entity .
  ?a a :HealthAnomaly ; :affectsEntity ?entity ; :observed ?observed ; :observedAt ?t ; :signal ?signal .
  ?b a :HealthBaseline ; :affectsEntity ?entity ; :signal ?signal ; :p95 ?p95 .
  FILTER(?t >= ?cutoff)
}
```

The caller supplies the cutoff for a frozen as-of of 2026-01-03T00:00:00Z. Entity joins use the object-valued affectsEntity edge, not the string-valued entity property, and match the signal to avoid combining unrelated baselines.

The query uses terms declared in `infrastructure-v1.ttl`. Answer shape: rows of entity, observed value, baseline p95, with provenance (source, record id, as-of) from the virtual mapping, or a typed abstention naming the unconnected Home Assistant source.

##### Out of scope

The 23 MISSING connectors; connector-side mapping code; copying source data; the cross-source federation mechanism (existing specs).

### Vocabulary decisions and acceptance

Home Assistant uses `infrastructure-v1.ttl`; the trace-store connector uses `orchestration-v1.ttl`.
Ciso Assistant and OneTrust use `government-v1.ttl`.
OneTrust derives overdue status from assessmentDueDate and assessmentStatus; assessedVendor refers to the existing TRM Vendor class.
ServiceNow uses the shared kg Incident and Change anchors in `archimate-v1.ttl`.
GitHub, GitLab, and Repository Manager use canonical Repository and Branch classes from `software-v1.ttl` together with the lifecycle-specific Worktree and other terms in `sdlc_lifecycle-v1.ttl`.
DockerHub uses `infrastructure-v1.ttl` and the canonical `software-v1.ttl` Repository via `MEDULLA-CQ-CONN-R069.2`.
Egeria and Legal Peripherals use `core-foundation-v1.ttl`; the generated catalog remains term-free.
Gramps, Paperless, and Nextcloud use `personal-v1.ttl`.
StirlingPDF uses `action-v1.ttl`.
Erpnext, Salesforce, and Twenty use `commercial-v1.ttl`, owned by `MEDULLA-SUPPLY-VOCAB-R003`.
Commercial Party and Contact representations reuse Organization and Person under that owner's current requirements.

`MEDULLA-CQ-CONN-R083` requires exact commercial vocabulary cells, registered IRIs, and missing-producer refusal fixtures.
The fifteen noncommercial `.3` acceptance rows list exact producer dependencies in requirements.md.
Their tests check exact coverage and requirement cells, registered IRIs, and refusal fixtures.
Tests must resolve full IRIs from registered modules, including cross-module prerequisites.
Implementations must retain each producer's current representation, domain, range, and module ownership.
Connector packages must verify source payload fields and mappings before live competency acceptance.
Vocabulary declarations alone do not prove runtime competency.

### Connector-pack competency questions (R084)

Every connector ships an OWL/RDF ontology pack with its domain ontology, ingest contract and surface. The engine imports it as native relationships (`MEDULLA-TYPED-PACKS-R101`, `MEDULLA-TYPED-PACKS-R102`). R084 states the acceptance questions for that design. The engine answers each by SPARQL over imported packs. The method-selection question also runs through `Decide` (`MEDULLA-DECISION-ENGINE-R172.12`). Call path: `ConnectorPack.Import` → pack graph → `SourceIngest` → served SPARQL handler (`crates/medulla-h-sparql/src/rdf/sparql.rs:30`) and `handle_decide` (`crates/medulla-h-decide/src/decide/stat_decide.rs:50`).

Decision (R084.7): the pack requirement rides the one served read. `Method::Sparql` carries `requires_packs` (connector ids); the dispatch checks them against the tenant's visible pack heads before any query runs and refuses with the registered code `PACK_NOT_IMPORTED` naming the first missing id. Each question declares its packs in `requires_packs.txt` beside its query. No per-question operation exists.

### North star

One engine, medulla, holds all facts. There is no second database and no sync. External stores are virtual-graph sources reached through connectors. Every connector that connects through arbora onboards itself. Its ontology extends the core ontology. Its prompts become system-prompt candidates. Its skills, tools and resources appear as virtual graph entries in the engine. The exit gate is competency questions. The ontology answers how to execute a task and produces cross-source reports from connector virtual graphs.

### Delivery phase

Row `MEDULLA-CQ-CONN-R090` serves P5. It is the engine side of the federated read in `ARBORA-ACCEPTANCE-R006.7`.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row, producer first: the served harness and the vocabulary rows precede the question rows that read them. Question tasks of different connectors run in parallel because each owns its own case file and recording. Tasks that edit the same ontology module run in sequence.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Every ontology module that a row changes still loads through `GraphSchemaSources::default()` and passes its shapes check.
- A public item added by a row has a non-test caller under a handler or server crate (`MEDULLA-CONTRACT-R055`).
- Served tests use recorded fixtures and need no live credential or network.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.

## Production caller chain

- `Method::Uql` (`crates/medulla-protocol-types/src/protocol/method/method_05.rs:119`) to `dispatch_sql_query` (`crates/medulla-h-query/src/query/dispatch.rs:127`) to `handle_uql` (`crates/medulla-h-query/src/query/uql_statement.rs:17`) to `foreign_scan` (`crates/medulla-federation/src/federation_opt/run.rs:26`) to the connector contract.
- `Method::UnifiedQuery` (`method_05.rs:95`) to `dispatch_sql_query` to `run_unified_off_lock_with` (`crates/medulla-h-query/src/query/planning.rs:877`) to `Op::ForeignScan` (`crates/medulla-exec/src/exec/dispatch.rs:363`) to `FederatedExec::foreign_scan` (`crates/medulla-federation/src/federation_opt/federated.rs:51`).
- `ConnectorPackOp::Import` (`crates/medulla-h-admin/src/admin/connector_pack.rs:53`) to `serve_import` (:146) to `project` (`crates/medulla-h-admin/src/connector_pack_projection.rs:286`), which writes the pack and catalog nodes.
- Ontology terms: server bootstrap to `GraphSchemaSources::default()` (`crates/medulla-graph-core/src/graph/schema_sources.rs`) to the registered modules in `crates/medulla-core/ontology/`.
- Obsolete paths to delete: none; every row adds to an existing path.

## Cross-repo seams

- Connector access contracts and mappings: the connector repositories provide through their `CONN-<REPO>-CONNECTOR-CONTENT-R002` rows, `MEDULLA-CQ-CONN-R067` consumes. Contract: [connector-access-contracts.md](contracts/connector-access-contracts.md).
- Fleet onboarding handshake: `ARBORA-FLEET-R041.7` provides, `MEDULLA-CQ-CONN-R085` consumes. Contract: [arbora-fleet-onboarding.md](contracts/arbora-fleet-onboarding.md).
- Federated read through the served door: `MEDULLA-CQ-CONN-R090` provides, `ARBORA-ACCEPTANCE-R006.7` consumes. Contract: [arbora-federated-read.md](contracts/arbora-federated-read.md).
