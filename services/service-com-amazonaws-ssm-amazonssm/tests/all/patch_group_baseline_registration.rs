// SPDX-License-Identifier: CC0-1.0

//! `RegisterPatchBaselineForPatchGroup` on its own: the write that pairs a patch group with
//! the baseline its nodes are patched against.
//!
//! RULE under test: a register records the pairing between the patch group the request names
//! and the baseline its `BaselineId` resolves to at the endpoint the request was signed for,
//! keyed by that baseline's own operating system — so a later read of the group reports that
//! baseline and the baseline reports the group among its `PatchGroups`, whatever operating
//! system the baseline applies to, however the group is spelled, and whether the baseline was
//! named by its id or by its full ARN. Registering the baseline a group already holds is the
//! state the call asks for and is served; a DIFFERENT baseline of that operating system is
//! refused `AlreadyExistsException`; a `BaselineId` that could never name a baseline of this
//! endpoint is refused `InvalidResourceId`; and one that names no baseline reachable here is
//! refused `DoesNotExistException` — a refusal persisting nothing.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol") — and everything a case asserts is
//! read back through the service's own reads, so what is pinned is what the emulator
//! persisted.
//!
//! The suites are tables: the rows differ along what the rule generalises over — the
//! operating system the baseline applies to, the spelling of the group, the spelling of the
//! `BaselineId`, whether the baseline is one the account created or one Systems Manager
//! predefines, which refusal class the id falls in, and the Region the call is signed for.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the calls of a case are signed for. Deliberately not the emulator's default, so
/// what is read back proves the Region came from the request's SigV4 credential scope.
const REGION: &str = "eu-west-1";

/// A second Region, to pin that a registration is held at the endpoint it was made through.
const OTHER_REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// The operating system a read that names none is served for: "the default value is WINDOWS"
/// (`GetPatchBaselineForPatchGroupRequest$OperatingSystem`).
const WINDOWS: &str = "WINDOWS";

/// A patch this suite's baselines approve by name, so a create needs no per-operating-system
/// approval rule to approve something ("Approved Patches and Approval Rule, at least one must
/// be non empty").
const APPROVED_PATCH: &str = "KB4012212";

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where the Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        engine,
    ));
    router
}

/// Call `operation` signed for `region`, returning `(status, body)`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // The error shape travels in `__type`; fold in the header the framework also sets so one
    // accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call `operation` signed for `region`, asserting it succeeded.
async fn call_ok_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = call_in(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} in {region} to succeed, got {status} with {parsed:?}"
    );
    parsed
}

/// Call `operation` signed for [`REGION`], asserting it succeeded.
async fn call_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    call_ok_in(router, case, REGION, operation, body).await
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// A baseline of `operating_system` created at `region`'s endpoint, and the id its create
/// minted.
async fn baseline_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    operating_system: &str,
) -> String {
    let created = call_ok_in(
        router,
        case,
        region,
        "CreatePatchBaseline",
        &json!({
            "Name": name,
            "OperatingSystem": operating_system,
            "ApprovedPatches": [APPROVED_PATCH],
        }),
    )
    .await;
    created["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create reports a BaselineId, got {created}"))
        .to_string()
}

/// A baseline of `operating_system` created at [`REGION`]'s endpoint.
async fn baseline(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    operating_system: &str,
) -> String {
    baseline_in(router, case, REGION, name, operating_system).await
}

/// Register the baseline `reference` names for `patch_group`, signed for `region`.
async fn register_in(
    router: &MultiServiceRouter,
    region: &str,
    reference: &str,
    patch_group: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "RegisterPatchBaselineForPatchGroup",
        &json!({ "BaselineId": reference, "PatchGroup": patch_group }),
    )
    .await
}

/// Register the baseline `reference` names for `patch_group`, asserting it was served.
async fn register_ok(
    router: &MultiServiceRouter,
    case: &str,
    reference: &str,
    patch_group: &str,
) -> Value {
    let (status, parsed) = register_in(router, REGION, reference, patch_group).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the register of {reference} for {patch_group:?} to succeed, \
         got {status} with {parsed:?}"
    );
    parsed
}

/// What `GetPatchBaselineForPatchGroup` reports for `patch_group` and `operating_system` at
/// `region`'s endpoint.
async fn baseline_for_group_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    patch_group: &str,
    operating_system: &str,
) -> Value {
    call_ok_in(
        router,
        case,
        region,
        "GetPatchBaselineForPatchGroup",
        &json!({ "PatchGroup": patch_group, "OperatingSystem": operating_system }),
    )
    .await
}

