// SPDX-License-Identifier: CC0-1.0

//! The failover-slate configuration ARN on EVERY channel summary `ListChannels`
//! projects, driven over the wire through `MultiServiceRouter::dispatch` exactly as a
//! signed `aws ivs` call arrives.
//!
//! The rule under test: a channel object reports each of its optional configuration ARNs
//! as a PRESENT key holding a string — the ARN of the resource configured for it, or the
//! empty string when none is — and that holds for every summary of every page a list
//! answers with, whatever the number of channels, the page size, the filters, or the
//! shape of the channel itself. A client decides "is a failover slate configured on this
//! channel?" by comparing the value with the empty string, so an absent key is a
//! different answer from "nothing is configured", and a summary that is not the first one
//! of the first page is no different from one that is.
//!
//! `channel_configuration_arns.rs` pins the rule across the PROJECTIONS (create, read,
//! batch read, list, update) for one channel at a time; this suite pins it across the
//! shape of the LIST — its length, its paging and its filters — which is where a summary
//! past the first one, and a page past the first one, is reached at all.
//!
//! The last cases take the rule round the LOOP a client closes: a summary read out of a
//! list goes back into an update, which must accept the values the list reported and
//! answer the same ones — so a member a client carries round that loop is not dropped on
//! the way back.
//!
//! The last cases of all state the rule on the BYTES the emulator sends, because a key
//! set is a property of those bytes and not of what a parser makes of them: a member
//! written twice, or written and then shadowed, reads back fine through `serde_json`
//! (which keeps one value per key) while a strict client sees a malformed object.
//!
//! The key-set cases state the rule as the KEY SET of a summary: a summary is the
//! projection of the stored channel onto the members its shape declares, the
//! failover-slate ARN included, so it reports each of those members exactly as the single
//! read of the same channel does and reports
//! nothing outside that set. Stated that way the rule catches a DROPPED member whichever
//! member it is, rather than only the one the recorded responses were compared on.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::channel_lifecycle::{text, Ivs, CHANNEL_ARN_PREFIX};
use super::channel_projection_scope::{
    ad_configuration, objects_carrying, recording_configuration, POLICY_ARN,
};

/// The member every projected channel carries, which this model revision declares on
/// both channel shapes (`smithy_query(query="shape",shape="Channel,ChannelSummary")`) and
/// `CreateChannel`/`UpdateChannel` take as an input: the channels of this suite configure
/// none, so every summary here must report the unconfigured value for it.
const FAILOVER_SLATE: &str = "failoverSlateConfigurationArn";

/// The two other optional configuration ARNs `ChannelSummary` declares, each documented
/// `Default: ""` (`smithy_query(query="doc",shape="ChannelSummary",member=...)`): the
/// failover slate reads the same way they do, so every assertion checks it beside them.
const DECLARED_CONFIGURATION_ARNS: &[&str] =
    &["recordingConfigurationArn", "playbackRestrictionPolicyArn"];

/// The value a configuration ARN reads as while nothing is configured for it.
const UNCONFIGURED: &str = "";

/// Assert one summary reports the failover-slate ARN, and the declared configuration ARNs
/// beside it, as a present string holding the unconfigured value.
fn assert_unconfigured_arns(case: &str, where_from: &str, summary: &Value) {
    for member in std::iter::once(&FAILOVER_SLATE).chain(DECLARED_CONFIGURATION_ARNS) {
        let actual = summary.get(*member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            actual,
            Value::String(UNCONFIGURED.to_string()),
            "case {case:?}: {where_from} must report {member} as the unconfigured \
             {UNCONFIGURED:?}, got {actual:?} in {summary}"
        );
    }
}

