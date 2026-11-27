// SPDX-License-Identifier: CC0-1.0

//! The tenant PARAMETER references a distribution configuration carries, and the rule they
//! are admitted by.
//!
//! A multi-tenant distribution is a template: the members of its configuration that address
//! the origin may name one of the distribution's own parameters, and CloudFront substitutes
//! the value each tenant supplies for it when it serves that tenant. The parameters a
//! configuration may name are exactly the ones its `TenantConfig$ParameterDefinitions`
//! declares — "a valid parameter value must exist for any parameter that is marked as
//! required in the multi-tenant distribution", and a tenant supplies values only for the
//! parameters that list defines ([`parameter_refusal`] is the same rule seen from the
//! tenant's side). A configuration declaring the STANDARD connection mode serves its own
//! traffic rather than a tenant's, so nothing is ever substituted into it and nothing in it
//! may reference a parameter.
//!
//! So a configuration carrying a reference no definition declares describes an origin
//! CloudFront could never resolve, and is refused before it is stored — on every path that
//! stores a `DistributionConfig`, so no stored configuration carries an unresolvable
//! reference.

use super::*;

/// What opens a tenant parameter reference inside a configuration member.
const REFERENCE_OPEN: &str = "{{";

/// What closes one.
const REFERENCE_CLOSE: &str = "}}";

/// The configuration members a tenant parameter reference may appear in, each named as the
/// service states it in the refusal below.
///
/// Both are members of `Origin`, the part of the configuration that says WHERE the content
/// comes from: `DomainName` is "the domain name for the origin" and `OriginPath` "an
/// optional path that CloudFront appends to the origin domain name when CloudFront requests
/// content from the origin" — the two values a tenant's own content is addressed by, and so
/// the two a multi-tenant distribution parameterises. The name each is stated by is the
/// model's member name with its first letter in lower case, which is how the service words
/// it on the wire.
const DOMAIN_NAME_MEMBER: &str = "domainName";

/// `Origin$OriginPath`, as [`DOMAIN_NAME_MEMBER`] explains.
const ORIGIN_PATH_MEMBER: &str = "originPath";

/// Every tenant parameter reference `value` carries, in the order it states them.
///
/// A reference is a parameter name between [`REFERENCE_OPEN`] and [`REFERENCE_CLOSE`]; text
/// that opens a reference and never closes it names nothing, so it carries none. Nothing is
/// indexed or sliced by a computed offset: the walk is `split_once` over the remainder.
pub fn parameter_references(value: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = value;
    while let Some((_, after_open)) = rest.split_once(REFERENCE_OPEN) {
        match after_open.split_once(REFERENCE_CLOSE) {
            Some((name, after_close)) => {
                found.push(name);
                rest = after_close;
            }
            None => break,
        }
    }
    found
}

/// The parameter names a configuration's `TenantConfig$ParameterDefinitions` declares.
fn declared_parameters(config: &DistributionConfig) -> BTreeSet<&str> {
    config
        .tenant_config
        .as_ref()
        .and_then(|tenant_config| tenant_config.parameter_definitions.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|definition| definition.name.as_str())
        .collect()
}

/// Why a submitted `DistributionConfig` references a tenant parameter it cannot resolve, or
/// `None` when every reference it carries names a parameter the configuration declares.
///
/// One rule, read off the connection mode the configuration DECLARES
/// ([`connection_mode_declared`]) — which is what says whether the configuration is a
/// template at all:
///
/// * a configuration declaring `direct` describes a standard distribution, which serves its
///   own traffic rather than a tenant's and has no parameter values substituted into it, so
///   any reference in one of its templated members is invalid and the refusal names that
///   member ([`unresolvable_parameter_member_message`]) — whatever definitions the
///   configuration may also carry, since none of them is ever applied to it, and however
///   many references the member carries, since no count of them is admissible;
/// * a configuration declaring `tenant-only` may reference the parameters its
///   `TenantConfig$ParameterDefinitions` declares and no others, so a reference to a name
///   that list does not carry — including a configuration that declares no definitions at
///   all — is refused with the sentence that states which list the name is missing from
///   ([`PARAMETER_NOT_DECLARED_MESSAGE`]).
///
/// The members walked are the two templated ones ([`DOMAIN_NAME_MEMBER`],
/// [`ORIGIN_PATH_MEMBER`]), read in the order the configuration states its origins, so the
/// refusal names the FIRST member that breaks the rule. Checked on the way IN, on every path
/// that stores a `DistributionConfig`.
pub fn tenant_parameter_refusal(config: &DistributionConfig) -> Option<String> {
    let multi_tenant = connection_mode_declared(config) == CONNECTION_MODE_TENANT_ONLY;
    let declared = declared_parameters(config);
    for origin in &config.origins.items {
        let members = [
            (DOMAIN_NAME_MEMBER, origin.domain_name.as_str()),
            (
                ORIGIN_PATH_MEMBER,
                origin.origin_path.as_deref().unwrap_or_default(),
            ),
        ];
        for (member, value) in members {
            let references = parameter_references(value);
            if references.is_empty() {
                continue;
            }
            // Nothing is ever substituted into a configuration that serves its own traffic,
            // so a reference in one of ITS members names a value that cannot exist: it is
            // refused by the member that carries it, before anything about how many
            // references that member states or which names a template would resolve — both
            // of which are properties of a TEMPLATE, and this configuration is none.
            if !multi_tenant {
                return Some(unresolvable_parameter_member_message(member));
            }
            // How MANY parameters one member of a template references is a property of that
            // member alone, so it is answered before the names are resolved: a field past
            // the ceiling is refused whether or not every name in it is declared.
            if references.len() > MAX_PARAMETERS_PER_FIELD {
                return Some(TOO_MANY_PARAMETERS_PER_FIELD_MESSAGE.to_string());
            }
            for name in references {
                if !declared.contains(name) {
                    return Some(PARAMETER_NOT_DECLARED_MESSAGE.to_string());
                }
            }
        }
    }
    None
}

