// SPDX-License-Identifier: CC0-1.0

//! The operations a CONNECTOR accepts against the partner's remote endpoint, and the
//! per-file results they are then read back through.
//!
//! Three rules:
//!
//! 1. `StartFileTransfer` accepts a transfer for a stored connector and answers the
//!    identifier it is recorded under, and `ListFileTransferResults` reports one result
//!    per file the request named, IN THE ORDER it named them, each `QUEUED` — the
//!    `TransferTableStatus` for a file whose transfer has not run, which is the only
//!    status this emulator can report for work it makes no outbound connection for.
//!    Those results are reported for an SFTP connector's transfers; asked of an AS2
//!    connector the operation is not supported at all (`crate::connector_protocol_gate`
//!    is that rule's suite).
//! 2. `StartRemoteMove` accepts a move the same way, and refuses a `SourcePath` or
//!    `TargetPath` that is not an absolute path on the remote server with
//!    `InvalidRequestException` (400) — the members document themselves as absolute
//!    paths, and a relative one names no file there.
//! 3. Every one of them resolves the resource it addresses first: a `ConnectorId` the
//!    store does not hold, and a `TransferId` the resolved connector has no record of,
//!    answer `ResourceNotFoundException`.
//!
//! The rows differ along what the rules generalise over: how many files a transfer names
//! (one, several, the modelled maximum of ten), which direction it names them in (send,
//! retrieve, both), the connector kind (AS2, SFTP), the page size the results are read
//! at, and the FORM of a path (absolute, bare relative, `./`-relative, a Windows-style
//! one).

use serde_json::{json, Value};

use crate::harness::{member_of, Emulator, ROLE};

/// `TransferTableStatus`' values (source: `smithy_query(query="shape",
/// shape="TransferTableStatus")` -> `[QUEUED, IN_PROGRESS, COMPLETED, FAILED]`).
const STATUS_QUEUED: &str = "QUEUED";

/// A `ProfileId` satisfying the modelled `^p-([0-9a-f]{17})$`, for the AS2 connector's
/// configuration.
const LOCAL_PROFILE: &str = "p-0123456789abcdef0";
const PARTNER_PROFILE: &str = "p-00000000000000001";

/// A `UserSecretId` satisfying the member's modelled `len 1..2048`.
const SECRET: &str = "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp-partner";

/// A `ConnectorId` satisfying the modelled `^c-([0-9a-f]{17})$` that no create minted.
const NEVER_MINTED_CONNECTOR: &str = "c-0000000000000dead";

/// Create one connector and hand back its `ConnectorId`.
async fn create_connector(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("CreateConnector", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectorId, got {}", answer.body))
        .to_string()
}

/// An SFTP connector, which can be asked to send AND to retrieve.
async fn sftp_connector(emulator: &Emulator, case: &str) -> String {
    create_connector(
        emulator,
        case,
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": SECRET},
        }),
    )
    .await
}

/// An AS2 connector, which can only be asked to send.
async fn as2_connector(emulator: &Emulator, case: &str) -> String {
    create_connector(
        emulator,
        case,
        json!({
            "AccessRole": ROLE,
            "Url": "https://partner.example.com/as2",
            "As2Config": {
                "LocalProfileId": LOCAL_PROFILE,
                "PartnerProfileId": PARTNER_PROFILE,
                "Compression": "DISABLED",
                "EncryptionAlgorithm": "NONE",
                "SigningAlgorithm": "NONE",
            },
        }),
    )
    .await
}

/// `TransferId` is modelled `len 1..512`, pattern `^[0-9a-zA-Z./-]+$`.
fn is_modelled_transfer_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 512
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '/' || c == '-')
}

/// Read every result of one accepted operation, `max_results` at a time (or unbounded
/// when `None`), asserting the paging contract as it walks: no page is longer than the
/// bound asked for, and a `NextToken` comes back only while a further page exists.
async fn walk_results(
    emulator: &Emulator,
    case: &str,
    connector_id: &str,
    transfer_id: &str,
    max_results: Option<u32>,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    // One page per named file is the worst case; the extra iterations catch a token that
    // never clears instead of looping forever.
    for _ in 0..32 {
        let mut request = json!({"ConnectorId": connector_id, "TransferId": transfer_id});
        if let Some(limit) = max_results {
            request["MaxResults"] = json!(limit);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let answer = emulator.call("ListFileTransferResults", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListFileTransferResults 200, got {} with body {}",
            answer.status, answer.body
        );
        let page = member_of(&answer.body, "FileTransferResults")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: FileTransferResults is modelled required, got {}",
                    answer.body
                )
            })
            .clone();
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit as usize,
                "case {case:?}: expected at most {limit} results per page, got {}",
                page.len()
            );
        }
        collected.extend(page);
        next_token = member_of(&answer.body, "NextToken")
            .as_str()
            .map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: ListFileTransferResults kept handing out a NextToken after 32 pages")
}

