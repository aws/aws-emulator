// SPDX-License-Identifier: CC0-1.0

//! `ExportStreamSessionFiles` end to end: where the archive lands, what it holds, and what
//! the stream session then reports about it.
//!
//! The rules under test, from this service's Smithy model
//! (`smithy_query(query="operation", operation="ExportStreamSessionFiles")`,
//! `smithy_query(query="shape", shape="ExportFilesMetadata,ExportFilesStatus")`) and the
//! operation's documentation:
//!
//! * the export delivers ONE compressed archive to the Amazon S3 location the request
//!   named ("You must provide an Amazon Simple Storage Service (Amazon S3) bucket to store
//!   the files in ... Amazon GameLift Streams produces a compressed folder that contains
//!   all of the files and directories that were modified or created by the application"),
//!   holding what was collected from the folders the service collects from
//!   ("application/: The folder where your application or game is stored. profile/: The
//!   user profile folder. temp/: The system temp folder") — so the object and its contents
//!   are asked of Amazon S3 itself here rather than of this service's own answer;
//! * `OutputUri` decides the object's name: a value ending in the archive extension IS the
//!   object, and one ending in the separator is a folder the service generates a name
//!   under ("Amazon GameLift Streams generates a ZIP file name based on the stream session
//!   metadata ... `YYYYMMDD-HHMMSS-appId-sg-Id-sessionId.zip` or another similar name");
//! * "To verify the status of the exported files, use GetStreamSession": the session
//!   reports the export as `ExportFilesMetadata`, `PENDING` while the upload runs and
//!   `SUCCEEDED` once "the exported files are successfully stored in an S3 bucket", with
//!   the URI of the object they are stored at — and nothing at all for a session nobody
//!   exported;
//! * "You can only call this action on a stream session that is in progress, specifically
//!   in one of the following statuses ACTIVE, CONNECTED, PENDING_CLIENT_RECONNECTION, and
//!   RECONNECTING": any other status is refused 400 `ValidationException`.
//!
//! The tables span those classes — five spellings of `OutputUri`, the stable statuses a
//! stream session holds in this emulator, and the identifier forms the two path labels
//! take — rather than one recorded call. Every request goes through the same front door a
//! client uses, against a router that mounts this service beside this emulator's own
//! Amazon S3 on ONE engine, so an exported archive is a real object of that store. Nothing
//! here waits a fixed time: every wait polls for the CONDITION it needs.
//!
//! The transient statuses (`ACTIVATING` while a placement is being resolved, `TERMINATING`
//! while a stop is settling) are deliberately not rows: the clock owes each of them a move
//! within one transition, so a row for one would assert how fast the host runs rather than
//! what the service does. `stream_session_admin_shell.rs` tables the same status class for
//! the other operation that gates on it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::application_read_projection::{
    application_in_ready_status, emulator_with_staged_content, poll_until, s3, staged_source_uri,
    stream_group_fully_active,
};
use super::stream_workflow::{as_label, call, call_with_headers, text, ACCOUNT, REGION};

/// The bucket every export below is delivered into, created through Amazon S3's own API.
const EXPORTS: &str = "gameliftstreams-exports";

/// A bucket this emulator's object store does not hold, so nothing can be uploaded to it.
const NO_SUCH_BUCKET: &str = "gameliftstreams-absent-bucket";

/// A `SignalRequest` carrying a WebRTC offer the placement accepts, so the session it
/// starts reaches `ACTIVE`.
const OFFER: &str = "{\"type\":\"offer\",\"sdp\":\"v=0\"}";

/// A `SignalRequest` carrying no offer at all, so the placement of the session it starts
/// fails and the session settles in `ERROR`.
const NOT_AN_OFFER: &str = "no-offer";

/// The folders the service collects files into: "Amazon GameLift Streams collects the
/// following generated and modified files. Find them in the corresponding folders in the
/// .zip archive. application/: The folder where your application or game is stored.
/// profile/: The user profile folder. temp/: The system temp folder."
const COLLECTED_FOLDERS: [&str; 3] = ["application/", "profile/", "temp/"];

