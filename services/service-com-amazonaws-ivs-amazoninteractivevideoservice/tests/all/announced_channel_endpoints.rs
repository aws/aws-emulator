// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two ADDRESSES every channel this service reports announces,
//! driven over the wire through `MultiServiceRouter::dispatch` on the harness
//! `channel_lifecycle` owns.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. Every address this emulator hands a caller names the address that caller reached
//!    this emulator at — never a host belonging to someone else. So a channel's
//!    `playbackUrl` is `https://<the address the request was answered on>` followed by the
//!    playlist path AWS lays out, `/api/video/v1/<region>.<account>.channel.<id>.m3u8`,
//!    with the Region, the account and the channel id taken from the channel's own ARN,
//!    and a channel's `ingestEndpoint` (with the `srt.endpoint` that equals it) is that
//!    same address, which is the only one an encoder on this machine can contribute to.
//! 2. Every projection of one channel agrees: the create, the single read, the batch read,
//!    the update and the live stream read off that channel all announce the same addresses
//!    for the same caller, and a read announces its own caller's address without changing
//!    what a later caller is told.
//! 3. The ingest server is the ACCOUNT's, not a channel's: every channel of the account
//!    reports the SAME `ingestEndpoint`, whatever Region its credential scope names and
//!    whatever the channel's own id is, and a read or an update does not move it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state;

use super::channel_lifecycle::{text, Ivs, AUTHORIZATION};

/// The playlist path AWS serves a channel's master manifest at, ahead of the channel's
/// identity: `Channel$playbackUrl` is the "Channel playback URL" and `Stream$playbackUrl`
/// the "URL of the master playlist, required by the video player to play the HLS stream"
/// (`smithy_query(query="shape",shape="Channel,Stream")`); neither shape constrains the
/// value, so the path AWS's recorded playback URLs carry is the form under test.
const PLAYBACK_PATH_PREFIX: &str = "/api/video/v1";

/// The suffix of that path.
const PLAYBACK_PATH_SUFFIX: &str = ".m3u8";

/// Hosts no address this emulator announces may name: a value on one of these leaves the
/// sandbox, because nothing here answers on an Amazon production host.
const FOREIGN_DOMAINS: [&str; 2] = ["live-video.net", "amazonaws.com"];

/// A SigV4 `Authorization` header in `region`'s credential scope, which is how a request
/// names the Region it is addressed to (and therefore the Region its resources are minted
/// in).
fn authorization_in(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=\
         0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The Region, the account and the channel id a `ChannelArn` names
/// (`arn:aws:ivs:<region>:<account>:channel/<id>`).
fn channel_of_arn(arn: &str) -> (String, String, String) {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "a channel ARN has six colon-separated segments, got {arn:?}"
    );
    let id = segments[5]
        .strip_prefix("channel/")
        .unwrap_or_else(|| panic!("a channel ARN names a channel resource, got {arn:?}"));
    (
        segments[3].to_string(),
        segments[4].to_string(),
        id.to_string(),
    )
}

/// The authority (`host[:port]`) of an absolute URL, and the path under it.
fn split_url(url: &str) -> (String, String) {
    let after_scheme = url
        .split_once("://")
        .unwrap_or_else(|| panic!("a playback URL is absolute, got {url:?}"))
        .1;
    match after_scheme.find('/') {
        Some(end) => (
            after_scheme[..end].to_string(),
            after_scheme[end..].to_string(),
        ),
        None => (after_scheme.to_string(), String::new()),
    }
}

/// The URL rule 1 states for the channel `arn` names, announced to a caller answered at
/// `authority`.
fn expected_playback_url(authority: &str, arn: &str) -> String {
    let (region, account, id) = channel_of_arn(arn);
    format!(
        "https://{authority}{PLAYBACK_PATH_PREFIX}/{region}.{account}.channel.{id}\
         {PLAYBACK_PATH_SUFFIX}"
    )
}

/// Rule 1 on one announced URL: it is the URL the rule states, it is `https`, its host is
/// the address the caller was answered on, and it is no foreign host's.
fn assert_announced(case: &str, member: &str, url: &str, authority: &str, arn: &str) {
    assert_eq!(
        url,
        expected_playback_url(authority, arn),
        "case {case:?}: {member} must be the playlist URL on the address this caller \
         reached the emulator at ({authority:?}), got {url:?}"
    );
    let (host, path) = split_url(url);
    assert_eq!(
        host, authority,
        "case {case:?}: {member} must name the address this caller reached the emulator \
         at, got host {host:?} in {url:?}"
    );
    for domain in FOREIGN_DOMAINS {
        assert!(
            !host.ends_with(domain),
            "case {case:?}: {member} must not send a client to a {domain} host, got \
             {url:?}"
        );
    }
    assert!(
        path.starts_with(PLAYBACK_PATH_PREFIX) && path.ends_with(PLAYBACK_PATH_SUFFIX),
        "case {case:?}: {member} must carry the playlist path AWS lays out, got {path:?}"
    );
}

