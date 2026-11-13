// SPDX-License-Identifier: CC0-1.0

//! The resource-policy slice — `PutResourcePolicy` and `GetResourcePolicy` — driven
//! over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE: a resource-based policy is state keyed by the ARN of the project or report
//! group it is attached to. `PutResourcePolicy` stores the caller's document VERBATIM
//! against a resource this account holds, replacing whatever document that ARN already
//! carried (the document is the whole grant, so the last writer wins), and answers that
//! ARN; `GetResourcePolicy` hands the stored document back byte for byte and refuses an
//! ARN carrying no policy with `ResourceNotFoundException`; an ARN that names no
//! resource of this account is refused with `ResourceNotFoundException`, and a value
//! that is not the ARN of a resource a policy attaches to, or a policy that is not a
//! JSON document, with `InvalidInputException`; and the policy lives and dies with the
//! resource it is attached to. Sharing a resource is READ-ONLY sharing, so a document
//! containing any action outside the read-only consumer set of the kind of resource the
//! ARN names is refused with `InvalidInputException` and stores nothing.
//! `DeleteResourcePolicy` removes the attachment, which unshares the resource, and is
//! refused at the same boundary the write path is — the RESOURCE's existence — so an ARN
//! naming a resource this account holds is accepted whether or not an attachment is
//! keyed by it, while one naming no resource of this account is
//! `ResourceNotFoundException`.
//!
//! The harness (`call`, `create_project`, `emulator`, `emulator_on`, `error_type`,
//! `REGION`) is the one `project_build_lifecycle` already drives this service with, and
//! the report-group request builder is `report_group_lifecycle`'s.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, create_project, emulator, emulator_on, error_type, REGION};
use super::report_group_lifecycle::create_report_group_body;

/// The two resource kinds CodeBuild attaches a resource-based policy to —
/// `PutResourcePolicy` "Stores a resource policy for the ARN of a Project or
/// ReportGroup object"
/// (`smithy_query(query="operation",operation="PutResourcePolicy")` plus that
/// operation's doc), so these two are the whole class the rule generalises over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Project,
    ReportGroup,
}

const KINDS: [Kind; 2] = [Kind::Project, Kind::ReportGroup];

/// Create a resource of `kind` named `name` and hand back the ARN the service stored it
/// with, read from the service's own answer rather than assembled here.
async fn create_target(router: &MultiServiceRouter, kind: Kind, name: &str) -> String {
    match kind {
        Kind::Project => {
            let project = create_project(router, name, json!({})).await;
            project["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateProject {name:?} must report an ARN, got {project}"))
                .to_string()
        }
        Kind::ReportGroup => {
            let (status, body) = call(
                router,
                "CreateReportGroup",
                create_report_group_body(name, "TEST", json!({"exportConfigType": "NO_EXPORT"}), None),
            )
            .await;
            assert_eq!(
                status, 200,
                "CreateReportGroup {name:?}: expected 200, got {status} with {body}"
            );
            body["reportGroup"]["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateReportGroup {name:?} must report an ARN, got {body}"))
                .to_string()
        }
    }
}

/// Store `policy` against `arn`, asserting the call succeeded and answered that ARN.
async fn put(router: &MultiServiceRouter, case: &str, arn: &str, policy: &str) {
    let (status, body) = call(
        router,
        "PutResourcePolicy",
        json!({"resourceArn": arn, "policy": policy}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutResourcePolicy must succeed, got {status} with {body}"
    );
    assert_eq!(
        body["resourceArn"].as_str(),
        Some(arn),
        "case {case:?}: the answer states the ARN the policy was stored against, got {body}"
    );
}

/// Read the policy stored against `arn`, asserting the call succeeded, and hand back the
/// document it reported.
async fn get(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let (status, body) = call(router, "GetResourcePolicy", json!({"resourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetResourcePolicy must succeed, got {status} with {body}"
    );
    body["policy"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer states the policy document, got {body}"))
        .to_string()
}

/// Call `operation`, assert it was refused with HTTP 400 and `code`, and hand back the
/// message it stated.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    code: &str,
) -> String {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from {operation}, got {status} with {answer}"
    );
    assert_eq!(
        error_type(&answer),
        code,
        "case {case:?}: expected {code} from {operation}, got {answer}"
    );
    answer["message"].as_str().unwrap_or_default().to_string()
}

/// One read-only consumer action a policy on `arn` may grant, picked for the KIND of
/// resource the ARN names — the sets differ per kind, so a project's grant is not a
/// report group's. Sources: the CodeBuild user guide's "Sharing a project" and "Sharing
/// a report group" permission pages, as
/// `state::PolicyTarget::shareable_actions` records them.
fn shareable_action(arn: &str) -> &'static str {
    if arn.contains(":report-group/") {
        "codebuild:BatchGetReportGroups"
    } else {
        "codebuild:BatchGetProjects"
    }
}

/// A sharing policy document whose `Sid` is `label` — one of the grants the CodeBuild
/// user guide's "Sharing a Project" describes. Built with `serde_json`, so the rows that
/// vary the LABEL vary only the document's content, not its shape.
fn sharing_policy(arn: &str, label: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": label,
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::222222222222:root"},
            "Action": [shareable_action(arn)],
            "Resource": arn,
        }],
    })
    .to_string()
}

