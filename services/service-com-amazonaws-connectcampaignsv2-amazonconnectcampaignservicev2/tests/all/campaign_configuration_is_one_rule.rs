// SPDX-License-Identifier: CC0-1.0

//! The ONE configuration-change rule every campaign-configuration operation judges a
//! call by, driven end to end over the wire through each of those operations.
//!
//! `UpdateCampaignSource`, `UpdateCampaignSchedule`, `UpdateCampaignFlowAssociation`,
//! the entry-limits, communication-limits and communication-time updates and the three
//! deletions beside them all resolve and judge a campaign through the same entry point
//! on this service's store, so these cases hold every one of them to that rule rather
//! than to one copy of it:
//!
//! * a campaign the clock has in a state that takes no further configuration change is
//!   refused with HTTP 409 `InvalidCampaignStateException` naming that state, with the
//!   same sentence from every one of the operations, and nothing about the campaign is
//!   written;
//! * a campaign in any state that still takes the change is served, and the call moves
//!   no lifecycle;
//! * an id the request's region holds no campaign under is HTTP 404
//!   `ResourceNotFoundException` with the sentence that operation's own wire states — the
//!   lookup sentence, naming the id or not, as `campaign_not_found_message.rs` tables it.
//!
//! The campaign's `lastModifiedTime` is the same one rule read from the other side: it
//! is the instant of the write that last changed the campaign's definition or its
//! configuration, so every operation that writes one of those moves it, and a call that
//! is refused or that writes nothing — a read, a lifecycle move, a tag write, a removal
//! with nothing to remove, a rename onto the name the campaign already carries — leaves
//! it exactly as it was. `UpdateCampaignName` and the two channel-subtype-config
//! operations are writers of the campaign as well, so they are driven here beside the
//! operations the state rule above covers.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    campaign_id_not_found_message, CAMPAIGN_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, campaign_state, create_campaign_with, describe_campaign, drive_lifecycle, error_code,
    instant, last_modified_time, router, Harness,
};

/// One campaign-configuration operation: the request the model binds it to, relative
/// to `/v2/campaigns/{id}`, and a well-formed body for it.
#[derive(Debug, Clone)]
struct Configure {
    operation: &'static str,
    method: &'static str,
    suffix: String,
    body: Value,
    /// Whether this operation's not-found sentence NAMES the id it was asked about, as
    /// its own wire states it (`campaign_not_found_message.rs` holds the whole table).
    names_the_id: bool,
}

