// SPDX-License-Identifier: CC0-1.0

//! What IVS SAYS when a delete is refused because a channel still references the
//! resource the request named — driven over the wire, for every operation that
//! answers that refusal.
//!
//! Every case goes through the real front door (`MultiServiceRouter::dispatch` with a
//! restJson1 request), using the dispatch harness [`crate::channel_lifecycle`] owns.
//!
//! The rule under test, stated once: a delete refused because a channel still points
//! at the resource is the modelled `ConflictException` (409) stating one attachment
//! sentence — `Unable to perform ivs:<OperationName> while ` then the resource (the
//! ARN the request sent, with the resource kind before it for the operations whose
//! sentence spells one) then ` is still attached to a channel`. The ARN is the
//! sentence's only variable part: the channel that holds the reference is not in it,
//! and neither is how many channels hold one.
//!
//! The rows span every delete of this service that answers the refusal — one per resource
//! kind a channel can reference and whose delete models `ConflictException`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::RECORDING_CONFIGURATION_ACTIVE;

use crate::channel_lifecycle::{text, Ivs, Reply};

/// How long a row that needs a settled resource keeps LOOKING for it, and how long it
/// waits between looks. Generous by design: it bounds the transition having happened at
/// all, not how fast this host runs.
const SETTLED_DEADLINE: Duration = Duration::from_secs(90);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One operation that answers this refusal, with everything needed to stand its
/// resource up and have a channel point at it.
struct Subject {
    /// The delete under test, which is also the `ivs:<OperationName>` action token
    /// its sentence carries.
    operation: &'static str,
    /// The resource kind this operation's sentence spells before the ARN, with its
    /// trailing space — empty for an operation whose sentence names the ARN alone.
    kind_prefix: &'static str,
    create_operation: &'static str,
    /// The member of the create's output that carries the created resource.
    created_member: &'static str,
    read_operation: &'static str,
    /// The `UpdateChannel`/`CreateChannel` member a channel holds the reference in.
    channel_member: &'static str,
    /// The smallest create body there is for this resource: just what its model
    /// requires, varied by `suffix` so no two rows collide.
    body: fn(&str) -> Value,
}

/// `DeleteRecordingConfiguration`, whose sentence names the ARN with no resource kind
/// before it.
const RECORDING_CONFIGURATION: Subject = Subject {
    operation: "DeleteRecordingConfiguration",
    kind_prefix: "",
    create_operation: "CreateRecordingConfiguration",
    created_member: "recordingConfiguration",
    read_operation: "GetRecordingConfiguration",
    channel_member: "recordingConfigurationArn",
    body: recording_configuration_body,
};

/// `DeleteAdConfiguration`, whose sentence spells the resource kind before the ARN.
const AD_CONFIGURATION: Subject = Subject {
    operation: "DeleteAdConfiguration",
    kind_prefix: "ad configuration ",
    create_operation: "CreateAdConfiguration",
    created_member: "adConfiguration",
    read_operation: "GetAdConfiguration",
    channel_member: "adConfigurationArn",
    body: ad_configuration_body,
};

/// `DeleteFailoverSlateConfiguration`, whose sentence spells its own resource kind before
/// the ARN.
const FAILOVER_SLATE_CONFIGURATION: Subject = Subject {
    operation: "DeleteFailoverSlateConfiguration",
    kind_prefix: "failover slate configuration ",
    create_operation: "CreateFailoverSlateConfiguration",
    created_member: "failoverSlateConfiguration",
    read_operation: "GetFailoverSlateConfiguration",
    channel_member: "failoverSlateConfigurationArn",
    body: failover_slate_configuration_body,
};

/// The only destination type `CreateRecordingConfiguration` models, which it requires.
fn recording_configuration_body(suffix: &str) -> Value {
    json!({ "destinationConfiguration": { "s3": { "bucketName": format!("attach-{suffix}") } } })
}

/// The required MediaTailor list, of the form the modelled
/// `MediaTailorPlaybackConfigurationArn` pattern pins.
fn ad_configuration_body(suffix: &str) -> Value {
    json!({
        "mediaTailorPlaybackConfigurations": [{
            "playbackConfigurationArn": format!(
                "arn:aws:mediatailor:us-east-1:000000000000:playbackConfiguration/{suffix}"
            ),
        }],
    })
}

