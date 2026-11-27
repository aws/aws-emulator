// SPDX-License-Identifier: CC0-1.0

//! A distribution configuration may reference a TENANT PARAMETER only where it declares
//! one, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a multi-tenant distribution's configuration is a TEMPLATE — the
//! members that address the origin (`Origin$DomainName`, `Origin$OriginPath`) may name one of
//! the parameters its `TenantConfig$ParameterDefinitions` declares, and CloudFront
//! substitutes the value each tenant supplies
//! (`smithy_query(query="shape",shape="TenantConfig,ParameterDefinition,Origin")`). So a
//! configuration declaring `tenant-only` may reference exactly the names that list carries,
//! and one declaring `direct` — a standard distribution, which serves its own traffic rather
//! than a tenant's and has nothing substituted into it — may reference none at all. Either
//! way the create or update is refused `InvalidArgument` (HTTP 400) before anything is
//! stored: the standard distribution's refusal names the offending member, the multi-tenant
//! one names the list the parameter is missing from.
//!
//! The rows below span what the rule generalises over: which member carries the reference,
//! which mode the document declares (the member, and the member left out), whether any
//! definitions are declared at all, whether the referenced name is among them (including a
//! name differing from a declared one only in case, which `ParameterName`'s pattern
//! `^[a-zA-Z0-9-_]+$` admits as a name of its own), and how many references one member
//! carries. Every refusal is asserted to have left nothing behind, and every acceptance to
//! have stored the members verbatim.
//!
//! The rows also fix the PRECEDENCE between this rule and the one about the `TenantConfig`
//! member itself ([`crate::tenant_config_admission`]): a configuration declaring `direct`
//! that carries a `TenantConfig` AND references a parameter in a templated member is refused
//! for the templated MEMBER — the reference is what a client has to change — whatever the
//! definitions it also declares and however many references that member carries, while one
//! carrying the member with no reference anywhere is refused for the member.
//!
//! The wire harness is the lifecycle suite's and the configuration builder the
//! connection-mode suite's, reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    unresolvable_parameter_member_message, PARAMETER_NOT_DECLARED_MESSAGE,
    TENANT_CONFIG_ON_DIRECT_DISTRIBUTION_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_mode_immutable::{
    get_distribution_config, TenancyConfig, DIRECT, TENANT_ONLY,
};
use crate::distribution_lifecycle::{
    create_distribution, emulator, list_distributions, update_distribution, Answer,
};

/// What the service is expected to answer a row's configuration with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// Stored as submitted.
    Accepted,
    /// Refused naming the member that carries the reference, in the form the service states
    /// it (the model's member name with its first letter in lower case).
    Member(&'static str),
    /// Refused because the name is not among the configuration's parameter definitions.
    NotDeclared,
    /// Refused because a configuration that does not declare the tenant-only connection mode
    /// may carry no `TenantConfig` at all, which is what it is told once no templated member
    /// of it references a parameter.
    TenantConfigOnDirect,
}

/// One configuration under test.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    /// The `ConnectionMode` member, or `None` to leave it out entirely.
    connection_mode: Option<&'static str>,
    /// The names `TenantConfig$ParameterDefinitions` declares, or `None` for no
    /// `TenantConfig` at all.
    parameters: Option<Vec<&'static str>>,
    domain_name: &'static str,
    origin_path: Option<&'static str>,
    expected: Expectation,
}

impl Row {
    /// This row's document, under the caller reference its case names.
    fn config(&self) -> TenancyConfig {
        TenancyConfig {
            domain_name: self.domain_name.to_string(),
            origin_path: self.origin_path.map(str::to_string),
            parameters: self.parameters.clone(),
            connection_mode: self.connection_mode,
            ..TenancyConfig::new(&self.reference())
        }
    }

    /// The same document with no reference in either templated member: the configuration a
    /// distribution this row then UPDATES is created from, so the update's only possible
    /// refusal is the one under test.
    fn baseline(&self) -> TenancyConfig {
        TenancyConfig {
            // A configuration that does not declare the tenant-only mode may carry no
            // `TenantConfig` at all, so the baseline of such a row declares none either and
            // the update is what adds the member along with the references under test.
            parameters: (self.connection_mode == Some(TENANT_ONLY))
                .then(|| self.parameters.clone())
                .flatten(),
            connection_mode: self.connection_mode,
            ..TenancyConfig::new(&self.reference())
        }
    }

    /// A caller reference of this row's own, so the rows of one table never collide.
    fn reference(&self) -> String {
        format!("ref-{}", self.case.replace(' ', "-"))
    }
}

