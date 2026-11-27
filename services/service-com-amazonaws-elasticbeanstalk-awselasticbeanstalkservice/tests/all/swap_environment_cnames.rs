// SPDX-License-Identifier: CC0-1.0

//! `SwapEnvironmentCNAMEs`, driven end to end over the service's awsQuery HTTP
//! surface with the shared harness in `crate::harness`.
//!
//! The rule under test: the operation exchanges the `CNAME` of the two stored
//! environments the request selects — either side selected by `EnvironmentId`,
//! by `EnvironmentName` or by both — and changes nothing else; it refuses with
//! `MissingRequiredParameter` when a side is unnamed or the two sides are named
//! by different members, and with `InvalidParameterValue` when a selector
//! matches no stored environment or matches one that is not `Ready`.
//!
//! Every assertion goes through `DescribeEnvironments`, so a swap that answers
//! 200 without writing the store fails here.

use crate::harness::{Answer, Emulator, PAST_DUE, SOLUTION_STACK};

/// The label every CNAME this service derives carries behind the Region — an
/// environment's CNAME is the DNS name its prefix forms, whose first label is the
/// prefix, whose second is the Region and whose third names this service, so the
/// marker below is that label and the prefix is read off the value rather than
/// repeated here.
const CNAME_SERVICE_LABEL: &str = ".elasticbeanstalk.";

/// The awsQuery error envelope's `<Message>`, or `""` on an answer that carries
/// none. Read through the shared harness's element reader, so this suite adds no
/// second XML reader of its own.
fn error_message(answer: &Answer) -> &str {
    answer.value("Error/Message").unwrap_or_default()
}

/// The application every environment in this suite belongs to.
const APPLICATION: &str = "swap-app";

/// Create [`APPLICATION`], failing loudly if the emulator refuses.
async fn create_application(emulator: &Emulator) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
}

/// Create an environment and answer its `EnvironmentId`.
async fn create_environment(
    emulator: &Emulator,
    environment_name: &str,
    cname_prefix: Option<&str>,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("ApplicationName", APPLICATION),
        ("EnvironmentName", environment_name),
        ("SolutionStackName", SOLUTION_STACK.as_str()),
    ];
    if let Some(prefix) = cname_prefix {
        params.push(("CNAMEPrefix", prefix));
    }
    let created = emulator.call("CreateEnvironment", &params).await;
    assert_eq!(
        created.status, 200,
        "case {environment_name:?}: expected 200 from CreateEnvironment, got {} and body {}",
        created.status, created.body
    );
    created
        .expect("EnvironmentId", environment_name)
        .to_string()
}

/// The stored `CNAME` of `environment_id`, read back through
/// `DescribeEnvironments` — never from the swap's own answer.
async fn stored_cname(emulator: &Emulator, environment_id: &str, case: &str) -> String {
    reported_cname(emulator, environment_id)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: expected a CNAME on environment {environment_id}"))
}

/// The `CNAME` `DescribeEnvironments` reports for `environment_id`, or `None`
/// when it publishes none — which an environment whose launch is still
/// provisioning does.
async fn reported_cname(emulator: &Emulator, environment_id: &str) -> Option<String> {
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentIds.member.1", environment_id)],
        )
        .await;
    described
        .value("Environments/member/CNAME")
        .map(str::to_string)
}

/// Block until a status change the clock owed is due and materialised.
fn wait_for_ready() {
    std::thread::sleep(PAST_DUE);
}

/// How a request names the two environments. The rule holds for every way the
/// model allows a side to be selected, so the selection is the table's axis.
#[derive(Debug, Clone, Copy)]
enum Selection {
    /// `SourceEnvironmentName` + `DestinationEnvironmentName`.
    Name,
    /// `SourceEnvironmentId` + `DestinationEnvironmentId`.
    Id,
    /// Both members on both sides ("You may also specify both").
    Both,
    /// Both members on the source and only `DestinationEnvironmentId` on the
    /// destination: the sides still share the id, which is what the condition
    /// ("specify the SourceEnvironmentId with the DestinationEnvironmentId")
    /// asks for.
    SourceBothDestinationId,
}

/// One row of the swap table: how the pair is selected, and how each of the two
/// environments got the CNAME it starts with.
#[derive(Debug, Clone, Copy)]
struct SwapCase {
    selection: Selection,
    source_name: &'static str,
    source_prefix: Option<&'static str>,
    destination_name: &'static str,
    destination_prefix: Option<&'static str>,
}