/// Every operation that changes a campaign's configuration — the ones whose model
/// declares `InvalidCampaignStateException` beside `ResourceNotFoundException`.
///
/// The `config` query values come from the model's own catalogs
/// (`CommunicationLimitsConfigType` and `CommunicationTimeConfigType`, as
/// `COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES` / `COMMUNICATION_TIME_CONFIG_TYPE_VALUES`
/// in this crate's `ops::types` render them).
fn configuration_operations() -> Vec<Configure> {
    let row = |operation: &'static str,
               method: &'static str,
               suffix: &str,
               body: Value,
               names_the_id: bool| Configure {
        operation,
        method,
        suffix: suffix.to_string(),
        body,
        names_the_id,
    };
    vec![
        row(
            "UpdateCampaignSource",
            "POST",
            "/source",
            json!({"source": {
                "customerProfilesSegmentArn":
                    "arn:aws:profile:us-east-1:000000000000:segment/one-rule"
            }}),
            true,
        ),
        row(
            "UpdateCampaignSchedule",
            "POST",
            "/schedule",
            // A window that is still open, so a served call leaves the campaign's
            // clock-derived state exactly where it was.
            json!({"schedule": {"startTime": instant(60.0), "endTime": instant(7_200.0)}}),
            false,
        ),
        row(
            "UpdateCampaignFlowAssociation",
            "POST",
            "/flow",
            json!({
                "connectCampaignFlowArn":
                    "arn:aws:connect:us-east-1:000000000000:instance/i-1/flow/f-1"
            }),
            true,
        ),
        row(
            "UpdateCampaignEntryLimits",
            "POST",
            "/entry-limits",
            json!({"entryLimitsConfig": {"maxEntryCount": 4, "minEntryInterval": "PT45M"}}),
            true,
        ),
        row(
            "DeleteCampaignEntryLimits",
            "DELETE",
            "/entry-limits",
            Value::Null,
            true,
        ),
        row(
            "UpdateCampaignCommunicationLimits",
            "POST",
            "/communication-limits",
            json!({"communicationLimitsOverride": {"allChannelSubtypes": {
                "communicationLimitsList": [
                    {"maxCountPerRecipient": 3, "frequency": 2, "unit": "DAY"}
                ]
            }}}),
            true,
        ),
        row(
            "DeleteCampaignCommunicationLimits",
            "DELETE",
            "/communication-limits?config=ALL_CHANNEL_SUBTYPES",
            Value::Null,
            true,
        ),
        row(
            "UpdateCampaignCommunicationTime",
            "POST",
            "/communication-time",
            json!({"communicationTimeConfig": {
                "localTimeZoneConfig": {"defaultTimeZone": "America/Los_Angeles"},
                "telephony": {"openHours": {
                    "dailyHours": {"TUESDAY": [{"startTime": "T08:00", "endTime": "T18:00"}]}
                }}
            }}),
            false,
        ),
        row(
            "DeleteCampaignCommunicationTime",
            "DELETE",
            "/communication-time?config=TELEPHONY",
            Value::Null,
            false,
        ),
    ]
}

/// The configuration a case's campaign is created carrying, so a write that slipped
/// past a refusal would show in `DescribeCampaign` whichever operation made it.
fn created_configuration() -> Value {
    json!({
        "source": {
            "customerProfilesSegmentArn":
                "arn:aws:profile:us-east-1:000000000000:segment/as-created"
        },
        "connectCampaignFlowArn":
            "arn:aws:connect:us-east-1:000000000000:instance/i-0/flow/f-0",
        "entryLimitsConfig": {"maxEntryCount": 9, "minEntryInterval": "P1D"},
        "communicationLimitsOverride": {"allChannelSubtypes": {
            "communicationLimitsList": [
                {"maxCountPerRecipient": 1, "frequency": 1, "unit": "DAY"}
            ]
        }},
        "communicationTimeConfig": {
            "localTimeZoneConfig": {"defaultTimeZone": "America/New_York"},
            "telephony": {"openHours": {
                "dailyHours": {"MONDAY": [{"startTime": "T09:00", "endTime": "T17:00"}]}
            }}
        },
    })
}

/// One configuration call on `id`, as the request the model binds it to.
async fn configure(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    op: &Configure,
) -> (u16, Value) {
    call(
        router,
        region,
        op.method,
        &format!("/v2/campaigns/{id}{}", op.suffix),
        &op.body,
    )
    .await
}

/// A campaign carrying [`created_configuration`], plus `window` as its schedule when
/// given (offsets in seconds from now — a closed window is how a case reaches the
/// state the CLOCK owns), put through `setup`. Hands back the minted id.
async fn configured_campaign(
    router: &Harness,
    region: &str,
    instance: &str,
    name: &str,
    window: Option<(f64, f64)>,
    setup: &[&str],
    case: &str,
) -> String {
    let mut extra = created_configuration();
    if let Some((start, end)) = window {
        extra["schedule"] = json!({"startTime": instant(start), "endTime": instant(end)});
    }
    let id = create_campaign_with(router, region, instance, name, extra).await;
    drive_lifecycle(router, region, &id, setup, case).await;
    id
}

/// A state a campaign still takes a configuration change in, and how a case gets it
/// there: the run window it is created with (`None` for no schedule at all) and the
/// lifecycle actions driven over it.
#[derive(Debug, Clone, Copy)]
struct Configurable {
    state: &'static str,
    window: Option<(f64, f64)>,
    setup: &'static [&'static str],
}

