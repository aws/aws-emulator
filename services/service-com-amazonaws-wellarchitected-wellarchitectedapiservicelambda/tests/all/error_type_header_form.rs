// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the FORM this service names an error shape in, in the
//! `x-amzn-errortype` response header a restJson1 client resolves that shape from.
//!
//! The rule these pin, over a TABLE of failures rather than one recorded request: every
//! failure this service answers names its error shape in `x-amzn-errortype` as the shape's
//! name, the separator, and the locator of the namespace that declares it — the qualified
//! form this service's wire carries, where the framework's renderers write the bare name —
//! while a success names no shape at all. The shape a client resolves is unchanged: it is
//! the text before the separator, and the body keeps naming the bare shape in `__type`.
//!
//! The rows differ along what the rule generalises over: the error CLASS (a missing
//! resource, a refused value, a name collision, an operation nothing serves), the
//! operation answering it, the SURFACE it is answered by (this service's own resource
//! operations, and the shared tagging frontend in front of its ARN-addressed tag
//! operations, whose error shape is in no model) and the LAYER the answer comes from — a
//! handler's modelled error, the boundary refusal raised before any handler runs, the
//! scaffold's 501, and a request that names no operation of this service at all, which no
//! operation answers.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. Resources, the tag requests and the request builders are the shared
//! `super::resource_tags`, `super::resource_tagging`, `super::untag_resource` and
//! `super::workload_harness` helpers, so no request in this crate's tests is spelled twice.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::resource_tagging::tag;
use super::resource_tags::{
    create_tagged, list_tags, tags_of, Kind, TAGGING_NOT_FOUND_CODE, KINDS,
};
use super::untag_resource::untag;
use super::workload_harness::{minimal_create, shape_named, Answer, Client};

/// The locator this service appends to a shape name: the separator, the locator root every
/// shape locator of this service stack hangs under, and the namespace that declares this
/// service's shapes — the namespace of its own Smithy shape id,
/// `com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda`
/// (`smithy_query(query="service_info")`). Spelled out here rather than read off the
/// implementation, so this suite pins the wire form itself.
const LOCATOR: &str = ":http://internal.amazon.com/coral/com.amazonaws.wellarchitected/";

/// The id form of a workload that does not exist: `WorkloadId` is a 32-character
/// lowercase hex string (`smithy_query(query="shape", shape="WorkloadId")`), so this is a
/// well-formed id the boundary accepts and the store does not hold.
const ABSENT_ID: &str = "fedcba9876543210fedcba9876543210";

/// The SHAPE `answer` names in `x-amzn-errortype`, asserted to be in this service's
/// qualified form whatever the shape is: a name, then this service's locator, and nothing
/// else.
///
/// Returned rather than compared here, so a row that knows which shape it expects asserts
/// that too and a row whose shape the framework chooses still pins the form.
fn qualified_shape(case: &str, answer: &Answer) -> String {
    let header = answer.error_type_header.clone();
    assert!(
        header.ends_with(LOCATOR),
        "case {case:?}: expected x-amzn-errortype to end with {LOCATOR:?}, got {answer:?}"
    );
    let shape = header[..header.len() - LOCATOR.len()].to_string();
    assert!(
        !shape.is_empty() && !shape.contains([':', '/', '#']),
        "case {case:?}: expected the locator to follow a bare shape name, got {header:?}"
    );
    assert_eq!(
        shape_named(&header),
        shape,
        "case {case:?}: a client resolving the shape from {header:?} must reach {shape:?}"
    );
    // The body still names the same shape, so a client reading either reaches one error.
    if let Some(named) = answer.body.get("__type").and_then(Value::as_str) {
        assert_eq!(
            shape_named(named),
            shape,
            "case {case:?}: the body's __type must name the same shape as the header, got \
             {answer:?}"
        );
    }
    shape
}

