// SPDX-License-Identifier: CC0-1.0

//! Every `ResourceNotFoundException` this service answers is worded the same way.
//!
//! The rule under test: the error's required `Message` is not free text — it is the
//! `ResourceType` the error carries, the fixed words `" was not found: "`, the `ResourceId`,
//! and a terminating `.` (smithy_query: shape ResourceNotFoundException, whose three members
//! are all required). The two variable parts come from the request and from state, and they
//! are exactly the error's other two members, so the sentence and the fields report the same
//! two facts.
//!
//! What the error discriminates on — `ResourceType`, and so the word the sentence opens with —
//! follows from what the call DOES:
//!
//! * a call addressed AT a resource (every read and delete of this service) could not find the
//!   resource it was asked about, so it reports that resource's KIND, in the two words that
//!   member's own doc states its domain as ("can be either `Connector` or `Challenge`"),
//!   whichever operation answers it and however the ARN came to name nothing;
//! * a CREATE could not resolve a resource its input only REFERENCES — the resource that call
//!   is about does not exist yet — so it reports the NAME of the input member that carried the
//!   reference. `CreateChallenge`'s `ConnectorArn` is such a member, and the reads handed the
//!   very same ARN still report the kind.
//!
//! Every case drives the service the way a client does, one HTTP request per call through the
//! same `MultiServiceRouter` the emulator boots. The rows differ along what the rule
//! generalises over: which operation raises the error, which resource kind the ARN names, HOW
//! the ARN came to name nothing (never created, deleted, owner torn down, an id under a live
//! owner), the partition and Region segments of the ARN, the resource ids, and how many other
//! records are stored when the call arrives. The five recorded read paths are five of the
//! rows.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state;

/// A region that is NOT the emulator default, so nothing here can pass by a hardcoded one.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN of this service carries.
const ACCOUNT: &str = "000000000000";

/// The two values `ResourceType` carries for a resource kind, from the service's own
/// vocabulary rather than respelled here.
const RESOURCE_TYPE_CONNECTOR: &str = state::RESOURCE_TYPE_CONNECTOR;
const RESOURCE_TYPE_CHALLENGE: &str = state::RESOURCE_TYPE_CHALLENGE;

/// The WHOLE domain of `ResourceType` for a call addressed AT one of this service's
/// resources, as that member's own doc states it (smithy_query: doc shape
/// ResourceNotFoundException member ResourceType — "can be either `Connector` or
/// `Challenge`"). No 404 such a call answers may carry anything else.
const RESOURCE_TYPE_DOMAIN: &[&str] = &[RESOURCE_TYPE_CONNECTOR, RESOURCE_TYPE_CHALLENGE];

/// The name of `CreateChallenge`'s input member that references the connector a challenge is
/// minted under (smithy_query: operation CreateChallenge), which is what that create's 404
/// discriminates on and the word its sentence opens with — the create having no resource of
/// its own to be addressed at.
const MEMBER_CONNECTOR_ARN: &str = "ConnectorArn";

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern. Rows name different CAs,
/// so no assertion can pass by echoing one recorded value.
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/deadbeef-0000-1111-2222-333344445555",
];

/// The sentence the rule requires, spelled here independently of the service's own helper so
/// a change to either side is a failure rather than a tautology.
fn expected_message(resource_type: &str, resource_id: &str) -> String {
    format!("{resource_type} was not found: {resource_id}.")
}

/// A connector ARN of exactly the form `ConnectorArn`'s own pattern requires.
fn connector_arn(partition: &str, region: &str, connector_id: &str) -> String {
    format!("arn:{partition}:pca-connector-scep:{region}:{ACCOUNT}:connector/{connector_id}")
}

/// A challenge ARN: its connector's ARN plus the challenge segment `ChallengeArn`'s own
/// pattern requires.
fn challenge_arn(connector_arn: &str, challenge_id: &str) -> String {
    format!("{connector_arn}/challenge/{challenge_id}")
}