impl SwapCase {
    fn case(&self) -> String {
        format!("{:?}/{}", self.selection, self.source_name)
    }

    /// The `SwapEnvironmentCNAMEs` parameters for this row.
    fn params<'a>(&self, source_id: &'a str, destination_id: &'a str) -> Vec<(&'a str, &'a str)> {
        let by_name = [
            ("SourceEnvironmentName", self.source_name),
            ("DestinationEnvironmentName", self.destination_name),
        ];
        let by_id = [
            ("SourceEnvironmentId", source_id),
            ("DestinationEnvironmentId", destination_id),
        ];
        match self.selection {
            Selection::Name => by_name.to_vec(),
            Selection::Id => by_id.to_vec(),
            Selection::Both => by_id.into_iter().chain(by_name).collect(),
            Selection::SourceBothDestinationId => {
                vec![by_id[0], by_name[0], by_id[1]]
            }
        }
    }
}

#[tokio::test]
async fn the_two_selected_environments_exchange_their_cnames() {
    // Rows differ along the axis the rule generalises over: which members select
    // each side, and whether each environment's CNAME came from an explicit
    // prefix or from the generated one (so the two values differ in form and in
    // length, not merely in spelling).
    let cases = [
        SwapCase {
            selection: Selection::Name,
            source_name: "blue-by-name",
            source_prefix: Some("blue-live"),
            destination_name: "green-by-name",
            destination_prefix: Some("green-staging-longer-prefix"),
        },
        SwapCase {
            selection: Selection::Id,
            source_name: "blue-by-id",
            source_prefix: None,
            destination_name: "green-by-id",
            // 4 characters: `CNAMEPrefix`'s published minimum length.
            destination_prefix: Some("gr2x"),
        },
        SwapCase {
            selection: Selection::Both,
            source_name: "blue-by-both",
            source_prefix: Some("b3zq"),
            destination_name: "green-by-both",
            destination_prefix: None,
        },
        SwapCase {
            selection: Selection::SourceBothDestinationId,
            source_name: "blue-mixed",
            source_prefix: None,
            destination_name: "green-mixed",
            destination_prefix: Some("green-mixed-live"),
        },
    ];

    let emulator = Emulator::new();
    create_application(&emulator).await;

    for row in &cases {
        let case = row.case();
        let source_id = create_environment(&emulator, row.source_name, row.source_prefix).await;
        let destination_id =
            create_environment(&emulator, row.destination_name, row.destination_prefix).await;
        // "Ready: Environment is available to have an action performed on it."
        wait_for_ready();

        let source_before = stored_cname(&emulator, &source_id, &case).await;
        let destination_before = stored_cname(&emulator, &destination_id, &case).await;
        // Both CNAMEs are this service's own published form, so the swap is
        // asserted against values the emulator derived rather than values typed
        // here.
        for (label, cname, prefix) in [
            ("source", &source_before, row.source_prefix),
            ("destination", &destination_before, row.destination_prefix),
        ] {
            assert!(
                cname.contains(CNAME_SERVICE_LABEL),
                "case {case:?}: the {label} CNAME must be a name this emulator announces for \
                 this service, got {cname:?}"
            );
            if let Some(prefix) = prefix {
                assert!(
                    cname.starts_with(&format!("{prefix}.")),
                    "case {case:?}: the {label} CNAME must name its prefix {prefix:?}, got {cname:?}"
                );
            }
        }
        assert_ne!(
            source_before, destination_before,
            "case {case:?}: the two environments must start with different CNAMEs"
        );

        let swapped = emulator
            .call(
                "SwapEnvironmentCNAMEs",
                &row.params(&source_id, &destination_id),
            )
            .await;
        assert_eq!(
            swapped.status, 200,
            "case {case:?}: expected 200 from SwapEnvironmentCNAMEs, got {} and body {}",
            swapped.status, swapped.body
        );
        assert_eq!(
            swapped.error_code(),
            "",
            "case {case:?}: expected no error envelope, got {}",
            swapped.body
        );

        let source_after = stored_cname(&emulator, &source_id, &case).await;
        let destination_after = stored_cname(&emulator, &destination_id, &case).await;
        assert_eq!(
            source_after, destination_before,
            "case {case:?}: the source must take the destination's CNAME, got {source_after:?}"
        );
        assert_eq!(
            destination_after, source_before,
            "case {case:?}: the destination must take the source's CNAME, got {destination_after:?}"
        );

        // Nothing else about either environment moved: the swap exchanges the
        // CNAME, not the identity or the application of an environment.
        let described = emulator
            .call(
                "DescribeEnvironments",
                &[("EnvironmentIds.member.1", source_id.as_str())],
            )
            .await;
        assert_eq!(
            described.values_at("Environments/member/EnvironmentName"),
            vec![row.source_name],
            "case {case:?}: the source environment must keep its name, got {}",
            described.body
        );
        assert_eq!(
            described.values_at("Environments/member/Status"),
            vec!["Ready"],
            "case {case:?}: the swap must not move the status, got {}",
            described.body
        );

        // The swap is its own inverse: applying it again restores both records.
        let back = emulator
            .call(
                "SwapEnvironmentCNAMEs",
                &row.params(&source_id, &destination_id),
            )
            .await;
        assert_eq!(
            back.status, 200,
            "case {case:?}: expected 200 from the second swap, got {} and body {}",
            back.status, back.body
        );
        assert_eq!(
            stored_cname(&emulator, &source_id, &case).await,
            source_before,
            "case {case:?}: swapping twice must restore the source's CNAME"
        );
        assert_eq!(
            stored_cname(&emulator, &destination_id, &case).await,
            destination_before,
            "case {case:?}: swapping twice must restore the destination's CNAME"
        );
    }
}

