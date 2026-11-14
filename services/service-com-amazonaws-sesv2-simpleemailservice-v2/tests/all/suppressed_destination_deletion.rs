// SPDX-License-Identifier: CC0-1.0

//! Removing an address from a suppression list, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and serialization are
//! all exercised.
//!
//! The rules these cases pin, each over a SET of inputs rather than one recorded
//! request:
//!
//! 1. `DeleteSuppressedDestination` removes the entry for the address the request names
//!    from the suppression list the request selects — a tenant's when it carries
//!    `TenantName`, the account-level list when it omits it — so that list's read and
//!    listing no longer report the address, while every other list's entry for the same
//!    address, and every other address, is left exactly as it was;
//! 2. the removal is not idempotent: a list that holds no entry for the address is
//!    refused with a terminal 404 `NotFoundException` naming the address and the list
//!    that was read, and the refusal removes nothing;
//! 3. a `TenantName` outside its documented form is refused 400 `BadRequestException`
//!    and a well-formed name the account does not hold 404 `NotFoundException`, both
//!    before any list is touched.
//!
//! The tables span what those rules generalise over: both `SuppressionListReason`
//! values, both kinds of list, the spelling of the address, one address on two lists at
//! once, each way a removal can miss, and the `TenantName` values no list can be
//! selected by.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::SUPPRESSION_LIST_REASON_VALUES;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    suppression_key, ComAmazonawsSesv2SimpleemailserviceV2State, SuppressedDestinationRecord,
    MAX_TENANT_NAME_CHARS,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// A `TenantName` is bound to `@httpQuery`, and a case below carries one holding
// characters a query string gives its own meaning to, so the value is escaped the way an
// SDK escapes it, by the one escape this test binary shares.
use crate::contact_lists::query_escaped;

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the body reached by its path through the nested structures, or `Null`
    /// when any step is absent.
    fn at(&self, path: &[&str]) -> Value {
        let mut here = &self.body;
        for step in path {
            match here.get(step) {
                Some(next) => here = next,
                None => return Value::Null,
            }
        }
        here.clone()
    }

    /// A string at `path`, or `""` when the member is absent.
    fn text_at(&self, path: &[&str]) -> String {
        self.at(path).as_str().unwrap_or_default().to_string()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can seed the
/// suppression-list entries a removal works over.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// The query parameters that select a list: a tenant's when the case names one.
    fn selection(tenant: Option<&str>) -> String {
        match tenant {
            Some(tenant) => format!("?TenantName={}", query_escaped(tenant)),
            None => String::new(),
        }
    }

    /// `DeleteSuppressedDestination`'s route, from the operation's HTTP binding
    /// (`smithy_query(query="operation",operation="DeleteSuppressedDestination")`:
    /// `DELETE /v2/email/suppression/addresses/{EmailAddress}`), with the tenant whose
    /// list to remove from as the `TenantName` query parameter.
    async fn delete_suppressed(&self, address: &str, tenant: Option<&str>) -> Reply {
        self.call(
            "DELETE",
            &format!(
                "/v2/email/suppression/addresses/{address}{}",
                Self::selection(tenant)
            ),
            None,
        )
        .await
    }

    /// `GetSuppressedDestination`, the read a removal has to be visible through.
    async fn get_suppressed(&self, address: &str, tenant: Option<&str>) -> Reply {
        self.call(
            "GET",
            &format!(
                "/v2/email/suppression/addresses/{address}{}",
                Self::selection(tenant)
            ),
            None,
        )
        .await
    }

    /// `ListSuppressedDestinations` for the list the case selects.
    async fn list_suppressed(&self, tenant: Option<&str>) -> Reply {
        self.call(
            "GET",
            &format!(
                "/v2/email/suppression/addresses{}",
                Self::selection(tenant)
            ),
            None,
        )
        .await
    }

    /// `PutSuppressedDestination`: the address and the reason in the request body, plus
    /// the tenant whose list to write to when the case names one.
    async fn put_suppressed(&self, address: &str, reason: &str, tenant: Option<&str>) -> Reply {
        let mut body = json!({"EmailAddress": address, "Reason": reason});
        if let Some(tenant) = tenant {
            body["TenantName"] = json!(tenant);
        }
        self.call("PUT", "/v2/email/suppression/addresses", Some(body))
            .await
    }

    /// `CreateTenant`, so a case that targets a tenant's suppression list targets a
    /// tenant this account really holds.
    async fn create_tenants(&self, names: &[&str]) {
        for name in names {
            let created = self
                .call("POST", "/v2/email/tenants", Some(json!({"TenantName": name})))
                .await;
            assert_eq!(
                created.status, 200,
                "case {name:?}: expected the tenant to be created, got {} {:?}",
                created.status, created.body
            );
        }
    }

    /// Grant this account production access, which is what takes it out of the sandbox —
    /// the state `PutSuppressedDestination` requires to accept a write.
    fn grant_production_access(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        state.lock().account.production_access_enabled = true;
    }

    /// Persist one suppression-list entry through the one gated path a suppression write
    /// takes, so a case works over a list this account holds without the write's own
    /// restrictions deciding what is on it.
    fn seed(&self, record: &SuppressedDestinationRecord) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let stored = store.suppress_destination(
            suppression_key(record.tenant_name.as_deref(), &record.email_address),
            record.clone(),
        );
        assert!(
            stored,
            "case {:?}: expected the suppression-list entry to be stored",
            record.email_address
        );
    }

    /// Every stored entry, as the `(key, address, reason)` triples a case compares
    /// before and after a call to show what it changed.
    fn stored(&self) -> Vec<(String, String, String)> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .suppressed_destinations
            .iter()
            .map(|(key, record)| {
                (
                    key.clone(),
                    record.email_address.clone(),
                    record.reason.clone(),
                )
            })
            .collect()
    }
}

