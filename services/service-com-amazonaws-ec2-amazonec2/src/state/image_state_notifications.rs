// SPDX-License-Identifier: CC0-1.0

//! The event every change of an AMI's state is published as.
//!
//! An AMI's lifecycle is an event SOURCE and not only a control plane a client polls: EC2
//! sends an AMI state-change event to Amazon EventBridge when an image it is assembling
//! becomes usable and when one is deregistered (the EC2 User Guide's AMI state-change
//! events). The event goes to the account's default event bus ([`super::DEFAULT_EVENT_BUS`])
//! under the `source` this service owns ([`super::EC2_EVENT_SOURCE`]) and the `detail-type`
//! of an AMI state change ([`IMAGE_STATE_CHANGE_DETAIL_TYPE`]), naming the image in the
//! envelope's `resources` and carrying in the event `detail` the image's id, the state it
//! moved INTO, the error message of a state that failed and the id of the call the move
//! belongs to. So a client that bakes an image learns that the AMI is usable, or that it is
//! gone, without polling `DescribeImages`.
//!
//! WHICH moves are published: those of the doors that make them. A create answers a
//! `pending` AMI and leaves the `available` move owing on the store's own clock, so that move
//! is published when it FALLS DUE ([`ImageStateNotification::publish_when_due`]) — a client
//! watching the bus makes no further EC2 call, so a move published only by the next reader
//! would never be published at all. A deregistration makes its move inside the call that
//! answers it, so that one is published at once.
//!
//! A move the image never makes is never published: the publication re-reads the store at
//! the due instant, materialises what the clock owes there (the move was owed at that instant
//! whether or not anybody called) and publishes only where the image really stands in the
//! state the event reports — an image something deregistered before it came up announces no
//! `available`.
//!
//! The event's `time` is the instant the move falls DUE, not the instant the publication was
//! made, so a consumer reads when the AMI moved.
//!
//! EventBridge is reached the way every emulated service reaches another: the AWS SDK over
//! the framework's in-process callback, through the ONE client construction
//! [`::service_common::eventbridge_client`] holds, so nothing here opens a socket or names
//! another service's crate. The only thing the address is built from is the Region the call
//! was signed for, so no part of a request is a destination. Publication is BEST EFFORT — an
//! emulator with no EventBridge mounted has nowhere to publish — so a failure is traced and
//! the operation's own answer stands.

use ::std::time::{Duration, Instant};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{ComAmazonawsEc2Amazonec2State, DEFAULT_EVENT_BUS, EC2_EVENT_SOURCE, epoch_millis_in};

/// The `detail-type` every notification about one AMI's state is published under — the whole
/// class of them, which is what a rule written for an image's lifecycle matches on.
pub const IMAGE_STATE_CHANGE_DETAIL_TYPE: &str = "EC2 AMI State Change";

/// How many AMI moves this process publishes AT ONCE.
///
/// Each publication waits out the span the store owes the move and then makes one in-process
/// call, so the work in flight grows with the rate images are created at unless it is capped.
/// The figure is well above the moves one caller's calls can have in flight within a single
/// transition span and far below the images this store holds at all ([`super::MAX_IMAGES`]),
/// so an ordinary client never meets it and a flood of creates cannot grow this process
/// without limit — the same cap, and for the same reason, as the volume outcomes beside them
/// ([`super::MAX_CONCURRENT_VOLUME_PUBLICATIONS`]).
pub const MAX_CONCURRENT_IMAGE_PUBLICATIONS: usize = 64;

/// The slots those publications take, one each, held until the publication ends.
static PUBLICATION_SLOTS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_IMAGE_PUBLICATIONS);

