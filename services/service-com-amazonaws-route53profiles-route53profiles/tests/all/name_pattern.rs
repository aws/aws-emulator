// SPDX-License-Identifier: CC0-1.0

//! How this service applies the modelled `Name` constraint, over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the
//! same restJson1 path a real SDK client takes: routing, deserialization, the
//! centralized constraint walk, the handler and error rendering.
//!
//! The rule under test: a value outside the `@pattern` the model declares on `Name`
//! is REFUSED — HTTP 400, the validation error shape in `x-amzn-errortype`, and the
//! front door's own constraint envelope blaming the `name` member and advertising
//! that expression — whichever operation carries the `Name`, and whichever half of
//! the expression the value breaks; a value inside it is accepted and persisted.
//!
//! The expression the model declares carries a negative lookahead, which the
//! centralized walk's regex engine cannot compile — so this is the constraint this
//! service applies itself, and the suite pins that it is applied in exactly the words
//! the walk applies the others in.
//!
//! The envelope, the member path and the advertised expression are TRANSCRIBED as
//! literal text, so the suite states them independently of the code that renders them.

use serde_json::json;

use super::harness::Emulator;

/// The expression a refusal ADVERTISES for `Name`: the modelled `@pattern`
/// (`smithy_query(query="shape",shape="Name")`) without the whole-value anchors it
/// is written with. Transcribed, so the suite states the published expression
/// independently of the code that strips those anchors.
const PUBLISHED_NAME_PATTERN: &str = r"(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)";

/// The member path a refusal of a `Name` blames: the modelled member name as the
/// validating walk spells a path segment.
const NAME_MEMBER_PATH: &str = "name";

/// The error shape a constraint refusal of this service is reported as.
const VALIDATION_ERROR: &str = "ValidationException";

/// A Profile that exists, so a refused `Name` is the ONLY thing wrong with an
/// association request.
const HOST_PROFILE_NAME: &str = "name-rule-host";

/// An ARN naming a resource a Profile can hold, for the resource-association
/// operation's own `Name` member.
fn associable_arn(index: usize) -> String {
    format!("arn:aws:route53:::hostedzone/Z0456789101{index:04}ABCD")
}

/// The whole message a `Name` outside the pattern is refused with.
fn refusal_message(value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{NAME_MEMBER_PATH}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {PUBLISHED_NAME_PATTERN}"
    )
}

/// Values outside the modelled pattern, each row a different reason it is outside:
/// the all-digits names its negative lookahead forbids (one digit, several, and a
/// run at the modelled 64-character maximum), the empty value its `+` admits no
/// match of even though `@length` allows a zero-length string, and values carrying a
/// character its class does not list — punctuation, a path separator, a non-ASCII
/// letter and a control character.
fn refused_names() -> Vec<String> {
    vec![
        "0".to_string(),
        "123456".to_string(),
        "9".repeat(64),
        String::new(),
        "profile!".to_string(),
        "profile.name".to_string(),
        "profile/name".to_string(),
        "caf\u{e9}".to_string(),
        "profile\tname".to_string(),
    ]
}

/// Values inside the modelled pattern, spanning what its character class admits and
/// the digits its lookahead forbids only when they are the WHOLE value: a single
/// letter, a plain word, a hyphen, an underscore with an interior space, an
/// apostrophe, a digit-leading name, digits with a space, and exactly the modelled
/// 64-character maximum.
fn accepted_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "nameprofile".to_string(),
        "name-profile".to_string(),
        "My_Profile 1".to_string(),
        "o'brien".to_string(),
        "0a".to_string(),
        "1 2".to_string(),
        "A".repeat(64),
    ]
}

