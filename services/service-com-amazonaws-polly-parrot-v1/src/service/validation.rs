// SPDX-License-Identifier: CC0-1.0

//! The accepted value set this service STATES when it refuses a value outside an
//! enumerated member.
//!
//! RULE: a request-shape refusal states an enumerated member's accepted values as the
//! SERVICE publishes that enum's set — its own membership, in its own fixed order —
//! which is a property of the service and not of the order the model happens to
//! declare that enum's members in. The set is a per-service constant: nothing in the
//! request and nothing in the store varies it, and an enum is recognised by its value
//! SET rather than by the member that carried the rejected value, so every member
//! targeting one enum — of any operation, at any depth, body-bound, query-bound or
//! header-bound — states that enum's set alike.
//!
//! Only the DATA is this crate's: [`STATED_LANGUAGE_CODE_VALUES`] and the table it sits
//! in. The restatement itself, the clause and the envelope all come from the shared home
//! for boundary-refusal wording ([`service_common::validation_message`]) and from the
//! framework underneath it, so this service cannot word a constraint differently from
//! any other.
//!
//! Which values the service ACCEPTS is a different question, and stays with the model:
//! the enum a member targets is what admits or refuses a value. A set this service
//! publishes a statement of may therefore list fewer values than the model declares —
//! that is what Amazon Polly's own refusal does — and listing a value is not what makes
//! it acceptable.
//!
//! Two paths reach this module, so a header-bound member and a body member state one
//! enum's set identically — both from the VIOLATIONS the framework reports, never from a
//! sentence already written:
//!
//! * [`stated_refusal`], from [`crate::service::EmulatedService::override_refusal`] —
//!   every operation whose request the framework's own boundary walks; and
//! * [`stated_envelope`], from the one hand-authored operation, which reads its own
//!   request through the shared reader
//!   ([`service_common::event_stream::read_request`]) and is handed the same violations.

use service_common::validation_message::{in_stated_enum_order, StatedEnumValueSets};
use smithy_server_runtime::{Refusal, Refused, Violation};

use crate::ops::types::LANGUAGE_CODE_VALUES;

/// The error code this service answers a request-shape refusal with.
///
/// It is the code the framework's own boundary names for one, kept here so restating
/// the message cannot change the code a client branches on.
const REQUEST_VALIDATION_ERROR_CODE: &str = "ValidationException";

/// The accepted languages Amazon Polly STATES when it refuses a value outside the
/// `LanguageCode` enum, in the order it states them in.
///
/// OBSERVED: the bracketed list of the `ValidationException` a `LanguageCode` outside
/// the enum earns (recorded trace
/// `polly.synthesize-speech-language-not-supported-error`). It is the service's own
/// published set, so it is neither sorted nor in the model's declaration order, and it
/// is not the set the model declares: the model also declares `en-SG`
/// (`smithy_query(query="shape",shape="LanguageCode")`), which this message does not
/// list. Every value here IS one the model declares, which the suite checks — a
/// statement that advertised a value the member would refuse would be worse than the
/// declaration order it replaces.
pub const STATED_LANGUAGE_CODE_VALUES: [&str; 41] = [
    "en-IE", "ar-AE", "en-US", "fr-BE", "en-IN", "es-MX", "en-ZA", "tr-TR", "ru-RU", "ro-RO",
    "pt-PT", "pl-PL", "nl-NL", "it-IT", "is-IS", "fr-FR", "fi-FI", "es-ES", "de-DE", "yue-CN",
    "ko-KR", "en-NZ", "en-GB-WLS", "hi-IN", "de-CH", "arb", "nl-BE", "cy-GB", "cs-CZ", "cmn-CN",
    "da-DK", "en-AU", "pt-BR", "nb-NO", "sv-SE", "ja-JP", "es-US", "ca-ES", "fr-CA", "en-GB",
    "de-AT",
];

/// Every enum this service publishes its own statement of the value set for, keyed by
/// the set the MODEL declares for that enum (`crate::ops::types::*_VALUES`).
///
/// One row per enum whose stated set is known to differ from the declared one. An enum
/// absent from this table is stated as the model declares it, because this service has
/// shown no other statement of it.
pub const STATED_ENUM_VALUE_SETS: &StatedEnumValueSets<'static> =
    &[(LANGUAGE_CODE_VALUES, &STATED_LANGUAGE_CODE_VALUES)];

/// One violation as THIS SERVICE states it: an enum value set replaced by the service's
/// own statement of that set.
///
/// Every other violation kind — and an enum this service publishes no statement of —
/// comes back unchanged, which is what [`stated_refusal`] reads to decide whether to
/// reword at all.
pub fn as_this_service_states_it(violation: &Violation) -> Violation {
    in_stated_enum_order(violation, STATED_ENUM_VALUE_SETS)
}

/// How this service states a refusal the framework made before any handler ran, or
/// `None` when it states that refusal exactly as the framework already does.
///
/// Only the value set inside an enum clause moves; the code and the envelope are the
/// framework's, so a refusal this service does restate stays the same 400
/// `ValidationException` a client branches on.
pub fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let stated: Vec<Violation> = violations.iter().map(as_this_service_states_it).collect();
    if stated == *violations {
        return None;
    }
    Some(Refused {
        code: REQUEST_VALIDATION_ERROR_CODE.to_string(),
        message: aws_smithy_schema_server::constraints::envelope(&stated)?,
    })
}