/// The two `SuppressionListReason` values the model declares, taken from the catalog
/// rather than spelled out here:
/// `smithy_query(query="shape",shape="SuppressionListReason")`.
fn modelled_reasons() -> (&'static str, &'static str) {
    match SUPPRESSION_LIST_REASON_VALUES {
        [first, second] => (*first, *second),
        other => panic!("the SuppressionListReason catalog changed shape: {other:?}"),
    }
}

/// An instant `seconds_ago` before now, truncated to whole seconds.
fn moment(seconds_ago: u64) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("now is after the epoch");
    SystemTime::UNIX_EPOCH + Duration::from_secs(now.as_secs() - seconds_ago)
}

/// A stored entry on the list `tenant` names, with the members a row does not vary left
/// unset.
fn entry(address: &str, reason: &str, tenant: Option<&str>) -> SuppressedDestinationRecord {
    SuppressedDestinationRecord {
        email_address: address.to_string(),
        reason: reason.to_string(),
        last_update_time: moment(120),
        message_id: None,
        feedback_id: None,
        tenant_name: tenant.map(str::to_string),
    }
}

/// The tenants every fixture of this suite names: one with each documented separator in
/// its name.
const TENANTS: [&str; 2] = ["tenant_one", "tenant-two"];

/// The entries every case of this suite works over.
///
/// The rows differ along the reason, the list the entry is on, and the spelling of the
/// address — including one address that is on two lists at once, which is what shows a
/// removal leaves the other list's entry alone.
fn fixture() -> Vec<SuppressedDestinationRecord> {
    let (bounce, complaint) = modelled_reasons();
    vec![
        entry("alpha@example.com", bounce, None),
        entry("bravo@example.com", complaint, None),
        entry("shared@example.com", bounce, None),
        entry("shared@example.com", complaint, Some("tenant_one")),
        entry("Mixed.Case@Example.COM", complaint, Some("tenant-two")),
    ]
}