/// `CreateFailoverSlateConfiguration` requires nothing at all, so the smallest body is
/// the name that tells the rows apart — inside the modelled
/// `FailoverSlateConfigurationName` pattern `^[a-zA-Z0-9-_]*$`.
fn failover_slate_configuration_body(suffix: &str) -> Value {
    json!({ "name": format!("slate-{suffix}") })
}

/// The sentence the rule requires, spelled out here rather than imported, so this
/// suite pins the TEXT and not just the emulator agreeing with itself.
fn expected_message(subject: &Subject, arn: &str) -> String {
    format!(
        "Unable to perform ivs:{} while {}{arn} is still attached to a channel",
        subject.operation, subject.kind_prefix
    )
}

/// Create the subject's resource and return its ARN.
async fn create(ivs: &Ivs, subject: &Subject, suffix: &str) -> String {
    let created = ivs
        .ok(subject.create_operation, (subject.body)(suffix))
        .await;
    text(&created[subject.created_member], "arn")
}

/// What the subject's delete answers for `arn`, success or failure.
async fn delete(ivs: &Ivs, subject: &Subject, arn: &str) -> Reply {
    ivs.call(subject.operation, &json!({ "arn": arn })).await
}

/// The text the emulator stated, read from the key every SDK error parser reads.
fn message(reply: &Reply) -> String {
    text(&reply.body, "message")
}

/// The rule, over a table spanning the operations that answer this refusal, how the
/// reference came to exist, and how many channels hold one: every refusal states the
/// one attachment sentence for its operation, changes nothing, and names no channel.
#[tokio::test]
async fn a_delete_blocked_by_a_channel_states_the_attachment_sentence() {
    // Rows differ along what the rule generalises over: which operation (and so which
    // action token and resource-kind prefix) answers, whether each channel took the
    // reference at create time (`false`) or through a later `UpdateChannel` (`true`),
    // how many channels hold one, and — for the resource that has a lifecycle — which
    // state it is in when the delete arrives.
    let cases: Vec<(&str, &Subject, Vec<bool>, bool)> = vec![
        (
            "recording configuration, one channel referencing from its create",
            &RECORDING_CONFIGURATION,
            vec![false],
            false,
        ),
        (
            "recording configuration, one channel from a later update, once ACTIVE",
            &RECORDING_CONFIGURATION,
            vec![true],
            true,
        ),
        (
            "recording configuration, three channels by both routes",
            &RECORDING_CONFIGURATION,
            vec![false, true, false],
            false,
        ),
        (
            "ad configuration, one channel referencing from its create",
            &AD_CONFIGURATION,
            vec![false],
            false,
        ),
        (
            "ad configuration, two channels, one from each route",
            &AD_CONFIGURATION,
            vec![false, true],
            false,
        ),
        (
            "ad configuration, three channels from later updates",
            &AD_CONFIGURATION,
            vec![true, true, true],
            false,
        ),
        (
            "failover slate configuration, one channel referencing from its create",
            &FAILOVER_SLATE_CONFIGURATION,
            vec![false],
            false,
        ),
        (
            "failover slate configuration, two channels, one from each route",
            &FAILOVER_SLATE_CONFIGURATION,
            vec![false, true],
            false,
        ),
    ];

    for (case, subject, routes, wait_for_active) in cases {
        // A fresh store per row: what the refusal says must not depend on anything
        // another row left behind.
        let ivs = Ivs::new();
        let arn = create(&ivs, subject, "referenced").await;
        // A second resource of the same kind that nothing references, to show the
        // refusal is about the ARN the request named.
        let unreferenced = create(&ivs, subject, "unreferenced").await;

        let mut channels = Vec::new();
        for (index, via_update) in routes.iter().enumerate() {
            let mut request = json!({ "name": format!("holder-{index}") });
            if !via_update {
                request[subject.channel_member] = json!(arn);
            }
            let channel_arn = ivs.create_arn(request).await;
            if *via_update {
                let mut update = json!({ "arn": channel_arn });
                update[subject.channel_member] = json!(arn);
                ivs.ok("UpdateChannel", update).await;
            }
            channels.push(channel_arn);
        }

        if wait_for_active {
            // The row needs the resource SETTLED, which is a condition to look for rather
            // than a span of the clock to wait out: look again until the read reports the
            // settled state or a generous deadline passes.
            let deadline = Instant::now() + SETTLED_DEADLINE;
            loop {
                let read = ivs
                    .call(subject.read_operation, &json!({ "arn": arn }))
                    .await;
                let state = text(&read.body[subject.created_member], "state");
                if state == RECORDING_CONFIGURATION_ACTIVE {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "case {case:?}: expected {arn} to settle within the deadline, got \
                     {state:?} ({})",
                    read.body
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }

        let expected = expected_message(subject, &arn);
        let mut remaining = channels.clone();
        while !remaining.is_empty() {
            let refused = delete(&ivs, subject, &arn).await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (409, "ConflictException"),
                "case {case:?} with {} reference(s) left: expected 409 ConflictException, \
                 got {} {} {}",
                remaining.len(),
                refused.status,
                refused.error_code,
                refused.body
            );
            assert_eq!(
                message(&refused),
                expected,
                "case {case:?} with {} reference(s) left: expected the attachment sentence, \
                 got {}",
                remaining.len(),
                refused.body
            );
            // The sentence names the resource the request named and nothing else: no
            // channel the caller never asked about.
            for channel_arn in &channels {
                assert!(
                    !message(&refused).contains(channel_arn.as_str()),
                    "case {case:?} with {} reference(s) left: the refusal must not name the \
                     referencing channel {channel_arn}, got {:?}",
                    remaining.len(),
                    message(&refused)
                );
            }
            // The refusal removed nothing.
            let read = ivs
                .call(subject.read_operation, &json!({ "arn": arn }))
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?} with {} reference(s) left: the refused delete must keep {arn} \
                 readable, got {} {}",
                remaining.len(),
                read.status,
                read.body
            );

            // Clearing one reference: the empty string is the documented "none" value
            // for both channel members.
            let cleared = remaining.remove(0);
            let mut clear = json!({ "arn": cleared });
            clear[subject.channel_member] = json!("");
            ivs.ok("UpdateChannel", clear).await;
        }

        // With the last reference gone the same delete goes through, so the sentence
        // above was the only thing standing between the caller and the delete.
        let removed = delete(&ivs, subject, &arn).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: with no reference left the delete answers its modelled 204, got {} {}",
            removed.status, removed.body
        );
        let gone = ivs
            .call(subject.read_operation, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            (gone.status, gone.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: after the delete the read must be 404 ResourceNotFoundException, \
             got {} {} {}",
            gone.status,
            gone.error_code,
            gone.body
        );
        // And the resource nothing ever referenced never met the refusal at all.
        let other = delete(&ivs, subject, &unreferenced).await;
        assert_eq!(
            other.status, 204,
            "case {case:?}: a resource nothing references deletes straight away, got {} {}",
            other.status, other.body
        );
    }
}

