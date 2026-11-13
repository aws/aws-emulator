// SPDX-License-Identifier: CC0-1.0

//! A modelled `@pattern` the boundary validator could not compile, applied at that same
//! front door and stated in that same envelope.
//!
//! ## The rule
//!
//! A value a member's `@pattern` does not admit is refused BEFORE the handler runs, with
//! the protocol-level constraint refusal — HTTP 400 `ValidationException` — whose body
//! counts the clauses, names the member by the path a validation message spells it with and
//! quotes the expression the model declares for it. That refusal belongs to the request
//! layer and not to an operation: a handler can only answer one of the errors ITS model
//! declares, which for this service is a Directory Service fault a client branches on as a
//! service error, so a constraint broken by the request would be reported from the wrong
//! layer, in the wrong class, with none of the diagnostic a caller fixes the request from.
//!
//! The framework's constraint walk compiles a `@pattern` with the `regex` crate and treats
//! one it cannot compile as SATISFIED (`pattern_matches` in
//! `vendor/smithy-schema-server/src/constraints.rs`, read-only from here). That crate has no
//! look-around, so every expression this model writes with one — an assertion about the
//! whole value that no character grammar can state — is skipped: the member is admitted
//! whatever it holds. This module is that gap closed, for every such member of every
//! operation it collects, with no member and no expression named here: which members have
//! one is read off the operation's own input schema.
//!
//! The CHECK is the model's expression itself. It is evaluated by a look-around-capable
//! engine (`fancy-regex`, as four sibling service crates already do for the same reason),
//! so the clause — whose expression is rendered from the model's own source text
//! ([`stated_pattern`]) — cannot state a constraint this code does not apply. Two bounds
//! keep that evaluation finite, and both are properties of the inputs rather than of the
//! engine: the expressions are MODEL CONSTANTS, never built from a request, and a
//! backtracking engine is given an explicit step budget ([`PATTERN_BACKTRACK_LIMIT`]). An
//! expression NEITHER engine compiles, and a match that exhausts that budget, are both left
//! SATISFIED — which is the framework's own treatment: no value is refused because of an
//! expression nothing can evaluate.
//!
//! ## Where it is applied
//!
//! In the operation wrapper every routable request of this service passes through
//! ([`super::request_id_header::RequestIdHeaderOperation`]), for the same reason the
//! list-element clause is stated there ([`super::list_element_refusals`]): the check needs
//! the REQUEST, which `Service::override_refusal` is not handed. The cost is that a request
//! whose operation declares such a member is read twice — once here and once in the
//! operation's own sequence — which `services/common/src/validation.rs` records as the core
//! seam this re-read exists only until.
//!
//! A request that satisfies every such expression is handed on untouched, so the framework
//! still words every other refusal, and the clauses of a request that breaks BOTH classes
//! are counted together in the one envelope: the walk's own violations first, in its order,
//! then the ones it skipped.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::aws_smithy_schema::traits::SensitiveTrait;
use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::constraints::{path, PatternTrait};
use ::aws_smithy_schema_server::{envelope, validate};
use ::smithy_server_runtime::{
    DeserializableStruct, Operation, Refused, ServerHttpProtocol, Violation, ViolationKind,
};

use super::VALIDATION_EXCEPTION;

/// The step budget ONE match of one caller value may spend.
///
/// A backtracking engine has no linear-time guarantee in general, so the work one request
/// can buy is bounded here rather than left to the shape of the expression — and this is the
/// only bound for a member whose model declares a `@pattern` and no `@length` maximum, which
/// is what the administrator `Password` declares. The expressions themselves are MODEL
/// CONSTANTS, never built from a request, so the only caller-chosen factor is the value. A
/// match that exhausts the budget is ANSWERED (the value is left admitted), never retried,
/// and never spins.
const PATTERN_BACKTRACK_LIMIT: usize = 1_000_000;

