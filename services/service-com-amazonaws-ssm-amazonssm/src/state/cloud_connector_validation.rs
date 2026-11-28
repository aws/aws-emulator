// SPDX-License-Identifier: CC0-1.0

//! Whether a cloud connector can actually reach the third-party cloud environment it
//! targets: the cross-cloud federation its IAM role has to be able to obtain, and the
//! findings a validation of one connector reports.
//!
//! A cloud connector federates INTO the third-party cloud: Systems Manager takes the role
//! `RoleArn` names and exchanges an Amazon Web Services web identity token for the
//! provider's own credential. That exchange is a call the role has to be permitted to make
//! ([`WEB_IDENTITY_TOKEN_ACTION`]), so a role whose permissions withhold it can obtain no
//! token and the connector can federate nothing — which is why the create resolves the
//! question BEFORE it mints a connector ([`federation_failure`]) and why
//! `ValidateCloudConnector` reports it as a finding of the connector it is asked about
//! ([`findings`]).
//!
//! Both the create and the validation ask the SAME questions of the same records — which
//! role of this account the value names and what its trust policy admits
//! ([`super::activation_role`]), what IAM answers about that role's permissions
//! ([`::service_common::iam_role::role_withholds_action`]), whether the ACCOUNT has the
//! outbound web identity federation that vends the token on, and whether the provider
//! credential the token is exchanged FOR can be created for the tenant and the application
//! the configuration names ([`super::provider_credentials`]) — so a connector the create
//! admitted validates clean, and one whose role lost the permission, or whose account or
//! verified provider credential went away, after it was minted validates with the finding
//! that says so. A refusal and a finding are therefore both grounded in what the service
//! HOLDING those records ANSWERED: an emulator with no IAM and no Config mounted to ask
//! refuses nothing and reports no such finding.

use crate::ops::types::{ValidationFinding, ValidationFindingScope};

/// The action a cloud connector's role has to be permitted for the federation step: STS
/// "returns a signed JSON Web Token (JWT) that represents the calling identity" through
/// `GetWebIdentityToken`, which is the token the connector exchanges with the third-party
/// cloud.
pub const WEB_IDENTITY_TOKEN_ACTION: &str = "sts:GetWebIdentityToken";

/// The resource that authorization question is asked about: the token is vended against the
/// CALLING identity rather than against a resource of its own, so a grant of it is a grant
/// over every resource.
pub const WEB_IDENTITY_TOKEN_RESOURCE: &str = "*";

/// How a request whose role cannot obtain that token is refused.
///
/// The whole sentence is FIXED text for this condition: it names the step that failed and
/// what to check, and interpolates nothing from the request — neither the role nor the
/// connector appears in it.
pub const WEB_IDENTITY_TOKEN_UNAVAILABLE: &str =
    "Unable to obtain an AWS web identity token for cross-cloud federation. Please verify \
     the cloud connector's AWS IAM role is permitted to call sts:GetWebIdentityToken.";

/// Whether IAM answered that the role `role_arn` names may not call
/// [`WEB_IDENTITY_TOKEN_ACTION`] — the one reading both the create's refusal and the
/// validation's finding rest on.
pub async fn web_identity_token_is_unobtainable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    ::service_common::iam_role::role_withholds_action(
        ctx,
        region,
        role_arn,
        WEB_IDENTITY_TOKEN_ACTION,
        WEB_IDENTITY_TOKEN_RESOURCE,
    )
    .await
}

/// The `ValidationFindingType` of a finding that reports a check that PASSED: one of the
/// modelled enum's values (`crate::ops::types::VALIDATION_FINDING_TYPE_VALUES`).
pub const FINDING_TYPE_INFO: &str = "INFO";

/// The `ValidationFindingType` of a finding that reports a check that FAILED, and so a
/// connector that cannot do what it was created for. The other end of the same enum.
pub const FINDING_TYPE_ERROR: &str = "ERROR";

