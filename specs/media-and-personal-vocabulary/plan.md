# MEDULLA-MEDIA-VOCAB — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

Medulla owns one media vocabulary (`crates/medulla-core/ontology/media-v1.ttl`), one content-channel vocabulary (`content-v1.ttl`), their shapes, the fifteen connector mapping fixtures and the ingest and query surfaces for meeting transcripts, bookmarks, media tags, paper facts and media memory. The live caller of the registered vocabulary is `GraphSchemaSources::default()` in `crates/medulla-graph-core/src/graph/schema_sources.rs`; the ingest surfaces enter at `Method::SourceIngest` in `crates/medulla-h-ingest/src/source_ingestion.rs`. Reuse candidates found by searching the code first: the semantic index service of `crates/medulla-h-retrieve`, the relationship admission of `source_ingestion.rs`, the `Derivation` type of `crates/medulla-modality`, and the refusal-table pattern of `MEDULLA-DECISION-ENGINE-R010.100`.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain of `rust-toolchain.toml`; Turtle ontology files; Python 3.14 (floor 3.12) for the one served Python test.
- **Primary Dependencies**: `medulla-rdf` (parser and SPARQL), `medulla-shacl`, `medulla-graph-core`, the handler crates named in the rows, and the standard library.
- **Storage**: the engine graph store through the one `SourceIngest` ChangeEnvelope commit; no second store.
- **Testing**: ontology load tests in `crates/medulla-core/tests/media_vocab_a.rs` and `media_vocab_b.rs`, SHACL tests in `crates/medulla-rdf/tests/media_vocab_shapes.rs`, and the served tests of `crates/medulla-server-served-tests` (L3).
- **Target Platform**: Linux x86_64 and aarch64 hosts.
- **Project Type**: Library crates, ontology files and server handler crates.
- **Performance Goals**: the media memory write meets the nightly budget of row R650.6; vocabulary loading adds no measurable start-up cost.
- **Constraints**: no new wire method for tag search; denied rows never reach a count; every refusal has a stable code; no version suffix in a name.
- **Scale/Scope**: the leaf rows of `requirements.md`, spread over 7 user stories and 8 `FR-` labels.

**Knob convention.** An engine knob named `knob x.y` in this spec's rows is a typed field of the server configuration, read from the command line or from the environment variable `MEDULLA_<NAME>` (the dotted name upper-cased, dots as underscores) and bounds-checked at startup: a value outside the stated range refuses startup with the knob name. Decision knobs are the exception and live only in the `MEDULLA-DECISION-ENGINE-R179` registry. Rows do not restate this.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Every behaviour row lands in a Rust crate and the engine store.
- **R-II. Every wait has a deadline**: pass. No row adds a wait.
- **R-III. Every row sets budgets and names a benchmark**: pass. R650.6 names the only hot path and its nightly budget.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates; no handler crate calls a sibling handler crate.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries.

## Project Structure

```text
crates/medulla-core/ontology/media-v1.ttl, content-v1.ttl           existing
crates/medulla-core/ontology/media-v1.shapes.ttl, content-v1.shapes.ttl  existing
crates/medulla-core/tests/media_vocab_a.rs, media_vocab_b.rs         existing
crates/medulla-core/tests/fixtures/media_vocab/                      existing: 15 mapping and response pairs
crates/medulla-rdf/tests/media_vocab_shapes.rs, media_vocab_cross_source.rs  existing
crates/medulla-h-ingest/src/source_ingestion/media_tag.rs            new
crates/medulla-h-ingest/src/meeting_transcript/                      new
crates/medulla-h-ingest/src/paper_ingest/                            new
crates/medulla-h-retrieve/src/bookmark.rs                            new
crates/medulla-h-query/src/bookmark_relations.rs                     new
crates/medulla-h-graph/src/graph_ops/memory_surface.rs               new (owned by the decision-engine spec)
crates/medulla-modality/src/media_memory.rs                          new
crates/medulla-types/src/{meeting_transcript,bookmark_query,media_tag,paper_ingest}.rs  new
crates/medulla-server-served-tests/src/split_tests/                  new served test files named by the rows
```

## Phase 0: Research

No unknowns. Every caveat is CONFIRMED, DECIDED or BLOCKED in the Decisions section; blocked items have a cross-repo row.

## Phase 1: Design

### Problem and evidence

`specs/connector-competency` marked the vocabulary for the whole media and personal-content domain MISSING because its method judged `crates/medulla-core/ontology/` by file name. This spec re-checked by opening the files.

