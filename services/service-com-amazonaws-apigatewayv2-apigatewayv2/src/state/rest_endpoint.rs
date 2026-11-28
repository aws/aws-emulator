// SPDX-License-Identifier: CC0-1.0

//! The REST endpoint a `ProductRestEndpointPage` documents, as the services that OWN API
//! Gateway's APIs report it.
//!
//! An `IdentifierParts` names one endpoint by its `RestApiId` and one of that API's stages.
//! API Gateway keeps ONE api-id space for the account: the REST APIs its older control
//! plane creates (`apigateway:CreateRestApi`, `apigateway:CreateStage`) and the HTTP and
//! WebSocket APIs this service creates (`CreateApi`, `CreateStage`) are addressed by ids
//! drawn from the same space and served on the same `execute-api` hosts. So "does the
//! account have the api this identifier names, with the stage it names" is a question asked
//! of EVERY store those APIs live in, and the identifier resolves when ANY of them holds the
//! pair. A page's content is GENERATED from the endpoint — "By default, this is the
//! documentation of your REST API from API Gateway" (`DisplayContentOverrides$Body`) — so a
//! write naming a pair no store holds is refused.
//!
//! Whether that content can be GENERATED is a second, narrower question
//! ([`rest_endpoint_generation_failure`]): the documentation a page is generated from lives
//! only in the store that owns REST APIs, and it is the one endpoint the identifier's path
//! and method name there. A write whose pair resolves is admitted, and a generation with no
//! such endpoint to read settles the page on its terminal failure status instead of
//! producing a page — which is what the modelled `Status`/`StatusException` pair reports.
//!
//! Another service's store is reached through its own `state` accessor on the SHARED engine,
//! the way [`crate::service::authorization`] reads the IAM and STS stores, and under the same
//! lock order: every store asked here is locked and released BEFORE this service's own store
//! lock is taken by the caller, so each caller resolves first and reaches its own store
//! after.
//!
//! Nothing is cached: a write's identifier is resolved by that write, so an API created
//! after it does not retroactively change what an already-stored page documents, and a read
//! of the page asks nothing of anybody.

use super::*;

/// The sentence this service refuses a `RestEndpointIdentifier` that names no API of the
/// account, or no stage of the API it names, with: the identifier identifies no endpoint to
/// document.
///
/// Held here once, so every writer of a page refuses the same fact with the same words. It
/// has no variable parts — API Gateway names neither the API nor the stage in it.
pub const UNRESOLVED_REST_ENDPOINT_MESSAGE: &str =
    "Invalid restEndpointIdentifier stage or restApiId";

/// `Some(the refusal message)` when the REST endpoint `parts` identify cannot be resolved,
/// and `None` when it resolves.
///
/// A page's content is GENERATED from the endpoint it documents, so a write naming one that
/// does not exist describes no page at all: the write is refused `BadRequestException`
/// rather than storing a page that points at nothing. The identifier resolves when ANY store
/// of the account's APIs holds the `RestApiId`/`stage` pair — see this module's
/// documentation for why there is more than one.
///
/// A HOP THAT CANNOT BE MADE IS EVIDENCE OF NOTHING: an engine carrying no API Gateway (v1)
/// store cannot be asked which REST APIs the account has, so an identifier none of the
/// stores that CAN be asked holds is still admitted — the pair may well name an API of the
/// store that is missing. That is the same reading [`super::ec2_subnets`] takes of silence
/// from the owner of subnets.
///
/// Call it BEFORE this service's own store lock is taken — see this module's documentation.
pub fn unresolved_rest_endpoint<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    parts: &wire::IdentifierParts,
) -> Option<String> {
    let answers = [
        rest_api_holds_stage(engine, parts),
        own_api_holds_stage(engine, parts),
    ];
    // One store holding the pair resolves the identifier, whichever store it is: the api-id
    // space is the account's, not one control plane's.
    if answers.contains(&Some(true)) {
        return ::std::option::Option::None;
    }
    // A store that could not be asked at all leaves "the account has no such api" unproven,
    // so nothing is refused on it.
    if answers.iter().any(::std::option::Option::is_none) {
        return ::std::option::Option::None;
    }
    Some(UNRESOLVED_REST_ENDPOINT_MESSAGE.to_string())
}

