// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a delete of a resource INSIDE a cluster leaves behind
//! while the removal it accepted is still running, driven over the wire through the
//! same `MultiServiceRouter` the production binary mounts.
//!
//! RULE: a delete this service ACCEPTS but finishes later answers with the resource
//! in its `DELETING` status and leaves it a resource of its cluster — readable
//! through the matching Describe, carried by the matching List, in the way of a
//! create of the same name, and answering a repeat of the delete with the same
//! record — until the removal the clock owes falls due; only then do the reads
//! report it absent and the name become free again.
//!
//! The suite asserts that rule over a TABLE whose rows differ along which resource is
//! deleted — a managed node group and a Fargate profile, the two children whose
//! delete this service accepts asynchronously — and therefore along the error class
//! each create answers a taken name with.
//!
//! Nothing here waits for a span: the end of a removal is waited for by LOOKING
//! AGAIN until the read reports the resource gone or a generous deadline passes, so
//! what is asserted is the state a call reaches and never how fast the host gets
//! there.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`; the cluster and node group requests are the shared ones in
//! `tests/all/nodegroup_lifecycle.rs`.

use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::nodegroup_lifecycle::{create_cluster, minimal_request};
use crate::wire::{call, emulator, encode_label, error_code, read_until, Answer};

/// Which of the two children a row is about.
#[derive(Clone, Copy, Debug)]
enum Child {
    Nodegroup,
    FargateProfile,
}

impl Child {
    /// The member the create, the delete and the read answer the resource under.
    fn document(self) -> &'static str {
        match self {
            Self::Nodegroup => "nodegroup",
            Self::FargateProfile => "fargateProfile",
        }
    }

    /// The member the matching List answers the names under.
    fn names(self) -> &'static str {
        match self {
            Self::Nodegroup => "nodegroups",
            Self::FargateProfile => "fargateProfileNames",
        }
    }

    /// The collection the create posts to and the List reads.
    fn collection(self, cluster: &str) -> String {
        let segment = match self {
            Self::Nodegroup => "node-groups",
            Self::FargateProfile => "fargate-profiles",
        };
        format!("/clusters/{}/{segment}", encode_label(cluster))
    }

    /// The one resource the Describe and the delete address.
    fn item(self, cluster: &str, name: &str) -> String {
        format!("{}/{}", self.collection(cluster), encode_label(name))
    }

    /// The smallest create request for this child.
    fn create_body(self, name: &str) -> serde_json::Value {
        match self {
            Self::Nodegroup => minimal_request(name),
            Self::FargateProfile => serde_json::json!({
                "fargateProfileName": name,
                "podExecutionRoleArn": format!(
                    "arn:aws:iam::{}:role/eksFargatePodExecutionRole",
                    engine::emulator_account_id()
                ),
                "selectors": [{ "namespace": "default" }],
            }),
        }
    }
}

async fn create_child(
    router: &MultiServiceRouter,
    child: Child,
    cluster: &str,
    name: &str,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &child.collection(cluster),
        &child.create_body(name).to_string(),
    )
    .await
}

async fn describe_child(
    router: &MultiServiceRouter,
    child: Child,
    cluster: &str,
    name: &str,
) -> Answer {
    call(router, REGION, "GET", &child.item(cluster, name), "").await
}

async fn delete_child(
    router: &MultiServiceRouter,
    child: Child,
    cluster: &str,
    name: &str,
) -> Answer {
    call(router, REGION, "DELETE", &child.item(cluster, name), "").await
}

async fn list_children(router: &MultiServiceRouter, child: Child, cluster: &str) -> Answer {
    call(router, REGION, "GET", &child.collection(cluster), "").await
}