- Listing: 41 files; none named for media, books, feeds, search, social posting, mailing or chat. Opened candidates: `personal-v1.ttl` (CalendarEvent, PersonalTask, VoiceMessage ⊑ `:CreativeWork`, Transcript ⊑ `:Document`, `transcribedFrom`, `transcriptionConfidence`), `core-foundation-v1.ttl` (`:CreativeWork`, `:Document`, `:Event`, `:Episode` = a discrete interaction unit, `:Person`), `documentation-v1.ttl` (documentation pages and revisions only), `orchestration-v1.ttl` (`:Session`, `:AgentMessage`, `:BusMessage` = agent traffic, not human chat), `software-v1.ttl` (`:Author` = git author, `:File` = source file), `native_source_connector-v1.ttl` (generated `:SourceDocument`), `virtual_graph-v1.ttl` (SourceConnection, MetadataContract, VirtualMapping), `company-v1.ttl` (names the qbittorrent specialist agent only). `grep -ciE "movie|album|torrent|podcast|subscriber|campaign"` finds only agent-roster labels in company-v1.ttl.
- Result: the gap is confirmed. Reusable terms include `:Team` as well as `:CreativeWork`, `:Document`, `:Transcript`, `:VoiceMessage`, `:Event`, `:Session`, `:Person`, `:Organization`, `:Tag` are reusable.
- New evidence: every connector package already ships a packaged vocabulary (`<pkg>/ontology/*.ttl`: arr.ttl, media.ttl, audiobookshelf.ttl, qbittorrent.ttl, rom.ttl, owncast.ttl, audio.ttl, feed.ttl, scholarx.ttl, searxng.ttl, pulselink.ttl, social.ttl, listmonk.ttl, mattermost.ttl), a `mappings/source.yaml`, and `connectors/mcp_source_presets.json`. Those declare connector-local classes in the shared `:` namespace, two of which collide with core (`:Episode` in arr.ttl vs core-foundation `:Episode`; `:Author` in jellyfin/audiobookshelf media.ttl vs software-v1 `:Author`). What is missing in Medulla is the shared layer they all subclass, and the cross-source identity keys.

### Architecture

Virtual-first: Medulla never copies a source. The core vocabulary is the join surface. Each connector's packaged ontology and mapping declare which tool fetches which class at query time.

```
question
  -> ontology (media-v1 / content-v1 classes + identity keys)
  -> source selection (decision engine / federated planner: specs/decision-engine, specs/federated-query-and-reasoning)
  -> vg:VirtualMapping per connector (vg:sourceKind "mcp", vg:mapsOntologyClass, vg:keyField)
  -> contract fetch (MCP tool call with action + params at query time)
  -> join on identity key (mv:tmdbId / mv:infoHash / cv:canonicalUrl) -> answer with provenance, or typed abstention naming the missing operation
```

Owners: vocabulary files and shapes: Medulla (`crates/medulla-core/ontology/`). Mapping rows: Medulla (fixtures) consuming `vg:` terms already in `virtual_graph-v1.ttl`. Operations and connector ontologies: each connector package (agent-packages). Planning, routing, federation: existing specs, cited, not redefined. Packs/agent library: `specs/typed-packs-and-catalog`.

### Vocabulary

Two new files, namespace `https://knuckles-team.github.io/ontology/core#`, prefixes `mv:` (media) and `cv:` (content channels) denote the section of the shared namespace. All IRIs stay in `:` as the other ontology files do, so `mv:`/`cv:` below are documentation prefixes.

#### media-v1.ttl

| Term | Kind | Domain / range / parent | Notes |
|---|---|---|---|
| MediaWork | class | ⊑ `:CreativeWork` | root of media works |
| Film, TelevisionSeries, TelevisionSeason, TelevisionEpisode | class | ⊑ MediaWork; Season partOf Series, Episode partOf Season | `TelevisionEpisode` avoids core `:Episode` |
| MusicArtist | class | ⊑ `:Organization` | as `:Family` in personal-v1 |
| MusicAlbum, MusicTrack | class | ⊑ MediaWork; Track partOf Album; Album by MusicArtist | |
| Audiobook, PodcastShow, PodcastEpisode | class | ⊑ MediaWork | `isbn` on Audiobook |
| Game | class | ⊑ MediaWork; `onPlatform` -> GamePlatform | covers ROM titles |
| GamePlatform, MediaLibrary | class | | |
| LibraryItem | class | `itemOf` -> MediaWork, `inLibrary` -> MediaLibrary, `addedAt` | one work in one library |
| MediaFile, RomFile | class | RomFile ⊑ MediaFile; `container`, `mediaFileSizeBytes` (under shared `sizeBytes`), `contentSha256`, `md5`, `sha1`, `crc32` | |
| PlaybackSession | class | ⊑ `:Session`; `playedBy` -> `:Person`, `itemOf`, `lastPlayedAt`, `playCount`, `playbackPositionSeconds` | live and historical |
| DownloadRequest | class | `requestsWork` -> MediaWork; `downloadRequestState` closed set; `requestedBy` -> `:Person` | wanted/requested |
| DownloadTransfer | class | `transferOf` -> DownloadRequest or MediaWork; `infoHash`, `transferState`, `shareRatio`, `lastActivityAt`, `addedAt` | torrent or job |
| GrabEvent, MediaIndexer | class | `grabbedFromIndexer` -> MediaIndexer | |
| BroadcastStream | class | ⊑ `:Event`; `lastConnectAt`, `streamOnline`; ViewerSample (`sampledAt`, `viewerCount`) | Owncast |
| TranscriptSegment | class | partOf the existing `:Transcript` (reused); `segmentStart`, `segmentEnd`, `noSpeechProb`, `transcriptOf` | no redefinition of Transcript |
| tmdbId, tvdbId, imdbId, musicBrainzId, isbn, infoHash, contentSha256 | datatype props | class-scoped correlation keys, not globally inverse-functional | scope provider IDs by work class; distinct source records retain their own identity |
| contributor | object prop | MediaWork -> `:Person` (role as annotation) | replaces connector `:Author` |