/// The ARN the account knows the AMI `image_id` of `region` by.
///
/// The one place an image's ARN is built, so the event's `resources` cannot name one image
/// two ways. An AMI's ARN carries NO account segment — the `image` resource type is
/// `arn:${Partition}:ec2:${Region}::image/${ImageId}` (the Service Authorization Reference's
/// Amazon EC2 resource types), an AMI being addressable across the accounts it is shared
/// with — so the canonical ARN this emulator builds (`::engine::emulator_arn`, which keeps
/// the partition, the segment order and the separators in one place) has that one SEGMENT
/// emptied: the ARN is split on its separator and the fifth field cleared, rather than the
/// account id edited out of the text, so a resource name that happens to carry the same
/// digits cannot be changed with it.
#[must_use]
pub fn image_arn(region: &str, image_id: &str) -> String {
    let canonical = ::engine::emulator_arn("ec2", region, &format!("image/{image_id}"));
    // `arn:<partition>:<service>:<region>:<account>:<resource>` — six fields, the resource
    // taking whatever is left so its own separators stay where they are.
    let mut segments: Vec<&str> = canonical.splitn(ARN_SEGMENTS, ARN_SEPARATOR).collect();
    if segments.len() != ARN_SEGMENTS {
        // An ARN of another shape is one this function cannot take apart, so it is answered
        // as the canonical builder wrote it rather than edited blind.
        return canonical;
    }
    segments[ARN_ACCOUNT_SEGMENT] = "";
    segments.join(&ARN_SEPARATOR.to_string())
}

/// The separator between an ARN's segments.
const ARN_SEPARATOR: char = ':';

/// How many segments an ARN has: `arn`, the partition, the service, the Region, the account
/// and the resource — the last of which keeps its own separators.
const ARN_SEGMENTS: usize = 6;

/// Which of those segments names the account, counting from zero.
const ARN_ACCOUNT_SEGMENT: usize = 4;

/// One AMI state-change notification, rendered from the stored image and waiting to be
/// published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageStateNotification {
    /// The Region the event is published in: the Region the call that moved the image was
    /// signed for.
    pub region: String,
    /// The image's ARN, which the envelope carries in `resources`: the event names the
    /// resource it is about, which is what a rule filtering on `resources` reads.
    pub image_arn: String,
    /// The image that moved, as the `detail` names it.
    pub image_id: String,
    /// The state it moved INTO, spelled as the `ImageState` catalog spells it.
    pub state: String,
    /// What went wrong for a state that reports a failure, and empty for one that does not —
    /// nothing having gone wrong.
    pub error_message: String,
    /// The id of the call the move belongs to.
    ///
    /// The id the ANSWER to that call carries is written by the response renderer after the
    /// handler has returned (`vendor/smithy-schema-server`'s ec2Query renderer), which no
    /// handler can reach, so the event names the call with an id of its own in the same form.
    pub request_id: String,
    /// WHEN that move falls due, in epoch milliseconds.
    pub at_millis: i64,
    /// How long the clock still owes before that instant.
    pub due_in: Duration,
    /// Whether the move is one the CLOCK still has to make, which the publication asks the
    /// store about at the due instant.
    ///
    /// `false` for a move the call that asked for this notification already made while it
    /// held the store's guard, so that move is a fact and needs no further evidence.
    pub awaits_clock: bool,
}

impl ImageStateNotification {
    /// The notification for a move the calling door has ALREADY made: the state the image
    /// stands in by the time the call is answered.
    #[must_use]
    pub fn of_applied_move(image_id: &str, state: &str, region: &str) -> Self {
        Self {
            region: region.to_string(),
            image_arn: image_arn(region, image_id),
            image_id: image_id.to_string(),
            state: state.to_string(),
            error_message: String::new(),
            request_id: ::service_common::query_error_envelope::new_request_id(),
            at_millis: epoch_millis_in(Duration::ZERO),
            due_in: Duration::ZERO,
            awaits_clock: false,
        }
    }

    /// The notification for a move the CLOCK owes the image at `due_at`, as of `now`.
    ///
    /// The publication is made at that instant and only where the image really reached the
    /// state, which is what the store is asked at the due instant.
    #[must_use]
    pub fn of_owed_move(
        image_id: &str,
        state: &str,
        region: &str,
        due_at: Instant,
        now: Instant,
    ) -> Self {
        let due_in = due_at.saturating_duration_since(now);
        Self {
            region: region.to_string(),
            image_arn: image_arn(region, image_id),
            image_id: image_id.to_string(),
            state: state.to_string(),
            error_message: String::new(),
            request_id: ::service_common::query_error_envelope::new_request_id(),
            at_millis: epoch_millis_in(due_in),
            due_in,
            awaits_clock: true,
        }
    }

