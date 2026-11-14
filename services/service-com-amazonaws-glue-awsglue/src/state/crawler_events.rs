// SPDX-License-Identifier: CC0-1.0

//! The EventBridge event this service publishes when a CRAWL comes to rest.
//!
//! # The rule this module implements
//!
//! Glue is an event SOURCE for its crawlers as well as its jobs: a crawl that comes to rest
//! is published onto the account's default event bus ([`DEFAULT_EVENT_BUS`]) under the
//! `source` this service owns ([`EVENT_SOURCE`]) and the `detail-type` of a crawler's state
//! change ([`CRAWLER_STATE_CHANGE_DETAIL_TYPE`]), carrying — in the event's `detail` — the
//! crawler the crawl belongs to, the account it is held in, the state the crawl came to rest
//! in ([`crawl_event_state`]), the sentence the service has about it and the tables it wrote.
//! A rule on that bus whose `EventPattern` matches then delivers it to that rule's targets,
//! so a client that wires crawler completion to downstream work — an ingestion job, an
//! archive of crawl outcomes, an alarm on failed crawls — observes the crawl without calling
//! Glue again.
//!
//! WHAT counts as the event: the state a crawl COMES TO REST in — the same instant at which
//! the crawler returns to [`CRAWLER_STATE_READY`] and the crawl's `LastCrawl` becomes
//! readable. Whichever `LastCrawlStatus` that crawl reached is published, so a crawl that
//! wrote what it found and one that could not be carried out are both reported; a crawl still
//! in flight has come to rest at nothing, so it publishes nothing, and a crawl is published
//! exactly once.
//!
//! The `state` the event carries is the EVENT's own vocabulary and not the modelled
//! `LastCrawlStatus` one: the statuses a crawl record holds are mapped onto it by
//! [`crawl_event_state`], so the member a rule's `EventPattern` filters on is the text this
//! event class carries rather than the text the control plane answers with.
//!
//! # How a transition becomes an event
//!
//! Nothing here decides WHEN a crawl finishes: the stored due instant does, through
//! [`CrawlerEntry::settle_crawl_if_due`]. That settle is the ONE place a crawl's record moves
//! from in-flight to finished, so it is also the one place the change is observed — whichever
//! door materialised it, the clock pump or a reader — and a crawl therefore cannot be
//! published twice or forgotten by a writer. The queue it feeds is bounded
//! ([`MAX_PENDING_CRAWLER_EVENTS`], oldest dropped first) because every crawl that finishes
//! appends to it.
//!
//! A queued change is published with NO store guard held
//! ([`publish_pending_crawler_events`]): EventBridge is reached the way every emulated
//! service reaches another, through the framework's in-process callback, so no socket is
//! opened and nothing outside this process is reachable from it. Publication is best effort —
//! an emulator with no EventBridge mounted has nowhere to publish — so a failure is logged and
//! the operation's own answer stands.
//!
//! WHICH call publishes: the store's CLOCK PUMP ([`drive_clock`]), for the reason the sibling
//! publisher of a job run's state change gives — a client that reacts to a crawl through
//! EventBridge makes no further Glue call, so an event published only by the next reader would
//! never be published at all. `StartCrawler` hands the crawl's lifecycle to that pump, which
//! waits for the due instant, materialises the finish and publishes it with no request waiting
//! on any of it.

use ::std::collections::VecDeque;
use ::std::time::{SystemTime, UNIX_EPOCH};

use super::*;

/// The `detail-type` of the event published when a CRAWL comes to rest.
pub const CRAWLER_STATE_CHANGE_DETAIL_TYPE: &str = "Glue Crawler State Change";

/// `detail.state` of a crawl that read its targets and wrote what it found — the event's own
/// spelling of [`LAST_CRAWL_STATUS_SUCCEEDED`].
pub const CRAWLER_EVENT_STATE_SUCCEEDED: &str = "Succeeded";

/// `detail.state` of a crawl that could not be carried out — the event's own spelling of
/// [`LAST_CRAWL_STATUS_FAILED`].
pub const CRAWLER_EVENT_STATE_FAILED: &str = "Failed";

/// `detail.state` of a crawl that was cut short, the third modelled `LastCrawlStatus`
/// (`CANCELLED`), in the event's own spelling.
pub const CRAWLER_EVENT_STATE_CANCELLED: &str = "Cancelled";