/// The `exception` a page's `StatusException` names when its content could not be generated:
/// the class API Gateway raises for a `RestEndpointIdentifier` that identifies no REST endpoint
/// to document — the PAGE'S OWN validation failure, which is the same class a write carrying
/// such an identifier is refused with (`BadRequestException`, "The request is not valid, for
/// example, the input is incomplete or incorrect").
///
/// `StatusException$Exception` is "The exception" (`__stringMin1Max256`), so the member names
/// the failure rather than describing it; the sentence goes in `Message` ("The error
/// message"). What it names is the failure of THIS page: the exception of a lookup this
/// service made inside another component's store is that component's, and it describes an
/// object the caller never addressed.
pub const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// `Some(the StatusException to report)` when the content a page is generated FROM cannot be
/// generated for the REST endpoint `parts` identify, and `None` when it can.
///
/// A product REST endpoint page's content is the documentation of a REST API — "By default,
/// this is the documentation of your REST API from API Gateway"
/// (`DisplayContentOverrides$Body`) — so the generation reads the API the identifier names out
/// of the service that OWNS REST APIs and their documentation (the API Gateway v1 control
/// plane), under the stage it names. That is a NARROWER question than the one the write is
/// admitted on ([`unresolved_rest_endpoint`], which resolves the pair against every store the
/// account's APIs live in, because the account has ONE api-id space): an id that names an HTTP
/// or WebSocket API of this service names an api the account really has, and no REST API
/// documentation to generate a page from. The generation therefore FAILS rather than producing
/// a page, which is the state the modelled `Status`/`StatusException` pair reports
/// ([`super::ProductRestEndpointPage::reported_status`]).
///
/// What it REPORTS is the page's own failure: the identifier the page carries identifies no
/// REST endpoint whose documentation can be generated, so the exception named is the class of
/// that validation failure and the message is this service's one sentence for it
/// ([`generation_failure`], [`UNRESOLVED_REST_ENDPOINT_MESSAGE`]). The internal lookup this
/// function makes in the store that owns REST APIs has refusals of its own, which name a REST
/// API identifier and interpolate the account that store answers for; those explain a read of
/// THAT record and not this page, so none of them is carried into the page's `StatusException`.
/// The identifier's `Method` and `Path` select WHICH endpoint of the API the page documents
/// rather than where its documentation lives, so they take no part here either — the same
/// reading [`super::rest_endpoint_invoke_url`] applies to them.
///
/// A HOP THAT CANNOT BE MADE IS EVIDENCE OF NOTHING, exactly as in
/// [`unresolved_rest_endpoint`]: an engine carrying no API Gateway (v1) store cannot be asked
/// what documentation the account's REST APIs have, so no failure is claimed there.
///
/// Call it BEFORE this service's own store lock is taken — see this module's documentation.
pub fn rest_endpoint_generation_failure<
    Eng: ::std::marker::Send + ::std::marker::Sync + 'static,
>(
    engine: &Eng,
    parts: &wire::IdentifierParts,
) -> Option<wire::StatusException> {
    use ::service_com_amazonaws_apigateway_backplanecontrolservice::state as rest;

    let owner = rest::service_state(engine)?;
    let rest_apis = owner.lock();
    let Some(rest_api) = rest_apis.rest_apis.get(&parts.rest_api_id) else {
        return Some(generation_failure());
    };
    // The documentation generated is of the ENDPOINT OF ONE STAGE, so a REST API the account
    // has under a name it has no such stage for leaves nothing to generate either. The write
    // is admitted while ANY store holds the pair, so this is reached where the account holds an
    // api of that id in BOTH control planes and only the OTHER one carries the stage — the two
    // stores mint ids from sequences of their own, so one id can name a record in each.
    if !rest_api.stages.contains_key(&parts.stage) {
        return Some(generation_failure());
    }
    ::std::option::Option::None
}

/// The `StatusException` a failed generation reports: the class of the page's own failure
/// ([`BAD_REQUEST_EXCEPTION`]) and the one sentence this service states for a
/// `RestEndpointIdentifier` that identifies no REST endpoint to document
/// ([`UNRESOLVED_REST_ENDPOINT_MESSAGE`]).
///
/// It is the PAGE'S failure that is reported, and the page failed on its own
/// `RestEndpointIdentifier`: that is the same fact a write carrying such an identifier is
/// refused with, so it is stated in the same words rather than in words of this emulator's own
/// composition — API Gateway's sentence for that fact is the one attested here, and it has no
/// variable parts (it names neither the API nor the stage). A page's `StatusException` may well
/// read differently on AWS than that refusal does, but the text of the one the recording carries
/// is withheld from this checkout, and a sentence invented to fill the gap would be wording no
/// client could rely on; this one is at least the wording API Gateway is known to state for the
/// failure the page hit.
///
/// What it never carries is a refusal of the internal lookup this module made inside the store
/// that owns REST APIs: that one names a REST API identifier the caller never addressed and
/// interpolates the account that store answers for.
fn generation_failure() -> wire::StatusException {
    wire::StatusException {
        exception: Some(BAD_REQUEST_EXCEPTION.to_string()),
        message: Some(UNRESOLVED_REST_ENDPOINT_MESSAGE.to_string()),
    }
}

