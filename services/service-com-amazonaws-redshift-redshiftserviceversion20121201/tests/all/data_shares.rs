// SPDX-License-Identifier: CC0-1.0

//! The DATASHARE family, end to end over the awsQuery wire.
//!
//! The RULES these cases assert:
//!
//! * every operation that acts on a datashare resolves the `DataShareArn` against
//!   this account's datashares first, and an ARN that resolves to nothing is the
//!   modelled `InvalidDataShareFault` — a datashare is created on the producer's
//!   DATABASE and Amazon Redshift publishes no API that creates one, so an account
//!   that has created none holds none;
//! * every operation that names a NAMESPACE resolves it against the control plane
//!   that owns it — this one for a provisioned cluster's namespace, Amazon Redshift
//!   Serverless for one of its namespaces — and an ARN that control plane holds no
//!   namespace under is the modelled `InvalidNamespaceFault`, while a peer that
//!   cannot be asked at all establishes nothing and so refuses nothing;
//! * the reads report the account's datashares, which is an EMPTY page for an
//!   account that holds none — not a refusal, because the account exists.
//!
//! The rows differ along what those rules generalise over: which operation, the
//! ARN's spelling and which control plane it belongs to. The harness is shared
//! with [`crate::cluster_lifecycle`], which owns it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// A datashare ARN this account holds no datashare under, in the form Amazon
/// Redshift mints one in.
const ABSENT_DATASHARE: &str =
    "arn:aws:redshift:us-west-2:000000000000:datashare:870e2a92-1aab-9599-6cd2-efe9d3cdd491/sales";

/// A namespace ARN of THIS control plane that no cluster of this account carries.
const ABSENT_NAMESPACE: &str =
    "arn:aws:redshift:us-west-2:000000000000:namespace:11111111-2222-3333-4444-555555555555";

/// A namespace ARN of Amazon Redshift Serverless — a DIFFERENT control plane,
/// which this suite does not mount, so it cannot be asked about it.
const SERVERLESS_NAMESPACE: &str =
    "arn:aws:redshift-serverless:us-west-2:000000000000:namespace/analytics";

/// The members one row of a table sends beside the ARN it names.
type Params<'a> = Vec<(&'a str, &'a str)>;

/// One row of an operation table: the case, the action, and the members beside
/// the ARN every row carries.
type OperationRow<'a> = (&'a str, &'a str, Params<'a>);

/// Assert one refusal: the status and the `<Code>` the model gives the fault.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// An emulator with one serving cluster, and that cluster's own namespace ARN —
/// which is a namespace of this account and so resolves.
async fn harness(cluster: &str) -> (Emulator, String) {
    let emulator = Emulator::new();
    let created = emulator.create_minimal(cluster).await;
    assert_eq!(
        created.status, 200,
        "case seed {cluster}: expected 200, got {} with body {}",
        created.status, created.body
    );
    let namespace = created.text("ClusterNamespaceArn").unwrap_or_default();
    assert!(
        !namespace.is_empty(),
        "case seed {cluster}: expected the cluster's namespace ARN, got body {}",
        created.body
    );
    (emulator, namespace)
}

/// Every operation that acts on a datashare answers the datashare's own modelled
/// fault for an ARN this account holds no datashare under — for a table spanning
/// the operations and the ARN's spelling.
#[tokio::test]
async fn every_datashare_operation_refuses_an_absent_datashare() {
    let (emulator, namespace) = harness("datashare-cluster").await;

    // (case, the action, the members beside the `DataShareArn` every row carries)
    let operations: Vec<OperationRow> = vec![
        ("DescribeDataShares", "DescribeDataShares", Vec::new()),
        (
            "AuthorizeDataShare",
            "AuthorizeDataShare",
            vec![("ConsumerIdentifier", "000000000001")],
        ),
        (
            "DeauthorizeDataShare",
            "DeauthorizeDataShare",
            vec![("ConsumerIdentifier", "000000000001")],
        ),
        (
            "AssociateDataShareConsumer",
            "AssociateDataShareConsumer",
            vec![("AssociateEntireAccount", "true")],
        ),
        (
            "AssociateDataShareConsumer naming a namespace of this account",
            "AssociateDataShareConsumer",
            vec![("ConsumerArn", namespace.as_str())],
        ),
        (
            "DisassociateDataShareConsumer",
            "DisassociateDataShareConsumer",
            vec![("DisassociateEntireAccount", "true")],
        ),
        ("RejectDataShare", "RejectDataShare", Vec::new()),
    ];

    // The ARN's spelling is what the rule generalises over, so each operation is
    // driven with several values that name no datashare of this account.
    let absent_arns: Vec<(&str, &str)> = vec![
        ("a datashare ARN nothing created", ABSENT_DATASHARE),
        (
            "a namespace ARN of this account, which names no datashare",
            namespace.as_str(),
        ),
        ("a value that is not an ARN at all", "sales"),
    ];

    for (arn_case, arn) in &absent_arns {
        for (operation, action, members) in &operations {
            let case = format!("{operation} with {arn_case}");
            let mut params: Vec<(&str, &str)> = vec![("DataShareArn", arn)];
            params.extend(members.iter().copied());
            let answer = emulator.call(action, &params).await;
            assert_refused(&case, &answer, 400, "InvalidDataShareFault");
            assert_eq!(
                answer.text("Message").as_deref(),
                Some(state::invalid_data_share_message(arn).as_str()),
                "case {case}: expected the ARN named in the refusal, got {:?}",
                answer.text("Message")
            );
        }
    }
}

