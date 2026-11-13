// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Rekognition's project POLICIES over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: RekognitionService.<Operation>`), dispatched through the
//! same [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `PutProjectPolicy` accepts a `PolicyDocument` only when the
//! document IS a policy — parseable JSON whose `Statement` names the resources it grants on
//! — and every resource it names is a model version of the project the policy is being
//! attached to; it then records the policy under its name, assigns it a fresh hexadecimal
//! `PolicyRevisionId` and answers with it, and a later write of that policy is accepted only
//! when it names the revision the policy currently carries. A document that is not a policy
//! and a document naming a resource that is not an existing model version are both refused
//! with the modelled 400 `MalformedPolicyDocumentException`, each with its own fixed wording,
//! and a refused call attaches nothing. `ListProjectPolicies` reports exactly the records the
//! project holds, one `MaxResults`-sized page at a time.
//!
//! Each test asserts that rule over a TABLE whose rows differ along what it generalises
//! over: how a document fails to be a policy (a parse failure, a non-object, a missing or
//! mis-typed `Statement`, a statement naming no resource), what an unacceptable resource is
//! (the project itself, another project's version, a version of this project that does not
//! exist, a wildcard, one bad entry among good ones), what an acceptable document looks like
//! (one statement or several, one resource or a list of them, the shortest and longest
//! `PolicyName`), and the page size (both published `ListProjectPoliciesPageSize` bounds and
//! one past each). The malformed-JSON and bare-project-ARN rows are the two the issue's
//! recorded exchange carries; the rest are the same rule at other inputs.
//!
//! Every assertion goes over HTTP and reads only the status, the wire error code and the
//! JSON body. The one thing the test writes directly into this service's store is a MODEL
//! VERSION of a project: a version is recorded by `CreateProjectVersion` / `CopyProjectVersion`,
//! each of which is still an unimplemented 501 (their own stub issues), so until one of them
//! is written there is no other way to have the resource a project policy grants on exist —
//! and with no version in the store, every syntactically valid document names a resource
//! that does not exist, which is exactly how AWS answers such a document.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::state;
use service_com_amazonaws_rekognition_rekognitionservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The dispatch helpers and the reduced reply come from the collection-lifecycle suite
// rather than being built a second time here.
use crate::face_collections::{call, label_less, Reply};

/// The region every call of this suite signs with, and therefore the region of every ARN
/// the service mints for it.
const REGION: &str = "us-east-1";

/// `MaxResults` is `ListProjectPoliciesPageSize`, `range 1..5` on `ListProjectPolicies`
/// (`smithy_query(query="operation",operation="ListProjectPolicies")`).
const POLICIES_PAGE_SIZE_MIN: i64 = 1;
const POLICIES_PAGE_SIZE_MAX: i64 = 5;

/// `PolicyName` is `ProjectPolicyName`, `len 1..128, pattern ^[a-zA-Z0-9_.\-]+$`
/// (same model query as above, on `PutProjectPolicy`).
const POLICY_NAME_MAX: usize = 128;

/// `PolicyRevisionId` is `ProjectPolicyRevisionId`, `len 0..64, pattern ^[0-9A-Fa-f]+$`
/// (same model query).
const POLICY_REVISION_ID_MAX: usize = 64;

/// The two wordings a refused document comes back with, both fixed text:
/// `rekognition.put-project-policy-malformed-document-rejected` records them verbatim.
const MALFORMED_POLICY_MESSAGE: &str = "Request contains a malformed policy.";
const INVALID_POLICY_RESOURCE_MESSAGE: &str =
    "Request contains an invalid resource in the provided policy.";

/// The emulator with this one service mounted, plus the engine it was mounted on, so a
/// test can record the model version a project policy grants on (see this module's docs).
fn harness_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still routes by the SigV4
    // credential scope, which is also where an ARN's region comes from.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Create a project over the wire and answer the ARN the service minted for it.
async fn create_project(router: &MultiServiceRouter, project_name: &str) -> String {
    let reply = call(router, "CreateProject", json!({ "ProjectName": project_name })).await;
    assert_eq!(
        reply.status, 200,
        "case {project_name}: the create must succeed, got {:?}",
        reply.body
    );
    reply
        .string("ProjectArn")
        .unwrap_or_else(|| panic!("case {project_name}: the create must answer a ProjectArn"))
}

