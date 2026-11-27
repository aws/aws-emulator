// SPDX-License-Identifier: CC0-1.0

//! The SETTINGS a listener's mutual authentication configuration carries, driven over the
//! wire exactly as a client drives them.
//!
//! Two rules are asserted here, each over a SET of configurations rather than over one:
//!
//! * A write stores a listener's client certificate handling as the request configured it —
//!   every member `MutualAuthenticationAttributes` declares, not only the mode — so the
//!   write echoes each of them and a later read reports them all back, with the settings the
//!   request left out carried at the value the service carries them at.
//! * `CreateListener` "is idempotent, which means that it completes at most one time. If you
//!   attempt to create multiple listeners with the same settings, each call succeeds" — the
//!   client certificate handling is one of those settings, so a repeat on the same port
//!   differing in ANY ONE of its members is the modelled `DuplicateListenerException`, while
//!   a repeat carrying the same settings answers with the listener already there.
//!
//! The rows differ along what those rules generalise over: which member of the
//! configuration is set or differs, and whether that member is a string, a boolean, an
//! enumerated value or a list — and, for the list, in its length, its order and the members
//! of one entry.
//!
//! `MutualAuthenticationAttributes$TrustStoreAssociationStatus` is the one member no row
//! drives: it "[i]ndicates a shared trust stores association status", which the service
//! reports rather than a caller configures.

use crate::forward_action_config_and_mutual_authentication::describe_listener;
use crate::alb_lambda_target_published_address::element;
use crate::listener_rules::Form;
use crate::provisioning_loop::{
    action, call, emulator, first_text, members, p, seed_trust_store, Reply, TrustStoreSeed,
};
use crate::target_group_attributes_and_modification::create_target_group;
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `DuplicateListenerException`, which `CreateListener` models
/// (`smithy_query(query="operation",operation="CreateListener")`: `awsQueryError` code
/// `DuplicateListener`).
const DUPLICATE_LISTENER: &str = "DuplicateListener";

/// The client certificate handling method a listener nothing configured a mode on is under:
/// `MutualAuthenticationAttributes$Mode` is "[t]he client certificate handling method. The
/// default value is `off`."
const DEFAULT_MODE: &str = "off";

/// A certificate ARN a TLS listener's default certificate resolves to, so a row reaches the
/// mutual authentication assertions rather than the certificate refusal pinned in
/// `all/listener_tls_certificates.rs`.
const CERTIFICATE: &str = "arn:aws:acm:us-west-2:000000000000:certificate/mtls-settings";

/// One client certificate identity filter, by the four members
/// `ClientCertificateIdentityFilter` declares and requires
/// (`smithy_query(query="shape",shape="ClientCertificateIdentityFilter")`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Filter {
    /// `Field`: "The possible values are: subject-cn … subject-ou … san-dns … san-uri".
    field: &'static str,
    /// `Action`: "The only supported action is `accept`" — so no row differs in it.
    action: &'static str,
    /// `MatchType`: "The possible values are `exact` and `prefix`".
    match_type: &'static str,
    /// `Value`: "The value to match."
    value: &'static str,
}

/// The filters the rows below are built from, each naming a different `Field` and
/// `MatchType` so a table spans the documented values rather than one of them.
const SUBJECT_CN: Filter = Filter {
    field: "subject-cn",
    action: "accept",
    match_type: "exact",
    value: "client.example.com",
};
const SAN_DNS: Filter = Filter {
    field: "san-dns",
    action: "accept",
    match_type: "prefix",
    value: "edge.",
};
const SAN_URI: Filter = Filter {
    field: "san-uri",
    action: "accept",
    // The example the member's own documentation gives for this field.
    match_type: "exact",
    value: "spiffe://example.com/workload",
};
const SUBJECT_OU: Filter = Filter {
    field: "subject-ou",
    action: "accept",
    match_type: "prefix",
    value: "payments",
};