/// Which operation a row drives. Every operation of this service that ANSWERS
/// `ResourceNotFoundException` and identifies its resource by ARN is here, plus the two
/// deletes the rows drive to make a record go away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    GetConnector,
    ListChallengeMetadata,
    CreateChallenge,
    DeleteConnector,
    GetChallengeMetadata,
    GetChallengePassword,
    DeleteChallenge,
}

impl Op {
    /// One call of this operation, naming `arn` as the resource it is about.
    async fn call(self, router: &MultiServiceRouter, arn: &str) -> Answer {
        let encoded = label(arn);
        match self {
            Self::GetConnector => {
                harness::call(router, REGION, "GET", &format!("/connectors/{encoded}"), None).await
            }
            Self::ListChallengeMetadata => {
                harness::call(
                    router,
                    REGION,
                    "GET",
                    &format!("/challengeMetadata?ConnectorArn={encoded}"),
                    None,
                )
                .await
            }
            Self::CreateChallenge => {
                harness::call(
                    router,
                    REGION,
                    "POST",
                    "/challenges",
                    Some(json!({ "ConnectorArn": arn })),
                )
                .await
            }
            Self::DeleteConnector => {
                harness::call(
                    router,
                    REGION,
                    "DELETE",
                    &format!("/connectors/{encoded}"),
                    None,
                )
                .await
            }
            Self::GetChallengeMetadata => {
                harness::call(
                    router,
                    REGION,
                    "GET",
                    &format!("/challengeMetadata/{encoded}"),
                    None,
                )
                .await
            }
            Self::GetChallengePassword => {
                harness::call(
                    router,
                    REGION,
                    "GET",
                    &format!("/challengePasswords/{encoded}"),
                    None,
                )
                .await
            }
            Self::DeleteChallenge => {
                harness::call(
                    router,
                    REGION,
                    "DELETE",
                    &format!("/challenges/{encoded}"),
                    None,
                )
                .await
            }
        }
    }
}

/// Every operation a connector ARN can be handed to that ANSWERS this error, with what its
/// 404 reports as `ResourceType`.
///
/// `DeleteConnector` is not among them: its whole output is the modelled 202 "accepted", so
/// an ARN that holds no connector is the end state it accepts rather than this error (see
/// `connector_delete_not_found`). Neither are the three tagging operations: they are served
/// by the resource-tagging layer, which reports an unresolvable ARN with its own error class
/// (see `resource_tags`, `tag_resource` and `untag_resource`).
const CONNECTOR_OPS: &[(Op, &str)] = &[
    (Op::GetConnector, RESOURCE_TYPE_CONNECTOR),
    (Op::ListChallengeMetadata, RESOURCE_TYPE_CONNECTOR),
    // The create is addressed at no connector of its own, so it reports the input member that
    // carried the ARN it could not resolve rather than the kind of resource that ARN names.
    (Op::CreateChallenge, MEMBER_CONNECTOR_ARN),
];

/// Every operation a challenge ARN can be handed to that answers this error, likewise.
const CHALLENGE_OPS: &[(Op, &str)] = &[
    (Op::GetChallengeMetadata, RESOURCE_TYPE_CHALLENGE),
    (Op::GetChallengePassword, RESOURCE_TYPE_CHALLENGE),
];

/// How the ARN under test came to name no record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Missing {
    /// Nothing was ever created under it.
    NeverCreated,
    /// The resource was created and then deleted, and its teardown has come due.
    Deleted,
    /// (Challenges only) the owning connector was deleted, taking the challenge with it.
    OwnerDeleted,
    /// (Challenges only) the owning connector is there and serving; the challenge id names
    /// nothing under it.
    UnknownUnderLiveOwner,
}