/// Record a model version named `version_name` on the project named `project_name`, and
/// answer its ARN.
///
/// The operations that create a model version are unimplemented 501s, so this writes the
/// record they would write; everything the rule under test does with it then happens over
/// the wire.
fn record_model_version(
    engine: &Arc<engine::InMemoryEngine>,
    project_name: &str,
    version_name: &str,
) -> String {
    let created_at = SystemTime::now();
    let arn = state::project_version_arn(REGION, project_name, version_name, created_at);
    let store = state::service_state(&**engine).expect("this service's store");
    let mut store = store.lock();
    let project = store
        .projects
        .get_mut(project_name)
        .unwrap_or_else(|| panic!("the project {project_name} the test created"));
    project.versions.insert(
        version_name.to_string(),
        state::ProjectVersion::new(version_name.to_string(), arn.clone(), created_at),
    );
    arn
}

/// The ARN a model version named `version_name` of `project_name` would carry, without
/// recording it: a version-shaped ARN that names no version of the store.
fn unrecorded_version_arn(project_name: &str, version_name: &str) -> String {
    state::project_version_arn(REGION, project_name, version_name, SystemTime::now())
}

/// A project policy document granting `rekognition:CopyProjectVersion` on `resources`:
/// one statement, with `Resource` written as a bare string when it names one resource and
/// as a list when it names several — both forms IAM's JSON policy grammar admits.
fn policy_document(resources: &[&str]) -> String {
    let resource = if let [only] = resources {
        json!(only)
    } else {
        json!(resources)
    };
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowCopyForTrustedAccount",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": "rekognition:CopyProjectVersion",
            "Resource": resource,
        }],
    })
    .to_string()
}

/// `PutProjectPolicy` for `policy_name` on `project_arn`, naming `revision_id` when the
/// write asks to replace a revision.
async fn put_policy(
    router: &MultiServiceRouter,
    project_arn: &str,
    policy_name: &str,
    document: &str,
    revision_id: Option<&str>,
) -> Reply {
    let mut body = json!({
        "ProjectArn": project_arn,
        "PolicyName": policy_name,
        "PolicyDocument": document,
    });
    if let Some(revision) = revision_id {
        body["PolicyRevisionId"] = json!(revision);
    }
    call(router, "PutProjectPolicy", body).await
}

/// Every project policy `project_arn` holds, walked page by page through
/// `ListProjectPolicies` with `max_results` (the default page when it is `None`).
async fn list_policies(
    router: &MultiServiceRouter,
    project_arn: &str,
    max_results: Option<i64>,
) -> Vec<Value> {
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "ProjectArn": project_arn });
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(cursor) = &token {
            body["NextToken"] = json!(cursor);
        }
        let reply = call(router, "ListProjectPolicies", body).await;
        assert_eq!(
            reply.status, 200,
            "case MaxResults={max_results:?}: the list must succeed, got {:?}",
            reply.body
        );
        walked.extend(reply.list("ProjectPolicies"));
        token = reply.string("NextToken");
        if token.is_none() {
            return walked;
        }
    }
}

/// The policy named `policy_name` as `ListProjectPolicies` reports it, if the project holds
/// it.
async fn listed_policy(
    router: &MultiServiceRouter,
    project_arn: &str,
    policy_name: &str,
) -> Option<Value> {
    list_policies(router, project_arn, None)
        .await
        .into_iter()
        .find(|policy| member(policy, "PolicyName").as_deref() == Some(policy_name))
}

fn member(policy: &Value, name: &str) -> Option<String> {
    policy.get(name).and_then(Value::as_str).map(str::to_string)
}

fn timestamp(policy: &Value, name: &str) -> f64 {
    policy
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a {name} on {policy:?}"))
}

