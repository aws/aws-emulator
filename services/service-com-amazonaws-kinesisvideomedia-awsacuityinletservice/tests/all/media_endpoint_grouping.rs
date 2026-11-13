// SPDX-License-Identifier: CC0-1.0

//! WHICH announced origin serves a stream's media APIs, over both routes of this
//! endpoint.
//!
//! The rule under test: `GetDataEndpoint` reports, per `APIName`, the address of the media
//! ENDPOINT that binds the API it names — this one, Kinesis Video Media, which binds
//! `GetMedia` and the recorded `PutMedia` ingest beside it, or the archived-media endpoint,
//! which binds the APIs that read a stream's STORED fragments. So `PUT_MEDIA` and
//! `GET_MEDIA` resolve to ONE origin, which serves both `/putMedia` and `/getMedia`; the
//! archived-media `APIName`s resolve to a DIFFERENT origin, which binds neither of those
//! two routes and answers each as the unrouted request it is; and neither origin is the
//! control-plane endpoint the discovery call was made to. Kinesis Video Media's
//! `InvalidEndpointException` is why the distinction is observable from a client at all:
//! "Caller used wrong endpoint to write data to a stream. On receiving such an exception,
//! the user must call `GetDataEndpoint` with `AccessMode` set to \"READ\"".
//!
//! The first table's rows differ in WHICH of the two `APIName`s each leg of one session
//! discovered its origin through — including the crossed cases, where the ingest is sent to
//! the origin found for the live read and the read to the origin found for the ingest — and
//! in how many fragments the session carries. The second table's rows differ in the
//! archived-media `APIName` the other origin was discovered through: every value the
//! archived-media endpoint's model accounts for, not only the ones a producer commonly
//! uses.

use std::time::{Duration, SystemTime};

use crate::ingest_plane_addressing::{event_types, status, text, Harness};
use crate::put_media_ingest::{cluster, ingest_headers, Container};
use crate::read_plane_addressing::unknown_operation_envelope;

/// This emulator's own front door, where a client on this machine makes its control calls
/// and where the planes are therefore announced from.
const CONTROL: &str = "127.0.0.1:9297";

/// The `APIName` values this media endpoint serves: the ingest front end and the live read.
/// From the Kinesis Video Media model's bound operations (`smithy_query(query="digest")`,
/// which binds `GetMedia`) plus the recorded `PutMedia` ingest its own documentation names.
const LIVE_MEDIA_APIS: &[&str] = &["PUT_MEDIA", "GET_MEDIA"];

/// The `APIName` values the ARCHIVED-media endpoint serves, one per operation that model
/// binds (`smithy_query(query="digest")` on the Kinesis Video Archived Media model:
/// `GetClip`, `GetDASHStreamingSessionURL`, `GetHLSStreamingSessionURL`, `GetImages`,
/// `GetMediaForFragmentList`, `ListFragments`).
const ARCHIVED_MEDIA_APIS: &[&str] = &[
    "GET_CLIP",
    "GET_DASH_STREAMING_SESSION_URL",
    "GET_HLS_STREAMING_SESSION_URL",
    "GET_IMAGES",
    "GET_MEDIA_FOR_FRAGMENT_LIST",
    "LIST_FRAGMENTS",
];

/// A producer clock a session's RELATIVE Cluster timecodes are counted from.
fn producer_start() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_000_000)
}

/// A `GetMedia` body reading one stream from the oldest chunk it holds.
fn read_from_the_start(name: &str) -> serde_json::Value {
    serde_json::json!({
        "StreamName": name,
        "StartSelector": { "StartSelectorType": "EARLIEST" },
    })
}

/// Whether `payload` carries `frame` as a contiguous run of bytes.
fn carries(payload: &[u8], frame: &[u8]) -> bool {
    !frame.is_empty() && payload.windows(frame.len()).any(|window| window == frame)
}

/// One row of the first table: which `APIName` each leg of the session discovered its
/// origin through, and how many Clusters the producer submits.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The `APIName` the producer discovered the origin it POSTs `/putMedia` to through.
    ingest_through: &'static str,
    /// The `APIName` the reader discovered the origin it POSTs `/getMedia` to through.
    read_through: &'static str,
    clusters: usize,
}