/// The table every assertion below runs over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "direct with a reference in the origin path",
            connection_mode: Some(DIRECT),
            parameters: None,
            domain_name: "origin.example.com",
            origin_path: Some("/{{TenantPath}}"),
            expected: Expectation::Member("originPath"),
        },
        Row {
            case: "direct with a reference in the origin domain name",
            connection_mode: Some(DIRECT),
            parameters: None,
            domain_name: "{{tenantName}}.example.com",
            origin_path: None,
            expected: Expectation::Member("domainName"),
        },
        Row {
            case: "direct with a reference its own definitions declare",
            connection_mode: Some(DIRECT),
            parameters: Some(vec!["TenantPath"]),
            domain_name: "origin.example.com",
            origin_path: Some("/{{TenantPath}}"),
            expected: Expectation::Member("originPath"),
        },
        Row {
            case: "direct with a declared reference in the origin domain name",
            connection_mode: Some(DIRECT),
            parameters: Some(vec!["tenantName"]),
            domain_name: "{{tenantName}}.example.com",
            origin_path: None,
            expected: Expectation::Member("domainName"),
        },
        Row {
            case: "direct with two declared references in one member",
            connection_mode: Some(DIRECT),
            parameters: Some(vec!["tenantName", "stage"]),
            domain_name: "origin.example.com",
            origin_path: Some("/{{tenantName}}/{{stage}}"),
            expected: Expectation::Member("originPath"),
        },
        Row {
            // Past the ceiling one member of a TEMPLATE may reference: no count of
            // references is admissible in a configuration nothing is substituted into, so
            // the member is still what the refusal names.
            case: "direct with three declared references in one member",
            connection_mode: Some(DIRECT),
            parameters: Some(vec!["tenantName", "stage", "region-label"]),
            domain_name: "origin.example.com",
            origin_path: Some("/{{tenantName}}/{{stage}}/{{region-label}}"),
            expected: Expectation::Member("originPath"),
        },
        Row {
            // The other side of the precedence: with no reference in either templated
            // member, the member a standard configuration may not carry is what is refused.
            case: "direct carrying definitions with no reference anywhere",
            connection_mode: Some(DIRECT),
            parameters: Some(vec!["TenantPath"]),
            domain_name: "origin.example.com",
            origin_path: Some("/static"),
            expected: Expectation::TenantConfigOnDirect,
        },
        Row {
            case: "no tenancy member with a reference in the origin path",
            connection_mode: None,
            parameters: None,
            domain_name: "origin.example.com",
            origin_path: Some("/{{TenantPath}}"),
            expected: Expectation::Member("originPath"),
        },
        Row {
            case: "tenant-only with no definitions at all",
            connection_mode: Some(TENANT_ONLY),
            parameters: None,
            domain_name: "origin.example.com",
            origin_path: Some("/{{TenantPath}}"),
            expected: Expectation::NotDeclared,
        },
        Row {
            case: "tenant-only with a reference outside its definitions",
            connection_mode: Some(TENANT_ONLY),
            parameters: Some(vec!["tenantName", "stage"]),
            domain_name: "origin.example.com",
            origin_path: Some("/{{TenantPath}}"),
            expected: Expectation::NotDeclared,
        },
        Row {
            case: "tenant-only with an undeclared reference in the domain name",
            connection_mode: Some(TENANT_ONLY),
            parameters: Some(vec!["tenantName"]),
            domain_name: "{{stage}}.example.com",
            origin_path: None,
            expected: Expectation::NotDeclared,
        },
        Row {
            case: "tenant-only with a reference differing from a definition only in case",
            connection_mode: Some(TENANT_ONLY),
            parameters: Some(vec!["tenantName"]),
            domain_name: "origin.example.com",
            origin_path: Some("/{{TenantName}}"),
            expected: Expectation::NotDeclared,
        },
        Row {
            case: "tenant-only referencing one of several definitions",
            connection_mode: Some(TENANT_ONLY),
            parameters: Some(vec!["tenantName", "stage", "region-label"]),
            domain_name: "origin.example.com",
            origin_path: Some("/{{stage}}/assets"),
            expected: Expectation::Accepted,
        },
        Row {
            case: "tenant-only referencing a definition in each templated member",
            connection_mode: Some(TENANT_ONLY),
            parameters: Some(vec!["tenantName", "stage"]),
            domain_name: "{{tenantName}}.example.com",
            origin_path: Some("/{{stage}}"),
            expected: Expectation::Accepted,
        },
        Row {
            case: "tenant-only with two references in one member",
            connection_mode: Some(TENANT_ONLY),
            parameters: Some(vec!["tenantName", "stage"]),
            domain_name: "origin.example.com",
            origin_path: Some("/{{tenantName}}/{{stage}}"),
            expected: Expectation::Accepted,
        },
        Row {
            case: "tenant-only with definitions no member references",
            connection_mode: Some(TENANT_ONLY),
            parameters: Some(vec!["tenantName"]),
            domain_name: "origin.example.com",
            origin_path: Some("/static"),
            expected: Expectation::Accepted,
        },
        Row {
            case: "direct with no reference anywhere",
            connection_mode: Some(DIRECT),
            parameters: None,
            domain_name: "origin.example.com",
            origin_path: Some("/static"),
            expected: Expectation::Accepted,
        },
    ]
}

