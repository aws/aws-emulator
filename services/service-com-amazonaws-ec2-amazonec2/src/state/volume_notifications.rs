// SPDX-License-Identifier: CC0-1.0

//! The event an EBS volume's LIFECYCLE OUTCOME is published as.
//!
//! A volume create is not finished when the API answers it: the answer reports a `creating`
//! volume, and what becomes of that volume — it becomes available, or it cannot be created
//! at all — is settled afterwards. Amazon EBS publishes that OUTCOME, once per volume it
//! was asked to create, onto the account's default EventBridge bus under the `source` this
//! service owns ([`EC2_EVENT_SOURCE`]) and the `detail-type` of a volume notification
//! ([`VOLUME_NOTIFICATION_DETAIL_TYPE`]), naming the volume in the envelope's `resources`
//! and carrying in the event `detail` which call it is about ([`CREATE_VOLUME_EVENT`]), the
//! `result` that call reached and the `cause` of a result that failed. So a client that
//! drives automation off a volume's fate — an alarm, a remediation rule, a cleanup job —
//! learns that a volume failed without polling EC2 for a volume that will never appear, and
//! a client waiting for the notification is told instead of hanging.
//!
//! A DELETE is published the same way ([`VolumeNotification::of_delete`]): a delete the
//! service performed takes the volume out of the account's volumes, which is the outcome
//! [`DELETE_VOLUME_EVENT`] reports with the `result` [`VOLUME_DELETED`], so a consumer that
//! reacts to a volume going away (cost cleanup, inventory) learns of it. A call the service
//! REFUSED performed no delete and publishes nothing: the event is the outcome of a delete,
//! not of a call.
//!
//! The notification is built from the STORED record ([`VolumeNotification::of_create`]), so
//! every value in it is one the store decided, and it is published WHEN THE OUTCOME FALLS
//! DUE ([`VolumeNotification::publish_when_due`]) — the create itself answers the `creating`
//! volume it accepted and never waits for the publication, and nothing holds a store lock
//! across it. A consumer that acts on the event therefore finds the volume as the event
//! reports it, rather than still `creating`.
//!
//! * `result` is the outcome the store settled on: the state a volume that will come into
//!   being reaches (`available`), or [`VOLUME_CREATE_FAILED`] for one that cannot.
//! * `cause` is why it failed, in the words of the service that refused it
//!   ([`crate::state::VolumeKeyRefusal::cause`]) — the key and the state it is in — and is
//!   EMPTY for an outcome that did not fail, nothing having caused it.
//! * the event's `time` is the instant the outcome falls DUE
//!   ([`crate::state::VolumeRecord::available_at`]), not the instant the publication was
//!   made, so a consumer reads when the volume settled.
//! * `request-id` identifies the call the outcome belongs to, in the form most AWS services
//!   report one and minted by the ONE mint this tree has for it
//!   (`service_common::query_error_envelope::new_request_id`), so two outcomes are told apart
//!   by it.
//!
//! EventBridge is reached the way every emulated service reaches another: the AWS SDK over
//! the framework's in-process callback, through the ONE client construction
//! [`::service_common::eventbridge_client`] holds, so nothing here opens a socket or names
//! another service's crate. The only thing the address is built from is the Region the
//! create was signed for, so no part of a request is a destination. Publication is BEST
//! EFFORT — an emulator with no EventBridge mounted has nowhere to publish — so a failure
//! is traced and the operation's own answer stands.

use ::std::time::{Duration, Instant};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{VolumeKeyRefusal, VolumeRecord, VOLUME_STATE_AVAILABLE, epoch_millis_in};

/// The `source` of every event this service publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's prefix is the one its own records carry
/// (`smithy_query(query="service_info")` -> `endpointPrefix: ec2`).
pub const EC2_EVENT_SOURCE: &str = "aws.ec2";

/// The `detail-type` every notification about one EBS VOLUME is published under — the whole
/// class of them, which is what a rule written for a volume's fate matches on.
pub const VOLUME_NOTIFICATION_DETAIL_TYPE: &str = "EBS Volume Notification";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The `event` member of a notification about a volume CREATE: which call the outcome is
/// the outcome of, spelled as the API action it names.
pub const CREATE_VOLUME_EVENT: &str = "createVolume";

