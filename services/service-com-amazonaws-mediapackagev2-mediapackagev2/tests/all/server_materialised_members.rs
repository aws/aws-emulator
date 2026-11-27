// SPDX-License-Identifier: CC0-1.0

//! The server-materialised members of a MediaPackageV2 channel and origin endpoint.
//!
//! The rule under test, asserted over a SET of inputs rather than over one recorded
//! request: a member the service applies on the server side — the channel's
//! `InputSwitchConfiguration` / `OutputHeaderConfiguration`, the endpoint's `Segment`,
//! `StartoverWindowSeconds`, `UriSeparator` and `StreamNameOutputMode`, and each
//! manifest configuration's `ManifestWindowSeconds` / `UriPathType` /
//! `UrlEncodeChildManifest` — is reported with a CONCRETE value by every projection of
//! the resource (create, read and, for the endpoint-level modes, list): the value the
//! request named when it named one, and the service's own default when it did not.
//!
//! Every row differs from the others along what the rule generalises over: which members
//! the request names, which manifest family carries them, how many manifests there are,
//! and (for the constrained ones) where in the modelled range the value sits.

use serde_json::{json, Value};

// The crate's one emulator client, declared in the channel-lifecycle suite `pub(crate)`
// for exactly this reuse: every suite in this test target drives the emulator through it,
// and reads an absent-or-present member through the harness's `at`.
use crate::channel_lifecycle::{at, Emulator};

/// A channel group and a channel to hang an endpoint off, named for the case index.
async fn parent_chain(emulator: &Emulator, index: usize) -> (String, String) {
    let group = format!("group-{index}");
    let channel = format!("channel-{index}");
    let (status, body) = emulator.create_group(&group, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateChannelGroup for {group} answered {status} {body}"
    );
    let (status, body) = emulator.create_channel(&group, &channel, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateChannel for {channel} answered {status} {body}"
    );
    (group, channel)
}