/// RULE 1 (see the module docs): a transfer's results are one per named file, in the
/// order the request named them, each `QUEUED`.
#[tokio::test]
async fn a_transfers_results_report_every_named_file_in_submission_order() {
    let emulator = Emulator::new();
    let sftp = sftp_connector(&emulator, "the SFTP connector").await;

    let ten: Vec<String> = (0..10)
        .map(|index| format!("amzn-s3-demo-bucket/batch/file-{index}.edi"))
        .collect();

    // (case, connector, the request's path members, the file paths the results must
    // report in order). Every row names an SFTP connector: the per-file results are
    // reported for those, and `ListFileTransferResults` is not supported for an AS2
    // connector at all.
    let rows: Vec<(&str, &String, Value, Vec<String>)> = vec![
        (
            "one outbound file",
            &sftp,
            json!({"SendFilePaths": ["amzn-s3-demo-bucket/invoice.edi"]}),
            vec!["amzn-s3-demo-bucket/invoice.edi".to_string()],
        ),
        (
            "three outbound files, in the order the request names them",
            &sftp,
            json!({"SendFilePaths": [
                "amzn-s3-demo-bucket/zeta.edi",
                "amzn-s3-demo-bucket/alpha.edi",
                "amzn-s3-demo-bucket/mu.edi",
            ]}),
            vec![
                "amzn-s3-demo-bucket/zeta.edi".to_string(),
                "amzn-s3-demo-bucket/alpha.edi".to_string(),
                "amzn-s3-demo-bucket/mu.edi".to_string(),
            ],
        ),
        (
            "the modelled maximum of ten files",
            &sftp,
            json!({"SendFilePaths": &ten}),
            ten.clone(),
        ),
        (
            "an inbound transfer on an SFTP connector",
            &sftp,
            json!({"RetrieveFilePaths": ["/partner/outbox/report.csv"],
                   "LocalDirectoryPath": "/amzn-s3-demo-bucket/inbox"}),
            vec!["/partner/outbox/report.csv".to_string()],
        ),
        (
            "both directions at once, outbound first",
            &sftp,
            json!({"SendFilePaths": ["amzn-s3-demo-bucket/out.csv"],
                   "RetrieveFilePaths": ["/partner/in-1.csv", "/partner/in-2.csv"],
                   "RemoteDirectoryPath": "/partner/inbox",
                   "LocalDirectoryPath": "/amzn-s3-demo-bucket/inbox"}),
            vec![
                "amzn-s3-demo-bucket/out.csv".to_string(),
                "/partner/in-1.csv".to_string(),
                "/partner/in-2.csv".to_string(),
            ],
        ),
    ];

    let mut accepted: Vec<(String, String)> = Vec::new();
    for (case, connector_id, paths, expected) in rows {
        let mut request = json!({"ConnectorId": connector_id});
        for (member, value) in paths.as_object().expect("the row's paths are an object") {
            request[member] = value.clone();
        }
        let answer = emulator.call("StartFileTransfer", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected StartFileTransfer 200, got {} with body {}",
            answer.status, answer.body
        );
        let transfer_id = answer
            .get("TransferId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a TransferId, got {}", answer.body))
            .to_string();
        assert!(
            is_modelled_transfer_id(&transfer_id),
            "case {case:?}: expected a TransferId matching ^[0-9a-zA-Z./-]+$ within 1..512, \
             got {transfer_id:?}"
        );
        assert!(
            !accepted
                .iter()
                .any(|(_, held): &(String, String)| held == &transfer_id),
            "case {case:?}: expected a fresh TransferId, got {transfer_id:?} twice"
        );

        // Every page size answers the same results in the same order.
        for max_results in [None, Some(1), Some(2), Some(expected.len() as u32), Some(1000)] {
            let results = walk_results(
                &emulator,
                case,
                connector_id,
                &transfer_id,
                max_results,
            )
            .await;
            let reported: Vec<String> = results
                .iter()
                .map(|result| {
                    member_of(result, "FilePath")
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                reported, expected,
                "case {case:?} paged by {max_results:?}: expected the named files in \
                 submission order, got {reported:?}"
            );
            for result in &results {
                assert_eq!(
                    member_of(result, "StatusCode").as_str(),
                    Some(STATUS_QUEUED),
                    "case {case:?}: expected a file nothing has transferred to be {STATUS_QUEUED}, \
                     got {result}"
                );
                // "For transfers that fail, this parameter contains a code": nothing
                // failed, so neither member is reported.
                for absent in ["FailureCode", "FailureMessage"] {
                    assert!(
                        member_of(result, absent).is_null(),
                        "case {case:?}: expected no {absent} on a queued file, got {result}"
                    );
                }
            }
        }
        accepted.push((connector_id.clone(), transfer_id));
    }

    // Each accepted transfer is still its own record: a later transfer does not change an
    // earlier one's results.
    for (connector_id, transfer_id) in &accepted {
        let results = walk_results(
            &emulator,
            "every accepted transfer afterwards",
            connector_id,
            transfer_id,
            None,
        )
        .await;
        assert!(
            !results.is_empty(),
            "case \"every accepted transfer afterwards\": expected {transfer_id} to still \
             report its files, got {results:?}"
        );
    }
}