SHACL (media-v1.shapes.ttl, aggregate-free like virtual_graph-v1.shapes.ttl): connector-sourced Film/Series carry one of the three ids. `infoHash` is 40 or 64 lower-case hex; `downloadRequestState` in (requested, wanted, grabbed, importing, imported, failed); a LibraryItem has exactly one `itemOf` and one `inLibrary`.

#### content-v1.ttl

| Term | Kind | Domain / range / parent | Notes |
|---|---|---|---|
| Feed, FeedSubscription | class | subscription -> feed; `feedUrl` | FreshRSS |
| Article | class | ⊑ `:Document`; `publishedAt`, `isRead`, `canonicalUrl` | |
| ResearchPaper | class | ⊑ `:Document`, seeAlso bibo:AcademicArticle; `doi`, `arxivId`, `citationCount` | ScholarX |
| SearchQuery, SearchResult, SearchEngine | class | result -> query; `resultUrl`, `rank`, `servedByEngine` | SearXNG, PulseLink |
| SocialPost, PublishingChannel, EngagementMetric | class | post -> channel; `postState`, `socialScheduledAt`, `publishedAt` | Postiz |
| EmailCampaign, MailingList, Subscriber | class | campaign -> lists; `sentCount`, `viewCount`, `clickCount`, `subscriptionStatus`, `subscriberKey` | Listmonk |
| MessageChannel, ChatMessage; existing kg:Team | class (Team reused without redeclaration) | message -> channel; `memberCount`, `lastPostAt`, `postedAt`, `postedBy` -> `:Person` | Mattermost, Owncast chat |
| canonicalUrl | datatype prop | class-scoped document correlation key | Article identity uses the normalized document URL. SearchResult identity is scoped to its query and native result key. Posts retain source-native identity |

Shared-namespace collision decisions: reuse `kg:Team`. Use `kg:socialScheduledAt` for a social-post timestamp because `kg:scheduledFor` already links CalendarEvent to Person. Use `kg:downloadRequestState` because `kg:requestState` already has TRMRequest domain. No global URL or provider-ID inverse-functional axioms are introduced. A URL join does not create `owl:sameAs` between a document and the search results or posts that reference it.

SHACL: Article/SearchResult carry exactly one normalised `canonicalUrl`; Subscriber has `subscriberKey` (keyed hash) and no raw-address property.

### Wiring

- Ontology files are read from `crates/medulla-core/ontology/*.ttl` by the existing loader (same as `personal-v1.ttl`); shapes live beside them as `*.shapes.ttl`.
- Mappings are `vg:VirtualMapping` rows (`vg:mapsOntologyClass`, `vg:mapsEntity`, `vg:keyField`, `vg:hasPredicateFieldPair`) validated by `virtual_graph-v1.shapes.ttl`; `vg:sourceKind` is `mcp` for all 15 connectors (the closed set is api, mcp, a2a, graphql, sql, iceberg, warehouse, sparql). `api` was the alternative; rejected because every connector already publishes tool schemas and fingerprints for MCP, and governance (default_acl quarantine, identity opaque-reference) is declared there.
- Source selection and join planning use `specs/decision-engine` and `specs/federated-query-and-reasoning`; scopes are those the engine contract already lists.
- Connector truth: each `<pkg>/ontology/mappings/source.yaml` and `connectors/mcp_source_presets.json` is the connector-side declaration; Medulla mappings must agree with them (test R-row .1).
- `specs/connector-competency` vocabulary column flips from MISSING after landing (row .4 of each connector rollup).

### Connector mapping tables

#### arr (arr-mcp; `MEDULLA-CQ-CONN-R005`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| radarr_action/get_movie, sonarr_action/get_series, get_episode | mv:Film, mv:TelevisionSeries, mv:TelevisionEpisode. Mv:tmdbId, mv:tvdbId, mv:monitored, mv:hasFile | mcp |
| radarr_action/get_wanted_missing, sonarr_action/get_wanted_missing | mv:DownloadRequest (state wanted) -> mv:requestsWork | mcp |
| radarr_action/get_queue, sonarr_action/get_queue | mv:DownloadTransfer; mv:infoHash (queue downloadId) -> mv:transferOf | mcp |
| radarr_action/get_history, get_history_movie; sonarr_action/get_history_series | mv:GrabEvent -> mv:grabbedFromIndexer -> mv:MediaIndexer | mcp |
| lidarr_action/get_album, get_track, get_wanted_missing | mv:MusicAlbum, mv:MusicTrack, mv:MusicArtist | mcp |
| prowlarr_action/get_indexer, get_indexerstats | mv:MediaIndexer; mv:indexerEnabled | mcp |
| seerr_action/get_request | mv:DownloadRequest (state requested); mv:requestedBy -> :Person | mcp |