/// One row of a projection table: a create body plus the `(pointer, value)` pairs every
/// projection of the created resource must report.
struct Case {
    label: &'static str,
    body: fn() -> Value,
    expected: Vec<(&'static str, Value)>,
}

/// Channel rows. Both configuration blocks are reported for every channel; the booleans
/// inside them default to `false` (the model's `MQCSInputSwitching` doc states "Default
/// is false") and are echoed when the request names them. `PreferredInput` has no
/// documented default and the service reports none, so it stays absent unless asked for;
/// its modelled `@range` is 1..2.
fn channel_cases() -> Vec<Case> {
    vec![
        Case {
            label: "no configuration blocks at all",
            body: || json!({}),
            expected: vec![
                ("/InputSwitchConfiguration/MQCSInputSwitching", json!(false)),
                ("/InputSwitchConfiguration/PreferredInput", Value::Null),
                ("/OutputHeaderConfiguration/PublishMQCS", json!(false)),
            ],
        },
        Case {
            label: "empty blocks: present but with no member set",
            body: || json!({"InputSwitchConfiguration": {}, "OutputHeaderConfiguration": {}}),
            expected: vec![
                ("/InputSwitchConfiguration/MQCSInputSwitching", json!(false)),
                ("/InputSwitchConfiguration/PreferredInput", Value::Null),
                ("/OutputHeaderConfiguration/PublishMQCS", json!(false)),
            ],
        },
        Case {
            label: "switching on, CMSD publication on, second input preferred",
            body: || {
                json!({
                    "InputType": "CMAF",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true, "PreferredInput": 2},
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
            expected: vec![
                ("/InputSwitchConfiguration/MQCSInputSwitching", json!(true)),
                ("/InputSwitchConfiguration/PreferredInput", json!(2)),
                ("/OutputHeaderConfiguration/PublishMQCS", json!(true)),
            ],
        },
        Case {
            label: "switching off explicitly, first input preferred",
            body: || {
                json!({
                    "InputType": "CMAF",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": false, "PreferredInput": 1},
                    "OutputHeaderConfiguration": {"PublishMQCS": false},
                })
            },
            expected: vec![
                ("/InputSwitchConfiguration/MQCSInputSwitching", json!(false)),
                ("/InputSwitchConfiguration/PreferredInput", json!(1)),
                ("/OutputHeaderConfiguration/PublishMQCS", json!(false)),
            ],
        },
    ]
}

/// Channel rows for the two members whose DEFAULT the model documents: `InputType` ("If
/// unprovided, the value defaults to HLS") and `OutputLockingMode` ("If you don't specify
/// a value, the default is `EPOCH_LOCKED`", on a channel whose input type may carry the
/// member at all).
///
/// The `InputType` catalogue is the model's own enum — HLS | CMAF | MULTIVIEW
/// (`smithy_query(query="shape", shape="InputType,OutputLockingMode")`) — and the rows
/// cover each of its values, present and absent, against a locking mode that is absent,
/// the default named explicitly, and the non-default one.
fn channel_default_cases() -> Vec<Case> {
    vec![
        Case {
            label: "nothing but a name: no input type, so the service applies its own",
            body: || json!({}),
            expected: vec![
                ("/InputType", json!("HLS")),
                // An HLS channel may not carry an output locking mode at all, so there is
                // no mode to default: the member stays absent.
                ("/OutputLockingMode", Value::Null),
            ],
        },
        Case {
            label: "a described, tagged channel that still names no input type",
            body: || json!({"Description": "ingest", "tags": {"team": "video"}}),
            expected: vec![
                ("/InputType", json!("HLS")),
                ("/OutputLockingMode", Value::Null),
            ],
        },
        Case {
            label: "the default input type named explicitly",
            body: || json!({"InputType": "HLS"}),
            expected: vec![
                ("/InputType", json!("HLS")),
                ("/OutputLockingMode", Value::Null),
            ],
        },
        Case {
            label: "CMAF naming no locking mode, so the service applies its own",
            body: || json!({"InputType": "CMAF"}),
            expected: vec![
                ("/InputType", json!("CMAF")),
                ("/OutputLockingMode", json!("EPOCH_LOCKED")),
            ],
        },
        Case {
            label: "CMAF naming the default locking mode explicitly",
            body: || json!({"InputType": "CMAF", "OutputLockingMode": "EPOCH_LOCKED"}),
            expected: vec![
                ("/InputType", json!("CMAF")),
                ("/OutputLockingMode", json!("EPOCH_LOCKED")),
            ],
        },
        Case {
            label: "CMAF naming the non-default locking mode",
            body: || json!({"InputType": "CMAF", "OutputLockingMode": "NON_EPOCH_LOCKED"}),
            expected: vec![
                ("/InputType", json!("CMAF")),
                ("/OutputLockingMode", json!("NON_EPOCH_LOCKED")),
            ],
        },
        Case {
            label: "a multiview channel naming no locking mode",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": ["feed-a", "feed-b"],
                        "AvailableLayouts": ["LAYOUT_2EH"],
                    },
                })
            },
            expected: vec![
                ("/InputType", json!("MULTIVIEW")),
                ("/OutputLockingMode", json!("EPOCH_LOCKED")),
            ],
        },
        Case {
            label: "a multiview channel over a single source and layout",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": ["feed-a"],
                        "AvailableLayouts": ["LAYOUT_4E"],
                    },
                })
            },
            expected: vec![
                ("/InputType", json!("MULTIVIEW")),
                // The documented default named explicitly is the only locking mode a
                // multiview channel may have, so it is also what one that names none
                // reports.
                ("/OutputLockingMode", json!("EPOCH_LOCKED")),
            ],
        },
    ]
}

