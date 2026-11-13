// SPDX-License-Identifier: CC0-1.0

//! `DescribeTypeRegistration` over the awsQuery wire: monitoring ONE extension registration
//! request by the token `RegisterType` answered with.
//!
//! The rule every case here holds the service to: **`DescribeTypeRegistration` reports the
//! registration request this account and Region holds under the token the request names — the
//! status the clock has already made due, the reason a request that FAILED registered nothing,
//! and the extension's and the version's ARNs only once the request has COMPLETEd — and a
//! token it resolves to no such request, whatever that token's spelling and whichever other
//! Region holds a request under it, is refused with the registry's own modelled
//! `CFNRegistryException` (400) whose sentence quotes nothing from the request; monitoring is a
//! report, so a read leaves every record as it stands beyond the conclusion the clock had
//! already made due.**
//!
//! The rows differ along what the rule generalises over: each status a request can hold,
//! whether the clock has made its conclusion due, the extension kinds the model admits
//! (`RESOURCE`, `MODULE`, `HOOK`), which version a request registers, the Region a token was
//! issued in, and — for the refusal — the spellings the modelled `RegistrationToken`
//! (`@length(1..128)`, `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`) admits, at each published
//! bound.
//!
//! `RegisterType` is the only writer that starts a registration, and it is driven end to end
//! by `extension_registrations.rs`; the requests a case here monitors are seeded straight into
//! the store through `state::record_type_registration`, the same device `type_registrations.rs`
//! uses, so each case fixes exactly the record it is about. Every READ goes through the real
//! request path (form-encoded body in, XML envelope out), so what is asserted is what a client
//! sees.

use std::sync::Arc;

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    registration_status, registry_type, type_arn, type_version_arn, StoredTypeRegistration,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    already_due, call_in, member, refusal, router_with_engine, still_running, store, xml_text,
    REGION,
};

/// Another Region's registry, for the rows that hold monitoring to its own Region: the
/// registry is regional, so a token issued elsewhere names no request here.
const OTHER_REGION: &str = "us-west-2";

/// The error code the registry's own modelled refusal carries on the wire
/// (`CFNRegistryException`'s `awsQueryError`).
const REGISTRY_REFUSAL_CODE: &str = "CFNRegistryException";

/// The sentence the registry answers a registration token it resolves to no request with,
/// written out here rather than read off the implementation: the token the request named is no
/// part of it.
const UNRESOLVED_TOKEN: &str = "UUID is invalid";

/// Why a seeded request that could not register its version concluded `FAILED` — the sentence
/// `Description` reports for it.
const FAILURE_REASON: &str = "The schema handler package is not a valid extension package";

/// One `DescribeTypeRegistration` call, as an SDK sends it.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    token: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeTypeRegistration",
        &[member("RegistrationToken", token)],
    )
    .await
}

/// One row of the monitoring table: the request seeded, and what a read of it must report.
struct Row {
    case: &'static str,
    /// The kind of extension the request registers.
    type_kind: &'static str,
    type_name: &'static str,
    /// The `TypeVersionId` the request registers, or `None` for a request that registers none
    /// because it concludes `FAILED`.
    version_id: Option<&'static str>,
    token: &'static str,
    /// Whether the clock has already made the request's conclusion due.
    concluded: bool,
    /// The `ProgressStatus` the read must report.
    progress: &'static str,
    /// The `Description` the read must report, or `None` where it reports none.
    description: Option<&'static str>,
}

impl Row {
    /// The record this row seeds: a request that registers its version, or one that fails.
    fn seeded(&self) -> StoredTypeRegistration {
        let concluded_at = if self.concluded {
            already_due()
        } else {
            still_running()
        };
        match self.version_id {
            Some(version_id) => StoredTypeRegistration::registering(
                REGION,
                self.type_kind,
                self.type_name,
                version_id,
                self.token,
                concluded_at,
            ),
            None => StoredTypeRegistration::failing(
                REGION,
                self.type_kind,
                self.type_name,
                self.token,
                FAILURE_REASON,
                concluded_at,
            ),
        }
    }
}