/// The whole assertion one row owes: the status, and `x-amzn-errortype` naming `shape` in
/// this service's qualified form.
fn assert_names_shape(case: &str, answer: &Answer, status: u16, shape: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {answer:?}"
    );
    let named = qualified_shape(case, answer);
    assert_eq!(
        named, shape,
        "case {case:?}: expected the header to name {shape:?}, got {:?}",
        answer.error_type_header
    );
}

/// Rule, the FAILURE half: every failure this service answers — whichever class,
/// operation, surface and layer answers it — names its shape in the qualified form.
#[tokio::test]
async fn every_failure_names_its_shape_with_this_services_locator() {
    let client = Client::new();
    let account = engine::emulator_account_id();
    let stored = tags_of(&[("Environment", "Production"), ("Owner", "platform")]);

    // ---- A missing resource on the tag plane: every operation of the family, and an
    // unresolvable ARN of every taggable kind.
    for (column, kind) in KINDS.into_iter().enumerate() {
        // The resource of this kind that DOES exist, so each row below is a real miss
        // rather than an empty store.
        let present = create_tagged(
            &client,
            kind,
            &format!("present / {kind:?}"),
            &format!("h{column}"),
            &stored,
        )
        .await;
        let absent = match kind {
            Kind::Workload => format!("arn:aws:wellarchitected:us-west-2:{account}:workload/{ABSENT_ID}"),
            Kind::CustomLens => format!("arn:aws:wellarchitected:us-west-2:{account}:lens/{ABSENT_ID}"),
            Kind::Profile => format!("arn:aws:wellarchitected:us-west-2:{account}:profile/{ABSENT_ID}"),
            Kind::ReviewTemplate => {
                format!("arn:aws:wellarchitected:us-west-2:{account}:review-template/{ABSENT_ID}")
            }
            Kind::AgentProfile => {
                format!("arn:aws:wellarchitected:us-west-2:{account}:agent-profile/{ABSENT_ID}")
            }
        };
        assert_ne!(
            absent, present,
            "case {kind:?}: the row must not name the resource that exists"
        );
        for (operation, answer) in [
            (
                "TagResource",
                tag(&client, &absent, &tags_of(&[("Fresh", "yes")])).await,
            ),
            ("UntagResource", untag(&client, &absent, &["Owner"]).await),
            ("ListTagsForResource", list_tags(&client, &absent).await),
        ] {
            assert_names_shape(
                &format!("{operation} / absent {kind:?}"),
                &answer,
                404,
                // The ARN-addressed tag operations are served by the shared tagging
                // frontend, so the shape named here is ITS `NotFoundException` and not
                // this service's modelled `ResourceNotFoundException` below.
                TAGGING_NOT_FOUND_CODE,
            );
        }
        // The same operations on the resource that EXISTS are successes, and a success
        // names no shape at all.
        for (operation, answer) in [
            (
                "TagResource",
                tag(&client, &present, &tags_of(&[("Fresh", "yes")])).await,
            ),
            ("UntagResource", untag(&client, &present, &["Fresh"]).await),
            ("ListTagsForResource", list_tags(&client, &present).await),
        ] {
            let case = format!("{operation} / present {kind:?}");
            assert!(
                answer.status < 400,
                "case {case:?}: expected a success, got {answer:?}"
            );
            assert!(
                answer.error_type_header.is_empty(),
                "case {case:?}: a success names no error shape, got {answer:?}"
            );
        }
    }

    // ---- A missing resource OFF the tag plane: the same shape, from another operation's
    // handler, so the form is not the tag path's.
    assert_names_shape(
        "GetWorkload / an id no workload carries",
        &client.get(ABSENT_ID).await,
        404,
        "ResourceNotFoundException",
    );
    assert_names_shape(
        "ListMilestones / an id no workload carries",
        &client
            .send(
                "POST",
                &format!("/workloads/{ABSENT_ID}/milestonesSummaries"),
                Some(json!({ "MaxResults": 10 })),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );

    // ---- A refused value, which the boundary answers BEFORE any handler runs: the
    // modelled `ValidationException` (`@httpError(400)`), on members bound to the body and
    // to the query string, and over two constraint classes (`@range`, `@length`).
    for (case, method, path, body) in [
        (
            "ListWorkloads / MaxResults past its published maximum",
            "POST",
            "/workloadsSummaries".to_string(),
            Some(json!({ "MaxResults": 51 })),
        ),
        (
            "ListProfiles / MaxResults below its published minimum",
            "GET",
            "/profileSummaries?MaxResults=0".to_string(),
            None,
        ),
        (
            "CreateWorkload / a name shorter than its published minimum",
            "POST",
            "/workloads".to_string(),
            Some(minimal_create("ab", "tok-too-short")),
        ),
    ] {
        let answer = client.send(method, &path, body).await;
        assert_names_shape(case, &answer, 400, "ValidationException");
    }

    // ---- A name already taken: the modelled `ConflictException` (`@httpError(409)`).
    let taken = "locator-conflict-host";
    let created = client.create(minimal_create(taken, "tok-locator-1")).await;
    assert_eq!(
        created.status, 200,
        "the colliding row needs a stored workload, got {created:?}"
    );
    assert!(
        created.error_type_header.is_empty(),
        "a success names no error shape, got {created:?}"
    );
    assert_names_shape(
        "CreateWorkload / a workload name already taken",
        &client.create(minimal_create(taken, "tok-locator-2")).await,
        409,
        "ConflictException",
    );
}

/// Rule, the half NO handler answers: a request refused at the boundary before any handler
/// runs, and one that names no operation of this service at all, carry the qualified form
/// too — whatever shape the framework names, since neither comes from a modelled handler.
#[tokio::test]
async fn a_failure_no_handler_answered_names_its_shape_the_same_way() {
    let client = Client::new();
    for (case, method, path) in [
        // A request carrying a value no `ReportFormat` publishes: the boundary's constraint
        // validation refuses it before the operation's handler runs.
        (
            "a value the boundary refuses",
            "GET",
            "/consolidatedReport?Format=CSV",
        ),
        // A path no operation of this service is bound to: the answer is the framework's
        // routing failure, which no operation produced.
        ("a path no operation names", "GET", "/no-such-path"),
        (
            "a method no operation binds on a modelled path",
            "PUT",
            "/workloadsSummaries",
        ),
    ] {
        let answer = client.send(method, path, None).await;
        assert!(
            answer.status >= 400,
            "case {case:?}: expected a failure, got {answer:?}"
        );
        let shape = qualified_shape(case, &answer);
        assert!(
            shape.ends_with("Exception") || shape == "NotImplemented",
            "case {case:?}: expected the header to name an error shape, got {shape:?}"
        );
    }
}

/// Rule, the SUCCESS half: an answer below 400 carries no `x-amzn-errortype` at all — the
/// header is not invented where there is no shape to name, whatever the operation and
/// whatever body it answers with.
#[tokio::test]
async fn a_success_names_no_error_shape() {
    let client = Client::new();
    let stored: BTreeMap<String, String> = tags_of(&[("Owner", "platform")]);
    let arn = create_tagged(
        &client,
        Kind::Workload,
        "a success names no error shape",
        "s0",
        &stored,
    )
    .await;
    let workload_id = arn.rsplit('/').next().unwrap_or_default().to_string();

    for (case, answer) in [
        ("GetWorkload", client.get(&workload_id).await),
        ("ListWorkloads", client.list(json!({ "MaxResults": 50 })).await),
        ("ListTagsForResource", list_tags(&client, &arn).await),
        (
            "TagResource",
            tag(&client, &arn, &tags_of(&[("Tier", "gold")])).await,
        ),
        ("UntagResource", untag(&client, &arn, &["Tier"]).await),
    ] {
        assert!(
            answer.status < 400,
            "case {case:?}: expected a success, got {answer:?}"
        );
        assert!(
            answer.error_type_header.is_empty(),
            "case {case:?}: a success names no error shape, got {answer:?}"
        );
    }
}