/// The fixture seeded behind a fresh router, with every tenant it names created.
async fn seeded() -> Ses {
    let ses = Ses::new();
    ses.create_tenants(&TENANTS).await;
    for record in &fixture() {
        ses.seed(record);
    }
    ses
}

/// The addresses a listing reported.
fn listed(reply: &Reply) -> BTreeSet<String> {
    reply
        .at(&["SuppressedDestinationSummaries"])
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("EmailAddress")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How this service words the refusal of a lookup on a list that holds no entry for the
/// address: the list that was read, and the address taken from the request.
fn not_on_list(address: &str, tenant: Option<&str>) -> String {
    let list = match tenant {
        Some(_) => "tenant suppression list",
        None => "suppression list",
    };
    format!("Email address {address} does not exist on your {list}.")
}

/// One row of the removal table: the address the request spells, the list it selects, and
/// the stored entry — key and address — the removal has to take off that list.
struct RemovalCase {
    name: &'static str,
    address: &'static str,
    tenant: Option<&'static str>,
    /// The stored address the entry carries, which need not be the request's spelling.
    stored_address: &'static str,
}

#[tokio::test]
async fn a_removed_address_leaves_the_list_it_was_on_and_no_other() {
    let cases = [
        RemovalCase {
            name: "the account-level list, a bounce",
            address: "alpha@example.com",
            tenant: None,
            stored_address: "alpha@example.com",
        },
        RemovalCase {
            name: "the account-level list, a complaint",
            address: "bravo@example.com",
            tenant: None,
            stored_address: "bravo@example.com",
        },
        RemovalCase {
            name: "a tenant's list, an address the account list also holds",
            address: "shared@example.com",
            tenant: Some("tenant_one"),
            stored_address: "shared@example.com",
        },
        RemovalCase {
            name: "the account-level list, the same address once the tenant's is gone",
            address: "shared@example.com",
            tenant: None,
            stored_address: "shared@example.com",
        },
        RemovalCase {
            name: "a tenant's list, the domain spelled in another case",
            address: "Mixed.Case@EXAMPLE.com",
            tenant: Some("tenant-two"),
            stored_address: "Mixed.Case@Example.COM",
        },
    ];

    let ses = seeded().await;
    let mut left: Vec<(String, String, String)> = ses.stored();

    for case in &cases {
        let name = case.name;
        // The entry this row removes, as it is stored: the key names the list and the
        // address, so what must be gone afterwards is exactly this triple.
        let key = suppression_key(case.tenant, case.address);
        assert!(
            left.iter().any(|(stored, _, _)| *stored == key),
            "case {name:?}: expected the fixture to hold the entry about to be removed, got {left:?}"
        );

        let removed = ses.delete_suppressed(case.address, case.tenant).await;
        assert_eq!(
            (removed.status, removed.error_code().as_str()),
            (200, ""),
            "case {name:?}: expected 200 from the removal, got {} {:?}",
            removed.status,
            removed.body
        );

        // The list the request selected no longer reports the address, and the refusal
        // names that list and the address the request spelled.
        let read = ses.get_suppressed(case.address, case.tenant).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {name:?}: expected the read to answer 404 NotFoundException after the removal, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.message(),
            not_on_list(case.address, case.tenant),
            "case {name:?}: expected the refusal to read {:?}, got {:?}",
            not_on_list(case.address, case.tenant),
            read.message()
        );

        // Exactly that one entry left the table; every other entry, on this list and on
        // every other, is still stored unchanged.
        left.retain(|(stored, _, _)| *stored != key);
        assert_eq!(
            ses.stored(),
            left,
            "case {name:?}: expected the removal to take only {key:?} off, got {:?}",
            ses.stored()
        );

        // The listing of the list the removal targeted reports exactly what is left on
        // it, which is what a caller walking the list sees.
        let listing = ses.list_suppressed(case.tenant).await;
        let expected: BTreeSet<String> = left
            .iter()
            .filter(|(stored, address, _)| {
                *stored == suppression_key(case.tenant, address)
            })
            .map(|(_, address, _)| address.clone())
            .collect();
        assert_eq!(
            listed(&listing),
            expected,
            "case {name:?}: expected the listing to report exactly {expected:?}, got {:?}",
            listing.body
        );
        assert!(
            !expected.contains(case.stored_address),
            "case {name:?}: expected {:?} to be off the list it was removed from, got {expected:?}",
            case.stored_address
        );

        // Every entry still stored reads back on its own list, so no removal reached a
        // list it did not name.
        for (stored_key, address, _) in &left {
            for tenant in [None, Some("tenant_one"), Some("tenant-two")] {
                if suppression_key(tenant, address) != *stored_key {
                    continue;
                }
                let read = ses.get_suppressed(address, tenant).await;
                assert_eq!(
                    read.status, 200,
                    "case {name:?}: expected {address:?} on {tenant:?} to survive the removal, got {} {:?}",
                    read.status, read.body
                );
            }
        }
    }

    // Every fixture entry was removed by one of the rows, so every list is now empty.
    assert!(
        ses.stored().is_empty(),
        "case \"after every row\": expected no suppression-list entry to be left, got {:?}",
        ses.stored()
    );
}

