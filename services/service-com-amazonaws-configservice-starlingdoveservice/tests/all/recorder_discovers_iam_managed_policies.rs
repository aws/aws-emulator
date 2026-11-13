// SPDX-License-Identifier: CC0-1.0

//! Where Config's record of an IAM CUSTOMER MANAGED POLICY comes from: the policy IAM
//! holds, and the identities IAM reports it is attached to.
//!
//! The rule under test: **while a configuration recorder is recording and its recording
//! group covers `AWS::IAM::Policy` — a GLOBAL type, so only where the group asks for the
//! global resource types or names the type itself — Config records a configuration item
//! for every CUSTOMER MANAGED policy the account holds, identified by the `PolicyId` IAM
//! minted for it, whose `configuration` reports the members IAM's own reads report
//! (`attachmentCount` among them, which tracks the identities the policy is attached to)
//! and whose `relationships` name each such identity with the wording Config reports that
//! attachment by — "Is attached to Role", "Is attached to User", "Is attached to Group" —
//! and whose later states are recorded as FURTHER configuration items, the first one
//! `ResourceDiscovered` and each later one `OK`.** A policy Amazon Web Services owns
//! belongs to no account, so no account's recorder records it.
//!
//! Config and IAM are mounted on ONE engine, reusing the harness
//! `conformance_pack_rule_evaluation` builds: the recorder reads the policies it records
//! from IAM's own API, so without IAM mounted there is nothing to discover. No row calls
//! `PutResourceConfig`: every item read back here got into the inventory because a
//! recorder was recording while the policy existed in IAM.
//!
//! The rows differ along what the rule generalises over: how many identities the policy is
//! attached to (none, one, two), which KIND each is (role, user, group), whether the
//! attachment was made before or after the policy was first recorded, whether it was
//! taken away again, which recording group the recorder records with, whether it is
//! recording at all, and whether the policy is the account's own or one AWS owns.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::conformance_pack_rule_evaluation::{
    Emulator, CONVERGENCE_BUDGET, POLL_INTERVAL, REGION,
};

/// The resource type Config records an IAM customer managed policy under, an entry of
/// `com.amazonaws.configservice#ResourceType`.
const IAM_POLICY_RESOURCE_TYPE: &str = "AWS::IAM::Policy";

/// The entries of that same enum the identities a policy is attached to are recorded
/// under.
const IAM_ROLE_RESOURCE_TYPE: &str = "AWS::IAM::Role";
/// See [`IAM_ROLE_RESOURCE_TYPE`].
const IAM_USER_RESOURCE_TYPE: &str = "AWS::IAM::User";
/// See [`IAM_ROLE_RESOURCE_TYPE`].
const IAM_GROUP_RESOURCE_TYPE: &str = "AWS::IAM::Group";

/// The wording Config reports a managed policy's attachment to each kind of identity by
/// (`Relationship$relationshipName`).
const ATTACHED_TO_ROLE: &str = "Is attached to Role";
/// See [`ATTACHED_TO_ROLE`].
const ATTACHED_TO_USER: &str = "Is attached to User";
/// See [`ATTACHED_TO_ROLE`].
const ATTACHED_TO_GROUP: &str = "Is attached to Group";

/// `ConfigurationItemStatus` values an item of a live resource carries
/// (`com.amazonaws.configservice#ConfigurationItemStatus`): the resource was newly
/// discovered, and its configuration has been updated since.
const RESOURCE_DISCOVERED: &str = "ResourceDiscovered";
/// See [`RESOURCE_DISCOVERED`].
const OK: &str = "OK";

/// The error a history read answers for a resource key Config recorded nothing for
/// (`GetResourceConfigHistory` models it: "you have specified a resource that is either
/// unknown or has not been discovered").
const NOT_DISCOVERED: &str = "ResourceNotDiscoveredException";