/// The `ValidationFindingCode` of the finding that reports the tenant a connector targets
/// (`crate::ops::types::VALIDATION_FINDING_CODE_VALUES`).
pub const FINDING_CODE_TENANT_SUMMARY: &str = "TenantSummary";

/// The `ValidationFindingCode` of the finding that reports a targeted subscription the
/// connector can reach.
pub const FINDING_CODE_SUBSCRIPTION_ACCESSIBLE: &str = "SubscriptionAccessible";

/// The `ValidationFindingCode` of the finding that reports a target the connector cannot
/// reach, which is every target it holds while its federation fails.
pub const FINDING_CODE_TARGET_INACCESSIBLE: &str = "TargetInaccessible";

/// The `ValidationFindingCode` of the finding that reports a role Systems Manager cannot
/// assume at all.
pub const FINDING_CODE_ROLE_ASSUMPTION_FAILED: &str = "AwsRoleAssumptionFailed";

/// The `ValidationFindingCode` of the finding that reports a role that cannot obtain the
/// web identity token the federation step needs.
pub const FINDING_CODE_WEB_IDENTITY_TOKEN_FAILED: &str = "WebIdentityTokenFailed";

/// The `ValidationFindingCode` of the finding that reports the ACCOUNT not having outbound
/// web identity federation on, so none of its principals can obtain the token at all.
pub const FINDING_CODE_OUTBOUND_FEDERATION_DISABLED: &str =
    "OutboundWebIdentityFederationDisabled";

/// The `ValidationFindingCode` of the finding that reports the step AFTER that token: the
/// provider credential it is exchanged for could not be created for the tenant and the
/// application the connector's configuration names.
pub const FINDING_CODE_PROVIDER_CREDENTIAL_CREATION_FAILED: &str =
    "ProviderCredentialCreationFailed";

/// The `ValidationFindingScopeType` of a finding about the Azure TENANT a connector targets
/// (`crate::ops::types::VALIDATION_FINDING_SCOPE_TYPE_VALUES`).
pub const FINDING_SCOPE_TENANT: &str = "azure:tenant";

/// The `ValidationFindingScopeType` of a finding about ONE Azure subscription the connector
/// targets. Another value of the same enum.
pub const FINDING_SCOPE_SUBSCRIPTION: &str = "azure:subscription";

/// The `ValidationFindingScopeType` of a finding about the GCP ORGANIZATION a connector
/// targets (same enum), which is the scope a GCP connector's federation is reported at.
pub const FINDING_SCOPE_ORGANIZATION: &str = "gcp:organization";

/// The `ValidationFindingScopeType` of a finding about ONE GCP project the connector
/// targets. The last value of the same enum.
pub const FINDING_SCOPE_PROJECT: &str = "gcp:project";

/// What a validation of a connector whose federation holds reports about its tenant: the
/// application the connector acts as in that tenant, which is what the configuration names.
pub fn tenant_summary_message(application_id: &str) -> String {
    format!("The cloud connector targets this tenant as application {application_id}")
}

/// What a validation reports about a subscription the connector targets and can reach.
pub const SUBSCRIPTION_ACCESSIBLE_MESSAGE: &str =
    "The cloud connector's federated credential reaches this subscription";

/// What a validation reports about a target the connector holds while its federation
/// fails: the target is not reachable, because nothing the connector could present to the
/// provider can be obtained.
pub const TARGET_INACCESSIBLE_MESSAGE: &str =
    "The cloud connector cannot reach this target while its cross-cloud federation fails";

