// SPDX-License-Identifier: CC0-1.0

//! A SEQUENCE of attaches onto ONE configuration set, where the Kinesis Data Firehose
//! destinations they establish all name the SAME delivery stream and differ only in the role
//! they send the set's events under — end to end over HTTP with AWS Identity and Access
//! Management mounted beside this service on one engine.
//!
//! The RULE these cases pin: whether an operation that establishes an event destination may
//! establish it is decided, for each request on its own, from the trust policy the account's
//! IAM holds for the role that destination names
//! (`KinesisFirehoseDestination$IamRoleArn`, "the ARN of the IAM role that the Amazon SES API
//! v2 uses to send email events to the Amazon Kinesis Data Firehose stream"). So the decision
//! depends on nothing else: not on how many times the same request was already made, not on
//! what else the configuration set holds, and not on the stream the destination names — a
//! role that withholds the assume is refused on EVERY attempt with the operation's modelled
//! input error (`BadRequestException`, HTTP 400) carrying SES's own could-not-assume-role
//! sentence and storing nothing, while a role that grants it is accepted on every attempt,
//! and the set then reports exactly the destinations that were accepted.
//!
//! The sibling suite [`crate::configuration_set_firehose_destination_role`] spans what one
//! trust document may say; these rows instead span the SEQUENCE the rule has to be
//! independent of: how many times a refused attach is repeated (twice and three times),
//! whether the repetitions reuse one destination name or each take a fresh one, where the
//! refused attaches sit relative to the accepted ones (after, before, between), a set every
//! attach of which is refused, and a set none of whose attaches is refused — the last row
//! being what makes a suite that refused everything fail.

use serde_json::{json, Value};

use crate::configuration_set_firehose_destination_role::{
    allow_assume, firehose_definition, given_role, refusal, reported_role, trust,
    FirehoseEventDestinations, SES_SERVICE_PRINCIPAL,
};
use crate::configuration_sets::Ses;

/// The role name whose trust policy grants this service the assume, as each row creates it.
const TRUSTED_ROLE: &str = "ses-event-publishing";

/// The role name whose trust policy grants the assume to ANOTHER service only, so this
/// service could never send a set's events under it.
const WITHHELD_ROLE: &str = "lambda-only";

/// The service the withheld role trusts instead: any principal other than this service's own
/// leaves the assume withheld from this service, and a function's execution role is the
/// ordinary way a role comes to trust one.
const OTHER_SERVICE_PRINCIPAL: &str = "lambda.amazonaws.com";

/// Which role one attach's destination names.
#[derive(Clone, Copy, Debug)]
enum Role {
    /// The role whose trust policy grants this service the assume.
    Trusted,
    /// The role whose trust policy grants it to another service only.
    Withheld,
}

/// One attach in a row's sequence: the destination name it attaches under, and the role its
/// destination sends the set's events under.
#[derive(Clone, Copy, Debug)]
struct Attach {
    name: &'static str,
    role: Role,
}

/// One row: the sequence of attaches onto one configuration set, and the destination names
/// the set must report once the whole sequence has been made.
struct Row {
    case: &'static str,
    /// The configuration set this row attaches onto, which is its own.
    set: &'static str,
    attaches: &'static [Attach],
    /// The names the set must report afterwards, ascending.
    reported: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "the withheld role attached twice under one name, after the accepted destination",
        set: "repeated-after",
        attaches: &[
            Attach {
                name: "right-trust",
                role: Role::Trusted,
            },
            Attach {
                name: "wrong-trust",
                role: Role::Withheld,
            },
            Attach {
                name: "wrong-trust",
                role: Role::Withheld,
            },
        ],
        reported: &["right-trust"],
    },
    Row {
        case: "the withheld role attached three times under a fresh name each time, before the \
               accepted destination",
        set: "repeated-before",
        attaches: &[
            Attach {
                name: "first-try",
                role: Role::Withheld,
            },
            Attach {
                name: "second-try",
                role: Role::Withheld,
            },
            Attach {
                name: "third-try",
                role: Role::Withheld,
            },
            Attach {
                name: "accepted",
                role: Role::Trusted,
            },
        ],
        reported: &["accepted"],
    },
    Row {
        case: "the withheld role attached between two accepted destinations",
        set: "refused-between",
        attaches: &[
            Attach {
                name: "events-one",
                role: Role::Trusted,
            },
            Attach {
                name: "events-refused",
                role: Role::Withheld,
            },
            Attach {
                name: "events-two",
                role: Role::Trusted,
            },
        ],
        reported: &["events-one", "events-two"],
    },
    Row {
        case: "a set every attach of which names the withheld role",
        set: "all-refused",
        attaches: &[
            Attach {
                name: "one",
                role: Role::Withheld,
            },
            Attach {
                name: "two",
                role: Role::Withheld,
            },
        ],
        reported: &[],
    },
    Row {
        case: "a set no attach of which names the withheld role",
        set: "none-refused",
        attaches: &[
            Attach {
                name: "one",
                role: Role::Trusted,
            },
            Attach {
                name: "two",
                role: Role::Trusted,
            },
        ],
        reported: &["one", "two"],
    },
];