#[tokio::test]
async fn naming_one_environment_on_both_sides_leaves_its_cname_alone() {
    let emulator = Emulator::new();
    create_application(&emulator).await;
    let environment_id = create_environment(&emulator, "self-swap", Some("self-swap-live")).await;
    wait_for_ready();

    let case = "self-swap";
    let before = stored_cname(&emulator, &environment_id, case).await;
    let swapped = emulator
        .call(
            "SwapEnvironmentCNAMEs",
            &[
                ("SourceEnvironmentId", environment_id.as_str()),
                ("DestinationEnvironmentId", environment_id.as_str()),
            ],
        )
        .await;
    assert_eq!(
        swapped.status, 200,
        "case {case:?}: expected 200 from SwapEnvironmentCNAMEs, got {} and body {}",
        swapped.status, swapped.body
    );
    assert_eq!(
        stored_cname(&emulator, &environment_id, case).await,
        before,
        "case {case:?}: exchanging a CNAME with itself must leave it as it was"
    );
}

#[tokio::test]
async fn an_id_and_a_name_that_disagree_select_the_environment_the_id_names() {
    // The store resolves an environment by `EnvironmentId` whenever one is
    // supplied, so a side carrying both members swaps what its id names and the
    // environment its name would have selected is left alone.
    let emulator = Emulator::new();
    create_application(&emulator).await;
    let by_id = create_environment(&emulator, "named-by-id", Some("named-by-id")).await;
    let by_name = create_environment(&emulator, "named-by-name", Some("named-by-name")).await;
    let partner = create_environment(&emulator, "swap-partner", Some("swap-partner")).await;
    wait_for_ready();

    let case = "id wins over name";
    let by_id_before = stored_cname(&emulator, &by_id, case).await;
    let by_name_before = stored_cname(&emulator, &by_name, case).await;
    let partner_before = stored_cname(&emulator, &partner, case).await;

    let swapped = emulator
        .call(
            "SwapEnvironmentCNAMEs",
            &[
                ("SourceEnvironmentId", by_id.as_str()),
                // Disagrees with the id above.
                ("SourceEnvironmentName", "named-by-name"),
                ("DestinationEnvironmentId", partner.as_str()),
                ("DestinationEnvironmentName", "swap-partner"),
            ],
        )
        .await;
    assert_eq!(
        swapped.status, 200,
        "case {case:?}: expected 200 from SwapEnvironmentCNAMEs, got {} and body {}",
        swapped.status, swapped.body
    );
    assert_eq!(
        stored_cname(&emulator, &by_id, case).await,
        partner_before,
        "case {case:?}: the environment the id names must take the partner's CNAME"
    );
    assert_eq!(
        stored_cname(&emulator, &partner, case).await,
        by_id_before,
        "case {case:?}: the partner must take the CNAME of the environment the id names"
    );
    assert_eq!(
        stored_cname(&emulator, &by_name, case).await,
        by_name_before,
        "case {case:?}: the environment only the name named must be untouched"
    );
}