/// How this service refuses a session that is not in progress, over the session's own ARN
/// — the same wording this service answers the other operation that gates on these four
/// statuses with (`CreateStreamSessionAdminShell`, see `stream_session_admin_shell.rs`).
/// The statuses are the eligible values of the model's `StreamSessionStatus`, in the order
/// the operation's documentation lists them.
fn not_in_progress(session_arn: &str) -> String {
    format!(
        "Stream session '{session_arn}' is not in ACTIVE, CONNECTED, \
         PENDING_CLIENT_RECONNECTION or RECONNECTING status for file export"
    )
}

/// This service beside this emulator's own Amazon S3, with the destination bucket of every
/// export below created in it.
async fn emulator_with_export_bucket() -> MultiServiceRouter {
    let router = emulator_with_staged_content().await;
    let created = s3(&router)
        .create_bucket()
        .bucket(EXPORTS)
        .create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await;
    assert!(
        created.is_ok(),
        "Amazon S3 must create the export bucket {EXPORTS}, got {created:?}"
    );
    router
}

/// Create an application from the staged content and wait until it is `READY`.
async fn ready_application(router: &MultiServiceRouter, description: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/applications",
        Some(json!({
            "Description": description,
            "RuntimeEnvironment": {"Type": "PROTON", "Version": "20250516"},
            "ExecutablePath": "game.exe",
            "ApplicationSourceUri": staged_source_uri(),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {description:?}: expected CreateApplication 201, got {status} {created}"
    );
    let id = text(&created, "/Id", description);
    application_in_ready_status(router, &id, description).await;
    id
}

/// Create a stream group holding `capacity` streams in this Region and wait until it and
/// its location are `ACTIVE`, answering its id and ARN.
async fn active_group(
    router: &MultiServiceRouter,
    description: &str,
    application: &str,
    capacity: i64,
) -> (String, String) {
    let (status, created) = call(
        router,
        "POST",
        "/streamgroups",
        Some(json!({
            "Description": description,
            "StreamClass": "gen5n_high",
            "DefaultApplicationIdentifier": application,
            "LocationConfigurations": [
                {"LocationName": REGION, "AlwaysOnCapacity": capacity}
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {description:?}: expected CreateStreamGroup 201, got {status} {created}"
    );
    let id = text(&created, "/Id", description);
    stream_group_fully_active(router, &id, description).await;
    (id, text(&created, "/Arn", description))
}

/// Start one stream session in `group` with `signal_request`, and wait until the clock has
/// settled its placement in `expected_status`. Answers the session's id and ARN.
async fn session_in_status(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    application: &str,
    signal_request: &str,
    expected_status: &str,
) -> (String, String) {
    let (status, session) = call(
        router,
        "POST",
        &format!("/streamgroups/{group}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": signal_request,
            "ApplicationIdentifier": application,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected StartStreamSession 201, got {status} {session}"
    );
    let arn = text(&session, "/Arn", case);
    let id = arn
        .rsplit('/')
        .next()
        .expect("a stream session ARN ends in the session id")
        .to_string();
    let settled = poll_until(
        router,
        &format!("/streamgroups/{group}/streamsessions/{id}"),
        case,
        |body| body["Status"] == json!(expected_status),
    )
    .await;
    assert_eq!(
        settled["Status"],
        json!(expected_status),
        "case {case:?}: expected a {expected_status} session, got {settled}"
    );
    (id, arn)
}

/// Terminate `session` and wait until the clock has made it `TERMINATED`.
async fn terminated_session(router: &MultiServiceRouter, case: &str, group: &str, session: &str) {
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/streamgroups/{group}/streamsessions/{session}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected TerminateStreamSession 204, got {status} {body}"
    );
    poll_until(
        router,
        &format!("/streamgroups/{group}/streamsessions/{session}"),
        case,
        |body| body["Status"] == json!("TERMINATED"),
    )
    .await;
}

/// `PUT /streamgroups/<group>/streamsessions/<session>/exportfiles`, answering the status,
/// the wire error code and the parsed body.
async fn export(
    router: &MultiServiceRouter,
    group: &str,
    session: &str,
    output_uri: &str,
) -> (u16, String, Value) {
    let (status, headers, body) = call_with_headers(
        router,
        "PUT",
        &format!("/streamgroups/{group}/streamsessions/{session}/exportfiles"),
        Some(json!({"OutputUri": output_uri})),
    )
    .await;
    let code = headers
        .get("x-amzn-errortype")
        .cloned()
        .unwrap_or_default();
    (status, code, body)
}

/// The `ExportFilesMetadata` the session reports once its export has reached
/// `expected_status`.
async fn export_metadata_in_status(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    session: &str,
    expected_status: &str,
) -> Value {
    let settled = poll_until(
        router,
        &format!("/streamgroups/{group}/streamsessions/{session}"),
        case,
        |body| body["ExportFilesMetadata"]["Status"] == json!(expected_status),
    )
    .await;
    settled["ExportFilesMetadata"].clone()
}

/// The bucket and key an `s3://bucket/key` URI names.
fn s3_location(uri: &str, case: &str) -> (String, String) {
    let rest = uri
        .strip_prefix("s3://")
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} is not an s3:// URI"));
    let (bucket, key) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no object key"));
    (bucket.to_string(), key.to_string())
}

/// The entries of the archive stored at `uri`, read back out of Amazon S3 itself — so this
/// asserts what the object store holds, not what this service answered.
async fn archive_entries(router: &MultiServiceRouter, uri: &str, case: &str) -> Vec<String> {
    let (bucket, key) = s3_location(uri, case);
    let object = s3(router)
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: Amazon S3 must hold the exported archive at {uri}: {error:?}")
        });
    let bytes = object
        .body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the archive's bytes are readable: {error:?}"))
        .into_bytes();
    assert!(
        !bytes.is_empty(),
        "case {case:?}: the exported archive at {uri} carries no bytes"
    );
    let mut archive = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes.to_vec()))
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the object at {uri} must be a readable .zip: {error:?}")
        });
    let mut entries: Vec<String> = Vec::new();
    for position in 0..archive.len() {
        let mut entry = archive
            .by_index(position)
            .unwrap_or_else(|error| panic!("case {case:?}: entry {position} is readable: {error:?}"));
        let name = entry
            .name()
            .unwrap_or_else(|error| panic!("case {case:?}: entry {position} names a path: {error:?}"))
            .to_string();
        let mut content = Vec::new();
        let read = ::std::io::Read::read_to_end(&mut entry, &mut content);
        assert!(
            read.is_ok(),
            "case {case:?}: entry {name:?} of the archive is readable, got {read:?}"
        );
        entries.push(name);
    }
    entries.sort();
    entries
}