/// One row of the refusal table: the removal that must miss, and the list it selects.
struct MissCase {
    name: &'static str,
    address: String,
    tenant: Option<&'static str>,
}

#[tokio::test]
async fn removing_an_address_the_selected_list_does_not_hold_is_refused() {
    let ses = seeded().await;
    // One entry is removed first, so a second removal of that same address is one of the
    // rows below: the removal is not idempotent.
    let gone = ses.delete_suppressed("alpha@example.com", None).await;
    assert_eq!(
        gone.status, 200,
        "case \"the removal that sets the rows up\": expected 200, got {} {:?}",
        gone.status, gone.body
    );
    let before = ses.stored();

    // The rows differ along why the selected list holds no entry for the address: it was
    // never suppressed anywhere, it is on the account's list and not the tenant's, it is
    // on a tenant's list and not the account's, it was removed a moment ago, its local
    // part is spelled in another case (the local part belongs to the receiving mailbox,
    // so it is matched exactly), and it carries a suffix.
    let cases = [
        MissCase {
            name: "an address no list holds",
            address: "nobody@example.com".to_string(),
            tenant: None,
        },
        MissCase {
            name: "an address on the account list, removed from a tenant's",
            address: "bravo@example.com".to_string(),
            tenant: Some("tenant_one"),
        },
        MissCase {
            name: "an address on a tenant's list, removed from the account's",
            address: "Mixed.Case@Example.COM".to_string(),
            tenant: None,
        },
        MissCase {
            name: "an address on one tenant's list, removed from another's",
            address: "Mixed.Case@Example.COM".to_string(),
            tenant: Some("tenant_one"),
        },
        MissCase {
            name: "an address removed a moment ago",
            address: "alpha@example.com".to_string(),
            tenant: None,
        },
        MissCase {
            name: "the local part spelled in another case",
            address: "BRAVO@example.com".to_string(),
            tenant: None,
        },
        MissCase {
            name: "a suppressed address with a suffix",
            address: "bravo@example.com.au".to_string(),
            tenant: None,
        },
        MissCase {
            name: "a tenant list holding only another address",
            address: "shared@example.com".to_string(),
            tenant: Some("tenant-two"),
        },
    ];

    for case in &cases {
        let name = case.name;
        let reply = ses.delete_suppressed(&case.address, case.tenant).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {name:?}: expected 404 NotFoundException, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.message(),
            not_on_list(&case.address, case.tenant),
            "case {name:?}: expected the refusal to read {:?}, got {:?}",
            not_on_list(&case.address, case.tenant),
            reply.message()
        );
        assert_eq!(
            ses.stored(),
            before,
            "case {name:?}: expected the refusal to remove nothing, got {:?}",
            ses.stored()
        );
    }
}