/// Why the federation of ONE connector fails, as a validation reports it: which step of it
/// IAM answered about.
///
/// `None` — nothing failed — is the connector whose role IAM admits and permits and whose
/// tenant this account holds a verified credential for, and also the connector nothing
/// answered about at all: a finding, like a refusal, is grounded in what the service holding
/// those records said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationFailure {
    /// IAM answered that this service cannot assume the connector's role: the account
    /// holds no such role, or its trust policy withholds the assume.
    RoleAssumption,
    /// IAM answered that the role's permissions withhold
    /// [`WEB_IDENTITY_TOKEN_ACTION`], so the federation step can obtain no token.
    WebIdentityToken,
    /// IAM answered that the ACCOUNT has outbound web identity federation off, so none of
    /// its principals can obtain that token
    /// ([`super::provider_credentials::outbound_federation_is_disabled`]).
    OutboundFederationDisabled,
    /// The token can be obtained, but nothing this account holds verifies a credential for
    /// the tenant and the application the configuration names, so the provider credential
    /// the token is exchanged for cannot be created
    /// ([`super::provider_credentials::azure_credentials_are_unobtainable`]).
    ProviderCredential,
}

impl FederationFailure {
    /// The `ValidationFindingCode` this failure is reported under.
    pub fn code(self) -> &'static str {
        match self {
            Self::RoleAssumption => FINDING_CODE_ROLE_ASSUMPTION_FAILED,
            Self::WebIdentityToken => FINDING_CODE_WEB_IDENTITY_TOKEN_FAILED,
            Self::OutboundFederationDisabled => FINDING_CODE_OUTBOUND_FEDERATION_DISABLED,
            Self::ProviderCredential => FINDING_CODE_PROVIDER_CREDENTIAL_CREATION_FAILED,
        }
    }

    /// The sentence that finding carries: for the federation step, the sentence a create is
    /// refused with for the same answer, so a client reads one wording for one condition.
    pub fn message(self, role_arn: &str) -> String {
        match self {
            Self::RoleAssumption => super::activation_role::unassumable_role_message(role_arn),
            Self::WebIdentityToken => WEB_IDENTITY_TOKEN_UNAVAILABLE.to_string(),
            Self::OutboundFederationDisabled => {
                super::provider_credentials::OUTBOUND_FEDERATION_DISABLED.to_string()
            }
            Self::ProviderCredential => {
                super::provider_credentials::AZURE_CREDENTIALS_UNOBTAINABLE.to_string()
            }
        }
    }
}

/// What the federation of ONE connector is resolved from: the role it acts under, and where
/// the verified credential for the tenant it acts in lives.
///
/// The same four values whether the connector is being CREATED — where they come out of the
/// request — or already held, where they come out of the record ([`Self::of`]), so one
/// question is asked of both.
#[derive(Debug, Clone, Copy)]
pub struct FederatedConnector<'a> {
    /// "The Amazon Resource Name (ARN) of the IAM role that the cloud connector uses to
    /// communicate with the third-party cloud environment."
    pub role_arn: &'a str,
    /// "The ARN of the Amazon Web Services Config connector associated with this cloud
    /// connector", which is where the verified provider credential lives — `None` for a
    /// connector created without one, which names no record to put that question to.
    pub config_connector_arn: Option<&'a str>,
    /// The Azure tenant the connector acts in and the application registration it acts as
    /// there (`AzureConfiguration$TenantId`, `$ApplicationId`), or `None` for a
    /// configuration of another provider, which names neither.
    pub azure_identity: Option<(&'a str, &'a str)>,
}

impl<'a> FederatedConnector<'a> {
    /// The same values read off a connector this service already holds.
    pub fn of(record: &'a super::cloud_connectors::CloudConnectorRecord) -> Self {
        Self {
            role_arn: &record.role_arn,
            config_connector_arn: record.config_connector_arn.as_deref(),
            azure_identity: record.azure_identity(),
        }
    }
}