/// The `BaselineId` `GetPatchBaselineForPatchGroup` reports for `patch_group` at [`REGION`].
async fn baseline_for_group(
    router: &MultiServiceRouter,
    case: &str,
    patch_group: &str,
    operating_system: &str,
) -> String {
    let reported = baseline_for_group_in(router, case, REGION, patch_group, operating_system).await;
    reported["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a read reports a BaselineId, got {reported}"))
        .to_string()
}

/// The `PatchGroups` the baseline `reference` names reports at [`REGION`].
async fn patch_groups_of(router: &MultiServiceRouter, case: &str, reference: &str) -> Vec<String> {
    let read = call_ok(
        router,
        case,
        "GetPatchBaseline",
        &json!({ "BaselineId": reference }),
    )
    .await;
    read["PatchGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|group| group.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The default baseline of `operating_system` at `region`'s endpoint, as that read names it.
async fn default_baseline_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operating_system: &str,
) -> String {
    let reported = call_ok_in(
        router,
        case,
        region,
        "GetDefaultPatchBaseline",
        &json!({ "OperatingSystem": operating_system }),
    )
    .await;
    reported["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a read reports a BaselineId, got {reported}"))
        .to_string()
}

/// A patch baseline's ARN at `region`'s endpoint, in the account every resource of this
/// emulator carries.
fn baseline_arn(region: &str, baseline_id: &str) -> String {
    format!(
        "arn:aws:ssm:{region}:{account}:patchbaseline/{baseline_id}",
        account = engine::emulator_account_id()
    )
}

/// How a row names the baseline it registers.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    /// The baseline's own id, as a create reports it.
    Id,
    /// The baseline's full ARN — "the name can be an Amazon Resource Name", the one parse
    /// every "name or ARN" member of this service reads.
    Arn,
}

/// RULE: a register records the pairing between the group and the baseline its `BaselineId`
/// resolves to, keyed by that baseline's operating system, and both reads of the pairing — the
/// group's and the baseline's — report it.
///
/// The rows differ along the operating system the baseline applies to (values of the modelled
/// `OperatingSystem` enum, `smithy_query(query="shape", shape="OperatingSystem")`), the
/// spelling of the group (every character class `PatchGroup`'s pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits, and its shortest value) and the spelling of the
/// `BaselineId`.
#[tokio::test]
async fn a_register_pairs_a_group_with_the_baseline_both_reads_then_report() {
    let router = service();
    let rows = [
        (
            "a Windows baseline named by its id, for a group whose name carries a space",
            WINDOWS,
            "windows fleet",
            Spelling::Id,
        ),
        (
            "an Amazon Linux 2023 baseline named by its ARN",
            "AMAZON_LINUX_2023",
            "prod/al2023",
            Spelling::Arn,
        ),
        (
            "an Ubuntu baseline, for a group spelling every punctuation the pattern admits",
            "UBUNTU",
            "team=platform+patching_group-1.0:eu@aws",
            Spelling::Id,
        ),
        (
            "a macOS baseline, for a group named in non-ASCII letters",
            "MACOS",
            "groupe de corrección",
            Spelling::Arn,
        ),
        (
            "a Red Hat baseline, for the shortest group a request may name",
            "REDHAT_ENTERPRISE_LINUX",
            "x",
            Spelling::Id,
        ),
    ];

    for (index, (case, operating_system, patch_group, spelling)) in rows.iter().enumerate() {
        let baseline_id = baseline(
            &router,
            case,
            &format!("Registration.Baseline.{index}"),
            operating_system,
        )
        .await;
        let reference = match spelling {
            Spelling::Id => baseline_id.clone(),
            Spelling::Arn => baseline_arn(REGION, &baseline_id),
        };

        // What the register answers back is the pairing it recorded: the baseline named the
        // way every read of a baseline the account created names it — its own id — and the
        // group the request named.
        let registered = register_ok(&router, case, &reference, patch_group).await;
        assert_eq!(
            (
                registered["BaselineId"].as_str(),
                registered["PatchGroup"].as_str()
            ),
            (Some(baseline_id.as_str()), Some(*patch_group)),
            "case {case:?}: expected the register to report {baseline_id} for \
             {patch_group:?}, got {registered}"
        );

        // The group's own read reports that baseline for that operating system.
        let reported = baseline_for_group_in(&router, case, REGION, patch_group, operating_system)
            .await;
        assert_eq!(
            (
                reported["BaselineId"].as_str(),
                reported["PatchGroup"].as_str()
            ),
            (Some(baseline_id.as_str()), Some(*patch_group)),
            "case {case:?}: expected the group to be patched against {baseline_id}, \
             got {reported}"
        );

        // And the baseline reports the group among its own patch groups.
        let groups = patch_groups_of(&router, case, &baseline_id).await;
        assert_eq!(
            groups,
            vec![(*patch_group).to_string()],
            "case {case:?}: expected the baseline to report the group once, got {groups:?}"
        );
    }
}