/// The entries an exported archive must hold: the folders the service collects files into,
/// and no file — no application ran in the session to write one.
fn expected_archive_entries() -> Vec<String> {
    let mut expected: Vec<String> = COLLECTED_FOLDERS
        .iter()
        .map(|folder| (*folder).to_string())
        .collect();
    expected.sort();
    expected
}

/// Whether `name` has the form the service documents for a generated archive name:
/// `YYYYMMDD-HHMMSS-appId-sg-Id-sessionId.zip`, ending in the ids of the application, the
/// stream group and the stream session the export was taken from.
fn is_generated_name(name: &str, application: &str, group: &str, session: &str) -> bool {
    let Some(stamp) = name.strip_suffix(&format!("-{application}-{group}-{session}.zip")) else {
        return false;
    };
    let Some((day, time)) = stamp.split_once('-') else {
        return false;
    };
    day.len() == 8
        && time.len() == 6
        && day.chars().all(|digit| digit.is_ascii_digit())
        && time.chars().all(|digit| digit.is_ascii_digit())
}

/// What an `OutputUri` names: the object itself, or a folder the service generates a name
/// under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Names {
    /// The value IS the object: the reported URI is the value, byte for byte.
    TheObject,
    /// The value is a folder: the reported URI is the value plus a generated archive name.
    AFolder,
}