/// Every non-terminal value of the model's `CampaignState` catalog the API can reach:
/// `Initialized` before any action, and the three a lifecycle call writes. `Failed` is
/// the one value no call in this slice reaches.
const CONFIGURABLE: &[Configurable] = &[
    Configurable {
        state: "Initialized",
        window: None,
        setup: &[],
    },
    Configurable {
        state: "Running",
        window: None,
        setup: &["start"],
    },
    Configurable {
        state: "Paused",
        window: None,
        setup: &["start", "pause"],
    },
    Configurable {
        state: "Stopped",
        window: None,
        setup: &["start", "stop"],
    },
    // Started inside a window that is still OPEN: the clock has not finished this
    // campaign, so it is still configurable.
    Configurable {
        state: "Running",
        window: Some((-60.0, 7_200.0)),
        setup: &["start"],
    },
];

/// Every configuration operation refuses a campaign the CLOCK has in a terminal state,
/// with one status, one code, one reported state and one sentence — and writes nothing.
///
/// The state is reached without any call having written it: the campaign was started
/// inside a run window that has since closed, which is the terminal `Completed`.
#[tokio::test]
async fn every_configuration_operation_refuses_a_terminal_campaign_alike() {
    let region = "us-west-2";
    let instance = "configuration-one-rule";
    let router = router();

    for op in configuration_operations() {
        let case = format!("{} on a Completed campaign", op.operation);
        let id = configured_campaign(
            &router,
            region,
            instance,
            &format!("one-rule-terminal-{}", op.operation),
            Some((-7_200.0, -3_600.0)),
            &["start"],
            &case,
        )
        .await;
        let state = campaign_state(&router, region, &id, &case).await;
        assert_eq!(
            state, "Completed",
            "case {case:?}: expected the closed run window to leave the campaign Completed, \
             got {state:?}"
        );
        let before = describe_campaign(&router, region, &id, &case).await;

        let (status, body) = configure(&router, region, &id, &op).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidCampaignStateException"),
            "case {case:?}: expected InvalidCampaignStateException, got {}",
            error_code(&body)
        );
        assert_eq!(
            body["state"].as_str(),
            Some("Completed"),
            "case {case:?}: expected the refusal to name state \"Completed\", got {:?}",
            body["state"]
        );
        let expected_message = "A campaign in state Completed cannot be reconfigured";
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message),
            "case {case:?}: expected {expected_message:?}, got {:?}",
            body["message"]
        );

        let after = describe_campaign(&router, region, &id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refusal must write nothing, got {after}"
        );
    }
}

/// Every configuration operation is SERVED for a campaign in any state that still
/// takes the change, and the call moves no lifecycle.
#[tokio::test]
async fn every_configuration_operation_serves_a_configurable_campaign() {
    let region = "eu-west-1";
    let instance = "configuration-one-rule-served";
    let router = router();

    for op in configuration_operations() {
        for (index, row) in CONFIGURABLE.iter().enumerate() {
            let case = format!(
                "{} on a {} campaign (row {index})",
                op.operation, row.state
            );
            let id = configured_campaign(
                &router,
                region,
                instance,
                &format!("one-rule-served-{}-{index}", op.operation),
                row.window,
                row.setup,
                &case,
            )
            .await;
            let before = campaign_state(&router, region, &id, &case).await;
            assert_eq!(
                before, row.state,
                "case {case:?}: expected the setup to leave the campaign at {:?}, \
                 got {before:?}",
                row.state
            );

            let (status, body) = configure(&router, region, &id, &op).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            let after = campaign_state(&router, region, &id, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a configuration change moves no lifecycle, got {after:?}"
            );
        }
    }
}

