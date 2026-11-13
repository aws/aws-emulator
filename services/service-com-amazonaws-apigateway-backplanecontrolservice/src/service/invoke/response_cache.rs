// SPDX-License-Identifier: CC0-1.0

//! The STAGE RESPONSE CACHE of the `execute-api` data plane: what the method settings in
//! force on a call make of its caching, the entry the stage's cluster holds that call's
//! response under, and what a cache-control header asking for the entry to be reloaded
//! amounts to.
//!
//! Moved out of the parent module verbatim, as its own concern: the parent resolves and
//! serves a call, and this is the one place that decides whether a stage's cluster answers
//! it and under which key.

use super::*;

/// What the stage's method settings make of this call's CACHING.
#[derive(Debug)]
pub(super) enum Caching {
    /// Nothing: the method's settings do not cache this call, so it is served by
    /// performing the integration and nothing is held.
    Off,
    /// The stage caches this call, under this plan.
    On(CachePlan),
    /// The request asked for the held entry to be reloaded, the stage requires
    /// authorization for that ask, the caller holds none and the stage's strategy for such
    /// a request is to fail it: this is the refusal it is answered through
    /// ([`invalidation_refusal`]).
    Refused(GatewayRefusal),
}

/// What this call's caching amounts to: the entry the stage's cluster holds it under, how
/// long for, and what became of a cache-control header asking for that entry to be
/// reloaded.
#[derive(Debug)]
pub(super) struct CachePlan {
    /// The key the stage's cache holds this call's response under; see [`cache_key`].
    pub(super) key: String,
    /// `cacheTtlInSeconds`, as the duration an entry stored now is usable for.
    pub(super) ttl: ::std::time::Duration,
    /// Whether the held entry is NOT to be read: the request asked for it to be reloaded
    /// from the integration and the stage honoured the ask, so the integration answers and
    /// what it answers replaces the entry.
    pub(super) reload: bool,
    /// Whether the response is to state that an unauthorized cache-control header was
    /// ignored ([`STRATEGY_SUCCEED_WITH_RESPONSE_HEADER`]).
    pub(super) warn: bool,
}

/// How the stage's cache serves this call, from the stage's own settings and the
/// integration's own cache configuration.
///
/// `integration` is the LIVE method's integration, whose `cacheNamespace` and
/// `cacheKeyParameters` say which entry this call is: a method with no integration caches
/// nothing because there is nothing to cache. `request_path` is the path the request
/// actually addressed, which is what a `method.request.path.{name}` cache key parameter
/// reads its value out of — the deployed path TEMPLATE it matched, and the verb the method
/// is addressed by, are `call`'s own members.
///
/// `policy` is the api's own resource policy and `call` the call it decides, which together
/// say whether this caller may ask for the held entry to be reloaded
/// ([`resource_policy::authorizes_invalidation`]).
pub(super) fn cache_plan(
    stage: &StageEntry,
    integration: Option<&GetIntegrationOutput>,
    request_path: &str,
    req: &RtRequest,
    policy: Option<&str>,
    call: &resource_policy::Call<'_>,
) -> Caching {
    let matched_path = call.resource_path;
    let verb = call.verb;
    let Some(settings) = method_settings_in_force(stage, matched_path, verb) else {
        return Caching::Off;
    };
    // "Specifies whether responses should be cached and returned for requests. A cache
    // cluster must be enabled on the stage for responses to be cached"
    // (`MethodSetting$cachingEnabled`), so a method that asks for caching on a stage whose
    // cluster is not up is served from its integration every time — as it is while the
    // cluster is still coming up, being torn down, or being flushed.
    if !settings.caching_enabled
        || stage.cache_cluster_status() != crate::state::CACHE_CLUSTER_AVAILABLE
    {
        return Caching::Off;
    }
    // "Specifies the time to live (TTL), in seconds, for cached responses. The higher the
    // TTL, the longer the response will be cached": a TTL of zero holds a response for no
    // time at all, so nothing is cached.
    let Ok(ttl) = u64::try_from(settings.cache_ttl_in_seconds) else {
        return Caching::Off;
    };
    if ttl == 0 {
        return Caching::Off;
    }
    let Some(integration) = integration else {
        return Caching::Off;
    };

    // What the request asked about the entry the cluster holds, and what the stage's
    // settings make of the ask.
    let mut reload = false;
    let mut warn = false;
    if invalidation_asked(req) {
        if !settings.require_authorization_for_cache_control
            || resource_policy::authorizes_invalidation(policy, call)
        {
            reload = true;
        } else {
            match settings
                .unauthorized_cache_control_header_strategy
                .as_deref()
            {
                Some(STRATEGY_FAIL_WITH_403) => {
                    return Caching::Refused(invalidation_refusal(req, call));
                }
                Some(STRATEGY_SUCCEED_WITH_RESPONSE_HEADER) => warn = true,
                // `SUCCEED_WITHOUT_RESPONSE_HEADER`, and a stage that names no strategy at
                // all: the header is ignored and nothing is said about it.
                _ => {}
            }
        }
    }
    Caching::On(CachePlan {
        key: cache_key(integration, matched_path, request_path, verb, req),
        ttl: ::std::time::Duration::from_secs(ttl),
        reload,
        warn,
    })
}