/// The same refusal the boundary states, over violations a HANDLER was handed rather
/// than ones the seam intercepted: every one of them stated as this service states it
/// ([`as_this_service_states_it`]), in the framework's own envelope.
///
/// For the hand-authored operation that reads its own request through the shared reader
/// ([`service_common::event_stream::read_request`]): the violations arrive in the same
/// form the seam gets them in, so that operation and the request boundary cannot disagree
/// about one enum. The reader reports a refusal only over a non-empty walk, so the
/// envelope is always there.
pub fn stated_envelope(violations: &[Violation]) -> String {
    let stated: Vec<Violation> = violations.iter().map(as_this_service_states_it).collect();
    aws_smithy_schema_server::constraints::envelope(&stated).unwrap_or_default()
}

/// The sentence a refusal of ONE member carries, for a rule of the SERVICE's own that
/// the model does not express — a member that must hold a particular value, or a group
/// of members that cannot all be set at once.
///
/// The envelope and the clause are the ones the request boundary words a single
/// violation with ([`aws_smithy_schema_server::constraints`]), so a caller cannot tell a
/// rule the service checks from a constraint the model states by the shape of the
/// sentence: only the REQUIREMENT inside it is the rule's own. `name` is the member's
/// path in the lowerCamel convention the boundary names members by, `value` is the value
/// the request carried (echoed, so the caller reads back what it sent) and `constraint`
/// states what that member must satisfy.
pub fn one_member_refusal(name: &str, value: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{name}' failed to satisfy \
         constraint: {constraint}"
    )
}

/// [`one_member_refusal`] for a member whose value must not be echoed.
///
/// The form the boundary itself takes for a `@sensitive` member: the path and the
/// requirement, and no value — which is also the only honest form for a member whose
/// value is a structure rather than a scalar.
pub fn one_unechoed_member_refusal(name: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value at '{name}' failed to satisfy constraint: \
         {constraint}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_schema_server::constraints::envelope;
    use smithy_server_runtime::ViolationKind;
    use std::collections::BTreeSet;

    /// A violation of `path` carrying `value`, outside the enum whose values are
    /// `declared` — the shape the framework hands this module.
    fn enum_violation(path: &str, value: &str, declared: &[&str]) -> Violation {
        Violation {
            path: path.to_string(),
            kind: ViolationKind::EnumValue(
                declared.iter().map(|value| (*value).to_string()).collect(),
            ),
            value: Some(value.to_string()),
            sensitive: false,
        }
    }

    /// A statement of a set may list fewer values than the model declares, but never a
    /// value the model does not declare: the member would refuse it.
    #[test]
    fn every_stated_value_is_one_the_model_declares() {
        for (modelled, stated) in STATED_ENUM_VALUE_SETS {
            for value in *stated {
                assert!(
                    modelled.contains(value),
                    "stated value {value:?} is not one the model declares ({modelled:?})"
                );
            }
            let unique: BTreeSet<&&str> = stated.iter().collect();
            assert_eq!(
                unique.len(),
                stated.len(),
                "a stated set lists no value twice, got {stated:?}"
            );
        }
    }

    /// A refusal naming the declared set is restated, in the order this service states
    /// that enum; one naming anything else keeps the framework's own rendering.
    #[test]
    fn only_a_published_set_is_restated() {
        let declared: Vec<&str> = LANGUAGE_CODE_VALUES.to_vec();
        let violation = enum_violation("languageCode", "xx-XX", &declared);
        let restated = as_this_service_states_it(&violation);
        assert_eq!(
            restated.kind,
            ViolationKind::EnumValue(
                STATED_LANGUAGE_CODE_VALUES
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect()
            ),
            "the published statement must replace the declared set"
        );
        assert_eq!(
            (restated.path, restated.value),
            ("languageCode".to_string(), Some("xx-XX".to_string())),
            "restating the set changes nothing else about the violation"
        );

        let other = enum_violation("outputFormat", "flac", &["mp3", "pcm"]);
        assert_eq!(
            as_this_service_states_it(&other),
            other,
            "a set with no published statement is left alone"
        );
        assert_eq!(
            stated_refusal(&Refusal::Invalid(vec![other])),
            None,
            "a refusal this service states as the framework does is not reworded"
        );
        assert_eq!(
            stated_refusal(&Refusal::Unauthenticated),
            None,
            "an authentication refusal states no value set"
        );
    }

    /// Restating the violations a handler was handed produces exactly the sentence the
    /// refusal seam produces for the same violations.
    #[test]
    fn the_handler_path_and_the_seam_path_agree() {
        let declared: Vec<&str> = LANGUAGE_CODE_VALUES.to_vec();
        let violations = vec![
            enum_violation("languageCode", "xx-XX", &declared),
            enum_violation("outputFormat", "flac", &["mp3", "pcm"]),
        ];
        let seam = stated_refusal(&Refusal::Invalid(violations.clone()))
            .expect("a published set is restated");
        assert_eq!(
            stated_envelope(&violations),
            seam.message,
            "the two paths must word one refusal alike"
        );

        // Nothing to restate: the envelope is still the framework's, so a refusal with no
        // published set in it reads exactly as the framework words it.
        let other = vec![enum_violation("outputFormat", "flac", &["mp3", "pcm"])];
        assert_eq!(
            stated_envelope(&other),
            envelope(&other).expect("one violation"),
            "a refusal stating no published set must read as the framework words it"
        );
        assert_eq!(
            stated_envelope(&[]),
            String::new(),
            "no violation is no sentence"
        );
    }
}