/// One addressing a client reaches this emulator through: the base the emulator is served
/// at, the `Host` the client sent, and the address a URL announced to it must name.
struct Addressing {
    case: &'static str,
    /// The base `OperationContext::endpoint` reports, or `None` for an emulator served on
    /// no port at all (the harness default).
    served_at: Option<&'static str>,
    /// The host the client addressed, or `None` for a client that names only this
    /// service's published endpoint on the request URI.
    host: Option<&'static str>,
    /// The `host[:port]` every URL announced to that client must be on.
    authority: &'static str,
}

/// The addressings this suite spans: rows differ along what rule 1 generalises over — how
/// the client addressed this emulator (an address with a port, a name with a port, an IPv6
/// literal, one of AWS's own hostnames, none at all) and whether this emulator is served
/// on a port.
fn addressings() -> Vec<Addressing> {
    vec![
        Addressing {
            case: "an SDK client naming this service's endpoint, on an unserved emulator",
            served_at: None,
            host: None,
            // `smithy_server_runtime::UNSERVED_URL_BASE`: the one base an emulator that
            // is served on no port reports, and still this emulator's own.
            authority: "emulator.invalid",
        },
        Addressing {
            case: "a client on the loopback address",
            served_at: None,
            host: Some("127.0.0.1:9297"),
            authority: "127.0.0.1:9297",
        },
        Addressing {
            case: "a client naming the loopback by name, on another port",
            served_at: None,
            host: Some("localhost:40305"),
            authority: "localhost:40305",
        },
        Addressing {
            case: "a client on an IPv6 literal",
            served_at: None,
            host: Some("[::1]:9297"),
            authority: "[::1]:9297",
        },
        Addressing {
            case: "a client that resolves AWS's own hosts, on an emulator served on a port",
            served_at: Some("https://127.0.0.1:40305"),
            host: Some("ivs.us-east-1.amazonaws.com"),
            authority: "127.0.0.1:40305",
        },
        Addressing {
            case: "an SDK client on an emulator served at a name",
            served_at: Some("https://emulator.test:8443"),
            host: None,
            authority: "emulator.test:8443",
        },
        Addressing {
            case: "a client on the loopback address of an emulator served elsewhere",
            served_at: Some("https://127.0.0.1:40305"),
            host: Some("localhost:9297"),
            authority: "localhost:9297",
        },
    ]
}

/// The creates this suite spans, so the rules are asserted over a set of channels rather
/// than one: rows differ along the channel type, the latency mode, the flags, the name and
/// the tags — none of which the announced addresses depend on.
fn create_requests() -> Vec<(&'static str, Value)> {
    vec![
        ("every member defaulted", json!({})),
        ("name only", json!({ "name": "announced" })),
        (
            "basic, normal latency, insecure ingest",
            json!({ "name": "basic_1", "type": "BASIC", "latencyMode": "NORMAL",
                    "insecureIngest": true }),
        ),
        (
            "advanced-hd, authorized, fragmented mp4, tagged",
            json!({ "name": "advanced-hd", "type": "ADVANCED_HD", "authorized": true,
                    "containerFormat": "FRAGMENTED_MP4", "tags": { "env": "prod" } }),
        ),
        ("an empty name", json!({ "name": "" })),
    ]
}

/// Rule 1 on a channel's INGEST address: it is the address the caller was answered on, and
/// the `srt.endpoint` beside it is the same value — one address serves both protocols.
fn assert_announced_ingest(case: &str, member: &str, channel: &Value, authority: &str) {
    let ingest = text(channel, "ingestEndpoint");
    assert_eq!(
        ingest, authority,
        "case {case:?}: {member} ingestEndpoint must be the address this caller reached \
         the emulator at ({authority:?}), got {ingest:?}"
    );
    for domain in FOREIGN_DOMAINS {
        assert!(
            !ingest.ends_with(domain),
            "case {case:?}: {member} ingestEndpoint must not send an encoder to a \
             {domain} host, got {ingest:?}"
        );
    }
    assert_eq!(
        text(&channel["srt"], "endpoint"),
        ingest,
        "case {case:?}: {member} srt.endpoint must be the channel's ingest endpoint, got {}",
        channel["srt"]
    );
}

