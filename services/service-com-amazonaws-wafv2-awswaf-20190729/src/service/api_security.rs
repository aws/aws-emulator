// SPDX-License-Identifier: CC0-1.0

//! WAF's API SECURITY analysis of the traffic a web ACL inspected: what one decided
//! request contributes to the events `ListEvents`, `ListEventsByWebACL`, `GetEvent`,
//! `ListEventActors` and `GetEventActor` report.
//!
//! # Where this runs, and on which traffic
//!
//! An event is what WAF made of the REQUESTS a web ACL saw, so this is written where
//! those requests are seen — the data plane's own bookkeeping phase
//! ([`super::edge::decide`]), under the same exclusive lock as the sample population and
//! the rate counts, and never on a read. Which traffic is analysed is the web ACL's
//! `AnalyticsConfig` ([`crate::state::analytics_enabled`]): security analytics is what
//! transfers and processes a web ACL's request logs, and the events are what that
//! processing produces.
//!
//! # What the detection is
//!
//! `ApiSecurityEventDetail$Label` publishes the form of a detection's label by example —
//! `awswaf:managed:aws:api-security:credential-stuffing-high` — and
//! `EventActorSummary$AuthFingerprint` what WAF reads off a request to recognise the
//! client behind one: "a hash of the authentication credentials that the client of the
//! event actor provides in the request. These can include a JSON Web Token (JWT), a JSON
//! Web Encryption (JWE) token, an Amazon Web Services Signature Version 4 (SigV4)
//! signature, or basic authentication credentials. You can use the fingerprint to match
//! the same client across events".
//!
//! Those two together are the detection implemented here, and the only one: CREDENTIAL
//! STUFFING, which from the request side is one API being offered many DISTINCT
//! credential sets. Every request that presents credentials contributes its fingerprint
//! to the resource it addressed; once the resource has been offered
//! [`crate::state::CREDENTIAL_STUFFING_FLOOR`] distinct ones the detection fires, and
//! the confidence rises with the count
//! ([`crate::state::CREDENTIAL_STUFFING_CONFIDENCE`]). A request presenting no
//! credentials is no part of such an attempt and contributes nothing.
//!
//! Nothing is invented about a request: the actor's address, the credentials it offered
//! and the API pattern it addressed are read off the request, and the attributes this
//! process cannot observe at all are left out (see [`crate::state::events`]).

use ::sha2::{Digest, Sha256};

use crate::ops::types::{EventActorChange, EventTimelineEntry};
use crate::state::{
    analytics_enabled, ApiPatternKey, ApiSecurityEventRecord, ComAmazonawsWafv2Awswaf20190729State,
    EventActorRecord, WebAclEntry, MAX_ACTOR_CHANGES, MAX_EVENT_ACTORS, MAX_EVENT_ACTORS_PER_SCOPE,
    MAX_EVENT_API_PATTERNS, MAX_EVENT_RECORDS_PER_WEB_ACL, MAX_EVENT_TIMELINE_ENTRIES,
};

use super::edge::InspectedRequest;

/// The header a client presents its authentication credentials in: every form
/// `EventActorSummary$AuthFingerprint` names — a JWT or JWE bearer token, a SigV4
/// signature, basic credentials — travels in it.
const AUTHORIZATION_HEADER: &str = "authorization";

/// The scheme token of an Amazon Web Services Signature Version 4 `Authorization`
/// header, whose credentials are the key id inside it rather than the whole value
/// ([`credential_fingerprint`]).
const SIGV4_SCHEME: &str = "AWS4-HMAC-SHA256";

/// How a SigV4 `Authorization` header names the key the request was signed with.
const SIGV4_CREDENTIAL_FIELD: &str = "Credential=";

/// The header a request states the host it addressed in, which is the domain and port
/// of the API pattern it belongs to (`ApiPattern$Domains`, `ApiPattern$Port`).
const HOST_HEADER: &str = "host";