/// Whether the REST (API Gateway v1) control plane holds the api `parts` names with the
/// stage it names, or `None` when that service cannot be asked at all.
fn rest_api_holds_stage<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    parts: &wire::IdentifierParts,
) -> Option<bool> {
    let owner = ::service_com_amazonaws_apigateway_backplanecontrolservice::state::service_state(
        engine,
    )?;
    let rest_apis = owner.lock();
    Some(
        rest_apis
            .rest_apis
            .get(&parts.rest_api_id)
            // A page documents ONE endpoint of ONE stage, and it is the stage that is
            // deployed and invocable, so an API the account has under a name it has no such
            // stage for identifies nothing either.
            .is_some_and(|rest_api| rest_api.stages.contains_key(&parts.stage)),
    )
}

/// Whether THIS service holds the api `parts` names with the stage it names, or `None` when
/// its own store cannot be reached.
///
/// The APIs this service creates share the account's api-id space and are invoked on the
/// same `execute-api` endpoint one of its stages is announced on, so an identifier naming
/// one of them names an endpoint the account really has.
fn own_api_holds_stage<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    parts: &wire::IdentifierParts,
) -> Option<bool> {
    let own = super::service_state(engine)?;
    let apis = own.lock();
    Some(
        apis.apis
            .get(&parts.rest_api_id)
            .is_some_and(|api| api.stages.contains_key(&parts.stage)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use ::service_com_amazonaws_apigateway_backplanecontrolservice::state as rest;

    /// The identifier of one endpoint of `rest_api_id`'s `stage`. The method and path take no
    /// part in where the documentation lives, so every row names the same pair.
    fn identified(rest_api_id: &str, stage: &str) -> wire::IdentifierParts {
        wire::IdentifierParts {
            method: "GET".to_string(),
            path: "/items".to_string(),
            rest_api_id: rest_api_id.to_string(),
            stage: stage.to_string(),
        }
    }

    /// RULE: a generation that has no REST endpoint to read reports the PAGE'S OWN failure —
    /// the class of the page's `RestEndpointIdentifier` validation failure and this service's
    /// one sentence for it — whichever part of the identifier did not resolve, and never a
    /// refusal of the internal lookup it made in the store that owns REST APIs; an endpoint
    /// that store does hold is generated with nothing to explain.
    ///
    /// Driven over the helper rather than over the wire because the STAGE branch is reached
    /// only where the account holds an api of one id in BOTH control planes (the admission the
    /// write is served on resolves the pair against either store), which no single request can
    /// arrange: the two stores mint ids from sequences of their own.
    #[test]
    fn a_generation_reports_the_pages_own_unresolved_identifier() {
        let engine = ::engine::InMemoryEngine::new();
        let held = "aaaa000001";
        {
            let owner = rest::service_state(&engine).expect("the REST control plane's store");
            let mut rest_apis = owner.lock();
            rest_apis.rest_apis.insert(
                held.to_string(),
                rest::RestApiEntry {
                    id: held.to_string(),
                    stages: [("prod".to_string(), rest::StageEntry::default())]
                        .into_iter()
                        .collect(),
                    ..rest::RestApiEntry::default()
                },
            );
        }

        // The rows span what the reading generalises over: the stage that service holds, a
        // stage name it holds none of, and an id it holds no REST API under at all. Both
        // failing rows report ONE exception — the page's own — rather than the sentence the
        // lookup inside the other store answers for its missing api or its missing stage.
        let cases: &[(&str, &str, &str, bool)] = &[
            ("the stage the REST API carries", held, "prod", false),
            (
                "a stage name that REST API has none of",
                held,
                "beta",
                true,
            ),
            (
                "an id no REST API of the account carries",
                "zzzz000009",
                "prod",
                true,
            ),
        ];
        for (case, rest_api_id, stage, fails) in cases {
            let answered =
                rest_endpoint_generation_failure(&engine, &identified(rest_api_id, stage));
            let reported = answered
                .as_ref()
                .map(|failure| (failure.exception.clone(), failure.message.clone()));
            let wanted = fails.then(|| {
                (
                    Some(BAD_REQUEST_EXCEPTION.to_string()),
                    Some(UNRESOLVED_REST_ENDPOINT_MESSAGE.to_string()),
                )
            });
            assert_eq!(
                reported, wanted,
                "case {case:?}: expected {wanted:?}, got {reported:?}"
            );
            // And nothing of the other store's own refusals travels in it: neither the
            // sentence it answers an unknown REST API identifier with — which interpolates the
            // account it answers for — nor the one it answers an unknown stage with.
            let message = reported
                .as_ref()
                .and_then(|(_exception, message)| message.clone())
                .unwrap_or_default();
            for internal in [
                rest::no_such_rest_api(rest_api_id),
                rest::NO_SUCH_STAGE.to_string(),
            ] {
                assert_ne!(
                    message, internal,
                    "case {case:?}: the page's exception must not carry the internal lookup's \
                     own refusal, got {message:?}"
                );
            }
        }
    }
}