/// The status the read reports for one child, or the empty string when it reports
/// none.
fn status_of(answer: &Answer, child: Child) -> String {
    answer.body[child.document()]["status"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// Whether the matching List carries `name`.
fn lists(answer: &Answer, child: Child, name: &str) -> bool {
    answer.body[child.names()]
        .as_array()
        .is_some_and(|names| names.iter().any(|entry| entry == name))
}

/// Read the child until it reports `status`, through the shared
/// [`crate::wire::read_until`] — the condition is waited for, never a span.
async fn child_until_status(
    router: &MultiServiceRouter,
    case: &str,
    child: Child,
    cluster: &str,
    name: &str,
    status: &str,
) -> Answer {
    read_until(
        case,
        &format!("{name:?} reports {status:?}"),
        || describe_child(router, child, cluster, name),
        |answer| answer.status == 200 && status_of(answer, child) == status,
    )
    .await
}

/// Read the child until the read reports it absent, waited for the same way.
async fn child_until_gone(
    router: &MultiServiceRouter,
    case: &str,
    child: Child,
    cluster: &str,
    name: &str,
) -> Answer {
    read_until(
        case,
        &format!("the removal of {name:?} finished"),
        || describe_child(router, child, cluster, name),
        |answer| answer.status == 404,
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE: an accepted delete leaves the child readable, listed and in the way of its
// own name in `DELETING` until the clock has finished the removal; only then is it
// absent.
// ---------------------------------------------------------------------------

/// The table spans the two children whose delete this service accepts
/// asynchronously, and with them the class each create answers a taken name with:
/// `CreateNodegroup` models `ResourceInUseException` (409) and
/// `CreateFargateProfile` models `InvalidRequestException` (400).
#[tokio::test]
async fn an_accepted_delete_leaves_the_child_deleting_until_the_removal_falls_due() {
    let router = emulator();
    let cluster = "removal-window";
    create_cluster(&router, cluster).await;

    struct Row {
        case: &'static str,
        child: Child,
        name: &'static str,
        taken_status: u16,
        taken_code: &'static str,
        /// Whether the matching List is served at all: `ListFargateProfiles` is
        /// still a scaffolded 501 in this emulator, so the profile row asserts the
        /// reads that ARE served and leaves the listing to that operation's own
        /// issue.
        listed: bool,
    }
    let rows = [
        Row {
            case: "a managed node group",
            child: Child::Nodegroup,
            name: "ng-removal",
            taken_status: 409,
            taken_code: "ResourceInUseException",
            listed: true,
        },
        Row {
            case: "a Fargate profile",
            child: Child::FargateProfile,
            name: "fp-removal",
            taken_status: 400,
            taken_code: "InvalidRequestException",
            listed: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let created = create_child(&router, row.child, cluster, row.name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup create expected 200, got {created:?}"
        );
        child_until_status(&router, case, row.child, cluster, row.name, "ACTIVE").await;

        let deleted = delete_child(&router, row.child, cluster, row.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from the delete, got {deleted:?}"
        );
        assert_eq!(
            status_of(&deleted, row.child),
            "DELETING",
            "case {case:?}: the delete answers the resource on its way out, got {deleted:?}"
        );

        // WHILE THE REMOVAL RUNS: the resource is still a resource of its cluster.
        let read = describe_child(&router, row.child, cluster, row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read must serve the resource while it is being deleted, \
             got {read:?}"
        );
        assert_eq!(
            status_of(&read, row.child),
            "DELETING",
            "case {case:?}: expected status DELETING from the read, got {read:?}"
        );
        if row.listed {
            let listed = list_children(&router, row.child, cluster).await;
            assert!(
                lists(&listed, row.child, row.name),
                "case {case:?}: the listing must carry {:?} while it is being deleted, got \
                 {listed:?}",
                row.name
            );
        }
        let again = delete_child(&router, row.child, cluster, row.name).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a repeat of the accepted delete answers the same record, got \
             {again:?}"
        );
        assert_eq!(
            status_of(&again, row.child),
            "DELETING",
            "case {case:?}: the repeat delete reports the same status, got {again:?}"
        );
        let taken = create_child(&router, row.child, cluster, row.name).await;
        assert_eq!(
            taken.status, row.taken_status,
            "case {case:?}: the name is taken while the removal runs, expected HTTP {}, got \
             {taken:?}",
            row.taken_status
        );
        let code = error_code(&taken);
        assert_eq!(
            code, row.taken_code,
            "case {case:?}: expected {:?} for the taken name, got {code:?}",
            row.taken_code
        );

        // ONCE THE CLOCK HAS FINISHED IT: the resource is absent and the name free.
        let gone = child_until_gone(&router, case, row.child, cluster, row.name).await;
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException once the removal finished, \
             got {gone:?}"
        );
        if row.listed {
            let listed = list_children(&router, row.child, cluster).await;
            assert!(
                !lists(&listed, row.child, row.name),
                "case {case:?}: the listing must drop {:?} once the removal finished, got \
                 {listed:?}",
                row.name
            );
        }
        let recreated = create_child(&router, row.child, cluster, row.name).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the name is free once the removal finished, got {recreated:?}"
        );
        assert_eq!(
            status_of(&recreated, row.child),
            "CREATING",
            "case {case:?}: the name created again is a new resource, got {recreated:?}"
        );
    }
}

/// A node group whose removal is still running keeps its cluster: "If you have
/// managed node groups or Fargate profiles attached to the cluster, you must delete
/// them first" (`DeleteCluster`), and the sentence that reports the node-group half
/// names no status — so the cluster delete is refused until the clock has finished
/// the removal, and accepted after it. The profile half names the statuses it is
/// about, so a profile on its way out holds the cluster no longer.
#[tokio::test]
async fn a_node_group_being_removed_still_holds_its_cluster() {
    let router = emulator();

    for (case, child, name, holds) in [
        ("a node group being removed", Child::Nodegroup, "ng-holds", true),
        (
            "a Fargate profile being removed",
            Child::FargateProfile,
            "fp-holds",
            false,
        ),
    ] {
        let cluster = format!("removal-window-{name}");
        create_cluster(&router, &cluster).await;
        let created = create_child(&router, child, &cluster, name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup expected 200 creating {name:?}, got {created:?}"
        );
        child_until_status(&router, case, child, &cluster, name, "ACTIVE").await;
        let deleted = delete_child(&router, child, &cluster, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: setup expected 200 deleting {name:?}, got {deleted:?}"
        );

        let answer = call(
            &router,
            REGION,
            "DELETE",
            &format!("/clusters/{}", encode_label(&cluster)),
            "",
        )
        .await;
        if holds {
            assert_eq!(
                answer.status, 409,
                "case {case:?}: expected HTTP 409 while the removal runs, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceInUseException",
                "case {case:?}: expected ResourceInUseException, got {code:?}"
            );
            child_until_gone(&router, case, child, &cluster, name).await;
            let deleted = call(
                &router,
                REGION,
                "DELETE",
                &format!("/clusters/{}", encode_label(&cluster)),
                "",
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected HTTP 200 once the removal finished, got {deleted:?}"
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 while the removal runs, got {answer:?}"
            );
        }
    }
}