/// One row: an ARN that names nothing, and how it got that way.
#[derive(Debug)]
struct Row {
    name: &'static str,
    how: Missing,
    /// The partition in a hand-built ARN. `ConnectorArn`'s pattern allows `aws` and any
    /// `aws-<suffix>`, so a row can vary it.
    partition: &'static str,
    /// The Region segment of a hand-built ARN.
    region: &'static str,
    /// The connector uuid of a hand-built ARN.
    connector_id: &'static str,
    /// The challenge uuid of a hand-built challenge ARN.
    challenge_id: &'static str,
    /// How many OTHER connectors (each with a challenge) are stored when the call arrives, so
    /// no refusal can come from an empty store.
    siblings: usize,
}

const CONNECTOR_ROWS: &[Row] = &[
    Row {
        name: "connector never created, empty store",
        how: Missing::NeverCreated,
        partition: "aws",
        region: REGION,
        connector_id: "fa9ce23c-aedb-47ef-94d6-505cf953adba",
        challenge_id: "",
        siblings: 0,
    },
    Row {
        name: "connector never created, other records stored",
        how: Missing::NeverCreated,
        partition: "aws",
        region: REGION,
        connector_id: "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb",
        challenge_id: "",
        siblings: 2,
    },
    Row {
        name: "connector of another partition and region",
        how: Missing::NeverCreated,
        partition: "aws-us-gov",
        region: "us-gov-west-1",
        connector_id: "0123456f-89ab-cdef-0123-456789abcdef",
        challenge_id: "",
        siblings: 1,
    },
    Row {
        name: "connector deleted and torn down",
        how: Missing::Deleted,
        partition: "aws",
        region: REGION,
        connector_id: "",
        challenge_id: "",
        siblings: 1,
    },
];

const CHALLENGE_ROWS: &[Row] = &[
    Row {
        name: "challenge never created, empty store",
        how: Missing::NeverCreated,
        partition: "aws",
        region: REGION,
        connector_id: "fa9ce23c-aedb-47ef-94d6-505cf953adba",
        challenge_id: "1b7f2991-f611-452a-9050-38557f08ec1d",
        siblings: 0,
    },
    Row {
        name: "challenge never created, another partition and region",
        how: Missing::NeverCreated,
        partition: "aws-us-gov",
        region: "us-gov-west-1",
        connector_id: "0123456f-89ab-cdef-0123-456789abcdef",
        challenge_id: "99999999-8888-7777-6666-555555555555",
        siblings: 2,
    },
    Row {
        name: "challenge id nothing was created under a serving connector",
        how: Missing::UnknownUnderLiveOwner,
        partition: "aws",
        region: REGION,
        connector_id: "",
        challenge_id: "22222222-3333-4444-5555-666666666666",
        siblings: 1,
    },
    Row {
        name: "challenge deleted",
        how: Missing::Deleted,
        partition: "aws",
        region: REGION,
        connector_id: "",
        challenge_id: "",
        siblings: 0,
    },
    Row {
        name: "challenge whose connector was torn down",
        how: Missing::OwnerDeleted,
        partition: "aws",
        region: REGION,
        connector_id: "",
        challenge_id: "",
        siblings: 2,
    },
];

/// Create a connector from `ca_arn`, settle it into `ACTIVE`, and answer its ARN.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    ca_arn: &str,
) -> String {
    let created = harness::call(
        router,
        REGION,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": ca_arn })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.body["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectorArn in {:?}", created.body))
        .to_string();
    harness::release_pending(engine, &arn);
    arn
}

/// Mint one challenge under `connector_arn` and answer its ARN.
async fn new_challenge(router: &MultiServiceRouter, case: &str, connector_arn: &str) -> String {
    let created = Op::CreateChallenge.call(router, connector_arn).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected 202 from CreateChallenge on {connector_arn:?}, got {} / {:?}",
        created.status, created.body
    );
    created.body["Challenge"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Challenge Arn in {:?}", created.body))
        .to_string()
}

/// Fill the store with `count` settled connectors, each holding one challenge, so no refusal
/// below is answered out of an empty store.
async fn seed_siblings(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    count: usize,
) {
    for index in 0..count {
        let arn = active_connector(router, engine, case, CA_ARNS[index % CA_ARNS.len()]).await;
        new_challenge(router, case, &arn).await;
    }
}

/// Delete `arn` and let the clock make its teardown due, so the record is gone.
async fn tear_down_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    arn: &str,
) {
    let accepted = Op::DeleteConnector.call(router, arn).await;
    assert_eq!(
        accepted.status, 202,
        "case {case:?}: expected 202 from DeleteConnector on {arn:?}, got {} / {:?}",
        accepted.status, accepted.body
    );
    harness::release_pending(engine, arn);
}