/// Origin-endpoint rows.
///
/// The enum catalogues come from the model: `UriSeparator` is UNDERSCORE | HYPHEN,
/// `StreamNameOutputMode` is INDEX | PASSTHROUGH_NAME and `UriPathType` is LEAF | ROOT
/// (`smithy_query(query="shape", shape="UriSeparator,StreamNameOutputMode,UriPathType")`),
/// and the bounds come from the same place: `Segment.SegmentDurationSeconds` `@range`
/// 1..30, `StartoverWindowSeconds` `@range` 0..1209600, and
/// `CreateHlsManifestConfiguration.ManifestWindowSeconds` `@range` min 30. Each row
/// exercises a different part of that space, including values no recorded request used.
fn endpoint_cases() -> Vec<Case> {
    vec![
        Case {
            label: "nothing but the required members, one bare HLS manifest",
            body: || {
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}],
                })
            },
            expected: vec![
                ("/Segment/SegmentDurationSeconds", json!(6)),
                ("/Segment/SegmentName", json!("segment")),
                ("/Segment/TsUseAudioRenditionGroup", json!(true)),
                ("/Segment/TsIncludeDvbSubtitles", json!(true)),
                ("/Segment/IncludeIframeOnlyStreams", json!(false)),
                ("/StartoverWindowSeconds", json!(900)),
                ("/UriSeparator", json!("UNDERSCORE")),
                ("/StreamNameOutputMode", json!("INDEX")),
                ("/HlsManifests/0/ManifestWindowSeconds", json!(60)),
                ("/HlsManifests/0/UriPathType", json!("LEAF")),
                ("/HlsManifests/0/UrlEncodeChildManifest", json!(false)),
            ],
        },
        Case {
            label: "a CMAF container naming nothing but its single manifest",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                })
            },
            expected: vec![
                ("/Segment/SegmentDurationSeconds", json!(6)),
                ("/Segment/SegmentName", json!("segment")),
                ("/Segment/TsUseAudioRenditionGroup", json!(true)),
                ("/Segment/TsIncludeDvbSubtitles", json!(true)),
                ("/Segment/IncludeIframeOnlyStreams", json!(false)),
                ("/StartoverWindowSeconds", json!(900)),
                ("/UriSeparator", json!("UNDERSCORE")),
                ("/StreamNameOutputMode", json!("INDEX")),
            ],
        },
        Case {
            label: "every member named, at the far end of each enum and range",
            body: || {
                json!({
                    "ContainerType": "TS",
                    "Segment": {
                        "SegmentDurationSeconds": 30,
                        "SegmentName": "custom_seg-1",
                        "TsUseAudioRenditionGroup": false,
                        "TsIncludeDvbSubtitles": false,
                        "IncludeIframeOnlyStreams": true,
                    },
                    "StartoverWindowSeconds": 1_209_600,
                    "UriSeparator": "HYPHEN",
                    "StreamNameOutputMode": "PASSTHROUGH_NAME",
                    "HlsManifests": [{
                        "ManifestName": "index",
                        "ManifestWindowSeconds": 30,
                        "UriPathType": "ROOT",
                        "UrlEncodeChildManifest": true,
                    }],
                })
            },
            expected: vec![
                ("/Segment/SegmentDurationSeconds", json!(30)),
                ("/Segment/SegmentName", json!("custom_seg-1")),
                ("/Segment/TsUseAudioRenditionGroup", json!(false)),
                ("/Segment/TsIncludeDvbSubtitles", json!(false)),
                ("/Segment/IncludeIframeOnlyStreams", json!(true)),
                ("/StartoverWindowSeconds", json!(1_209_600)),
                ("/UriSeparator", json!("HYPHEN")),
                ("/StreamNameOutputMode", json!("PASSTHROUGH_NAME")),
                ("/HlsManifests/0/ManifestWindowSeconds", json!(30)),
                ("/HlsManifests/0/UriPathType", json!("ROOT")),
                ("/HlsManifests/0/UrlEncodeChildManifest", json!(true)),
            ],
        },
        Case {
            label: "the low bounds: a one-second segment and a zero startover window",
            body: || {
                json!({
                    "ContainerType": "TS",
                    "Segment": {"SegmentDurationSeconds": 1, "SegmentName": "s"},
                    "StartoverWindowSeconds": 0,
                    "HlsManifests": [{"ManifestName": "a"}, {"ManifestName": "b"}],
                })
            },
            expected: vec![
                ("/Segment/SegmentDurationSeconds", json!(1)),
                ("/Segment/SegmentName", json!("s")),
                // Untouched members of a submitted segment still get the service's value.
                ("/Segment/TsUseAudioRenditionGroup", json!(true)),
                ("/Segment/TsIncludeDvbSubtitles", json!(true)),
                ("/Segment/IncludeIframeOnlyStreams", json!(false)),
                ("/StartoverWindowSeconds", json!(0)),
                // Several manifests: every entry is materialised, not just the first.
                ("/HlsManifests/0/ManifestWindowSeconds", json!(60)),
                ("/HlsManifests/0/UriPathType", json!("LEAF")),
                ("/HlsManifests/0/UrlEncodeChildManifest", json!(false)),
                ("/HlsManifests/1/ManifestWindowSeconds", json!(60)),
                ("/HlsManifests/1/UriPathType", json!("LEAF")),
                ("/HlsManifests/1/UrlEncodeChildManifest", json!(false)),
            ],
        },
        Case {
            label: "the other manifest families, each with only a name",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "LowLatencyHlsManifests": [{"ManifestName": "ll"}],
                    "DashManifests": [{"ManifestName": "dash"}],
                })
            },
            expected: vec![
                ("/LowLatencyHlsManifests/0/ManifestWindowSeconds", json!(60)),
                ("/LowLatencyHlsManifests/0/UriPathType", json!("LEAF")),
                ("/LowLatencyHlsManifests/0/UrlEncodeChildManifest", json!(false)),
                ("/DashManifests/0/ManifestWindowSeconds", json!(60)),
                ("/DashManifests/0/UriPathType", json!("LEAF")),
            ],
        },
        Case {
            label: "MSS manifests, which model no URI path type: one bare, one with a window",
            body: || {
                json!({
                    "ContainerType": "ISM",
                    "MssManifests": [
                        {"ManifestName": "smooth"},
                        {"ManifestName": "smooth-hd", "ManifestWindowSeconds": 90},
                    ],
                })
            },
            expected: vec![
                ("/MssManifests/0/ManifestWindowSeconds", json!(60)),
                ("/MssManifests/1/ManifestWindowSeconds", json!(90)),
                ("/MssManifests/0/UriPathType", Value::Null),
            ],
        },
        Case {
            label: "named manifest windows and path types per family",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "LowLatencyHlsManifests": [{
                        "ManifestName": "ll",
                        "ManifestWindowSeconds": 45,
                        "UriPathType": "ROOT",
                        "UrlEncodeChildManifest": true,
                    }],
                    "DashManifests": [{
                        "ManifestName": "dash",
                        "ManifestWindowSeconds": 120,
                        "UriPathType": "ROOT",
                    }],
                })
            },
            expected: vec![
                ("/LowLatencyHlsManifests/0/ManifestWindowSeconds", json!(45)),
                ("/LowLatencyHlsManifests/0/UriPathType", json!("ROOT")),
                ("/LowLatencyHlsManifests/0/UrlEncodeChildManifest", json!(true)),
                ("/DashManifests/0/ManifestWindowSeconds", json!(120)),
                ("/DashManifests/0/UriPathType", json!("ROOT")),
            ],
        },
    ]
}

