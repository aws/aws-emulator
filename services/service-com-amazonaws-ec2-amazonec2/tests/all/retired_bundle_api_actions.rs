// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two bundle API actions EC2 NO LONGER SERVES — `CancelBundleTask`
//! and `BundleInstance` — driven over the wire the service actually speaks (`ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness).
//!
//! The rule under test: AN API ACTION THIS SERVICE'S MODEL DOCUMENTS AS NO LONGER SUPPORTED
//! IS NOT SERVED AT ALL. EC2 refuses every call to one with HTTP 400 `UnsupportedOperation`
//! and one fixed sentence naming the action — `<Action> API is not supported.` — whatever the
//! request carries and whatever the account holds. The two actions this covers are the ones
//! whose own documentation says so: "CancelBundleTask is no longer supported because
//! BundleInstance, the operation it cancels, is no longer supported" and "BundleInstance is no
//! longer supported. To create an AMI, use CreateImage instead"
//! (`smithy_query(query="operation",operation="CancelBundleTask,BundleInstance")` for the
//! shapes, the operations' own docs for those sentences).
//!
//! Four things that refusal is never:
//!
//! * a `<bundleInstanceTask>` with HTTP 200, which would report a bundling or a cancellation
//!   nothing performed;
//! * `InvalidBundleID.NotFound` or an instance-id refusal, which would say the action is
//!   served and only the id the request named is wrong — so the rows name an id the account
//!   holds NOTHING under and an id that RESOLVES, and both are answered the same way;
//! * `DryRunOperation`, which would say the request would have succeeded;
//! * the scaffold's 501, which would say nobody has written the operation.
//!
//! No row is the specification. The rows span what the rule generalises over: which retired
//! action is called, which id the request names (absent, well-formed but held by nothing,
//! resolving to a stored task, a value that is not an id of that kind, an id of another
//! resource kind, empty text), which optional members come with it (an S3 bucket alone, every
//! `S3Storage` member at once, none), `DryRun` both ways, every member at once in either field
//! order, the Region the call is signed for, what the account holds when it is made, and how
//! many times it has been made already. Beside them: the media type and the `<RequestID>` of
//! the refusal's envelope, which the recorded response for `CancelBundleTask` also names, and
//! that a refused call leaves the store — the bundle task it named included — exactly as it
//! found it.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::bundle_tasks::seed_one_task;
use super::ec2_query::{
    call, call_with_content_type, described_instance_state, element, elements, error_code,
    error_message,
};
use super::ec2_query_error_envelope::{root_children, ERROR_ENVELOPE_ROOT, ERROR_REQUEST_ID};
use super::wire::{param, Param};

/// The two actions under test, as the model names them.
const CANCEL_BUNDLE_TASK: &str = "CancelBundleTask";
const BUNDLE_INSTANCE: &str = "BundleInstance";

/// The code EC2 refuses a call to an action it no longer serves with.
const UNSUPPORTED_OPERATION: &str = "UnsupportedOperation";

/// The sentence EC2 words that refusal with, per action.
///
/// Fixed text whose only variable part is the ACTION: the refusal names no parameter and no
/// resource, because nothing a caller could change about the request would make the action
/// servable. The `CancelBundleTask` sentence is the recorded response's own; the
/// `BundleInstance` one is the same sentence for the action retired beside it. Written out
/// here rather than read back from the service, so this suite pins the wording instead of
/// restating whatever the handler happens to carry.
const CANCEL_REFUSAL_MESSAGE: &str = "CancelBundleTask API is not supported.";
const BUNDLE_REFUSAL_MESSAGE: &str = "BundleInstance API is not supported.";

/// The media type an EC2 ec2Query response declares, which a refusal of either action
/// announces like every other response this service renders — the recorded response for
/// `CancelBundleTask` names the `charset` parameter, where the protocol's own default is the
/// bare `text/xml`.
const RESPONSE_CONTENT_TYPE: &str = "text/xml;charset=UTF-8";

/// A well-formed bundle task id (the `bun-` prefix a bundle task id carries) the account holds
/// no bundle task for.
const ABSENT_BUNDLE_ID: &str = "bun-0fedcba987654321f";