/// Assert one answer is the 404 the rule requires, with the sentence and the fields agreeing.
fn assert_not_found(answer: &Answer, case: &str, operation: &str, arn: &str, resource_type: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?} on {operation}: expected 404 ResourceNotFoundException for {arn:?}, got \
         {} / {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.body["ResourceId"],
        json!(arn),
        "case {case:?} on {operation}: expected the requested ARN as ResourceId, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["ResourceType"],
        json!(resource_type),
        "case {case:?} on {operation}: expected ResourceType {resource_type:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        expected_message(resource_type, arn),
        "case {case:?} on {operation}: expected AWS's not-found sentence, got {:?}",
        answer.message()
    );
    // The sentence reports the SAME two facts as the fields it is built from, whatever they
    // are: no operation may word one and field the other.
    let fielded = expected_message(
        answer.body["ResourceType"].as_str().unwrap_or_default(),
        answer.body["ResourceId"].as_str().unwrap_or_default(),
    );
    assert_eq!(
        answer.message(),
        fielded,
        "case {case:?} on {operation}: the Message must agree with ResourceType/ResourceId, got \
         {:?}",
        answer.body
    );
}

/// The whole rule on a connector ARN that names nothing, row by operation.
#[tokio::test]
async fn every_operation_words_a_missing_connector_the_aws_way() {
    for row in CONNECTOR_ROWS {
        let case = row.name;
        for (op, resource_type) in CONNECTOR_OPS {
            let (router, engine) = new_router();
            seed_siblings(&router, &engine, case, row.siblings).await;

            let arn = match row.how {
                Missing::NeverCreated => {
                    connector_arn(row.partition, row.region, row.connector_id)
                }
                Missing::Deleted => {
                    let arn =
                        active_connector(&router, &engine, case, CA_ARNS[row.siblings % CA_ARNS.len()])
                            .await;
                    new_challenge(&router, case, &arn).await;
                    tear_down_connector(&router, &engine, case, &arn).await;
                    arn
                }
                other => panic!("case {case:?}: {other:?} is not a connector row"),
            };

            let answer = op.call(&router, &arn).await;
            assert_not_found(&answer, case, &format!("{op:?}"), &arn, resource_type);
        }
    }
}