/// The documents the round-trip rows store: every class of text a JSON policy document
/// can be, so the assertion is about the bytes rather than about one recorded policy.
/// The labels span ASCII, scripts outside the basic plane (CJK, Cyrillic, Greek), a
/// right-to-left script, combining marks, an astral-plane emoji, text that carries
/// `\u` ESCAPES rather than the characters themselves (which only survive a store that
/// does not re-serialize), and quoting that a re-serializer would normalise.
fn documents(arn: &str) -> Vec<(String, String)> {
    let labels = [
        ("ascii", "SharedWithPartner"),
        ("cjk", "共有プロジェクト-功能"),
        ("cyrillic and greek", "Общий-доступ-Δοκιμή"),
        ("right to left", "مشاركة-المشروع"),
        ("combining marks", "e\u{0301}te\u{0301}-partage\u{0301}"),
        ("astral plane", "share-🚀-🙂\u{1F1EF}\u{1F1F5}"),
        ("escaped scalars", r"share-\u00e9-\u4f53"),
        ("quotes and escapes", r#"she said \"share\" \\ it"#),
    ];
    let mut rows: Vec<(String, String)> = labels
        .into_iter()
        .map(|(case, label)| (case.to_string(), sharing_policy(arn, label)))
        .collect();
    // Formatting a re-serializer would normalise away: newlines, tabs and padding
    // between the members, and the members in an order no canonical form keeps.
    let action = shareable_action(arn);
    rows.push((
        "pretty printed with tabs".to_string(),
        format!(
            "{{\n\t\"Statement\" : [\n\t\t{{\n\t\t\t\"Resource\":   \"{arn}\",\n\t\t\t\
             \"Action\": \"{action}\",\n\t\t\t\"Effect\" : \"Allow\",\n\t\t\t\
             \"Principal\": {{\"AWS\": \"*\"}}\n\t\t}}\n\t],\n\t\
             \"Version\":\"2012-10-17\"\n}}"
        ),
    ));
    // The smallest document that is still a JSON object, and one whose values are every
    // other JSON type.
    rows.push(("empty object".to_string(), "{}".to_string()));
    rows.push((
        "every json value type".to_string(),
        json!({
            "Version": "2012-10-17",
            "Statement": [],
            "numbers": [0, -1, 1.5e3],
            "flags": {"yes": true, "no": false},
            "nothing": Value::Null,
        })
        .to_string(),
    ));
    // The same GRANT written four ways, which is the dimension a store that decoded the
    // document to a policy model and re-emitted it would collapse: a statement's `Action`
    // and `Principal` are each a single value or a list of them, a principal is an account
    // root ARN or the bare account id, and a document carries whatever padding its author
    // wrote. Each form has to come back as it went in.
    let principal_arn = format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID);
    for (case, principal, granted) in [
        ("scalar action, root ARN principal", format!("\"{principal_arn}\""), format!("\"{action}\"")),
        ("listed action, root ARN principal", format!("\"{principal_arn}\""), format!("[\"{action}\"]")),
        (
            "listed action, listed account principal",
            format!("[\"{}\"]", engine::EMULATOR_ACCOUNT_ID),
            format!("[\"{action}\"]"),
        ),
    ] {
        rows.push((
            case.to_string(),
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
                 \"Principal\":{{\"AWS\":{principal}}},\"Action\":{granted},\
                 \"Resource\":\"{arn}\"}}]}}"
            ),
        ));
        rows.push((
            format!("{case}, padded between every member"),
            format!(
                "{{\"Version\": \"2012-10-17\", \"Statement\": [{{\"Effect\": \"Allow\", \
                 \"Principal\": {{\"AWS\": {principal}}}, \"Action\": {granted}, \
                 \"Resource\": \"{arn}\"}}]}}"
            ),
        ));
    }
    rows
}