/// The requests every monitoring case reads, one per status a request can hold and per side of
/// the clock.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a resource registration still running",
            type_kind: registry_type::RESOURCE,
            type_name: "My::Monitored::Resource",
            version_id: Some("00000001"),
            token: "monitored-resource-running",
            concluded: false,
            progress: registration_status::IN_PROGRESS,
            description: None,
        },
        Row {
            case: "a resource registration whose conclusion the clock has made due",
            type_kind: registry_type::RESOURCE,
            type_name: "My::Monitored::Resource",
            // A later version of the same extension: which version a request registered is
            // what its own version ARN names.
            version_id: Some("00000003"),
            token: "monitored-resource-complete",
            concluded: true,
            progress: registration_status::COMPLETE,
            description: None,
        },
        Row {
            case: "a module registration whose conclusion the clock has made due",
            type_kind: registry_type::MODULE,
            type_name: "My::Monitored::Thing::MODULE",
            version_id: Some("00000001"),
            token: "monitored-module-complete",
            concluded: true,
            progress: registration_status::COMPLETE,
            description: None,
        },
        Row {
            case: "a Hook registration that could not register its version",
            type_kind: registry_type::HOOK,
            type_name: "My::Monitored::Hook",
            version_id: None,
            token: "monitored-hook-failed",
            concluded: true,
            progress: registration_status::FAILED,
            description: Some(FAILURE_REASON),
        },
        Row {
            case: "a registration that will fail, read before the clock makes that due",
            type_kind: registry_type::HOOK,
            type_name: "My::Monitored::Hook",
            version_id: None,
            token: "monitored-hook-running",
            concluded: false,
            progress: registration_status::IN_PROGRESS,
            description: None,
        },
    ]
}

/// Seed `rows` into this Region's registry.
fn seed(engine: &Arc<engine::InMemoryEngine>, rows: &[Row]) {
    for row in rows {
        store(engine).lock().record_type_registration(row.seeded());
    }
}

/// The record this Region's registry holds under `token`, as the store holds it: what a case
/// about what a READ leaves behind compares across its reads.
fn held_record(engine: &Arc<engine::InMemoryEngine>, token: &str) -> String {
    format!("{:?}", store(engine).lock().type_registration(REGION, token))
}