/// The same rule on a challenge ARN that names nothing.
#[tokio::test]
async fn every_operation_words_a_missing_challenge_the_aws_way() {
    for row in CHALLENGE_ROWS {
        let case = row.name;
        for (op, resource_type) in CHALLENGE_OPS {
            let (router, engine) = new_router();
            seed_siblings(&router, &engine, case, row.siblings).await;

            let arn = match row.how {
                Missing::NeverCreated => challenge_arn(
                    &connector_arn(row.partition, row.region, row.connector_id),
                    row.challenge_id,
                ),
                Missing::UnknownUnderLiveOwner => {
                    let owner =
                        active_connector(&router, &engine, case, CA_ARNS[row.siblings % CA_ARNS.len()])
                            .await;
                    challenge_arn(&owner, row.challenge_id)
                }
                Missing::Deleted => {
                    let owner =
                        active_connector(&router, &engine, case, CA_ARNS[row.siblings % CA_ARNS.len()])
                            .await;
                    let arn = new_challenge(&router, case, &owner).await;
                    let deleted = Op::DeleteChallenge.call(&router, &arn).await;
                    assert_eq!(
                        deleted.status, 202,
                        "case {case:?}: expected 202 from DeleteChallenge, got {} / {:?}",
                        deleted.status, deleted.body
                    );
                    arn
                }
                Missing::OwnerDeleted => {
                    let owner =
                        active_connector(&router, &engine, case, CA_ARNS[row.siblings % CA_ARNS.len()])
                            .await;
                    let arn = new_challenge(&router, case, &owner).await;
                    tear_down_connector(&router, &engine, case, &owner).await;
                    arn
                }
            };

            let answer = op.call(&router, &arn).await;
            assert_not_found(&answer, case, &format!("{op:?}"), &arn, resource_type);
        }
    }
}

/// The rule holds wherever this error is raised, not only for a record that is gone:
/// `GetChallengePassword` answers the same 404 — same sentence, same fields — for a STORED
/// challenge whose connector has been accepted for teardown and so has no password to hand
/// out, while the readers that still report that record answer 200.
#[tokio::test]
async fn a_challenge_whose_connector_is_not_serving_gets_the_same_sentence() {
    // Rows differ in how many challenges the connector holds and which of them is asked for,
    // so the sentence cannot come from there being exactly one.
    for (count, asked) in [(1usize, 0usize), (2, 1), (3, 0), (3, 2)] {
        let case = format!("{count} challenges, asking for #{asked}");
        let (router, engine) = new_router();
        let owner = active_connector(&router, &engine, &case, CA_ARNS[count % CA_ARNS.len()]).await;

        let mut challenges = Vec::new();
        for _ in 0..count {
            challenges.push(new_challenge(&router, &case, &owner).await);
        }
        let arn = challenges[asked].clone();

        let accepted = Op::DeleteConnector.call(&router, &owner).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: expected 202 from DeleteConnector, got {} / {:?}",
            accepted.status, accepted.body
        );
        // Pin the teardown window, so the record is still stored while it is read.
        harness::hold_pending(&engine, &owner);

        let password = Op::GetChallengePassword.call(&router, &arn).await;
        assert_not_found(
            &password,
            &case,
            "GetChallengePassword",
            &arn,
            RESOURCE_TYPE_CHALLENGE,
        );

        let metadata = Op::GetChallengeMetadata.call(&router, &arn).await;
        assert_eq!(
            metadata.status, 200,
            "case {case:?}: the metadata of a stored challenge must still be reported, got {} / \
             {:?}",
            metadata.status, metadata.body
        );
    }
}

// ---------------------------------------------------------------------------
// The `ResourceType` member's domain, over both kinds and every operation.
// ---------------------------------------------------------------------------

/// One hand-built ARN that names nothing, with the kind the ARN's own FORM makes it.
///
/// The segments vary along everything the ARN patterns let a caller vary (smithy_query:
/// shapes ConnectorArn and ChallengeArn): the partition, the Region, the twelve-digit
/// account, the connector uuid and — for a challenge ARN — the challenge uuid. None of them
/// may change the word the 404 carries.
#[derive(Debug, Clone, Copy)]
struct ArnRow {
    name: &'static str,
    partition: &'static str,
    region: &'static str,
    account: &'static str,
    connector_id: &'static str,
    /// Empty makes the row a connector ARN; a uuid appends the challenge segment.
    challenge_id: &'static str,
}

impl ArnRow {
    fn arn(&self) -> String {
        let connector = format!(
            "arn:{}:pca-connector-scep:{}:{}:connector/{}",
            self.partition, self.region, self.account, self.connector_id
        );
        if self.challenge_id.is_empty() {
            connector
        } else {
            format!("{connector}/challenge/{}", self.challenge_id)
        }
    }