/// An account that has created no datashare has none: the reads report an empty
/// page rather than a refusal, whichever way the request asks.
#[tokio::test]
async fn an_account_with_no_datashare_is_served_an_empty_page() {
    let (emulator, namespace) = harness("empty-datashare-cluster").await;

    // (case, the action, the members the request carries)
    let rows: Vec<OperationRow> = vec![
        ("every datashare of the account", "DescribeDataShares", Vec::new()),
        (
            "the datashares of a producer namespace of this account",
            "DescribeDataSharesForProducer",
            vec![("ProducerArn", namespace.as_str())],
        ),
        (
            "the datashares of a consumer namespace of this account",
            "DescribeDataSharesForConsumer",
            vec![("ConsumerArn", namespace.as_str())],
        ),
        (
            "the datashares of every producer",
            "DescribeDataSharesForProducer",
            Vec::new(),
        ),
        (
            "the datashares of every consumer",
            "DescribeDataSharesForConsumer",
            Vec::new(),
        ),
        (
            "a producer namespace of a control plane this emulator cannot ask",
            "DescribeDataSharesForProducer",
            vec![("ProducerArn", SERVERLESS_NAMESPACE)],
        ),
        (
            "the datashares of a producer in one status",
            "DescribeDataSharesForProducer",
            vec![("ProducerArn", namespace.as_str()), ("Status", "ACTIVE")],
        ),
    ];

    for (case, action, params) in rows {
        let answer = emulator.call(action, &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            all_text(&answer.body, "DataShareArn").is_empty(),
            "case {case}: expected no datashare reported, got body {}",
            answer.body
        );
        // A page that holds everything there is hands out no cursor.
        assert_eq!(
            answer.text("Marker"),
            None,
            "case {case}: expected no Marker on a complete page, got {:?}",
            answer.text("Marker")
        );
    }
}

/// A namespace an operation names has to be one the control plane that owns it
/// holds: this service's own store answers for a provisioned cluster's namespace,
/// and a peer that cannot be asked establishes nothing — so silence refuses
/// nothing.
#[tokio::test]
async fn a_namespace_an_operation_names_is_resolved_where_it_lives() {
    let (emulator, namespace) = harness("namespace-datashare-cluster").await;

    // (case, the action, the member naming the namespace, the ARN, the refusal or
    // `None` when the call is served)
    let rows: Vec<(&str, &str, &str, &str, Option<&str>)> = vec![
        (
            "a producer namespace of this account",
            "DescribeDataSharesForProducer",
            "ProducerArn",
            namespace.as_str(),
            None,
        ),
        (
            "a namespace ARN of this control plane that no cluster carries",
            "DescribeDataSharesForProducer",
            "ProducerArn",
            ABSENT_NAMESPACE,
            Some("InvalidNamespaceFault"),
        ),
        (
            "a consumer namespace ARN of this control plane that no cluster carries",
            "DescribeDataSharesForConsumer",
            "ConsumerArn",
            ABSENT_NAMESPACE,
            Some("InvalidNamespaceFault"),
        ),
        (
            "a value that is not a namespace ARN of either control plane",
            "DescribeDataSharesForConsumer",
            "ConsumerArn",
            "analytics",
            Some("InvalidNamespaceFault"),
        ),
        (
            // Amazon Redshift Serverless is not mounted beside this emulator, so
            // nothing is known about the namespace either way: the read is served
            // rather than refused on the strength of a peer that never answered.
            "a namespace of a control plane this emulator cannot ask",
            "DescribeDataSharesForConsumer",
            "ConsumerArn",
            SERVERLESS_NAMESPACE,
            None,
        ),
    ];

    for (case, action, member, arn, refusal) in rows {
        let answer = emulator.call(action, &[(member, arn)]).await;
        match refusal {
            None => assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with body {}",
                answer.status, answer.body
            ),
            Some(code) => {
                assert_refused(case, &answer, 400, code);
                assert_eq!(
                    answer.text("Message").as_deref(),
                    Some(state::invalid_namespace_message(arn).as_str()),
                    "case {case}: expected the ARN named in the refusal, got {:?}",
                    answer.text("Message")
                );
            }
        }
    }

    // The association pair resolves the NAMESPACE before the datashare, so a
    // request naming a namespace that does not exist is answered about the
    // namespace however the datashare resolves …
    for action in [
        "AssociateDataShareConsumer",
        "DisassociateDataShareConsumer",
    ] {
        let answer = emulator
            .call(
                action,
                &[
                    ("DataShareArn", ABSENT_DATASHARE),
                    ("ConsumerArn", ABSENT_NAMESPACE),
                ],
            )
            .await;
        assert_refused(
            &format!("{action} naming a namespace that does not exist"),
            &answer,
            400,
            "InvalidNamespaceFault",
        );
    }

    // … and a request naming NO consumer at all names nothing to associate with.
    for (action, case) in [
        ("AssociateDataShareConsumer", "an association naming no consumer"),
        (
            "DisassociateDataShareConsumer",
            "a disassociation naming no consumer",
        ),
    ] {
        let answer = emulator
            .call(action, &[("DataShareArn", ABSENT_DATASHARE)])
            .await;
        assert_refused(case, &answer, 400, "InvalidParameterCombination");
    }
}