/// A well-formed instance id the account holds no instance for.
const ABSENT_INSTANCE_ID: &str = "i-0fedcba987654321f";

/// The AMI the rows that launch an instance launch from, in the long id form.
const AMI: &str = "ami-0123456789abcdef0";

/// The elements a SERVED call would have reported, none of which a refusal may carry:
/// `BundleInstanceResult$BundleTask` and `CancelBundleTaskResult$BundleTask` are both written
/// as `<bundleInstanceTask>`, and these are the members of the task itself.
const REPORTED_ELEMENTS: [&str; 5] = [
    "bundleInstanceTask",
    "bundleId",
    "state",
    "progress",
    "storage",
];

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The expected sentence for `action`.
fn refusal_message(action: &str) -> &'static str {
    match action {
        CANCEL_BUNDLE_TASK => CANCEL_REFUSAL_MESSAGE,
        BUNDLE_INSTANCE => BUNDLE_REFUSAL_MESSAGE,
        other => panic!("{other} is not one of the retired actions this suite drives"),
    }
}

/// Assert that `status` and `body` are the refusal of the retired `action`, and that they
/// report no bundle task of any kind.
fn assert_refused_as_retired(case: &str, action: &str, status: u16, body: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(body),
        UNSUPPORTED_OPERATION,
        "case {case:?}: expected the code {UNSUPPORTED_OPERATION}, got {body}"
    );
    assert_eq!(
        error_message(body),
        refusal_message(action),
        "case {case:?}: expected the refusal to be worded {:?}, got {:?} in {body}",
        refusal_message(action),
        error_message(body)
    );
    // Not a task dressed as a refusal either: none of the result shape's elements is in the
    // body, so nothing says a bundling was started or a cancellation performed.
    for reported in REPORTED_ELEMENTS {
        assert!(
            element(body, reported).is_none(),
            "case {case:?}: a refusal reports no <{reported}>, got {body}"
        );
    }
    // And it is a refusal of this service's own envelope: exactly one `<RequestID>`, the
    // element `super::ec2_query_error_envelope` owns the rule for.
    let request_ids = root_children(case, body, ERROR_ENVELOPE_ROOT, ERROR_REQUEST_ID);
    assert_eq!(
        request_ids.len(),
        1,
        "case {case:?}: expected exactly one <{ERROR_REQUEST_ID}> naming the answered \
         request, got {request_ids:?} in {body}"
    );
    assert!(
        !request_ids[0].trim().is_empty(),
        "case {case:?}: expected <{ERROR_REQUEST_ID}> to name the answered request, got \
         {request_ids:?} in {body}"
    );
}

/// Everything a served call — or a handler that wrote while refusing — would have moved: the
/// bundle tasks the account holds, its instances, and the caller-supplied bytes charged
/// against the store. Compared before and against after.
fn stored(engine: &engine::InMemoryEngine) -> (usize, usize, usize) {
    let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
    let guard = store.lock();
    (
        guard.bundle_tasks.len(),
        guard.instances.len(),
        guard.stored_bytes(),
    )
}

/// The `S3Storage` members a `BundleInstance` request can carry, as the form fields of the
/// nested `Storage$S3` structure — source:
/// `smithy_query(query="shape",shape="Storage,S3Storage")`, which names `AWSAccessKeyId`,
/// `Bucket`, `Prefix`, `UploadPolicy` and `UploadPolicySignature`.
fn every_storage_member() -> Vec<Param> {
    vec![
        param("Storage.S3.AWSAccessKeyId", "AKIAIOSFODNN7EXAMPLE"),
        param("Storage.S3.Bucket", "my-bundles"),
        param("Storage.S3.Prefix", "winsrv-a"),
        param("Storage.S3.UploadPolicy", "eyJleHBpcmF0aW9uIjoiMjAyNi0wMS0wMSJ9"),
        param("Storage.S3.UploadPolicySignature", "c2lnbmF0dXJl"),
    ]
}