/// Assert that `revision_id` is a revision this service may have minted: hexadecimal, and
/// inside the modelled length.
fn assert_revision_id(case: &str, revision_id: &str) {
    assert!(
        !revision_id.is_empty()
            && revision_id.len() <= POLICY_REVISION_ID_MAX
            && revision_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "case {case:?}: expected a hexadecimal revision id of at most {POLICY_REVISION_ID_MAX} \
         characters, got {revision_id:?}"
    );
}

/// Assert that `reply` is the modelled refusal of a document, with the wording its class
/// carries.
fn assert_document_refused(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "MalformedPolicyDocumentException"),
        "case {case:?}: the document must be refused as malformed, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Message").as_deref(),
        Some(message),
        "case {case:?}: expected the recorded wording {message:?}, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused_as_malformed() {
    let (router, engine) = harness_with_engine();
    let project_arn = create_project(&router, "malformed-document-project").await;
    // A model version exists, so nothing below is refused for naming a resource that does
    // not exist: each row fails to be a policy at all.
    record_model_version(&engine, "malformed-document-project", "version-one");

    // Each row is a document that is not a policy, differing in HOW it is not one.
    for (case, document) in [
        // The row the recorded exchange carries.
        ("unterminated object", "{this is not valid json".to_string()),
        ("unterminated list", "[{\"Effect\":\"Allow\"}".to_string()),
        (
            "a trailing comma",
            r#"{"Statement":[{"Resource":"x"},]}"#.to_string(),
        ),
        ("a JSON string", json!("not a policy").to_string()),
        ("a JSON number", "17".to_string()),
        ("JSON null", "null".to_string()),
        ("a JSON list", r#"[{"Resource":"x"}]"#.to_string()),
        (
            "an object with no Statement",
            json!({ "Version": "2012-10-17" }).to_string(),
        ),
        (
            "Statement is a string",
            json!({ "Statement": "allow the copy" }).to_string(),
        ),
        (
            "Statement is an empty list",
            json!({ "Statement": [] }).to_string(),
        ),
        (
            "a statement naming no Resource",
            json!({ "Statement": [{ "Effect": "Allow", "Action": "rekognition:CopyProjectVersion" }] })
                .to_string(),
        ),
        (
            "Resource is a number",
            json!({ "Statement": [{ "Effect": "Allow", "Resource": 17 }] }).to_string(),
        ),
        (
            "a Resource list holding a non-string",
            json!({ "Statement": [{ "Effect": "Allow", "Resource": ["arn", 17] }] }).to_string(),
        ),
        (
            "one statement of several naming no Resource",
            json!({ "Statement": [
                { "Effect": "Allow", "Resource": "arn" },
                { "Effect": "Allow", "Action": "rekognition:CopyProjectVersion" },
            ] })
            .to_string(),
        ),
    ] {
        let reply = put_policy(&router, &project_arn, "malformed", &document, None).await;
        assert_document_refused(case, &reply, MALFORMED_POLICY_MESSAGE);
    }

    // A refused attach attaches nothing: the project still holds no policy at all.
    assert_eq!(
        list_policies(&router, &project_arn, None).await,
        Vec::<Value>::new(),
        "a refused document must leave the project without a policy"
    );
}