/// RULE — a channel reports its switching and CMSD-header state on create and on read,
/// whether the request named it or not.
#[tokio::test]
async fn a_channel_reports_the_switching_state_the_service_applied() {
    let emulator = Emulator::new();

    for (index, case) in channel_cases().into_iter().enumerate() {
        let label = case.label;
        let group = format!("switch-group-{index}");
        let channel = format!("switch-channel-{index}");
        let (status, body) = emulator.create_group(&group, json!({})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannelGroup expected 200, got {status} {body}"
        );

        let create_body = (case.body)();
        let (status, created) = emulator
            .create_channel(&group, &channel, create_body)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created}"
        );
        for (pointer, expected) in &case.expected {
            let actual = at(&created, pointer);
            assert_eq!(
                &actual, expected,
                "case {label:?}: CreateChannel {pointer} expected {expected}, got {actual}"
            );
        }

        let (status, read) = emulator.get_channel(&group, &channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        for (pointer, expected) in &case.expected {
            let actual = at(&read, pointer);
            assert_eq!(
                &actual, expected,
                "case {label:?}: GetChannel {pointer} expected {expected}, got {actual}"
            );
        }
    }
}

/// RULE — an origin endpoint reports the segment, startover, URI/stream-name and
/// manifest settings the service applied, on create, on read and (for the endpoint-level
/// modes) in the list projection.
#[tokio::test]
async fn an_origin_endpoint_reports_the_settings_the_service_applied() {
    let emulator = Emulator::new();

    for (index, case) in endpoint_cases().into_iter().enumerate() {
        let label = case.label;
        let (group, channel) = parent_chain(&emulator, index).await;
        let endpoint = format!("endpoint-{index}");

        let create_body = (case.body)();
        let (status, created) = emulator
            .create_endpoint(&group, &channel, &endpoint, create_body)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        for (pointer, expected) in &case.expected {
            let actual = at(&created, pointer);
            assert_eq!(
                &actual, expected,
                "case {label:?}: CreateOriginEndpoint {pointer} expected {expected}, got {actual}"
            );
        }

        let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
        );
        for (pointer, expected) in &case.expected {
            let actual = at(&read, pointer);
            assert_eq!(
                &actual, expected,
                "case {label:?}: GetOriginEndpoint {pointer} expected {expected}, got {actual}"
            );
        }

        // The list projection models the two endpoint-level modes and must report the
        // same concrete values the per-endpoint read reports — concrete, because an
        // absent member on BOTH sides is exactly the defect under test.
        let (status, listed) = emulator.list_endpoints(&group, &channel, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListOriginEndpoints expected 200, got {status} {listed}"
        );
        let item = at(&listed, "/Items/0");
        for member in ["UriSeparator", "StreamNameOutputMode"] {
            let actual = at(&item, &format!("/{member}"));
            let expected = at(&read, &format!("/{member}"));
            assert!(
                expected.is_string(),
                "case {label:?}: GetOriginEndpoint must report a concrete {member}, \
                 got {expected}"
            );
            assert_eq!(
                actual, expected,
                "case {label:?}: ListOriginEndpoints item {member} expected {expected}, \
                 got {actual}"
            );
        }
    }
}