/// One row of the tenant table: the `TenantName` the request carries, and the refusal it
/// must draw.
struct TenantCase {
    name: &'static str,
    tenant: String,
    status: u16,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_tenant_the_account_does_not_hold_selects_no_list_to_remove_from() {
    // The documented `TenantName` form: "The name can contain up to 64 alphanumeric
    // characters, including letters, numbers, hyphens (-) and underscores (_) only"
    // (`smithy_query(query="shape",shape="TenantName")`), so `MAX_TENANT_NAME_CHARS`
    // characters is the last accepted length and one more the first refused one.
    let longest = "t".repeat(MAX_TENANT_NAME_CHARS);
    let too_long = "t".repeat(MAX_TENANT_NAME_CHARS + 1);
    let invalid = |tenant: &str| {
        format!(
            "Invalid tenant name: {tenant}. The name can contain up to \
             {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, \
             numbers, hyphens (-) and underscores (_) only."
        )
    };
    let absent = |tenant: &str| format!("The requested tenant <{tenant}> does not exist.");
    let cases = [
        TenantCase {
            name: "a well-formed tenant the account never created",
            tenant: "absent_tenant".to_string(),
            status: 404,
            code: "NotFoundException",
            message: absent("absent_tenant"),
        },
        TenantCase {
            name: "the longest well-formed name, never created",
            tenant: longest.clone(),
            status: 404,
            code: "NotFoundException",
            message: absent(&longest),
        },
        TenantCase {
            name: "a name one character past the documented maximum",
            tenant: too_long.clone(),
            status: 400,
            code: "BadRequestException",
            message: invalid(&too_long),
        },
        TenantCase {
            name: "a name carrying a character the form does not allow",
            tenant: "tenant.one".to_string(),
            status: 400,
            code: "BadRequestException",
            message: invalid("tenant.one"),
        },
        TenantCase {
            name: "a name carrying a space",
            tenant: "tenant one".to_string(),
            status: 400,
            code: "BadRequestException",
            message: invalid("tenant one"),
        },
    ];

    let ses = seeded().await;
    let before = ses.stored();
    for case in &cases {
        let name = case.name;
        // The address is one the ACCOUNT's list holds, so only the tenant the request
        // names can be what the refusal is about.
        let reply = ses
            .delete_suppressed("bravo@example.com", Some(&case.tenant))
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (case.status, case.code.to_string()),
            "case {name:?}: expected {} {}, got {} {} {:?}",
            case.status,
            case.code,
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.message(),
            case.message,
            "case {name:?}: expected the refusal to read {:?}, got {:?}",
            case.message,
            reply.message()
        );
        assert_eq!(
            ses.stored(),
            before,
            "case {name:?}: expected the refusal to remove nothing, got {:?}",
            ses.stored()
        );
    }

    // The control: the same address on a tenant list the account does hold is refused for
    // the list, not for the tenant, and the entry on the tenant's own list is removable.
    let on_held_tenant = ses
        .delete_suppressed("bravo@example.com", Some("tenant_one"))
        .await;
    assert_eq!(
        (on_held_tenant.status, on_held_tenant.message()),
        (404, not_on_list("bravo@example.com", Some("tenant_one"))),
        "case \"a tenant the account holds\": expected the list's own refusal, got {} {:?}",
        on_held_tenant.status,
        on_held_tenant.body
    );
    let held = ses
        .delete_suppressed("shared@example.com", Some("tenant_one"))
        .await;
    assert_eq!(
        (held.status, held.error_code().as_str()),
        (200, ""),
        "case \"a tenant the account holds\": expected the removal to be accepted, got {} {:?}",
        held.status,
        held.body
    );
}