/// Rules 1 and 2 over the addressing table: every operation that projects a channel
/// announces both addresses on the one its own caller reached this emulator at.
#[tokio::test]
async fn every_channel_projection_announces_the_playback_url_on_the_callers_own_address() {
    for addressing in addressings() {
        let Addressing {
            case,
            served_at,
            host,
            authority,
        } = addressing;
        let ivs = match served_at {
            Some(base) => Ivs::served_at(base),
            None => Ivs::new(),
        };
        let call = |operation: &'static str, body: Value| {
            let ivs = &ivs;
            async move {
                let reply = ivs.call_as(operation, &body, host, AUTHORIZATION).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: {operation} must answer HTTP 200, got {} {}",
                    reply.status, reply.body
                );
                reply.body
            }
        };

        // Rule 1 on the response a client caches at creation time.
        let created = call("CreateChannel", json!({ "name": "announced" })).await;
        let channel = &created["channel"];
        let arn = text(channel, "arn");
        assert_announced(
            case,
            "CreateChannel $.channel.playbackUrl",
            &text(channel, "playbackUrl"),
            authority,
            &arn,
        );
        assert_announced_ingest(case, "CreateChannel $.channel", channel, authority);

        // Rule 2: every other projection of the same channel agrees, for the same caller.
        let read = call("GetChannel", json!({ "arn": arn })).await;
        assert_announced(
            case,
            "GetChannel $.channel.playbackUrl",
            &text(&read["channel"], "playbackUrl"),
            authority,
            &arn,
        );
        assert_announced_ingest(case, "GetChannel $.channel", &read["channel"], authority);
        let updated = call("UpdateChannel", json!({ "arn": arn, "name": "renamed" })).await;
        assert_announced(
            case,
            "UpdateChannel $.channel.playbackUrl",
            &text(&updated["channel"], "playbackUrl"),
            authority,
            &arn,
        );
        assert_announced_ingest(
            case,
            "UpdateChannel $.channel",
            &updated["channel"],
            authority,
        );
        let batch = call("BatchGetChannel", json!({ "arns": [arn] })).await;
        let batched = &batch["channels"][0];
        assert_announced(
            case,
            "BatchGetChannel $.channels[0].playbackUrl",
            &text(batched, "playbackUrl"),
            authority,
            &arn,
        );
        assert_announced_ingest(case, "BatchGetChannel $.channels[0]", batched, authority);

        // The live stream is played back from its channel's own URL, so the same rule
        // holds for the stream a broadcasting channel reports.
        ivs.with_store_mut(|store| state::start_stream(store, &arn));
        let stream = call("GetStream", json!({ "channelArn": arn })).await;
        assert_announced(
            case,
            "GetStream $.stream.playbackUrl",
            &text(&stream["stream"], "playbackUrl"),
            authority,
            &arn,
        );
    }
}

/// Rule 2's other half: a read announces its OWN caller's address and changes nothing, so
/// a second caller reached at another address is answered its own, and the first caller
/// still gets what it got before.
#[tokio::test]
async fn a_read_through_one_address_does_not_change_what_another_caller_is_told() {
    let ivs = Ivs::new();
    let created = ivs
        .call_as("CreateChannel", &json!({ "name": "shared" }), None, AUTHORIZATION)
        .await;
    let arn = text(&created.body["channel"], "arn");

    // Rows: the addresses two clients of one emulator reach it at, each read twice so the
    // second read of the first address proves the first read of the second changed nothing.
    let addresses = ["127.0.0.1:9297", "localhost:40305", "[::1]:8443"];
    for round in 1..=2 {
        for host in addresses {
            let case = format!("round {round} at {host}");
            let read = ivs
                .call_as("GetChannel", &json!({ "arn": arn }), Some(host), AUTHORIZATION)
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetChannel must answer HTTP 200, got {} {}",
                read.status, read.body
            );
            assert_announced(
                &case,
                "GetChannel $.channel.playbackUrl",
                &text(&read.body["channel"], "playbackUrl"),
                host,
                &arn,
            );
        }
    }
}