/// One mutual authentication configuration a request carries: each member it names, and the
/// filters it configures. A member left `None` is a member the request does not carry at
/// all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Configuration {
    mode: Option<&'static str>,
    /// Which of the suite's trust stores the configuration names, by index.
    trust_store: Option<usize>,
    ignore_client_certificate_expiry: Option<bool>,
    advertise_trust_store_ca_names: Option<&'static str>,
    filters: Vec<Filter>,
}

impl Configuration {
    /// The request fields this configuration travels as, under the member's own prefix.
    fn fields(&self, trust_stores: &[String]) -> Form {
        let mut form = Form::new();
        if let Some(mode) = self.mode {
            form.push(p("MutualAuthentication.Mode", mode));
        }
        if let Some(index) = self.trust_store {
            form.push(p("MutualAuthentication.TrustStoreArn", &trust_stores[index]));
        }
        if let Some(ignored) = self.ignore_client_certificate_expiry {
            form.push(p(
                "MutualAuthentication.IgnoreClientCertificateExpiry",
                if ignored { "true" } else { "false" },
            ));
        }
        if let Some(advertised) = self.advertise_trust_store_ca_names {
            form.push(p(
                "MutualAuthentication.AdvertiseTrustStoreCaNames",
                advertised,
            ));
        }
        for (index, filter) in self.filters.iter().enumerate() {
            let at = format!(
                "MutualAuthentication.ClientCertificateIdentityFilters.member.{}",
                index + 1
            );
            form.push(p(&format!("{at}.Field"), filter.field));
            form.push(p(&format!("{at}.Action"), filter.action));
            form.push(p(&format!("{at}.MatchType"), filter.match_type));
            form.push(p(&format!("{at}.Value"), filter.value));
        }
        form
    }
}

/// What one reply reports of a listener's client certificate handling: the block's presence
/// and each member of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Reported {
    present: bool,
    mode: Option<String>,
    trust_store_arn: Option<String>,
    ignore_client_certificate_expiry: Option<String>,
    advertise_trust_store_ca_names: Option<String>,
    filters: Vec<(String, String, String, String)>,
}

impl Reported {
    /// This block as one line, for the assertion message that names what was reported:
    /// every member of it, in the order the model declares them.
    fn line(&self) -> String {
        format!(
            "present={} Mode={:?} TrustStoreArn={:?} IgnoreClientCertificateExpiry={:?} \
             AdvertiseTrustStoreCaNames={:?} ClientCertificateIdentityFilters={:?}",
            self.present,
            self.mode,
            self.trust_store_arn,
            self.ignore_client_certificate_expiry,
            self.advertise_trust_store_ca_names,
            self.filters,
        )
    }
}

/// The mutual authentication the one listener of `body` is reported under.
fn reported(body: &str) -> Reported {
    let listeners = members(body, "Listeners");
    let listener = listeners.first().cloned().unwrap_or_default();
    if !listener.contains("<MutualAuthentication>") {
        return Reported::default();
    }
    let block = element(&listener, "MutualAuthentication");
    Reported {
        present: true,
        mode: first_text(&block, "Mode"),
        trust_store_arn: first_text(&block, "TrustStoreArn"),
        ignore_client_certificate_expiry: first_text(&block, "IgnoreClientCertificateExpiry"),
        advertise_trust_store_ca_names: first_text(&block, "AdvertiseTrustStoreCaNames"),
        filters: members(&block, "ClientCertificateIdentityFilters")
            .iter()
            .map(|filter| {
                (
                    first_text(filter, "Field").unwrap_or_default(),
                    first_text(filter, "Action").unwrap_or_default(),
                    first_text(filter, "MatchType").unwrap_or_default(),
                    first_text(filter, "Value").unwrap_or_default(),
                )
            })
            .collect(),
    }
}

