//! Streaming / CDC / subscriptions / triggers handler (CONCEPT:MEDULLA-KG.query.streaming-cdc-subscriptions/230,
//! feature `streaming`).
//!
//! Owns the `// ── Streaming ──` protocol section. These are STATEFUL (they drive the
//! [`CdcHub`] on `ServerState`), so like the txn/timeseries handlers they take
//! `state`. The hub is fed from the dispatch write-side-effect block (every durable
//! mutation emits a `CdcEvent`); this handler is the READ + REGISTER surface over it.
//!
//! Transport-compatible: every op is one Request → one Response over the existing
//! socket. `CdcRead`/`Watch`/`FiredTriggers` are cursor-driven (a `from_seq` the
//! consumer advances); `Watch` long-polls — it awaits the per-graph `Notify` up to
//! `timeout_ms` for the first change, then returns whatever arrived (NOT a streaming
//! frame side-channel).

use medulla_server_cdc::cdc::CdcStateExt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use medulla_isolation::isolation::AccessLevel;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_protocol_types::result_contract::messaging as results;
use medulla_query_types::wire::CdcEvent;
use medulla_query_types::wire::ContinuousAgg;
use medulla_query_types::wire::ContinuousQuerySpec;
use medulla_query_types::wire::WatchBatch;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;
use medulla_server_auth::access::check_graph_access;

/// Pull the CDC hub, or an ERROR response if the engine booted without one (only if a
/// future build path leaves it `None` — `streaming` builds always construct it).
async fn hub_of(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
) -> Result<Arc<medulla_server_cdc::cdc::CdcHub>, Response> {
    let s = state.read().await;
    match s.cdc_hub() {
        Some(h) => Ok(h.clone()),
        None => Err(Response::err(
            req_id,
            "ENGINE_UNAVAILABLE: streaming/CDC not configured",
        )),
    }
}

fn sum_numeric_field(rows: &[(String, Vec<u8>)], field: &str) -> f64 {
    let mut total = 0.0;
    for (_, blob) in rows {
        if let Ok(value) = medulla_types::msgpack::decode_property_value(blob) {
            if let Some(number) = value.get(field).and_then(|field| field.as_f64()) {
                total += number;
            }
        }
    }
    total
}

/// Compute the seed value for a continuous query from the graph's CURRENT state, so
/// the view is correct at registration (then deltas maintain it). `count` = matching
/// node count; `sum:<field>` = sum of the numeric field over matching nodes.
async fn seed_value(
    state: &Arc<RwLock<ServerState>>,
    authority: &GraphReadAuthority,
    spec: &ContinuousQuerySpec,
) -> f64 {
    let core = {
        let s = state.read().await;
        match s.registry.get(&spec.graph).map(|e| e.core.clone()) {
            Some(c) => c,
            None => return 0.0,
        }
    };
    let core = authority.project_core(&core);
    // Matching node rows: by label if set, else every visible node.
    let rows: Vec<(String, Vec<u8>)> = if spec.label.is_empty() {
        core.get_nodes()
    } else {
        core.get_nodes_by_label(&spec.label, 0)
    };
    match &spec.agg {
        ContinuousAgg::Count => rows.len() as f64,
        ContinuousAgg::Sum { field } => sum_numeric_field(&rows, field),
    }
}

async fn authorize_graph(
    state: &Arc<RwLock<ServerState>>,
    authority: &CarrierAuthority,
    graph: &str,
    access: AccessLevel,
) -> Result<(), String> {
    let s = state.read().await;
    let entry = s
        .registry
        .get(graph)
        .ok_or_else(|| format!("Graph '{graph}' not found"))?;
    check_graph_access(
        &s.isolation,
        Some(authority.agent_id()),
        graph,
        entry.graph_type,
        entry.owner.as_deref(),
        entry.importing.as_deref(),
        access,
    )
}

fn owned_name(authority: &CarrierAuthority, domain: &str, name: &str) -> String {
    format!("{domain}:{}:{name}", authority.owner_scope())
}

fn owned_prefix(authority: &CarrierAuthority, domain: &str) -> String {
    format!("{domain}:{}:", authority.owner_scope())
}