/// Create `count` channels whose own shapes differ, and answer their ARNs.
///
/// The shapes cycle through the channel surface — the type (and so the preset the type
/// decides), the flags, whether the channel is named and whether it carries tags — so no
/// row of a table below is a set of channels that differ only by name.
async fn channels_of_differing_shapes(ivs: &Ivs, count: usize) -> BTreeSet<String> {
    let shapes: [Value; 5] = [
        json!({}),
        json!({ "type": "BASIC", "tags": { "team": "video" } }),
        json!({ "type": "STANDARD", "authorized": true, "insecureIngest": true }),
        json!({ "type": "ADVANCED_HD", "latencyMode": "NORMAL" }),
        json!({ "type": "ADVANCED_SD", "latencyMode": "LOW" }),
    ];
    let mut arns = BTreeSet::new();
    for index in 0..count {
        let mut request = shapes[index % shapes.len()].clone();
        // A distinct name only so each channel is individually addressable; what the
        // rows vary is the shape above.
        request["name"] = json!(format!("chan-{index}"));
        arns.insert(ivs.create_arn(request).await);
    }
    arns
}

/// Every summary of an unpaged list carries the member, over list lengths from one
/// channel up.
#[tokio::test]
async fn every_summary_of_a_list_reports_the_failover_slate_arn() {
    // Rows differ along the number of channels the list answers with: one, a few, and a
    // length past the default page the absent `maxResults` means is not needed here —
    // paging is the next test's axis.
    for count in [1_usize, 2, 5] {
        let ivs = Ivs::new();
        let expected = channels_of_differing_shapes(&ivs, count).await;

        let listed = ivs.ok("ListChannels", json!({})).await;
        let summaries = listed["channels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let case = format!("{count} channels listed unpaged");
        assert_eq!(
            summaries.len(),
            count,
            "case {case:?}: expected {count} summaries, got {listed}"
        );
        assert_eq!(
            summaries
                .iter()
                .map(|summary| text(summary, "arn"))
                .collect::<BTreeSet<_>>(),
            expected,
            "case {case:?}: the list must answer exactly the persisted channels, got {listed}"
        );
        for (index, summary) in summaries.iter().enumerate() {
            assert_unconfigured_arns(&case, &format!("the summary at index {index}"), summary);
        }
    }
}

/// Every summary of every page carries it, including the pages a `nextToken` resumes.
#[tokio::test]
async fn every_page_of_a_paginated_list_reports_the_failover_slate_arn() {
    let ivs = Ivs::new();
    let expected = channels_of_differing_shapes(&ivs, 5).await;

    // 1 and 100 are the published bounds of the modelled `MaxChannelResults`
    // (`range(min: 1, max: 100)`); 2 does not divide the set and 5 is exactly it.
    for page_size in [1_usize, 2, 5, 100] {
        let case = format!("maxResults={page_size} over {} channels", expected.len());
        let mut seen = BTreeSet::new();
        let mut token = Value::Null;
        for page_index in 0..(expected.len() + 2) {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = token.as_str() {
                request["nextToken"] = json!(token);
            }
            let page = ivs.ok("ListChannels", request).await;
            let summaries = page["channels"].as_array().cloned().unwrap_or_default();
            for (index, summary) in summaries.iter().enumerate() {
                assert_unconfigured_arns(
                    &case,
                    &format!("the summary at index {index} of page {page_index}"),
                    summary,
                );
                seen.insert(text(summary, "arn"));
            }
            token = page.get("nextToken").cloned().unwrap_or(Value::Null);
            if token.as_str().is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must reach every persisted channel, reached {seen:?}"
        );
    }
}

/// Every summary a filtered list answers with carries it: the filters narrow which
/// channels are projected, not how one is projected.
#[tokio::test]
async fn every_summary_of_a_filtered_list_reports_the_failover_slate_arn() {
    let ivs = Ivs::new();
    channels_of_differing_shapes(&ivs, 3).await;

    // One row per `filterBy*` member the operation declares
    // (`smithy_query(query="operation",operation="ListChannels")`), each with a value
    // the channels above match: the name of one of them, and the empty string every
    // unconfigured channel reports for the three configuration ARNs.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("filterByName naming one channel", json!({ "filterByName": "chan-1" }), 1),
        (
            "filterByRecordingConfigurationArn= matching every unconfigured channel",
            json!({ "filterByRecordingConfigurationArn": "" }),
            3,
        ),
        (
            "filterByPlaybackRestrictionPolicyArn= matching every unconfigured channel",
            json!({ "filterByPlaybackRestrictionPolicyArn": "" }),
            3,
        ),
        (
            "filterByAdConfigurationArn= matching every unconfigured channel",
            json!({ "filterByAdConfigurationArn": "" }),
            3,
        ),
    ];

    for (case, request, matches) in cases {
        let listed = ivs.ok("ListChannels", request).await;
        let summaries = listed["channels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            matches,
            "case {case:?}: expected {matches} summaries, got {listed}"
        );
        for (index, summary) in summaries.iter().enumerate() {
            assert_unconfigured_arns(case, &format!("the summary at index {index}"), summary);
        }
    }
}