/// `(case, action, the request's members)` for every shape a call to either retired action
/// can take, each row differing along what the rule generalises over.
fn request_rows() -> Vec<(String, &'static str, Vec<Param>)> {
    let bucket = || param("Storage.S3.Bucket", "my-bundles");
    let mut rows: Vec<(String, &'static str, Vec<Param>)> = vec![
        (
            "a cancel naming a well-formed bundle id the account holds nothing under".to_string(),
            CANCEL_BUNDLE_TASK,
            vec![param("BundleId", ABSENT_BUNDLE_ID)],
        ),
        (
            "a cancel naming a value that is not a bundle task id at all".to_string(),
            CANCEL_BUNDLE_TASK,
            vec![param("BundleId", "not-a-bundle-id")],
        ),
        (
            "a cancel naming an id of another resource kind".to_string(),
            CANCEL_BUNDLE_TASK,
            vec![param("BundleId", ABSENT_INSTANCE_ID)],
        ),
        (
            "a cancel naming an empty bundle id".to_string(),
            CANCEL_BUNDLE_TASK,
            vec![param("BundleId", "")],
        ),
        (
            "a cancel under the DryRun probe, which asks whether it would have succeeded"
                .to_string(),
            CANCEL_BUNDLE_TASK,
            vec![param("BundleId", ABSENT_BUNDLE_ID), param("DryRun", "true")],
        ),
        (
            "a cancel with DryRun asked for and declined".to_string(),
            CANCEL_BUNDLE_TASK,
            vec![param("BundleId", ABSENT_BUNDLE_ID), param("DryRun", "false")],
        ),
        (
            "a bundle of an instance the account does not hold".to_string(),
            BUNDLE_INSTANCE,
            vec![param("InstanceId", ABSENT_INSTANCE_ID), bucket()],
        ),
        (
            "a bundle naming a value that is not an instance id at all".to_string(),
            BUNDLE_INSTANCE,
            vec![param("InstanceId", "not-an-instance-id"), bucket()],
        ),
        (
            "a bundle naming an empty instance id".to_string(),
            BUNDLE_INSTANCE,
            vec![param("InstanceId", ""), bucket()],
        ),
        (
            "a bundle whose storage location names an empty bucket".to_string(),
            BUNDLE_INSTANCE,
            vec![
                param("InstanceId", ABSENT_INSTANCE_ID),
                param("Storage.S3.Bucket", ""),
            ],
        ),
        (
            "a bundle under the DryRun probe".to_string(),
            BUNDLE_INSTANCE,
            vec![
                param("InstanceId", ABSENT_INSTANCE_ID),
                bucket(),
                param("DryRun", "true"),
            ],
        ),
        (
            "a bundle with DryRun asked for and declined".to_string(),
            BUNDLE_INSTANCE,
            vec![
                param("InstanceId", ABSENT_INSTANCE_ID),
                bucket(),
                param("DryRun", "false"),
            ],
        ),
    ];

    // Every member at once, which is the request a client that still expected the action to
    // be served would send — and the same members in the reverse order, because a form
    // field's position is not part of the request either.
    let mut cancel_everything = vec![param("BundleId", ABSENT_BUNDLE_ID), param("DryRun", "true")];
    rows.push((
        "a cancel carrying every member at once".to_string(),
        CANCEL_BUNDLE_TASK,
        cancel_everything.clone(),
    ));
    cancel_everything.reverse();
    rows.push((
        "a cancel carrying every member at once, its fields in the reverse order".to_string(),
        CANCEL_BUNDLE_TASK,
        cancel_everything,
    ));

    let mut bundle_everything = vec![param("InstanceId", ABSENT_INSTANCE_ID)];
    bundle_everything.extend(every_storage_member());
    bundle_everything.push(param("DryRun", "true"));
    rows.push((
        "a bundle carrying every member of its storage location at once".to_string(),
        BUNDLE_INSTANCE,
        bundle_everything.clone(),
    ));
    bundle_everything.reverse();
    rows.push((
        "a bundle carrying every member at once, its fields in the reverse order".to_string(),
        BUNDLE_INSTANCE,
        bundle_everything,
    ));

    rows
}