/// An export lands as ONE readable `.zip` object at the location the request named, the
/// session reports it as `SUCCEEDED` with that object's URI, and the object store itself
/// holds the archive there.
#[tokio::test]
async fn an_export_lands_as_one_archive_at_the_location_the_request_named() {
    let router = emulator_with_export_bucket().await;
    let application = ready_application(&router, "export-destinations").await;
    // (case, the OutputUri the request carries, what it names). The rows differ along what
    // the rule generalises over: whether the value names the object or a folder, the case
    // of the extension the model's pattern admits, how deep the prefix is, and the bucket
    // root — not along the name of anything.
    let rows: [(&str, String, Names); 5] = [
        (
            "the object the request named",
            format!("s3://{EXPORTS}/MyGame_Session1.zip"),
            Names::TheObject,
        ),
        (
            "an object named with the uppercase extension",
            format!("s3://{EXPORTS}/archives/MyGame_Session2.ZIP"),
            Names::TheObject,
        ),
        (
            "a folder",
            format!("s3://{EXPORTS}/MyGameSessions_ExportedFiles/"),
            Names::AFolder,
        ),
        (
            "a multi-level folder",
            format!("s3://{EXPORTS}/sessions/2026/exports/"),
            Names::AFolder,
        ),
        (
            "the root of the bucket",
            format!("s3://{EXPORTS}/"),
            Names::AFolder,
        ),
    ];
    let (group, _) = active_group(
        &router,
        "export-destination-group",
        &application,
        rows.len() as i64,
    )
    .await;

    for (case, output_uri, names) in &rows {
        let (session, _) =
            session_in_status(&router, case, &group, &application, OFFER, "ACTIVE").await;

        // A session nobody exported reports no export at all.
        let before = poll_until(
            &router,
            &format!("/streamgroups/{group}/streamsessions/{session}"),
            case,
            |_| true,
        )
        .await;
        assert_eq!(
            before.get("ExportFilesMetadata"),
            None,
            "case {case:?}: a session with no export reports no ExportFilesMetadata, got \
             {before}"
        );

        let (status, code, body) = export(&router, &group, &session, output_uri).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: expected ExportStreamSessionFiles 200, got {status} {code:?} {body}"
        );

        let metadata =
            export_metadata_in_status(&router, case, &group, &session, "SUCCEEDED").await;
        let reported = text(&metadata, "/OutputUri", case);
        match names {
            Names::TheObject => assert_eq!(
                &reported, output_uri,
                "case {case:?}: the export reports the object the request named, got \
                 {reported:?}"
            ),
            Names::AFolder => {
                let generated = reported.strip_prefix(output_uri.as_str()).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the archive under {output_uri:?}, got \
                         {reported:?}"
                    )
                });
                assert!(
                    is_generated_name(generated, &application, &group, &session),
                    "case {case:?}: expected a generated name \
                     YYYYMMDD-HHMMSS-{application}-{group}-{session}.zip, got {generated:?}"
                );
            }
        }
        assert_eq!(
            metadata.get("StatusReason"),
            None,
            "case {case:?}: a SUCCEEDED export carries no StatusReason, got {metadata}"
        );

        // The object store itself answers for the archive, and it holds the folders the
        // export collects from.
        let entries = archive_entries(&router, &reported, case).await;
        let expected = expected_archive_entries();
        assert_eq!(
            entries, expected,
            "case {case:?}: expected the collected folders {expected:?} in the archive, got \
             {entries:?}"
        );
    }
}