/// How one operation's skipped-`@pattern` refusal is collected: over that operation's own
/// input schema and input type, which is why a collector is needed per operation rather
/// than one for the service.
type Collect = fn(&dyn ServerHttpProtocol, &RtRequest) -> Option<(Vec<Violation>, Refused)>;

/// The collector for each operation this module checks, resolved by the operation's own
/// wire name ([`Operation::NAME`], so no name is spelled twice).
macro_rules! collected_per_operation {
    ( $( $module:ident :: $operation:ident ),* $(,)? ) => {
        /// The collector for the operation `key` names, or `None` for one whose input
        /// declares no `@pattern` the boundary validator skips — the members a clause of
        /// this class can be about at all. An operation with no such member is not read
        /// twice for nothing.
        fn collector(key: &str) -> Option<Collect> {
            $(
                if key == <crate::ops::$module::$operation<()> as Operation>::NAME {
                    return Some(stated_from_request::<crate::ops::$module::$operation<()>>);
                }
            )*
            None
        }
    };
}

// Every operation of this service whose input declares a `@pattern` the boundary validator
// cannot compile, and so never applies: the administrator `Password` both creates that make
// one share (a length assertion and a character-class count, all look-ahead), the
// `TrustPassword` a trust is established with (a character class the engines this crate can
// reach do not know, so it stays admitted — see the module header) and the account
// `UserName` a password reset names (a look-ahead listing the characters a name may not
// carry). `CreateAlias` declares one too and is deliberately NOT here: a recorded refusal of
// a malformed alias quotes that expression with a rendering of its own, which its handler
// states (`crate::ops::create_alias`), and a refusal from here would replace it. The
// operation TYPE is named with a placeholder engine, because nothing here constructs one:
// only the operation's name, its input schema and its input type are read, and all three are
// associated items of the type.
collected_per_operation!(
    create_directory::CreateDirectory,
    create_microsoft_ad::CreateMicrosoftAD,
    create_trust::CreateTrust,
    reset_user_password::ResetUserPassword,
);

/// The `@pattern` a member schema declares, as the model wrote it.
fn modelled_pattern(member: &'static Schema) -> Option<&'static str> {
    let declared = member.traits()?.get(&PatternTrait::TRAIT_ID)?;
    Some(declared.as_any().downcast_ref::<PatternTrait>()?.regex)
}

/// Whether the model marks `member` `@sensitive`, so a clause about it states no value.
fn is_sensitive(member: &'static Schema) -> bool {
    member
        .traits()
        .is_some_and(|traits| traits.get(&SensitiveTrait::TRAIT_ID).is_some())
}

/// One member whose `@pattern` the boundary validator skips: the path a validation message
/// names it by, the expression the model declares, and whether a clause about it may quote
/// the value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkippedPattern {
    path: String,
    pattern: &'static str,
    sensitive: bool,
}

/// Every member of `schema` whose `@pattern` the boundary validator does not apply, because
/// the `regex` crate behind `@pattern` cannot compile it.
///
/// Read off the schema, so nothing here names a member or an expression: the model says
/// which members carry one and the engine says which of those it compiles. The members read
/// are those of the operation input itself, which is where this model declares every
/// look-around expression it has (`the_collected_operations_are_the_ones_the_boundary_skips`
/// is what keeps that true); a `@pattern` on a nested, list or map member would need the
/// walk below to reach it.
fn skipped_patterns(schema: &'static Schema) -> Vec<SkippedPattern> {
    schema
        .members()
        .iter()
        .filter_map(|member| {
            let pattern = modelled_pattern(member)?;
            if boundary_applies(pattern) {
                return None;
            }
            Some(SkippedPattern {
                path: path::member(member.member_name()?),
                pattern,
                sensitive: is_sensitive(member),
            })
        })
        .collect()
}

/// Whether the boundary validator APPLIES `pattern`: whether the `regex` crate it validates
/// with compiles the expression at all. One that does not compile is treated there as
/// satisfied, which is the gap this module closes.
fn boundary_applies(pattern: &str) -> bool {
    ::regex::Regex::new(pattern).is_ok()
}