/// Every shape of request to either retired action is refused for the action, and none of
/// them is served, probed or answered a task — and the call writes nothing.
#[tokio::test]
async fn every_call_to_a_retired_bundle_action_is_refused_for_the_action() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    for (case, action, params) in request_rows() {
        let (status, body) = call(&router, "us-west-2", action, &params).await;
        assert_refused_as_retired(&case, action, status, &body);
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "a refused call writes nothing: expected the store to end where it started at \
         {baseline:?}, got {:?}",
        stored(&engine)
    );
}

/// The answer does not depend on the Region the call is signed for: neither action is served
/// in any of them.
#[tokio::test]
async fn the_refusal_is_the_same_in_every_region() {
    let (router, _engine) = emulator();

    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        for (label, action, params) in [
            (
                "a cancel",
                CANCEL_BUNDLE_TASK,
                vec![param("BundleId", ABSENT_BUNDLE_ID)],
            ),
            (
                "a bundle",
                BUNDLE_INSTANCE,
                vec![
                    param("InstanceId", ABSENT_INSTANCE_ID),
                    param("Storage.S3.Bucket", "my-bundles"),
                ],
            ),
        ] {
            let case = format!("{label}, signed for {region}");
            let (status, body) = call(&router, region, action, &params).await;
            assert_refused_as_retired(&case, action, status, &body);
        }
    }
}