/// RULE: a stored policy comes back byte for byte from the ARN it was stored against,
/// for either resource kind a policy attaches to, and attaching one to a resource this
/// account OWNS shares it OUT — so it stays in the account's own listing and appears in
/// neither of the account's shared listings, which report what other accounts shared
/// WITH it.
#[tokio::test]
async fn a_stored_policy_round_trips_byte_for_byte_for_either_resource_kind() {
    for kind in KINDS {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let arn = create_target(&router, kind, "policy-target").await;
        // A second resource of the same kind, never given a policy: the store is keyed
        // by ARN, so one resource's policy must never answer for another's.
        let untouched = create_target(&router, kind, "policy-untouched").await;

        for (case, document) in documents(&arn) {
            let case = format!("{kind:?}, {case}");
            put(&router, &case, &arn, &document).await;
            let read = get(&router, &case, &arn).await;
            assert_eq!(
                read, document,
                "case {case:?}: the stored document must come back verbatim, got {read:?}"
            );
            // The store holds exactly what the wire reported, so the read is computed
            // from state rather than from the request.
            let held = engine
                .state::<CodeBuildState>()
                .lock()
                .resource_policies
                .get(&arn)
                .map(|stored| stored.policy.clone());
            assert_eq!(
                held.as_deref(),
                Some(document.as_str()),
                "case {case:?}: the store must hold the document the write persisted"
            );

            // A second read answers the same thing: reading a policy changes nothing.
            let again = get(&router, &case, &arn).await;
            assert_eq!(
                again, read,
                "case {case:?}: a second read must answer the same document, got {again:?}"
            );

            let message = refusal(
                &router,
                &case,
                "GetResourcePolicy",
                json!({"resourceArn": untouched}),
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                message, "Resource ARN resource policy does not exist",
                "case {case:?}: a sibling resource carries no policy, got {message:?}"
            );
        }

        // The resource is this account's OWN, so attaching a policy to it shares it OUT:
        // neither shared listing may report it. Those listings are the CONSUMER's
        // inventory — "shared with the current Amazon Web Services account or user" — and
        // sharing runs one way, so with nothing shared WITH this account both answer an
        // empty list. The other direction (a resource another account owns and shared
        // with this one) is pinned by `list_shared_projects` and
        // `list_shared_report_groups`.
        let nothing: Vec<Value> = Vec::new();
        for (operation, member) in [
            ("ListSharedProjects", "projects"),
            ("ListSharedReportGroups", "reportGroups"),
        ] {
            let (status, body) = call(&router, operation, json!({})).await;
            assert_eq!(
                status, 200,
                "{kind:?}: {operation} must succeed, got {status} with {body}"
            );
            assert_eq!(
                body[member].as_array(),
                Some(&nothing),
                "{kind:?}: {operation} reports nothing shared with the account that owns \
                 the policied resource, got {body}"
            );
        }

        // The policied resource is still the account's own, so its own listing keeps
        // reporting it: `ListProjects` by name, `ListReportGroups` by ARN.
        let (operation, member, entry) = match kind {
            Kind::Project => ("ListProjects", "projects", "policy-target".to_string()),
            Kind::ReportGroup => ("ListReportGroups", "reportGroups", arn.clone()),
        };
        let (status, held_body) = call(&router, operation, json!({})).await;
        assert_eq!(
            status, 200,
            "{kind:?}: {operation} must succeed, got {status} with {held_body}"
        );
        assert!(
            held_body[member]
                .as_array()
                .is_some_and(|entries| entries.contains(&json!(entry))),
            "{kind:?}: the account's own listing must still report {entry}, got {held_body}"
        );
    }
}

/// RULE: a policy document is the whole grant, so a later write REPLACES the one the ARN
/// already carried — the last writer wins and nothing of an earlier document survives.
#[tokio::test]
async fn a_later_write_replaces_the_document_the_arn_already_carried() {
    for kind in KINDS {
        let router = emulator();
        let arn = create_target(&router, kind, "overwritten").await;

        // The sequence spans a document growing, shrinking, changing only in one
        // character, repeating an earlier one, and ending at the smallest object — so no
        // row can pass by merging, appending to or remembering an earlier write.
        let sequence = [
            sharing_policy(&arn, "first"),
            sharing_policy(&arn, "second-🙂"),
            sharing_policy(&arn, "second-🙃"),
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
            sharing_policy(&arn, "first"),
            "{}".to_string(),
        ];

        for (index, document) in sequence.iter().enumerate() {
            let case = format!("{kind:?}, write {index}");
            put(&router, &case, &arn, document).await;
            let read = get(&router, &case, &arn).await;
            assert_eq!(
                &read, document,
                "case {case:?}: the newest document is the one in force, got {read:?}"
            );
        }

        // The ARN carries ONE policy, not a history of them.
        let (status, body) = call(&router, "GetResourcePolicy", json!({"resourceArn": arn})).await;
        assert_eq!(
            (status, body["policy"].as_str()),
            (200, Some("{}")),
            "{kind:?}: the last write is what the ARN carries, got {body}"
        );
    }
}