/// The rows: each of the two `APIName`s of this endpoint used for each leg, so one host
/// serving both legs is asserted whichever name found it, and a session of several
/// fragments beside one of a single fragment.
const ROWS: &[Row] = &[
    Row {
        case: "both legs discovered through PUT_MEDIA",
        ingest_through: "PUT_MEDIA",
        read_through: "PUT_MEDIA",
        clusters: 1,
    },
    Row {
        case: "both legs discovered through GET_MEDIA",
        ingest_through: "GET_MEDIA",
        read_through: "GET_MEDIA",
        clusters: 2,
    },
    Row {
        case: "ingest discovered through GET_MEDIA, read through PUT_MEDIA",
        ingest_through: "GET_MEDIA",
        read_through: "PUT_MEDIA",
        clusters: 3,
    },
    Row {
        case: "ingest discovered through PUT_MEDIA, read through GET_MEDIA",
        ingest_through: "PUT_MEDIA",
        read_through: "GET_MEDIA",
        clusters: 2,
    },
];

/// RULE: `PUT_MEDIA` and `GET_MEDIA` are two APIs of ONE media endpoint, so they resolve to
/// one origin and that origin serves both `/putMedia` and `/getMedia` — a session ingested
/// at the address either name found is read back, frame for frame, at the address either
/// name found.
#[tokio::test]
async fn one_announced_origin_serves_both_routes_of_this_media_endpoint() {
    let harness = Harness::new();

    for (index, row) in ROWS.iter().enumerate() {
        let Row {
            case,
            ingest_through,
            read_through,
            clusters,
        } = *row;
        let name = format!("grouped-{index}");
        harness.create_stream(CONTROL, &name).await;

        let ingest_origin = harness
            .announced_authority(CONTROL, &name, ingest_through)
            .await;
        let read_origin = harness
            .announced_authority(CONTROL, &name, read_through)
            .await;
        assert_eq!(
            ingest_origin, read_origin,
            "case {case:?}: the APIs of one media endpoint resolve to one origin, \
             got {ingest_origin:?} for {ingest_through} and {read_origin:?} for {read_through}"
        );
        assert_ne!(
            ingest_origin, CONTROL,
            "case {case:?}: the media origin is not the control-plane endpoint the discovery \
             call was made to, got {ingest_origin:?}"
        );

        let submitted: Vec<_> = (0..clusters)
            .map(|cluster_index| {
                cluster(
                    (cluster_index as u64) * 1_000,
                    2,
                    20,
                    0x90 + cluster_index as u8,
                )
            })
            .collect();
        let container = Container::of(submitted.clone()).bytes();
        let headers = ingest_headers(
            ("x-amzn-stream-name", name.clone()),
            "RELATIVE",
            producer_start(),
        );
        let ingested = harness
            .put_media_at(&ingest_origin, &headers, container)
            .await;
        assert_eq!(
            status(&ingested),
            200,
            "case {case:?} ({ingest_origin}): expected the ingest route to be served at the \
             origin {ingest_through} announced, got {} {}",
            status(&ingested),
            text(&ingested)
        );
        let persisted = event_types(&ingested)
            .into_iter()
            .filter(|event| event == "PERSISTED")
            .count();
        assert_eq!(
            persisted, clusters,
            "case {case:?}: expected one PERSISTED acknowledgement per submitted Cluster, \
             got {persisted} in {}",
            text(&ingested)
        );

        let read = harness
            .call_at(&read_origin, "/getMedia", &read_from_the_start(&name))
            .await;
        assert_eq!(
            status(&read),
            200,
            "case {case:?} ({read_origin}): expected the live read to be served at the origin \
             {read_through} announced, got {} {}",
            status(&read),
            text(&read)
        );
        let payload = read.body().to_vec();
        for (cluster_index, submitted) in submitted.iter().enumerate() {
            for (frame_index, frame) in submitted.frames().iter().enumerate() {
                assert!(
                    carries(&payload, frame),
                    "case {case:?}: the read at {read_origin} must carry frame {frame_index} of \
                     Cluster {cluster_index} ({frame:?}), got {} payload bytes",
                    payload.len()
                );
            }
        }
    }
}