/// RULE — a channel member the model documents a service-applied default for is
/// materialised ONCE, at create time, from the submitted value or that default, and
/// stored: every projection of the channel (the create response, a later read, an
/// update's response and the list item) reports the same concrete value, and reports one
/// even when the create request named none.
#[tokio::test]
async fn a_channel_reports_the_documented_defaults_the_service_applied() {
    let emulator = Emulator::new();

    for (index, case) in channel_default_cases().into_iter().enumerate() {
        let label = case.label;
        let group = format!("default-group-{index}");
        let channel = format!("default-channel-{index}");
        let (status, body) = emulator.create_group(&group, json!({})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannelGroup expected 200, got {status} {body}"
        );
        // A multiview row's available sources have to be CMAF channels of the same group
        // before the row's own channel can name them, so each named source is seeded.
        let body_json = (case.body)();
        let sources = body_json
            .pointer("/MultiviewConfiguration/AvailableSources")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for source in &sources {
            let source = source.as_str().unwrap_or_default();
            let (status, seeded) = emulator
                .create_channel(&group, source, json!({"InputType": "CMAF"}))
                .await;
            assert!(
                // A row may name the same source twice over its lists; 409 is it already
                // being there.
                status == 200 || status == 409,
                "case {label:?}: seeding source {source} expected 200 or 409, got {status} {seeded}"
            );
        }

        let (status, created) = emulator.create_channel(&group, &channel, body_json).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created}"
        );

        let (status, read) = emulator.get_channel(&group, &channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );

        // An update names neither member — `InputType` is immutable and
        // `OutputLockingMode` is not an `UpdateChannel` input at all — so its response
        // must still report what the create materialised.
        let (status, updated) = emulator
            .update_channel(&group, &channel, json!({}), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateChannel expected 200, got {status} {updated}"
        );

        let (status, listed) = emulator.list_channels(&group, "?maxResults=100").await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListChannels expected 200, got {status} {listed}"
        );
        // The group may hold the seeded CMAF sources beside the row's own channel, so the
        // list item is found by name rather than by position.
        let item = listed["Items"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["ChannelName"] == json!(channel))
                    .cloned()
            })
            .unwrap_or(Value::Null);
        assert!(
            item.is_object(),
            "case {label:?}: ListChannels must carry an item for {channel}, got {listed}"
        );

        for (projection, body) in [
            ("CreateChannel", &created),
            ("GetChannel", &read),
            ("UpdateChannel", &updated),
            ("ListChannels item", &item),
        ] {
            for (pointer, expected) in &case.expected {
                let actual = at(body, pointer);
                assert_eq!(
                    &actual, expected,
                    "case {label:?}: {projection} {pointer} expected {expected}, got {actual}"
                );
            }
            // The input type is on the channel whatever the request said, so no
            // projection of it may leave the member out.
            let input_type = at(body, "/InputType");
            assert!(
                input_type.is_string(),
                "case {label:?}: {projection} must report a concrete InputType, got \
                 {input_type}"
            );
        }
    }
}