/// How the endpoint refuses an invalidation ask the stage requires authorization for and
/// this caller does not hold, under the strategy that fails such a request
/// ([`STRATEGY_FAIL_WITH_403`]).
///
/// Which refusal that is follows the ONE rule this endpoint refuses every request needing an
/// authorization it cannot take by: a request carrying NO credential at all authenticated
/// nobody, so there is no principal for a policy to be read against and it is answered the
/// way an unsigned request to a method needing a credential is — the missing-token refusal,
/// `403` through [`MISSING_AUTHENTICATION_TOKEN`]. A request that DID present a credential
/// names a principal the api's policy decided against, which is an authorization failure: it
/// is answered through `ACCESS_DENIED`, carrying the sentence AWS refuses the action it asked
/// for with ([`resource_policy::invalidation_not_authorized`]).
fn invalidation_refusal(req: &RtRequest, call: &resource_policy::Call<'_>) -> GatewayRefusal {
    if carries_sigv4_credential(req) {
        return access_denied(resource_policy::invalidation_not_authorized(call));
    }
    MISSING_AUTHENTICATION_TOKEN
}

/// The method settings in force for the method `verb` on the deployed resource path
/// `matched_path` of this stage: the entry keyed by THAT method, else the one that names
/// every method of the stage, else none at all.
///
/// A `methodSettings` key is the method path a patch addressed —
/// `{resource_path}/{http_method}`, with the resource path's own `/` written as the JSON
/// Pointer escape ([`ESCAPED_PATH_SEPARATOR`]) where the pointer spelled the method path in
/// two reference tokens and decoded where it spelled it in one — or
/// [`EVERY_METHOD_SETTINGS_KEY`]. Both spellings of the method's own key are tried, the
/// escaped one first, and they are the only two `UpdateStage` can store: a pointer whose
/// resource half is no resource path names no method there, so no stored key can fail to
/// resolve here. The whole-stage key is the fallback, which is what makes a
/// `/*/*/caching/enabled` patch cache every method.
pub(super) fn method_settings_in_force<'s>(
    stage: &'s StageEntry,
    matched_path: &str,
    verb: &str,
) -> Option<&'s MethodSetting> {
    let verb = method_key(verb);
    let escaped = matched_path.replace('/', ESCAPED_PATH_SEPARATOR);
    [format!("{escaped}/{verb}"), format!("{matched_path}/{verb}")]
        .iter()
        .find_map(|key| stage.method_settings.get(key))
        .or_else(|| stage.method_settings.get(EVERY_METHOD_SETTINGS_KEY))
}

/// The key the stage's cache holds this call's response under.
///
/// `cacheNamespace` IS the group: it "specifies a group of related cached parameters. By
/// default, API Gateway uses the resource ID as the cacheNamespace. You can specify the
/// same cacheNamespace across resources to return the same cached data for requests to
/// different resources" — so the key names the NAMESPACE and not the resource, which is
/// what lets two resources share an entry. Nothing is lost by leaving the resource out:
/// `PutIntegration` defaults the member to the resource's own id, so two resources a
/// developer did not deliberately group carry two namespaces already. An integration stored
/// with no namespace at all falls back to the deployed resource path, which is the group of
/// one resource that default would have given it.
///
/// The verb follows — a namespace groups the resources a developer grouped, not the methods
/// on them — and then the value this request carries for each `cacheKeyParameters` entry ("a
/// list of request parameters whose values API Gateway caches"), in the order the integration
/// lists them. A parameter the request does not carry contributes its absence, so a request
/// carrying it and one that does not are two entries rather than one.
///
/// What is NOT in the key is anything the integration did not name: two requests differing
/// only in a query parameter, or in a path parameter, that is no cache key parameter are ONE
/// entry, which is the documented consequence of leaving it out.
pub(super) fn cache_key(
    integration: &GetIntegrationOutput,
    matched_path: &str,
    request_path: &str,
    verb: &str,
    req: &RtRequest,
) -> String {
    let namespace = integration
        .cache_namespace
        .as_deref()
        .filter(|namespace| !namespace.is_empty())
        .unwrap_or(matched_path);
    let mut key = format!("{namespace}|{verb}");
    let path_parameters = matched_path_parameters(matched_path, request_path);
    for parameter in integration.cache_key_parameters.iter().flatten() {
        let carried = match parameter_source(parameter) {
            Some((ParameterIn::Query, name)) => {
                ::service_common::query_parameter(query_of(req.uri()), name).map(str::to_string)
            }
            Some((ParameterIn::Header, name)) => req
                .headers()
                .get(name.to_ascii_lowercase())
                .map(str::to_string),
            Some((ParameterIn::Path, name)) => path_parameters.get(name).cloned(),
            // A `cacheKeyParameters` entry is required to be a request parameter of the
            // method, so an entry in any other form names no value: it contributes its
            // absence, which is the same for every request and so cannot make two calls
            // one entry.
            None => None,
        };
        key.push('|');
        key.push_str(parameter);
        key.push('=');
        key.push_str(carried.as_deref().unwrap_or_default());
    }
    key
}