/// Every member a channel summary may report: the eleven `ChannelSummary` declares
/// (`smithy_query(query="shape",shape="ChannelSummary")`) and the projected
/// failover-slate ARN the recorded responses carry beside them.
///
/// Used both ways round below: a summary reports EVERY member of this set and NOTHING
/// outside it, which is why neither direction can pass on an omission.
///
/// Each of them has a value for every channel this service holds — the three it documents
/// a `Default: ""` for, the two it documents `Default: false` for, the enums it documents
/// a default member for, the tag map (empty when nothing is tagged), the ARN, the name
/// (the empty string for an unnamed channel) and the preset the type decides — so a member
/// missing from a summary is a dropped member rather than an absent value.
const SUMMARY_MEMBERS: &[&str] = &[
    "arn",
    "name",
    "latencyMode",
    "authorized",
    "recordingConfigurationArn",
    "tags",
    "insecureIngest",
    "type",
    "preset",
    "playbackRestrictionPolicyArn",
    "adConfigurationArn",
    FAILOVER_SLATE,
];

/// Assert the summary `ListChannels` projects for `arn` is the projection of the channel
/// a single read answers with: it reports every member of [`SUMMARY_MEMBERS`], each
/// holding exactly what the channel reports for it, and no member outside that set.
///
/// The failover-slate ARN is checked on its own as well, because the channels of this
/// suite configure no failover slate, so every one of them reports it as the unconfigured
/// value.
async fn assert_the_summary_projects_the_channel(ivs: &Ivs, case: &str, arn: &str) {
    let channel = ivs.ok("GetChannel", json!({ "arn": arn })).await["channel"].clone();
    let listed = ivs.ok("ListChannels", json!({})).await;
    let summary = listed["channels"]
        .as_array()
        .and_then(|channels| {
            channels
                .iter()
                .find(|summary| text(summary, "arn") == arn)
                .cloned()
        })
        .unwrap_or(Value::Null);
    let reported = match summary.as_object() {
        Some(members) => members.clone(),
        None => panic!("case {case:?}: the list must project the channel {arn}, got {listed}"),
    };

    for member in reported.keys() {
        assert!(
            SUMMARY_MEMBERS.contains(&member.as_str()),
            "case {case:?}: a summary reports only the members of its shape, the \
             {FAILOVER_SLATE} included, got {member:?} in {summary}"
        );
    }
    for member in SUMMARY_MEMBERS {
        let on_channel = channel.get(*member).cloned();
        let on_summary = summary.get(*member).cloned();
        assert!(
            on_summary.is_some(),
            "case {case:?}: the summary must report {member}, got no such key in {summary}"
        );
        assert_eq!(
            on_summary, on_channel,
            "case {case:?}: the summary must report {member} exactly as the channel does \
             ({on_channel:?}), got {on_summary:?} in {summary}"
        );
    }
    let failover = summary.get(FAILOVER_SLATE).cloned().unwrap_or(Value::Null);
    assert_eq!(
        failover,
        Value::String(UNCONFIGURED.to_string()),
        "case {case:?}: the summary must report {FAILOVER_SLATE} as the unconfigured \
         {UNCONFIGURED:?}, got {failover:?} in {summary}"
    );
}

