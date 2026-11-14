// SPDX-License-Identifier: CC0-1.0

//! The CLASS and the WORDING of the refusal `AssociateInstanceEventWindow` answers a target
//! that is not composed of one kind of target, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST carrying
//! `Action=AssociateInstanceEventWindow`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rule under test: the three sets of `InstanceEventWindowAssociationRequest` —
//! `InstanceIds`, `InstanceTags` and `DedicatedHostIds`
//! (`smithy_query(query="operation",operation="AssociateInstanceEventWindow")`) — are members
//! of ONE request structure, and "Only one type of target (instance IDs, Dedicated Host IDs,
//! or tags) can be specified with an event window", so a target composed of more than one of
//! them is an inadmissible VALUE of that structure rather than a pair of request parameters
//! that cannot be named together: EC2 answers HTTP 400 `InvalidParameterValue` with the
//! sentence it words for that class, which names the three admissible kinds of target and
//! nothing the request carried, and writes nothing. A target composed of exactly one kind is
//! served, so the refusal follows from the mixture rather than from the ids and tags.
//!
//! Every row is a distinct input along what the rule generalises over: which kinds the target
//! mixes and in which order, how many entries each of them carries, whether the mixture is in
//! ONE request or spread over the window's held target and the request's, and — for the
//! served rows — which single kind the target is composed of.
//!
//! The sentence is spelled out here as a literal rather than read from `ec2::state`: a suite
//! that imported the service's own constant would pass by agreeing with itself. The
//! fixtures (a window, instances, Dedicated Hosts) and the readers of a window's target are
//! the ones [`super::instance_event_window_associations`] drives the association's happy
//! path with, so one set of helpers serves both suites.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message};
use super::instance_event_window_associations::{
    allocate_hosts, associate, create_window, described, launch_instances, Target,
};

/// EC2's code for a request member whose value the operation cannot take.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The sentence EC2 words for a target association that is not composed of one kind of
/// target: fixed text naming the three admissible kinds, with no slot for the ids, the tags
/// or the window the request carried.
const ONE_KIND_SENTENCE: &str = "Target Associations must be composed entirely of Instance \
                                 IDs, Instance Tags, or Dedicated Host IDs.";

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Pin that associating `target` with `window_id` is refused HTTP 400
/// `InvalidParameterValue` carrying exactly [`ONE_KIND_SENTENCE`], and that the window's
/// target is `held` afterwards — what it was before the call.
async fn refused(
    case: &str,
    router: &MultiServiceRouter,
    window_id: &str,
    target: &Target,
    held: &Option<Target>,
) {
    let (status, body) = associate(router, window_id, target).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_PARAMETER_VALUE,
        "case {case:?}: expected the code {INVALID_PARAMETER_VALUE:?}, got {code:?} in {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, ONE_KIND_SENTENCE,
        "case {case:?}: expected the message {ONE_KIND_SENTENCE:?}, got {message:?}"
    );
    let (after, _) = described(case, router, window_id).await;
    assert_eq!(
        &after, held,
        "case {case:?}: a refused association must leave the window's target alone, \
         expected {held:?}, got {after:?}"
    );
}