/// The value each `{parameter}` segment of the matched resource path takes in the path this
/// request addressed.
///
/// A deployed resource path is matched by TEMPLATE ([`match_resource_path`]), so a
/// `method.request.path.{name}` source names a `{name}` segment of that template and its
/// value is the segment the REQUEST carried in that position; a greedy `{name+}` takes
/// every remaining segment, which is what makes it greedy.
///
/// `pub` because a mapping template reads the same binding through `$input.params(x)`, and
/// `TestInvokeMethod` binds the path a caller simulates against the resource's own template
/// the same way: one binding, so no two readers of a path parameter can disagree.
pub fn matched_path_parameters(matched_path: &str, request_path: &str) -> BTreeMap<String, String> {
    let declared = path_segments(matched_path);
    let sent = path_segments(request_path);
    let mut parameters = BTreeMap::new();
    for (position, segment) in declared.iter().enumerate() {
        let Some(name) = segment
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
        else {
            continue;
        };
        let greedy = name.ends_with('+');
        let name = name.trim_end_matches('+');
        let carried = if greedy {
            sent.get(position..).map(|rest| rest.join("/"))
        } else {
            sent.get(position).map(|segment| (*segment).to_string())
        };
        if let Some(carried) = carried {
            parameters.insert(name.to_string(), carried);
        }
    }
    parameters
}

/// Whether this request asks for the stage's held entry to be reloaded from the
/// integration.
///
/// The ask is the documented `Cache-Control: max-age=0` ([`CACHE_CONTROL_HEADER`]). A
/// `Cache-Control` value is a comma-separated list of directives, so the ask stands wherever
/// in the list it sits, and the directive's name is compared case-insensitively as HTTP
/// compares it.
pub(super) fn invalidation_asked(req: &RtRequest) -> bool {
    req.headers()
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(CACHE_CONTROL_HEADER))
        .flat_map(|(_, value)| value.split(','))
        .any(|directive| {
            let (name, value) = directive.split_once('=').unwrap_or((directive, ""));
            name.trim().eq_ignore_ascii_case(MAX_AGE_DIRECTIVE) && value.trim() == "0"
        })
}

/// The response a cache HIT serves: the response the integration produced, as it was
/// served.
///
/// Every header is re-inserted through the FALLIBLE form for the reason
/// [`served_response`] gives: the entry was built from a response this endpoint already
/// sent, so a value the wire cannot carry is skipped rather than failing the request.
pub(super) fn cached_response(held: CachedResponse) -> RtResponse {
    let mut response = response_with(held.status, &held.content_type, held.body);
    for (name, value) in held.headers {
        if let Err(refused) = response.headers_mut().try_insert(name.clone(), value) {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the cached response's header cannot be sent on the wire and is skipped"
            );
        }
    }
    response
}

/// What the stage's cache is to hold for the response `served`, due to expire a TTL from
/// now — or `None` for a response whose body is not readable as bytes, which is nothing a
/// later request could be served.
pub(super) fn cached_entry(served: &RtResponse, ttl: ::std::time::Duration) -> Option<CachedResponse> {
    let body = served.body().bytes()?.to_vec();
    let mut headers = BTreeMap::new();
    let mut content_type = JSON_CONTENT_TYPE.to_string();
    for (name, value) in served.headers().iter() {
        if name.eq_ignore_ascii_case("content-type") {
            content_type = value.to_string();
            continue;
        }
        // `content-length` is computed from the body when the entry is served again
        // ([`response_with`]), so holding the one this response carried would be a second
        // copy of a derived value.
        if name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        headers.insert(name.to_string(), value.to_string());
    }
    Some(CachedResponse {
        status: served.status().as_u16(),
        content_type,
        body,
        headers,
        expires_at: ::std::time::Instant::now() + ttl,
    })
}

/// `response`, stating that the request's cache-control header was ignored when the stage's
/// strategy for an unauthorized ask is to state it ([`IGNORED_CACHE_CONTROL_WARNING`]).
pub(super) fn warned(mut response: RtResponse, warn: bool) -> RtResponse {
    if warn {
        let _ = response
            .headers_mut()
            .try_insert(WARNING_HEADER, IGNORED_CACHE_CONTROL_WARNING.to_string());
    }
    response
}