/// A summary reports the channel it names member for member, whatever that channel
/// carries: the member set of a summary is the member set of the shape, the
/// failover-slate ARN included, never one member short of it.
#[tokio::test]
async fn a_summary_projects_its_channel_member_for_member() {
    let ivs = Ivs::new();
    let recording = recording_configuration(&ivs, "recording").await;
    let ad = ad_configuration(&ivs, "ads").await;

    // Rows differ along the shape of the channel: which configuration ARNs it has, its
    // type (and so the preset the type decides), its flags, whether it is named, whether
    // it carries tags, and whether it carries the multitrack members `Channel` declares
    // and `ChannelSummary` does not — the last row being where "no member outside the
    // set" has something to exclude.
    let cases: Vec<(&str, Value)> = vec![
        ("a channel created from an empty request", json!({})),
        (
            "a BASIC channel with tags",
            json!({ "name": "tagged", "type": "BASIC", "tags": { "team": "video" } }),
        ),
        (
            "a channel with a recording configuration attached",
            json!({ "name": "recorded", "recordingConfigurationArn": recording.clone() }),
        ),
        (
            "a channel with an ad configuration and a playback restriction policy",
            json!({ "name": "monetised", "adConfigurationArn": ad.clone(),
                    "playbackRestrictionPolicyArn": POLICY_ARN }),
        ),
        (
            "an authorized ADVANCED_HD channel ingesting insecurely",
            json!({ "name": "advanced", "type": "ADVANCED_HD", "authorized": true,
                    "insecureIngest": true }),
        ),
        (
            "an ADVANCED_SD channel carrying the multitrack members no summary declares",
            json!({ "name": "multitrack", "type": "ADVANCED_SD",
                    "containerFormat": "FRAGMENTED_MP4",
                    "multitrackInputConfiguration": { "enabled": true, "policy": "REQUIRE",
                                                      "maximumResolution": "HD" } }),
        ),
    ];

    let mut arns = Vec::new();
    for (case, request) in &cases {
        arns.push((*case, ivs.create_arn(request.clone()).await));
    }
    for (case, arn) in &arns {
        assert_the_summary_projects_the_channel(&ivs, case, arn).await;
    }
}

/// The same rule after an update: a summary is a rendering of the CURRENT record, so an
/// update that attaches, detaches or re-types is reflected member for member on the
/// summary, with the projected member still on it.
#[tokio::test]
async fn a_summary_follows_its_channel_through_an_update() {
    let ivs = Ivs::new();
    let recording = recording_configuration(&ivs, "recording").await;
    let ad = ad_configuration(&ivs, "ads").await;
    let arn = ivs.create_arn(json!({ "name": "updated" })).await;

    // Rows differ along what the update changes: attaching each configuration ARN,
    // detaching one by sending the empty string the model documents as "none", the
    // channel type (which the preset follows), the flags and the tags.
    let updates: Vec<(&str, Value)> = vec![
        (
            "an update attaching a recording configuration",
            json!({ "recordingConfigurationArn": recording.clone() }),
        ),
        (
            "an update attaching an ad configuration and a policy too",
            json!({ "adConfigurationArn": ad.clone(),
                    "playbackRestrictionPolicyArn": POLICY_ARN }),
        ),
        (
            "an update detaching the recording configuration again",
            json!({ "recordingConfigurationArn": "" }),
        ),
        (
            "an update re-typing the channel, which the preset follows",
            json!({ "type": "ADVANCED_SD" }),
        ),
        (
            "an update renaming the channel and authorizing it",
            json!({ "name": "renamed", "authorized": true }),
        ),
    ];

    for (case, change) in updates {
        let mut request = change;
        request["arn"] = json!(arn);
        let updated = ivs.ok("UpdateChannel", request).await["channel"].clone();
        assert_eq!(
            text(&updated, "arn"),
            arn,
            "case {case:?}: the update must answer the channel it named, got {updated}"
        );
        assert_the_summary_projects_the_channel(&ivs, case, &arn).await;
    }
}