#[tokio::test]
async fn an_address_written_through_the_api_can_be_removed_and_written_again() {
    let (bounce, complaint) = modelled_reasons();
    let ses = Ses::new();
    ses.create_tenants(&TENANTS).await;
    ses.grant_production_access();

    // The rows differ along the list the write targets, the reason it carries and the
    // spelling of the address, so each row drives the whole write → read → remove → read
    // cycle through the operations themselves.
    let cases: [(&str, &str, &str, Option<&str>); 4] = [
        (
            "the account list, a bounce",
            "written-bounce@example.com",
            bounce,
            None,
        ),
        (
            "the account list, a complaint",
            "Written.Complaint@Example.com",
            complaint,
            None,
        ),
        (
            "a tenant's list, a bounce",
            "written-tenant@example.com",
            bounce,
            Some("tenant_one"),
        ),
        (
            "another tenant's list, a complaint",
            "written-tenant@example.com",
            complaint,
            Some("tenant-two"),
        ),
    ];

    for (name, address, reason, tenant) in cases {
        let written = ses.put_suppressed(address, reason, tenant).await;
        assert_eq!(
            written.status, 200,
            "case {name:?}: expected the write to be accepted, got {} {:?}",
            written.status, written.body
        );
        let read = ses.get_suppressed(address, tenant).await;
        assert_eq!(
            (read.status, read.text_at(&["SuppressedDestination", "Reason"])),
            (200, reason.to_string()),
            "case {name:?}: expected the written entry to read back, got {} {:?}",
            read.status,
            read.body
        );

        let removed = ses.delete_suppressed(address, tenant).await;
        assert_eq!(
            (removed.status, removed.error_code().as_str()),
            (200, ""),
            "case {name:?}: expected 200 from the removal, got {} {:?}",
            removed.status,
            removed.body
        );
        let gone = ses.get_suppressed(address, tenant).await;
        assert_eq!(
            (gone.status, gone.message()),
            (404, not_on_list(address, tenant)),
            "case {name:?}: expected the read after the removal to refuse, got {} {:?}",
            gone.status,
            gone.body
        );

        // The address can be suppressed again afterwards, with the other reason: the
        // removal took the entry off the list and left nothing behind that blocks a
        // later write.
        let other = if reason == bounce { complaint } else { bounce };
        let again = ses.put_suppressed(address, other, tenant).await;
        assert_eq!(
            again.status, 200,
            "case {name:?}: expected the address to be writable again, got {} {:?}",
            again.status, again.body
        );
        let reread = ses.get_suppressed(address, tenant).await;
        assert_eq!(
            (
                reread.status,
                reread.text_at(&["SuppressedDestination", "Reason"])
            ),
            (200, other.to_string()),
            "case {name:?}: expected the rewritten entry to read back, got {} {:?}",
            reread.status,
            reread.body
        );
    }
}

#[tokio::test]
async fn an_account_in_the_sandbox_can_remove_an_address_from_its_list() {
    // Nothing here grants production access: the entries are seeded, so the account is
    // in the sandbox for every call below. Removing an address is not a write to a list
    // and sends no mail, so the restriction that bounds what reaches a list does not
    // bound what leaves one.
    let ses = seeded().await;
    for (address, tenant) in [
        ("alpha@example.com", None),
        ("shared@example.com", Some("tenant_one")),
        ("Mixed.Case@Example.COM", Some("tenant-two")),
    ] {
        let case = format!("{address} on {tenant:?}");
        let removed = ses.delete_suppressed(address, tenant).await;
        assert_eq!(
            (removed.status, removed.error_code().as_str()),
            (200, ""),
            "case {case:?}: expected the sandboxed account's removal to be accepted, got {} {:?}",
            removed.status,
            removed.body
        );
        let read = ses.get_suppressed(address, tenant).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected the address to be off the list, got {} {:?}",
            read.status,
            read.body
        );
    }
}