/// An id no campaign of the request's region answers to is the same refusal from every
/// configuration operation, over several well-formed ids — so the refusal is "no such
/// campaign" rather than anything about one id's shape, and a campaign of another
/// region is exactly that: absent here, and untouched.
#[tokio::test]
async fn every_configuration_operation_refuses_an_unknown_id_alike() {
    let home = "us-east-1";
    let away = "ap-south-1";
    let instance = "configuration-one-rule-404";
    let router = router();
    let kept = configured_campaign(&router, home, instance, "kept", None, &[], "the kept campaign")
        .await;
    let before = describe_campaign(&router, home, &kept, "the kept campaign").await;

    // Each row is within `CampaignId`'s pattern `^[-:/a-zA-Z0-9]+$` and its 1..256
    // length bound (source: the operations' `id` member), carries no `/` — the id
    // travels as one path label — and none of them is the id the create above minted.
    let unknown = [
        ("a minted-shaped id", "000000009999".to_string()),
        ("a word", "no-such-campaign".to_string()),
        ("a single character", "x".to_string()),
        ("a colon-shaped id", "campaign:42".to_string()),
        ("the longest id the model takes", "a".repeat(256)),
        // The campaign another region holds: absent from this one.
        ("a campaign of another region", kept.clone()),
    ];

    for op in configuration_operations() {
        for (id_case, id) in &unknown {
            let case = format!("{} on {id_case}", op.operation);
            let region = if id_case == &"a campaign of another region" {
                away
            } else {
                home
            };
            let (status, body) = configure(&router, region, id, &op).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {}",
                error_code(&body)
            );
            let expected = if op.names_the_id {
                campaign_id_not_found_message(id)
            } else {
                CAMPAIGN_NOT_FOUND_MESSAGE.to_string()
            };
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {:?}",
                body["message"]
            );
        }
    }

    // The refusals changed nothing: the campaign the home region does hold is exactly
    // as its create left it.
    let after = describe_campaign(&router, home, &kept, "the kept campaign").await;
    assert_eq!(
        after, before,
        "case \"the kept campaign\": the refusals must write nothing, got {after}"
    );
}

/// The instant a projected `lastModifiedTime` names, for comparing two of them.
///
/// Ordered by the instant, which string comparison is not: the member travels as an
/// RFC 3339 `date-time`, whose fractional part is absent when it is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Stamp {
    secs: i64,
    subsec_nanos: u32,
}

impl Stamp {
    /// `text` as the instant it names, refusing anything that is not an RFC 3339
    /// `date-time` kept to the MILLISECOND — the precision this service stores a
    /// `date-time` at, so a stamp carrying a finer remainder would be one no other
    /// member of the record could be compared with.
    fn parse(text: &str, case: &str) -> Self {
        let instant = ::aws_smithy_types::DateTime::from_str(
            text,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected an RFC 3339 instant, got {text:?} ({error})")
        });
        let subsec_nanos = instant.subsec_nanos();
        assert_eq!(
            subsec_nanos % 1_000_000,
            0,
            "case {case:?}: expected a stamp kept to the millisecond, got {text:?}"
        );
        Self {
            secs: instant.secs(),
            subsec_nanos,
        }
    }
}

/// The `lastModifiedTime` the summary of `id` carries in `ListCampaigns`, over as many
/// pages as the listing hands out.
async fn listed_stamp(router: &MultiServiceRouter, region: &str, id: &str, case: &str) -> String {
    let mut token = String::new();
    // Bounded, so a listing that kept handing out a cursor would be a failure here
    // rather than a test that never returns.
    for _ in 0..64 {
        let request = if token.is_empty() {
            json!({})
        } else {
            json!({"nextToken": token})
        };
        let (status, body) = call(router, region, "POST", "/v2/campaigns-summary", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCampaigns expected 200, got {status} {body}"
        );
        let summary = body["campaignSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListCampaigns returned no list: {body}"))
            .iter()
            .find(|summary| summary["id"].as_str() == Some(id))
            .cloned();
        if let Some(summary) = summary {
            return last_modified_time(&summary, case);
        }
        match body["nextToken"].as_str() {
            Some(next) => token = next.to_string(),
            None => panic!("case {case:?}: ListCampaigns never listed the campaign {id:?}"),
        }
    }
    panic!("case {case:?}: ListCampaigns kept paging past the campaign {id:?}");
}