/// RULE (the update half) — an update writes the whole configuration, so the members the
/// service applies are materialised again: an update naming none of them leaves the
/// endpoint reporting the service's own settings, and one naming them reports those.
#[tokio::test]
async fn an_update_re_materialises_the_settings_the_service_applies() {
    let emulator = Emulator::new();
    let (group, channel) = parent_chain(&emulator, 500).await;

    // Created with values that differ from every service default, so a default the update
    // re-applies cannot be mistaken for a leftover of the create.
    let (status, created) = emulator
        .create_endpoint(
            &group,
            &channel,
            "ep",
            json!({
                "ContainerType": "TS",
                "Segment": {
                    "SegmentDurationSeconds": 2,
                    "SegmentName": "created_seg",
                    "TsUseAudioRenditionGroup": false,
                    "TsIncludeDvbSubtitles": false,
                    "IncludeIframeOnlyStreams": true,
                },
                "StartoverWindowSeconds": 120,
                "UriSeparator": "HYPHEN",
                "HlsManifests": [{
                    "ManifestName": "index",
                    "ManifestWindowSeconds": 300,
                    "UriPathType": "ROOT",
                    "UrlEncodeChildManifest": true,
                }],
            }),
        )
        .await;
    assert_eq!(status, 200, "case \"setup\": got {status} {created}");

    // An update naming none of the server-applied members: each comes back as the
    // service's own value, not as an absent member and not as the created one.
    let (status, updated) = emulator
        .update_endpoint(
            &group,
            &channel,
            "ep",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
            }),
            &[],
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"an update naming none of them\": expected 200, got {status} {updated}"
    );
    let defaults: Vec<(&str, Value)> = vec![
        ("/Segment/SegmentDurationSeconds", json!(6)),
        ("/Segment/SegmentName", json!("segment")),
        ("/Segment/TsUseAudioRenditionGroup", json!(true)),
        ("/Segment/TsIncludeDvbSubtitles", json!(true)),
        ("/Segment/IncludeIframeOnlyStreams", json!(false)),
        ("/StartoverWindowSeconds", json!(900)),
        ("/StreamNameOutputMode", json!("INDEX")),
        ("/HlsManifests/0/ManifestWindowSeconds", json!(60)),
        ("/HlsManifests/0/UriPathType", json!("LEAF")),
        ("/HlsManifests/0/UrlEncodeChildManifest", json!(false)),
        // The model says this one survives an update that does not name it: "If you
        // don't specify a value in the update request, the current value is preserved".
        ("/UriSeparator", json!("HYPHEN")),
    ];
    let (_, read) = emulator.get_endpoint(&group, &channel, "ep").await;
    for (pointer, expected) in &defaults {
        for (projection, body) in [("UpdateOriginEndpoint", &updated), ("GetOriginEndpoint", &read)]
        {
            let actual = at(body, pointer);
            assert_eq!(
                &actual, expected,
                "case \"an update naming none of them\": {projection} {pointer} expected \
                 {expected}, got {actual}"
            );
        }
    }

    // An update that names them reports what it named.
    let (status, named) = emulator
        .update_endpoint(
            &group,
            &channel,
            "ep",
            json!({
                "ContainerType": "TS",
                "Segment": {"SegmentDurationSeconds": 10, "SegmentName": "updated-seg"},
                "StartoverWindowSeconds": 60,
                "UriSeparator": "UNDERSCORE",
                "HlsManifests": [{
                    "ManifestName": "index",
                    "ManifestWindowSeconds": 90,
                    "UriPathType": "ROOT",
                    "UrlEncodeChildManifest": true,
                }],
            }),
            &[],
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"an update naming them\": expected 200, got {status} {named}"
    );
    let named_values: Vec<(&str, Value)> = vec![
        ("/Segment/SegmentDurationSeconds", json!(10)),
        ("/Segment/SegmentName", json!("updated-seg")),
        // Untouched members of a submitted segment still get the service's value.
        ("/Segment/TsIncludeDvbSubtitles", json!(true)),
        ("/StartoverWindowSeconds", json!(60)),
        ("/UriSeparator", json!("UNDERSCORE")),
        ("/StreamNameOutputMode", json!("INDEX")),
        ("/HlsManifests/0/ManifestWindowSeconds", json!(90)),
        ("/HlsManifests/0/UriPathType", json!("ROOT")),
        ("/HlsManifests/0/UrlEncodeChildManifest", json!(true)),
    ];
    let (_, read) = emulator.get_endpoint(&group, &channel, "ep").await;
    for (pointer, expected) in &named_values {
        for (projection, body) in [("UpdateOriginEndpoint", &named), ("GetOriginEndpoint", &read)] {
            let actual = at(body, pointer);
            assert_eq!(
                &actual, expected,
                "case \"an update naming them\": {projection} {pointer} expected {expected}, \
                 got {actual}"
            );
        }
    }
}