/// RULE: the archived-media `APIName`s resolve to a DIFFERENT origin, which binds neither
/// of this endpoint's two routes: `/putMedia` sent there is the 404 an endpoint gives a
/// path it binds no route for and ingests nothing, `/getMedia` sent there is the front
/// door's unknown-operation answer, while that origin does serve the archive listing it
/// was announced for.
#[tokio::test]
async fn the_archived_media_origin_binds_neither_route_of_this_media_endpoint() {
    let harness = Harness::new();
    let name = "archive-scoped";
    harness.create_stream(CONTROL, name).await;
    let media_origin = harness.announced_authority(CONTROL, name, "PUT_MEDIA").await;

    for api_name in ARCHIVED_MEDIA_APIS {
        let origin = harness.announced_authority(CONTROL, name, api_name).await;
        assert_ne!(
            origin, media_origin,
            "case {api_name:?}: an API the archived-media endpoint binds resolves to that \
             endpoint's own origin, got the live media one ({media_origin:?})"
        );
        assert_ne!(
            origin, CONTROL,
            "case {api_name:?}: the archive origin is not the control-plane endpoint, \
             got {origin:?}"
        );

        let headers = ingest_headers(
            ("x-amzn-stream-name", name.to_string()),
            "RELATIVE",
            producer_start(),
        );
        let ingest = harness
            .put_media_at(
                &origin,
                &headers,
                Container::of(vec![cluster(0, 2, 20, 0x61)]).bytes(),
            )
            .await;
        assert_eq!(
            status(&ingest),
            404,
            "case {api_name:?} ({origin}): the archive origin binds no ingest route, got {} {}",
            status(&ingest),
            text(&ingest)
        );
        assert_eq!(
            harness.archived_fragments(CONTROL, name).await,
            0,
            "case {api_name:?}: a refused ingest archives nothing"
        );

        let read = harness
            .call_at(&origin, "/getMedia", &read_from_the_start(name))
            .await;
        assert_eq!(
            status(&read),
            400,
            "case {api_name:?} ({origin}): the archive origin binds no live read, got {} {}",
            status(&read),
            text(&read)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(read.body())
                .unwrap_or(serde_json::Value::Null),
            unknown_operation_envelope(),
            "case {api_name:?} ({origin}): expected the front door's unknown-operation \
             envelope, got {:?}",
            text(&read)
        );

        // And the archive routes the origin WAS announced for are served there, so the two
        // refusals above are about which routes that endpoint binds and not about the
        // address being unserved.
        let listed = harness
            .call_at(
                &origin,
                "/listFragments",
                &serde_json::json!({
                    "StreamName": name,
                    "FragmentSelector": {
                        "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                        "TimestampRange": {
                            "StartTimestamp": 0,
                            "EndTimestamp": 4_000_000_000i64,
                        },
                    },
                }),
            )
            .await;
        assert_eq!(
            status(&listed),
            200,
            "case {api_name:?} ({origin}): the archive origin serves the archive listing, \
             got {} {}",
            status(&listed),
            text(&listed)
        );
    }
}

/// RULE: every `APIName` the control plane's model publishes is accounted for by exactly one
/// of the two media endpoints — so no published value is left without an endpoint to serve
/// it, and none is claimed by both.
#[test]
fn every_published_api_name_belongs_to_exactly_one_media_endpoint() {
    let published: Vec<&str> =
        service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::ops::types::A_P_I_NAME_VALUES
            .to_vec();
    for api_name in &published {
        let live = LIVE_MEDIA_APIS.contains(api_name);
        let archived = ARCHIVED_MEDIA_APIS.contains(api_name);
        assert!(
            live != archived,
            "case {api_name:?}: exactly one of the two media endpoints serves it, \
             got live={live} archived={archived}"
        );
    }
    for api_name in LIVE_MEDIA_APIS.iter().chain(ARCHIVED_MEDIA_APIS) {
        assert!(
            published.contains(api_name),
            "case {api_name:?}: this suite names an APIName the model does not publish, \
             got {published:?}"
        );
    }
}