/// Keep only an event image visible to this actor. Ownership-changing updates may
/// expose the before image to the old owner and the after image to the new owner,
/// never both merely because one side was authorized.
fn sanitize_event(authority: &GraphReadAuthority, mut event: CdcEvent) -> Option<CdcEvent> {
    if is_topic_marker(&event) {
        return Some(event);
    }
    let before_visible = event.had_before && authority.can_see_blob(&event.before);
    let after_visible = event.had_after && authority.can_see_blob(&event.after);
    if !before_visible {
        event.had_before = false;
        event.before.clear();
    }
    if !after_visible {
        event.had_after = false;
        event.after.clear();
    }
    (before_visible || after_visible).then_some(event)
}

/// A reserved `__`-named topic marker (an action outbox topic, an incident or
/// ingestion marker) names no row and carries no image, so it discloses
/// nothing a graph reader cannot already see and passes through as it is.
fn is_topic_marker(event: &CdcEvent) -> bool {
    event.node_id.starts_with("__") && !event.had_before && !event.had_after
}

fn sanitize_watch(authority: &GraphReadAuthority, batch: WatchBatch) -> WatchBatch {
    WatchBatch {
        events: batch
            .events
            .into_iter()
            .filter_map(|event| sanitize_event(authority, event))
            .collect(),
        next_seq: batch.next_seq,
        gap: batch.gap,
        watermark: batch.watermark,
        head_seq: batch.head_seq,
        epoch: batch.epoch,
    }
}

async fn authorized_hub(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    carrier: &CarrierAuthority,
    graph: &str,
) -> Result<Arc<medulla_server_cdc::cdc::CdcHub>, Response> {
    if let Err(error) = authorize_graph(state, carrier, graph, AccessLevel::Read).await {
        return Err(Response::err(req_id, error));
    }
    hub_of(state, req_id).await
}

/// The graph a `view:<name>` feed key reads through: the graph the named plan view is
/// defined over. `Ok(None)` for a key that is not a view feed; `MATVIEW_UNKNOWN` when no
/// such view is defined.
#[cfg(feature = "matview")]
fn view_feed_graph(feed: &str) -> Result<Option<String>, String> {
    let Some(name) = feed.strip_prefix("view:") else {
        return Ok(None);
    };
    match medulla_server_cdc::matview::manager().get(name) {
        Some(def) => Ok(Some(def.graph)),
        None => Err(format!(
            "{}: plan view '{name}' is not defined",
            medulla_types::contract::ServerErrorCode::MatviewUnknown.as_str()
        )),
    }
}

/// Without plan views no key is a view feed.
#[cfg(not(feature = "matview"))]
fn view_feed_graph(_feed: &str) -> Result<Option<String>, String> {
    Ok(None)
}

struct StreamingRequest<'a> {
    state: &'a Arc<RwLock<ServerState>>,
    req_id: u64,
    carrier: &'a CarrierAuthority,
    read_authority: &'a GraphReadAuthority,
}

impl<'a> StreamingRequest<'a> {
    async fn cdc_read(&self, graph: String, from_seq: u64, limit: u32) -> Response {
        // A `view:<name>` feed is read-checked against the graph its view is defined over
        // (MEDULLA-INCREMENTAL-VIEWS-R010.3); a view that is not defined is refused.
        let access_graph = match view_feed_graph(&graph) {
            Ok(access_graph) => access_graph.unwrap_or_else(|| graph.clone()),
            Err(error) => return Response::err(self.req_id, error),
        };
        let hub = match authorized_hub(self.state, self.req_id, self.carrier, &access_graph).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        // B-8: `read` now returns a typed `CdcReadResult` carrying
        // `gap`/`watermark`/`head_seq`/`epoch` instead of a bare `Vec<CdcEvent>`
        // (which made a genuinely-caught-up cursor and a silently-fallen-off-the-
        // ring one indistinguishable `[]`) — sanitize only the events (a `gap`
        // result's `events` is always empty already) and pass the rest through.
        let mut result = hub.read(&graph, from_seq, limit);
        result.events = result
            .events
            .into_iter()
            .filter_map(|event| sanitize_event(self.read_authority, event))
            .collect();
        Response::ok(
            self.req_id,
            ResultPayload::of_ref::<results::CdcRead>(&result),
        )
    }