/// A later export of the same session replaces what the session reports and lands as its
/// own object, so both archives are in the bucket and the session names the newer one.
#[tokio::test]
async fn a_second_export_of_a_session_lands_as_its_own_object() {
    let case = "two exports of one session";
    let router = emulator_with_export_bucket().await;
    let application = ready_application(&router, "export-twice").await;
    let (group, _) = active_group(&router, "export-twice-group", &application, 1).await;
    let (session, _) =
        session_in_status(&router, case, &group, &application, OFFER, "ACTIVE").await;

    let first = format!("s3://{EXPORTS}/twice/first.zip");
    let second = format!("s3://{EXPORTS}/twice/second.zip");
    for output_uri in [&first, &second] {
        let (status, code, body) = export(&router, &group, &session, output_uri).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: expected ExportStreamSessionFiles 200 for {output_uri}, got \
             {status} {code:?} {body}"
        );
        let metadata = poll_until(
            &router,
            &format!("/streamgroups/{group}/streamsessions/{session}"),
            case,
            |body| body["ExportFilesMetadata"]["OutputUri"] == json!(output_uri),
        )
        .await;
        assert_eq!(
            metadata["ExportFilesMetadata"]["OutputUri"],
            json!(output_uri),
            "case {case:?}: the session reports the export it was last asked for, got \
             {metadata}"
        );
    }

    // Both archives are objects of the store: the second export did not move the first.
    let expected = expected_archive_entries();
    for output_uri in [&first, &second] {
        let entries = archive_entries(&router, output_uri, case).await;
        assert_eq!(
            entries, expected,
            "case {case:?}: the archive at {output_uri} holds the collected folders, got \
             {entries:?}"
        );
    }
}

/// Only a stream session that is in progress exports its files: every other status is
/// refused 400 `ValidationException` naming that session and the eligible statuses — never
/// 501, and never a success — and the refusal records no export.
#[tokio::test]
async fn an_export_is_refused_unless_the_session_is_in_progress() {
    let router = emulator_with_export_bucket().await;
    let application = ready_application(&router, "export-statuses").await;
    // Two streams of capacity, so one row's session does not starve the next row's
    // placement.
    let (group, _) = active_group(&router, "export-status-group", &application, 2).await;

    // (case, the SignalRequest the start carries, whether the session is terminated first,
    // the status the session then holds, whether the export is accepted). The rows differ
    // along how the session left the statuses the export is documented for: a placement
    // that succeeded, a placement that failed, and a stream that was stopped.
    let rows: [(&str, &str, bool, &str, bool); 3] = [
        ("a placed session", OFFER, false, "ACTIVE", true),
        (
            "a session whose placement failed",
            NOT_AN_OFFER,
            false,
            "ERROR",
            false,
        ),
        ("a terminated session", OFFER, true, "TERMINATED", false),
    ];
    let output_uri = format!("s3://{EXPORTS}/statuses/");
    for (case, signal_request, terminate, status_held, accepted) in rows {
        let (session, session_arn) = session_in_status(
            &router,
            case,
            &group,
            &application,
            signal_request,
            match terminate {
                true => "ACTIVE",
                false => status_held,
            },
        )
        .await;
        if terminate {
            terminated_session(&router, case, &group, &session).await;
        }

        let (status, code, body) = export(&router, &group, &session, &output_uri).await;
        if accepted {
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case:?}: a {status_held} session exports its files, got {status} \
                 {code:?} {body}"
            );
            export_metadata_in_status(&router, case, &group, &session, "SUCCEEDED").await;
            continue;
        }
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException for a {status_held} session, \
             got {status} {code:?} {body}"
        );
        let message = text(&body, "/Message", case);
        let expected = not_in_progress(&session_arn);
        assert!(
            message.starts_with(&expected),
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
        assert!(
            message.contains("(Internal ID: "),
            "case {case:?}: this service's handler messages carry a per-request internal id, \
             got {message:?}"
        );

        // A refused export records nothing: the session reports no export at all.
        let after = poll_until(
            &router,
            &format!("/streamgroups/{group}/streamsessions/{session}"),
            case,
            |_| true,
        )
        .await;
        assert_eq!(
            after.get("ExportFilesMetadata"),
            None,
            "case {case:?}: a refused export must record nothing, got {after}"
        );
    }
}