/// RULE: either spelling of a `BaselineId` resolves to the same baseline, and what a register
/// reports is how a read NAMES that baseline — the full ARN for one Systems Manager
/// predefines, which is not the account's.
#[tokio::test]
async fn a_predefined_baseline_registers_under_the_name_a_read_gives_it() {
    let router = service();
    let case = "the predefined Debian baseline";
    let operating_system = "DEBIAN";
    let patch_group = "debian fleet";

    // A baseline nobody created: the default of its operating system, named by its ARN.
    let predefined = default_baseline_in(&router, case, REGION, operating_system).await;
    assert!(
        predefined.starts_with("arn:aws:ssm:"),
        "case {case:?}: expected a predefined baseline to be named by its ARN, \
         got {predefined:?}"
    );
    let bare_id = predefined
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    let registered = register_ok(&router, case, &predefined, patch_group).await;
    assert_eq!(
        registered["BaselineId"].as_str(),
        Some(predefined.as_str()),
        "case {case:?}: expected the register to name the baseline the way a read does, \
         got {registered}"
    );

    // The same baseline named by its bare id is the SAME baseline, so the register is the
    // state the group already holds and is served rather than refused.
    let again = register_ok(&router, case, &bare_id, patch_group).await;
    assert_eq!(
        again, registered,
        "case {case:?}: expected either spelling to report the same pairing, got {again}"
    );

    let reported = baseline_for_group(&router, case, patch_group, operating_system).await;
    assert_eq!(
        reported, predefined,
        "case {case:?}: expected the group to be patched against {predefined}, got {reported:?}"
    );
    let groups = patch_groups_of(&router, case, &bare_id).await;
    assert_eq!(
        groups,
        vec![patch_group.to_string()],
        "case {case:?}: expected the baseline to report the group once, got {groups:?}"
    );
}

/// RULE: one group holds one baseline per operating system — registering the one it already
/// holds is served, a different one of that operating system is refused
/// `AlreadyExistsException` and persists nothing, and a baseline of ANOTHER operating system
/// is a second pairing of the same group rather than a conflict.
#[tokio::test]
async fn a_group_holds_one_baseline_per_operating_system() {
    let router = service();
    let case = "a group registered for two operating systems";
    let patch_group = "mixed fleet";
    let linux = "AMAZON_LINUX_2023";
    let held = baseline(&router, case, "Held.Linux", linux).await;
    let rival = baseline(&router, case, "Rival.Linux", linux).await;
    let windows = baseline(&router, case, "Held.Windows", WINDOWS).await;

    let first = register_ok(&router, case, &held, patch_group).await;

    // Registering the baseline the group already holds is the state the call asks for.
    let repeated = register_ok(&router, case, &held, patch_group).await;
    assert_eq!(
        repeated, first,
        "case {case:?}: expected a repeated register to report the same pairing, got {repeated}"
    );

    // A DIFFERENT baseline of that operating system is what `AlreadyExistsException` is for,
    // and the refusal names the group and the baseline that group holds.
    let (status, refused) = register_in(&router, REGION, &rival, patch_group).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "AlreadyExistsException"),
        "case {case:?}: expected the second baseline of {linux} to be refused, \
         got {status} with {refused}"
    );
    let message = refused["message"]
        .as_str()
        .or_else(|| refused["Message"].as_str())
        .unwrap_or_default();
    assert!(
        message.contains(patch_group) && message.contains(&held),
        "case {case:?}: expected the refusal to name the group and the baseline it holds, \
         got {message:?}"
    );

    // The refusal persisted nothing: the group still holds the baseline it held.
    let reported = baseline_for_group(&router, case, patch_group, linux).await;
    assert_eq!(
        reported, held,
        "case {case:?}: expected the refused register to leave {held} in place, got {reported:?}"
    );
    let rival_groups = patch_groups_of(&router, case, &rival).await;
    assert!(
        rival_groups.is_empty(),
        "case {case:?}: expected the refused baseline to hold no group, got {rival_groups:?}"
    );

    // A baseline of another operating system is a pairing of its own, so the group is patched
    // against one baseline per operating system.
    register_ok(&router, case, &windows, patch_group).await;
    let rows = [(linux, &held), (WINDOWS, &windows)];
    for (operating_system, expected) in rows {
        let reported = baseline_for_group(&router, case, patch_group, operating_system).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected the {operating_system} baseline {expected}, got {reported:?}"
        );
    }
    for baseline_id in [&held, &windows] {
        let groups = patch_groups_of(&router, case, baseline_id).await;
        assert_eq!(
            groups,
            vec![patch_group.to_string()],
            "case {case:?}: expected {baseline_id} to report the group once, got {groups:?}"
        );
    }
}