/// The `lastModifiedTime` the campaign `id` carries in `DescribeCampaign`.
async fn described_stamp(router: &Harness, region: &str, id: &str, case: &str) -> Stamp {
    let described = last_modified_time(&describe_campaign(router, region, id, case).await, case);
    Stamp::parse(&described, case)
}

/// The one `lastModifiedTime` the campaign `id` reports: `DescribeCampaign` and
/// `ListCampaigns` project the same stored instant, so a case that reads one has read
/// the other.
async fn stamp_of(router: &Harness, region: &str, id: &str, case: &str) -> Stamp {
    let described = last_modified_time(&describe_campaign(router, region, id, case).await, case);
    let listed = listed_stamp(router, region, id, case).await;
    assert_eq!(
        described, listed,
        "case {case:?}: expected DescribeCampaign and ListCampaigns to project one stamp, \
         got {described:?} and {listed:?}"
    );
    Stamp::parse(&described, case)
}

/// The POST that puts a removed block back: the path suffix it arrives at, relative to
/// `/v2/campaigns/{id}`, and the body it carries.
type Restore = (&'static str, fn() -> Value);

/// One operation that WRITES a campaign's definition or configuration.
///
/// `body` is given the attempt number, so an operation whose write is a write only
/// while the value CHANGES — the rename — can carry a new value each time.
struct Writer {
    operation: &'static str,
    method: &'static str,
    suffix: &'static str,
    body: fn(usize) -> Value,
    /// The POST that puts back what this operation REMOVES, for the operations whose
    /// write is a removal: such a call writes only while the campaign still holds the
    /// block it takes out, so an attempt puts the block back before removing it again.
    restore: Option<Restore>,
}

/// A telephony-and-SMS channel-subtype config, in the minimal form each block's own
/// shape requires: `outboundMode` and `defaultOutboundConfig` on both,
/// `connectContactFlowId` on the telephony block, `connectSourcePhoneNumberArn` with
/// `wisdomTemplateArn` on the SMS one.
fn two_channel_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        },
        "sms": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-west-2:000000000000:phone-number/p-1",
                "wisdomTemplateArn": "arn:aws:wisdom:us-west-2:000000000000:template/t-1"
            }
        }
    })
}

/// A well-formed `communicationTimeConfig` carrying a telephony window:
/// `localTimeZoneConfig` is the shape's one `@required` member.
fn communication_time_config() -> Value {
    json!({
        "localTimeZoneConfig": {"defaultTimeZone": "America/Chicago"},
        "telephony": {"openHours": {
            "dailyHours": {"WEDNESDAY": [{"startTime": "T08:30", "endTime": "T17:30"}]}
        }}
    })
}

/// A well-formed `communicationLimitsOverride`: every member of `CommunicationLimit`
/// is `@required`.
fn communication_limits_override() -> Value {
    json!({"allChannelSubtypes": {
        "communicationLimitsList": [{"maxCountPerRecipient": 2, "frequency": 3, "unit": "DAY"}]
    }})
}

/// A well-formed `entryLimitsConfig`.
fn entry_limits_config() -> Value {
    json!({"entryLimitsConfig": {"maxEntryCount": 6, "minEntryInterval": "PT30M"}})
}