/// Why the federation of `connector` fails, or `None` when nothing the services holding
/// those records answered says it does.
///
/// The steps are asked in the order the federation takes them: this service assumes the
/// role first ([`super::activation_role::arn_refusal`]), the assumed role then obtains the
/// web identity token, which takes both the role's own permission
/// ([`web_identity_token_is_unobtainable`]) and the ACCOUNT having the feature that vends it
/// on ([`super::provider_credentials::outbound_federation_is_disabled`]), and that token is
/// finally exchanged for the provider's own credential, which can be created only for a
/// tenant and an application this account holds a verified credential for
/// ([`super::provider_credentials::azure_credentials_are_unobtainable`]). A create resolves
/// this BEFORE it mints a connector and refuses with the failing step's own sentence; a
/// validation reports it as a finding of the connector it is asked about.
///
/// The last step is put only about a connector that names BOTH a Config connector to read the
/// verified credential from and the Azure pair it would be verified for: a connector created
/// without a Config connector names no record to read, and a configuration of another
/// provider names no Azure tenant and application to read one for, so in neither case has
/// anything answered that no credential stands behind it.
pub async fn federation_failure(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    connector: FederatedConnector<'_>,
) -> Option<FederationFailure> {
    if super::activation_role::arn_refusal(ctx, region, connector.role_arn)
        .await
        .is_some()
    {
        return Some(FederationFailure::RoleAssumption);
    }
    if web_identity_token_is_unobtainable(ctx, region, connector.role_arn).await {
        return Some(FederationFailure::WebIdentityToken);
    }
    if super::provider_credentials::outbound_federation_is_disabled(ctx, region).await {
        return Some(FederationFailure::OutboundFederationDisabled);
    }
    let (config_connector_arn, (tenant_id, application_id)) =
        match (connector.config_connector_arn, connector.azure_identity) {
            (Some(config_connector_arn), Some(identity)) => (config_connector_arn, identity),
            _ => return None,
        };
    super::provider_credentials::azure_credentials_are_unobtainable(
        ctx,
        region,
        config_connector_arn,
        tenant_id,
        application_id,
    )
    .await
    .then_some(FederationFailure::ProviderCredential)
}

/// One finding of `code`, reported at `scope_type`/`scope_id`.
fn finding(
    finding_type: &str,
    code: &str,
    message: String,
    scope_type: &str,
    scope_id: &str,
) -> ValidationFinding {
    ValidationFinding {
        r#type: Some(finding_type.to_string()),
        code: Some(code.to_string()),
        message: Some(message),
        // "A message from the third-party cloud provider related to the validation
        // finding": these findings are what Systems Manager itself established about the
        // connector, so no provider answered beside them.
        provider_message: None,
        scope: Some(ValidationFindingScope {
            r#type: Some(scope_type.to_string()),
            id: Some(scope_id.to_string()),
        }),
    }
}

/// The findings a validation of `record` reports, given what IAM answered about its role
/// (`failure`).
///
/// Every finding is read out of what the connector's own configuration persisted and what
/// the services holding the records answered about it (`failure`) — the tenant or the
/// organization it targets, the application it acts as, the subscriptions or projects it
/// targets — so two validations of one unchanged connector report the same findings and a
/// validation changes none of them.
///
/// The order is total, which is what lets a page resume at the finding it stopped on: what
/// the connector targets as a whole first, then what the step that failed answered, then its
/// targets in the order the configuration lists them.
///
/// The scopes are the provider's own (`ValidationFindingScopeType`): an Azure connector is
/// reported at its tenant and its subscriptions, a GCP one at its organization and its
/// projects. Only an Azure connector carries the two INFO findings the model declares codes
/// for — `TenantSummary` and `SubscriptionAccessible` — so a GCP connector whose federation
/// holds has nothing to report and reports no finding at all.
pub fn findings(
    record: &super::cloud_connectors::CloudConnectorRecord,
    failure: Option<FederationFailure>,
) -> Vec<ValidationFinding> {
    match record.azure_identity() {
        Some((tenant_id, application_id)) => {
            azure_findings(record, failure, tenant_id, application_id)
        }
        None => gcp_findings(record, failure),
    }
}