#[tokio::test]
async fn each_request_is_reported_under_the_status_the_clock_has_made_due() {
    let (router, engine) = router_with_engine();
    let rows = rows();
    seed(&engine, &rows);

    for row in &rows {
        let (status, body) = describe(&router, REGION, row.token).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the read to succeed, got {status} {body}",
            row.case
        );
        let progress = xml_text(&body, "ProgressStatus");
        assert_eq!(
            progress.as_deref(),
            Some(row.progress),
            "case {:?}: expected ProgressStatus {:?}, got {progress:?}",
            row.case,
            row.progress
        );
        let description = xml_text(&body, "Description");
        assert_eq!(
            description.as_deref(),
            row.description,
            "case {:?}: expected Description {:?}, got {description:?}",
            row.case,
            row.description
        );

        // "For registration requests with a ProgressStatus of other than COMPLETE, this will
        // be null": the extension and the version a request registered are reported once the
        // request has concluded COMPLETE, and not before.
        let extension_arn = type_arn(REGION, row.type_kind, row.type_name);
        let (expected_type_arn, expected_version_arn) =
            match (row.progress, row.version_id) {
                (registration_status::COMPLETE, Some(version_id)) => (
                    Some(extension_arn.clone()),
                    Some(type_version_arn(&extension_arn, version_id)),
                ),
                _ => (None, None),
            };
        let reported_type_arn = xml_text(&body, "TypeArn");
        assert_eq!(
            reported_type_arn, expected_type_arn,
            "case {:?}: expected TypeArn {expected_type_arn:?}, got {reported_type_arn:?}",
            row.case
        );
        let reported_version_arn = xml_text(&body, "TypeVersionArn");
        assert_eq!(
            reported_version_arn, expected_version_arn,
            "case {:?}: expected TypeVersionArn {expected_version_arn:?}, got \
             {reported_version_arn:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn monitoring_a_running_request_leaves_the_record_as_it_stands() {
    let (router, engine) = router_with_engine();
    let rows = rows();
    seed(&engine, &rows);
    // The request still running: its conclusion is not due, so nothing about it is owed at
    // this instant and a read may not move it.
    let token = "monitored-resource-running";

    let before = held_record(&engine, token);
    for read in 1..=3 {
        let (status, body) = describe(&router, REGION, token).await;
        assert_eq!(
            status, 200,
            "case \"read {read}\": expected the read to succeed, got {status} {body}"
        );
        let progress = xml_text(&body, "ProgressStatus");
        assert_eq!(
            progress.as_deref(),
            Some(registration_status::IN_PROGRESS),
            "case \"read {read}\": expected the request still running, got {progress:?}"
        );
    }
    let after = held_record(&engine, token);
    assert_eq!(
        before, after,
        "case \"a request still running\": expected the record unchanged by reads, got \
         {after}"
    );
}

#[tokio::test]
async fn a_token_the_region_holds_no_request_under_is_refused_with_the_registry_sentence() {
    let (router, engine) = router_with_engine();
    let rows = rows();
    seed(&engine, &rows);
    // A request ANOTHER Region holds, under a token this Region's registry holds none under:
    // the registry is regional, so the Region the request is signed for decides.
    let elsewhere = "monitored-elsewhere-running";
    store(&engine)
        .lock()
        .record_type_registration(StoredTypeRegistration::registering(
            OTHER_REGION,
            registry_type::RESOURCE,
            "Other::Region::Resource",
            "00000001",
            elsewhere,
            still_running(),
        ));
    let seeded = store(&engine).lock().type_registrations.len();

    // The spellings differ along what the modelled `RegistrationToken` admits — length 1..128
    // and pattern `^[a-zA-Z0-9][-a-zA-Z0-9]*$` — including each published length bound.
    let unresolved: Vec<(&str, String)> = vec![
        (
            "a token nothing issued",
            "not-a-token-this-service-issued".to_string(),
        ),
        (
            "a one-character token, the shortest the model admits",
            "x".to_string(),
        ),
        (
            "a token at the modelled maximum length",
            "t".repeat(128),
        ),
        (
            "a token of the uuid form this service mints",
            "0123abcd-4567-89ef-0123-456789abcdef".to_string(),
        ),
        (
            "a version-4 uuid no registration was issued under",
            "9f3c1b2a-5d6e-4f70-8a91-b2c3d4e5f607".to_string(),
        ),
        ("the all-zero uuid", "00000000-0000-0000-0000-000000000000".to_string()),
        (
            "a mixed-case alphanumeric token",
            "NotARegistrationToken99".to_string(),
        ),
        (
            "a token another Region holds a request under",
            elsewhere.to_string(),
        ),
    ];

    for (case, token) in &unresolved {
        let (status, body) = describe(&router, REGION, token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the read to be refused, got {status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code, REGISTRY_REFUSAL_CODE,
            "case {case:?}: expected {REGISTRY_REFUSAL_CODE}, got {code:?} in {body}"
        );
        assert_eq!(
            message, UNRESOLVED_TOKEN,
            "case {case:?}: expected {UNRESOLVED_TOKEN:?}, got {message:?}"
        );
    }

    // A refused read writes nothing: the registry holds exactly the requests it was seeded
    // with.
    assert_eq!(
        store(&engine).lock().type_registrations.len(),
        seeded,
        "case \"after every refused read\": expected the seeded requests only"
    );

    // The same token, signed for the Region that DOES hold the request, is monitored — so what
    // the refusal above is about is the Region, not the token's spelling.
    let (status, body) = describe(&router, OTHER_REGION, elsewhere).await;
    assert_eq!(
        status, 200,
        "case \"the token read in the Region that holds the request\": expected the read to \
         succeed, got {status} {body}"
    );
    let progress = xml_text(&body, "ProgressStatus");
    assert_eq!(
        progress.as_deref(),
        Some(registration_status::IN_PROGRESS),
        "case \"the token read in the Region that holds the request\": expected the request \
         still running, got {progress:?}"
    );
}