- Single-source question: CONFIRMED for wanted-without-file (get_wanted_missing, api_client_radarr.py / api_client_sonarr.py). DECIDED for the indexer clause: answer 'indexer that last GRABBED it' from get_history (eventType grabbed carries the indexer). Alternative of a true 'last searched' indexer is not recorded per title by Radarr or Sonarr.
- Cross-source question: CONFIRMED: queue exposes the download client id (get_queue) which is the torrent info-hash. Qbittorrent hash and jellyfin provider ids supply the other two legs. DECIDED: join key is mv:infoHash then mv:tmdbId.
- Cross-repo row: arr-mcp (per-title 'last searched indexer' (Radarr/Sonarr keep no per-title search log; Prowlarr history is query-keyed, not title-keyed): add a prowlarr_action history-by-title projection)

#### jellyfin (jellyfin-mcp; `MEDULLA-CQ-CONN-R027`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| jellyfin_library/get_items (Fields=ProviderIds,DateCreated) | mv:LibraryItem, mv:Film, mv:TelevisionEpisode, mv:MusicTrack; mv:tmdbId, mv:addedAt | mcp |
| jellyfin_library/get_item_user_data, get_resume_items | mv:PlaybackSession; mv:playCount, mv:lastPlayedAt, mv:playbackPositionSeconds | mcp |
| jellyfin_system/get_sessions | mv:PlaybackSession (live); mv:playedBy -> :Person | mcp |
| jellyfin_media/get_playback_info | mv:MediaFile; mv:container, mv:sizeBytes | mcp |
| jellyfin_library/get_item_counts | mv:MediaLibrary count | mcp |

- Single-source question: CONFIRMED: get_items and get_item_user_data exist in the jellyfin api client (get_items, get_item_user_data, get_resume_items); never-played-and-older-than-one-year is a filter on lastPlayedAt unbound plus addedAt (DateCreated is the preset updated_field).
- Cross-source question: DECIDED: the join to Radarr/Sonarr needs ProviderIds, which get_items returns only when asked via its Fields parameter. The mapping fixes Fields=ProviderIds. Alternative (title-and-year fuzzy join) rejected as unsound.