/// RULE: an ARN of the right shape that this account holds no resource for is refused
/// with `ResourceNotFoundException` naming the ARN MEMBER at fault — this operation is
/// ARN-generic, so its refusal never points the caller at one of the two resource kinds
/// the value could have named — and nothing is stored.
#[tokio::test]
async fn an_arn_this_account_holds_no_resource_for_is_refused_as_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let project_arn = create_target(&router, Kind::Project, "held-project").await;
    let group_arn = create_target(&router, Kind::ReportGroup, "held-group").await;

    // The rows span a name never created, a held resource addressed in another region,
    // in another account and in another partition, and a name that differs from a held
    // one only in case — every way an ARN of the right SHAPE can still name nothing
    // here.
    let other_account = format!("arn:aws:codebuild:{REGION}:111122223333:project/held-project");
    let other_partition =
        format!("arn:aws-us-gov:codebuild:{REGION}:{}:project/held-project", engine::EMULATOR_ACCOUNT_ID);
    // Every row states the SAME sentence, whichever resource kind the ARN names: that is
    // the rule under test.
    let expected = "Resource ARN does not exist";
    let cases: [(&str, String); 7] = [
        (
            "a project never created",
            state::project_arn(REGION, "no-such-project"),
        ),
        (
            "a held project in another region",
            state::project_arn("eu-west-1", "held-project"),
        ),
        ("a held project in another account", other_account),
        ("a held project in another partition", other_partition),
        (
            "a held project in another case",
            state::project_arn(REGION, "HELD-PROJECT"),
        ),
        (
            "a report group never created",
            state::report_group_arn(REGION, "no-such-group"),
        ),
        (
            "a held report group in another region",
            state::report_group_arn("eu-west-1", "held-group"),
        ),
    ];

    for (case, arn) in cases {
        let message = refusal(
            &router,
            case,
            "PutResourcePolicy",
            json!({"resourceArn": arn, "policy": sharing_policy(&arn, "grant")}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected CodeBuild's absent-resource-ARN sentence, got {message:?}"
        );
        for kind in ["Project", "Report group", "report-group"] {
            assert!(
                !message.contains(kind),
                "case {case:?}: the sentence must not name the resource kind {kind:?}, \
                 got {message:?}"
            );
        }
        // Reading it back reports the ABSENT POLICY rather than a stored one: the
        // refused write stored nothing.
        let message = refusal(
            &router,
            case,
            "GetResourcePolicy",
            json!({"resourceArn": arn}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource ARN resource policy does not exist",
            "case {case:?}: a refused write must store nothing, got {message:?}"
        );
    }

    // The controls: the resources this account DOES hold take a policy.
    for arn in [project_arn, group_arn] {
        put(&router, "a held resource", &arn, &sharing_policy(&arn, "grant")).await;
    }
    assert_eq!(
        engine.state::<CodeBuildState>().lock().resource_policies.len(),
        2,
        "only the two held resources may carry a policy"
    );
}

/// RULE: a value that is not the ARN of a resource a policy attaches to, and a policy
/// that is not a JSON document, are refused with `InvalidInputException` — a malformed
/// request rather than a missing resource — and the sentence echoes nothing of the
/// request.
#[tokio::test]
async fn a_malformed_request_is_refused_as_invalid_input() {
    let router = emulator();
    let arn = create_target(&router, Kind::Project, "validated").await;

    // ARNs of CodeBuild resources a policy does NOT attach to, ARNs of other services,
    // and values that are not ARNs at all.
    let not_a_target = [
        ("a build ARN", state::build_arn(REGION, "validated:1111")),
        ("a fleet ARN", state::fleet_arn(REGION, "validated:1111")),
        ("a sandbox ARN", state::sandbox_arn(REGION, "validated:1111")),
        (
            "another service's ARN",
            format!("arn:aws:s3:::bucket/{}", "key"),
        ),
        (
            "a project ARN with an empty name",
            format!("arn:aws:codebuild:{REGION}:{}:project/", engine::EMULATOR_ACCOUNT_ID),
        ),
        (
            "an ARN with no partition",
            format!("arn::codebuild:{REGION}:{}:project/validated", engine::EMULATOR_ACCOUNT_ID),
        ),
        ("a bare project name", "validated".to_string()),
        ("text that is not an ARN", "not-an-arn".to_string()),
    ];

    for (case, value) in not_a_target {
        for operation in ["PutResourcePolicy", "GetResourcePolicy"] {
            let mut body = json!({"resourceArn": value});
            if operation == "PutResourcePolicy" {
                if let Value::Object(members) = &mut body {
                    members.insert("policy".to_string(), json!(sharing_policy(&arn, "grant")));
                }
            }
            let case = format!("{operation}, {case}");
            let message = refusal(&router, &case, operation, body, "InvalidInputException").await;
            assert_eq!(
                message, "Resource ARN is invalid",
                "case {case:?}: expected CodeBuild's invalid-ARN sentence, got {message:?}"
            );
            assert!(
                !message.contains(&value),
                "case {case:?}: the sentence must not echo the value, got {message:?}"
            );
        }
    }

    // `policy` is documented as "A JSON-formatted resource policy", and a policy
    // document is a JSON object: every other form of text is not one.
    let not_a_document = [
        ("plain text", "this is not json".to_string()),
        ("a JSON array", json!([{"Effect": "Allow"}]).to_string()),
        ("a JSON string", json!("Allow").to_string()),
        ("a JSON number", "42".to_string()),
        ("a JSON boolean", "true".to_string()),
        ("JSON null", "null".to_string()),
        ("an unterminated object", r#"{"Version": "2012-10-17""#.to_string()),
        ("two objects", "{}{}".to_string()),
        ("a single space", " ".to_string()),
    ];

    for (case, policy) in not_a_document {
        let message = refusal(
            &router,
            case,
            "PutResourcePolicy",
            json!({"resourceArn": arn, "policy": policy}),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            message, "Problem parsing given policy",
            "case {case:?}: expected CodeBuild's invalid-document sentence, got {message:?}"
        );
        assert!(
            !message.contains(policy.trim()) || policy.trim().is_empty(),
            "case {case:?}: the sentence must not echo the document, got {message:?}"
        );
        // The refused write left the resource carrying no policy.
        let message = refusal(
            &router,
            case,
            "GetResourcePolicy",
            json!({"resourceArn": arn}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource ARN resource policy does not exist",
            "case {case:?}: a refused write must store nothing, got {message:?}"
        );
    }

    // The control: the same ARN takes a document that IS one.
    put(&router, "a JSON object", &arn, &sharing_policy(&arn, "grant")).await;
}

/// RULE: a resource policy is meaningful only for a resource this account holds, so
/// deleting the resource drops the policy attached to it — and a resource of the same
/// name created afterwards carries no grant nobody wrote for it.
#[tokio::test]
async fn deleting_the_resource_drops_the_policy_attached_to_it() {
    // (the resource kind, the operation that deletes one, the member it addresses it by)
    let cases: [(Kind, &str, &str); 2] = [
        (Kind::Project, "DeleteProject", "name"),
        (Kind::ReportGroup, "DeleteReportGroup", "arn"),
    ];

    for (kind, operation, member) in cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let name = "deleted-target";
        let arn = create_target(&router, kind, name).await;
        let document = sharing_policy(&arn, "grant");
        put(&router, operation, &arn, &document).await;
        assert_eq!(
            get(&router, operation, &arn).await,
            document,
            "{operation}: the policy is in force before the delete"
        );

        let addressed = if member == "name" { name.to_string() } else { arn.clone() };
        let (status, body) = call(&router, operation, json!({member: addressed})).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed, got {status} with {body}"
        );

        assert!(
            engine
                .state::<CodeBuildState>()
                .lock()
                .resource_policies
                .is_empty(),
            "{operation}: the policy must go with the resource it was attached to"
        );
        let message = refusal(
            &router,
            operation,
            "GetResourcePolicy",
            json!({"resourceArn": arn}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource ARN resource policy does not exist",
            "{operation}: the deleted resource's policy must be gone, got {message:?}"
        );

        // A resource created again under the same name — so with the same ARN — starts
        // unshared.
        let again = create_target(&router, kind, name).await;
        assert_eq!(
            again, arn,
            "{operation}: recreating {name:?} must mint the same ARN"
        );
        let message = refusal(
            &router,
            operation,
            "GetResourcePolicy",
            json!({"resourceArn": again}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource ARN resource policy does not exist",
            "{operation}: a recreated resource carries no earlier grant, got {message:?}"
        );
    }
}

/// The IAM principal every statement below grants to — a fixed account root, so no row
/// varies in anything but the ACTIONS it names.
const PRINCIPAL: &str = "arn:aws:iam::222222222222:root";

/// Per resource kind: the resource-type token CodeBuild names the kind by when it
/// refuses a policy for it, the read-only consumer actions a policy on that kind may
/// grant, and actions it may not.
///
/// The allowed sets are hand-written from the CodeBuild user guide's sharing-permission
/// pages ("Sharing a report group" lists the four report-group actions, "Sharing a
/// project" the three project ones); the model types `policy` only as a
/// `NonEmptyString`, so neither set is modelled. The disallowed rows span a delete, an
/// update and a start on the kind itself, plus the OTHER kind's read action — which is
/// how the two sets are shown to be per-kind rather than one shared list.
fn policy_actions(kind: Kind) -> (&'static str, &'static [&'static str], &'static [&'static str]) {
    match kind {
        Kind::Project => (
            "codebuild:Project",
            &[
                "codebuild:BatchGetProjects",
                "codebuild:BatchGetBuilds",
                "codebuild:ListBuildsForProject",
            ],
            &[
                "codebuild:DeleteProject",
                "codebuild:UpdateProject",
                "codebuild:StartBuild",
                "codebuild:BatchGetReportGroups",
            ],
        ),
        Kind::ReportGroup => (
            "codebuild:ReportGroup",
            &[
                "codebuild:BatchGetReportGroups",
                "codebuild:BatchGetReports",
                "codebuild:ListReportsForReportGroup",
                "codebuild:DescribeTestCases",
            ],
            &[
                "codebuild:DeleteReportGroup",
                "codebuild:UpdateReportGroup",
                "codebuild:CreateReportGroup",
                "codebuild:BatchGetProjects",
            ],
        ),
    }
}

/// One statement granting `action` — a single action or a list of them — on `arn`.
fn statement(action: Value, arn: &str) -> Value {
    json!({
        "Sid": "Shared",
        "Effect": "Allow",
        "Principal": {"AWS": PRINCIPAL},
        "Action": action,
        "Resource": arn,
    })
}

/// A document whose `Statement` is `statements` — itself either one statement or a list.
fn policy_of(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// The documents a policy write on `kind` must REFUSE: every form in which a document
/// can contain an action outside the kind's read-only set.
fn documents_containing_a_disallowed_action(kind: Kind, arn: &str) -> Vec<(String, String)> {
    let (_, allowed, disallowed) = policy_actions(kind);
    let mut rows: Vec<(String, String)> = disallowed
        .iter()
        .map(|action| {
            (
                format!("grants {action}"),
                policy_of(json!([statement(json!(action), arn)])),
            )
        })
        .collect();
    rows.extend([
        (
            "a list mixing an allowed and a disallowed action".to_string(),
            policy_of(json!([statement(
                json!([allowed[0], disallowed[0]]),
                arn
            )])),
        ),
        (
            "a wildcard over the service".to_string(),
            policy_of(json!([statement(json!("codebuild:*"), arn)])),
        ),
        (
            "a wildcard over everything".to_string(),
            policy_of(json!([statement(json!("*"), arn)])),
        ),
        (
            "another service's action".to_string(),
            policy_of(json!([statement(json!("s3:GetObject"), arn)])),
        ),
        (
            "an action that is not a string".to_string(),
            policy_of(json!([statement(json!(42), arn)])),
        ),
        (
            "one statement rather than a list of them".to_string(),
            policy_of(statement(json!(disallowed[0]), arn)),
        ),
        (
            "a disallowed statement after an allowed one".to_string(),
            policy_of(json!([
                statement(json!(allowed[0]), arn),
                statement(json!(disallowed[0]), arn),
            ])),
        ),
        (
            // A document names an action whatever case its member names are written in,
            // so casing cannot carry a grant past the check.
            "member names in another case".to_string(),
            json!({
                "version": "2012-10-17",
                "statement": [{
                    "effect": "Allow",
                    "principal": {"AWS": PRINCIPAL},
                    "action": disallowed[0],
                    "resource": arn,
                }],
            })
            .to_string(),
        ),
        (
            // The refusal is about what the document CONTAINS, so the effect a
            // disallowed action is named under does not matter.
            "a disallowed action under Deny".to_string(),
            policy_of(json!([{
                "Effect": "Deny",
                "Principal": {"AWS": PRINCIPAL},
                "Action": disallowed[0],
                "Resource": arn,
            }])),
        ),
    ]);
    rows
}

/// The documents a policy write on `kind` must ACCEPT: every read-only action of the
/// kind's own set, the whole set at once, that set written in another case (an action
/// names the same operation however it is cased), and documents that name no action at
/// all.
fn documents_containing_only_shareable_actions(kind: Kind, arn: &str) -> Vec<(String, String)> {
    let (_, allowed, _) = policy_actions(kind);
    let mut rows: Vec<(String, String)> = allowed
        .iter()
        .map(|action| {
            (
                format!("grants only {action}"),
                policy_of(json!([statement(json!(action), arn)])),
            )
        })
        .collect();
    rows.extend([
        (
            "grants the whole read-only set".to_string(),
            policy_of(json!([statement(json!(allowed), arn)])),
        ),
        (
            "grants a read-only action in another case".to_string(),
            policy_of(json!([statement(
                json!(allowed[0].to_lowercase()),
                arn
            )])),
        ),
        (
            "a statement naming no action".to_string(),
            policy_of(json!([{
                "Effect": "Allow",
                "Principal": {"AWS": PRINCIPAL},
                "Resource": arn,
            }])),
        ),
        ("a document with no statement".to_string(), "{}".to_string()),
    ]);
    rows
}

/// RULE: sharing a CodeBuild resource gives a consumer READ access to it, so a policy
/// may contain only the read-only consumer actions of the kind of resource its ARN
/// names; a document containing any other action is refused with
/// `InvalidInputException`, naming that kind, and nothing is stored or replaced.
#[tokio::test]
async fn a_policy_containing_an_action_outside_the_kinds_read_only_set_is_refused() {
    for kind in KINDS {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let arn = create_target(&router, kind, "action-checked").await;
        let (resource_type, _, _) = policy_actions(kind);
        let expected = format!(
            "Proposed policy was not valid: InvalidPolicy: Input policy containing action \
             that is not allowed for resource type {resource_type}"
        );

        // Nothing has been stored yet, so each refusal must leave the ARN carrying no
        // policy at all.
        for (case, document) in documents_containing_a_disallowed_action(kind, &arn) {
            let case = format!("{kind:?}, {case}");
            let message = refusal(
                &router,
                &case,
                "PutResourcePolicy",
                json!({"resourceArn": arn, "policy": document}),
                "InvalidInputException",
            )
            .await;
            assert_eq!(
                message, expected,
                "case {case:?}: expected CodeBuild's disallowed-action sentence, got {message:?}"
            );
            assert!(
                !message.contains(document.as_str()),
                "case {case:?}: the sentence must not echo the document, got {message:?}"
            );
            let absent = refusal(
                &router,
                &case,
                "GetResourcePolicy",
                json!({"resourceArn": arn}),
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                absent, "Resource ARN resource policy does not exist",
                "case {case:?}: a refused write must store nothing, got {absent:?}"
            );
        }

        // The positive controls: the same resource takes a document whose actions are
        // all read-only ones, and hands it back verbatim.
        for (case, document) in documents_containing_only_shareable_actions(kind, &arn) {
            let case = format!("{kind:?}, {case}");
            put(&router, &case, &arn, &document).await;
            let read = get(&router, &case, &arn).await;
            assert_eq!(
                read, document,
                "case {case:?}: a read-only grant must round-trip verbatim, got {read:?}"
            );
        }

        // A refused write leaves the document already in force untouched — it neither
        // stores nor clobbers.
        let in_force = sharing_policy(&arn, "in-force");
        put(&router, "the grant in force", &arn, &in_force).await;
        for (case, document) in documents_containing_a_disallowed_action(kind, &arn) {
            let case = format!("{kind:?}, {case}, over a stored grant");
            let message = refusal(
                &router,
                &case,
                "PutResourcePolicy",
                json!({"resourceArn": arn, "policy": document}),
                "InvalidInputException",
            )
            .await;
            assert_eq!(
                message, expected,
                "case {case:?}: expected CodeBuild's disallowed-action sentence, got {message:?}"
            );
            let read = get(&router, &case, &arn).await;
            assert_eq!(
                read, in_force,
                "case {case:?}: a refused write must leave the stored grant in force, got {read:?}"
            );
        }

        assert_eq!(
            engine
                .state::<CodeBuildState>()
                .lock()
                .resource_policies
                .len(),
            1,
            "{kind:?}: only the one accepted grant may be held"
        );
    }
}

/// Delete the policy attached to `arn`, asserting the call succeeded and answered a body
/// with no members (`DeleteResourcePolicyOutput` declares none).
async fn delete(router: &MultiServiceRouter, case: &str, arn: &str) {
    let (status, body) = call(router, "DeleteResourcePolicy", json!({"resourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteResourcePolicy must succeed, got {status} with {body}"
    );
    let empty = match &body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: the answer carries no members, got {body}"
    );
}

/// RULE: deleting the attachment unshares the resource — the document leaves the store
/// and a read reports the absent policy — and a resource this account holds takes the
/// delete whether or not an attachment is keyed by it, so deleting one that is not there
/// is accepted and changes nothing.
#[tokio::test]
async fn deleting_the_policy_unshares_the_resource_and_is_idempotent() {
    for kind in KINDS {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let arn = create_target(&router, kind, "unshared").await;
        // A sibling of the same kind, shared and left alone: the attachment is keyed by
        // ARN, so one resource's delete must never reach another's grant.
        let sibling = create_target(&router, kind, "unshared-sibling").await;
        let sibling_document = sharing_policy(&sibling, "sibling-grant");
        put(&router, "the sibling's grant", &sibling, &sibling_document).await;

        // Rows: a delete that removes a stored grant, two that find nothing left to
        // remove (so the call is idempotent), and a delete of a grant stored again after
        // one was removed.
        let rows = [
            ("a stored grant", true),
            ("nothing left to remove", false),
            ("still nothing left to remove", false),
            ("a grant stored again", true),
        ];
        for (case, stored_first) in rows {
            let case = format!("{kind:?}, {case}");
            if stored_first {
                let document = sharing_policy(&arn, "grant");
                put(&router, &case, &arn, &document).await;
                assert_eq!(
                    get(&router, &case, &arn).await,
                    document,
                    "case {case:?}: the grant is in force before the delete"
                );
            }
            delete(&router, &case, &arn).await;
            let message = refusal(
                &router,
                &case,
                "GetResourcePolicy",
                json!({"resourceArn": arn}),
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                message, "Resource ARN resource policy does not exist",
                "case {case:?}: the unshared resource carries no policy, got {message:?}"
            );
            assert_eq!(
                get(&router, &case, &sibling).await,
                sibling_document,
                "case {case:?}: the sibling's grant must survive another ARN's delete"
            );
        }

        // The resource itself survives being unshared: only the attachment was removed,
        // so the same ARN takes a policy again.
        let again = sharing_policy(&arn, "shared-again");
        put(&router, "shared again", &arn, &again).await;
        assert_eq!(
            get(&router, "shared again", &arn).await,
            again,
            "{kind:?}: an unshared resource can be shared again"
        );
        let held: Vec<String> = engine
            .state::<CodeBuildState>()
            .lock()
            .resource_policies
            .keys()
            .cloned()
            .collect();
        let mut expected = vec![arn.clone(), sibling.clone()];
        expected.sort();
        assert_eq!(
            held, expected,
            "{kind:?}: exactly the two shared resources may be held"
        );
    }
}

/// RULE: a value that is not the ARN of a resource a policy attaches to is a malformed
/// input on the DELETE path too, refused with `InvalidInputException` and CodeBuild's
/// invalid-ARN sentence, and nothing stored is touched.
#[tokio::test]
async fn deleting_a_policy_from_a_value_that_is_not_a_policy_target_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let arn = create_target(&router, Kind::Project, "delete-validated").await;
    let document = sharing_policy(&arn, "grant");
    put(&router, "the grant in force", &arn, &document).await;

    // The same class of values the write and the read path refuse: ARNs of CodeBuild
    // resources a policy does not attach to, another service's ARN, and values that are
    // not ARNs at all.
    let not_a_target = [
        ("a build ARN", state::build_arn(REGION, "delete-validated:1111")),
        ("a report ARN", state::report_arn(REGION, "group", "2222")),
        ("another service's ARN", "arn:aws:s3:::bucket/key".to_string()),
        ("a bare project name", "delete-validated".to_string()),
        ("text that is not an ARN", "not-an-arn".to_string()),
    ];
    for (case, value) in not_a_target {
        let message = refusal(
            &router,
            case,
            "DeleteResourcePolicy",
            json!({"resourceArn": value}),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            message, "Resource ARN is invalid",
            "case {case:?}: expected CodeBuild's invalid-ARN sentence, got {message:?}"
        );
        assert!(
            !message.contains(&value),
            "case {case:?}: the sentence must not echo the value, got {message:?}"
        );
    }

    // A refused delete removed nothing.
    assert_eq!(
        get(&router, "the grant in force", &arn).await,
        document,
        "a refused delete must leave the stored grant in force"
    );
}

/// RULE: the boundary the delete path refuses at is the RESOURCE's existence, not the
/// attachment's — an ARN of the right shape naming a resource this account does not hold
/// is refused with `ResourceNotFoundException`, exactly as the write path refuses it,
/// while an ARN naming a resource this account DOES hold is accepted whether or not an
/// attachment is keyed by it: the attachment leaves the store if one is there, nothing is
/// removed if none is, and no other ARN's attachment is touched.
#[tokio::test]
async fn deleting_a_policy_is_refused_for_an_absent_resource_and_accepted_without_one() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    // One SHARED resource of each kind, named by no row below: whatever a delete of an
    // ARN carrying no attachment removes, it must not be one of these.
    let mut shared: Vec<(Kind, String, String)> = Vec::new();
    for kind in KINDS {
        let arn = create_target(&router, kind, "delete-untouched").await;
        let document = sharing_policy(&arn, "untouched-grant");
        put(&router, "the grant left alone", &arn, &document).await;
        shared.push((kind, arn, document));
    }
    // A resource this account DOES hold that was never shared — the other way an ARN of
    // the right shape carries no attachment.
    let held_unshared = create_target(&router, Kind::Project, "delete-never-shared").await;

    // (the case, the ARN, whether this account holds the resource it names). The rows
    // span a name never created, a shared resource addressed in another region, another
    // account and another partition, a name differing from a shared one only in case,
    // both resource kinds, and a held resource carrying no grant.
    let other_account = format!("arn:aws:codebuild:{REGION}:111122223333:project/delete-untouched");
    let other_partition = format!(
        "arn:aws-us-gov:codebuild:{REGION}:{}:project/delete-untouched",
        engine::EMULATOR_ACCOUNT_ID
    );
    let rows: [(&str, String, bool); 8] = [
        (
            "a project never created",
            state::project_arn(REGION, "no-such-project"),
            false,
        ),
        (
            "a shared project in another region",
            state::project_arn("eu-west-1", "delete-untouched"),
            false,
        ),
        ("a shared project in another account", other_account, false),
        (
            "a shared project in another partition",
            other_partition,
            false,
        ),
        (
            "a shared project in another case",
            state::project_arn(REGION, "DELETE-UNTOUCHED"),
            false,
        ),
        (
            "a report group never created",
            state::report_group_arn(REGION, "no-such-group"),
            false,
        ),
        (
            "a shared report group in another region",
            state::report_group_arn("eu-west-1", "delete-untouched"),
            false,
        ),
        ("a held project never shared", held_unshared.clone(), true),
    ];

    for (case, arn, held) in rows {
        if held {
            // A resource this account holds takes the delete even with nothing attached.
            delete(&router, case, &arn).await;
        } else {
            // A resource this account does not hold is refused — the same answer, in the
            // same words, the WRITE path gives for this very ARN.
            for operation in ["PutResourcePolicy", "DeleteResourcePolicy"] {
                let message = refusal(
                    &router,
                    case,
                    operation,
                    json!({"resourceArn": arn, "policy": sharing_policy(&arn, "grant")}),
                    "ResourceNotFoundException",
                )
                .await;
                assert_eq!(
                    message, "Resource ARN does not exist",
                    "case {case:?} on {operation}: expected CodeBuild's absent-resource \
                     sentence, got {message:?}"
                );
                assert!(
                    !message.contains(&arn),
                    "case {case:?} on {operation}: the sentence must not echo the ARN, got \
                     {message:?}"
                );
            }
        }
        let message = refusal(
            &router,
            case,
            "GetResourcePolicy",
            json!({"resourceArn": arn}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource ARN resource policy does not exist",
            "case {case:?}: the ARN still carries no policy, got {message:?}"
        );
        // Every call above left the store exactly as it was: the two grants this account
        // holds, and no third record.
        for (kind, shared_arn, document) in &shared {
            assert_eq!(
                &get(&router, case, shared_arn).await,
                document,
                "case {case:?}: the {kind:?} grant must survive another ARN's delete"
            );
        }
        let held_keys: Vec<String> = engine
            .state::<CodeBuildState>()
            .lock()
            .resource_policies
            .keys()
            .cloned()
            .collect();
        let mut expected: Vec<String> = shared.iter().map(|(_, arn, _)| arn.clone()).collect();
        expected.sort();
        assert_eq!(
            held_keys, expected,
            "case {case:?}: exactly the two shared resources may be held"
        );
    }

    // The control, so no row above can pass against a delete that removes nothing at
    // all: deleting one shared resource's grant DOES unshare it, and the other kind's
    // grant stays in force.
    let (first_kind, first_arn, _) = shared[0].clone();
    delete(&router, "the shared resource's own grant", &first_arn).await;
    let message = refusal(
        &router,
        "the shared resource's own grant",
        "GetResourcePolicy",
        json!({"resourceArn": first_arn}),
        "ResourceNotFoundException",
    )
    .await;
    assert_eq!(
        message, "Resource ARN resource policy does not exist",
        "{first_kind:?}: the deleted grant must be gone, got {message:?}"
    );
    let (second_kind, second_arn, second_document) = shared[1].clone();
    assert_eq!(
        get(&router, "the other kind's grant", &second_arn).await,
        second_document,
        "{second_kind:?}: the other kind's grant must still be in force"
    );
}