/// The two path labels are resolved the way every session-scoped operation of this service
/// resolves them: a label that can name no resource of its own type is refused 400
/// `ValidationException` before the store is read, an identifier naming nothing stored is a
/// 404 `ResourceNotFoundException` reported against the composite stream session ARN, and
/// either label in ARN form resolves the same stored session a bare id does.
#[tokio::test]
async fn the_path_labels_resolve_the_stream_session_they_name() {
    let router = emulator_with_export_bucket().await;
    let application = ready_application(&router, "export-identifiers").await;
    let (group, group_arn) = active_group(&router, "export-id-group", &application, 1).await;
    let (session, session_arn) =
        session_in_status(&router, "identifier rows", &group, &application, OFFER, "ACTIVE").await;
    let application_arn =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application/{application}");
    let output_uri = format!("s3://{EXPORTS}/identifiers/");

    // (case, group label, session label, expected status, expected error code, a fragment
    // of the expected message).
    let rows: [(&str, String, String, u16, &str, String); 6] = [
        (
            "an unknown stream group",
            "sg-missing".to_string(),
            session.clone(),
            404,
            "ResourceNotFoundException",
            format!(
                "Stream session 'arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/\
                 sg-missing/{session}' does not exist"
            ),
        ),
        (
            "an unknown stream session",
            group.clone(),
            "ss-missing".to_string(),
            404,
            "ResourceNotFoundException",
            format!(
                "Stream session 'arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/\
                 {group}/ss-missing' does not exist"
            ),
        ),
        (
            "a stream group named by an application ARN",
            as_label(&application_arn),
            session.clone(),
            400,
            "ValidationException",
            "'Identifier' is not a valid resource ARN of type 'streamgroup'".to_string(),
        ),
        (
            "a stream session named by an application ARN",
            group.clone(),
            as_label(&application_arn),
            400,
            "ValidationException",
            "'StreamSessionIdentifier' is not a valid resource ARN of type 'streamsession'"
                .to_string(),
        ),
        (
            "the stream group by ARN",
            as_label(&group_arn),
            session.clone(),
            200,
            "",
            String::new(),
        ),
        (
            "the stream session by ARN",
            group.clone(),
            as_label(&session_arn),
            200,
            "",
            String::new(),
        ),
    ];
    for (case, group_label, session_label, expected_status, expected_code, fragment) in rows {
        let (status, code, body) = export(&router, &group_label, &session_label, &output_uri).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code:?}, got {status} \
             {code:?} {body}"
        );
        if expected_status == 200 {
            // The label resolved the stored session, which is what the export it recorded
            // proves.
            export_metadata_in_status(&router, case, &group, &session, "SUCCEEDED").await;
            continue;
        }
        let message = text(&body, "/Message", case);
        assert!(
            message.contains(&fragment),
            "case {case:?}: expected a message carrying {fragment:?}, got {message:?}"
        );
    }
}

/// A destination the object store will not take the archive into is reported as a FAILED
/// export carrying the reason and the destination, and no success is claimed for a file
/// that is not there.
#[tokio::test]
async fn an_export_the_object_store_refuses_is_reported_as_failed() {
    let case = "a bucket the object store does not hold";
    let router = emulator_with_export_bucket().await;
    let application = ready_application(&router, "export-refused").await;
    let (group, _) = active_group(&router, "export-refused-group", &application, 1).await;
    let (session, _) =
        session_in_status(&router, case, &group, &application, OFFER, "ACTIVE").await;

    let output_uri = format!("s3://{NO_SUCH_BUCKET}/out.zip");
    let (status, code, body) = export(&router, &group, &session, &output_uri).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case {case:?}: expected ExportStreamSessionFiles 200, got {status} {code:?} {body}"
    );

    let metadata = export_metadata_in_status(&router, case, &group, &session, "FAILED").await;
    assert_eq!(
        metadata["OutputUri"],
        json!(output_uri),
        "case {case:?}: the failed export names the destination the request asked for, got \
         {metadata}"
    );
    let reason = text(&metadata, "/StatusReason", case);
    assert!(
        reason.contains(&output_uri),
        "case {case:?}: expected a StatusReason naming {output_uri}, got {reason:?}"
    );

    // Nothing was stored anywhere: the store holds no such bucket, and therefore no object.
    let (bucket, key) = s3_location(&output_uri, case);
    let found = s3(&router)
        .head_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await;
    assert!(
        found.is_err(),
        "case {case:?}: no object may exist for a failed export, got {found:?}"
    );
}