/// One refusal row: the parameters sent, and the awsQuery error code and message
/// fragment the service answers with.
struct RefusalCase {
    case: &'static str,
    params: Vec<(&'static str, &'static str)>,
    code: &'static str,
    /// Fragments the message must carry — for an incomplete request the member
    /// the caller has to fix, and for a value that resolves to nothing the
    /// service's not-found template (which names the side and the kind of
    /// identifier) plus the value itself. Kept free of punctuation the XML
    /// envelope escapes.
    message_contains: &'static [&'static str],
}

#[tokio::test]
async fn unresolvable_and_incomplete_swaps_are_refused_and_change_nothing() {
    let emulator = Emulator::new();
    create_application(&emulator).await;
    let live_id = create_environment(&emulator, "live-env", Some("live-prefix")).await;
    let other_id = create_environment(&emulator, "other-env", Some("other-prefix")).await;
    wait_for_ready();
    let live_before = stored_cname(&emulator, &live_id, "refusals").await;
    let other_before = stored_cname(&emulator, &other_id, "refusals").await;

    // Rows differ along WHICH part of the selection is wrong: an absent side, a
    // side named by a member the other side does not use, and a member whose
    // value matches no stored environment (by name and by id, on either side).
    // An `EnvironmentId` the service never issued and an `EnvironmentName` no
    // environment carries are both resolvable-to-nothing, which is the class.
    let cases = [
        RefusalCase {
            case: "no parameters at all",
            params: vec![],
            code: "MissingRequiredParameter",
            // "You must specify at least the SourceEnvironmentID or the
            // SourceEnvironmentName."
            message_contains: &["SourceEnvironment"],
        },
        RefusalCase {
            case: "no destination",
            params: vec![("SourceEnvironmentName", "live-env")],
            code: "MissingRequiredParameter",
            message_contains: &["DestinationEnvironment"],
        },
        RefusalCase {
            case: "no source",
            params: vec![("DestinationEnvironmentName", "other-env")],
            code: "MissingRequiredParameter",
            message_contains: &["SourceEnvironment"],
        },
        RefusalCase {
            case: "source by name, destination by id",
            params: vec![
                ("SourceEnvironmentName", "live-env"),
                // Never resolved: the two sides share no member, which is
                // refused before any lookup happens.
                ("DestinationEnvironmentId", "e-00000001"),
            ],
            // "You must specify the SourceEnvironmentId with the
            // DestinationEnvironmentId."
            code: "MissingRequiredParameter",
            message_contains: &["SourceEnvironmentId", "DestinationEnvironmentId"],
        },
        RefusalCase {
            case: "source by id, destination by name",
            params: vec![
                ("SourceEnvironmentId", "e-00000001"),
                ("DestinationEnvironmentName", "other-env"),
            ],
            // "If you specify the SourceEnvironmentId, you must specify the
            // DestinationEnvironmentId."
            code: "MissingRequiredParameter",
            message_contains: &["DestinationEnvironmentId", "SourceEnvironmentName"],
        },
        RefusalCase {
            case: "unknown source name",
            params: vec![
                ("SourceEnvironmentName", "no-such-env"),
                ("DestinationEnvironmentName", "other-env"),
            ],
            code: "InvalidParameterValue",
            message_contains: &["source environment not found with Name", "no-such-env"],
        },
        RefusalCase {
            case: "unknown destination name",
            params: vec![
                ("SourceEnvironmentName", "live-env"),
                ("DestinationEnvironmentName", "no-such-env"),
            ],
            code: "InvalidParameterValue",
            message_contains: &[
                "destination environment not found with Name",
                "no-such-env",
            ],
        },
        RefusalCase {
            case: "both names unknown",
            params: vec![
                ("SourceEnvironmentName", "ghost-one"),
                ("DestinationEnvironmentName", "ghost-two"),
            ],
            code: "InvalidParameterValue",
            message_contains: &["source environment not found with Name", "ghost-one"],
        },
        RefusalCase {
            case: "unknown ids",
            params: vec![
                ("SourceEnvironmentId", "e-ffffffff"),
                ("DestinationEnvironmentId", "e-eeeeeeee"),
            ],
            code: "InvalidParameterValue",
            message_contains: &["source environment not found with Id", "e-ffffffff"],
        },
    ];

    for row in &cases {
        let case = row.case;
        let refused = emulator
            .call("SwapEnvironmentCNAMEs", &row.params)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            row.code,
            "case {case:?}: unexpected error code in {}",
            refused.body
        );
        let message = error_message(&refused);
        for fragment in row.message_contains {
            assert!(
                message.contains(fragment),
                "case {case:?}: expected a message naming {fragment:?}, got {message:?}"
            );
        }
    }

    // A refused swap wrote nothing: both CNAMEs are still the ones the create
    // derived.
    assert_eq!(
        stored_cname(&emulator, &live_id, "refusals").await,
        live_before,
        "a refused swap must leave the source CNAME alone"
    );
    assert_eq!(
        stored_cname(&emulator, &other_id, "refusals").await,
        other_before,
        "a refused swap must leave the destination CNAME alone"
    );
}