/// Whether `value` satisfies the modelled `pattern`, or `None` when no engine this crate
/// reaches can decide it — a pattern neither engine compiles, or a match that spent its
/// whole step budget. Undecided leaves the value admitted, the framework's own treatment.
///
/// A Smithy `@pattern` is a FIND rather than a whole-value match
/// (`vendor/smithy-schema-server/src/constraints.rs`), so the expression is searched in the
/// value exactly as the boundary searches the ones it does compile.
fn satisfies(pattern: &'static str, value: &str) -> Option<bool> {
    compiled(pattern)?.is_match(value).ok()
}

/// What one modelled expression compiled to under the look-around-capable engine: the
/// compiled form, or nothing when that engine cannot compile it either.
type Compiled = Option<Arc<::fancy_regex::Regex>>;

/// The compiled form of `pattern` under a look-around-capable engine, or `None` when that
/// engine cannot compile it either.
///
/// Compiled once per expression and held, keyed by the source text, so the compiled form
/// cannot drift from the text a clause quotes. Nothing expensive happens under the lock: it
/// is taken for a lookup and for an insert and never held across a compile or a match, so
/// concurrent requests do not serialise behind one.
fn compiled(pattern: &'static str) -> Compiled {
    static COMPILED: LazyLock<Mutex<HashMap<&'static str, Compiled>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let held = locked(&COMPILED).get(pattern).cloned();
    if let Some(held) = held {
        return held;
    }
    let built = ::fancy_regex::RegexBuilder::new(pattern)
        .backtrack_limit(PATTERN_BACKTRACK_LIMIT)
        .build()
        .ok()
        .map(Arc::new);
    locked(&COMPILED)
        .entry(pattern)
        .or_insert_with(|| built.clone())
        .clone()
}

/// One of this module's caches, locked. `into_inner` on poisoning rather than `unwrap`: a
/// panic elsewhere cannot corrupt a map of compiled model constants, and must not make
/// every later request panic with it.
fn locked<T>(cache: &'static LazyLock<Mutex<T>>) -> MutexGuard<'static, T> {
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Every `@pattern` violation of `input` the boundary validator will not find, against the
/// members `skipped` names.
///
/// The values come from the SAME built input the walk validates, read through the one shared
/// collecting walk (`::service_common::validation::submitted_leaves`), so a clause states
/// the bytes the caller submitted and the path it names them by is the path that walk
/// reports a violation at. A clause about a `@sensitive` member states NO value: the member
/// is a password the caller sent, and a refusal that echoed it would disclose it into every
/// log the response passes through.
fn unseen_pattern_violations(
    input: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    schema: &'static Schema,
    skipped: &[SkippedPattern],
) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (path, value) in ::service_common::validation::submitted_leaves(input, schema) {
        let Some(member) = skipped.iter().find(|member| member.path == path) else {
            continue;
        };
        if satisfies(member.pattern, &value) != Some(false) {
            continue;
        }
        violations.push(Violation {
            path,
            kind: ViolationKind::Pattern(stated_pattern(member.pattern)),
            value: (!member.sensitive).then(|| {
                ::service_common::validation::quoted_echo(&value).into_owned()
            }),
            sensitive: member.sensitive,
        });
    }
    violations
}

/// The expression a clause QUOTES for the modelled `pattern`.
///
/// A Smithy `@pattern` is a FIND, so the expression a member DECLARES and the expression its
/// whole value has to satisfy are two different things wherever the model does not anchor
/// both ends — and a clause states the second, because that is what a caller has to satisfy:
///
/// * an expression the model wraps in a matched `^`…`$` pair already describes the whole
///   value, and is quoted AS THE MODEL DECLARES IT, anchors and all. That is the rendering
///   every other clause of this service carries ([`super::stated_violation`]) and the one its
///   recorded refusals of such a member quote, so a clause built here reads like one the
///   boundary built.
/// * an expression the model anchors at ONE end only leaves the other end OPEN — any run of
///   characters may precede or follow the match — and the clause spells that end out, which
///   is what the recorded Directory Service refusal of the administrator `Password` quotes
///   (its model text opens with `^` and closes on an inner anchor, and the refusal states
///   the unconstrained run that open end admits).
///
/// Both halves are the shared tree-wide readings of an anchor
/// (`::service_common::validation::unanchored` decides whether the model wrote a matched
/// pair, `whole_value_expression` renders the open ends), so no anchor rule is spelled here
/// and the CHECK above still runs the model's own text whatever this quotes.
fn stated_pattern(pattern: &str) -> String {
    if ::service_common::validation::unanchored(pattern) != pattern {
        return pattern.to_string();
    }
    ::service_common::validation::whole_value_expression(pattern)
}

/// The refusal this service states for `req` under the operation `O`, or `None` when the
/// request satisfies every `@pattern` the boundary validator skips — which leaves the
/// framework's own answer, and this service's other wording of it, exactly as they are.
fn stated_from_request<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let skipped = skipped_patterns(O::INPUT_SCHEMA);
    if skipped.is_empty() {
        return None;
    }
    // The request as the framework itself reads it: a body it cannot open, or a `@required`
    // member absent, is a refusal the framework's own sequence words, so nothing is
    // collected for it here.
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let input = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer).ok()?;
    let unseen = unseen_pattern_violations(&input, O::INPUT_SCHEMA, &skipped);
    if unseen.is_empty() {
        return None;
    }
    // The clauses of both classes, counted in ONE envelope: the walk's own violations in
    // the order it found them, then the ones it skipped.
    let mut violations = validate(&input, O::INPUT_SCHEMA);
    violations.extend(unseen);
    let restated: Vec<Violation> = violations.iter().map(super::stated_violation).collect();
    let message = envelope(&restated)?;
    Some((
        violations,
        Refused {
            // The code a client branches on for a refused value, restated because a chosen
            // refusal carries both halves.
            code: VALIDATION_EXCEPTION.to_string(),
            message,
        },
    ))
}