/// One policy AWS owns, from the catalog IAM serves every account
/// (`arn:aws:iam::aws:policy/<name>`): the row that pins that an account's recorder
/// records its OWN managed policies and not AWS's.
const AWS_MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/AdministratorAccess";

/// How long a row watches for a record it does not expect — several passes' worth of
/// time, so a record that was going to appear would have appeared.
const QUIET_BUDGET: Duration = Duration::from_millis(2_500);

/// The policy document a created policy grants, as `CreatePolicy` requires one.
fn document(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": "*" }],
    })
    .to_string()
}

/// A percent-encoded policy document, decoded: IAM reports a policy version's document
/// percent-encoded ("the policy document returned in this structure is URL-encoded
/// compliant with RFC 3986", `GetPolicyVersion`), and a recorder records what IAM reports,
/// so a row comparing the recorded document to the one it created decodes it first.
fn percent_decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .to_string()
}

/// The trust document every role below carries, which `CreateRole` requires.
fn trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "ec2.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create a customer managed policy through IAM's own API and answer the `PolicyId` and
/// the ARN IAM minted for it — the two values a configuration item records as the
/// resource's id and its ARN.
async fn create_policy(
    case: &str,
    emulator: &Emulator,
    name: &str,
    action: &str,
) -> (String, String) {
    let created = emulator
        .iam()
        .create_policy()
        .policy_name(name)
        .policy_document(document(action))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: creating the policy {name} must succeed, got {error:?}")
        });
    let policy = created
        .policy
        .unwrap_or_else(|| panic!("case {case:?}: CreatePolicy must answer the policy"));
    (
        policy
            .policy_id
            .unwrap_or_else(|| panic!("case {case:?}: CreatePolicy answers a PolicyId")),
        policy
            .arn
            .unwrap_or_else(|| panic!("case {case:?}: CreatePolicy answers an ARN")),
    )
}

/// Which kind of identity a row attaches the policy to.
#[derive(Debug, Clone, Copy)]
enum Identity {
    /// An IAM role, created and attached by [`attach`].
    Role,
    /// An IAM user.
    User,
    /// An IAM group.
    Group,
}

/// Create the identity `name` of kind `identity` through IAM's own API, attach
/// `policy_arn` to it, and answer the relationship the policy's configuration item owes
/// for that attachment.
async fn attach(
    case: &str,
    emulator: &Emulator,
    identity: Identity,
    name: &str,
    policy_arn: &str,
) -> Value {
    let iam = emulator.iam();
    let (resource_type, relationship_name, resource_id) = match identity {
        Identity::Role => {
            let created = iam
                .create_role()
                .role_name(name)
                .assume_role_policy_document(trust())
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: creating the role {name} must succeed, got {error:?}")
                });
            let role = created
                .role
                .unwrap_or_else(|| panic!("case {case:?}: CreateRole answers the role"));
            iam.attach_role_policy()
                .role_name(name)
                .policy_arn(policy_arn)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: attaching to the role {name} must succeed, got {error:?}")
                });
            (IAM_ROLE_RESOURCE_TYPE, ATTACHED_TO_ROLE, role.role_id)
        }
        Identity::User => {
            let created = iam
                .create_user()
                .user_name(name)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: creating the user {name} must succeed, got {error:?}")
                });
            let user = created
                .user
                .unwrap_or_else(|| panic!("case {case:?}: CreateUser answers the user"));
            iam.attach_user_policy()
                .user_name(name)
                .policy_arn(policy_arn)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: attaching to the user {name} must succeed, got {error:?}")
                });
            (IAM_USER_RESOURCE_TYPE, ATTACHED_TO_USER, user.user_id)
        }
        Identity::Group => {
            let created = iam
                .create_group()
                .group_name(name)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: creating the group {name} must succeed, got {error:?}")
                });
            let group = created
                .group
                .unwrap_or_else(|| panic!("case {case:?}: CreateGroup answers the group"));
            iam.attach_group_policy()
                .group_name(name)
                .policy_arn(policy_arn)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: attaching to the group {name} must succeed, got {error:?}"
                    )
                });
            (IAM_GROUP_RESOURCE_TYPE, ATTACHED_TO_GROUP, group.group_id)
        }
    };
    json!({
        "resourceType": resource_type,
        "resourceId": resource_id,
        "resourceName": name,
        "relationshipName": relationship_name,
    })
}