/// What a configuration must be reported as: every member it carried, with the mode filled
/// in at the documented default when the request named none — which is what a listener the
/// member is bracketed to is EFFECTIVELY under.
fn expected(configuration: &Configuration, trust_stores: &[String], effective: bool) -> Reported {
    Reported {
        present: true,
        mode: configuration
            .mode
            .map(str::to_string)
            .or_else(|| effective.then(|| DEFAULT_MODE.to_string())),
        trust_store_arn: configuration.trust_store.map(|index| trust_stores[index].clone()),
        ignore_client_certificate_expiry: configuration
            .ignore_client_certificate_expiry
            .map(|ignored| ignored.to_string()),
        advertise_trust_store_ca_names: configuration
            .advertise_trust_store_ca_names
            .map(str::to_string),
        filters: configuration
            .filters
            .iter()
            .map(|filter| {
                (
                    filter.field.to_string(),
                    filter.action.to_string(),
                    filter.match_type.to_string(),
                    filter.value.to_string(),
                )
            })
            .collect(),
    }
}

/// A network load balancer across two subnets — the family whose TLS listener the model
/// brackets the client certificate identity filters to ("[Network Load Balancers] The client
/// certificate identity filters").
async fn network_load_balancer(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Type", "network"));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create the network load balancer {name}: expected 200, got {} {}",
        reply.status, reply.body
    );
    reply.text("LoadBalancerArn").unwrap_or_else(|| {
        panic!(
            "case create the network load balancer {name}: no ARN in {}",
            reply.body
        )
    })
}

/// A `CreateListener` request for a TLS listener of `load_balancer` on `port`, forwarding to
/// `target_group`, carrying `configuration`.
fn create_listener_form(
    load_balancer: &str,
    target_group: &str,
    port: i32,
    configuration: &Configuration,
    trust_stores: &[String],
) -> Form {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", load_balancer));
    form.push(p("Protocol", "TLS"));
    form.push(p("Port", &port.to_string()));
    form.push(p("Certificates.member.1.CertificateArn", CERTIFICATE));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", target_group));
    form.extend(configuration.fields(trust_stores));
    form
}

/// The two trust stores the configurations name, seeded so a configuration names a trust
/// store this account really holds.
fn trust_stores(engine: &std::sync::Arc<engine::InMemoryEngine>) -> Vec<String> {
    ["mtls-settings-one", "mtls-settings-two"]
        .iter()
        .enumerate()
        .map(|(index, name)| {
            seed_trust_store(
                engine,
                index,
                &TrustStoreSeed {
                    name,
                    certificates: 3,
                    revoked: 0,
                    settled: true,
                    tags: &[],
                },
            )
        })
        .collect()
}

/// A write stores every member of the configuration the request carried, and a read reports
/// them all back — the members the refreshed model declares beside the mode included.
#[tokio::test]
async fn every_mutual_authentication_setting_round_trips() {
    let (router, engine) = emulator();
    let stores = trust_stores(&engine);
    let target_group = create_target_group(
        &router,
        "mtls-settings-tg",
        &[("Protocol", "TCP"), ("Port", "443"), ("VpcId", "vpc-1")],
    )
    .await;
    let lb = network_load_balancer(&router, "mtls-settings-lb").await;

    // The rows differ in which members the configuration names and in how many filters it
    // carries: the mode alone, the whole configuration with one filter, the whole
    // configuration with several, and filters with no mode at all.
    let rows: Vec<(&str, Configuration)> = vec![
        (
            "the mode alone",
            Configuration {
                mode: Some("verify"),
                ..Configuration::default()
            },
        ),
        (
            "every member, with one filter",
            Configuration {
                mode: Some("verify"),
                trust_store: Some(0),
                ignore_client_certificate_expiry: Some(true),
                advertise_trust_store_ca_names: Some("on"),
                filters: vec![SUBJECT_CN],
            },
        ),
        (
            "every member, with three filters",
            Configuration {
                mode: Some("verify"),
                trust_store: Some(1),
                ignore_client_certificate_expiry: Some(false),
                advertise_trust_store_ca_names: Some("off"),
                filters: vec![SAN_DNS, SAN_URI, SUBJECT_OU],
            },
        ),
        (
            "filters with no mode of their own",
            Configuration {
                trust_store: Some(0),
                filters: vec![SUBJECT_OU],
                ..Configuration::default()
            },
        ),
    ];

    for (index, (case, configuration)) in rows.iter().enumerate() {
        let port = 9_000 + index as i32;
        let form = create_listener_form(&lb, &target_group, port, configuration, &stores);
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateListener, got {} {}",
            created.status, created.body
        );
        // A WRITE echoes the configuration the request carried.
        let echoed = reported(&created.body);
        assert_eq!(
            echoed,
            expected(configuration, &stores, false),
            "case {case}: CreateListener must echo the configuration it was given, got {}",
            echoed.line()
        );

        // And a READ reports the configuration the listener is effectively under, which
        // fills the mode in and leaves every other member as it was configured.
        let listener = first_text(&created.body, "ListenerArn")
            .unwrap_or_else(|| panic!("case {case}: no ListenerArn in {}", created.body));
        let read = describe_listener(&router, &listener).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from DescribeListeners, got {} {}",
            read.status, read.body
        );
        let stored = reported(&read.body);
        assert_eq!(
            stored,
            expected(configuration, &stores, true),
            "case {case}: DescribeListeners must report the stored configuration, got {}",
            stored.line()
        );
    }
}