/// Assert `answer` is the refusal `expected` states, under the one code both forms of it
/// travel under.
fn assert_refusal(answer: &Answer, case: &str, expected: Expectation) {
    let sentence = match expected {
        Expectation::Member(member) => unresolvable_parameter_member_message(member),
        Expectation::NotDeclared => PARAMETER_NOT_DECLARED_MESSAGE.to_string(),
        Expectation::TenantConfigOnDirect => {
            TENANT_CONFIG_ON_DIRECT_DISTRIBUTION_MESSAGE.to_string()
        }
        Expectation::Accepted => unreachable!("case {case}: an acceptance is not a refusal"),
    };
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidArgument",
        "case {case}: expected InvalidArgument, got {:?} in {}",
        answer.error_code(),
        answer.raw
    );
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, sentence,
        "case {case}: expected {sentence:?}, got {message:?}"
    );
}

/// Assert the configuration `id` names was stored exactly as `submitted` states its
/// templated members.
async fn assert_stored(router: &MultiServiceRouter, id: &str, case: &str, submitted: &TenancyConfig) {
    let read = get_distribution_config(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected HTTP 200 reading the configuration back, got {} with {}",
        read.status, read.raw
    );
    let origin = read
        .root()
        .at("Origins/Items/Origin")
        .unwrap_or_else(|| panic!("case {case}: the configuration carries no origin: {}", read.raw));
    assert_eq!(
        origin.text_at("DomainName"),
        Some(submitted.domain_name.as_str()),
        "case {case}: expected the submitted domain name, got {}",
        read.raw
    );
    assert_eq!(
        origin.text_at("OriginPath"),
        // A configuration that states no origin path runs with the empty one, which the
        // service resolves into the stored record — so that is what a read answers with.
        Some(submitted.origin_path.as_deref().unwrap_or_default()),
        "case {case}: expected the submitted origin path, got {}",
        read.raw
    );
}

/// `CreateDistribution` refuses every configuration that references a parameter it cannot
/// resolve, stores every one that does not, and leaves nothing behind for a refused one.
#[tokio::test]
async fn a_create_admits_only_the_parameter_references_a_configuration_declares() {
    let router = emulator();
    let mut accepted = 0usize;

    for row in rows() {
        let case = row.case;
        let submitted = row.config();
        let answer = create_distribution(&router, &submitted.xml()).await;

        if row.expected == Expectation::Accepted {
            assert_eq!(
                answer.status, 201,
                "case {case}: expected HTTP 201, got {} with {}",
                answer.status, answer.raw
            );
            let id = answer
                .root()
                .text_at("Id")
                .unwrap_or_default()
                .to_string();
            assert_stored(&router, &id, case, &submitted).await;
            accepted += 1;
            continue;
        }

        assert_refusal(&answer, case, row.expected);
    }

    // A refused create claims nothing: the store holds exactly the distributions the
    // accepted rows created.
    let page = list_distributions(&router, "").await;
    assert_eq!(
        page.root().text_at("Quantity"),
        Some(accepted.to_string().as_str()),
        "expected the store to hold only the {accepted} accepted configurations, got {}",
        page.raw
    );
}

/// `UpdateDistribution` applies the SAME rule: a configuration that references a parameter it
/// cannot resolve is refused, and the distribution keeps the configuration it had.
#[tokio::test]
async fn an_update_admits_only_the_parameter_references_a_configuration_declares() {
    for row in rows() {
        let case = row.case;
        let router = emulator();

        // The distribution is created from the row's own configuration with no reference in
        // either templated member, so it declares the row's connection mode and carries the
        // row's definitions: the only thing the update changes is the references.
        let baseline = row.baseline();
        let create = create_distribution(&router, &baseline.xml()).await;
        assert_eq!(
            create.status, 201,
            "case {case}: expected HTTP 201 from the baseline create, got {} with {}",
            create.status, create.raw
        );
        let id = create
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let etag = create.header("etag").to_string();

        let submitted = row.config();
        let answer = update_distribution(&router, &id, &submitted.xml(), Some(&etag)).await;

        if row.expected == Expectation::Accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            assert_stored(&router, &id, case, &submitted).await;
            continue;
        }

        assert_refusal(&answer, case, row.expected);

        // Nothing was written: the distribution still holds the baseline configuration at the
        // version the refused update matched on.
        assert_stored(&router, &id, case, &baseline).await;
        let read = get_distribution_config(&router, &id).await;
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the version token to be untouched, got {:?}",
            read.header("etag")
        );
    }
}