/// The refusal is about the ARN the request named, not about the store holding
/// references: with one resource of each kind referenced in the same store, each
/// delete states its OWN operation's sentence about its OWN ARN.
#[tokio::test]
async fn each_operation_states_its_own_action_and_resource() {
    let ivs = Ivs::new();
    let recording = create(&ivs, &RECORDING_CONFIGURATION, "both-recording").await;
    let ad = create(&ivs, &AD_CONFIGURATION, "both-ad").await;
    let slate = create(&ivs, &FAILOVER_SLATE_CONFIGURATION, "both-slate").await;
    // One channel holding every reference at once.
    ivs.create_arn(json!({
        "name": "holder",
        "recordingConfigurationArn": recording,
        "adConfigurationArn": ad,
        "failoverSlateConfigurationArn": slate,
    }))
    .await;

    let referenced = [
        (&RECORDING_CONFIGURATION, &recording),
        (&AD_CONFIGURATION, &ad),
        (&FAILOVER_SLATE_CONFIGURATION, &slate),
    ];
    for (subject, arn) in referenced {
        let case = subject.operation;
        let refused = delete(&ivs, subject, arn).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {} {} {}",
            refused.status,
            refused.error_code,
            refused.body
        );
        assert_eq!(
            message(&refused),
            expected_message(subject, arn),
            "case {case:?}: expected this operation's own attachment sentence, got {}",
            refused.body
        );
        // No other resource's ARN is in this operation's sentence.
        for (_, other) in referenced {
            if other == arn {
                continue;
            }
            assert!(
                !message(&refused).contains(other.as_str()),
                "case {case:?}: the refusal must name only the ARN the request sent, got {:?}",
                message(&refused)
            );
        }
    }
}