/// The `event` member of a notification about a volume DELETE, spelled as the API action it
/// names the same way [`CREATE_VOLUME_EVENT`] is.
pub const DELETE_VOLUME_EVENT: &str = "deleteVolume";

/// The `result` of a delete that took the volume out of the account's volumes.
///
/// A delete has one outcome it can reach — the volume is gone — so this is the only result a
/// delete notification carries; a delete the service REFUSED performed none and publishes
/// nothing at all.
pub const VOLUME_DELETED: &str = "deleted";

/// The `result` of a create that cannot produce a volume at all.
///
/// The other results are states the volume really reached, so they are named by the state
/// catalog itself ([`VOLUME_STATE_AVAILABLE`]) rather than spelled a second time here.
pub const VOLUME_CREATE_FAILED: &str = "failed";

/// How many volume outcomes this process publishes AT ONCE.
///
/// Each publication waits out the span the store owes the volume and then makes one
/// in-process call, so the work in flight grows with the rate creates arrive at unless it is
/// capped. The figure is well above the outcomes one caller's creates can have in flight
/// within a single transition span and far below the volumes this store holds at all
/// ([`crate::state::MAX_STORED_VOLUMES`]), so an ordinary client never meets it and a flood
/// of creates cannot grow this process without limit.
pub const MAX_CONCURRENT_VOLUME_PUBLICATIONS: usize = 64;

/// The slots those publications take, one each, held until the publication ends.
static PUBLICATION_SLOTS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_VOLUME_PUBLICATIONS);

/// One EBS volume notification, rendered from the stored volume and waiting to be
/// published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeNotification {
    /// The Region the event is published in: the Region the volume was created in.
    pub region: String,
    /// The volume's ARN, which the envelope carries in `resources`: the event names the
    /// resource it is about, which is what a rule filtering on `resources` reads.
    pub volume_arn: String,
    /// WHEN the outcome falls due, in epoch milliseconds.
    pub at_millis: i64,
    /// How long the clock still owes before that instant.
    pub due_in: Duration,
    /// The call this is the outcome of ([`CREATE_VOLUME_EVENT`]).
    pub event: &'static str,
    /// The outcome itself.
    pub result: String,
    /// Why a failed outcome failed, and empty for one that did not fail.
    pub cause: String,
    /// The id of the call this is the outcome of.
    ///
    /// The id the ANSWER to that call carries is written by the response renderer after the
    /// handler has returned (`vendor/smithy-schema-server`'s ec2Query renderer), which no
    /// handler can reach, so the event names the call with an id of its own in the same form.
    pub request_id: String,
}

impl VolumeNotification {
    /// The notification for the volume `record` a create has just stored, as of `now`.
    ///
    /// `refusal` is what the service that owns the volume's encryption key said when it
    /// would not produce the volume's data key ([`crate::state::volume_key_refusal`]), and
    /// `None` for every create that will produce a volume: so the result follows the
    /// outcome the store settled on rather than the shape of the request.
    #[must_use]
    pub fn of_create(
        record: &VolumeRecord,
        now: Instant,
        region: &str,
        refusal: Option<&VolumeKeyRefusal>,
    ) -> Self {
        let (result, cause) = match refusal {
            Some(refusal) => (VOLUME_CREATE_FAILED.to_string(), refusal.cause()),
            None => (VOLUME_STATE_AVAILABLE.to_string(), String::new()),
        };
        Self {
            region: region.to_string(),
            volume_arn: record.view.volume_arn.clone().unwrap_or_default(),
            at_millis: outcome_millis(record, now),
            due_in: record.available_at.saturating_duration_since(now),
            event: CREATE_VOLUME_EVENT,
            result,
            cause,
            request_id: ::service_common::query_error_envelope::new_request_id(),
        }
    }