    /// The kind the ARN names, decided from its form alone: the challenge segment makes it a
    /// challenge, anything else a connector.
    fn expected_resource_type(&self) -> &'static str {
        if self.challenge_id.is_empty() {
            RESOURCE_TYPE_CONNECTOR
        } else {
            RESOURCE_TYPE_CHALLENGE
        }
    }

    /// The operations such an ARN can be handed to, with the kind word each must answer.
    fn operations(&self) -> &'static [(Op, &'static str)] {
        if self.challenge_id.is_empty() {
            CONNECTOR_OPS
        } else {
            CHALLENGE_OPS
        }
    }
}

/// ARNs no record was ever created under, spanning both resource kinds and every segment the
/// patterns allow to vary.
const DOMAIN_ROWS: &[ArnRow] = &[
    ArnRow {
        name: "connector ARN of this partition and the request's own Region",
        partition: "aws",
        region: REGION,
        account: ACCOUNT,
        connector_id: "7d1f3b52-9a0c-4e6d-8f21-0b4c5d6e7f80",
        challenge_id: "",
    },
    ArnRow {
        name: "connector ARN of the GovCloud partition",
        partition: "aws-us-gov",
        region: "us-gov-east-1",
        account: ACCOUNT,
        connector_id: "0123456f-89ab-cdef-0123-456789abcdef",
        challenge_id: "",
    },
    ArnRow {
        name: "connector ARN of the China partition and another account",
        partition: "aws-cn",
        region: "cn-north-1",
        account: "123456789012",
        connector_id: "deadbeef-0000-1111-2222-333344445555",
        challenge_id: "",
    },
    ArnRow {
        name: "challenge ARN of this partition and the request's own Region",
        partition: "aws",
        region: REGION,
        account: ACCOUNT,
        connector_id: "7d1f3b52-9a0c-4e6d-8f21-0b4c5d6e7f80",
        challenge_id: "1b7f2991-f611-452a-9050-38557f08ec1d",
    },
    ArnRow {
        name: "challenge ARN of the GovCloud partition and another account",
        partition: "aws-us-gov",
        region: "us-gov-west-1",
        account: "210987654321",
        connector_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        challenge_id: "99999999-8888-7777-6666-555555555555",
    },
    ArnRow {
        name: "challenge ARN of the China partition",
        partition: "aws-cn",
        region: "cn-northwest-1",
        account: ACCOUNT,
        connector_id: "11111111-2222-3333-4444-555555555555",
        challenge_id: "22222222-3333-4444-5555-666666666666",
    },
];

/// Whichever operation ADDRESSED AT a resource answers this error, the `ResourceType` it
/// carries is one of the two words the member's documented domain holds, and it is the kind
/// the `ResourceId` ARN names — never the name of the input member that carried the ARN. The
/// create handed that very same ARN reports that member name instead, so the two cases a
/// client branches on stay distinguishable.
#[tokio::test]
async fn resource_type_stays_inside_its_documented_domain() {
    for row in DOMAIN_ROWS {
        let case = row.name;
        let arn = row.arn();
        let expected = row.expected_resource_type();

        for (op, reported_type) in row.operations() {
            let (router, engine) = new_router();
            // A live connector with a live challenge is stored when the call arrives, so no
            // refusal below can come from an empty store.
            let live = active_connector(&router, &engine, case, CA_ARNS[0]).await;
            new_challenge(&router, case, &live).await;

            let answer = op.call(&router, &arn).await;
            let operation = format!("{op:?}");
            assert_not_found(&answer, case, &operation, &arn, reported_type);

            let answered = answer.body["ResourceType"].as_str().unwrap_or_default();
            if *op == Op::CreateChallenge {
                assert_eq!(
                    answered, MEMBER_CONNECTOR_ARN,
                    "case {case:?} on {operation}: expected the input member that carried the \
                     reference, got {answered:?}"
                );
                assert!(
                    !RESOURCE_TYPE_DOMAIN.contains(&answered),
                    "case {case:?} on {operation}: a create must not report a resource kind \
                     ({RESOURCE_TYPE_DOMAIN:?}), got {answered:?}"
                );
            } else {
                assert!(
                    RESOURCE_TYPE_DOMAIN.contains(&answered),
                    "case {case:?} on {operation}: ResourceType must be one of \
                     {RESOURCE_TYPE_DOMAIN:?}, got {answered:?}"
                );
                assert_eq!(
                    answered, expected,
                    "case {case:?} on {operation}: expected the kind the ARN names, got \
                     {answered:?}"
                );
            }
        }
    }
}