/// How many crawler state changes the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every crawl that
/// finishes appends to it, so without a bound a client starting crawls without end could grow
/// it without end. The oldest change is dropped first, so the crawls that have just finished
/// are the ones that survive. It is far above what a realistic burst produces — a crawler has
/// at most one crawl in flight, and the crawlers one store holds are themselves bounded
/// ([`MAX_CRAWLERS`]).
pub const MAX_PENDING_CRAWLER_EVENTS: usize = 2_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a drain holding more changes than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many changes ONE drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_CRAWLER_EVENTS`]), but one call draining all of
/// it would fan out to a rule's targets that many times: this caps one drain, and
/// [`publish_pending_crawler_events`] repeats a drain until nothing of that Region is left
/// queued, so nothing waits for a later call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// One crawler state change waiting to be published, rendered against the crawl as it stood
/// when it came to rest.
#[derive(Debug, Clone)]
pub struct CrawlerStateChange {
    /// The Region whose default bus the event goes onto: the Region the crawl was STARTED in,
    /// recorded on the crawler when it left the state with no run in it
    /// ([`CrawlerEntry::crawl_region`]), so the event is published through the endpoint that
    /// set the crawl going and nowhere else.
    pub region: String,
    /// WHEN the crawl came to rest — the DUE instant the clock owed the crawler its way back
    /// at, not the instant some later call materialised or published it, so the event and a
    /// read of the same crawler agree about the timeline.
    pub at: SystemTime,
    /// The rendered `detail`.
    pub detail: ::serde_json::Value,
}

/// The `state` this event carries for a crawl whose `LastCrawlStatus` is `status`.
///
/// The event's vocabulary is its own: the modelled statuses are upper case
/// ([`LAST_CRAWL_STATUS_SUCCEEDED`], [`LAST_CRAWL_STATUS_FAILED`] and the `CANCELLED` a crawl
/// cut short reaches), and this event class words each of them as a capitalised word. A status
/// outside that vocabulary is carried through as stored rather than reported as one of them.
#[must_use]
pub fn crawl_event_state(status: &str) -> String {
    match status {
        LAST_CRAWL_STATUS_SUCCEEDED => CRAWLER_EVENT_STATE_SUCCEEDED.to_string(),
        LAST_CRAWL_STATUS_FAILED => CRAWLER_EVENT_STATE_FAILED.to_string(),
        LAST_CRAWL_STATUS_CANCELLED => CRAWLER_EVENT_STATE_CANCELLED.to_string(),
        other => other.to_string(),
    }
}

/// The `detail` of a Glue crawler state change: WHICH crawler, in which account, the state the
/// crawl came to rest in, the sentence the service has about it and the tables it wrote.
///
/// Every member is read off the stored crawl, so the event reports the crawl a `GetCrawler` of
/// the same instant reports as `LastCrawl` and never a second, invented copy of it. The table
/// counts are the ones a metrics read answers with, carried as the text this event class
/// states them in. A crawl that failed at nothing has no reason to state, so its `message` is
/// the account of what became of it; one that failed carries the reason the crawl recorded,
/// which is the same text `LastCrawl.ErrorMessage` answers with.
fn crawler_state_change_detail(crawler_name: &str, record: &CrawlRecord) -> ::serde_json::Value {
    let state = crawl_event_state(&record.status);
    let mut detail = ::serde_json::Map::new();
    detail.insert(
        "crawlerName".to_string(),
        ::serde_json::Value::String(crawler_name.to_string()),
    );
    detail.insert(
        "accountId".to_string(),
        ::serde_json::Value::String(::engine::emulator_account_id().to_string()),
    );
    detail.insert("state".to_string(), ::serde_json::Value::String(state.clone()));
    detail.insert(
        "message".to_string(),
        ::serde_json::Value::String(
            record
                .error_message
                .clone()
                .unwrap_or_else(|| format!("Crawler {state}")),
        ),
    );
    detail.insert(
        "tablesCreated".to_string(),
        ::serde_json::Value::String(record.tables_created.to_string()),
    );
    detail.insert(
        "tablesUpdated".to_string(),
        ::serde_json::Value::String(record.tables_updated.to_string()),
    );
    detail.insert(
        "tablesDeleted".to_string(),
        ::serde_json::Value::String(record.tables_deleted.to_string()),
    );
    ::serde_json::Value::Object(detail)
}

impl CrawlerEntry {
    /// The state change the crawl `record` coming to rest at `at` publishes, or `None` when
    /// this crawler names no Region to publish it through.
    ///
    /// Called by the settle that moved the record from in-flight to finished
    /// ([`Self::settle_crawl_if_due`]), which is what makes the publication exactly once.
    pub(super) fn crawl_state_change(
        &self,
        record: &CrawlRecord,
        at: SystemTime,
    ) -> Option<CrawlerStateChange> {
        let region = self.crawl_region.clone()?;
        Some(CrawlerStateChange {
            region,
            at,
            detail: crawler_state_change_detail(&self.name, record),
        })
    }
}

impl ComAmazonawsGlueAwsglueState {
    /// Queue `change` for publication, oldest first.
    ///
    /// Called by the settle that observed the crawl come to rest, under the store's guard.
    /// The queue is bounded and the oldest change is dropped first, so a store whose events
    /// nothing drains cannot grow without end.
    pub(super) fn queue_crawler_state_change(&mut self, change: CrawlerStateChange) {
        while self.pending_crawler_events.len() >= MAX_PENDING_CRAWLER_EVENTS {
            self.pending_crawler_events.pop_front();
            ::tracing::warn!(
                "the queue of Glue crawler state changes waiting to be published to \
                 EventBridge is full at {MAX_PENDING_CRAWLER_EVENTS}; the oldest change was \
                 dropped"
            );
        }
        self.pending_crawler_events.push_back(change);
    }

    /// Take at most `limit` of this Region's changes that are not published yet, oldest first,
    /// leaving every other Region's where they are.
    ///
    /// Taking the changes out under the guard is what stops two concurrent calls from
    /// publishing one change twice.
    fn take_crawler_state_changes(&mut self, region: &str, limit: usize) -> Vec<CrawlerStateChange> {
        let mut taken: Vec<CrawlerStateChange> = Vec::new();
        let mut kept: VecDeque<CrawlerStateChange> = VecDeque::new();
        for change in ::std::mem::take(&mut self.pending_crawler_events) {
            if taken.len() < limit && change.region == region {
                taken.push(change);
            } else {
                kept.push_back(change);
            }
        }
        self.pending_crawler_events = kept;
        taken
    }

    /// How many changes are queued for publication — reporting only, for a reader that has to
    /// tell "no crawl finished" from "nothing was published".
    #[must_use]
    pub fn pending_crawler_event_count(&self) -> usize {
        self.pending_crawler_events.len()
    }

    /// The Regions this store has changes queued for, each once, in the order they were
    /// queued — the buses one publication pass has to reach.
    fn pending_crawler_event_regions(&self) -> Vec<String> {
        let mut regions: Vec<String> = Vec::new();
        for change in &self.pending_crawler_events {
            if !regions.iter().any(|region| region == &change.region) {
                regions.push(change.region.clone());
            }
        }
        regions
    }

    /// The earliest instant at which this store owes a crawler its way back to
    /// [`CRAWLER_STATE_READY`], if any — one of the instants the clock pump waits for.
    pub(super) fn next_crawl_due_at(&self) -> Option<SystemTime> {
        self.crawlers
            .values()
            .filter_map(|crawler| crawler.crawl_settles_at)
            .min()
    }

    /// Whether this store has a crawler state change waiting to be published, which is what
    /// keeps the clock pump awake until the queue is empty.
    pub(super) fn has_pending_crawler_events(&self) -> bool {
        !self.pending_crawler_events.is_empty()
    }
}

/// Publish every queued crawler state change onto the default EventBridge bus of the Region it
/// belongs to, and leave the queue empty.
///
/// Called with NO store guard held: each drain takes its changes out in one critical section,
/// so nothing is published twice, and the in-process `PutEvents` runs outside it. A Region's
/// changes are drained until it has none left — one drain is capped
/// ([`MAX_PUBLISHED_PER_DRAIN`]) so no single request is unbounded. Nothing about the answer
/// any operation gives changes either way.
pub async fn publish_pending_crawler_events(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<ComAmazonawsGlueAwsglueState>,
) {
    let regions = state.lock().pending_crawler_event_regions();
    for region in regions {
        let client = ::service_common::eventbridge_client::eventbridge_client(callback, &region);
        let mut events = {
            let mut store = state.lock();
            store.take_crawler_state_changes(&region, MAX_PUBLISHED_PER_DRAIN)
        };
        while !events.is_empty() {
            publish_batch(&client, &events).await;
            events = {
                let mut store = state.lock();
                store.take_crawler_state_changes(&region, MAX_PUBLISHED_PER_DRAIN)
            };
        }
    }
}

/// Publish one drain, in `PutEvents` requests of at most [`MAX_PUT_EVENTS_ENTRIES`] entries.
///
/// The event names no resource in its envelope: the `detail` names the crawler, which is how
/// a crawl is addressed, and the model gives a crawl no ARN of its own for `resources` to name
/// it by.
async fn publish_batch(client: &::aws_sdk_eventbridge::Client, events: &[CrawlerStateChange]) {
    for window in events.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for event in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(CRAWLER_STATE_CHANGE_DETAIL_TYPE)
                // The event's `time` is WHEN the crawl came to rest, not when the queue was
                // drained: `PutEventsRequestEntry$Time` is "the time stamp of the event, per
                // RFC3339. If no time stamp is provided, the time stamp of the PutEvents call
                // is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(event.at),
                ))
                .detail(event.detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a Glue crawler state change could not be published to EventBridge"
            );
        }
    }
}

/// `at` as the epoch milliseconds the event's `time` carries.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}