/// The summary members `UpdateChannel` declares an input member for
/// (`smithy_query(query="operation",operation="UpdateChannel")`), `arn` aside: this is
/// what a client that reads a summary and sends it back into an update can carry over.
const ROUND_TRIPPED_MEMBERS: &[&str] = &[
    "name",
    "latencyMode",
    "authorized",
    "recordingConfigurationArn",
    "insecureIngest",
    "type",
    "preset",
    "playbackRestrictionPolicyArn",
    "adConfigurationArn",
];

/// The summary members `UpdateChannel` declares NO input member for: the tag map, which
/// the tagging operations own, and the failover-slate ARN, which the channels of this
/// suite leave unconfigured. A client sending a whole summary back carries them
/// along, so they are the members an update must tolerate rather than refuse.
const NON_INPUT_SUMMARY_MEMBERS: &[&str] = &["tags", FAILOVER_SLATE];

/// An `UpdateChannel` request that sends one summary's own values back, so the update
/// asks for exactly the channel the summary reported.
///
/// `preset` is a `TranscodePreset`, whose declared values are
/// `HIGHER_BANDWIDTH_DELIVERY` and `CONSTRAINED_BANDWIDTH_DELIVERY`
/// (`smithy_query(query="shape",shape="TranscodePreset")`); the empty string a `BASIC` or
/// `STANDARD` channel reports for it is not one of them, so it is not a value an update
/// can carry back and is left out.
fn update_sending_back(summary: &Value) -> Value {
    let mut request = json!({ "arn": text(summary, "arn") });
    for member in ROUND_TRIPPED_MEMBERS {
        let Some(value) = summary.get(*member) else {
            continue;
        };
        if *member == "preset" && value.as_str() == Some(UNCONFIGURED) {
            continue;
        }
        request[*member] = value.clone();
    }
    request
}

/// The summary `ListChannels` projects for `arn`.
async fn summary_of(ivs: &Ivs, case: &str, arn: &str) -> Value {
    let listed = ivs.ok("ListChannels", json!({})).await;
    listed["channels"]
        .as_array()
        .and_then(|channels| {
            channels
                .iter()
                .find(|summary| text(summary, "arn") == arn)
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: the list must project the channel {arn}, got {listed}")
        })
}