/// The most tenant parameters ONE member of a configuration may reference.
///
/// RECORDED behaviour, which is where this number comes from: the service creates a
/// multi-tenant distribution whose `OriginPath` references exactly this many of its declared
/// parameters and reports the path back unchanged, and refuses one referencing one more —
/// "your request exceeds the maximum number of parameters per field". CloudFront publishes no
/// figure for it, so it is the recording that fixes the bound here.
pub const MAX_PARAMETERS_PER_FIELD: usize = 2;

/// The sentence CloudFront refuses a configuration whose member references more parameters
/// than [`MAX_PARAMETERS_PER_FIELD`] with.
///
/// FIXED text: it states the ceiling the field broke and what to do about it, and names
/// neither the member nor the value that carried the references. Recorded behaviour.
pub const TOO_MANY_PARAMETERS_PER_FIELD_MESSAGE: &str =
    "Your request exceeds the maximum number of parameters per field. Specify fewer parameters \
     and try again.";

/// Why the `TenantConfig` of a submitted `DistributionConfig` is one CloudFront would not
/// store, or `None` when the configuration carries none or carries a usable one.
///
/// Two rules about the member itself, read AFTER the references its definitions would admit
/// ([`tenant_parameter_refusal`]) are resolved — of a configuration that both carries the
/// member and references a parameter in a templated one, the reference is what the service
/// answers:
///
/// * `TenantConfig` "only supports multi-tenant distributions. You can't specify this field
///   for standard distributions", so only a configuration DECLARING the tenant-only
///   connection mode may carry it. What declares that is the `ConnectionMode` MEMBER — a
///   configuration that states `direct`, states another mode or states none at all describes
///   a standard distribution, which is what "a direct distribution" is in the sentence the
///   refusal states ([`TENANT_CONFIG_ON_DIRECT_DISTRIBUTION_MESSAGE`]);
/// * a parameter definition's `Definition` is "the value that you assigned to the parameter",
///   and `StringSchemaConfig` is the schema that value is admitted by — so a definition has to
///   state a schema at all, and a schema that marks the parameter `Required` ("whether the
///   defined parameter is required") states no `DefaultValue` for it: a default would supply
///   the value every tenant is required to supply itself, so the two together describe no
///   admission rule. Either way the whole list is refused with the one sentence the service
///   states for it ([`INVALID_PARAMETER_DEFINITION_SCHEMAS_MESSAGE`]).
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, so no stored
/// configuration carries a `TenantConfig` the distribution it describes could not have.
pub fn tenant_config_refusal(config: &DistributionConfig) -> Option<String> {
    let tenant_config = config.tenant_config.as_ref()?;
    if config.connection_mode.as_deref() != Some(CONNECTION_MODE_TENANT_ONLY) {
        return Some(TENANT_CONFIG_ON_DIRECT_DISTRIBUTION_MESSAGE.to_string());
    }
    tenant_config
        .parameter_definitions
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(unusable_parameter_definition)
        .then(|| INVALID_PARAMETER_DEFINITION_SCHEMAS_MESSAGE.to_string())
}

/// Whether `definition` states no schema a tenant's value for that parameter could be
/// admitted by ([`tenant_config_refusal`] states the rule).
fn unusable_parameter_definition(definition: &ParameterDefinition) -> bool {
    match definition.definition.string_schema.as_ref() {
        None => true,
        Some(schema) => {
            schema.required
                && schema
                    .default_value
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
        }
    }
}

/// The sentence CloudFront refuses a `TenantConfig` on a configuration that does not declare
/// the tenant-only connection mode with: the fixed text names the member that may not be
/// there and the kind of distribution it may not be there for. Recorded behaviour.
pub const TENANT_CONFIG_ON_DIRECT_DISTRIBUTION_MESSAGE: &str =
    "The parameter TenantConfig cannot be set for a direct distribution.";

/// The sentence CloudFront refuses a parameter definition list carrying a schema it cannot
/// admit values by with.
///
/// FIXED text: it states the condition over the whole list and what to review, so neither the
/// parameter that broke it nor anything else of the request belongs in it. Recorded behaviour.
pub const INVALID_PARAMETER_DEFINITION_SCHEMAS_MESSAGE: &str =
    "One or more parameter definition schemas aren't valid. Review the parameters for the \
     multi-tenant distribution and try again.";

/// The sentence CloudFront refuses a configuration whose member `member` references a
/// parameter that configuration cannot resolve with, on the wire.
///
/// The member is the one variable part of the text — it is what the caller has to change —
/// and the value that carried the reference is never echoed back.
pub fn unresolvable_parameter_member_message(member: &str) -> String {
    format!("The parameter {member} contains one or more parameters that are not valid.")
}

/// The sentence CloudFront refuses a MULTI-TENANT configuration that references a parameter
/// its `ParameterDefinitions` does not declare with, on the wire.
///
/// FIXED text: what the refusal states is the rule the configuration broke — a parameter it
/// references has to be declared — which is a property of the configuration as a whole, so
/// no name from the request belongs in it.
pub const PARAMETER_NOT_DECLARED_MESSAGE: &str =
    "A parameter in the DistributionConfig doesn't exist in the ParameterDefinitions list. \
     Remove the parameter and try again.";