/// One request naming more than one kind of target at once is refused with the class and the
/// sentence of the rule, and writes nothing.
///
/// The rows span every mixture of the three kinds — each of the three pairs, in both orders
/// of naming, and all three at once — and vary how many entries each kind carries, so neither
/// the pair nor the count is the specification.
#[tokio::test]
async fn one_request_naming_two_kinds_of_target_is_an_invalid_parameter_value() {
    let router = emulator();
    let instances = launch_instances(&router, 2).await;
    let hosts = allocate_hosts(&router, 2).await;
    let tag = |key: &str, value: &str| (key.to_string(), value.to_string());

    // (case, the target one request names)
    let rows: Vec<(&str, Target)> = vec![
        (
            "one instance id and one instance tag",
            Target {
                instance_ids: instances[..1].to_vec(),
                tags: vec![tag("Name", "web")],
                dedicated_host_ids: Vec::new(),
            },
        ),
        (
            "two instance ids and two instance tags",
            Target {
                instance_ids: instances.clone(),
                tags: vec![tag("Name", "web"), tag("Stage", "prod")],
                dedicated_host_ids: Vec::new(),
            },
        ),
        (
            "one instance id and one Dedicated Host id",
            Target {
                instance_ids: instances[..1].to_vec(),
                tags: Vec::new(),
                dedicated_host_ids: hosts[..1].to_vec(),
            },
        ),
        (
            "two Dedicated Host ids and one instance id",
            Target {
                instance_ids: instances[1..].to_vec(),
                tags: Vec::new(),
                dedicated_host_ids: hosts.clone(),
            },
        ),
        (
            "one instance tag and one Dedicated Host id",
            Target {
                instance_ids: Vec::new(),
                tags: vec![tag("Name", "web")],
                dedicated_host_ids: hosts[..1].to_vec(),
            },
        ),
        (
            "two instance tags and two Dedicated Host ids",
            Target {
                instance_ids: Vec::new(),
                tags: vec![tag("Owner", "platform"), tag("Stage", "dev")],
                dedicated_host_ids: hosts.clone(),
            },
        ),
        (
            "all three kinds at once",
            Target {
                instance_ids: instances[..1].to_vec(),
                tags: vec![tag("Name", "web")],
                dedicated_host_ids: hosts[..1].to_vec(),
            },
        ),
    ];

    for (case, target) in rows {
        // A window of its own per row, so what one row's refusal left behind cannot be read
        // as another row's.
        let window = create_window(&router, "mixed").await;
        let (before, _) = described(case, &router, &window).await;
        refused(case, &router, &window, &target, &before).await;
    }
}

/// The rule is about the WINDOW and not only about one request: a window already associated
/// with one kind of target cannot take on another kind beside it, and the refusal is the same
/// class and the same sentence as a request that named two kinds at once.
///
/// The rows are every ORDERED pair of distinct kinds — each kind held, each other kind
/// offered — so the refusal follows from the mixture the window would hold rather than from
/// which kind the request named.
#[tokio::test]
async fn associating_a_second_kind_onto_a_window_is_an_invalid_parameter_value() {
    let router = emulator();
    let instances = launch_instances(&router, 1).await;
    let hosts = allocate_hosts(&router, 1).await;

    // (the kind's name, the target that is composed of it)
    let kinds: Vec<(&str, Target)> = vec![
        ("instance ids", Target::of_instances(&instances)),
        ("instance tags", Target::of_tags(&[("Name", "web")])),
        ("Dedicated Host ids", Target::of_hosts(&hosts)),
    ];

    for (held_name, held) in &kinds {
        for (offered_name, offered) in &kinds {
            if held_name == offered_name {
                continue;
            }
            let case = format!("{offered_name} onto a window holding {held_name}");
            let window = create_window(&router, "held").await;
            let (status, body) = associate(&router, &window, held).await;
            assert_eq!(
                status, 200,
                "case {case:?}: seeding the window with {held_name} must succeed, got \
                 {status} with body {body}"
            );
            let (seeded, _) = described(&case, &router, &window).await;
            refused(&case, &router, &window, offered, &seeded).await;
        }
    }
}

/// The other side of the rule: a target composed of exactly ONE kind is served, whichever
/// kind it is and however many entries it carries — so the refusals above are the mixture
/// being refused rather than the ids, the tags or the window.
#[tokio::test]
async fn a_target_of_one_kind_is_served() {
    let router = emulator();
    let instances = launch_instances(&router, 2).await;
    let hosts = allocate_hosts(&router, 2).await;

    // (case, the target one request names, the kinds it must leave the window holding)
    let rows: Vec<(&str, Target)> = vec![
        ("one instance id", Target::of_instances(&instances[..1])),
        ("two instance ids", Target::of_instances(&instances)),
        ("one instance tag", Target::of_tags(&[("Name", "web")])),
        (
            "two instance tags",
            Target::of_tags(&[("Owner", "platform"), ("Stage", "prod")]),
        ),
        ("one Dedicated Host id", Target::of_hosts(&hosts[..1])),
        ("two Dedicated Host ids", Target::of_hosts(&hosts)),
    ];

    for (case, target) in rows {
        let window = create_window(&router, "single").await;
        let (status, body) = associate(&router, &window, &target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a target of one kind must be served, got {status} with body \
             {body}"
        );
        let (after, _) = described(case, &router, &window).await;
        assert_eq!(
            after,
            Some(target.clone()),
            "case {case:?}: expected the window to report the target {target:?}, got \
             {after:?}"
        );
    }
}