/// Analyse one decided request, under the store's exclusive lock.
///
/// `protected_resource` is the ARN the web ACL was associated with, the resource whose
/// API the request addressed (`EventDetailSummary$ResourceArn`); `None` for a resource
/// that names its own web ACL instead, in which case the record belongs to the web ACL
/// alone.
///
/// Writes nothing for a web ACL whose security analytics is disabled, and nothing for a
/// request that presents no credentials: neither is traffic this detection is about.
pub fn observe(
    store: &mut ComAmazonawsWafv2Awswaf20190729State,
    entry: &WebAclEntry,
    protected_resource: Option<&str>,
    request: &InspectedRequest,
    now: i64,
) {
    if !analytics_enabled(&entry.scope, entry.web_acl.analytics_config.as_ref()) {
        return;
    }
    let Some(fingerprint) = credential_fingerprint(request) else {
        return;
    };
    let pattern = api_pattern_of(request);

    // The actor the credentials belong to: the same client across every event it
    // appears in, so an actor already seen under this fingerprint is updated rather
    // than minted again.
    let actor_id = recognise_actor(store, &entry.scope, &fingerprint, request, now);

    let key = (
        entry.scope.clone(),
        entry.web_acl.id.clone(),
        protected_resource.unwrap_or_default().to_string(),
    );
    if !store.api_security_events.contains_key(&key) {
        make_room_for_event_record(store, &entry.scope, &entry.web_acl.id);
        store.api_security_events.insert(
            key.clone(),
            ApiSecurityEventRecord {
                scope: entry.scope.clone(),
                web_acl_id: entry.web_acl.id.clone(),
                web_acl_arn: entry.web_acl.arn.clone(),
                resource_arn: protected_resource.map(str::to_string),
                api_patterns: Vec::new(),
                actors: Vec::new(),
                credentials_observed: 0,
                first_observed_millis: now,
                last_observed_millis: now,
                event_id: None,
                observed_since_reported: 0,
                timeline: Vec::new(),
            },
        );
    }
    let Some(record) = store.api_security_events.get_mut(&key) else {
        return;
    };

    record.last_observed_millis = now;
    if record.reported() {
        record.observed_since_reported = record.observed_since_reported.saturating_add(1);
    }

    // What the request ADDS to the record: a credential set it had not been offered
    // before, the actor that offered it, and the API pattern it addressed.
    let before = (record.confidence(), record.api_patterns.len());
    if !record.actors.contains(&actor_id) {
        record.credentials_observed = record.credentials_observed.saturating_add(1);
        if record.actors.len() < MAX_EVENT_ACTORS {
            record.actors.push(actor_id);
        }
    }
    if !record.api_patterns.contains(&pattern) && record.api_patterns.len() < MAX_EVENT_API_PATTERNS
    {
        record.api_patterns.push(pattern);
    }

    // The detection, and the timeline that records every change to it: an event is
    // reported from the first instant the floor is reached, and a later change to its
    // label, its confidence or its API patterns is one more timeline entry ("the changes
    // to the event's label, confidence, and API patterns over time, ordered from oldest
    // to newest").
    let after = (record.confidence(), record.api_patterns.len());
    if record.event_id.is_none() && record.confidence().is_some() {
        record.event_id = Some(crate::state::new_uuid());
        record.observed_since_reported = 0;
        append_timeline_entry(record, now);
    } else if record.reported() && after != before {
        append_timeline_entry(record, now);
    }
}

/// Record what the event's detection says NOW as one more timeline entry.
fn append_timeline_entry(record: &mut ApiSecurityEventRecord, now: i64) {
    let entry = EventTimelineEntry {
        change_time: Some(::aws_smithy_types::DateTime::from_millis(now)),
        label: record.label(),
        confidence: record.confidence().map(str::to_string),
        api_patterns: Some(
            record
                .api_patterns
                .iter()
                .map(ApiPatternKey::reported)
                .collect(),
        ),
    };
    record.timeline.push(entry);
    if record.timeline.len() > MAX_EVENT_TIMELINE_ENTRIES {
        let overflow = record.timeline.len() - MAX_EVENT_TIMELINE_ENTRIES;
        record.timeline.drain(..overflow);
    }
}