/// Every operation that writes a campaign's definition or its configuration: the eight
/// the configuration rule above covers, the rename, and the two channel-subtype-config
/// operations, whose model declares no `InvalidCampaignStateException` and which are
/// therefore not in [`configuration_operations`].
///
/// The rows differ along what the rule generalises over: which member of the record the
/// call writes, whether the write is a replacement or a removal, and the method and
/// binding it arrives by.
fn campaign_writers() -> Vec<Writer> {
    vec![
        Writer {
            operation: "UpdateCampaignName",
            method: "POST",
            suffix: "/name",
            // A rename onto the name the campaign already carries writes nothing, so
            // each attempt asks for a name of its own.
            body: |attempt| json!({"name": format!("stamp-renamed-{attempt}")}),
            restore: None,
        },
        Writer {
            operation: "UpdateCampaignSource",
            method: "POST",
            suffix: "/source",
            body: |_| {
                json!({"source": {
                    "customerProfilesSegmentArn":
                        "arn:aws:profile:us-west-2:000000000000:segment/stamped"
                }})
            },
            restore: None,
        },
        Writer {
            operation: "UpdateCampaignSchedule",
            method: "POST",
            // A window that is still open, so the campaign stays configurable for the
            // next attempt.
            suffix: "/schedule",
            body: |_| json!({"schedule": {"startTime": instant(60.0), "endTime": instant(7_200.0)}}),
            restore: None,
        },
        Writer {
            operation: "UpdateCampaignFlowAssociation",
            method: "POST",
            suffix: "/flow",
            body: |_| {
                json!({"connectCampaignFlowArn":
                    "arn:aws:connect:us-west-2:000000000000:instance/i-1/flow/f-1"})
            },
            restore: None,
        },
        Writer {
            operation: "UpdateCampaignEntryLimits",
            method: "POST",
            suffix: "/entry-limits",
            body: |_| entry_limits_config(),
            restore: None,
        },
        Writer {
            operation: "DeleteCampaignEntryLimits",
            method: "DELETE",
            suffix: "/entry-limits",
            body: |_| Value::Null,
            restore: Some(("/entry-limits", entry_limits_config)),
        },
        Writer {
            operation: "UpdateCampaignCommunicationLimits",
            method: "POST",
            suffix: "/communication-limits",
            body: |_| json!({"communicationLimitsOverride": communication_limits_override()}),
            restore: None,
        },
        Writer {
            operation: "DeleteCampaignCommunicationLimits",
            method: "DELETE",
            // The one value `CommunicationLimitsConfigType` declares.
            suffix: "/communication-limits?config=ALL_CHANNEL_SUBTYPES",
            body: |_| Value::Null,
            restore: Some(("/communication-limits", || {
                json!({"communicationLimitsOverride": communication_limits_override()})
            })),
        },
        Writer {
            operation: "UpdateCampaignCommunicationTime",
            method: "POST",
            suffix: "/communication-time",
            body: |_| json!({"communicationTimeConfig": communication_time_config()}),
            restore: None,
        },
        Writer {
            operation: "DeleteCampaignCommunicationTime",
            method: "DELETE",
            suffix: "/communication-time?config=TELEPHONY",
            body: |_| Value::Null,
            restore: Some(("/communication-time", || {
                json!({"communicationTimeConfig": communication_time_config()})
            })),
        },
        Writer {
            operation: "UpdateCampaignChannelSubtypeConfig",
            method: "POST",
            suffix: "/channel-subtype-config",
            body: |_| json!({"channelSubtypeConfig": two_channel_config()}),
            restore: None,
        },
        Writer {
            operation: "DeleteCampaignChannelSubtypeConfig",
            method: "DELETE",
            // The campaign keeps its telephony block, so this removal is not the one
            // that would leave it configured for no channel at all.
            suffix: "/channel-subtype-config?channelSubtype=SMS",
            body: |_| Value::Null,
            restore: Some(("/channel-subtype-config", || {
                json!({"channelSubtypeConfig": two_channel_config()})
            })),
        },
    ]
}

/// A campaign carrying [`created_configuration`] and both channel blocks, so every
/// writer in [`campaign_writers`] has something to write and something to remove.
async fn stamped_campaign(
    router: &Harness,
    region: &str,
    instance: &str,
    name: &str,
) -> String {
    let mut extra = created_configuration();
    extra["channelSubtypeConfig"] = two_channel_config();
    create_campaign_with(router, region, instance, name, extra).await
}