    async fn register_query(&self, name: String, spec_msgpack: Vec<u8>) -> Response {
        let spec: ContinuousQuerySpec = match medulla_types::msgpack::decode_bounded(
            &spec_msgpack,
            medulla_types::msgpack::MsgpackLimits::new(1024 * 1024, 50_000, 64),
        ) {
            Ok(spec) => spec,
            Err(_) => {
                return Response::err(
                    self.req_id,
                    "invalid or over-complex continuous query specification",
                )
            }
        };
        let hub = match authorized_hub(self.state, self.req_id, self.carrier, &spec.graph).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        let initial = seed_value(self.state, self.read_authority, &spec).await;
        hub.register_query(owned_name(self.carrier, "cq", &name), spec, initial);
        Response::ok(
            self.req_id,
            ResultPayload::scalar::<results::RegisterContinuousQuery>(name),
        )
    }

    async fn read_query(&self, name: String) -> Response {
        let hub = match hub_of(self.state, self.req_id).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        let storage_name = owned_name(self.carrier, "cq", &name);
        let Some(spec) = hub.query_spec(&storage_name) else {
            return Response::err(self.req_id, format!("continuous query '{name}' not found"));
        };
        if let Err(error) =
            authorize_graph(self.state, self.carrier, &spec.graph, AccessLevel::Read).await
        {
            return Response::err(self.req_id, error);
        }
        let value = seed_value(self.state, self.read_authority, &spec).await;
        match hub.read_query(&storage_name) {
            Some(mut result) => {
                result.name = name;
                result.value = value;
                Response::ok(
                    self.req_id,
                    ResultPayload::of_ref::<results::ReadContinuousQuery>(&result),
                )
            }
            None => Response::err(self.req_id, format!("continuous query '{name}' not found")),
        }
    }

    async fn drop_query(&self, name: String) -> Response {
        let hub = match hub_of(self.state, self.req_id).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        Response::ok(
            self.req_id,
            ResultPayload::scalar::<results::DropContinuousQuery>(hub.drop_query(&owned_name(
                self.carrier,
                "cq",
                &name,
            ))),
        )
    }

    async fn watch(
        &self,
        graph: String,
        from_seq: u64,
        label: String,
        timeout_ms: u64,
    ) -> Response {
        let hub = match authorized_hub(self.state, self.req_id, self.carrier, &graph).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        // Arm the change-notification future BEFORE the first pending check so a
        // write landing in the gap between the check and the await still wakes us
        // (`Notify::notified()` captures any `notify_waiters` after its creation) —
        // closing the lost-wakeup race. The future is enabled (registers the waiter)
        // on first poll, so pin it and enable it up front.
        let notify = hub.notifier(&graph);
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // First pass: anything already pending since the cursor? B-8: `gap` is
        // surfaced IMMEDIATELY, never masked behind a long-poll wait — waiting
        // could let unrelated fresh writes numerically "resolve" the
        // `from_seq > head_seq` check without the cursor's original history
        // ever having been recovered, turning an explicit signal back into a
        // silent one.
        let batch = sanitize_watch(
            self.read_authority,
            hub.watch_batch(&graph, from_seq, &label, 0),
        );
        if batch.gap || !batch.events.is_empty() {
            return Response::ok(self.req_id, ResultPayload::of_ref::<results::Watch>(&batch));
        }
        // Nothing yet — long-poll: await the next change up to timeout_ms, then
        // return whatever arrived (possibly still empty on timeout). One
        // Request → one Response; the client re-issues Watch to keep tailing.
        let wait = Duration::from_millis(timeout_ms);
        if !wait.is_zero() {
            let _ = tokio::time::timeout(wait, notified).await;
        }
        let batch = sanitize_watch(
            self.read_authority,
            hub.watch_batch(&graph, from_seq, &label, 0),
        );
        Response::ok(self.req_id, ResultPayload::of_ref::<results::Watch>(&batch))
    }