/// One history read of the policy `resource_id`, as the status and the parsed body.
async fn history(emulator: &Emulator, resource_id: &str) -> (u16, Value) {
    emulator
        .call(
            "GetResourceConfigHistory",
            &json!({ "resourceType": IAM_POLICY_RESOURCE_TYPE, "resourceId": resource_id }),
        )
        .await
}

/// The configuration items a history read reported, newest first — empty for a key Config
/// has recorded nothing for, which that read refuses about rather than answering empty.
async fn recorded_items(case: &str, emulator: &Emulator, resource_id: &str) -> Vec<Value> {
    let (status, body) = history(emulator, resource_id).await;
    if status == 200 {
        return body["configurationItems"]
            .as_array()
            .cloned()
            .unwrap_or_default();
    }
    let code = body["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_default();
    assert_eq!(
        (status, code.as_str()),
        (400, NOT_DISCOVERED),
        "case {case:?}: an unrecorded policy is refused as undiscovered, got {status} with {body}"
    );
    Vec::new()
}

/// The `configuration` document of one configuration item, parsed.
fn configuration(case: &str, item: &Value) -> Value {
    let encoded = item["configuration"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an item reports a configuration, got {item}"));
    serde_json::from_str(encoded).unwrap_or_else(|error| {
        panic!("case {case:?}: the configuration is a JSON document, got {encoded} ({error})")
    })
}

/// The `attachmentCount` one configuration item reports.
fn attachment_count(case: &str, item: &Value) -> i64 {
    configuration(case, item)["attachmentCount"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: the configuration reports attachmentCount"))
}

/// The relationships one configuration item carries, as a comparable set.
fn relationships(item: &Value) -> BTreeSet<String> {
    item["relationships"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(Value::to_string)
        .collect()
}

/// What a row expects of the policy's relationships, as the same comparable set.
fn expected_relationships(expected: &[Value]) -> BTreeSet<String> {
    expected.iter().map(Value::to_string).collect()
}

/// Read the policy's history until its NEWEST item reports `count` attachments, and answer
/// every item that read reported, newest first.
///
/// What is being waited for is the recording pass the recorder owes, never a value: the
/// assertion that fails at the deadline names the count the read did report.
async fn await_attachment_count(
    case: &str,
    emulator: &Emulator,
    resource_id: &str,
    count: i64,
) -> Vec<Value> {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let items = recorded_items(case, emulator, resource_id).await;
        let reported = items.first().map(|item| attachment_count(case, item));
        if reported == Some(count) {
            return items;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a configuration item reporting attachmentCount {count}, \
             got {reported:?} over {} item(s)",
            items.len()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert that `resource_id` is recorded nowhere for as long as [`QUIET_BUDGET`], and that
/// the history read says so with the modelled refusal rather than an empty success.
async fn stays_unrecorded(case: &str, emulator: &Emulator, resource_id: &str) {
    let deadline = Instant::now() + QUIET_BUDGET;
    loop {
        let items = recorded_items(case, emulator, resource_id).await;
        assert!(
            items.is_empty(),
            "case {case:?}: nothing must be recorded for {resource_id}, got {items:?}"
        );
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Put a recorder recording with `group` as its recording group, and the delivery channel
/// a recorder needs, starting it only when `start` says so.
async fn recorder_with_group(emulator: &Emulator, group: &Value, start: bool) {
    let role = emulator.config_role().await;
    emulator
        .ok(
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": "the-recorder",
                    "roleARN": role,
                    "recordingGroup": group,
                }
            }),
        )
        .await;
    emulator
        .ok(
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": "config-delivery" } }),
        )
        .await;
    if start {
        emulator
            .ok(
                "StartConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": "the-recorder" }),
            )
            .await;
    }
}

/// One row of the attachment table: the policy the row creates, and the identities it
/// attaches that policy to, each as the kind of identity it is and the name it is created
/// under.
struct Row {
    /// See [`Row`].
    case: &'static str,
    /// See [`Row`].
    policy_name: &'static str,
    /// See [`Row`].
    attached: &'static [(Identity, &'static str)],
}

/// The policy a row was recorded as, once it and its attachments exist: what the row then
/// asserts the recorded configuration item against.
struct Recorded {
    /// See [`Recorded`].
    case: &'static str,
    /// See [`Recorded`].
    policy_name: &'static str,
    /// See [`Recorded`].
    policy_id: String,
    /// See [`Recorded`].
    policy_arn: String,
    /// The relationships the policy's configuration item owes, one per attachment.
    links: Vec<Value>,
}

#[tokio::test]
async fn a_managed_policy_is_recorded_with_the_identities_it_is_attached_to() {
    let emulator = Emulator::new();
    emulator.recording().await;

    // The rows differ along how many identities the policy is attached to and which kind
    // each is. Every policy and every identity is created through IAM's own API; Config is
    // told nothing about any of them.
    let rows: [Row; 5] = [
        Row {
            case: "a policy attached to nothing",
            policy_name: "cfg-unattached",
            attached: &[],
        },
        Row {
            case: "a policy attached to one role",
            policy_name: "cfg-one-role",
            attached: &[(Identity::Role, "cfg-role-only")],
        },
        Row {
            case: "a policy attached to two roles",
            policy_name: "cfg-two-roles",
            attached: &[
                (Identity::Role, "cfg-role-first"),
                (Identity::Role, "cfg-role-second"),
            ],
        },
        Row {
            case: "a policy attached to a role and a user",
            policy_name: "cfg-role-and-user",
            attached: &[
                (Identity::Role, "cfg-role-mixed"),
                (Identity::User, "cfg-user-mixed"),
            ],
        },
        Row {
            case: "a policy attached to a group",
            policy_name: "cfg-one-group",
            attached: &[(Identity::Group, "cfg-group-only")],
        },
    ];

    let mut expected = Vec::new();
    for row in rows {
        let case = row.case;
        let (policy_id, policy_arn) =
            create_policy(case, &emulator, row.policy_name, "s3:GetObject").await;
        let mut links = Vec::new();
        for (identity, name) in row.attached {
            links.push(attach(case, &emulator, *identity, name, &policy_arn).await);
        }
        expected.push(Recorded {
            case,
            policy_name: row.policy_name,
            policy_id,
            policy_arn,
            links,
        });
    }

    for recorded in expected {
        let Recorded {
            case,
            policy_name,
            policy_id,
            policy_arn,
            links,
        } = recorded;
        let items = await_attachment_count(case, &emulator, &policy_id, links.len() as i64).await;
        let item = items
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the policy is recorded"));

        // The item names the resource as IAM named it: the type it is recorded under, the
        // PolicyId as its id, the policy name as its custom name, and the ARN IAM minted.
        assert_eq!(
            (
                item["resourceType"].as_str(),
                item["resourceId"].as_str(),
                item["resourceName"].as_str(),
                item["arn"].as_str(),
                item["awsRegion"].as_str(),
            ),
            (
                Some(IAM_POLICY_RESOURCE_TYPE),
                Some(policy_id.as_str()),
                Some(policy_name),
                Some(policy_arn.as_str()),
                Some(REGION),
            ),
            "case {case:?}: expected the policy as IAM names it, got {item}"
        );
        let status = item["configurationItemStatus"].as_str().unwrap_or_default();
        assert!(
            [RESOURCE_DISCOVERED, OK].contains(&status),
            "case {case:?}: expected a live item's status, got {status:?} in {item}"
        );

        // The configuration is the policy IAM reports, including the count of the
        // identities it is attached to and the document the version grants.
        let configured = configuration(case, item);
        assert_eq!(
            (
                configured["policyId"].as_str(),
                configured["policyName"].as_str(),
                configured["arn"].as_str(),
                configured["attachmentCount"].as_i64(),
                configured["isAttachable"].as_bool(),
            ),
            (
                Some(policy_id.as_str()),
                Some(policy_name),
                Some(policy_arn.as_str()),
                Some(links.len() as i64),
                Some(true),
            ),
            "case {case:?}: expected the policy IAM reports, got {configured}"
        );
        let default_version = configured["defaultVersionId"].as_str().unwrap_or_default();
        let versions = configured["policyVersionList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            versions.len(),
            1,
            "case {case:?}: a policy created once carries one version, got {configured}"
        );
        assert_eq!(
            (
                versions[0]["versionId"].as_str(),
                versions[0]["isDefaultVersion"].as_bool(),
                versions[0]["document"].as_str().map(percent_decode),
            ),
            (
                Some(default_version),
                Some(true),
                Some(document("s3:GetObject")),
            ),
            "case {case:?}: expected the version IAM reports, got {configured}"
        );

        // The relationships name every identity the policy is attached to, each with the
        // wording Config reports that attachment by — and nothing else.
        assert_eq!(
            relationships(item),
            expected_relationships(&links),
            "case {case:?}: expected the identities the policy is attached to, got {item}"
        );

        // The read is a report: reading again reports the same item.
        let again = recorded_items(case, &emulator, &policy_id).await;
        let digest = |item: &Value| item["configurationItemMD5Hash"].clone();
        assert_eq!(
            again.first().map(digest),
            Some(digest(item)),
            "case {case:?}: a second read must report the same item, got {again:?}"
        );
    }
}

#[tokio::test]
async fn an_attachment_made_after_the_policy_was_recorded_is_a_later_item() {
    let emulator = Emulator::new();
    emulator.recording().await;

    let case = "a policy whose attachments change";
    let (policy_id, policy_arn) =
        create_policy(case, &emulator, "cfg-tracked", "sqs:SendMessage").await;

    // Recorded before anything is attached: the policy's FIRST item, which is its
    // discovery and carries no relationship.
    let discovered = await_attachment_count(case, &emulator, &policy_id, 0).await;
    assert_eq!(
        discovered.len(),
        1,
        "case {case:?}: a policy recorded once has one item, got {discovered:?}"
    );
    assert_eq!(
        discovered[0]["configurationItemStatus"].as_str(),
        Some(RESOURCE_DISCOVERED),
        "case {case:?}: the first item of a resource is its discovery, got {discovered:?}"
    );
    assert!(
        relationships(&discovered[0]).is_empty(),
        "case {case:?}: an unattached policy is related to nothing, got {discovered:?}"
    );

    // The rows walk the attachments the policy GAINS and then LOSES, each a state the
    // policy changed into and so a further configuration item.
    let role = attach(case, &emulator, Identity::Role, "cfg-tracked-role", &policy_arn).await;
    let after_attach = await_attachment_count(case, &emulator, &policy_id, 1).await;
    assert_eq!(
        relationships(&after_attach[0]),
        expected_relationships(std::slice::from_ref(&role)),
        "case {case:?}: the later item names the role the policy is attached to, got \
         {after_attach:?}"
    );
    assert_eq!(
        after_attach[0]["configurationItemStatus"].as_str(),
        Some(OK),
        "case {case:?}: an item recording a changed configuration is OK, got {after_attach:?}"
    );
    assert!(
        after_attach.len() >= 2,
        "case {case:?}: the history keeps the state the policy left, got {after_attach:?}"
    );

    // The state the policy LEFT is still reported, with the count and the relationships it
    // was recorded with: the history is the states the policy went through.
    let earlier = &after_attach[after_attach.len() - 1];
    assert_eq!(
        (
            attachment_count(case, earlier),
            earlier["configurationItemStatus"].as_str(),
        ),
        (0, Some(RESOURCE_DISCOVERED)),
        "case {case:?}: the earliest item is the one before the attachment, got {earlier}"
    );
    assert!(
        relationships(earlier).is_empty(),
        "case {case:?}: the earliest item carries no relationship, got {earlier}"
    );

    // An attachment taken away is a change too: the count follows the attachments down.
    emulator
        .iam()
        .detach_role_policy()
        .role_name("cfg-tracked-role")
        .policy_arn(&policy_arn)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: detaching must succeed, got {error:?}"));
    let after_detach = await_attachment_count(case, &emulator, &policy_id, 0).await;
    assert!(
        relationships(&after_detach[0]).is_empty(),
        "case {case:?}: a detached policy is related to nothing, got {after_detach:?}"
    );
    assert!(
        after_detach.len() > after_attach.len(),
        "case {case:?}: every state the policy went through is one item, got {after_detach:?}"
    );
}

#[tokio::test]
async fn the_recording_group_decides_whether_a_managed_policy_is_recorded() {
    // The rows differ along which recording group the recorder records with — the global
    // resource types asked for, not asked for, the type enumerated, the type excluded —
    // and along whether the recorder is recording at all.
    let cases: [(&str, Value, bool, bool); 5] = [
        (
            "all supported types, with the global ones asked for",
            json!({ "allSupported": true, "includeGlobalResourceTypes": true }),
            true,
            true,
        ),
        (
            "all supported types, with the global ones not asked for",
            json!({ "allSupported": true }),
            true,
            false,
        ),
        (
            "an inclusion group naming this type",
            json!({ "resourceTypes": [IAM_POLICY_RESOURCE_TYPE] }),
            true,
            true,
        ),
        (
            "an exclusion group excluding this type",
            json!({
                "exclusionByResourceTypes": { "resourceTypes": [IAM_POLICY_RESOURCE_TYPE] },
            }),
            true,
            false,
        ),
        (
            "a recorder that was never started",
            json!({ "allSupported": true, "includeGlobalResourceTypes": true }),
            false,
            false,
        ),
    ];

    for (case, group, recording, recorded) in cases {
        let emulator = Emulator::new();
        recorder_with_group(&emulator, &group, recording).await;
        let (policy_id, _) = create_policy(case, &emulator, "cfg-grouped", "s3:ListBucket").await;

        if recorded {
            await_attachment_count(case, &emulator, &policy_id, 0).await;
        } else {
            stays_unrecorded(case, &emulator, &policy_id).await;
        }
    }
}

#[tokio::test]
async fn a_policy_amazon_web_services_owns_is_not_recorded() {
    let emulator = Emulator::new();
    emulator.recording().await;

    let case = "a policy AWS owns";
    // The id IAM publishes for the AWS managed policy, read from IAM rather than written
    // here: it is the key a recorder would have recorded it under.
    let aws_owned = emulator
        .iam()
        .get_policy()
        .policy_arn(AWS_MANAGED_POLICY_ARN)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: reading {AWS_MANAGED_POLICY_ARN} must succeed, got {error:?}")
        })
        .policy
        .and_then(|policy| policy.policy_id)
        .unwrap_or_else(|| panic!("case {case:?}: GetPolicy answers a PolicyId"));

    // The account's OWN policy is recorded, which is what shows the pass ran at all.
    let (policy_id, _) = create_policy(case, &emulator, "cfg-own-policy", "s3:GetObject").await;
    await_attachment_count(case, &emulator, &policy_id, 0).await;

    stays_unrecorded(case, &emulator, &aws_owned).await;
}