/// How many times a case repeats a writer's own call while waiting for the stamp it
/// leaves to move.
///
/// The store keeps a `date-time` to the millisecond, so two writes landing inside one
/// millisecond leave the same stamp: the case looks again rather than waiting out a
/// span of the clock, and the bound is on the number of attempts rather than on how
/// fast the host runs them.
const STAMP_ATTEMPTS: usize = 200;

/// Every operation that writes a campaign's definition or configuration moves the
/// campaign's `lastModifiedTime`, and no read of it does.
#[tokio::test]
async fn every_writer_of_a_campaign_moves_its_last_modified_stamp() {
    let region = "us-west-2";
    let instance = "configuration-stamp-writers";
    let router = router();

    for writer in campaign_writers() {
        let case = format!("{} on a configurable campaign", writer.operation);
        let id = stamped_campaign(
            &router,
            region,
            instance,
            &format!("stamp-{}", writer.operation),
        )
        .await;

        // The create stamped the record, and reading it does not move it: a second
        // read — through both projections, which agree — reports the same instant.
        let created = stamp_of(&router, region, &id, &case).await;
        let read_again = stamp_of(&router, region, &id, &case).await;
        assert_eq!(
            read_again, created,
            "case {case:?}: expected reading the campaign to leave the stamp at \
             {created:?}, got {read_again:?}"
        );

        let mut previous = created;
        let mut moved = None;
        for attempt in 0..STAMP_ATTEMPTS {
            if let Some((suffix, restore)) = writer.restore {
                let (status, body) = call(
                    &router,
                    region,
                    "POST",
                    &format!("/v2/campaigns/{id}{suffix}"),
                    &restore(),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: putting the block back expected 200, got {status} {body}"
                );
                // The restore is a write of its own, so the removal below has to move
                // the stamp past the one IT left.
                previous = described_stamp(&router, region, &id, &case).await;
            }
            let (status, body) = call(
                &router,
                region,
                writer.method,
                &format!("/v2/campaigns/{id}{}", writer.suffix),
                &(writer.body)(attempt),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 on attempt {attempt}, got {status} {body}"
            );
            // One projection per attempt: both are read and compared before the loop
            // and once the stamp has moved, so an attempt that only waits for the
            // clock costs one read rather than a whole listing.
            let after = described_stamp(&router, region, &id, &case).await;
            assert!(
                after >= previous,
                "case {case:?}: a stamp never moves back, got {after:?} after {previous:?}"
            );
            if after > previous {
                moved = Some(after);
                break;
            }
            previous = after;
        }
        assert!(
            moved.is_some(),
            "case {case:?}: expected the write to move the stamp past {previous:?} \
             within {STAMP_ATTEMPTS} attempts, got {moved:?}"
        );
        // And the moved stamp is the one BOTH projections report, as the one before
        // the loop was.
        let listed = stamp_of(&router, region, &id, &case).await;
        assert_eq!(
            Some(listed),
            moved,
            "case {case:?}: expected both projections to report {moved:?}, got {listed:?}"
        );
    }
}