#[tokio::test]
async fn a_document_naming_a_resource_that_is_not_a_model_version_is_refused() {
    let (router, engine) = harness_with_engine();
    let project_arn = create_project(&router, "invalid-resource-project").await;
    let version_arn = record_model_version(&engine, "invalid-resource-project", "version-one");

    // A second project with its own model version, so "a version that exists" and "a
    // version of THIS project" can be told apart.
    let other_arn = create_project(&router, "other-project").await;
    let other_version_arn = record_model_version(&engine, "other-project", "version-one");

    let collection_arn = state::collection_arn(REGION, "a-collection");
    let unrecorded = unrecorded_version_arn("invalid-resource-project", "never-created");

    // Each row names a resource that is not a model version of this project, differing in
    // what it names instead.
    for (case, resources) in [
        // The row the recorded exchange carries: the project's own ARN, not a version's.
        ("the bare project ARN", vec![project_arn.clone()]),
        (
            "the project ARN without its label",
            vec![label_less(&project_arn)],
        ),
        (
            "a version-shaped ARN of a version that does not exist",
            vec![unrecorded.clone()],
        ),
        (
            "another project's model version",
            vec![other_version_arn.clone()],
        ),
        ("another project's ARN", vec![other_arn.clone()]),
        ("a collection of this account", vec![collection_arn.clone()]),
        ("a wildcard", vec!["*".to_string()]),
        (
            "a version-shaped ARN of another account",
            vec![version_arn.replace(engine::emulator_account_id(), "123456789012")],
        ),
        (
            "one unknown version beside an existing one",
            vec![version_arn.clone(), unrecorded.clone()],
        ),
    ] {
        let borrowed: Vec<&str> = resources.iter().map(String::as_str).collect();
        let reply = put_policy(
            &router,
            &project_arn,
            "invalid-resource",
            &policy_document(&borrowed),
            None,
        )
        .await;
        assert_document_refused(case, &reply, INVALID_POLICY_RESOURCE_MESSAGE);
    }

    // Neither project was changed by any of the refusals.
    for arn in [&project_arn, &other_arn] {
        assert_eq!(
            list_policies(&router, arn, None).await,
            Vec::<Value>::new(),
            "case {arn}: a refused document must leave the project without a policy"
        );
    }
}

#[tokio::test]
async fn an_accepted_policy_is_recorded_and_reported_back() {
    let (router, engine) = harness_with_engine();
    let project_arn = create_project(&router, "accepted-policy-project").await;
    let first = record_model_version(&engine, "accepted-policy-project", "version-one");
    let second = record_model_version(&engine, "accepted-policy-project", "version-two");

    // Each row is an acceptable document, differing in the form the policy takes and in the
    // `PolicyName` it is attached under — including both published name bounds.
    let long_name = "p".repeat(POLICY_NAME_MAX);
    let cases: Vec<(&str, String, String)> = vec![
        (
            "one statement, one resource",
            "copy-one".to_string(),
            policy_document(&[first.as_str()]),
        ),
        (
            "one statement, both resources",
            "copy-both".to_string(),
            policy_document(&[first.as_str(), second.as_str()]),
        ),
        (
            "the resource without its ARN label",
            "copy-label-less".to_string(),
            policy_document(&[label_less(&first).as_str()]),
        ),
        (
            "Statement as a single object",
            "copy-single-statement".to_string(),
            json!({
                "Version": "2012-10-17",
                "Statement": { "Effect": "Allow", "Resource": first.clone() },
            })
            .to_string(),
        ),
        (
            "two statements, one resource each",
            "copy-two-statements".to_string(),
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Resource": first.clone() },
                    { "Effect": "Allow", "Resource": second.clone() },
                ],
            })
            .to_string(),
        ),
        (
            "the shortest policy name",
            "p".to_string(),
            policy_document(&[first.as_str()]),
        ),
        (
            "the longest policy name",
            long_name.clone(),
            policy_document(&[second.as_str()]),
        ),
    ];

    for (case, policy_name, document) in &cases {
        let reply = put_policy(&router, &project_arn, policy_name, document, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: an acceptable document must be accepted, got {:?}",
            reply.body
        );
        let revision = reply
            .string("PolicyRevisionId")
            .unwrap_or_else(|| panic!("case {case:?}: the attach must answer a PolicyRevisionId"));
        assert_revision_id(case, &revision);

        // The read-back: what the attach recorded is what the list reports.
        let listed = listed_policy(&router, &project_arn, policy_name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the attached policy must be listed"));
        assert_eq!(
            (
                member(&listed, "ProjectArn"),
                member(&listed, "PolicyName"),
                member(&listed, "PolicyDocument"),
                member(&listed, "PolicyRevisionId"),
            ),
            (
                Some(project_arn.clone()),
                Some(policy_name.clone()),
                Some(document.clone()),
                Some(revision.clone()),
            ),
            "case {case:?}: the listed policy must be the record the attach wrote, got {listed:?}"
        );
        assert!(
            timestamp(&listed, "LastUpdatedTimestamp") >= timestamp(&listed, "CreationTimestamp"),
            "case {case:?}: a policy was last updated no earlier than it was created, got {listed:?}"
        );
    }

    // Every row's policy is attached to the one project, and nothing else is.
    let mut listed: Vec<String> = list_policies(&router, &project_arn, None)
        .await
        .iter()
        .filter_map(|policy| member(policy, "PolicyName"))
        .collect();
    listed.sort();
    let mut expected: Vec<String> = cases.iter().map(|(_, name, _)| name.clone()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "the project must hold exactly the policies the accepted attaches wrote"
    );
}