/// The id of the actor whose credentials `fingerprint` is, minting one for a client
/// never seen before.
///
/// An actor already known under that fingerprint is the SAME client, so its latest
/// observed attributes are updated — and a change to its network attributes is recorded
/// as the `EventActorChange` the model carries for exactly that ("a change to the network
/// attributes of the event actor"), ordered oldest to newest.
fn recognise_actor(
    store: &mut ComAmazonawsWafv2Awswaf20190729State,
    scope: &str,
    fingerprint: &str,
    request: &InspectedRequest,
    now: i64,
) -> String {
    let index_key = (scope.to_string(), fingerprint.to_string());
    if let Some(actor_id) = store.event_actor_ids.get(&index_key).cloned() {
        if let Some(actor) = store
            .event_actors
            .get_mut(&(scope.to_string(), actor_id.clone()))
        {
            actor.last_observed_millis = now;
            if actor.client_ip != request.client_ip {
                actor.client_ip = request.client_ip.clone();
                actor.changes.push(EventActorChange {
                    change_time: Some(::aws_smithy_types::DateTime::from_millis(now)),
                    client_ip: Some(request.client_ip.clone()),
                    // The attributes nothing here can observe; see the events module.
                    tls_fingerprint: None,
                    asn: None,
                    asn_organization: None,
                    country: None,
                });
                if actor.changes.len() > MAX_ACTOR_CHANGES {
                    let overflow = actor.changes.len() - MAX_ACTOR_CHANGES;
                    actor.changes.drain(..overflow);
                }
            }
            return actor_id;
        }
    }
    let actor_id = crate::state::new_uuid();
    make_room_for_actor(store, scope);
    store.event_actors.insert(
        (scope.to_string(), actor_id.clone()),
        EventActorRecord {
            event_actor_id: actor_id.clone(),
            auth_fingerprint: fingerprint.to_string(),
            client_ip: request.client_ip.clone(),
            created_millis: now,
            last_observed_millis: now,
            changes: Vec::new(),
        },
    );
    store.event_actor_ids.insert(index_key, actor_id.clone());
    actor_id
}

/// Make room, if there is none, for one more actor of `scope`.
///
/// How many actors one scope holds is this side's choice
/// ([`MAX_EVENT_ACTORS_PER_SCOPE`]): the actor whose last observation is oldest is
/// released, and with it every reference to it an event carried — an actor exists to be
/// resolved from an event, so an event must not name one no report can resolve. What the
/// event saw is not forgotten: the count the detection is judged on
/// ([`ApiSecurityEventRecord::credentials_observed`]) is a count of credential sets
/// offered, not of actors still held.
fn make_room_for_actor(store: &mut ComAmazonawsWafv2Awswaf20190729State, scope: &str) {
    let mut held: Vec<(String, i64)> = store
        .event_actors
        .iter()
        .filter(|((held_scope, _), _)| held_scope == scope)
        .map(|((_, actor_id), record)| (actor_id.clone(), record.last_observed_millis))
        .collect();
    if held.len() < MAX_EVENT_ACTORS_PER_SCOPE {
        return;
    }
    held.sort_by_key(|(_, last_observed)| *last_observed);
    let evicting = held.len() + 1 - MAX_EVENT_ACTORS_PER_SCOPE;
    for (actor_id, _) in held.into_iter().take(evicting) {
        store
            .event_actors
            .remove(&(scope.to_string(), actor_id.clone()));
        store
            .event_actor_ids
            .retain(|(held_scope, _), held| held_scope != scope || *held != actor_id);
        for record in store
            .api_security_events
            .values_mut()
            .filter(|record| record.scope == scope)
        {
            record.actors.retain(|held| *held != actor_id);
        }
    }
}