/// Rule 3: the ingest server is the ACCOUNT's, not a channel's — one address for every
/// channel of the account whatever Region its caller's credential scope names, carrying no
/// part of the channel's own identity, and unmoved by a read or an update.
#[tokio::test]
async fn the_ingest_endpoint_is_the_accounts_and_not_a_channels() {
    let ivs = Ivs::new();
    // Rows: the Regions a caller's credential scope can name — ordinary Region codes and
    // the shapes around them (a one-character one, a digit-bearing one) — each with the
    // whole create table driven through it. None of them may change the address, because
    // this one deployment is what every channel of the account contributes to.
    let regions = [
        "us-east-1",
        "us-west-2",
        "ap-southeast-2",
        "a",
        "us-gov-west-1",
        "x1-2-3",
    ];
    let mut announced: BTreeSet<String> = BTreeSet::new();

    for region in regions {
        let authorization = authorization_in(region);
        for (row, request) in create_requests() {
            let case = format!("{region}: {row}");
            let created = ivs
                .call_as("CreateChannel", &request, None, &authorization)
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateChannel must answer HTTP 200, got {} {}",
                created.status, created.body
            );
            let channel = &created.body["channel"];
            let arn = text(channel, "arn");
            let ingest = text(channel, "ingestEndpoint");
            assert_announced_ingest(&case, "CreateChannel $.channel", channel, &ingest);

            // The ingest server is not the channel: nothing of the channel's own identity
            // is in the address it contributes to.
            let (_, _, channel_id) = channel_of_arn(&arn);
            assert!(
                !ingest.contains(&channel_id) && !ingest.contains(&arn),
                "case {case:?}: the ingest endpoint must carry no part of the channel's \
                 own identity, got {ingest:?} for channel {channel_id:?}"
            );

            // Rule 3 on every other projection of the same channel: a read, an update and
            // a batch read all report the address the create reported.
            let read = ivs
                .call_as("GetChannel", &json!({ "arn": arn }), None, &authorization)
                .await;
            assert_eq!(
                text(&read.body["channel"], "ingestEndpoint"),
                ingest,
                "case {case:?}: GetChannel must answer the ingest endpoint the create \
                 reported, got {}",
                read.body["channel"]
            );
            let updated = ivs
                .call_as(
                    "UpdateChannel",
                    &json!({ "arn": arn, "latencyMode": "NORMAL" }),
                    None,
                    &authorization,
                )
                .await;
            assert_eq!(
                text(&updated.body["channel"], "ingestEndpoint"),
                ingest,
                "case {case:?}: an update must not move the account's ingest endpoint, \
                 got {}",
                updated.body["channel"]
            );
            let batch = ivs
                .call_as(
                    "BatchGetChannel",
                    &json!({ "arns": [arn] }),
                    None,
                    &authorization,
                )
                .await;
            assert_eq!(
                text(&batch.body["channels"][0], "ingestEndpoint"),
                ingest,
                "case {case:?}: a batch read must answer the same ingest endpoint as the \
                 single read, got {}",
                batch.body["channels"][0]
            );

            announced.insert(ingest);
        }
    }

    assert_eq!(
        announced.len(),
        1,
        "every channel of this account ingests on ONE address, whatever Region its caller \
         named, got {announced:?}"
    );
}

/// Rule 1 on the ingest address, over the same addressings as the playback URL: a client
/// is told the ingest server at the address IT reached this emulator at, so an encoder
/// configured from the answer dials this machine and nothing else.
#[tokio::test]
async fn the_ingest_endpoint_follows_the_address_each_caller_reached_the_emulator_at() {
    for addressing in addressings() {
        let Addressing {
            case,
            served_at,
            host,
            authority,
        } = addressing;
        let ivs = match served_at {
            Some(base) => Ivs::served_at(base),
            None => Ivs::new(),
        };
        // The channel is created through ONE addressing and read through this one, so what
        // the read answers cannot have come from the create's own request.
        let created = ivs
            .call_as(
                "CreateChannel",
                &json!({ "name": "ingest" }),
                Some("127.0.0.1:9297"),
                AUTHORIZATION,
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateChannel must answer HTTP 200, got {} {}",
            created.status, created.body
        );
        let arn = text(&created.body["channel"], "arn");
        let read = ivs
            .call_as("GetChannel", &json!({ "arn": arn }), host, AUTHORIZATION)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetChannel must answer HTTP 200, got {} {}",
            read.status, read.body
        );
        assert_announced_ingest(case, "GetChannel $.channel", &read.body["channel"], authority);
    }
}