#[tokio::test]
async fn a_policy_is_rewritten_only_by_naming_its_current_revision() {
    let (router, engine) = harness_with_engine();
    let project_arn = create_project(&router, "revision-project").await;
    let first = record_model_version(&engine, "revision-project", "version-one");
    let second = record_model_version(&engine, "revision-project", "version-two");
    let original = policy_document(&[first.as_str()]);
    let replacement = policy_document(&[second.as_str()]);

    let attached = put_policy(&router, &project_arn, "revisioned", &original, None).await;
    assert_eq!(
        attached.status, 200,
        "the first attach must succeed, got {:?}",
        attached.body
    );
    let first_revision = attached
        .string("PolicyRevisionId")
        .expect("the attach answers a revision");
    let created_at = timestamp(
        &listed_policy(&router, &project_arn, "revisioned")
            .await
            .expect("the attached policy is listed"),
        "CreationTimestamp",
    );

    // A write that does not name the current revision is refused, and changes nothing.
    for (case, revision, code) in [
        ("no revision at all", None, "ResourceAlreadyExistsException"),
        (
            // `ProjectPolicyRevisionId` is `len 0..64` but its pattern needs at least one
            // hexadecimal digit, so the empty revision is outside the model and is refused
            // at the boundary rather than read as naming no revision.
            "the empty revision",
            Some(String::new()),
            "ValidationException",
        ),
        (
            "a revision the service never minted",
            Some("deadbeef".to_string()),
            "InvalidPolicyRevisionIdException",
        ),
    ] {
        let reply = put_policy(
            &router,
            &project_arn,
            "revisioned",
            &replacement,
            revision.as_deref(),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, code),
            "case {case:?}: the write must be refused, got {:?}",
            reply.body
        );
        let listed = listed_policy(&router, &project_arn, "revisioned")
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the policy must survive the refusal"));
        assert_eq!(
            (
                member(&listed, "PolicyDocument"),
                member(&listed, "PolicyRevisionId"),
            ),
            (Some(original.clone()), Some(first_revision.clone())),
            "case {case:?}: a refused write must leave the policy as it was, got {listed:?}"
        );
    }

    // A revision named for a policy the project does not hold names no revision either.
    let unknown_policy = put_policy(
        &router,
        &project_arn,
        "never-attached",
        &original,
        Some(&first_revision),
    )
    .await;
    assert_eq!(
        (unknown_policy.status, unknown_policy.error_code().as_str()),
        (400, "InvalidPolicyRevisionIdException"),
        "a revision named for a policy that does not exist must be refused, got {:?}",
        unknown_policy.body
    );

    // Naming the current revision rewrites the policy: a fresh revision, the new document,
    // and the creation instant of the first attach.
    let updated = put_policy(
        &router,
        &project_arn,
        "revisioned",
        &replacement,
        Some(&first_revision),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "naming the current revision must be accepted, got {:?}",
        updated.body
    );
    let second_revision = updated
        .string("PolicyRevisionId")
        .expect("the rewrite answers a revision");
    assert_revision_id("the rewrite", &second_revision);
    assert_ne!(
        second_revision, first_revision,
        "every write assigns a fresh revision, got {second_revision:?} twice"
    );
    let listed = listed_policy(&router, &project_arn, "revisioned")
        .await
        .expect("the rewritten policy is listed");
    assert_eq!(
        (
            member(&listed, "PolicyDocument"),
            member(&listed, "PolicyRevisionId"),
        ),
        (Some(replacement.clone()), Some(second_revision.clone())),
        "the rewrite must be what the list reports, got {listed:?}"
    );
    assert_eq!(
        timestamp(&listed, "CreationTimestamp"),
        created_at,
        "a rewrite does not create the policy again, got {listed:?}"
    );
    assert!(
        timestamp(&listed, "LastUpdatedTimestamp") >= created_at,
        "a rewrite is no earlier than the attach it replaced, got {listed:?}"
    );

    // The revision the rewrite replaced is now stale, and a revision is hexadecimal, so the
    // current one still names the policy written in either case.
    let stale = put_policy(
        &router,
        &project_arn,
        "revisioned",
        &original,
        Some(&first_revision),
    )
    .await;
    assert_eq!(
        (stale.status, stale.error_code().as_str()),
        (400, "InvalidPolicyRevisionIdException"),
        "a replaced revision must no longer name the policy, got {:?}",
        stale.body
    );
    let upper = put_policy(
        &router,
        &project_arn,
        "revisioned",
        &original,
        Some(&second_revision.to_ascii_uppercase()),
    )
    .await;
    assert_eq!(
        upper.status, 200,
        "a hexadecimal revision names the same revision in either case, got {:?}",
        upper.body
    );

    // A policy whose document no longer resolves cannot be written even with the right
    // revision: the document rule holds for a rewrite exactly as for an attach.
    let revision = upper
        .string("PolicyRevisionId")
        .expect("the rewrite answers a revision");
    let refused = put_policy(
        &router,
        &project_arn,
        "revisioned",
        &policy_document(&[unrecorded_version_arn("revision-project", "never-created").as_str()]),
        Some(&revision),
    )
    .await;
    assert_document_refused(
        "a rewrite naming a resource that does not exist",
        &refused,
        INVALID_POLICY_RESOURCE_MESSAGE,
    );
}