/// A summary survives the ROUND TRIP a client puts it through: reading a channel out of a
/// list and sending those same values back into an update is a no-op, and the channel the
/// update answers reports every member of the summary it was built from — the projected
/// failover-slate ARN included — so nothing a summary reported is dropped on the way back.
///
/// This is the axis the suites above do not reach: they pin what a list REPORTS, while a
/// client that round-trips a summary also depends on the reported value being one the API
/// accepts back and on the update not clearing what it did not mention.
#[tokio::test]
async fn a_listed_summary_round_trips_back_through_an_update() {
    let ivs = Ivs::new();
    let recording = recording_configuration(&ivs, "recording").await;
    let ad = ad_configuration(&ivs, "ads").await;

    // Rows differ along the shape of the channel the summary describes: which
    // configuration ARNs it carries, its type (and so the preset the type decides), its
    // flags, whether it is named, whether it carries tags, and whether it carries the
    // `Channel`-only multitrack members no summary declares.
    let cases: Vec<(&str, Value)> = vec![
        ("a channel created from an empty request", json!({})),
        (
            "a BASIC channel with tags",
            json!({ "name": "tagged", "type": "BASIC", "tags": { "team": "video" } }),
        ),
        (
            "a channel with every configurable ARN attached",
            json!({ "name": "configured", "recordingConfigurationArn": recording.clone(),
                    "playbackRestrictionPolicyArn": POLICY_ARN,
                    "adConfigurationArn": ad.clone() }),
        ),
        (
            "an authorized ADVANCED_HD channel ingesting insecurely",
            json!({ "name": "advanced", "type": "ADVANCED_HD", "authorized": true,
                    "insecureIngest": true }),
        ),
        (
            "an ADVANCED_SD channel carrying the multitrack members no summary declares",
            json!({ "name": "multitrack", "type": "ADVANCED_SD",
                    "containerFormat": "FRAGMENTED_MP4",
                    "multitrackInputConfiguration": { "enabled": true, "policy": "REQUIRE",
                                                      "maximumResolution": "HD" } }),
        ),
        (
            "a STANDARD channel with the low latency mode",
            json!({ "name": "low", "type": "STANDARD", "latencyMode": "LOW" }),
        ),
    ];

    for (case, request) in cases {
        let arn = ivs.create_arn(request).await;
        let summary = summary_of(&ivs, case, &arn).await;

        let updated = ivs
            .ok("UpdateChannel", update_sending_back(&summary))
            .await["channel"]
            .clone();
        for member in SUMMARY_MEMBERS {
            let sent = summary.get(*member).cloned();
            let answered = updated.get(*member).cloned();
            assert_eq!(
                answered, sent,
                "case {case:?}: an update sending the summary back must answer {member} as \
                 the summary reported it ({sent:?}), got {answered:?} in {updated}"
            );
        }

        let again = summary_of(&ivs, case, &arn).await;
        assert_eq!(
            again, summary,
            "case {case:?}: sending a summary back must leave the channel as it was, got \
             {again} after {summary}"
        );
    }
}

/// The same round trip made the way a dynamic client makes it: the WHOLE summary object
/// goes back as the update's request body, the members `UpdateChannel` declares no input
/// member for included — the tag map and the projected failover-slate ARN — and the update
/// is accepted and still reports the projected member.
///
/// Only the ADVANCED types are driven here, because they are the ones whose summary
/// reports a `preset` the enum admits (see [`update_sending_back`]), so the whole object
/// is a valid request as it stands.
#[tokio::test]
async fn a_whole_summary_sent_back_verbatim_is_accepted() {
    let ivs = Ivs::new();
    let recording = recording_configuration(&ivs, "recording").await;

    // Rows differ along the ADVANCED type, which decides the preset, and along whether
    // the channel carries configuration ARNs and tags that travel back with it.
    let cases: Vec<(&str, Value)> = vec![
        (
            "an ADVANCED_HD channel with tags",
            json!({ "name": "hd", "type": "ADVANCED_HD", "tags": { "team": "video" } }),
        ),
        (
            "an ADVANCED_SD channel with a recording configuration",
            json!({ "name": "sd", "type": "ADVANCED_SD",
                    "recordingConfigurationArn": recording.clone() }),
        ),
    ];

    for (case, request) in cases {
        let arn = ivs.create_arn(request).await;
        let summary = summary_of(&ivs, case, &arn).await;
        for member in NON_INPUT_SUMMARY_MEMBERS {
            assert!(
                summary.get(*member).is_some(),
                "case {case:?}: the summary must report {member}, got {summary}"
            );
        }

        let updated = ivs.ok("UpdateChannel", summary.clone()).await["channel"].clone();
        let failover = updated.get(FAILOVER_SLATE).cloned().unwrap_or(Value::Null);
        assert_eq!(
            failover,
            Value::String(UNCONFIGURED.to_string()),
            "case {case:?}: the updated channel must report {FAILOVER_SLATE} as the \
             unconfigured {UNCONFIGURED:?}, got {failover:?} in {updated}"
        );

        let again = summary_of(&ivs, case, &arn).await;
        assert_eq!(
            again, summary,
            "case {case:?}: sending a summary back verbatim must leave the channel as it \
             was, got {again} after {summary}"
        );
    }
}