/// RULE 2: a move is accepted for a stored connector and its path members must be
/// absolute paths on the remote server.
#[tokio::test]
async fn a_remote_move_requires_absolute_paths() {
    let emulator = Emulator::new();
    let connector_id = sftp_connector(&emulator, "the SFTP connector").await;

    // The rows differ along the FORM of the relative path and which member carries it;
    // the absolute rows differ along the depth and the trailing form of the path.
    let accepted: Vec<(&str, &str, &str)> = vec![
        ("a file at the root", "/report.csv", "/archive.csv"),
        (
            "a file in a directory",
            "/partner/outbox/report.csv",
            "/partner/archive/report.csv",
        ),
        (
            "a rename within one directory",
            "/partner/outbox/a.csv",
            "/partner/outbox/b.csv",
        ),
        ("a directory", "/partner/outbox", "/partner/archive"),
    ];
    for (case, source, target) in accepted {
        let answer = emulator
            .call(
                "StartRemoteMove",
                json!({"ConnectorId": &connector_id, "SourcePath": source,
                       "TargetPath": target}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected StartRemoteMove 200, got {} with body {}",
            answer.status, answer.body
        );
        let move_id = answer
            .get("MoveId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a MoveId, got {}", answer.body))
            .to_string();
        assert!(
            is_modelled_transfer_id(&move_id),
            "case {case:?}: expected a MoveId matching ^[0-9a-zA-Z./-]+$ within 1..512, got \
             {move_id:?}"
        );

        // The file the move addresses is the one its results report, still queued.
        let results = walk_results(&emulator, case, &connector_id, &move_id, None).await;
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: expected the one path the move addresses, got {results:?}"
        );
        assert_eq!(
            member_of(&results[0], "FilePath").as_str(),
            Some(source),
            "case {case:?}: expected the source path reported, got {:?}",
            results[0]
        );
        assert_eq!(
            member_of(&results[0], "StatusCode").as_str(),
            Some(STATUS_QUEUED),
            "case {case:?}: expected the move's file to be {STATUS_QUEUED}, got {:?}",
            results[0]
        );
    }

    let refused: Vec<(&str, &str, &str)> = vec![
        ("a bare relative source", "not-an-absolute-path.txt", "/archive.txt"),
        ("a ./-relative source", "./outbox/report.csv", "/archive/report.csv"),
        ("a relative target", "/outbox/report.csv", "archive/report.csv"),
        ("a Windows-style source", "C:\\outbox\\report.csv", "/archive.csv"),
        ("both paths relative", "outbox/a.csv", "outbox/b.csv"),
    ];
    for (case, source, target) in refused {
        let answer = emulator
            .call(
                "StartRemoteMove",
                json!({"ConnectorId": &connector_id, "SourcePath": source,
                       "TargetPath": target}),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
            answer.status,
            answer.body
        );
        assert!(
            member_of(&answer.body, "MoveId").is_null(),
            "case {case:?}: expected a refusal to mint no id, got {}",
            answer.body
        );
    }
}