#### audiobookshelf (audiobookshelf-mcp; `MEDULLA-CQ-CONN-R007`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| library_operations/list, get_library_items | mv:MediaLibrary, mv:Audiobook; mv:isbn | mcp |
| series_operations/get_library_series, get_series | mv:BookSeries (mv:TelevisionSeries sibling under mv:SeriesWork); mv:seriesSequence | mcp |
| author_operations/get_library_authors, get_author_by_id | mv:contributor -> :Person (role author) | mcp |
| podcast_operations/* | mv:PodcastShow, mv:PodcastEpisode | mcp |
| notification_operations / email_operations | out of scope (configuration) | - |

- Single-source question: BLOCKED: the connector's api client exposes libraries, items, series and authors only. There is no per-user listening-progress operation (no /me/progress or user media-progress call in audiobookshelf_mcp/api). The question 'stopped partway for which user' cannot be answered. Owner audiobookshelf-mcp, row below.
- Cross-source question: BLOCKED for the media-downloader leg (see media-downloader). DECIDED for the audiobookshelf leg and the Jellyfin leg: both answer via mv:Audiobook plus mv:isbn / normalised title+author key. The answer abstains naming media-downloader until its list operation exists.
- Cross-repo row: audiobookshelf-mcp (user listening progress: add a library_operations or users action for media progress per user and item)
- Cross-repo row: media-downloader (see media-downloader B1)

#### qbittorrent (qbittorrent-agent; `MEDULLA-CQ-CONN-R051`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| qbittorrent_torrents/get_torrent_list | mv:DownloadTransfer. Mv:infoHash (hash), mv:transferState (state), mv:shareRatio (ratio), mv:lastActivityAt (last_activity), mv:addedAt (added_on) | mcp |
| qbittorrent_torrents/get_torrent_properties, get_torrent_trackers | mv:DownloadTransfer detail; mv:trackerUrl | mcp |
| qbittorrent_transfer/* | mv:TransferSpeedSample (global) | mcp |
| qbittorrent_sync/* | delta of torrent list (incremental fetch) | mcp |
| qbittorrent_rss / qbittorrent_search / qbittorrent_log / qbittorrent_app | out of scope | - |

- Single-source question: CONFIRMED: state, ratio, added_on and last_activity are fields of the torrent list (qbittorrent_models.py, mcp_torrents.py); 'stalled' is state stalledDL or stalledUP.
- Cross-source question: CONFIRMED with arr: the join is mv:infoHash against the Radarr/Sonarr queue entry's downloadId (case-folded). DECIDED: a stalled torrent with no arr queue entry is reported separately as unmanaged.

#### rom-manager (rom-manager; `MEDULLA-CQ-CONN-R053`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| romm_roms/list, get_rom, get_rom_by_hash | mv:Game, mv:RomFile. Mv:md5, mv:sha1, mv:crc32, mv:fileName | mcp |
| romm_platforms/list, get_platform | mv:GamePlatform | mcp |
| rom_game_codes/* | mv:Game; mv:gameCode | mcp |
| rom_conversion/convert_rom (write) | not read-mapped; effect only | - |

- Single-source question: BLOCKED: RomM is queried by hash (get_rom_by_hash) and lists hashes, but there is no operation that compares a library against a reference (DAT) set and reports mismatches. DECIDED interim: report ROMs whose metadata-provider match is absent (get_rom_by_metadata_provider) as 'unidentified', labelled as not a checksum verdict. Owner rom-manager for the verify operation.
- Cross-source question: BLOCKED: no media-downloader job history to say which ROM came from a download (media-downloader B1). The 'unconverted' leg is CONFIRMED only if conversion state is a rom field. DECIDED to infer it from file extension against rom_conversion target formats.
- Cross-repo row: rom-manager (reference-set verification: add a romm_roms verify action returning per-ROM checksum verdict)
- Cross-repo row: media-downloader (see media-downloader B1)

#### owncast (owncast-agent; `MEDULLA-CQ-CONN-R045`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| owncast_internal/get_status, owncast_external/external_get_status | mv:BroadcastStream; mv:streamOnline, mv:lastConnectAt (lastConnectTime), mv:viewerCount | mcp |
| owncast_internal/get_viewers_over_time, get_active_viewers | mv:ViewerSample; mv:sampledAt, mv:viewerCount | mcp |
| owncast_internal/get_hardware_stats | mv:HardwareSample | mcp |
| owncast_chat/get_chat_messages | mv:ChatMessage (content ⊑ :Message) | mcp |
| owncast_internal/get_followers | mv:Follower -> :Person | mcp |

- Single-source question: BLOCKED for 'highest peak across streams': the connector exposes the live status and a viewers-over-time series for the current window only (owncast_models.py status.lastConnectTime; get_viewers_over_time), with no list of past broadcasts. DECIDED: answer 'current or most recent broadcast peak and duration' now; historical ranking needs an owncast-agent stream-history operation.
- Cross-source question: BLOCKED: broadcast-to-post link is not an Owncast field. DECIDED: join via the stream URL appearing in the Postiz post content (mv:canonicalUrl). Audience from ViewerSample, reach from postiz_analytics. History limitation applies.
- Cross-repo row: owncast-agent (past broadcast list with start/end and peak viewers: add an owncast_internal stream-history action or persist sessions)

#### media-downloader (media-downloader; `MEDULLA-CQ-CONN-R038`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| download_media, download_video, download_audio, download_directory (write) | mv:DownloadRequest (requested), result path -> mv:MediaFile | mcp |
| list_watch_skills / watch_media | mv:MediaFile (watch preset) | mcp |

- Single-source question: BLOCKED: media-downloader exposes only download actions and watch prompts; there is no operation that lists past jobs, their status or error (media_downloader/*.py has only download_* and watch_media). The question 'failed jobs last week with error and source URL' cannot be answered. Owner media-downloader.
- Cross-source question: BLOCKED for the same reason: 'completed downloads never added to Jellyfin or Audiobookshelf' needs the completed-download list. Jellyfin and Audiobookshelf legs are CONFIRMED.
- Cross-repo row: media-downloader (download history: add a list_downloads operation returning job id, source URL, status, error, completedAt, output path)

#### audio-transcriber (audio-transcriber; `MEDULLA-CQ-CONN-R008`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| transcribe_audio / transcribe_media (produce transcript on demand) | mv:Transcript (reuse personal-v1), mv:TranscriptSegment. Mv:segmentStart, mv:segmentEnd, mv:noSpeechProb | mcp |
| audio_ingest_transcription | same, plus blob occurrence | mcp |

- Single-source question: BLOCKED: (a) the connector transcribes on request and has no list/get operation for past transcripts. (b) segments carry noSpeechProb but not avg_logprob / confidence (kg_ingest.py segment dict), so 'low confidence' is only approximable. DECIDED: low confidence := noSpeechProb above a threshold, recorded as a proxy. Owner audio-transcriber for list/get and a confidence field.
- Cross-source question: DECIDED: virtual cross-source with PulseLink (pulse_transcribe) and Paperless works at the mv:Transcript text level. The audio-transcriber leg abstains until list/get exists.
- Cross-repo row: audio-transcriber (transcript list/get operation by recording id and date)
- Cross-repo row: audio-transcriber (expose avg_logprob (confidence) per segment)

#### freshrss (freshrss-agent; `MEDULLA-CQ-CONN-R022`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| freshrss_reader/stream_contents (newer_than, continuation) | cv:Article ⊑ :Document; cv:publishedAt, cv:canonicalUrl, cv:isRead | mcp |
| freshrss_reader/unread_count, item_contents | cv:Feed unread count; article body | mcp |
| freshrss_subscriptions/subscription_list | cv:FeedSubscription -> cv:Feed; cv:feedUrl | mcp |
| mark_read (write) | effect only | - |

- Single-source question: CONFIRMED: stream_contents with newer_than and unread filter, plus unread_count (freshrss_reader docstring, mcp_freshrss reader tool). Published is the preset updated_field.
- Cross-source question: CONFIRMED: join on cv:canonicalUrl (normalised URL) with SearXNG result urls. 'new' means absent from the earlier fetch window, DECIDED as publishedAt within the question window.

#### scholarx (scholarx; `MEDULLA-CQ-CONN-R055`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| sx_search/search, get_paper, recent, author | cv:ResearchPaper ⊑ :Document; cv:doi, cv:arxivId, cv:publishedAt, cv:citationCount | mcp |
| sx_info/* | cv:ResearchCategory (:Tag), cv:PaperSource | mcp |
| sx_storage/* | cv:ResearchPaper; local stored copy (mv:RomFile-like file) | mcp |

- Single-source question: BLOCKED: only citation COUNT is available (_fill_citation_count in the scholarx source). No operation returns the list of papers citing a given paper. Owner scholarx.
- Cross-source question: CONFIRMED: sx_search for the topic and searxng web_search for the same query. Join on cv:doi / cv:arxivId extracted from result URLs, else cv:canonicalUrl.
- Cross-repo row: scholarx (citing-papers operation: add sx_search action 'citations' returning papers that cite a given id)

#### searxng (searxng-mcp; `MEDULLA-CQ-CONN-R056`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| web_search | cv:SearchQuery, cv:SearchResult; cv:resultUrl, cv:rank, cv:servedByEngine -> cv:SearchEngine | mcp |
| searxng_settings (get/set) | cv:SearchEngine enabled set | mcp |

- Single-source question: BLOCKED for 'engines that returned no results': web_search returns result rows with their engines, so engines with ZERO results are invisible (no unresponsive-engines or per-engine count field is passed through). Most-results-per-engine is CONFIRMED by counting servedByEngine. Owner searxng-mcp for per-engine counts / unresponsive engines.
- Cross-source question: CONFIRMED: SearXNG URLs against ArchiveBox snapshots by cv:canonicalUrl (archivebox is covered by documentation-v1).
- Cross-repo row: searxng-mcp (pass through per-engine result counts and unresponsive_engines from the SearXNG response)

#### pulselink (pulselink-mcp; `MEDULLA-CQ-CONN-R050`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| pulse_search (source=youtube|web|hackernews|...) | cv:SearchResult / mv:LibraryItem (video); cv:canonicalUrl, cv:publishedAt | mcp |
| pulse_fetch, pulse_list | cv:Article body or listing | mcp |
| pulse_transcribe | mv:Transcript for a video; mv:transcriptOf | mcp |
| pulse_status | source backend readiness (:SourceBackend) | mcp |

- Single-source question: CONFIRMED: pulse_search finds videos by topic and pulse_transcribe returns the transcript on demand (pulselink_mcp mcp tools). DECIDED: term matching is applied to transcripts fetched for at most N search hits (bounded fan-out) because transcripts are produced per call, not stored.
- Cross-source question: CONFIRMED legs: pulse_transcribe, searxng web_search, archivebox. DECIDED: topic is a :Concept/:Tag; archived-page absence is by cv:canonicalUrl.

#### postiz (postiz-agent; `MEDULLA-CQ-CONN-R049`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| postiz_posts/postiz_list_posts | cv:SocialPost; cv:postState, cv:socialScheduledAt, cv:publishedAt -> cv:PublishingChannel | mcp |
| postiz_integrations/postiz_list_integrations | cv:PublishingChannel; cv:platform | mcp |
| postiz_analytics/postiz_get_analytics, postiz_get_post_analytics | cv:EngagementMetric (impressions, likes) | mcp |
| postiz_notifications/postiz_list_notifications | notification feed | mcp |
| postiz_uploads, postiz_video | effect only | - |

- Single-source question: CONFIRMED op: postiz_list_posts. DECIDED: failed state is read from the post's state field and verified by the fixture; no separate failures operation is needed.
- Cross-source question: DECIDED: there is no campaign id shared by Postiz and Listmonk. The join is a shared cv:canonicalUrl (tracked link or landing URL) found in post content and campaign body. Alternative (explicit campaign tag) rejected: needs both connectors to agree on a field.

#### listmonk (listmonk-api; `MEDULLA-CQ-CONN-R035`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| listmonk_campaigns/get_campaigns, get_campaign, get_campaign_stats | cv:EmailCampaign. Cv:sentCount, cv:viewCount, cv:clickCount, cv:campaignStatus -> cv:MailingList | mcp |
| listmonk_lists/get_lists, get_list | cv:MailingList | mcp |
| listmonk_subscribers/get_subscribers, get_subscribers_from_list | cv:Subscriber; cv:subscriptionStatus; identity by keyed hash of email | mcp |
| listmonk_templates / listmonk_tx / listmonk_media / listmonk_imports | templates, transactional send, media, import: effect or out of scope | - |

- Single-source question: CONFIRMED: get_campaigns and get_campaign_stats exist (listmonk_api/api). Open rate := viewCount / sentCount, DECIDED as the definition. List average computed at query time.
- Cross-source question: DECIDED: subscribers to Twenty contacts join on a keyed hash of the email address, never the raw address, because the connector's governance is identity: opaque-reference.

#### mattermost (mattermost-mcp; `MEDULLA-CQ-CONN-R036`)

| Connector operation / tool | Class / property | Contract kind |
|---|---|---|
| mattermost_mcp_channels (get channel, channel stats, members) | cv:MessageChannel; cv:memberCount, cv:lastPostAt | mcp |
| mattermost_mcp_posts (get posts for channel) | cv:ChatMessage ⊑ :Message-like; cv:postedAt, cv:postedBy -> :Person | mcp |
| mattermost_mcp_teams / users | existing kg:Team (reuse), :Person | mcp |
| mattermost_mcp_files / reactions / webhooks / admin groups | out of scope | - |

- Single-source question: CONFIRMED: channels, channel stats and posts-for-channel are generated from the Mattermost API (mattermost_mcp/mcp/mcp_posts.py, mcp_channels). DECIDED: inactivity := lastPostAt older than 90 days, member count from channel stats.
- Cross-source question: DECIDED: join to Keycloak and Okta on a keyed hash of the email / username identity. Absence of an IdP account is a typed absence, not an error.

### Decisions

Every caveat is resolved as CONFIRMED, DECIDED or BLOCKED. The per-connector decisions are in the tables above; summary:

| Connector | Question 1 | Question 2 | Cross-repo rows |
|---|---|---|---|
| arr | DECIDED (indexer that last grabbed) | CONFIRMED | arr-mcp: per-title search log |
| jellyfin | CONFIRMED | DECIDED (Fields=ProviderIds) | none |
| audiobookshelf | BLOCKED (listening progress) | partly BLOCKED | audiobookshelf-mcp progress; media-downloader list |
| qbittorrent | CONFIRMED | CONFIRMED | none |
| rom-manager | BLOCKED (reference-set verify) | BLOCKED (download provenance) | rom-manager verify; media-downloader list |
| owncast | BLOCKED (past streams) | BLOCKED | owncast-agent stream history |
| media-downloader | BLOCKED (job history) | BLOCKED | media-downloader list_downloads |
| audio-transcriber | BLOCKED (list/get, confidence) | DECIDED | audio-transcriber list/get, avg_logprob |
| freshrss | CONFIRMED | CONFIRMED | none |
| scholarx | BLOCKED (citing papers) | CONFIRMED | scholarx citations |
| searxng | BLOCKED for zero-result engines | CONFIRMED | searxng-mcp per-engine counts |
| pulselink | CONFIRMED (bounded fan-out) | CONFIRMED | none |
| postiz | CONFIRMED op, state verified by fixture | DECIDED (shared URL) | none |
| listmonk | CONFIRMED | DECIDED (keyed email hash) | none |
| mattermost | CONFIRMED | DECIDED (keyed identity hash) | none |

Other decisions: D1 reuse `:Transcript`, `:VoiceMessage`, `:CreativeWork`, `:Document`, `:Session`, `:Event`, `:Person`, `:Tag` from existing files; do not redefine. D2 `TelevisionEpisode` not `:Episode` (collision, alternative: rename core `:Episode`, rejected, it is used by the memory model). D3 two files, not one, because the domains share no classes and ship on different cadences. D4 materialisation is not the default; the accelerated copy remains the federated-query spec's option.

### Worked examples

#### Example 1: Which requested films are downloaded but not yet in the library?

Legs: arr (Radarr `get_wanted_missing` plus Seerr `get_request` for requested; `get_queue` for in-flight), qbittorrent (`get_torrent_list`), jellyfin (`get_items` with ProviderIds).

```sparql
SELECT ?film ?tmdb ?hash WHERE {
  ?req a :DownloadRequest ; :requestsWork ?film ; :downloadRequestState ?s . FILTER(?s IN ("requested","wanted","grabbed","importing"))
  ?film a :Film ; :tmdbId ?tmdb .
  ?xfer a :DownloadTransfer ; :transferOf ?req ; :infoHash ?hash ; :transferState ?st . FILTER(?st IN ("uploading","stalledUP","pausedUP","queuedUP","forcedUP"))
  OPTIONAL { ?li a :LibraryItem ; :itemOf ?lf . ?lf :tmdbId ?tmdb . } FILTER(!BOUND(?li))
}
```

Flow: arr rows give ?film/?tmdb and the queue downloadId (= ?hash). Qbittorrent rows are matched on ?hash and filtered to completed states. Jellyfin rows supply library membership by ?tmdb. The anti-join (OPTIONAL plus `!BOUND`, the aggregate-free form) leaves films downloaded but absent. Answer shape: rows of (film title, tmdbId, torrent hash, transferState, completedAt) each with provenance (source, record id, as-of) per leg. If Jellyfin is unconnected the answer is a typed abstention naming jellyfin. No row is guessed.

#### Example 2: Which feed articles are not archived and were not returned for a search?

freshrss `stream_contents` (unread, window), searxng `web_search`, archivebox snapshots. Join on `cv:canonicalUrl`; answer is the articles with no matching result or snapshot. Same shape and abstention rule.

### Out of scope

Writing connector operations (owned by the connector packages, tracked as cross-repo rows). Bulk ingestion of media metadata (virtual-first). ArchiveBox, Paperless, Nextcloud, Gramps, Mealie vocabularies (documentation-v1, personal-v1, life-v1 or other specs). Plans, routing, topology (decision-engine).

### Quality and release gates

Vocabulary and shapes land with `cargo test -p medulla-core` ontology load and SHACL tests, the repo's CCCC, jscpd, dupehound and KISS gates. No duplicate class IRIs across `ontology/*.ttl` except the documented connector aliases. No scanner gaming.

### Risks, alternatives, and decisions

Connector ontologies already exist and may drift from the core vocabulary: mitigated by the .1 mapping tests and the collision test. Several questions are BLOCKED on connector operations; Medulla ships abstentions first so the questions fail honestly.

### Term decisions

The rows reuse existing terms. Terms that no ontology file declares are decided as follows. `prov:wasDerivedFrom` is reused for the derivation lineage link. Everything else is minted in `media-v1.ttl`, every new class extending a core class: `:MediaTag` and its ten properties (R001.100.1; `skos:closeMatch oa:Annotation`, `:taggedMedia` `skos:closeMatch oa:hasTarget`), `:MediaDerivation` with `:derivationKind` and `:derivationText` (R650.1; `skos:closeMatch prov:Derivation`), `:engagementMetricName` and `:engagementMetricValue` (R651), `:announcesStream` (R652) and the three transcript clock properties (R654), none of which has an open-vocabulary equivalent. Rows R029 and R031 are cross-repo operations, not terms. The mapping table is in [data-model.md](data-model.md).

## Phase 2: Task approach

`tasks.md` has one line per leaf row. Producer first: vocabulary, shapes and identity keys (US1, US2), then the connector mappings (US3, parallel), then the cross-source and cross-repo rows (US4), then the served surfaces (US5 to US7). Connector mapping rows and cross-repo rows are marked parallel because they touch disjoint files.

## Complexity Tracking

None.

## Build contract

Gates for any change under these rows: `cargo test -p medulla-core --test media_vocab_a --test media_vocab_b`, `cargo test -p medulla-rdf --test media_vocab_shapes --test media_vocab_cross_source`, `cargo clippy --workspace --all-targets -- -D warnings`, the repository complexity, duplication, dead-code and KISS gates with their committed thresholds, and the spec gate set (`pre-commit run --config .config/pre-commit.yaml --hook-stage pre-push`). No scanner gaming and no duplicate implementation: the bookmark search reuses the semantic service, and the media memory path reuses the one memory record of the decision-engine spec.

## Production caller chain

- Registered vocabulary: `GraphSchemaSources::default()` (`crates/medulla-graph-core/src/graph/schema_sources.rs:207`) → `current_core_catalog` (:656) → `core_specs` entries for `media`, `personal`, `retrieval` and `nutrition`. Not registered today: `content-v1.ttl` and the media and content shapes.
- Ingest surfaces: `Method::SourceIngest` → `crates/medulla-server-graph/src/dispatch/graph_pipeline/native_routes.rs::route_source_ingestion_or_resources` (:399) → `crates/medulla-h-ingest/src/source_ingestion.rs::prepare` (:167) → `admit_records` (:978) → `build_envelope` (:1658). Media tags, paper facts and meeting transcripts add steps here.
- Reads: `Method::SemanticSearch` → `crates/medulla-h-graph/src/graph_ops/semantic.rs::try_handle_semantic_compute` (:281); bookmark search through `crates/medulla-server-router/src/dispatch/router/control_plane.rs` (:400) → `crates/medulla-h-retrieve/src/semantic_index.rs` (:59); relations through `crates/medulla-h-query/src/query/dispatch.rs` (:138).
- Memory: `crates/medulla-h-graph/src/graph_ops/terminal.rs::try_handle_graph_domains` (:14) → the new `memory_surface` handler.
- Obsolete paths to delete: none.

## Cross-repo seams

- scion-sdk source envelopes for bookmarks and papers: `SCION-SOURCE-INGEST-R005.8` (consumer rows R001.102.1 and R002.5.1); contract `contracts/scion-sdk-source-ingest.md`.
- arbora served adapters for meeting and bookmark operations: `ARBORA-OPS-R065.4` and `ARBORA-OPS-R066`; contract `contracts/arbora-served-operations.md`.
- overstory renderers of summaries, mindmaps and exports: `OVERSTORY-APPS-R001.5` and `OVERSTORY-APPS-R001.6`; contract `contracts/overstory-presentation.md`.
- meristem invocation receipts for summaries: `MERISTEM-HARNESS-CYCLE-R068`; contract `contracts/meristem-summary-receipt.md`.
- connector operations and ontology cleanups owned by the connector repositories: `MEDULLA-CQ-CONN-R067.9` and rows R028 to R033; contract `contracts/connector-operations.md`.