#[tokio::test]
async fn an_environment_that_is_not_ready_cannot_be_swapped() {
    let emulator = Emulator::new();
    create_application(&emulator).await;
    let settled_id = create_environment(&emulator, "settled-env", Some("settled")).await;
    wait_for_ready();
    let launching_id = create_environment(&emulator, "launching-env", Some("launching")).await;

    let settled_before = stored_cname(&emulator, &settled_id, "launching").await;
    // The still-launching environment has been assigned no name yet, so it
    // publishes none: what it RESERVED becomes its CNAME when the launch
    // finishes, which is read below once it has.
    assert!(
        reported_cname(&emulator, &launching_id).await.is_none(),
        "case \"launching\": an environment still launching publishes no CNAME"
    );

    // Each row names the side the still-launching environment is on, so the
    // refusal does not depend on which side carries it.
    for (case, source, destination) in [
        ("launching source", &launching_id, &settled_id),
        ("launching destination", &settled_id, &launching_id),
    ] {
        let refused = emulator
            .call(
                "SwapEnvironmentCNAMEs",
                &[
                    ("SourceEnvironmentId", source.as_str()),
                    ("DestinationEnvironmentId", destination.as_str()),
                ],
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: unexpected error code in {}",
            refused.body
        );
        assert!(
            error_message(&refused).contains("launching-env"),
            "case {case:?}: the message must name the environment that is not ready, got {:?}",
            error_message(&refused)
        );
    }

    // Once the clock has made the launch due, the environment is assigned the
    // name it reserved and the same request succeeds.
    wait_for_ready();
    let launching_before = stored_cname(&emulator, &launching_id, "ready").await;
    let swapped = emulator
        .call(
            "SwapEnvironmentCNAMEs",
            &[
                ("SourceEnvironmentId", launching_id.as_str()),
                ("DestinationEnvironmentId", settled_id.as_str()),
            ],
        )
        .await;
    assert_eq!(
        swapped.status, 200,
        "case \"ready\": expected 200 once both environments are Ready, got {} and body {}",
        swapped.status, swapped.body
    );
    assert_eq!(
        stored_cname(&emulator, &launching_id, "ready").await,
        settled_before,
        "case \"ready\": the swap must exchange the CNAMEs once both are Ready"
    );
    assert_eq!(
        stored_cname(&emulator, &settled_id, "ready").await,
        launching_before,
        "case \"ready\": the swap must exchange the CNAMEs once both are Ready"
    );

    // A terminating environment is no more swappable than a launching one.
    let terminated = emulator
        .call(
            "TerminateEnvironment",
            &[("EnvironmentId", settled_id.as_str())],
        )
        .await;
    assert_eq!(
        terminated.status, 200,
        "case \"terminating\": expected 200 from TerminateEnvironment, got {} and body {}",
        terminated.status, terminated.body
    );
    let refused = emulator
        .call(
            "SwapEnvironmentCNAMEs",
            &[
                ("SourceEnvironmentId", launching_id.as_str()),
                ("DestinationEnvironmentId", settled_id.as_str()),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"terminating\": expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case \"terminating\": a terminating environment must not be swapped, got {}",
        refused.body
    );
    assert_eq!(
        stored_cname(&emulator, &launching_id, "terminating").await,
        settled_before,
        "case \"terminating\": the refused swap must change nothing"
    );
}