/// RULE 3: the resource a request addresses is resolved first, and a request that names
/// no file to transfer, or a direction the connector cannot serve, is malformed.
#[tokio::test]
async fn a_request_that_addresses_nothing_is_refused() {
    let emulator = Emulator::new();
    let sftp = sftp_connector(&emulator, "the SFTP connector").await;
    // A SECOND SFTP connector, so "a transfer another connector accepted" is a question
    // about the records of a connector this operation IS supported for.
    let other_sftp = sftp_connector(&emulator, "the other SFTP connector").await;
    let as2 = as2_connector(&emulator, "the AS2 connector").await;
    let accepted = emulator
        .call(
            "StartFileTransfer",
            json!({"ConnectorId": &sftp, "SendFilePaths": ["amzn-s3-demo-bucket/one.csv"]}),
        )
        .await;
    let transfer_id = accepted
        .get("TransferId")
        .as_str()
        .expect("the accepted transfer answers a TransferId")
        .to_string();

    // (case, operation, request, expected code)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a transfer on a connector never created",
            "StartFileTransfer",
            json!({"ConnectorId": NEVER_MINTED_CONNECTOR,
                   "SendFilePaths": ["amzn-s3-demo-bucket/one.csv"]}),
            "ResourceNotFoundException",
        ),
        (
            "a move on a connector never created",
            "StartRemoteMove",
            json!({"ConnectorId": "c-fffffffffffffffff", "SourcePath": "/a", "TargetPath": "/b"}),
            "ResourceNotFoundException",
        ),
        (
            "results of a connector never created",
            "ListFileTransferResults",
            json!({"ConnectorId": NEVER_MINTED_CONNECTOR, "TransferId": &transfer_id}),
            "ResourceNotFoundException",
        ),
        (
            "results of a transfer the connector never accepted",
            "ListFileTransferResults",
            json!({"ConnectorId": &sftp, "TransferId": "0123456789abcdef"}),
            "ResourceNotFoundException",
        ),
        (
            "a transfer accepted by ANOTHER connector",
            "ListFileTransferResults",
            json!({"ConnectorId": &other_sftp, "TransferId": &transfer_id}),
            "ResourceNotFoundException",
        ),
        (
            "results asked of an AS2 connector, which the operation does not support",
            "ListFileTransferResults",
            json!({"ConnectorId": &as2, "TransferId": &transfer_id}),
            "InvalidRequestException",
        ),
        (
            "a transfer naming no file at all",
            "StartFileTransfer",
            json!({"ConnectorId": &sftp}),
            "InvalidRequestException",
        ),
        (
            "a transfer naming empty path lists",
            "StartFileTransfer",
            json!({"ConnectorId": &sftp, "SendFilePaths": [], "RetrieveFilePaths": []}),
            // `FilePaths` is modelled `len 1..10`, so an empty list is refused by the
            // framework's boundary validator before the handler runs at all.
            "ValidationException",
        ),
        (
            "a retrieval asked of an AS2 connector",
            "StartFileTransfer",
            json!({"ConnectorId": &as2, "RetrieveFilePaths": ["/partner/in.edi"],
                   "LocalDirectoryPath": "/amzn-s3-demo-bucket/inbox"}),
            "InvalidRequestException",
        ),
        (
            "a results page resumed with a token this service never issued",
            "ListFileTransferResults",
            json!({"ConnectorId": &sftp, "TransferId": &transfer_id,
                   "NextToken": "not-a-token"}),
            "InvalidRequestException",
        ),
    ];

    for (case, operation, request, code) in rows {
        let answer = emulator.call(operation, request).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {} with body {}",
            answer.status,
            answer.body
        );
    }

    // The transfer the connector did accept is untouched by every refusal above.
    let results = walk_results(
        &emulator,
        "the accepted transfer after the refusals",
        &sftp,
        &transfer_id,
        None,
    )
    .await;
    assert_eq!(
        results.len(),
        1,
        "case \"the accepted transfer after the refusals\": expected its one file still \
         reported, got {results:?}"
    );
}