    /// The notification for the volume `record` a delete has just taken out of the account's
    /// volumes.
    ///
    /// A delete that the service PERFORMED is a fact by the time the call is answered — the
    /// volume has left the account's volumes — so the outcome is due at once and carries no
    /// cause, nothing having failed. A delete the service REFUSED performed none, so its
    /// caller builds no notification for it at all: the event is the outcome of a delete, not
    /// of a call.
    #[must_use]
    pub fn of_delete(record: &VolumeRecord, region: &str) -> Self {
        Self {
            region: region.to_string(),
            volume_arn: record.view.volume_arn.clone().unwrap_or_default(),
            at_millis: epoch_millis_in(Duration::ZERO),
            due_in: Duration::ZERO,
            event: DELETE_VOLUME_EVENT,
            result: VOLUME_DELETED.to_string(),
            cause: String::new(),
            request_id: ::service_common::query_error_envelope::new_request_id(),
        }
    }

    /// The event's `detail`: which call, which result, and why.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        ::serde_json::json!({
            "event": self.event,
            "result": self.result,
            "cause": self.cause,
            "request-id": self.request_id,
        })
    }

    /// Publish this notification once the outcome it reports has FALLEN DUE, without the
    /// call that created the volume waiting for it.
    ///
    /// A volume is answered `creating` and settles a transition span later, so the outcome
    /// this event reports is not a fact yet when the create is answered: publishing it then
    /// would tell a consumer that a volume had failed, or become available, while a read of
    /// it still said `creating`. The publication is therefore made from a task that resumes
    /// at the due instant — the one thing in this service that waits for a clock-owed
    /// instant, because no later call of this account can carry it: a consumer of the bus
    /// makes none. That task holds no lock and nothing of the store, it makes exactly one
    /// publication and ends, and there is at most one of it per volume this service was asked
    /// to create.
    ///
    /// The tasks in flight are CAPPED at [`MAX_CONCURRENT_VOLUME_PUBLICATIONS`]: a slot is
    /// taken before one is spawned and held inside it until it ends, on every path, so the
    /// work and the in-process calls this makes cannot grow with the rate creates arrive at.
    /// An outcome that finds no slot is not published, and says so in the log, which is the
    /// same best-effort answer as an emulator with no EventBridge to publish to.
    ///
    /// Call it with NO store lock held. Nothing about the answer to the create changes,
    /// whatever becomes of the publication, for the reason the module docs give.
    pub fn publish_when_due(self, callback: EmbeddedEmulator) {
        let Ok(slot) = PUBLICATION_SLOTS.try_acquire() else {
            ::tracing::warn!(
                volume = %self.volume_arn,
                "this process is already publishing {MAX_CONCURRENT_VOLUME_PUBLICATIONS} \
                 volume outcomes, so this one is not published"
            );
            return;
        };
        ::tokio::spawn(async move {
            // Held for the whole publication and given back when this task ends, however it
            // ends, so the cap above counts what is really in flight.
            let _slot = slot;
            if !self.due_in.is_zero() {
                ::tokio::time::sleep(self.due_in).await;
            }
            self.publish(&callback).await;
        });
    }

    /// Publish this notification onto the account's default event bus in the volume's
    /// Region.
    ///
    /// Call it with NO store lock held. Nothing about the answer changes what the caller of
    /// the operation is told, for the reason the module docs give.
    pub async fn publish(self, callback: &EmbeddedEmulator) {
        let client = ::service_common::eventbridge_client::eventbridge_client(callback, &self.region);
        let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(EC2_EVENT_SOURCE)
            .detail_type(VOLUME_NOTIFICATION_DETAIL_TYPE)
            // The event's `time` is WHEN the outcome falls due, not when it was published:
            // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339. If
            // no time stamp is provided, the time stamp of the PutEvents call is used."
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.at_millis,
            ))
            .resources(&self.volume_arn)
            .detail(self.detail().to_string())
            .build();
        match client.put_events().entries(entry).send().await {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    volume = %self.volume_arn,
                    region = %self.region,
                    "an EBS volume notification was not accepted by the default event bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    volume = %self.volume_arn,
                    region = %self.region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "an EBS volume notification could not be published"
                );
            }
        }
    }
}

/// The wall-clock instant a volume's create outcome falls due, in epoch milliseconds.
///
/// The store keeps that instant on the monotonic clock it moves every transition on
/// ([`VolumeRecord::available_at`]), so the span still owed at `now` is what the one
/// computation of an event's time ([`epoch_millis_in`]) is asked for.
fn outcome_millis(record: &VolumeRecord, now: Instant) -> i64 {
    epoch_millis_in(record.available_at.saturating_duration_since(now))
}