/// A value one step past a modelled bound is refused by the shared validation walk
/// rather than stored and reported — the bounds are
/// `Segment.SegmentDurationSeconds` 1..30, `StartoverWindowSeconds` 0..1209600 and
/// `CreateHlsManifestConfiguration.ManifestWindowSeconds` min 30, and `UriSeparator` /
/// `StreamNameOutputMode` / `UriPathType` admit only the values their enums list.
#[tokio::test]
async fn a_value_past_a_modelled_bound_is_refused() {
    let emulator = Emulator::new();
    let (group, channel) = parent_chain(&emulator, 900).await;

    // Each body carries one manifest, which the service requires of any endpoint, so the
    // refusal under test is the bound and not the empty manifest set.
    let rejected: Vec<(&str, Value)> = vec![
        (
            "a segment one second longer than the maximum",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
                "Segment": {"SegmentDurationSeconds": 31},
            }),
        ),
        (
            "a segment one second shorter than the minimum",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
                "Segment": {"SegmentDurationSeconds": 0},
            }),
        ),
        (
            "a startover window one second past fourteen days",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
                "StartoverWindowSeconds": 1_209_601,
            }),
        ),
        (
            "a negative startover window",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
                "StartoverWindowSeconds": -1,
            }),
        ),
        (
            "a manifest window one second below the minimum",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index", "ManifestWindowSeconds": 29}],
            }),
        ),
        (
            "a URI separator the enum does not list",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
                "UriSeparator": "COLON",
            }),
        ),
        (
            "a stream-name output mode the enum does not list",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
                "StreamNameOutputMode": "PASSTHROUGH",
            }),
        ),
        (
            "a URI path type the enum does not list",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index", "UriPathType": "BRANCH"}],
            }),
        ),
    ];

    for (index, (label, body)) in rejected.into_iter().enumerate() {
        let name = format!("refused-{index}");
        let (status, answer) = emulator
            .create_endpoint(&group, &channel, &name, body)
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a 400 ValidationException, got {status} {answer}"
        );

        let (status, read) = emulator.get_endpoint(&group, &channel, &name).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a refused create must store nothing, got {status} {read}"
        );
    }
}

/// DASH rows for the manifest member whose DEFAULT the model documents inside its own
/// value list: `BandwidthReportingMode` ("`AVERAGE` – MediaPackage reports the average
/// bitrate of the source video track. This is the default").
///
/// The catalogue is the model's own enum — AVERAGE | MAX
/// (`smithy_query(query="shape", shape="DashBandwidthReportingMode")`) — and the rows
/// cover it absent, named as the default, and named as the other value, alone and beside
/// other DASH manifest members, over one manifest and over several at once, with the
/// neighbouring families asserted too: only the DASH shapes model the member.
fn dash_bandwidth_cases() -> Vec<Case> {
    vec![
        Case {
            label: "a bare DASH manifest: no mode named, so the service applies its own",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "DashManifests": [{"ManifestName": "dash"}],
                })
            },
            expected: vec![("/DashManifests/0/BandwidthReportingMode", json!("AVERAGE"))],
        },
        Case {
            label: "the default mode named explicitly",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "DashManifests": [{
                        "ManifestName": "dash",
                        "BandwidthReportingMode": "AVERAGE",
                    }],
                })
            },
            expected: vec![("/DashManifests/0/BandwidthReportingMode", json!("AVERAGE"))],
        },
        Case {
            label: "the peak-bitrate mode named",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "DashManifests": [{
                        "ManifestName": "dash",
                        "BandwidthReportingMode": "MAX",
                    }],
                })
            },
            expected: vec![("/DashManifests/0/BandwidthReportingMode", json!("MAX"))],
        },
        Case {
            label: "the mode beside the other DASH manifest settings",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "DashManifests": [{
                        "ManifestName": "dash",
                        "BandwidthReportingMode": "MAX",
                        "ManifestWindowSeconds": 120,
                        "UriPathType": "ROOT",
                        "Compactness": "NONE",
                        "DrmSignaling": "INDIVIDUAL",
                    }],
                })
            },
            expected: vec![
                ("/DashManifests/0/BandwidthReportingMode", json!("MAX")),
                ("/DashManifests/0/ManifestWindowSeconds", json!(120)),
                ("/DashManifests/0/UriPathType", json!("ROOT")),
                ("/DashManifests/0/Compactness", json!("NONE")),
                ("/DashManifests/0/DrmSignaling", json!("INDIVIDUAL")),
            ],
        },
        Case {
            label: "three DASH manifests: one bare, one default, one peak",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "DashManifests": [
                        {"ManifestName": "bare"},
                        {"ManifestName": "avg", "BandwidthReportingMode": "AVERAGE"},
                        {"ManifestName": "peak", "BandwidthReportingMode": "MAX"},
                    ],
                })
            },
            expected: vec![
                ("/DashManifests/0/BandwidthReportingMode", json!("AVERAGE")),
                ("/DashManifests/1/BandwidthReportingMode", json!("AVERAGE")),
                ("/DashManifests/2/BandwidthReportingMode", json!("MAX")),
            ],
        },
        Case {
            label: "DASH beside the families that do not model the member",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "hls"}],
                    "LowLatencyHlsManifests": [{"ManifestName": "ll"}],
                    "DashManifests": [{"ManifestName": "dash"}],
                })
            },
            expected: vec![
                ("/DashManifests/0/BandwidthReportingMode", json!("AVERAGE")),
                ("/HlsManifests/0/BandwidthReportingMode", Value::Null),
                (
                    "/LowLatencyHlsManifests/0/BandwidthReportingMode",
                    Value::Null,
                ),
            ],
        },
    ]
}