#[tokio::test]
async fn a_name_outside_the_modelled_pattern_is_refused_by_every_operation_that_takes_one() {
    let em = Emulator::new();
    let host = em
        .create_named(HOST_PROFILE_NAME)
        .await
        .nested_string("Profile", "Id");

    for (index, name) in refused_names().into_iter().enumerate() {
        // One row per operation carrying a `Name`, each with everything else about
        // the request valid, so the refusal can only be the name's.
        let replies = vec![
            (
                "CreateProfile",
                em.create_profile(json!({ "Name": name, "ClientToken": format!("tok-bad-{index}") }))
                    .await,
            ),
            (
                "AssociateProfile",
                em.associate(&host, &format!("vpc-0bad{index}"), &name).await,
            ),
            (
                "AssociateResourceToProfile",
                em.associate_resource(&host, &associable_arn(index), &name, None)
                    .await,
            ),
        ];

        for (operation, reply) in replies {
            let case = format!("{operation} name={name:?}");
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {} / {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type_header.as_deref(),
                Some(VALIDATION_ERROR),
                "case {case}: expected x-amzn-errortype {VALIDATION_ERROR:?}, got {:?}",
                reply.error_type_header
            );
            let expected = refusal_message(&name);
            assert_eq!(
                reply.lower_message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                reply.lower_message()
            );
        }
    }

    // Nothing a refused request named reached the store: the only Profile is the host.
    let listed = em.list_profiles("").await;
    assert_eq!(
        listed.list("ProfileSummaries").len(),
        1,
        "case \"after every refused Name\": a refused request must persist nothing, got {:?}",
        listed.body
    );
    let associations = em.list_associations(&format!("?ProfileId={host}")).await;
    assert_eq!(
        associations.list("ProfileAssociations").len(),
        0,
        "case \"after every refused Name\": a refused association must persist nothing, got {:?}",
        associations.body
    );
}

#[tokio::test]
async fn a_name_past_the_modelled_maximum_is_refused_before_the_handler() {
    // The `@length(0..64)` half of the constraint is one the centralized walk CAN
    // apply, so a longer name never reaches the pattern check at all: it is refused
    // in the same envelope, stating the length requirement instead.
    let em = Emulator::new();
    let over_long = "A".repeat(65);
    let reply = em
        .create_profile(json!({ "Name": over_long, "ClientToken": "tok-too-long" }))
        .await;
    let case = format!("CreateProfile name={} characters", over_long.len());
    assert_eq!(
        reply.status, 400,
        "case {case}: expected HTTP 400, got {} / {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type_header.as_deref(),
        Some(VALIDATION_ERROR),
        "case {case}: expected x-amzn-errortype {VALIDATION_ERROR:?}, got {:?}",
        reply.error_type_header
    );
    let stated = reply.lower_message();
    let requirement = format!(
        "at '{NAME_MEMBER_PATH}' failed to satisfy constraint: Member must have length less than \
         or equal to 64"
    );
    assert!(
        stated.starts_with("1 validation error detected: ") && stated.contains(&requirement),
        "case {case}: expected the length requirement {requirement:?} in the one-violation \
         envelope, got {stated:?}"
    );
}

#[tokio::test]
async fn a_name_inside_the_modelled_pattern_is_accepted_and_echoed() {
    let em = Emulator::new();
    let host = em
        .create_named(HOST_PROFILE_NAME)
        .await
        .nested_string("Profile", "Id");

    for (index, name) in accepted_names().into_iter().enumerate() {
        let case = format!("name={name:?}");

        let created = em
            .create_profile(json!({ "Name": name, "ClientToken": format!("tok-ok-{index}") }))
            .await;
        assert_eq!(
            created.status, 200,
            "case CreateProfile {case}: expected HTTP 200, got {} / {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.nested_string("Profile", "Name"),
            name,
            "case CreateProfile {case}: the Name must be echoed, got {:?}",
            created.body
        );
        // Read it back, so the accepted name is one the store really holds.
        let id = created.nested_string("Profile", "Id");
        let fetched = em.get_profile(&id).await;
        assert_eq!(
            fetched.nested_string("Profile", "Name"),
            name,
            "case GetProfile {case}: the stored Name must be the one requested, got {:?}",
            fetched.body
        );

        let associated = em
            .associate(&host, &format!("vpc-0ok{index}"), &name)
            .await;
        assert_eq!(
            associated.status, 200,
            "case AssociateProfile {case}: expected HTTP 200, got {} / {:?}",
            associated.status, associated.body
        );
        assert_eq!(
            associated.nested_string("ProfileAssociation", "Name"),
            name,
            "case AssociateProfile {case}: the Name must be echoed, got {:?}",
            associated.body
        );

        let resource = em
            .associate_resource(&host, &associable_arn(index), &name, None)
            .await;
        assert_eq!(
            resource.status, 200,
            "case AssociateResourceToProfile {case}: expected HTTP 200, got {} / {:?}",
            resource.status, resource.body
        );
        assert_eq!(
            resource.nested_string("ProfileResourceAssociation", "Name"),
            name,
            "case AssociateResourceToProfile {case}: the Name must be echoed, got {:?}",
            resource.body
        );
    }
}