/// A call that writes nothing about the campaign's definition or configuration leaves
/// its `lastModifiedTime` exactly where it was.
///
/// The rows span what the rule generalises over: a refused configuration change, a
/// removal with nothing left to remove, a rename onto the name the campaign already
/// carries, the reads, the lifecycle moves and the tag writes — the last three being
/// calls that write the campaign's STATE or its TAGS, which are neither its definition
/// nor its configuration.
#[tokio::test]
async fn a_call_that_writes_no_configuration_leaves_the_last_modified_stamp() {
    let region = "eu-west-1";
    let instance = "configuration-stamp-writes-nothing";
    let router = router();

    // A removal, repeated: the first call takes the block out and the second has
    // nothing left to take, so only the first stamps the record.
    for writer in campaign_writers() {
        let Some((restore_suffix, restore)) = writer.restore else {
            continue;
        };
        let case = format!("{} with nothing left to remove", writer.operation);
        let id = stamped_campaign(
            &router,
            region,
            instance,
            &format!("stamp-repeat-{}", writer.operation),
        )
        .await;
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}{restore_suffix}"),
            &restore(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: putting the block in expected 200, got {status} {body}"
        );
        let path = format!("/v2/campaigns/{id}{}", writer.suffix);
        let (status, body) = call(&router, region, writer.method, &path, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the removal expected 200, got {status} {body}"
        );
        let removed = stamp_of(&router, region, &id, &case).await;
        // The same removal again: there is nothing left to take out, so it writes
        // nothing at all — the stamp included.
        let (status, body) = call(&router, region, writer.method, &path, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated removal expected 200, got {status} {body}"
        );
        let after = stamp_of(&router, region, &id, &case).await;
        assert_eq!(
            after, removed,
            "case {case:?}: expected the stamp to stay at {removed:?}, got {after:?}"
        );
    }

    // A configuration change the clock-derived state refuses: the campaign was started
    // inside a run window that has since closed, so it is `Completed`.
    for op in configuration_operations() {
        let case = format!("{} refused on a Completed campaign", op.operation);
        let id = configured_campaign(
            &router,
            region,
            instance,
            &format!("stamp-refused-{}", op.operation),
            Some((-7_200.0, -3_600.0)),
            &["start"],
            &case,
        )
        .await;
        let before = stamp_of(&router, region, &id, &case).await;
        let (status, body) = configure(&router, region, &id, &op).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} {body}"
        );
        let after = stamp_of(&router, region, &id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to leave the stamp at {before:?}, \
             got {after:?}"
        );
    }

    // Everything else a caller can do to a campaign without changing its definition or
    // its configuration.
    let case = "the calls that write no configuration";
    let id = stamped_campaign(&router, region, instance, "stamp-untouched").await;
    let described = describe_campaign(&router, region, &id, case).await;
    let name = described["name"]
        .as_str()
        .expect("a campaign projects its name")
        .to_string();
    let arn = described["arn"]
        .as_str()
        .expect("a campaign projects its ARN")
        .to_string();
    let tag_path = format!("/v2/tags/{}", arn.replace(':', "%3A").replace('/', "%2F"));
    let before = stamp_of(&router, region, &id, case).await;

    // (what the call is, its method, its path, its body, the status it answers)
    let quiet: Vec<(&str, &str, String, Value, u16)> = vec![
        (
            "a rename onto the name it already carries",
            "POST",
            format!("/v2/campaigns/{id}/name"),
            json!({"name": name}),
            200,
        ),
        (
            "GetCampaignState",
            "GET",
            format!("/v2/campaigns/{id}/state"),
            Value::Null,
            200,
        ),
        (
            "ListTagsForResource",
            "GET",
            tag_path.clone(),
            Value::Null,
            200,
        ),
        (
            "StartCampaign",
            "POST",
            format!("/v2/campaigns/{id}/start"),
            Value::Null,
            200,
        ),
        (
            "PauseCampaign",
            "POST",
            format!("/v2/campaigns/{id}/pause"),
            Value::Null,
            200,
        ),
        (
            "ResumeCampaign",
            "POST",
            format!("/v2/campaigns/{id}/resume"),
            Value::Null,
            200,
        ),
        (
            "StopCampaign",
            "POST",
            format!("/v2/campaigns/{id}/stop"),
            Value::Null,
            200,
        ),
        (
            "TagResource",
            "POST",
            tag_path.clone(),
            json!({"tags": {"owner": "stamp"}}),
            200,
        ),
        (
            "UntagResource",
            "DELETE",
            format!("{tag_path}?tagKeys=owner"),
            Value::Null,
            200,
        ),
    ];

    for (what, method, path, body, expected_status) in &quiet {
        let (status, answer) = call(&router, region, method, path, body).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {what} to answer HTTP {expected_status}, \
             got {status} {answer}"
        );
        let after = stamp_of(&router, region, &id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected {what} to leave the stamp at {before:?}, got {after:?}"
        );
    }
}