/// The refusal this service states for one request, by the operation its routing key names,
/// or `None` for an operation this class cannot arise in and for a request that breaks no
/// such expression.
pub fn stated(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let key = protocol.routing_key(req).ok()?;
    collector(key)?(protocol, req)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the operations collected are exactly the ones whose input declares a
    /// `@pattern` the boundary validator cannot compile — so no request is read twice for a
    /// clause that cannot arise in it, and no member that needs this check is left to the
    /// walk that skips it.
    #[test]
    fn the_collected_operations_are_the_ones_the_boundary_skips() {
        // Each collected operation's input really does declare such a member, and declares
        // it at the top level, where the walk above reads it.
        for (key, schema) in [
            (
                "CreateDirectory",
                crate::ops::types::CREATE_DIRECTORY_INPUT_SCHEMA,
            ),
            (
                "CreateMicrosoftAD",
                crate::ops::types::CREATE_MICROSOFT_AD_INPUT_SCHEMA,
            ),
            ("CreateTrust", crate::ops::types::CREATE_TRUST_INPUT_SCHEMA),
            (
                "ResetUserPassword",
                crate::ops::types::RESET_USER_PASSWORD_INPUT_SCHEMA,
            ),
        ] {
            assert!(
                collector(key).is_some(),
                "case {key:?}: expected the operation to be collected"
            );
            assert!(
                !skipped_patterns(schema).is_empty(),
                "case {key:?}: expected a member whose @pattern the boundary skips"
            );
        }

        // ...and an operation whose input declares only expressions the boundary itself
        // applies is not collected: its members are refused by the walk, not from here.
        for (key, schema) in [
            (
                "CreateComputer",
                crate::ops::types::CREATE_COMPUTER_INPUT_SCHEMA,
            ),
            (
                "RegisterEventTopic",
                crate::ops::types::REGISTER_EVENT_TOPIC_INPUT_SCHEMA,
            ),
            (
                "DeleteDirectory",
                crate::ops::types::DELETE_DIRECTORY_INPUT_SCHEMA,
            ),
        ] {
            assert!(
                collector(key).is_none(),
                "case {key:?}: expected the operation not to be collected"
            );
            assert!(
                skipped_patterns(schema).is_empty(),
                "case {key:?}: expected every declared @pattern to be one the boundary \
                 applies, got {:?}",
                skipped_patterns(schema)
            );
        }

        assert!(
            collector("NotAnOperation").is_none(),
            "case {:?}: expected no collector for a name the model does not declare",
            "NotAnOperation"
        );
    }

    /// RULE: the check IS the model's expression — a value it admits is not refused, one it
    /// does not is, and an expression no engine here compiles leaves the value admitted.
    #[test]
    fn the_check_is_the_modelled_expression() {
        let password = skipped_patterns(crate::ops::types::CREATE_DIRECTORY_INPUT_SCHEMA)
            .into_iter()
            .find(|member| member.path == "password")
            .expect("the administrator password's @pattern is one the boundary skips");
        assert!(
            password.sensitive,
            "the administrator password is @sensitive, so its clause states no value"
        );

        // Three of Active Directory's four character classes and a length inside 8..=64 is
        // what the expression asserts; each row names which clause it breaks or satisfies.
        // The published length bounds, built FROM the bounds rather than written out, with
        // three character classes so the LENGTH is the clause each row turns on.
        let at_the_maximum = format!("Sim2{}", "a".repeat(60));
        let past_the_maximum = format!("Sim2{}", "a".repeat(61));
        let rows: [(&str, &str, Option<bool>); 8] = [
            ("three classes, no symbol", "Sim2RealPassword", Some(true)),
            ("all four classes", "Sim2Real#Pass99", Some(true)),
            ("at the minimum length", "Sim2Real", Some(true)),
            ("one below the minimum", "Sim2Rea", Some(false)),
            ("lower and digits only", "sim2real99pass", Some(false)),
            ("upper and lower only", "SimRealPassword", Some(false)),
            ("at the maximum length", at_the_maximum.as_str(), Some(true)),
            ("one past the maximum", past_the_maximum.as_str(), Some(false)),
        ];
        for (case, value, expected) in rows {
            let decided = satisfies(password.pattern, value);
            assert_eq!(
                decided, expected,
                "case {case:?}: expected the modelled expression to decide {expected:?}, got \
                 {decided:?}"
            );
        }

        // An expression neither engine compiles decides nothing, so the value stays
        // admitted — the treatment the boundary validator itself gives one.
        assert_eq!(
            satisfies("^(?<!x)(?R)$", "anything"),
            None,
            "an expression no engine here compiles must decide nothing"
        );
    }

    /// RULE: the expression a clause QUOTES is the one the whole value has to satisfy — the
    /// model's text as declared where the model wraps it in a matched anchor pair, and with
    /// an end the model leaves open spelled out where it does not.
    #[test]
    fn a_clause_quotes_the_expression_the_whole_value_must_satisfy() {
        // One row per form the ends of an expression can take, each written the way a
        // `@pattern` of this model writes it.
        let rows: [(&str, &str, &str); 4] = [
            (
                "a matched pair (an event topic name)",
                r"^[a-zA-Z0-9_-]+$",
                r"^[a-zA-Z0-9_-]+$",
            ),
            (
                "a matched pair around look-ahead (a reset's account name)",
                r#"^(?!.*\\|.*@).*$"#,
                r#"^(?!.*\\|.*@).*$"#,
            ),
            (
                "anchored at the start only (the administrator password)",
                r"^(?=^.{8,64}$)((?=.*\d)(?=.*[A-Z])(?=.*[a-z]))^",
                r"(?=^.{8,64}$)((?=.*\d)(?=.*[A-Z])(?=.*[a-z]))^.*",
            ),
            (
                "anchored at the end only",
                r"([\da-zA-Z]+)$",
                r".*([\da-zA-Z]+)",
            ),
        ];
        for (case, declared, expected) in rows {
            let stated = stated_pattern(declared);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