    /// The event's `detail`: which call, which image, the state it moved into, and what went
    /// wrong.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        ::serde_json::json!({
            "RequestId": self.request_id,
            "ImageId": self.image_id,
            "State": self.state,
            "ErrorMessage": self.error_message,
        })
    }

    /// Publish this notification once the move it reports has FALLEN DUE, without the call
    /// that started the move waiting for it.
    ///
    /// An AMI a create assembles moves on the store's own clock, so the move this event
    /// reports is not a fact yet when the call is answered: publishing it then would tell a
    /// consumer that an image was usable while a read of it still said `pending`. The
    /// publication is therefore made from a task that resumes at the due instant — because no
    /// later call of this account can carry it: a consumer of the bus makes none. That task
    /// holds no lock across the publication, it makes exactly one publication and ends, and
    /// there is at most one of it per move a door recorded.
    ///
    /// `state` is this service's store, which the task re-reads at the due instant for a move
    /// the clock owes: the move is MATERIALISED there (it was owed at that instant whether or
    /// not anybody called) and the event is published only where the image really stands in
    /// the state this event reports, so an image something deregistered on the way up
    /// announces nothing.
    ///
    /// The tasks in flight are CAPPED at [`MAX_CONCURRENT_IMAGE_PUBLICATIONS`]: a slot is
    /// taken before one is spawned and held inside it until it ends, on every path, so the
    /// work and the in-process calls this makes cannot grow with the rate calls arrive at. A
    /// move that finds no slot is not published, and says so in the log, which is the same
    /// best-effort answer as an emulator with no EventBridge to publish to.
    ///
    /// Call it with NO store lock held.
    pub fn publish_when_due(
        self,
        state: ::engine::ServiceState<ComAmazonawsEc2Amazonec2State>,
        callback: EmbeddedEmulator,
    ) {
        let Ok(slot) = PUBLICATION_SLOTS.try_acquire() else {
            ::tracing::warn!(
                image = %self.image_id,
                "this process is already publishing {MAX_CONCURRENT_IMAGE_PUBLICATIONS} \
                 AMI moves, so this one is not published"
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
            if self.awaits_clock && !clock_made_move(&state, &self.image_id, &self.state) {
                ::tracing::debug!(
                    image = %self.image_id,
                    state = %self.state,
                    "this image did not reach the state this event would report, so nothing \
                     is published for it"
                );
                return;
            }
            self.publish(&callback).await;
        });
    }

    /// Publish this notification onto the account's default event bus in the image's Region.
    ///
    /// Call it with NO store lock held. Nothing about the answer changes what the caller of
    /// the operation is told, for the reason the module docs give.
    pub async fn publish(self, callback: &EmbeddedEmulator) {
        let client =
            ::service_common::eventbridge_client::eventbridge_client(callback, &self.region);
        let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(EC2_EVENT_SOURCE)
            .detail_type(IMAGE_STATE_CHANGE_DETAIL_TYPE)
            // The event's `time` is WHEN the move falls due, not when it was published:
            // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339. If
            // no time stamp is provided, the time stamp of the PutEvents call is used."
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.at_millis,
            ))
            .resources(&self.image_arn)
            .detail(self.detail().to_string())
            .build();
        match client.put_events().entries(entry).send().await {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    image = %self.image_id,
                    region = %self.region,
                    "an AMI state-change notification was not accepted by the default event bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    image = %self.image_id,
                    region = %self.region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "an AMI state-change notification could not be published"
                );
            }
        }
    }
}

/// Whether the CLOCK carried the image `image_id` into the state `expected` names, with
/// everything the clock has already made due applied first.
///
/// A WRITE of exactly the kind a reporting operation makes: each move it materialises was
/// owed at this instant whether or not anybody called, so nothing a later caller would
/// otherwise have seen changes. Taken and given back without awaiting anything, so no lock of
/// this store is ever held across a call to another service.
fn clock_made_move(
    state: &::engine::ServiceState<ComAmazonawsEc2Amazonec2State>,
    image_id: &str,
    expected: &str,
) -> bool {
    let mut store = state.lock();
    store.materialize_due_transitions();
    store
        .images
        .get(image_id)
        .is_some_and(|image| image.state.as_deref() == Some(expected))
}