/// The destination names the set `set` reports, ascending.
async fn reported_names(ses: &Ses, case: &str, set: &str) -> Vec<String> {
    let reply = ses.get_destinations(set).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: the read of {set}'s event destinations expected 200, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    let destinations = match reply.member("EventDestinations") {
        Value::Array(destinations) => destinations,
        other => panic!(
            "case {case:?}: the read of {set}'s event destinations expected an array, got \
             {other:?}"
        ),
    };
    let mut names: Vec<String> = destinations
        .iter()
        .map(|destination| {
            destination
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn the_role_decides_every_attach_on_its_own_whatever_the_sequence_around_it() {
    for row in ROWS {
        let case = row.case;
        let ses = Ses::with_iam();
        let trusted = given_role(
            &ses,
            TRUSTED_ROLE,
            &trust(json!([allow_assume(SES_SERVICE_PRINCIPAL)])),
        )
        .await;
        let withheld = given_role(
            &ses,
            WITHHELD_ROLE,
            &trust(json!([allow_assume(OTHER_SERVICE_PRINCIPAL)])),
        )
        .await;
        let created = ses.given_set(row.set).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: setup CreateConfigurationSet expected 200, got {} {} ({})",
            created.status,
            created.error_code(),
            created.message()
        );

        // Every attach of the row, in order. The destinations all name ONE delivery stream,
        // so nothing about the stream can be what separates an accepted attach from a
        // refused one.
        for (index, attach) in row.attaches.iter().enumerate() {
            let role_arn = match attach.role {
                Role::Trusted => trusted.as_str(),
                Role::Withheld => withheld.as_str(),
            };
            let reply = ses
                .create_destination(row.set, attach.name, firehose_definition(role_arn))
                .await;
            match attach.role {
                Role::Withheld => {
                    assert_eq!(
                        (reply.status, reply.error_code().as_str()),
                        (400, "BadRequestException"),
                        "case {case:?}: attach {index} of {:?} expected a 400 \
                         BadRequestException, got {} {} ({})",
                        attach.name,
                        reply.status,
                        reply.error_code(),
                        reply.message()
                    );
                    assert_eq!(
                        reply.message(),
                        refusal(role_arn),
                        "case {case:?}: attach {index} of {:?} expected the \
                         could-not-assume-role sentence about {role_arn}, got {:?}",
                        attach.name,
                        reply.message()
                    );
                    assert_eq!(
                        reported_role(&ses, row.set, attach.name).await,
                        None,
                        "case {case:?}: attach {index} of {:?} was refused, so it must have \
                         stored nothing",
                        attach.name
                    );
                }
                Role::Trusted => {
                    assert_eq!(
                        reply.status,
                        200,
                        "case {case:?}: attach {index} of {:?} expected 200, got {} {} ({})",
                        attach.name,
                        reply.status,
                        reply.error_code(),
                        reply.message()
                    );
                    assert!(
                        reply.is_empty_body(),
                        "case {case:?}: attach {index} of {:?} answers an output shape \
                         declaring no members, so the body must be empty, got {:?}",
                        attach.name,
                        reply.member("EventDestinations")
                    );
                    assert_eq!(
                        reported_role(&ses, row.set, attach.name).await.as_deref(),
                        Some(role_arn),
                        "case {case:?}: attach {index} of {:?} was accepted, so the set must \
                         report it sending its events under {role_arn}",
                        attach.name
                    );
                }
            }
        }

        assert_eq!(
            reported_names(&ses, case, row.set).await,
            row.reported,
            "case {case:?}: expected the set to report exactly the destinations whose role \
             grants this service the assume"
        );
    }
}