/// Nor does it depend on what the account holds: a cancel naming a bundle task that RESOLVES
/// is refused exactly like one naming an id nothing is held under, and the task is still
/// reported by the operation that does report it afterwards — so the refusal is the action's
/// and not a not-found, and it cancelled nothing.
#[tokio::test]
async fn a_cancel_of_a_stored_bundle_task_is_refused_and_leaves_the_task_alone() {
    let (router, engine) = emulator();
    let region = "us-east-1";
    let (bundle_id, stored_task) = seed_one_task(&engine);
    let held = stored(&engine);
    assert_eq!(
        held.0, 1,
        "the account must hold the one seeded bundle task, got {held:?}"
    );

    // Rows differing along which id the cancel names, with the stored task's own id among
    // them: the answer is the same for an id that resolves and one that does not.
    for (case, bundle) in [
        ("the id of the stored bundle task", bundle_id.as_str()),
        ("an id the account holds nothing under", ABSENT_BUNDLE_ID),
    ] {
        for attempt in 1..=2 {
            let attempted = format!("{case} (attempt {attempt})");
            let (status, body) = call(
                &router,
                region,
                CANCEL_BUNDLE_TASK,
                &[param("BundleId", bundle)],
            )
            .await;
            assert_refused_as_retired(&attempted, CANCEL_BUNDLE_TASK, status, &body);
        }
    }

    assert_eq!(
        stored(&engine),
        held,
        "the refused cancels leave the store as they found it, expected {held:?}, got {:?}",
        stored(&engine)
    );

    // Read back through the describe that does report a bundle task: the task is still there,
    // in the state it was stored in, so nothing moved it to `cancelling` or took it out.
    let (status, body) = call(
        &router,
        region,
        "DescribeBundleTasks",
        &[param("BundleId.1", &bundle_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeBundleTasks must still report the seeded task, got {status}: {body}"
    );
    assert_eq!(
        elements(&body, "bundleId"),
        vec![bundle_id.clone()],
        "the describe must report exactly the seeded task {bundle_id}, got {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        stored_task.state.as_deref(),
        "the refused cancels must leave the task's state at {:?}, got {body}",
        stored_task.state
    );
}

/// The same for the other half: a bundle of an instance the account really holds is refused
/// for the action, mints no bundle task, and leaves the instance exactly as it was.
#[tokio::test]
async fn a_bundle_of_a_held_instance_is_refused_and_leaves_the_instance_alone() {
    let (router, engine) = emulator();
    let region = "us-west-2";

    let (status, body) = call(
        &router,
        region,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the instance the bundle rows name must launch, got {status}: {body}"
    );
    let instance_id =
        element(&body, "instanceId").expect("RunInstances must report the instance it minted");
    let state_before = described_instance_state(&router, region, &instance_id).await;
    let held = stored(&engine);
    assert_eq!(
        (held.0, held.1),
        (0, 1),
        "the account must hold the one launched instance and no bundle task, got {held:?}"
    );

    // Rows differing along which instance the bundle names and how much of the storage
    // location it carries.
    for (case, params) in [
        (
            "the instance the account holds, with a bucket".to_string(),
            vec![
                param("InstanceId", &instance_id),
                param("Storage.S3.Bucket", "my-bundles"),
            ],
        ),
        ("the instance the account holds, with every storage member".to_string(), {
            let mut params = vec![param("InstanceId", &instance_id)];
            params.extend(every_storage_member());
            params
        }),
        (
            "the instance the account holds, under the DryRun probe".to_string(),
            vec![
                param("InstanceId", &instance_id),
                param("Storage.S3.Bucket", "my-bundles"),
                param("DryRun", "true"),
            ],
        ),
    ] {
        let (status, body) = call(&router, region, BUNDLE_INSTANCE, &params).await;
        assert_refused_as_retired(&case, BUNDLE_INSTANCE, status, &body);
    }

    assert_eq!(
        stored(&engine),
        held,
        "the refused bundles mint no task and leave the account's instances as they found \
         them, expected {held:?}, got {:?}",
        stored(&engine)
    );
    assert_eq!(
        described_instance_state(&router, region, &instance_id).await,
        state_before,
        "the refused bundles must leave the instance in the state it was launched into \
         ({state_before}), got {:?}",
        described_instance_state(&router, region, &instance_id).await
    );

    // And the account still holds no bundle task to report: a refusal that had quietly
    // minted one would show up here.
    let (status, body) = call(&router, region, "DescribeBundleTasks", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeBundleTasks must answer the empty report, got {status}: {body}"
    );
    assert!(
        elements(&body, "bundleId").is_empty(),
        "no bundle task may have been minted, got {body}"
    );
}

/// The refusal is announced as the XML media type the rest of this service's responses are,
/// `charset` parameter and all — a refused call is still an EC2 response, so a client decodes
/// its envelope the same way whether the operation served anything or not.
#[tokio::test]
async fn the_refusal_declares_the_xml_media_type_with_its_charset() {
    let (router, _engine) = emulator();

    for (case, action, params) in [
        (
            "a cancel naming a bundle id".to_string(),
            CANCEL_BUNDLE_TASK,
            vec![param("BundleId", ABSENT_BUNDLE_ID)],
        ),
        (
            "a bundle naming an instance and a bucket".to_string(),
            BUNDLE_INSTANCE,
            vec![
                param("InstanceId", ABSENT_INSTANCE_ID),
                param("Storage.S3.Bucket", "my-bundles"),
            ],
        ),
    ] {
        let (status, content_type, body) =
            call_with_content_type(&router, "eu-west-1", action, &params).await;
        assert_refused_as_retired(&case, action, status, &body);
        assert_eq!(
            content_type.as_deref(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the refusal to declare {RESPONSE_CONTENT_TYPE:?}, got \
             {content_type:?} with body {body}"
        );
    }
}

/// The same request asked repeatedly is answered the same way every time: the refusal
/// consumes nothing and remembers nothing, so there is no first call that behaves differently
/// from the second.
#[tokio::test]
async fn the_refusal_repeats_itself() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    for (case, action, params) in [
        (
            "a cancel".to_string(),
            CANCEL_BUNDLE_TASK,
            vec![param("BundleId", ABSENT_BUNDLE_ID)],
        ),
        (
            "a bundle".to_string(),
            BUNDLE_INSTANCE,
            vec![
                param("InstanceId", ABSENT_INSTANCE_ID),
                param("Storage.S3.Bucket", "my-bundles"),
            ],
        ),
    ] {
        let mut answers = Vec::new();
        for attempt in 0..3 {
            let attempted = format!("{case} (attempt {attempt})");
            let (status, body) = call(&router, "us-east-1", action, &params).await;
            assert_refused_as_retired(&attempted, action, status, &body);
            answers.push((status, error_code(&body), error_message(&body)));
        }
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: every attempt is answered the same way, got {answers:?}"
        );
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "repeating the calls stores nothing either, expected {baseline:?}, got {:?}",
        stored(&engine)
    );
}