/// The projected member as a response BODY spells the pair: the quoted object key, and
/// the key together with the unconfigured value.
///
/// Counted over the whole body, the first is a count of the objects that carry the member
/// — which is the one thing a parsed body cannot report, because a parser keeps a single
/// value per key and so reads a member written twice exactly as it reads one written once.
const FAILOVER_SLATE_KEY_ON_THE_WIRE: &str = "\"failoverSlateConfigurationArn\"";
const FAILOVER_SLATE_UNCONFIGURED_ON_THE_WIRE: &str = "\"failoverSlateConfigurationArn\":\"\"";

/// How many times `needle` occurs in `haystack`.
fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// The channel objects of a parsed response, found WITHOUT looking at the projected
/// member, so a row's expected count is independent of the thing under test.
///
/// A channel object is one carrying an `arn` in the channel ARN space together with
/// `latencyMode`, a member both channel shapes declare and every channel this store holds
/// reports (`smithy_query(query="shape",shape="Channel,ChannelSummary")`). The second half
/// matters because an entry of the `errors` list a partial `BatchGetChannel` answers with
/// carries the channel ARN it could not resolve and nothing else of a channel.
fn channel_objects(body: &Value) -> Vec<Value> {
    objects_carrying(body, "arn")
        .into_iter()
        .filter(|object| {
            text(object, "arn").starts_with(CHANNEL_ARN_PREFIX)
                && object.get("latencyMode").is_some()
        })
        .collect()
}

/// One row of the wire-bytes table: the case name, how many channels the store holds, the
/// call to make, and how many channel objects its answer carries.
struct WireCase {
    case: &'static str,
    channels: usize,
    operation: &'static str,
    /// The request, built from the ARNs of the channels the row created.
    request: fn(&[String]) -> Value,
    expected: usize,
}

/// The member is written ONCE per channel object in the bytes the emulator sends, and
/// written nowhere else — so a client reading the body as text, as the recorded responses
/// are compared, finds exactly one `failoverSlateConfigurationArn` key per channel it was
/// answered, each holding the unconfigured value.
///
/// Rows differ along the operation that serves the channel objects, how many it serves
/// (none, one, several), and whether the answer carries objects that are NOT channels — a
/// create's stream key, a partial batch's error entries, another resource's summaries.
#[tokio::test]
async fn every_channel_object_on_the_wire_carries_the_member_exactly_once() {
    let cases: Vec<WireCase> = vec![
        WireCase {
            case: "a list of one channel",
            channels: 1,
            operation: "ListChannels",
            request: |_| json!({}),
            expected: 1,
        },
        WireCase {
            case: "a list of four channels",
            channels: 4,
            operation: "ListChannels",
            request: |_| json!({}),
            expected: 4,
        },
        WireCase {
            case: "a page of two out of four channels",
            channels: 4,
            operation: "ListChannels",
            request: |_| json!({ "maxResults": 2 }),
            expected: 2,
        },
        WireCase {
            case: "a list over a store holding no channel",
            channels: 0,
            operation: "ListChannels",
            request: |_| json!({}),
            expected: 0,
        },
        WireCase {
            case: "a create, whose answer carries a stream key beside the channel",
            channels: 0,
            operation: "CreateChannel",
            request: |_| json!({ "name": "created" }),
            expected: 1,
        },
        WireCase {
            case: "a single read",
            channels: 1,
            operation: "GetChannel",
            request: |arns| json!({ "arn": arns[0] }),
            expected: 1,
        },
        WireCase {
            case: "an update",
            channels: 1,
            operation: "UpdateChannel",
            request: |arns| json!({ "arn": arns[0], "name": "renamed" }),
            expected: 1,
        },
        WireCase {
            case: "a batch read of three channels",
            channels: 3,
            operation: "BatchGetChannel",
            request: |arns| json!({ "arns": arns }),
            expected: 3,
        },
        WireCase {
            case: "a batch read whose last ARN names no channel",
            channels: 2,
            operation: "BatchGetChannel",
            request: |arns| {
                json!({ "arns": [arns[0].clone(), arns[1].clone(),
                                 format!("{CHANNEL_ARN_PREFIX}absentchannel")] })
            },
            expected: 2,
        },
        WireCase {
            case: "the stream keys of a channel, an answer with no channel object in it",
            channels: 1,
            operation: "ListStreamKeys",
            request: |arns| json!({ "channelArn": arns[0] }),
            expected: 0,
        },
    ];

    for WireCase {
        case,
        channels,
        operation,
        request,
        expected,
    } in cases
    {
        let ivs = Ivs::new();
        let arns: Vec<String> = channels_of_differing_shapes(&ivs, channels)
            .await
            .into_iter()
            .collect();
        let reply = ivs.call(operation, &request(&arns)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {} {}",
            reply.status, reply.raw
        );

        let carried = channel_objects(&reply.body);
        assert_eq!(
            carried.len(),
            expected,
            "case {case:?}: expected {expected} channel object(s) in the answer, got {}, in {}",
            carried.len(),
            reply.raw
        );
        let keys = occurrences(&reply.raw, FAILOVER_SLATE_KEY_ON_THE_WIRE);
        assert_eq!(
            keys, expected,
            "case {case:?}: the body must spell the {FAILOVER_SLATE} key once per channel \
             object ({expected}), got {keys} in {}",
            reply.raw
        );
        let unconfigured = occurrences(&reply.raw, FAILOVER_SLATE_UNCONFIGURED_ON_THE_WIRE);
        assert_eq!(
            unconfigured, expected,
            "case {case:?}: every one of those {expected} key(s) must hold the unconfigured \
             {UNCONFIGURED:?}, {unconfigured} do in {}",
            reply.raw
        );
    }
}