/// Make room, if there is none, for one more event record of the web ACL `web_acl_id`.
///
/// How many records one web ACL holds is this side's choice
/// ([`MAX_EVENT_RECORDS_PER_WEB_ACL`]): the record whose last observation is oldest is
/// evicted, together with the actors no remaining record is associated with — an actor
/// exists to be resolved from an event, so one no event references any more is released
/// with it.
fn make_room_for_event_record(
    store: &mut ComAmazonawsWafv2Awswaf20190729State,
    scope: &str,
    web_acl_id: &str,
) {
    let mut held: Vec<((String, String, String), i64)> = store
        .api_security_events
        .iter()
        .filter(|((held_scope, held_web_acl, _), _)| {
            held_scope == scope && held_web_acl == web_acl_id
        })
        .map(|(key, record)| (key.clone(), record.last_observed_millis))
        .collect();
    if held.len() < MAX_EVENT_RECORDS_PER_WEB_ACL {
        return;
    }
    held.sort_by_key(|(_, last_observed)| *last_observed);
    let evicting = held.len() + 1 - MAX_EVENT_RECORDS_PER_WEB_ACL;
    for (key, _) in held.into_iter().take(evicting) {
        store.api_security_events.remove(&key);
    }
    release_unreferenced_actors(store, scope);
}

/// Drop the actors of `scope` no event record is associated with any more.
fn release_unreferenced_actors(store: &mut ComAmazonawsWafv2Awswaf20190729State, scope: &str) {
    let referenced: ::std::collections::BTreeSet<String> = store
        .api_security_events
        .values()
        .filter(|record| record.scope == scope)
        .flat_map(|record| record.actors.iter().cloned())
        .collect();
    store
        .event_actors
        .retain(|(held_scope, actor_id), _| held_scope != scope || referenced.contains(actor_id));
    store.event_actor_ids.retain(|(held_scope, _), actor_id| {
        held_scope != scope || referenced.contains(actor_id)
    });
}

/// The fingerprint of the credentials `request` presents, or `None` when it presents
/// none.
///
/// A SigV4 request is signed afresh for every call — the signature changes while the
/// client does not — so what is hashed for one is the key id the header names
/// (`Credential=<key id>/<scope>`), which is the client's own credential; every other
/// form (a bearer JWT or JWE, basic credentials) is the credential itself, so the whole
/// header value is hashed. Either way the fingerprint is what "match[es] the same client
/// across events" (`EventActorSummary$AuthFingerprint`).
///
/// The hash is SHA-256, hex-rendered, through the same maintained digest this crate
/// draws every other hash from; the credentials themselves are never stored.
pub fn credential_fingerprint(request: &InspectedRequest) -> Option<String> {
    let presented = header_of(request, AUTHORIZATION_HEADER)?.trim();
    if presented.is_empty() {
        return None;
    }
    let credentials = match presented.strip_prefix(SIGV4_SCHEME) {
        Some(signed) => signed
            .split(',')
            .find_map(|field| field.trim().strip_prefix(SIGV4_CREDENTIAL_FIELD))
            .map(|credential| credential.split('/').next().unwrap_or(credential))
            .unwrap_or(presented),
        None => presented,
    };
    let mut hasher = Sha256::new();
    hasher.update(credentials.as_bytes());
    Some(::hex::encode(hasher.finalize()))
}

/// The API pattern `request` addressed, as `ApiPattern` reports one.
///
/// The method and the normalized path come from the request line
/// ([`crate::state::normalized_path_pattern`]); the domain and the port come from the
/// host it addressed, and a request that addressed none carries no domain and the port
/// the member defaults to.
pub fn api_pattern_of(request: &InspectedRequest) -> ApiPatternKey {
    let host = header_of(request, HOST_HEADER).unwrap_or_default();
    let (domain, port) = match host.rsplit_once(':') {
        Some((domain, port)) => (domain, port.parse::<i32>().unwrap_or_default()),
        None => (host, 0),
    };
    ApiPatternKey {
        method: request.method.to_ascii_uppercase(),
        path_pattern: crate::state::normalized_path_pattern(&request.uri_path),
        port,
        domains: if domain.is_empty() {
            Vec::new()
        } else {
            vec![domain.to_string()]
        },
    }
}

/// The first value of the header `name` (compared case-insensitively, as HTTP header
/// names are), or `None` when the request carries none.
fn header_of<'r>(request: &'r InspectedRequest, name: &str) -> Option<&'r str> {
    request
        .headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}