#[tokio::test]
async fn project_policies_are_listed_one_page_at_a_time() {
    let (router, engine) = harness_with_engine();
    let project_arn = create_project(&router, "paged-policy-project").await;
    let version = record_model_version(&engine, "paged-policy-project", "version-one");
    let document = policy_document(&[version.as_str()]);

    // More policies than the largest page holds, so every page size walks several pages.
    let mut attached: Vec<String> = Vec::new();
    for index in 0..7 {
        let policy_name = format!("policy-{index}");
        let reply = put_policy(&router, &project_arn, &policy_name, &document, None).await;
        assert_eq!(
            reply.status, 200,
            "case {policy_name}: the attach must succeed, got {:?}",
            reply.body
        );
        attached.push(policy_name);
    }
    attached.sort();

    for max_results in [
        Some(POLICIES_PAGE_SIZE_MIN),
        Some(POLICIES_PAGE_SIZE_MAX),
        Some(3),
        None,
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "ProjectArn": project_arn });
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(cursor) = &token {
                body["NextToken"] = json!(cursor);
            }
            let reply = call(&router, "ListProjectPolicies", body).await;
            let page = reply.list("ProjectPolicies");
            let served = max_results.unwrap_or(POLICIES_PAGE_SIZE_MAX);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= served,
                "case MaxResults={max_results:?}: a page must hold at most {served} policies, \
                 got {}",
                page.len()
            );
            walked.extend(page.iter().filter_map(|policy| member(policy, "PolicyName")));
            token = reply.string("NextToken");
            assert_eq!(
                token.is_some(),
                walked.len() < attached.len(),
                "case MaxResults={max_results:?}: a NextToken must come back exactly while \
                 policies remain, walked {} of {}",
                walked.len(),
                attached.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, attached,
            "case MaxResults={max_results:?}: the pages must walk every policy exactly once"
        );
    }

    // A token this list did not mint names no position in it; the empty token is inside
    // `ExtendedPaginationToken`'s range and asks for the first page.
    for token in ["", "not-a-token", "6e6f742d612d746f6b656e"] {
        let reply = call(
            &router,
            "ListProjectPolicies",
            json!({ "ProjectArn": project_arn, "NextToken": token }),
        )
        .await;
        if token.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {token:?}: an empty token asks for the first page, got {:?}",
                reply.body
            );
            continue;
        }
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidPaginationTokenException"),
            "case {token:?}: a token the service did not mint must be refused, got {:?}",
            reply.body
        );
    }

    // One past each published bound of `ListProjectPoliciesPageSize` is outside the
    // model's range, so it is refused rather than served.
    for max_results in [POLICIES_PAGE_SIZE_MIN - 1, POLICIES_PAGE_SIZE_MAX + 1] {
        let reply = call(
            &router,
            "ListProjectPolicies",
            json!({ "ProjectArn": project_arn, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case MaxResults={max_results}: a page size outside the modelled range must be \
             refused, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_project_arn_naming_no_project_names_nothing_to_attach_to() {
    let (router, engine) = harness_with_engine();
    let project_arn = create_project(&router, "live-project").await;
    let version = record_model_version(&engine, "live-project", "version-one");
    let document = policy_document(&[version.as_str()]);

    // Each row is an ARN inside `ProjectArn`'s pattern that names no project of this store.
    for (case, arn) in [
        (
            "a project name nothing was created under",
            format!(
                "arn:aws:rekognition:{REGION}:{}:project/ghost-project/1700000000000",
                engine::emulator_account_id()
            ),
        ),
        (
            "a live project's name under another account",
            project_arn.replace(engine::emulator_account_id(), "123456789012"),
        ),
        (
            "a live project's name in another region",
            project_arn.replace(REGION, "eu-west-1"),
        ),
    ] {
        for operation in ["PutProjectPolicy", "ListProjectPolicies"] {
            let body = if operation == "PutProjectPolicy" {
                json!({
                    "ProjectArn": arn,
                    "PolicyName": "orphan",
                    "PolicyDocument": document,
                })
            } else {
                json!({ "ProjectArn": arn })
            };
            let reply = call(&router, operation, body).await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}/{operation}: an ARN naming no project must be refused, got {:?}",
                reply.body
            );
        }
    }

    // The live project is untouched by any of it.
    assert_eq!(
        list_policies(&router, &project_arn, None).await,
        Vec::<Value>::new(),
        "a refusal on another ARN must not attach anything here"
    );
}

#[tokio::test]
async fn a_project_refuses_to_hold_more_policies_than_its_capacity() {
    let (router, engine) = harness_with_engine();
    let project_arn = create_project(&router, "capacity-project").await;
    let version = record_model_version(&engine, "capacity-project", "version-one");
    let document = policy_document(&[version.as_str()]);

    for index in 0..state::project_policies::MAX_POLICIES_PER_PROJECT {
        let policy_name = format!("policy-{index:04}");
        let reply = put_policy(&router, &project_arn, &policy_name, &document, None).await;
        assert_eq!(
            reply.status, 200,
            "case {policy_name}: attaching up to the capacity must succeed, got {:?}",
            reply.body
        );
    }
    let over = put_policy(&router, &project_arn, "one-too-many", &document, None).await;
    assert_eq!(
        (over.status, over.error_code().as_str()),
        (400, "LimitExceededException"),
        "case policies={}: attaching past the capacity must be refused, got {:?}",
        state::project_policies::MAX_POLICIES_PER_PROJECT,
        over.body
    );

    // A refused attach stores nothing: the project holds exactly its capacity, and the
    // policy the refused request named is not among them.
    let listed: Vec<String> = list_policies(&router, &project_arn, None)
        .await
        .iter()
        .filter_map(|policy| member(policy, "PolicyName"))
        .collect();
    assert_eq!(
        listed.len(),
        state::project_policies::MAX_POLICIES_PER_PROJECT,
        "the store must hold exactly its capacity"
    );
    assert!(
        !listed.iter().any(|name| name == "one-too-many"),
        "the refused attach must not be among the stored policies"
    );

    // A rewrite of a policy already counted is not a new one, so it is accepted at the
    // capacity.
    let rewritten = list_policies(&router, &project_arn, None)
        .await
        .into_iter()
        .next()
        .expect("the project holds policies");
    let reply = put_policy(
        &router,
        &project_arn,
        &member(&rewritten, "PolicyName").expect("a listed policy carries its name"),
        &document,
        member(&rewritten, "PolicyRevisionId").as_deref(),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "a rewrite at the capacity must be accepted, got {:?}",
        reply.body
    );
}