/// The same count, page by page: every page of a paginated list spells the member once per
/// summary that page carries, including the pages a `nextToken` resumes and the last page,
/// which carries fewer summaries than the page size asks for.
#[tokio::test]
async fn every_page_of_a_list_spells_the_member_once_per_summary() {
    let ivs = Ivs::new();
    let expected = channels_of_differing_shapes(&ivs, 5).await;

    // 1 and 100 are the published bounds of the modelled `MaxChannelResults`
    // (`range(min: 1, max: 100)`); 2 does not divide the set and 5 is exactly it.
    for page_size in [1_usize, 2, 5, 100] {
        let case = format!("maxResults={page_size} over {} channels", expected.len());
        let mut seen = BTreeSet::new();
        let mut token = Value::Null;
        for page_index in 0..(expected.len() + 2) {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = token.as_str() {
                request["nextToken"] = json!(token);
            }
            let reply = ivs.call("ListChannels", &request).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200 for page {page_index}, got {} {}",
                reply.status, reply.raw
            );
            let summaries = reply.body["channels"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let keys = occurrences(&reply.raw, FAILOVER_SLATE_UNCONFIGURED_ON_THE_WIRE);
            assert_eq!(
                keys,
                summaries.len(),
                "case {case:?}: page {page_index} must spell {FAILOVER_SLATE} as the \
                 unconfigured {UNCONFIGURED:?} once per summary ({}), got {keys} in {}",
                summaries.len(),
                reply.raw
            );
            for summary in &summaries {
                seen.insert(text(summary, "arn"));
            }
            token = reply.body.get("nextToken").cloned().unwrap_or(Value::Null);
            if token.as_str().is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must reach every persisted channel, reached {seen:?}"
        );
    }
}

/// Listing is a report: repeating it answers byte-identical summaries, so the projected
/// member is a rendering of the stored channel rather than something a read consumed.
#[tokio::test]
async fn repeating_the_list_answers_identical_summaries() {
    let ivs = Ivs::new();
    channels_of_differing_shapes(&ivs, 3).await;

    let first = ivs.ok("ListChannels", json!({})).await;
    for round in 1..3 {
        let again = ivs.ok("ListChannels", json!({})).await;
        assert_eq!(
            again, first,
            "case round {round}: ListChannels must answer identically, got {again} after {first}"
        );
    }
}