    /// `AwaitWatermark` (MEDULLA-DURABLE-KERNEL-R878): answer once the named resource's
    /// watermark reaches `at_least`, or when `deadline_ms` has passed on the engine clock.
    async fn await_watermark(&self, resource: String, at_least: u64, deadline_ms: u64) -> Response {
        use medulla_catalog_types::await_watermark::{WatermarkAwaited, WatermarkResource};
        use medulla_catalog_types::probe_machine::ProbeState;
        let Some(kind) = WatermarkResource::parse(&resource) else {
            return Response::err(
                self.req_id,
                "INVALID_ARGUMENT: a watermark resource is `graph:<name>`, `catalog:<source>`, \
                 `projection:<graph>` or `projection:<consumer>@<graph>`",
            );
        };
        let clock = medulla_kernel::kernel::dispatch_support::EngineClock;
        let outcome = match kind {
            WatermarkResource::Graph(graph) => {
                let hub = match authorized_hub(self.state, self.req_id, self.carrier, &graph).await
                {
                    Ok(hub) => hub,
                    Err(response) => return response,
                };
                let notify = hub.notifier(&graph);
                wait_for_watermark(&clock, deadline_ms, at_least, &notify, || async {
                    Ok(hub.head_seq(&graph))
                })
                .await
            }
            WatermarkResource::Catalog(source) => {
                use medulla_server_ops::api::query_txn::ForeignStateExt;
                let catalog = self.state.read().await.foreign_sources();
                if catalog.probe_state(self.carrier, &source).is_none() {
                    return Response::err(
                        self.req_id,
                        format!("INVALID_ARGUMENT: no source '{source}' is registered"),
                    );
                }
                wait_for_watermark(
                    &clock,
                    deadline_ms,
                    at_least,
                    catalog.probe_ended(),
                    || async {
                        Ok(u64::from(matches!(
                            catalog.probe_state(self.carrier, &source),
                            Some(
                                ProbeState::Baselined
                                    | ProbeState::Drifted
                                    | ProbeState::ProbeFailed
                            )
                        )))
                    },
                )
                .await
            }
            WatermarkResource::Projection { consumer, graph } => {
                // A consumer's cursor is read on a graph the caller may read; a pack graph is
                // keyed by the verified tenant alone and may not exist before its first projection.
                if consumer.is_some() {
                    if let Err(error) =
                        authorize_graph(self.state, self.carrier, &graph, AccessLevel::Read).await
                    {
                        return Response::err(self.req_id, error);
                    }
                }
                let hub = match hub_of(self.state, self.req_id).await {
                    Ok(hub) => hub,
                    Err(response) => return response,
                };
                let notify = hub.notifier(&graph);
                wait_for_watermark(&clock, deadline_ms, at_least, &notify, || {
                    self.projection_watermark(consumer.as_deref(), &graph)
                })
                .await
            }
        };
        match outcome {
            Ok((watermark, reached)) => Response::ok(
                self.req_id,
                ResultPayload::of::<results::AwaitWatermark>(WatermarkAwaited {
                    watermark,
                    reached,
                }),
            ),
            Err(error) => Response::err(self.req_id, error),
        }
    }

    /// The applied revision of the projection into `graph`: the named outbox consumer's
    /// acknowledged graph version, or the verified tenant's pack projection of that graph.
    async fn projection_watermark(
        &self,
        consumer: Option<&str>,
        graph: &str,
    ) -> Result<u64, String> {
        if let Some(consumer) = consumer {
            use medulla_mutation_types::mutation_scope::CommittedVersion;
            let persistence = self
                .state
                .read()
                .await
                .persistence
                .clone()
                .ok_or("UNAVAILABLE: projection cursors need durable persistence")?;
            let cursor = persistence
                .read_mutation_projection_cursor(
                    &medulla_kernel::persist::sanitize(graph),
                    consumer,
                )
                .await?;
            return Ok(match cursor.map(|cursor| cursor.committed_version) {
                Some(CommittedVersion::Graph { target, .. })
                | Some(CommittedVersion::Native { target, .. }) => target,
                Some(CommittedVersion::None) | None => 0,
            });
        }
        let store =
            medulla_persistence_records::agent_library::shared_agent_library(self.state).await?;
        let tenant = self.carrier.log_owner().0.to_string();
        let named = graph.to_string();
        tokio::task::spawn_blocking(move || {
            store.connector_pack_projection_watermark(&tenant, &named)
        })
        .await
        .map_err(|error| format!("INTERNAL: projection watermark task failed: {error}"))??
        .ok_or_else(|| format!("INVALID_ARGUMENT: no pack of this tenant projects into '{graph}'"))
    }