/// [`findings`] for a connector whose configuration names an Azure tenant and application.
fn azure_findings(
    record: &super::cloud_connectors::CloudConnectorRecord,
    failure: Option<FederationFailure>,
    tenant_id: &str,
    application_id: &str,
) -> Vec<ValidationFinding> {
    let mut findings = vec![finding(
        FINDING_TYPE_INFO,
        FINDING_CODE_TENANT_SUMMARY,
        tenant_summary_message(application_id),
        FINDING_SCOPE_TENANT,
        tenant_id,
    )];
    if let Some(failure) = failure {
        findings.push(finding(
            FINDING_TYPE_ERROR,
            failure.code(),
            failure.message(&record.role_arn),
            FINDING_SCOPE_TENANT,
            tenant_id,
        ));
    }
    for subscription_id in record.subscription_ids() {
        // A subscription is reachable only through a federated credential, so while the
        // federation fails every target the connector holds is a target it cannot reach.
        let (finding_type, code, message) = match failure {
            None => (
                FINDING_TYPE_INFO,
                FINDING_CODE_SUBSCRIPTION_ACCESSIBLE,
                SUBSCRIPTION_ACCESSIBLE_MESSAGE,
            ),
            Some(_) => (
                FINDING_TYPE_ERROR,
                FINDING_CODE_TARGET_INACCESSIBLE,
                TARGET_INACCESSIBLE_MESSAGE,
            ),
        };
        findings.push(finding(
            finding_type,
            code,
            message.to_string(),
            FINDING_SCOPE_SUBSCRIPTION,
            subscription_id,
        ));
    }
    findings
}

/// [`findings`] for a connector whose configuration targets Google Cloud Platform.
///
/// A failing federation is reported at the ORGANIZATION the configuration names, which is the
/// whole of what the connector reaches there, and every targeted project is a target the
/// connector cannot reach while it fails — the same reading the Azure half takes of a
/// targeted subscription. A connector whose federation holds reports nothing: the modelled
/// finding codes state no reachable GCP target.
fn gcp_findings(
    record: &super::cloud_connectors::CloudConnectorRecord,
    failure: Option<FederationFailure>,
) -> Vec<ValidationFinding> {
    let mut findings = Vec::new();
    let Some(failure) = failure else {
        return findings;
    };
    if let Some(organization_id) = record.organization_id() {
        findings.push(finding(
            FINDING_TYPE_ERROR,
            failure.code(),
            failure.message(&record.role_arn),
            FINDING_SCOPE_ORGANIZATION,
            organization_id,
        ));
    }
    for project_id in record.project_ids() {
        findings.push(finding(
            FINDING_TYPE_ERROR,
            FINDING_CODE_TARGET_INACCESSIBLE,
            TARGET_INACCESSIBLE_MESSAGE.to_string(),
            FINDING_SCOPE_PROJECT,
            project_id,
        ));
    }
    findings
}

/// The published ceiling of `ValidateCloudConnectorMaxResults` (range 0..75), and the page
/// size a request that names no positive `MaxResults` is served: "The maximum number of
/// validation findings to return" bounds a page the caller asked to be bounded, so a caller
/// asking for no bound is served the whole page the operation allows.
pub const MAX_VALIDATION_FINDINGS_PAGE_SIZE: i32 = 75;

/// The smallest page a `MaxResults` can ask for: one finding. The modelled range reaches
/// down to zero because the member carries the shape default `0`, which is the value a
/// request naming no page size carries rather than a page of no findings.
pub const MIN_VALIDATION_FINDINGS_PAGE_SIZE: i32 = 1;

/// The page size a validation serves, or the refusal message for a `MaxResults` outside the
/// modelled range — which the protocol's boundary validator already refuses, so this is a
/// backstop rather than the first line of defence.
pub fn validation_findings_page_size(max_results: Option<i32>) -> Result<usize, String> {
    match max_results {
        None | Some(0) => Ok(MAX_VALIDATION_FINDINGS_PAGE_SIZE as usize),
        requested => super::page_size(
            requested,
            MIN_VALIDATION_FINDINGS_PAGE_SIZE,
            MAX_VALIDATION_FINDINGS_PAGE_SIZE,
        ),
    }
}