/// RULE: a `BaselineId` that could never name a baseline of this endpoint — an ARN of another
/// service, account or Region, or of a resource that is no patch baseline — is refused
/// `InvalidResourceId`, one that names no baseline reachable here is refused
/// `DoesNotExistException`, and neither registers anything.
///
/// The rows differ along which part of the id puts it out of reach.
#[tokio::test]
async fn a_register_is_refused_for_a_baseline_this_endpoint_holds_nothing_under() {
    let router = service();
    let case = "a baseline nothing here holds";
    let patch_group = "refused fleet";
    let existing = baseline(&router, case, "Reachable.Windows", WINDOWS).await;
    let account = engine::emulator_account_id();
    // The default of the group's operating system, which is what a read reports for a group
    // nothing has registered a baseline for.
    let default_windows = default_baseline_in(&router, case, REGION, WINDOWS).await;

    let rows = [
        (
            "an id of the published form that names no baseline",
            "pb-00000000000000000".to_string(),
            "DoesNotExistException",
        ),
        (
            "a bare value that is no baseline id at all",
            "no-such-patch-baseline".to_string(),
            "DoesNotExistException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:{REGION}:{account}:patchbaseline/{existing}"),
            "InvalidResourceId",
        ),
        (
            "an ARN of another account",
            format!("arn:aws:ssm:{REGION}:210987654321:patchbaseline/{existing}"),
            "InvalidResourceId",
        ),
        (
            "an ARN of another Region",
            baseline_arn(OTHER_REGION, &existing),
            "InvalidResourceId",
        ),
        (
            "an ARN of an ssm resource that is no patch baseline",
            format!("arn:aws:ssm:{REGION}:{account}:document/AWS-RunPatchBaseline"),
            "InvalidResourceId",
        ),
    ];

    for (row, reference, expected) in &rows {
        let (status, refused) = register_in(&router, REGION, reference, patch_group).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, *expected),
            "case {row:?}: expected {reference} to be refused {expected}, \
             got {status} with {refused}"
        );
        // Nothing was registered, so the group is still patched against the default of its
        // operating system.
        let reported = baseline_for_group(&router, row, patch_group, WINDOWS).await;
        assert_eq!(
            reported, default_windows,
            "case {row:?}: expected the refused register to persist nothing, got {reported:?}"
        );
    }
}

/// RULE: a registration is held at the endpoint the register was signed for — a baseline is
/// reached through the endpoint of one Region — so another Region's read of the same group
/// reports that Region's default, and the baseline's id names nothing there.
#[tokio::test]
async fn a_registration_is_held_at_the_endpoint_it_was_made_through() {
    let router = service();
    let case = "a group registered in one Region";
    let patch_group = "regional fleet";
    let here = baseline(&router, case, "Regional.Windows", WINDOWS).await;
    register_ok(&router, case, &here, patch_group).await;

    let reported = baseline_for_group(&router, case, patch_group, WINDOWS).await;
    assert_eq!(
        reported, here,
        "case {case:?}: expected the signing Region's read to report {here}, got {reported:?}"
    );

    // The other Region holds no registration for that group, so its read reports its own
    // default rather than a baseline another endpoint holds.
    let elsewhere_default = default_baseline_in(&router, case, OTHER_REGION, WINDOWS).await;
    let elsewhere =
        baseline_for_group_in(&router, case, OTHER_REGION, patch_group, WINDOWS).await;
    assert_eq!(
        elsewhere["BaselineId"].as_str(),
        Some(elsewhere_default.as_str()),
        "case {case:?}: expected the other Region to report its own default, got {elsewhere}"
    );

    // And the baseline of this Region names nothing at that endpoint, so a register of it
    // there is refused rather than recorded.
    let (status, refused) = register_in(&router, OTHER_REGION, &here, patch_group).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "DoesNotExistException"),
        "case {case:?}: expected a register of {here} in {OTHER_REGION} to be refused, \
         got {status} with {refused}"
    );

    // A read of either endpoint does not change what a later caller sees.
    let again = baseline_for_group(&router, case, patch_group, WINDOWS).await;
    assert_eq!(
        again, here,
        "case {case:?}: expected the registration to stand after the reads, got {again:?}"
    );
    let groups = patch_groups_of(&router, case, &here).await;
    assert_eq!(
        groups,
        vec![patch_group.to_string()],
        "case {case:?}: expected the baseline to report the group once, got {groups:?}"
    );
}