    async fn register_trigger(
        &self,
        name: String,
        graph: String,
        label: String,
        op: String,
        action_msgpack: Vec<u8>,
    ) -> Response {
        let hub = match authorized_hub(self.state, self.req_id, self.carrier, &graph).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        hub.register_trigger(
            owned_name(self.carrier, "trigger", &name),
            graph,
            label,
            op,
            action_msgpack,
        );
        Response::ok(
            self.req_id,
            ResultPayload::scalar::<results::RegisterTrigger>(name),
        )
    }

    async fn drop_trigger(&self, name: String) -> Response {
        let hub = match hub_of(self.state, self.req_id).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        Response::ok(
            self.req_id,
            ResultPayload::scalar::<results::DropTrigger>(hub.drop_trigger(&owned_name(
                self.carrier,
                "trigger",
                &name,
            ))),
        )
    }

    async fn list_triggers(&self, graph: String) -> Response {
        let hub = match authorized_hub(self.state, self.req_id, self.carrier, &graph).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        let prefix = owned_prefix(self.carrier, "trigger");
        let mut triggers = hub.list_triggers(&graph);
        triggers.retain_mut(|trigger| {
            let Some(display) = trigger.name.strip_prefix(&prefix) else {
                return false;
            };
            trigger.name = display.to_string();
            // The hub's raw counter includes invisible events; never expose it.
            trigger.fire_count = 0;
            true
        });
        Response::ok(
            self.req_id,
            ResultPayload::of_ref::<results::ListTriggers>(&triggers),
        )
    }

    async fn fired_triggers(&self, graph: String, from_seq: u64, limit: u32) -> Response {
        let hub = match authorized_hub(self.state, self.req_id, self.carrier, &graph).await {
            Ok(hub) => hub,
            Err(response) => return response,
        };
        let prefix = owned_prefix(self.carrier, "trigger");
        // B-8 follow-up: `fired` now returns a typed `FiredTriggersResult`
        // (`gap`/`watermark`/`head_seq`/`epoch`) instead of a bare
        // `Vec<FiredAction>` — a `gap` result's `fired` is always empty already,
        // so the visibility filter below is a no-op for it.
        let mut result = hub.fired(&graph, from_seq, limit);
        result.fired.retain_mut(|action| {
            let Some(display) = action.trigger.strip_prefix(&prefix) else {
                return false;
            };
            let visible = hub
                .read(&graph, action.change_seq, 1)
                .events
                .into_iter()
                .next()
                .and_then(|event| sanitize_event(self.read_authority, event))
                .is_some();
            if visible {
                action.trigger = display.to_string();
            }
            visible
        });
        Response::ok(
            self.req_id,
            ResultPayload::of_ref::<results::FiredTriggers>(&result),
        )
    }
}

/// Handle the streaming methods. Returns `Err(method)` for any non-streaming method so
/// the dispatch chain falls through — though dispatch only routes streaming methods here.
pub async fn try_handle(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    carrier: &CarrierAuthority,
    read_authority: &GraphReadAuthority,
    method: Method,
) -> Result<Response, Method> {
    // Raw hub sequence numbers, long-poll wakeups and aggregate through-seq
    // cursors reveal the existence/rate of filtered rows even when their payloads
    // are projected away.  The hub has no per-event actor-stable cursor ownership,
    // so active RLS fails this whole carrier closed except for an explicit admin.
    let admitted = admit_streaming_under_rls(read_authority, carrier, req_id);
    let Ok(()) = admitted else {
        return *admitted.err().unwrap();
    };
    let request = StreamingRequest {
        state,
        req_id,
        carrier,
        read_authority,
    };
    match method {
        Method::CdcRead {
            graph,
            from_seq,
            limit,
        } => Ok(request.cdc_read(graph, from_seq, limit).await),
        Method::RegisterContinuousQuery { name, spec_msgpack } => {
            Ok(request.register_query(name, spec_msgpack).await)
        }
        Method::ReadContinuousQuery { name } => Ok(request.read_query(name).await),
        Method::DropContinuousQuery { name } => Ok(request.drop_query(name).await),
        Method::Watch {
            graph,
            from_seq,
            label,
            timeout_ms,
        } => Ok(request.watch(graph, from_seq, label, timeout_ms).await),
        Method::AwaitWatermark {
            resource,
            at_least,
            deadline_ms,
        } => Ok(request
            .await_watermark(resource, at_least, deadline_ms)
            .await),
        Method::RegisterTrigger {
            name,
            graph,
            label,
            op,
            action_msgpack,
        } => Ok(request
            .register_trigger(name, graph, label, op, action_msgpack)
            .await),
        Method::DropTrigger { name } => Ok(request.drop_trigger(name).await),
        Method::ListTriggers { graph } => Ok(request.list_triggers(graph).await),
        Method::FiredTriggers {
            graph,
            from_seq,
            limit,
        } => Ok(request.fired_triggers(graph, from_seq, limit).await),
        other => Err(other),
    }
}