/// One row of the repeat table: how the repeat's configuration differs from the one the
/// listener was created with, and whether that is the same settings.
struct RepeatRow {
    case: &'static str,
    configuration: Configuration,
    same_settings: bool,
}

/// Every member of the configuration is one of the settings a repeat is judged by: a repeat
/// differing in any one of them is the modelled duplicate, and one carrying the same
/// settings answers with the listener already there.
#[tokio::test]
async fn a_create_repeat_is_judged_by_every_mutual_authentication_setting() {
    let (router, engine) = emulator();
    let stores = trust_stores(&engine);
    let target_group = create_target_group(
        &router,
        "mtls-repeat-tg",
        &[("Protocol", "TCP"), ("Port", "443"), ("VpcId", "vpc-1")],
    )
    .await;
    let lb = network_load_balancer(&router, "mtls-repeat-lb").await;

    /// The configuration the listener under test is created with, which every row below
    /// repeats.
    fn base() -> Configuration {
        Configuration {
            mode: Some("verify"),
            trust_store: Some(0),
            ignore_client_certificate_expiry: Some(true),
            advertise_trust_store_ca_names: Some("on"),
            filters: vec![SUBJECT_CN, SAN_DNS],
        }
    }

    let port = 443;
    let created = call(
        &router,
        &create_listener_form(&lb, &target_group, port, &base(), &stores),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the first create: expected 200, got {} {}",
        created.status, created.body
    );
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the first create: no ListenerArn in {}", created.body));

    let rows: Vec<RepeatRow> = vec![
        RepeatRow {
            case: "the same configuration",
            configuration: base(),
            same_settings: true,
        },
        RepeatRow {
            case: "the same filters in the other order, which admit the same connections",
            configuration: Configuration {
                filters: vec![SAN_DNS, SUBJECT_CN],
                ..base()
            },
            same_settings: true,
        },
        RepeatRow {
            case: "a different mode",
            configuration: Configuration {
                mode: Some(DEFAULT_MODE),
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "a different trust store",
            configuration: Configuration {
                trust_store: Some(1),
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "a different expired-certificate handling",
            configuration: Configuration {
                ignore_client_certificate_expiry: Some(false),
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "no expired-certificate handling at all",
            configuration: Configuration {
                ignore_client_certificate_expiry: None,
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "a different CA name advertisement",
            configuration: Configuration {
                advertise_trust_store_ca_names: Some("off"),
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "one filter's field",
            configuration: Configuration {
                filters: vec![
                    Filter {
                        field: "subject-ou",
                        ..SUBJECT_CN
                    },
                    SAN_DNS,
                ],
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "one filter's match type",
            configuration: Configuration {
                filters: vec![
                    Filter {
                        match_type: "prefix",
                        ..SUBJECT_CN
                    },
                    SAN_DNS,
                ],
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "one filter's value",
            configuration: Configuration {
                filters: vec![
                    Filter {
                        value: "other.example.com",
                        ..SUBJECT_CN
                    },
                    SAN_DNS,
                ],
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "one filter fewer",
            configuration: Configuration {
                filters: vec![SUBJECT_CN],
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "one filter more",
            configuration: Configuration {
                filters: vec![SUBJECT_CN, SAN_DNS, SAN_URI],
                ..base()
            },
            same_settings: false,
        },
        RepeatRow {
            case: "no client certificate handling at all",
            configuration: Configuration::default(),
            same_settings: false,
        },
    ];

    for row in &rows {
        let repeat = call(
            &router,
            &create_listener_form(&lb, &target_group, port, &row.configuration, &stores),
        )
        .await;
        if row.same_settings {
            assert_eq!(
                (repeat.status, first_text(&repeat.body, "ListenerArn")),
                (200, Some(listener.clone())),
                "case {}: expected the stored listener, got {} {}",
                row.case,
                repeat.status,
                repeat.body
            );
        } else {
            assert_eq!(
                (repeat.status, repeat.error_code().as_str()),
                (400, DUPLICATE_LISTENER),
                "case {}: expected 400 {DUPLICATE_LISTENER}, got {} {}",
                row.case,
                repeat.status,
                repeat.body
            );
        }
        // However it was answered, the repeat left the listener under the configuration the
        // first create stored.
        let read = describe_listener(&router, &listener).await;
        let stored = reported(&read.body);
        assert_eq!(
            stored,
            expected(&base(), &stores, true),
            "case {}: the repeat must have changed nothing, got {}",
            row.case,
            stored.line()
        );
    }
}

/// A listener MOVED between the two protocols the member is bracketed to keeps its client
/// certificate handling, and one moved outside that bracket loses it — the bracket being
/// "[HTTPS and TLS listeners]" on both writes that set the member.
#[tokio::test]
async fn a_listener_keeps_its_client_certificate_handling_across_the_bracket() {
    let (router, engine) = emulator();
    let stores = trust_stores(&engine);
    let target_group = create_target_group(
        &router,
        "mtls-move-tg",
        &[("Protocol", "TCP"), ("Port", "443"), ("VpcId", "vpc-1")],
    )
    .await;
    let lb = network_load_balancer(&router, "mtls-move-lb").await;

    let configuration = Configuration {
        mode: Some("verify"),
        trust_store: Some(0),
        ignore_client_certificate_expiry: Some(true),
        advertise_trust_store_ca_names: Some("on"),
        filters: vec![SAN_URI],
    };
    let created = call(
        &router,
        &create_listener_form(&lb, &target_group, 8_443, &configuration, &stores),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the TLS listener: expected 200, got {} {}",
        created.status, created.body
    );
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the TLS listener: no ListenerArn in {}", created.body));

    // TLS to HTTPS: both are inside the bracket, so the configuration survives the move
    // whole.
    let moved = modify_protocol(&router, &listener, "HTTPS", true).await;
    assert_eq!(
        moved.status, 200,
        "case moved onto HTTPS: expected 200 from ModifyListener, got {} {}",
        moved.status, moved.body
    );
    let read = describe_listener(&router, &listener).await;
    let stored = reported(&read.body);
    assert_eq!(
        stored,
        expected(&configuration, &stores, true),
        "case moved onto HTTPS: the listener must keep its configuration, got {}",
        stored.line()
    );

    // And outside the bracket there is no client certificate handling to report at all.
    let moved = modify_protocol(&router, &listener, "TCP", false).await;
    assert_eq!(
        moved.status, 200,
        "case moved onto TCP: expected 200 from ModifyListener, got {} {}",
        moved.status, moved.body
    );
    let read = describe_listener(&router, &listener).await;
    let stored = reported(&read.body);
    assert_eq!(
        stored,
        Reported::default(),
        "case moved onto TCP: expected no mutual authentication, got {}",
        stored.line()
    );
}

/// Move `listener` onto `protocol`, naming a default certificate when the protocol it is
/// left on carries one.
async fn modify_protocol(
    router: &MultiServiceRouter,
    listener: &str,
    protocol: &str,
    certificate: bool,
) -> Reply {
    let mut form = action("ModifyListener");
    form.push(p("ListenerArn", listener));
    form.push(p("Protocol", protocol));
    if certificate {
        form.push(p("Certificates.member.1.CertificateArn", CERTIFICATE));
    }
    call(router, &form).await
}