/// The create's half of the rule, independent of everything else the request carries and of
/// what the store holds: `CreateChallenge` handed a `ConnectorArn` that resolves to no
/// connector reports that INPUT MEMBER as `ResourceType`, words its sentence with it, and
/// carries the supplied ARN as `ResourceId` — while the reads and the delete handed the very
/// same ARN in the very same store report the connector KIND.
///
/// The rows differ along what the rule generalises over: the other input members the create
/// models (a `ClientToken`, which makes the call retryable, and `Tags`), the partition, Region
/// and id segments of the unresolvable ARN, and how many connectors and challenges are stored
/// when the call arrives. None of them may change the member name the refusal reports.
#[tokio::test]
async fn a_create_reports_the_input_member_its_unresolvable_reference_came_in_on() {
    let rows: &[(&str, &str, &str, usize, serde_json::Value)] = &[
        (
            "the reference alone, empty store",
            "aws",
            "fa9ce23c-aedb-47ef-94d6-505cf953adba",
            0,
            json!({}),
        ),
        (
            "a client token, which makes the call retryable",
            "aws",
            "0123456f-89ab-cdef-0123-456789abcdef",
            1,
            json!({ "ClientToken": "create-challenge-retry-1" }),
        ),
        (
            "tags, with other records stored",
            "aws",
            "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb",
            2,
            json!({ "Tags": { "owner": "platform", "env": "test" } }),
        ),
        (
            "every other input at once, on an ARN of another partition and Region",
            "aws-us-gov",
            "deadbeef-0000-1111-2222-333344445555",
            3,
            json!({
                "ClientToken": "create-challenge-everything-1",
                "Tags": { "owner": "platform" },
            }),
        ),
    ];

    for (case, partition, connector_id, siblings, extra) in rows {
        let (router, engine) = new_router();
        seed_siblings(&router, &engine, case, *siblings).await;

        let region = if *partition == "aws" {
            REGION
        } else {
            "us-gov-west-1"
        };
        let arn = connector_arn(partition, region, connector_id);

        let mut request = json!({ "ConnectorArn": arn });
        for (member, value) in extra.as_object().expect("the row's other input members") {
            request[member.as_str()] = value.clone();
        }
        let refused = harness::call(&router, REGION, "POST", "/challenges", Some(request)).await;
        assert_not_found(
            &refused,
            case,
            "CreateChallenge",
            &arn,
            MEMBER_CONNECTOR_ARN,
        );

        // The same ARN, in the same store, handed to the calls that ARE addressed at that
        // connector: those report the resource kind.
        for op in [Op::GetConnector, Op::ListChallengeMetadata] {
            let answer = op.call(&router, &arn).await;
            assert_not_found(
                &answer,
                case,
                &format!("{op:?}"),
                &arn,
                RESOURCE_TYPE_CONNECTOR,
            );
        }

        // The refused create minted nothing: the store still holds exactly the connectors the
        // seeding gave it.
        let listed = harness::call(&router, REGION, "GET", "/connectors", None).await;
        assert_eq!(
            listed.count("Connectors"),
            *siblings,
            "case {case:?}: the refused create must register nothing, got {:?}",
            listed.body
        );
    }
}