/// RULE — a DASH manifest configuration reports a CONCRETE bandwidth reporting mode in
/// every projection of the endpoint: the value the request named, and the service's own
/// documented default when it named none. An update rewrites the whole configuration, so
/// it materialises the member again.
#[tokio::test]
async fn a_dash_manifest_reports_the_bandwidth_reporting_mode_the_service_applied() {
    let emulator = Emulator::new();

    for (index, case) in dash_bandwidth_cases().into_iter().enumerate() {
        let label = case.label;
        let (group, channel) = parent_chain(&emulator, 700 + index).await;
        let endpoint = format!("bandwidth-{index}");

        let create_body = (case.body)();
        let (status, created) = emulator
            .create_endpoint(&group, &channel, &endpoint, create_body.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );

        let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
        );

        // The same body sent as an update: the member is materialised by the write, so
        // the update's own response and the read after it report the same value.
        let (status, updated) = emulator
            .update_endpoint(&group, &channel, &endpoint, create_body, &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );
        let (status, read_after_update) = emulator.get_endpoint(&group, &channel, &endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint after the update expected 200, got \
             {status} {read_after_update}"
        );

        for (projection, body) in [
            ("CreateOriginEndpoint", &created),
            ("GetOriginEndpoint", &read),
            ("UpdateOriginEndpoint", &updated),
            ("GetOriginEndpoint after the update", &read_after_update),
        ] {
            for (pointer, expected) in &case.expected {
                let actual = at(body, pointer);
                assert_eq!(
                    &actual, expected,
                    "case {label:?}: {projection} {pointer} expected {expected}, got {actual}"
                );
            }
        }
    }
}

/// RULE (the write-side half) — an update that drops the member re-applies the service's
/// default instead of leaving the previously stored value or an absent member, and a mode
/// the enum does not list is refused with nothing stored.
#[tokio::test]
async fn an_update_re_materialises_the_dash_bandwidth_reporting_mode() {
    let emulator = Emulator::new();
    let (group, channel) = parent_chain(&emulator, 710).await;

    // Created with the non-default mode, so the default a later update re-applies cannot
    // be mistaken for a leftover of the create.
    let (status, created) = emulator
        .create_endpoint(
            &group,
            &channel,
            "ep",
            json!({
                "ContainerType": "CMAF",
                "DashManifests": [{
                    "ManifestName": "dash",
                    "BandwidthReportingMode": "MAX",
                }],
            }),
        )
        .await;
    assert_eq!(status, 200, "case \"setup\": got {status} {created}");

    let (status, updated) = emulator
        .update_endpoint(
            &group,
            &channel,
            "ep",
            json!({
                "ContainerType": "CMAF",
                "DashManifests": [{"ManifestName": "dash"}],
            }),
            &[],
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"an update naming no mode\": expected 200, got {status} {updated}"
    );
    let (_, read) = emulator.get_endpoint(&group, &channel, "ep").await;
    for (projection, body) in [("UpdateOriginEndpoint", &updated), ("GetOriginEndpoint", &read)] {
        let actual = at(body, "/DashManifests/0/BandwidthReportingMode");
        assert_eq!(
            actual,
            json!("AVERAGE"),
            "case \"an update naming no mode\": {projection} expected AVERAGE, got {actual}"
        );
    }

    // A mode outside the modelled enum is refused, and the stored endpoint keeps the mode
    // the last accepted write materialised.
    let (status, answer) = emulator
        .update_endpoint(
            &group,
            &channel,
            "ep",
            json!({
                "ContainerType": "CMAF",
                "DashManifests": [{
                    "ManifestName": "dash",
                    "BandwidthReportingMode": "MAXIMUM",
                }],
            }),
            &[],
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a mode the enum does not list\": expected a 400 ValidationException, got \
         {status} {answer}"
    );
    let (_, read) = emulator.get_endpoint(&group, &channel, "ep").await;
    let actual = at(&read, "/DashManifests/0/BandwidthReportingMode");
    assert_eq!(
        actual,
        json!("AVERAGE"),
        "case \"a mode the enum does not list\": a refused update must store nothing, got \
         {actual}"
    );
}