/// Wait until `read()` reaches `at_least`, woken by `notify`, for at most `deadline_ms` measured
/// on `clock`. Answers the last watermark read and whether it reached `at_least`. The waiter is
/// armed before each read, so a change landing between the read and the wait still wakes it.
async fn wait_for_watermark<F, Fut>(
    clock: &dyn medulla_types::clock::Clock,
    deadline_ms: u64,
    at_least: u64,
    notify: &tokio::sync::Notify,
    read: F,
) -> Result<(u64, bool), String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<u64, String>>,
{
    let deadline = clock.now_ms().saturating_add(deadline_ms);
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let watermark = read().await?;
        if watermark >= at_least {
            return Ok((watermark, true));
        }
        let remaining = deadline.saturating_sub(clock.now_ms());
        if remaining == 0 {
            return Ok((watermark, false));
        }
        let _ = tokio::time::timeout(Duration::from_millis(remaining), notified).await;
    }
}

/// Streaming cursors have no actor-stable cursor ownership, so active RLS fails
/// the whole carrier closed except for an explicit admin — `Err` is the final
/// routing/error outcome `try_handle` should return as-is (a streaming method
/// always gets an explicit denial, never a dispatch fall-through).
fn admit_streaming_under_rls(
    read_authority: &GraphReadAuthority,
    carrier: &CarrierAuthority,
    req_id: u64,
) -> Result<(), Box<Result<Response, Method>>> {
    if read_authority.is_active() && !carrier.is_admin() {
        return Err(Box::new(Ok(Response::err(
            req_id,
            "ACCESS_DENIED: streaming cursors have no actor-stable ownership under active RLS",
        ))));
    }
    Ok(())
}

#[cfg(test)]
mod await_watermark_tests {
    use super::wait_for_watermark;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    fn reader(mark: &Arc<AtomicU64>) -> impl Fn() -> std::future::Ready<Result<u64, String>> {
        let mark = Arc::clone(mark);
        move || std::future::ready(Ok(mark.load(Ordering::Relaxed)))
    }

    // spec: MEDULLA-DURABLE-KERNEL-R878
    #[tokio::test]
    async fn the_barrier_answers_on_the_watermark_or_the_engine_clock_deadline() {
        let notify = Arc::new(tokio::sync::Notify::new());
        let mark = Arc::new(AtomicU64::new(0));
        let clock = medulla_types::clock::ManualClock::new(1_000);

        // A watermark already at the target answers at once.
        mark.store(3, Ordering::Relaxed);
        let at_target = wait_for_watermark(&clock, 60_000, 3, &notify, reader(&mark)).await;
        assert_eq!(at_target, Ok((3, true)));

        // A deadline of zero on the engine clock answers the unmet watermark without waiting.
        mark.store(1, Ordering::Relaxed);
        let expired = wait_for_watermark(&clock, 0, 3, &notify, reader(&mark)).await;
        assert_eq!(expired, Ok((1, false)));

        // A failing read ends the wait with its refusal.
        let failed = wait_for_watermark(&clock, 60_000, 3, &notify, || {
            std::future::ready(Err("UNAVAILABLE: no cursor".to_string()))
        })
        .await;
        assert_eq!(failed, Err("UNAVAILABLE: no cursor".to_string()));

        // A change that lands while the barrier waits wakes it with the new watermark.
        mark.store(0, Ordering::Relaxed);
        let read = reader(&mark);
        let waiter_notify = Arc::clone(&notify);
        let waiter = tokio::spawn(async move {
            wait_for_watermark(
                &medulla_types::clock::SystemClock,
                60_000,
                2,
                &waiter_notify,
                read,
            )
            .await
        });
        tokio::task::yield_now().await;
        mark.store(2, Ordering::Relaxed);
        notify.notify_waiters();
        assert_eq!(waiter.await.expect("the barrier task ends"), Ok((2, true)));
    }
}
