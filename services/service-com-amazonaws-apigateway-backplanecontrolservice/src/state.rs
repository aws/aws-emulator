// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigateway-backplanecontrolservice service.
//!
//! This holds the REST API resource tree the control plane persists — a `RestApi`, the
//! `Resource`s under it (including the implicit root resource every API is created
//! with), the `Method`s put on those resources, the API's `Stage`s and the gateway
//! responses a developer has customized on it — beside the
//! account's `ApiKey`, `VpcLink`, `UsagePlan` and custom `DomainName` records and the
//! base path mappings and domain name access associations created against those domain
//! names. Every
//! handler in `src/ops/` computes its response from these records; nothing is synthesised
//! per request.
//!
//! A few leaf values are stored as the modelled shapes from [`crate::ops::types`]
//! (`EndpointConfiguration`, `Method`'s responses and integration) rather than as a
//! second hand-written mirror of the same members: the model already declares them, and
//! two copies of one shape is how two operations end up reading two different records.
//!
//! Two collections of this surface are the SERVICE's own rather than an account's — the
//! gateway responses API Gateway generates and the `SdkType`s it can generate a client
//! SDK for. Neither is created, updated or deleted by any operation of this service, so
//! each is a catalog here ([`default_gateway_response`], [`sdk_type`]) that every
//! handler reading it answers from, instead of a per-account record a caller could never
//! have written.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    AccessLogSettings, ApiStage, CanarySettings, DocumentationPartLocation, EndpointConfiguration,
    GetGatewayResponseOutput, GetIntegrationOutput, GetMethodResponseOutput, GetSdkTypeOutput,
    MethodSetting, MethodSnapshot, MutualTlsAuthentication, QuotaSettings,
    SdkConfigurationProperty, ThrottleSettings, SECURITY_POLICY_VALUES,
};

/// Per-service state for com-amazonaws-apigateway-backplanecontrolservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApigatewayBackplanecontrolserviceState {
    /// Every created `RestApi`, keyed by its API id.
    pub rest_apis: BTreeMap<String, RestApiEntry>,
    /// Every created `ApiKey`, keyed by its API key id — the identifier `GetApiKey`
    /// and `DeleteApiKey` take in their `apiKey` path label, NOT the key's value.
    pub api_keys: BTreeMap<String, ApiKeyEntry>,
    /// Every created `VpcLink`, keyed by its VPC link id — the identifier `GetVpcLink`
    /// takes in its `vpcLinkId` path label and an `Integration`'s `connectionId` names.
    /// A VPC link belongs to the ACCOUNT rather than to one API, so it sits beside the
    /// APIs here rather than under one of them.
    pub vpc_links: BTreeMap<String, VpcLinkEntry>,
    /// Every created `UsagePlan`, keyed by its usage plan id — the identifier
    /// `GetUsagePlan` takes in its `usagePlanId` path label.
    pub usage_plans: BTreeMap<String, UsagePlanEntry>,
    /// Every created custom `DomainName`, keyed by [`domain_name_key`]: the domain name
    /// itself, or `<domainName>+<domainNameId>` for a private one, which is addressed by
    /// both members because several private domain names may carry one name.
    pub domain_names: BTreeMap<String, DomainNameEntry>,
    /// Every created `DomainNameAccessAssociation`, keyed by the identifier the resource
    /// portion of its ARN carries — the one `DeleteDomainNameAccessAssociation` and
    /// `RejectDomainNameAccessAssociation` address an association by, inside the ARN they
    /// take. An access association belongs to the ACCOUNT rather than to the domain name
    /// it names, so it sits beside the domain names here rather than inside one of them:
    /// the operation that lists them lists the account's, whichever domain name each one
    /// is for.
    pub domain_name_access_associations: BTreeMap<String, DomainNameAccessAssociationEntry>,
    /// Every `ClientCertificate` of the account, keyed by its client certificate id —
    /// the identifier `GetClientCertificate` takes in its `clientCertificateId` path
    /// label and a `Stage`'s `clientCertificateId` names one of. A client certificate
    /// belongs to the ACCOUNT rather than to one API, so it sits beside the APIs here.
    ///
    /// `GenerateClientCertificate` is the operation that mints one, so a `clientCertificateId`
    /// no entry of this map answers to names no certificate of this account — which is the
    /// refusal [`NO_SUCH_CLIENT_CERTIFICATE`] words. See [`client_certificates`] for the
    /// record's material, its validity window and the reference a stage holds on it.
    pub client_certificates: BTreeMap<String, ClientCertificateEntry>,
    /// The account-level settings `UpdateAccount` writes and `GetAccount` reports.
    /// There is exactly one per account, so it is a field rather than a map.
    pub account: AccountEntry,
    /// The token bucket each API key that has invoked a stage is metered against, keyed
    /// by [`metering_key`]: one bucket per `(usage plan, API key)` pair, because a usage
    /// plan's throttle is the request rate it admits FOR EACH of its keys
    /// (`ThrottleSettings$rateLimit`, "the API target request rate limit" of the plan the
    /// key is metered by).
    ///
    /// Written by the `execute-api` data plane while METERING a request (see
    /// [`crate::service::invoke`]), which is what a usage plan's throttle is: the rate a
    /// key's requests are admitted at cannot be computed without recording the ones
    /// already admitted. Bounded by the stores it joins — at most
    /// [`MAX_USAGE_PLANS`] x [`MAX_API_KEYS`] pairs can ever exist — and reclaimed with
    /// the key it meters
    /// ([`ComAmazonawsApigatewayBackplanecontrolserviceState::forget_api_key`]), so it
    /// never grows with the request count.
    pub throttle_buckets: BTreeMap<String, ThrottleBucket>,
    /// The requests each API key that has invoked a stage has sent, counted by the day
    /// each one fell on and keyed by [`metering_key`] — one log per `(usage plan, API
    /// key)` pair, for the reason the throttle buckets above are: a usage plan's quota is
    /// "the target maximum number of requests that can be made in a given time period"
    /// (`QuotaSettings$limit`) FOR EACH of its keys.
    ///
    /// Written by the `execute-api` data plane while METERING a request (see
    /// [`crate::service::invoke`]) and read by the usage report, through the one
    /// accounting in [`usage`]: the throttle limits the RATE a key sends at, this count
    /// the NUMBER of requests it may send in a metering period. Bounded by the stores it
    /// joins, as the buckets are, times the days a report may cover
    /// ([`MAX_USAGE_INTERVAL_DAYS`]), and reclaimed with the key it meters
    /// ([`ComAmazonawsApigatewayBackplanecontrolserviceState::forget_api_key`]).
    pub usage_logs: BTreeMap<String, UsageLog>,
    /// The ADDRESS each regional custom domain name is served at, keyed by the
    /// [`domain_name_key`] of the record it belongs to — the `host:port` the record reports
    /// as its `regionalDomainName` and the live socket that answers there
    /// ([`CustomDomainEndpoint`]). One entry per domain name this emulator opened an address
    /// for; dropping an entry closes that port and severs every connection in flight on it,
    /// which is what a delete of the domain name (or a migration to another endpoint type)
    /// does.
    pub custom_domain_endpoints: BTreeMap<String, CustomDomainEndpoint>,
    /// Drives [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`]: the creation rank stamped on each
    /// record so a listing can report creation order rather than id order.
    pub sequence_counter: u64,
}

/// One `ClientCertificate` of the account: "a client certificate used to configure
/// client-side SSL authentication while sending requests to the integration endpoint"
/// (`smithy_query(query="shape", shape="ClientCertificate")`), whose members this record
/// holds beside the id it is keyed by.
#[derive(Debug, Clone, Default)]
pub struct ClientCertificateEntry {
    /// "The identifier of the client certificate", minted by
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::mint_id`] and the key this
    /// record is held under.
    pub id: String,
    /// "The description of the client certificate".
    pub description: Option<String>,
    /// "The PEM-encoded public key of the client certificate", the certificate API Gateway
    /// generated for this record; see [`mint_client_certificate_pem`].
    pub pem_encoded_certificate: Option<String>,
    /// "The timestamp when the client certificate was created".
    pub created_date: Option<::aws_smithy_types::DateTime>,
    /// "The timestamp when the client certificate will expire": the creation instant plus
    /// [`CLIENT_CERTIFICATE_VALIDITY`], which is the window the generated material itself
    /// bears.
    pub expiration_date: Option<::aws_smithy_types::DateTime>,
    /// "The collection of tags" the certificate carries.
    pub tags: BTreeMap<String, String>,
    /// This certificate's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a listing
    /// can report creation order rather than id order.
    pub sequence: u64,
}

/// The `BadRequestException` message for a `clientCertificateId` that names no client
/// certificate of the account.
///
/// A stage presents "the identifier of a client certificate for an API stage"
/// (`Stage$clientCertificateId`), so the value has to name one the account has. Entirely
/// fixed text, in the shape of this surface's other invalid-identifier sentences: it
/// quotes neither the value nor the stage.
pub const NO_SUCH_CLIENT_CERTIFICATE: &str = "Invalid client certificate id specified";

/// The account's own `Account` settings: what `GetAccount` reports and `UpdateAccount`
/// patches.
///
/// The two values a caller never sets — the request limits API Gateway applies to the
/// account and the version of the API keys it issues — are the service's own and are
/// seeded from [`DEFAULT_ACCOUNT_THROTTLE`] / [`API_KEY_VERSION`]; `cloudwatch_role_arn`
/// is absent until a caller sets one, which is what distinguishes an account with no
/// CloudWatch role from one whose role is configured.
#[derive(Debug, Clone)]
pub struct AccountEntry {
    /// The ARN of the IAM role API Gateway uses to write logs to CloudWatch Logs;
    /// `None` on an account that has never had one set.
    pub cloudwatch_role_arn: Option<String>,
    /// The account-wide request limits, in `(burstLimit, rateLimit)` order; see
    /// [`DEFAULT_ACCOUNT_THROTTLE`].
    pub throttle_burst_limit: i32,
    /// See [`DEFAULT_ACCOUNT_THROTTLE`].
    pub throttle_rate_limit: f64,
    /// The features enabled for the account, reported in the `features` list.
    pub features: Vec<String>,
    /// The version of the API keys the account issues; see [`API_KEY_VERSION`].
    pub api_key_version: String,
}

impl Default for AccountEntry {
    fn default() -> Self {
        Self {
            cloudwatch_role_arn: None,
            throttle_burst_limit: DEFAULT_ACCOUNT_THROTTLE.0,
            throttle_rate_limit: DEFAULT_ACCOUNT_THROTTLE.1,
            features: vec![USAGE_PLANS_FEATURE.to_string()],
            api_key_version: API_KEY_VERSION.to_string(),
        }
    }
}

/// The account-wide request limits API Gateway applies, as `(burstLimit, rateLimit)`.
///
/// From API Gateway's published default quota for an account in one Region: "10,000
/// requests per second (RPS) with an additional burst capacity provided by the token
/// bucket algorithm, using a maximum bucket capacity of 5,000 requests" — the pair
/// `GetAccount` reports as `throttleSettings`.
pub const DEFAULT_ACCOUNT_THROTTLE: (i32, f64) = (5_000, 10_000.0);

/// The `features` entry an account with usage plans enabled carries, from
/// `Account$features`'s own documentation: "When usage plans are enabled, the features
/// list will include an entry of `UsagePlans`". It is an account-level flag of the
/// Region's API Gateway, not a claim about which operations of this emulator are
/// implemented, and AWS reports it for every account.
pub const USAGE_PLANS_FEATURE: &str = "UsagePlans";

/// The `apiKeyVersion` of an account: the current version of the API keys API Gateway
/// issues, which every account reports.
pub const API_KEY_VERSION: &str = "4";

/// The `patchOperations` path `UpdateAccount` applies: the account's CloudWatch role.
/// It is the only member of `Account` a caller sets — the others are the service's own
/// (see [`AccountEntry`]) — so it is the only path this operation supports.
pub const ACCOUNT_CLOUDWATCH_ROLE_PATH: &str = "cloudwatchRoleArn";

/// One stored `RestApi`, with the resource tree that hangs off it.
#[derive(Debug, Clone, Default)]
pub struct RestApiEntry {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    /// The Region this API was created in — the Region of the request that created it
    /// ([`request_region`]), recorded once rather than re-derived per read.
    ///
    /// Every ARN this service builds for the API, and for the stages under it, names this
    /// Region, so the ARN is the same string whichever endpoint a later caller addressed
    /// the emulator by. That matters most where the later caller carries no Region of its
    /// own: a deployed stage is invoked at a host that need not spell one, while the stage
    /// ARN another service bound the stage by was built from the Region of the create.
    pub region: String,
    pub binary_media_types: Vec<String>,
    pub minimum_compression_size: Option<i32>,
    /// What the request set, or [`DEFAULT_API_KEY_SOURCE`] when it set nothing: the
    /// service picks a value at create time, so a stored API always has one.
    pub api_key_source: Option<String>,
    /// What the request set, completed by [`defaulted_endpoint_configuration`], so the
    /// stored configuration always names its endpoint types and its IP address type.
    pub endpoint_configuration: Option<EndpointConfiguration>,
    pub policy: Option<String>,
    /// What the request set, or [`DEFAULT_SECURITY_POLICY`]; see [`api_key_source`].
    ///
    /// [`api_key_source`]: RestApiEntry::api_key_source
    pub security_policy: Option<String>,
    pub endpoint_access_mode: Option<String>,
    pub api_status: Option<String>,
    pub api_status_message: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub disable_execute_api_endpoint: bool,
    /// The id of this API's implicit root resource, which is also a key of `resources`.
    pub root_resource_id: String,
    /// Every resource of this API, keyed by resource id; the root resource included.
    pub resources: BTreeMap<String, ResourceEntry>,
    /// Every `Stage` of this API, keyed by stage name. A stage is what an API key is
    /// associated with and what a caller invokes, so it is the record a `stageKeys`
    /// entry must name.
    pub stages: BTreeMap<String, StageEntry>,
    /// Every `Deployment` of this API, keyed by deployment id: what a stage points at.
    pub deployments: BTreeMap<String, DeploymentEntry>,
    /// The gateway responses a developer has CUSTOMIZED on this API, keyed by their
    /// `responseType`; written by `PutGatewayResponse` and reported by
    /// `GetGatewayResponse` and `GetGatewayResponses`.
    ///
    /// A type with no entry here is not missing: the API still has a gateway response
    /// for it, the one API Gateway serves for the type, which
    /// [`reported_gateway_response_of`] resolves. So this map holds only the
    /// customizations, and a stored record is always reported with
    /// `defaultResponse: false`.
    pub gateway_responses: BTreeMap<String, GetGatewayResponseOutput>,
    /// Every `DocumentationPart` of this API, keyed by documentation part id: the
    /// records `CreateDocumentationPart` and `ImportDocumentationParts` write and
    /// `GetDocumentationPart` / `GetDocumentationParts` report.
    pub documentation_parts: BTreeMap<String, DocumentationPartEntry>,
    /// Every `DocumentationVersion` of this API, keyed by its version identifier — the
    /// identifier `CreateDocumentationVersion` mints the snapshot under and
    /// `GetDocumentationVersion` takes in its `documentationVersion` path label. A
    /// version identifier is the record's KEY, which is what makes a second snapshot
    /// under one identifier a conflict.
    pub documentation_versions: BTreeMap<String, DocumentationVersionEntry>,
    /// Every `Authorizer` of this API, keyed by authorizer id — the identifier a
    /// `Method`'s `authorizerId` names and `GetAuthorizer` takes in its path label.
    pub authorizers: BTreeMap<String, AuthorizerEntry>,
    /// Every `RequestValidator` of this API, keyed by request validator id — the
    /// identifier a `Method`'s `requestValidatorId` names and `GetRequestValidator`
    /// takes in its path label.
    pub request_validators: BTreeMap<String, RequestValidatorEntry>,
    /// Every `Model` of this API, keyed by the model NAME — the identifier `GetModel`
    /// takes in its `modelName` path label and a `Method`'s `requestModels` /
    /// `MethodResponse`'s `responseModels` entry names. The NAME is the record's key,
    /// which is what makes a second model under one name a conflict; the minted
    /// `id` is carried on the record rather than keying it, because no operation of this
    /// service addresses a model by id.
    pub models: BTreeMap<String, ModelEntry>,
    /// This API's creation rank, from [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`].
    pub sequence: u64,
}

impl RestApiEntry {
    /// The Region [`Self::region`] records, or [`DEFAULT_REGION`] for a record that
    /// carries none: the Region every ARN of this API and of its stages names.
    pub fn arn_region(&self) -> &str {
        if self.region.is_empty() {
            DEFAULT_REGION
        } else {
            &self.region
        }
    }

    /// This API's resources in creation order — the root resource first, because it is
    /// created with the API and so carries the lowest rank.
    pub fn resources_in_creation_order(&self) -> Vec<&ResourceEntry> {
        let mut ordered: Vec<&ResourceEntry> = self.resources.values().collect();
        ordered.sort_by_key(|resource| (resource.sequence, resource.id.clone()));
        ordered
    }

    /// The ids of [`Self::resources_in_creation_order`], the stable sequence
    /// [`page_of`] cuts a `GetResources` page out of.
    pub fn resource_ids_in_creation_order(&self) -> Vec<String> {
        self.resources_in_creation_order()
            .into_iter()
            .map(|resource| resource.id.clone())
            .collect()
    }

    /// Every resource of this API that `resource_id` owns — itself and everything whose
    /// stored `parentId` chain reaches it.
    ///
    /// This is the subtree a resource carries with it: the set `DeleteResource` removes,
    /// the set whose derived `path` a rename or a move recomputes, and the set a resource
    /// may not be moved into. One walk, so the ownership rule cannot differ between the
    /// operations that apply it. A record whose parent link is missing ends its branch.
    pub fn descendant_ids(&self, resource_id: &str) -> Vec<String> {
        let mut found: BTreeSet<String> = BTreeSet::new();
        found.insert(resource_id.to_string());
        let mut frontier = vec![resource_id.to_string()];
        while let Some(parent) = frontier.pop() {
            for child in self.resources.values() {
                if child.parent_id.as_deref() == Some(parent.as_str())
                    && found.insert(child.id.clone())
                {
                    frontier.push(child.id.clone());
                }
            }
        }
        found.into_iter().collect()
    }

    /// This API's documentation parts in creation order — the stable sequence
    /// [`page_of`] cuts a `GetDocumentationParts` page out of.
    pub fn documentation_parts_in_creation_order(&self) -> Vec<&DocumentationPartEntry> {
        let mut ordered: Vec<&DocumentationPartEntry> = self.documentation_parts.values().collect();
        ordered.sort_by_key(|part| (part.sequence, part.id.clone()));
        ordered
    }

    /// This API's documentation versions in creation order — the oldest snapshot first,
    /// by the creation rank stamped on each record, which is what
    /// [`DocumentationVersionEntry::sequence`] is for: the records are KEYED by version
    /// identifier, so their map order is identifier order rather than the order the
    /// snapshots were taken in.
    pub fn documentation_versions_in_creation_order(&self) -> Vec<&DocumentationVersionEntry> {
        let mut ordered: Vec<&DocumentationVersionEntry> =
            self.documentation_versions.values().collect();
        ordered.sort_by_key(|version| (version.sequence, version.version.clone()));
        ordered
    }

    /// The version identifiers of [`Self::documentation_versions_in_creation_order`], the
    /// stable sequence [`page_of`] cuts a `GetDocumentationVersions` page out of. A
    /// snapshot's identifier IS its key, so it is also the `position` cursor a page hands
    /// back.
    pub fn documentation_version_ids_in_creation_order(&self) -> Vec<String> {
        self.documentation_versions_in_creation_order()
            .into_iter()
            .map(|version| version.version.clone())
            .collect()
    }

    /// This API's deployments in creation order — the oldest deployment first, by the
    /// creation rank stamped on each record, exactly as this crate's other collections of
    /// an API's children are ordered.
    pub fn deployments_in_creation_order(&self) -> Vec<&DeploymentEntry> {
        let mut ordered: Vec<&DeploymentEntry> = self.deployments.values().collect();
        ordered.sort_by_key(|deployment| (deployment.sequence, deployment.id.clone()));
        ordered
    }

    /// The ids of [`Self::deployments_in_creation_order`], the stable sequence
    /// [`page_of`] cuts a `GetDeployments` page out of.
    pub fn deployment_ids_in_creation_order(&self) -> Vec<String> {
        self.deployments_in_creation_order()
            .into_iter()
            .map(|deployment| deployment.id.clone())
            .collect()
    }

    /// This API's models in creation order — the oldest model first, by the creation rank
    /// stamped on each record, exactly as this crate's other collections of an API's
    /// children are ordered. The records are KEYED by model name, so their map order is
    /// name order rather than the order the models were created in.
    pub fn models_in_creation_order(&self) -> Vec<&ModelEntry> {
        let mut ordered: Vec<&ModelEntry> = self.models.values().collect();
        ordered.sort_by(|left, right| {
            (left.sequence, left.name.as_str()).cmp(&(right.sequence, right.name.as_str()))
        });
        ordered
    }

    /// The names of [`Self::models_in_creation_order`], the stable sequence [`page_of`]
    /// cuts a `GetModels` page out of. A model's NAME is its key — the label `GetModel`
    /// addresses it by — so it is also the `position` cursor a page hands back.
    pub fn model_names_in_creation_order(&self) -> Vec<String> {
        self.models_in_creation_order()
            .into_iter()
            .map(|model| model.name.clone())
            .collect()
    }

    /// The names of this API's stages ASSOCIATED with `deployment_id`, in stage-name
    /// order: the one record that decides whether a deployment is still in use.
    ///
    /// A stage is associated with a deployment when it names it — as the deployment it
    /// serves, or as the deployment its canary serves. Both references are the SAME
    /// reference as far as stored state is concerned: `CreateStage` and `UpdateStage`
    /// resolve each one against this API's deployments before storing it, so a deployment
    /// either reference names is a record the API must still hold.
    pub fn stage_names_using_deployment(&self, deployment_id: &str) -> Vec<String> {
        self.stages
            .values()
            .filter(|stage| {
                stage.deployment_id == deployment_id
                    || stage
                        .canary_settings
                        .as_ref()
                        .and_then(|canary| canary.deployment_id.as_deref())
                        == Some(deployment_id)
            })
            .map(|stage| stage.stage_name.clone())
            .collect()
    }

    /// The method of this API that names `authorizer_id` in its `authorizerId`, or `None`
    /// when none does.
    ///
    /// A method names its authorizer in `Method$authorizerId`, so a method carrying that
    /// identifier is a reference to the authorizer; one walk, so every operation that has
    /// to know whether an authorizer is referenced reads the same records.
    pub fn method_referencing_authorizer(&self, authorizer_id: &str) -> Option<MethodReference> {
        self.referencing_method(|method| method.authorizer_id.as_deref() == Some(authorizer_id))
    }

    /// The method of this API that names `request_validator_id` in its
    /// `requestValidatorId`, or `None` when none does.
    ///
    /// A method names the validator its requests are validated by in
    /// `Method$requestValidatorId`, so a method carrying that identifier is a reference to
    /// the validator — the same kind of reference
    /// [`Self::method_referencing_authorizer`] finds, through the same walk, so every
    /// operation that has to know whether a validator is referenced reads the same
    /// records.
    pub fn method_referencing_request_validator(
        &self,
        request_validator_id: &str,
    ) -> Option<MethodReference> {
        self.referencing_method(|method| {
            method.request_validator_id.as_deref() == Some(request_validator_id)
        })
    }

    /// The place in this API that still names `model_name` as a data schema, or `None`
    /// when nothing does.
    ///
    /// `Method$requestModels` ("a key-value map specifying data schemas ... the value is a
    /// `Model` name") and `MethodResponse$responseModels` ("specifies the `Model`
    /// resources used for the response's content type") are the two maps that name a
    /// model, so a method request or a method response carrying that name is a reference
    /// to the model — the same kind of reference [`Self::method_referencing_authorizer`]
    /// finds. Both sites are walked in one pass, in the order
    /// [`Self::methods_in_path_order`] fixes and then by the response's status code, so a
    /// refusal NAMES the same reference on every call for one set of records.
    pub fn reference_to_model(&self, model_name: &str) -> Option<ModelReference> {
        let mut referencing: Vec<ModelReference> = Vec::new();
        for (at, method) in self.methods_in_path_order() {
            if method
                .request_models
                .values()
                .any(|named| named == model_name)
            {
                referencing.push(ModelReference::MethodRequest(at.clone()));
            }
            for (status_code, response) in &method.method_responses {
                if response
                    .response_models
                    .iter()
                    .flatten()
                    .any(|(_, named)| named == model_name)
                {
                    referencing
                        .push(ModelReference::MethodResponse(at.clone(), status_code.clone()));
                }
            }
        }
        referencing.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        referencing.into_iter().next()
    }

    /// The first method of this API, in resource-path then verb order, that `names`
    /// something — the one lookup the single-valued references above share.
    fn referencing_method(
        &self,
        mut names: impl FnMut(&MethodEntry) -> bool,
    ) -> Option<MethodReference> {
        self.methods_in_path_order()
            .into_iter()
            .find(|(_, method)| names(method))
            .map(|(at, _)| at)
    }

    /// Every method of this API paired with the reference that names it, in resource-path
    /// then verb order — the one walk every reference lookup here shares.
    ///
    /// The order is not the stored one (`resources` is keyed by id, so the paths are
    /// sorted here), which is what makes the method a refusal NAMES the same one on every
    /// call for one set of records rather than whichever the map yielded first.
    fn methods_in_path_order(&self) -> Vec<(MethodReference, &MethodEntry)> {
        let mut methods: Vec<(MethodReference, &MethodEntry)> = Vec::new();
        for resource in self.resources.values() {
            for (verb, method) in &resource.methods {
                methods.push((
                    MethodReference {
                        http_method: verb.clone(),
                        resource_path: resource.path.clone(),
                        resource_id: resource.id.clone(),
                    },
                    method,
                ));
            }
        }
        methods.sort_by(|(left, _), (right, _)| {
            (&left.resource_path, &left.http_method)
                .cmp(&(&right.resource_path, &right.http_method))
        });
        methods
    }
}

/// One method of an API, as a refusal that is BLOCKED BY it names it: the verb and the
/// resource the method sits on, which is how API Gateway identifies a method in such a
/// sentence — by the resource's PATH in some of them and by its ID in others, so the
/// reference carries both and each refusal takes the one its own sentence states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodReference {
    /// The method's uppercase HTTP verb.
    pub http_method: String,
    /// The materialised path of the resource the method sits on, leading `/` included.
    pub resource_path: String,
    /// The id of the resource the method sits on — the `Resource$id` a refusal that
    /// identifies the method by its RESOURCE IDENTIFIER names (see
    /// [`vpc_link_in_use`]).
    pub resource_id: String,
}

impl ::std::fmt::Display for MethodReference {
    /// `<HTTP-METHOD>/<resource-path>`, the spelling a refusal naming the blocked
    /// AUTHORIZER carries. The resource path keeps its own leading `/`, so the method on
    /// `/pets` is `GET//pets` and the one on the root resource is `GET//`.
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(formatter, "{}/{}", self.http_method, self.resource_path)
    }
}

impl MethodReference {
    /// `<resource-path>/<HTTP-METHOD>`, the spelling a refusal naming the method REQUEST
    /// that references a blocked model carries — the resource path first, so the method on
    /// the root resource reads `//POST`. The two spellings are each what API Gateway states
    /// for its own refusal, which is why this is not the `Display` one.
    pub fn in_method_request(&self) -> String {
        format!("{}/{}", self.resource_path, self.http_method)
    }

    /// `<resource-path>/<HTTP-METHOD>/<statusCode>`, the spelling a refusal naming the
    /// method RESPONSE that references a blocked model carries: a method response is keyed
    /// by its `statusCode` under the method, so identifying the reference site takes that
    /// status code on top of the method's own [`Self::in_method_request`] spelling.
    pub fn in_method_response(&self, status_code: &str) -> String {
        format!("{}/{status_code}", self.in_method_request())
    }
}

/// Where a `Model` of an API is still referenced from — the site a refusal blocked by that
/// reference names.
///
/// A model is named as a data schema in two places, and they are DIFFERENT sites: the
/// method request's `requestModels` and one method response's `responseModels`, the latter
/// keyed by the response's status code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelReference {
    /// A method whose `requestModels` names the model.
    MethodRequest(MethodReference),
    /// A method response, under the status code it is keyed by, whose `responseModels`
    /// names the model.
    MethodResponse(MethodReference, String),
}

impl ModelReference {
    /// The order two references of one API are compared in: the method first, then the
    /// status code, so the request site of a method precedes its response sites.
    fn order_key(&self) -> (&str, &str, &str) {
        match self {
            Self::MethodRequest(at) => (&at.resource_path, &at.http_method, ""),
            Self::MethodResponse(at, status_code) => {
                (&at.resource_path, &at.http_method, status_code)
            }
        }
    }
}

/// One stored `Resource`: a node of an API's path tree.
#[derive(Debug, Clone, Default)]
pub struct ResourceEntry {
    pub id: String,
    /// Absent on the root resource only.
    pub parent_id: Option<String>,
    /// Absent on the root resource only.
    pub path_part: Option<String>,
    /// The materialised full path, the parent's path with this resource's path part
    /// appended; `/` on the root resource.
    pub path: String,
    /// The methods put on this resource, keyed by uppercase HTTP verb.
    pub methods: BTreeMap<String, MethodEntry>,
    /// This resource's creation rank, from [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`]; `0` on
    /// the root resource.
    pub sequence: u64,
}

/// One stored `Stage` of an API: a named pointer at one deployment, which is what a
/// caller invokes and what an `ApiKey`'s `stageKeys` entry associates a key with.
#[derive(Debug, Clone, Default)]
pub struct StageEntry {
    pub stage_name: String,
    /// The deployment this stage serves; always the id of a stored
    /// [`DeploymentEntry`] of the same API.
    pub deployment_id: String,
    pub description: Option<String>,
    pub cache_cluster_enabled: bool,
    pub cache_cluster_size: Option<String>,
    /// When the stage's cache cluster finishes being created. A cluster is reported
    /// `CREATE_IN_PROGRESS` until this instant and `AVAILABLE` after it; absent when the
    /// stage has no cache cluster, which is reported `NOT_AVAILABLE`. See
    /// [`CACHE_CLUSTER_CREATE_SPAN`].
    pub cache_cluster_ready_at: Option<::std::time::Instant>,
    /// When the LAST cache flush `FlushStageCache` started on this stage finishes. A
    /// cluster is reported `FLUSH_IN_PROGRESS` until this instant and `AVAILABLE` from
    /// it on, so an instant already in the past means no flush is outstanding; absent
    /// until the stage is flushed at all. See [`CACHE_CLUSTER_FLUSH_SPAN`].
    pub cache_flush_done_at: Option<::std::time::Instant>,
    /// When the cache cluster an `UpdateStage` DISABLED finishes being torn down. A
    /// cluster is reported `DELETE_IN_PROGRESS` until this instant and `NOT_AVAILABLE`
    /// from it on; absent while the stage has a cluster and on one that never had one.
    /// See [`CACHE_CLUSTER_DELETE_SPAN`].
    pub cache_cluster_removed_at: Option<::std::time::Instant>,
    /// When `FlushStageAuthorizersCache` LAST dropped this stage's authorizer cache
    /// entries; absent until the stage's authorizer cache is flushed at all.
    ///
    /// This is the stage's authorizer-cache invalidation watermark: an authorizer result
    /// cached before this instant is no longer usable, so a reader of a cached authorizer
    /// result must treat anything older as a miss. Unlike a cache cluster, which a stage
    /// only has when the caller asked for one, EVERY stage has an authorizer cache (each
    /// authorizer caches its result for its own
    /// `authorizerResultTtlInSeconds`), so this watermark is written on every accepted
    /// flush of a stage that exists.
    ///
    /// No operation of this service REPORTS it: API Gateway's `Stage` has no member for
    /// the authorizer cache, so a flush of it leaves every member `GetStage` reports —
    /// `cacheClusterStatus` included — exactly as it was.
    pub authorizers_cache_flushed_at: Option<::std::time::Instant>,
    pub variables: BTreeMap<String, String>,
    pub documentation_version: Option<String>,
    /// The canary release configured on this stage: `CreateStage`'s or
    /// `CreateDeployment`'s `canarySettings`, and what an `UpdateStage` patch of
    /// `/canarySettings` changes; absent on a stage with no canary.
    ///
    /// A canary that carries no `deployment_id` of its own serves the deployment the
    /// STAGE serves, which every reader derives when it reports the stage (see
    /// [`crate::ops::create_stage::stage_body`]) rather than storing here, so re-pointing
    /// the stage moves such a canary with it.
    pub canary_settings: Option<CanarySettings>,
    /// The access logging an `UpdateStage` patch of `/accessLogSettings` configured on
    /// this stage, kept as the modelled shape for the reason the module docs give;
    /// absent until a patch sets it.
    pub access_log_settings: Option<AccessLogSettings>,
    /// The client certificate an `UpdateStage` patch of `/clientCertificateId`
    /// associated with this stage: one of the account's
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::client_certificates`].
    pub client_certificate_id: Option<String>,
    pub tracing_enabled: bool,
    /// The per-method settings `UpdateStage` writes, keyed by the method path the patch
    /// addressed — `{resource_path}/{http_method}`, or `*/*` for every method of the
    /// stage; empty until a patch sets one, which is the empty `methodSettings` map a
    /// freshly created stage reports.
    pub method_settings: BTreeMap<String, MethodSetting>,
    /// The responses this stage's cache cluster is HOLDING, keyed by the cache key the
    /// data plane builds for one method request
    /// ([`crate::service::invoke`]); empty on a stage nothing has been
    /// cached for, which is every stage whose methods do not enable caching.
    ///
    /// Written by the `execute-api` data plane while SERVING a request, which is what a
    /// response cache is: a cached response cannot be served again without the serving
    /// that stored it. Bounded at [`MAX_CACHED_RESPONSES_PER_STAGE`] entries
    /// ([`StageEntry::cache_response`]), so it never grows with the request count, and
    /// emptied by `FlushStageCache` ([`StageEntry::flush_response_cache`]). No operation
    /// of this service REPORTS it: API Gateway's `Stage` has no member for what its cache
    /// holds.
    pub response_cache: BTreeMap<String, CachedResponse>,
    /// The authorizer RESULTS this stage is holding, keyed by the authorizer and the value
    /// the request carried for its identity source ([`crate::service::invoke`]); empty on a
    /// stage no authorizer has decided a call of yet.
    ///
    /// `Authorizer$authorizerResultTtlInSeconds` is "the TTL in seconds of cached
    /// authorizer results", so a result is HELD for that long and the authorizer is not
    /// called again while it is: written by the `execute-api` data plane while serving a
    /// request, which is the only thing that can obtain a result to hold. Bounded at
    /// [`MAX_CACHED_AUTHORIZER_RESULTS_PER_STAGE`] entries
    /// ([`StageEntry::cache_authorizer_result`]), and dropped before its TTL by
    /// `FlushStageAuthorizersCache` through [`StageEntry::authorizers_cache_flushed_at`].
    /// No operation of this service REPORTS it: API Gateway's `Stage` has no member for
    /// what its authorizer cache holds.
    pub authorizer_results: BTreeMap<String, CachedAuthorizerResult>,
    pub tags: BTreeMap<String, String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    pub last_updated_date: Option<::aws_smithy_types::DateTime>,
}

impl StageEntry {
    /// The `cacheClusterStatus` this stage reports now.
    ///
    /// A stage without a cache cluster has none to report, which is `NOT_AVAILABLE`. A
    /// cluster the caller asked for is created over [`CACHE_CLUSTER_CREATE_SPAN`], so a
    /// caller reading straight after the call still sees `CREATE_IN_PROGRESS`; the
    /// transition is one the CLOCK owes, materialised from the stored due instant rather
    /// than by sleeping or by being read. A cluster a `FlushStageCache` call emptied
    /// reports `FLUSH_IN_PROGRESS` over [`CACHE_CLUSTER_FLUSH_SPAN`] the same way, and is
    /// `AVAILABLE` again once that instant has passed. A cluster an `UpdateStage`
    /// disabled is torn down over [`CACHE_CLUSTER_DELETE_SPAN`], reporting
    /// `DELETE_IN_PROGRESS` until then and `NOT_AVAILABLE` after it.
    pub fn cache_cluster_status(&self) -> &'static str {
        let now = ::std::time::Instant::now();
        // A teardown the clock owes outranks the cluster that is going away: the stage
        // reports the removal until it is due, and `NOT_AVAILABLE` from then on.
        if let Some(removed_at) = self.cache_cluster_removed_at {
            return if now < removed_at {
                CACHE_CLUSTER_DELETE_IN_PROGRESS
            } else {
                CACHE_CLUSTER_NOT_AVAILABLE
            };
        }
        match self.cache_cluster_ready_at {
            None => CACHE_CLUSTER_NOT_AVAILABLE,
            Some(ready_at) if now < ready_at => CACHE_CLUSTER_CREATE_IN_PROGRESS,
            Some(_) => {
                if self.cache_flush_done_at.is_some_and(|done_at| now < done_at) {
                    CACHE_CLUSTER_FLUSH_IN_PROGRESS
                } else {
                    CACHE_CLUSTER_AVAILABLE
                }
            }
        }
    }

    /// The response this stage's cache is holding under `key`, or `None` when it holds
    /// none or the one it holds has stopped being usable.
    ///
    /// A stage only serves from a cache cluster that is up, which is the caller's check,
    /// not this one's: what this answers is what the cluster HOLDS. An entry whose
    /// `cacheTtlInSeconds` has passed is no longer one of them — the expiry is a
    /// transition the CLOCK owes, so it is materialised from the stored due instant here
    /// rather than by a timer, and the entry itself is reclaimed the next time this stage
    /// caches anything ([`Self::cache_response`]).
    pub fn cached_response(&self, key: &str) -> Option<&CachedResponse> {
        let now = ::std::time::Instant::now();
        self.response_cache
            .get(key)
            .filter(|held| now < held.expires_at)
    }

    /// Hold `response` under `key`, as the stage's cache cluster holds a served response.
    ///
    /// Every entry whose TTL has passed is dropped first — nothing else reclaims them —
    /// and a cluster that is at its bound makes room by dropping the entry due to expire
    /// first, which is the one that would be a miss soonest anyway. See
    /// [`MAX_CACHED_RESPONSES_PER_STAGE`] for why there is a bound at all.
    pub fn cache_response(&mut self, key: String, response: CachedResponse) {
        let now = ::std::time::Instant::now();
        self.response_cache.retain(|_, held| now < held.expires_at);
        while self.response_cache.len() >= MAX_CACHED_RESPONSES_PER_STAGE
            && !self.response_cache.contains_key(&key)
        {
            let Some(dropped) = self
                .response_cache
                .iter()
                .min_by_key(|(_, held)| held.expires_at)
                .map(|(held_under, _)| held_under.clone())
            else {
                break;
            };
            self.response_cache.remove(&dropped);
        }
        self.response_cache.insert(key, response);
    }

    /// Drop every response this stage's cache is holding: what `FlushStageCache` empties,
    /// and what a cache cluster that is re-provisioned comes up with.
    pub fn flush_response_cache(&mut self) {
        self.response_cache.clear();
    }

    /// The authorizer result this stage is holding under `key`, or `None` when it holds
    /// none, the one it holds has stopped being usable, or a flush has dropped it.
    ///
    /// Two things end an entry, both materialised here from stored instants rather than by
    /// a timer: its own `authorizerResultTtlInSeconds` passing, and a
    /// `FlushStageAuthorizersCache` that ran after it was stored
    /// ([`Self::authorizers_cache_flushed_at`] is the stage's invalidation watermark, so an
    /// entry cached before it is no longer one the stage holds).
    pub fn cached_authorizer_result(&self, key: &str) -> Option<&CachedAuthorizerResult> {
        let now = ::std::time::Instant::now();
        self.authorizer_results.get(key).filter(|held| {
            now < held.expires_at
                && self
                    .authorizers_cache_flushed_at
                    .is_none_or(|flushed_at| flushed_at <= held.cached_at)
        })
    }

    /// Hold `result` under `key`, as a stage holds the result an authorizer returned for
    /// one identity.
    ///
    /// Every entry whose TTL has passed is dropped first — nothing else reclaims them — and
    /// a stage at its bound makes room by dropping the entry due to expire first, which is
    /// the one that would be a miss soonest anyway. See
    /// [`MAX_CACHED_AUTHORIZER_RESULTS_PER_STAGE`] for why there is a bound at all.
    pub fn cache_authorizer_result(&mut self, key: String, result: CachedAuthorizerResult) {
        let now = ::std::time::Instant::now();
        self.authorizer_results
            .retain(|_, held| now < held.expires_at);
        while self.authorizer_results.len() >= MAX_CACHED_AUTHORIZER_RESULTS_PER_STAGE
            && !self.authorizer_results.contains_key(&key)
        {
            let Some(dropped) = self
                .authorizer_results
                .iter()
                .min_by_key(|(_, held)| held.expires_at)
                .map(|(held_under, _)| held_under.clone())
            else {
                break;
            };
            self.authorizer_results.remove(&dropped);
        }
        self.authorizer_results.insert(key, result);
    }
}

/// One result an `Authorizer` returned for one identity, as the `execute-api` data plane
/// stored it.
///
/// What is held is the authorizer's OWN return value — the principal it named and the IAM
/// policy document it returned, with the context it published — rather than the decision
/// taken from it: the policy is what API Gateway caches ("the TTL in seconds of cached
/// authorizer results"), and it is evaluated against each method the identity goes on to
/// address, so one held result admits a call to one method and refuses another exactly as
/// the document says.
#[derive(Debug, Clone)]
pub struct CachedAuthorizerResult {
    /// The `principalId` the authorizer named for this identity, which the integration
    /// reads as `$context.authorizer.principalId`.
    pub principal_id: String,
    /// The IAM policy document the authorizer returned, verbatim.
    pub policy_document: String,
    /// The `context` map the authorizer published, each entry of which the integration
    /// reads as `$context.authorizer.{key}`.
    pub context: BTreeMap<String, String>,
    /// When this result was stored, which is what a later flush of the stage's authorizer
    /// cache is compared against ([`StageEntry::cached_authorizer_result`]).
    pub cached_at: ::std::time::Instant,
    /// When it stops being usable: when it was stored plus the authorizer's
    /// `authorizerResultTtlInSeconds`. A read at or after this instant is a MISS, so the
    /// authorizer is called again.
    pub expires_at: ::std::time::Instant,
}

/// One response a stage's cache cluster is holding for a method request, as the
/// `execute-api` data plane stored it.
///
/// The WHOLE response is kept, because that is what a cache hit serves: the response the
/// integration produced, served again, rather than a response derived a second time from
/// the records.
#[derive(Debug, Clone)]
pub struct CachedResponse {
    /// The status of the response the integration produced.
    pub status: u16,
    /// The media type that response was served under.
    pub content_type: String,
    /// Its body, verbatim.
    pub body: Vec<u8>,
    /// The other headers it carried, keyed by the header name.
    pub headers: BTreeMap<String, String>,
    /// When this entry stops being usable: when it was stored plus the method setting's
    /// `cacheTtlInSeconds` ("the time to live (TTL), in seconds, for cached responses").
    /// A read at or after this instant is a MISS; see [`StageEntry::cached_response`].
    pub expires_at: ::std::time::Instant,
}

/// One stored `VpcLink`: a named set of target ARNs a private integration reaches a VPC
/// through, with the provisioning status the clock moves.
#[derive(Debug, Clone, Default)]
pub struct VpcLinkEntry {
    /// The VPC link id, which is also this record's key in
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::vpc_links`].
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// The target ARNs the request supplied, in the order it listed them: "The ARN of
    /// the network load balancer of the VPC targeted by the VPC link"
    /// (`VpcLink$targetArns`).
    pub target_arns: Vec<String>,
    /// When this link finishes being provisioned. It is reported [`VPC_LINK_PENDING`]
    /// until this instant and settled from it on; see [`VPC_LINK_PROVISION_SPAN`] and
    /// [`Self::status`].
    pub available_at: Option<::std::time::Instant>,
    /// Why provisioning this link cannot succeed, when it cannot: the link settles
    /// [`VPC_LINK_FAILED`] instead of [`VPC_LINK_AVAILABLE`] and reports this as its
    /// `statusMessage`. `None` for a link that provisions.
    pub failure: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// This link's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order. Read by
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::vpc_link_ids_in_creation_order`],
    /// the stable sequence [`page_of`] cuts a `GetVpcLinks` page out of.
    pub sequence: u64,
}

impl VpcLinkEntry {
    /// The `status` this link reports now.
    ///
    /// `CreateVpcLink` is "an asynchronous operation that typically takes 2-4 minutes to
    /// complete and become operational" (the operation's own documentation), so a link
    /// is created [`VPC_LINK_PENDING`] and SETTLES once [`VPC_LINK_PROVISION_SPAN`] has
    /// passed — a transition the CLOCK owes, materialised from the stored due instant
    /// rather than by sleeping or by being read, the way
    /// [`StageEntry::cache_cluster_status`] materialises its own.
    ///
    /// Which state it settles in is decided by what the link targets: a link whose
    /// provisioning cannot succeed settles [`VPC_LINK_FAILED`] (see [`Self::failure`]),
    /// and one that can settles [`VPC_LINK_AVAILABLE`].
    pub fn status(&self) -> &'static str {
        match self.available_at {
            Some(available_at) if ::std::time::Instant::now() < available_at => VPC_LINK_PENDING,
            _ if self.failure.is_some() => VPC_LINK_FAILED,
            _ => VPC_LINK_AVAILABLE,
        }
    }

    /// The `statusMessage` this link reports now: the reason it could not be provisioned,
    /// once it has settled [`VPC_LINK_FAILED`].
    ///
    /// A link that is still [`VPC_LINK_PENDING`], or that provisioned, reports none: AWS
    /// publishes no sentence a provisioning or provisioned link carries, so nothing is
    /// reported rather than a message this emulator made up.
    pub fn status_message(&self) -> Option<String> {
        (self.status() == VPC_LINK_FAILED)
            .then(|| self.failure.clone())
            .flatten()
    }
}

/// One stored `Deployment` of an API: the snapshot of the API's methods at the instant
/// it was created, which a `Stage` points at.
#[derive(Debug, Clone, Default)]
pub struct DeploymentEntry {
    pub id: String,
    pub description: Option<String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    /// The `apiSummary` this deployment reports: resource path -> HTTP verb -> the
    /// method as it stood when the deployment was created.
    pub api_summary: BTreeMap<String, BTreeMap<String, MethodSnapshot>>,
    /// The API's resource tree as it stood when this deployment was created, keyed by
    /// resource id exactly as [`RestApiEntry::resources`] is.
    ///
    /// A deployment is a SNAPSHOT: "a Deployment resource is a snapshot of your REST API
    /// configuration", and a stage serves the deployment it points at, so this — and not
    /// the live tree — is what the execute-api data plane
    /// ([`crate::service::execute_api`]) matches a request against. An integration put on
    /// a method after this deployment was created is therefore not served until the API
    /// is redeployed, which is what AWS does. `apiSummary` cannot stand in for it: the
    /// modelled `MethodSnapshot` carries only `authorizationType` and `apiKeyRequired`.
    pub resources: BTreeMap<String, ResourceEntry>,
    /// This deployment's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`].
    pub sequence: u64,
}

/// One stored `DocumentationPart`: the API entity the documentation applies to, and the
/// content map describing it.
///
/// `location` is kept as the modelled [`DocumentationPartLocation`] rather than a second
/// hand-written mirror of its five members, for the reason the module docs give — and it
/// is stored COMPLETED (the members the entity type makes valid, defaulted where the
/// model documents a default), so every read of the part reports the same location.
#[derive(Debug, Clone, Default)]
pub struct DocumentationPartEntry {
    pub id: String,
    pub location: DocumentationPartLocation,
    /// The content map, as the JSON text the request supplied: "The map must be encoded
    /// as a JSON string" (`DocumentationPart$properties`).
    pub properties: String,
    /// This part's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing reports creation order rather than id order.
    pub sequence: u64,
}

/// One stored `DocumentationVersion`: "a snapshot of the documentation of an API"
/// (`DocumentationVersion` in this service's model), under the version identifier the
/// create minted it as.
///
/// The snapshot is the API's documentation parts as they stood at that instant, kept on
/// the record rather than recomputed from the API's current parts: later calls go on
/// creating, updating and deleting parts, and a snapshot that followed them would not be
/// a snapshot.
#[derive(Debug, Clone, Default)]
pub struct DocumentationVersionEntry {
    /// The version identifier, which is also this record's key in
    /// [`RestApiEntry::documentation_versions`].
    pub version: String,
    pub description: Option<String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    /// The documentation parts this snapshot froze, in the creation order they had then;
    /// what `GetExport` serves for this version once it is implemented.
    pub documentation_parts: Vec<DocumentationPartEntry>,
    /// This version's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing reports creation order rather than version-identifier order.
    pub sequence: u64,
}

/// One stored `Model` of an API: "the data structure of a method request or response
/// payload", with the schema that describes it.
///
/// `schema` is optional because `CreateModel$schema` is: a model created without one
/// carries none, and a read of it reports none rather than a schema invented per read.
/// `content_type` is not optional, because `CreateModel$contentType` is `@required`, so a
/// stored model always has one.
#[derive(Debug, Clone, Default)]
pub struct ModelEntry {
    pub id: String,
    /// The model's name, which is also this record's key in [`RestApiEntry::models`].
    pub name: String,
    pub description: Option<String>,
    pub content_type: String,
    pub schema: Option<String>,
    /// This model's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than name order.
    pub sequence: u64,
}

/// One stored `Method` on a resource.
#[derive(Debug, Clone, Default)]
pub struct MethodEntry {
    pub http_method: String,
    pub authorization_type: String,
    pub authorizer_id: Option<String>,
    pub api_key_required: bool,
    pub operation_name: Option<String>,
    pub request_parameters: BTreeMap<String, bool>,
    pub request_models: BTreeMap<String, String>,
    pub request_validator_id: Option<String>,
    pub authorization_scopes: Vec<String>,
    /// Written by `PutMethodResponse` and reported by `GetMethodResponse` and a `Method`
    /// body's `methodResponses`, keyed by the response's `statusCode`; empty until a
    /// response is put on the method.
    pub method_responses: BTreeMap<String, GetMethodResponseOutput>,
    /// Written by `PutIntegration` and reported by `GetIntegration` and a `Method`
    /// body's `methodIntegration`; absent until an integration is put on the method.
    ///
    /// Its `integrationResponses` map is where `PutIntegrationResponse` writes and
    /// `GetIntegrationResponse` reads, keyed by the response's `statusCode`: an
    /// integration response hangs off the INTEGRATION, so the one record lives on the
    /// integration that owns it rather than in a second map beside it.
    pub integration: Option<GetIntegrationOutput>,
}

/// One stored `Authorizer` of an API: the custom authorization an API's methods may
/// name in their `authorizerId`.
///
/// `authorizer_result_ttl_in_seconds` is NOT optional here even though the request
/// member is: the service picks [`DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS`] when the
/// request sets nothing, so a stored authorizer always has a TTL. Everything else is
/// kept exactly as the request set it, so a reader reports what was stored rather than a
/// value invented per read.
#[derive(Debug, Clone, Default)]
pub struct AuthorizerEntry {
    pub id: String,
    pub name: String,
    /// One of [`crate::ops::types::AUTHORIZER_TYPE_VALUES`]; the boundary validator
    /// refuses anything else before a handler runs.
    pub r#type: String,
    pub provider_arns: Vec<String>,
    pub auth_type: Option<String>,
    pub authorizer_uri: Option<String>,
    pub authorizer_credentials: Option<String>,
    pub identity_source: Option<String>,
    pub identity_validation_expression: Option<String>,
    /// The TTL in seconds of cached authorizer results; `0` disables caching.
    pub authorizer_result_ttl_in_seconds: i32,
    /// This authorizer's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
}

/// One stored `RequestValidator` of an API: the pair of validation flags an API's
/// methods may name in their `requestValidatorId`.
///
/// `name` is optional on the request and so optional here: a validator created without
/// one carries none, and a read of it reports none rather than a name invented per read.
/// The two flags are NOT optional, because `validateRequestBody` and
/// `validateRequestParameters` are modelled with a default of `false`, so a stored
/// validator always has both.
#[derive(Debug, Clone, Default)]
pub struct RequestValidatorEntry {
    pub id: String,
    pub name: Option<String>,
    pub validate_request_body: bool,
    pub validate_request_parameters: bool,
    /// This validator's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
}

/// One stored `ApiKey`: the identifier the control plane assigns, the key value a
/// client presents in `x-api-key`, and the record's own attributes.
#[derive(Debug, Clone, Default)]
pub struct ApiKeyEntry {
    pub id: String,
    /// The key's secret value. Minted by the emulator's credential store (see
    /// [`mint_api_key_value`]) or supplied by the caller through `CreateApiKey$value`,
    /// and reported only where the model says so — `CreateApiKey`, and `GetApiKey`
    /// when the request asks for it with `includeValue`.
    pub value: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub customer_id: Option<String>,
    pub enabled: bool,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    pub last_updated_date: Option<::aws_smithy_types::DateTime>,
    /// The stages this key is associated with, each rendered `<restApiId>/<stageName>`
    /// as the modelled `ApiKey$stageKeys` list of strings reports them.
    pub stage_keys: Vec<String>,
    /// The labels an `UpdateApiKey` patch of `/labels` adds to and removes from the key.
    ///
    /// Stored and reported by nothing: AWS accepts `/labels` as a collection path on an
    /// API key (a recorded refusal lists it beside `/stages`), while the `ApiKey` shape
    /// this service's model declares carries no member for it — so the patch is applied
    /// to the record and no response renders it.
    pub labels: Vec<String>,
    pub tags: BTreeMap<String, String>,
    /// This key's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
}

/// One stored `UsagePlan`: the throttle and quota limits a set of API keys is metered
/// against, and the API stages those limits apply to.
///
/// `throttle`, `quota` and the per-method throttle map inside each `api_stages` entry are
/// kept as the modelled shapes from [`crate::ops::types`] rather than as a second
/// hand-written mirror of their members, for the reason the module docs give. Each is
/// `Option` because a usage plan need not have one: a plan created without a `quota` has
/// no quota at all, which is what distinguishes it from a plan whose quota limit is zero.
///
/// `product_code` is the Amazon Web Services Marketplace product a plan is sold as. It is
/// not a member of `CreateUsagePlan`'s input, so a created plan carries none and a read
/// reports none rather than a code this emulator would have to invent; `UpdateUsagePlan`
/// is what associates one, through a patch of the plan's `productCode`.
#[derive(Debug, Clone, Default)]
pub struct UsagePlanEntry {
    pub id: String,
    /// `CreateUsagePlanRequest$name` is `@required`, so a stored plan always has one.
    pub name: String,
    pub description: Option<String>,
    /// The API stages the plan's limits apply to, each naming a stored stage of a stored
    /// API at the instant the plan was created or last updated.
    pub api_stages: Vec<ApiStage>,
    pub throttle: Option<ThrottleSettings>,
    pub quota: Option<QuotaSettings>,
    /// See the struct docs: absent on a created plan, written by `UpdateUsagePlan`.
    pub product_code: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The API keys metered against this plan, keyed by the API key id each one names —
    /// the `keyId` a `UsagePlanKey` is addressed by. A key is in a plan at most once, so
    /// the association is a map keyed by that id rather than a list.
    ///
    /// Bounded by [`MAX_USAGE_PLAN_KEYS_PER_PLAN`] and reclaimed with the key it names:
    /// `DeleteApiKey` drops the deleted key's association from every plan (see
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::forget_api_key`]), because a
    /// deleted key is metered by no plan. A read of this map still joins it against
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::api_keys`] through
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::usage_plan_key`] rather than
    /// enumerating it directly, so no read can report a key that no longer exists.
    pub keys: BTreeMap<String, UsagePlanKeyEntry>,
    /// The UTC day this plan was created on: the "initial time period" its quota's
    /// `offset` is subtracted from the limit in ("the number of requests subtracted from
    /// the given limit in the initial time period", `QuotaSettings$offset`), so every
    /// period after the one the plan was created in starts at the whole limit again. See
    /// [`quota_window`], which is where it is read.
    ///
    /// Not a member of any modelled `UsagePlan` body and reported by no operation: the
    /// plan's own creation day is what anchors the metering, not something a caller sets
    /// or reads. `Option` only so the struct keeps the `Default` its siblings have.
    pub created_on: Option<::chrono::NaiveDate>,
    /// This plan's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
}

/// One stored `UsagePlanKey`: the association between a stored [`UsagePlanEntry`] and a
/// stored [`ApiKeyEntry`], held on the plan that meters the key.
///
/// The association stores only what it OWNS — which key it names and the type that key
/// was added as. The `UsagePlanKey` body's `name` and `value` are the API key's own, so
/// they are read from the [`ApiKeyEntry`] at report time rather than copied here: two
/// copies of one key's name is how a renamed key ends up reported two ways.
///
/// No `Default`: an association with no key id names nothing, so there is no meaningful
/// default one — every one is built from the request that added it.
#[derive(Debug, Clone)]
pub struct UsagePlanKeyEntry {
    /// The API key id this association names, which is also the `UsagePlanKey`'s own
    /// `id`: a usage plan key is addressed by the key it covers.
    pub key_id: String,
    /// The type the key was added as; always [`USAGE_PLAN_KEY_TYPE`], the one type the
    /// model declares valid.
    pub key_type: String,
    /// This association's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report the order keys were added in rather than id order.
    pub sequence: u64,
}

/// The one key type a `UsagePlanKey` takes: from `UsagePlanKey$type`'s own documentation,
/// "Currently, the valid key type is `API_KEY`."
pub const USAGE_PLAN_KEY_TYPE: &str = "API_KEY";

/// The key one `(usage plan, API key)` pair's throttle accounting is held under in
/// [`ComAmazonawsApigatewayBackplanecontrolserviceState::throttle_buckets`].
///
/// Both identifiers, because the limits belong to the PLAN and are applied to EACH of its
/// keys: two keys of one plan are admitted independently, and one key metered by two plans
/// is metered against each plan's own limits.
pub fn metering_key(usage_plan_id: &str, key_id: &str) -> String {
    format!("{usage_plan_id}/{key_id}")
}

/// The request rate a usage plan's `throttle` admits, as the token bucket that meters it
/// reads the pair: the bucket's capacity and the rate it refills at.
///
/// `ThrottleSettings` documents the pair as "the API request rate limits" —
/// `rateLimit` "the API request steady-state rate limit" and `burstLimit` "the API request
/// burst limit", which is the capacity "provided by the token bucket algorithm" the
/// account-level limits ([`DEFAULT_ACCOUNT_THROTTLE`]) are published as. A limit that is
/// not POSITIVE is no limit: both "must be numeric and greater than zero"
/// ([`INVALID_THROTTLE_SETTINGS`]), so a plan whose throttle sets neither (the structure's
/// own zeroes) throttles nothing rather than refusing everything.
#[derive(Debug, Clone, Copy)]
pub struct ThrottleLimits {
    /// The most requests admissible at once — the bucket's capacity.
    pub burst: f64,
    /// The requests per second the bucket refills at.
    pub rate: f64,
}

impl ThrottleLimits {
    /// The limits `throttle` declares, or `None` when it declares none that limits
    /// anything.
    pub fn declared(throttle: Option<&ThrottleSettings>) -> Option<Self> {
        let throttle = throttle?;
        let burst = f64::from(throttle.burst_limit);
        let rate = throttle.rate_limit;
        (burst > 0.0 && rate.is_finite() && rate > 0.0).then_some(Self { burst, rate })
    }
}

/// The token bucket one metered `(usage plan, API key)` pair's requests are admitted
/// through.
///
/// A bucket starts FULL — a key that has sent nothing may burst up to the plan's burst
/// limit — and refills at the plan's rate, so the limits a plan is updated to take effect
/// on the next request rather than on a bucket frozen at the old ones.
#[derive(Debug, Clone)]
pub struct ThrottleBucket {
    /// The requests still admissible as of [`Self::refilled_at`].
    tokens: f64,
    /// The instant `tokens` was last computed at.
    refilled_at: ::std::time::Instant,
}

impl ThrottleBucket {
    /// A bucket holding `limits.burst` requests as of `now`.
    fn full(limits: ThrottleLimits, now: ::std::time::Instant) -> Self {
        Self {
            tokens: limits.burst,
            refilled_at: now,
        }
    }

    /// Admit one request and record it, or report that `limits` admit none at `now`.
    ///
    /// The refill is the time since the last request times the rate, capped at the burst
    /// limit: the bucket cannot hold more than the capacity however long a key has been
    /// idle, which is what makes the steady-state rate the rate.
    fn admits(&mut self, limits: ThrottleLimits, now: ::std::time::Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.refilled_at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * limits.rate).min(limits.burst);
        self.refilled_at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// The `BadRequestException` message for a `keyType` that is not [`USAGE_PLAN_KEY_TYPE`].
///
/// A SENTENCE OF ITS OWN, like [`INVALID_REST_API_IDENTIFIER`]: the rejected value is not
/// quoted back. What it carries is the set of types the request COULD have named — the
/// types the model declares, so the list is [`USAGE_PLAN_KEY_TYPE`] itself rather than a
/// second spelling of it — because that set is what the caller has to choose from.
pub fn invalid_usage_plan_key_type() -> String {
    format!("Invalid Usage Plan key type specified. Must be one of [{USAGE_PLAN_KEY_TYPE}]")
}

/// The `ConflictException` message for a key already metered by the plan.
///
/// A SENTENCE OF ITS OWN, like [`INVALID_REST_API_IDENTIFIER`]: API Gateway states one
/// fixed sentence for this conflict and quotes back neither the plan nor the key, so the
/// pair the caller already named in the request is not repeated into the message.
pub const USAGE_PLAN_KEY_ALREADY_EXISTS: &str = "API Key is already subscribed to Usage Plan";

/// The `ConflictException` message for a status code the addressed method already answers
/// a response for.
///
/// A SENTENCE OF ITS OWN, like [`USAGE_PLAN_KEY_ALREADY_EXISTS`]: it has no variable part,
/// so neither the status code nor the verb the caller sent is quoted back.
pub const METHOD_RESPONSE_ALREADY_EXISTS: &str = "Response already exists for this resource";

/// The `LimitExceededException` message for a plan that already meters as many API keys
/// as this emulator stores for one plan; see [`MAX_USAGE_PLAN_KEYS_PER_PLAN`].
pub fn too_many_usage_plan_keys() -> String {
    quota_reached("API Keys", "Usage Plan")
}

/// The `LimitExceededException` message for a quota of `resources` that has been reached
/// in `scope`.
///
/// API Gateway reports a reached quota with ONE fixed sentence: it names the resources the
/// quota counts and the scope it counts them in, and points the caller at AWS for a raise.
/// The LIMIT ITSELF is not in it — a recorded refusal of the per-API stage quota reads
/// `Maximum number of Stages for this API has been reached.  Please contact AWS if you
/// need additional Stages.`, with no number anywhere — so neither the count nor the cap
/// is quoted back here. One copy for every quota this service enforces, so no two of them
/// word the same kind of refusal differently.
pub fn quota_reached(resources: &str, scope: &str) -> String {
    format!(
        "Maximum number of {resources} for this {scope} has been reached.  \
         Please contact AWS if you need additional {resources}."
    )
}

/// One stored custom `DomainName`: the name a client reaches an API by, the certificate
/// and endpoint configuration it was created with, and the endpoint names API Gateway
/// assigned it.
///
/// `distribution*` and `regional*` are the endpoint the domain name is served by, and a
/// domain name has exactly the one its endpoint type names: the `types` member documents
/// `EDGE` for "an edge-optimized API and its custom domain name" and `REGIONAL` for "a
/// regional API and its custom domain name", so an edge-optimized record carries the
/// CloudFront distribution's names and a regional one the regional endpoint's. A PRIVATE
/// record carries NEITHER: it is reached through the VPC endpoints its endpoint
/// configuration names (`vpcEndpointIds` is "only supported for `PRIVATE` endpoint
/// type"), not through a public alias. Whichever it has is assigned by the service when
/// the record is created and stored, so every later read reports the same values.
///
/// `regionalHostedZoneId` is NOT among them: it is the Route 53 hosted zone of the
/// Region's API Gateway endpoint, a per-Region constant this service's model carries no
/// value for (the member's doc points at the AWS General Reference table instead), so a
/// stored record reports none rather than a zone id this emulator would have to invent.
/// `distributionHostedZoneId`, whose single value the model DOES state, is stored.
#[derive(Debug, Clone, Default)]
pub struct DomainNameEntry {
    pub domain_name: String,
    /// The identifier API Gateway assigns a PRIVATE custom domain name
    /// (`domainNameId` is "supported only for private custom domain names"), minted from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::mint_id`]; absent for an
    /// edge-optimized or regional one.
    pub domain_name_id: Option<String>,
    pub domain_name_arn: String,
    pub certificate_name: Option<String>,
    pub certificate_arn: Option<String>,
    /// When the certificate this domain name uses was uploaded: the instant the record
    /// was created, and only for a record created WITH a certificate.
    pub certificate_upload_date: Option<::aws_smithy_types::DateTime>,
    pub regional_domain_name: Option<String>,
    pub regional_certificate_name: Option<String>,
    pub regional_certificate_arn: Option<String>,
    pub distribution_domain_name: Option<String>,
    /// `Z2FDTNDATAQYW2` for an edge-optimized domain name, absent otherwise; see
    /// [`EDGE_DISTRIBUTION_HOSTED_ZONE_ID`].
    pub distribution_hosted_zone_id: Option<String>,
    /// What the request set, completed by [`defaulted_endpoint_configuration`], so the
    /// stored configuration always names its endpoint types and its IP address type.
    pub endpoint_configuration: Option<EndpointConfiguration>,
    /// When this domain name's endpoint finishes being set up, after which it reports
    /// [`DOMAIN_NAME_AVAILABLE`]; absent for a domain name that was ready when it was
    /// created. See [`DomainNameEntry::status`] and [`DOMAIN_NAME_CREATE_SPAN`].
    pub available_at: Option<::std::time::Instant>,
    /// When the endpoint migration an update started is complete, until which the record
    /// reports [`DOMAIN_NAME_UPDATING`]; absent for a domain name no update has moved to
    /// another endpoint type. See [`DomainNameEntry::status`] and
    /// [`DOMAIN_NAME_MIGRATION_SPAN`].
    pub updating_until: Option<::std::time::Instant>,
    pub domain_name_status_message: Option<String>,
    /// What the request set, or [`DEFAULT_SECURITY_POLICY`]: the service picks a value
    /// at create time, so a stored domain name always has one.
    pub security_policy: Option<String>,
    pub endpoint_access_mode: Option<String>,
    pub mutual_tls_authentication: Option<MutualTlsAuthentication>,
    pub ownership_verification_certificate_arn: Option<String>,
    pub management_policy: Option<String>,
    pub policy: Option<String>,
    pub routing_mode: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The `BasePathMapping`s created against this domain name, keyed by the base path
    /// each one answers to — [`base_path_key`], which is also the `basePath` path label
    /// `GetBasePathMapping` addresses one by.
    ///
    /// A base path mapping is a record OF the domain name it was created against (it is
    /// created, read, listed and deleted under `/domainnames/{domainName}`), and the base
    /// path "must be unique for all of the mappings across a single API" of that domain
    /// name, so the mappings live INSIDE the record rather than in a second map beside the
    /// domain names: the key is unique exactly where the uniqueness rule applies, and a
    /// deleted domain name takes its mappings with it instead of leaving orphans a later
    /// create of the same name would inherit.
    pub base_path_mappings: BTreeMap<String, BasePathMappingEntry>,
    /// This domain name's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than key order.
    pub sequence: u64,
}

impl DomainNameEntry {
    /// The `domainNameStatus` this domain name reports now.
    ///
    /// An edge-optimized domain name is served by a CloudFront distribution API Gateway
    /// creates for it, which is not in place when the create call answers, so the record
    /// is reported [`DOMAIN_NAME_PENDING`] until [`DOMAIN_NAME_CREATE_SPAN`] has passed
    /// and [`DOMAIN_NAME_AVAILABLE`] after it; a regional or private domain name is
    /// ready when it is created and has no span to wait out. The transition is one the
    /// CLOCK owes, materialised from the stored due instant rather than by sleeping or
    /// by being read.
    ///
    /// An update that moves the record to another endpoint type starts the migration the
    /// status is named for — "the status of the DomainName migration" — so such a record
    /// reports [`DOMAIN_NAME_UPDATING`] until that migration is due, which is the status
    /// the member documents for a domain name whose existing operation is not complete.
    pub fn status(&self) -> &'static str {
        let now = ::std::time::Instant::now();
        if self.updating_until.is_some_and(|due| now < due) {
            return DOMAIN_NAME_UPDATING;
        }
        match self.available_at {
            None => DOMAIN_NAME_AVAILABLE,
            Some(available_at) if now < available_at => DOMAIN_NAME_PENDING,
            Some(_) => DOMAIN_NAME_AVAILABLE,
        }
    }

    /// The key this record is stored under; see [`domain_name_key`].
    pub fn key(&self) -> String {
        domain_name_key(&self.domain_name, self.domain_name_id.as_deref())
    }

    /// The base paths of this domain name's mappings in creation order: the stable
    /// sequence a `position` cursor walks and [`page_of`] cuts a `GetBasePathMappings`
    /// page out of.
    ///
    /// The rank is the [`BasePathMappingEntry::sequence`] the create stamped on each
    /// mapping — the same rank every other collection of this service is ordered by — so
    /// the listing reports the order the mappings were created in rather than the
    /// base-path order [`Self::base_path_mappings`] happens to hold them in; the base path
    /// breaks a tie no create can actually produce, which keeps the order total. A base
    /// path IS the key of the mapping it names, so the sequence is also what
    /// `GetBasePathMapping` addresses each listed record by.
    pub fn base_paths_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&BasePathMappingEntry> = self.base_path_mappings.values().collect();
        ordered.sort_by(|left, right| {
            (left.sequence, left.base_path.as_str())
                .cmp(&(right.sequence, right.base_path.as_str()))
        });
        ordered
            .into_iter()
            .map(|mapping| mapping.base_path.clone())
            .collect()
    }
}

/// One stored `BasePathMapping`: the base path of a custom domain name, and the REST API
/// (and, when the mapping names one, the stage of it) that a call arriving on that base
/// path is served by.
///
/// The record stores only what it OWNS — the three members `BasePathMapping` models. The
/// API's and the stage's own records are NOT copied here: the mapping names them by
/// identifier, which is what it is, and two copies of one stage is how a reader ends up
/// reporting a stale one.
#[derive(Debug, Clone, Default)]
pub struct BasePathMappingEntry {
    /// The base path this mapping answers to, which is also its key in
    /// [`DomainNameEntry::base_path_mappings`]; never empty, because an absent base path
    /// is stored as [`NONE_BASE_PATH`]. See [`base_path_key`].
    pub base_path: String,
    /// "The string identifier of the associated RestApi" — the id of a stored
    /// [`RestApiEntry`] at the time the mapping was created.
    pub rest_api_id: String,
    /// "The name of the associated stage": a stage of that API, or `None` for a mapping
    /// created without one, which is what distinguishes a mapping that names no stage
    /// from one that names a stage called something.
    pub stage: Option<String>,
    /// This mapping's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than base-path order.
    pub sequence: u64,
}

/// WHICH stored `BasePathMapping` one is: the custom domain name that holds it (by the
/// [`domain_name_key`] it is addressed under) and the base path it answers to.
///
/// A mapping has no identifier of its own — it is addressed by its domain name and its
/// base path — so this pair is how a refusal names the mapping that still stands in the
/// way of a delete, the way [`MethodReference`] names the method that still references an
/// authorizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasePathMappingReference {
    /// The key of the [`DomainNameEntry`] that holds the mapping; see [`domain_name_key`].
    pub domain_name: String,
    /// The mapping's base path, which is its address under that domain name.
    pub base_path: String,
}

impl ::std::fmt::Display for BasePathMappingReference {
    /// The mapping as a refusal names it: the domain name and the base path, joined the
    /// way the URL the mapping serves joins them, because a base path is unique only
    /// within one domain name. One terse composite, as this service's other in-use
    /// refusals render the reference that blocks a delete ([`MethodReference`]).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(formatter, "{}/{}", self.domain_name, self.base_path)
    }
}

/// The base path a `BasePathMapping` is stored and addressed under, from the (possibly
/// absent) `basePath` a request carried.
///
/// API Gateway spells the EMPTY base path — the mapping a caller reaches by naming the
/// domain name and nothing after it — as the literal `(none)`: `basePath` is documented
/// as "Specify '(none)' if you do not want callers to specify any base path name after
/// the domain name", which is the spelling both the create and the read take and so the
/// one a stored record must carry. An absent or empty `basePath` is therefore the same
/// request as one naming `(none)`, and a mapping created either way is addressable both
/// ways; anything else the request names is the base path itself, kept as given.
pub fn base_path_key(base_path: Option<&str>) -> String {
    match base_path {
        Some(base_path) if !base_path.is_empty() => base_path.to_string(),
        _ => NONE_BASE_PATH.to_string(),
    }
}

/// How API Gateway spells the empty base path; see [`base_path_key`].
pub const NONE_BASE_PATH: &str = "(none)";

/// The key a stored [`DomainNameEntry`] is addressed by, and the cursor a
/// `GetDomainNames` page hands back.
///
/// A `domainNameId` is "supported only for private custom domain names", and a private
/// domain name needs it to be identified at all: AWS lets several private records carry
/// one name, so the name alone does not select one. The two members are therefore joined
/// into `<domainName>+<domainNameId>` — the same form this emulator writes into a
/// private domain name's ARN — while an edge-optimized or regional record, which has no
/// id, is keyed by its name alone.
pub fn domain_name_key(domain_name: &str, domain_name_id: Option<&str>) -> String {
    match domain_name_id {
        Some(domain_name_id) if !domain_name_id.is_empty() => {
            format!("{domain_name}+{domain_name_id}")
        }
        _ => domain_name.to_string(),
    }
}

/// One stored `DomainNameAccessAssociation`: the access association source a private
/// custom domain name may be invoked from, and the domain name it was created against.
///
/// The record stores only what it OWNS — the identifier the service minted for it, the
/// ARN that identifier spells, the pair of members that say WHICH source is associated
/// with WHICH domain name, and its tags. Nothing of the domain name's own record is
/// copied here: two copies of one domain name is how a reader ends up reporting a stale
/// one.
#[derive(Debug, Clone, Default)]
pub struct DomainNameAccessAssociationEntry {
    /// The identifier this record is keyed by, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::mint_id`].
    pub id: String,
    /// The ARN of this association, as
    /// [`domain_name_access_association_arn`] spells it: assigned when the record is
    /// created and stored, so every later read reports the same value.
    pub domain_name_access_association_arn: String,
    /// The ARN of the domain name this association is for, as the request named it.
    pub domain_name_arn: String,
    /// The type of the access association source; the one value the modelled
    /// `AccessAssociationSourceType` enum declares.
    pub access_association_source_type: String,
    /// The access association source itself — "For a VPCE, the value is the VPC endpoint
    /// ID" — as the request named it.
    pub access_association_source: String,
    pub tags: BTreeMap<String, String>,
    /// This association's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
}

// ---------------------------------------------------------------------------
// Service-side defaults for a created `RestApi` or custom `DomainName`
//
// Several members are optional on input and yet never absent on a created record: the
// service picks a value when the request sets none, stores it, and reports it from
// every read. The constants below are those values, and
// [`defaulted_endpoint_configuration`] is the one place the nested
// `EndpointConfiguration` is completed, so a record can never be stored
// half-defaulted. The endpoint configuration is shared because the model's own
// `EndpointConfiguration` is: it is "the endpoint configuration to indicate the types
// of endpoints an API (RestApi) or its custom domain name (DomainName) has".
// ---------------------------------------------------------------------------

/// The `apiKeySource` a created API carries when the request sets none: from the
/// member's own documentation, `HEADER` reads the key from the `X-API-Key` header, which
/// is where API Gateway looks unless a custom authorizer is named instead.
pub const DEFAULT_API_KEY_SOURCE: &str = "HEADER";

/// The `endpointConfiguration.types` entry a created API carries when the request sets
/// none: an edge-optimized API, which is API Gateway's default endpoint type (the
/// `types` member documents `EDGE` as "an edge-optimized API").
pub const DEFAULT_ENDPOINT_TYPE: &str = "EDGE";

/// The endpoint type whose only supported IP address type is [`DUALSTACK_ADDRESS_TYPE`];
/// from `EndpointConfiguration$ipAddressType`: "For the `PRIVATE` endpoint type, only
/// `dualstack` is supported."
pub const PRIVATE_ENDPOINT_TYPE: &str = "PRIVATE";

/// The endpoint type of a regional API and of its custom domain name, from the `types`
/// member's own documentation ("`REGIONAL` for regional API endpoint setup"). A domain
/// name of this type is served by the Region's API Gateway endpoint rather than by a
/// CloudFront distribution, which is what decides which endpoint members it carries.
pub const REGIONAL_ENDPOINT_TYPE: &str = "REGIONAL";

/// The `endpointConfiguration.ipAddressType` a created API carries when the request
/// sets none and the API is not private: `ipv4`, the first of the two documented values
/// ("Use `ipv4` to allow only IPv4 addresses to invoke an API").
pub const IPV4_ADDRESS_TYPE: &str = "ipv4";

/// The `ipAddressType` a private API carries when the request sets none; see
/// [`PRIVATE_ENDPOINT_TYPE`].
pub const DUALSTACK_ADDRESS_TYPE: &str = "dualstack";

/// The `securityPolicy` a created API carries when the request sets none: `TLS_1_0`, the
/// TLS version API Gateway starts an API on (and the first value of the modelled
/// `SecurityPolicy` enum). A created custom domain name is given the same value by the
/// same rule: the member is optional on input and reported from every read.
pub const DEFAULT_SECURITY_POLICY: &str = "TLS_1_0";

/// The `distributionHostedZoneId` of an edge-optimized custom domain name, from the
/// member's own documentation: "The region-agnostic Amazon Route 53 Hosted Zone ID of
/// the edge-optimized endpoint. The valid value is Z2FDTNDATAQYW2 for all the regions."
pub const EDGE_DISTRIBUTION_HOSTED_ZONE_ID: &str = "Z2FDTNDATAQYW2";

/// The `domainNameStatus` of a custom domain name whose endpoint is in place: one of the
/// two values `DomainName$domainNameStatus` documents ("The valid values are AVAILABLE
/// and UPDATING") and a value of the modelled `DomainNameStatus` enum.
pub const DOMAIN_NAME_AVAILABLE: &str = "AVAILABLE";

/// The `domainNameStatus` of a custom domain name whose endpoint is still being set up —
/// a value of the modelled `DomainNameStatus` enum
/// (`smithy_query(query="shape", shape="DomainNameStatus")`).
pub const DOMAIN_NAME_PENDING: &str = "PENDING";

/// The `domainNameStatus` of a custom domain name whose endpoint is being moved to
/// another endpoint type: the second of the two values `DomainName$domainNameStatus`
/// documents ("The valid values are AVAILABLE and UPDATING. If the status is UPDATING,
/// the domain cannot be modified further until the existing operation is complete").
pub const DOMAIN_NAME_UPDATING: &str = "UPDATING";

/// The `distributionDomainName` of an edge-optimized custom domain name: the CloudFront
/// distribution API Gateway puts in front of it, which is "the domain name of the Amazon
/// CloudFront distribution associated with this custom domain name" the caller points a
/// DNS record at. `id` is drawn by the store's own identifier mint and stored on the
/// record, so the name is assigned once and reported unchanged by every later read.
pub fn edge_distribution_domain_name(id: &str) -> String {
    format!("d{id}.cloudfront.net")
}

/// The `regionalDomainName` of a regional custom domain name: the Region's API Gateway
/// endpoint, which the member documents as "returned by API Gateway when you create a
/// regional endpoint" and which a caller points a DNS record at. The host label is
/// minted and stored the way [`edge_distribution_domain_name`]'s is.
pub fn regional_endpoint_domain_name(id: &str, region: &str) -> String {
    format!("d-{id}.execute-api.{region}.amazonaws.com")
}

/// Complete a request's (possibly absent) `endpointConfiguration` with the values the
/// service fills in, so what is stored is what every read of the record must report.
///
/// An absent or empty `types` becomes [`DEFAULT_ENDPOINT_TYPE`]; an absent
/// `ipAddressType` becomes [`DUALSTACK_ADDRESS_TYPE`] for a private endpoint, which
/// supports no other, and [`IPV4_ADDRESS_TYPE`] otherwise. Whatever the request DID set
/// — its endpoint types, its address type, its `vpcEndpointIds` — is kept as given.
///
/// Shared by `CreateRestApi` and `CreateDomainName`: the shape and its defaulting rule
/// are the model's own, declared for "an API (RestApi) or its custom domain name
/// (DomainName)".
pub fn defaulted_endpoint_configuration(
    requested: Option<EndpointConfiguration>,
) -> EndpointConfiguration {
    let mut configuration = requested.unwrap_or_default();
    let types = match configuration.types {
        Some(types) if !types.is_empty() => types,
        _ => vec![DEFAULT_ENDPOINT_TYPE.to_string()],
    };
    let private = types.iter().any(|kind| kind == PRIVATE_ENDPOINT_TYPE);
    configuration.ip_address_type.get_or_insert_with(|| {
        if private {
            DUALSTACK_ADDRESS_TYPE.to_string()
        } else {
            IPV4_ADDRESS_TYPE.to_string()
        }
    });
    configuration.types = Some(types);
    configuration
}

// ---------------------------------------------------------------------------
// Gateway responses
//
// Every API has a gateway response for EVERY modelled `GatewayResponseType` from the
// moment it is created: until a developer customizes one, it is the response API
// Gateway generates itself, reported with `defaultResponse: true`. Only the
// customizations are stored (`RestApiEntry::gateway_responses`); what an uncustomized
// type reports is DERIVED — from the catalog below and from the class default standing in
// for the type ([`reported_gateway_response_of`]) — so a read of an uncustomized type and
// a read of a customized one answer from one rule.
// ---------------------------------------------------------------------------

/// The gateway responses API Gateway generates for an API, IN THE ORDER the service
/// reports its collection in, each beside the HTTP status code its generated response
/// carries.
///
/// The ORDER is the collection's own: API Gateway reports the gateway responses of an API
/// in this sequence rather than by response type, and a client that indexes or pages the
/// collection positionally reads it that way. Source: the API Gateway REST API
/// reference's `GatewayResponses` example, which lists the whole collection of an API
/// nothing has been customized on.
///
/// The STATUS CODES come from the API Gateway Developer Guide's gateway response table
/// ("Setting up gateway responses to customize error responses"), which lists each
/// response type beside the status code of the response API Gateway returns for it. Both
/// are hand-written because the model carries the type's values and nothing else:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#GatewayResponseType")`
/// lists the 21 values alphabetically, with no status code and no collection order
/// attached, and `GatewayResponse$statusCode` documents only the member. `DEFAULT_4XX`
/// and `DEFAULT_5XX` are the two rows that carry NO status code of their own — each is a
/// fallback for a whole class of responses, which keep their own code — which is why
/// [`default_gateway_status_code`] answers an `Option`.
const GATEWAY_RESPONSE_CATALOG: &[(&str, Option<&str>)] = &[
    ("INTEGRATION_FAILURE", Some("504")),
    ("RESOURCE_NOT_FOUND", Some("404")),
    ("REQUEST_TOO_LARGE", Some("413")),
    ("THROTTLED", Some("429")),
    ("UNSUPPORTED_MEDIA_TYPE", Some("415")),
    ("AUTHORIZER_CONFIGURATION_ERROR", Some("500")),
    ("DEFAULT_5XX", None),
    ("DEFAULT_4XX", None),
    ("BAD_REQUEST_PARAMETERS", Some("400")),
    ("BAD_REQUEST_BODY", Some("400")),
    ("WAF_FILTERED", Some("403")),
    ("EXPIRED_TOKEN", Some("403")),
    ("ACCESS_DENIED", Some("403")),
    ("INVALID_API_KEY", Some("403")),
    ("UNAUTHORIZED", Some("401")),
    ("API_CONFIGURATION_ERROR", Some("500")),
    ("QUOTA_EXCEEDED", Some("429")),
    ("INTEGRATION_TIMEOUT", Some("504")),
    ("MISSING_AUTHENTICATION_TOKEN", Some("403")),
    ("INVALID_SIGNATURE", Some("403")),
    ("AUTHORIZER_FAILURE", Some("500")),
];

/// Every gateway response type an API has a gateway response for, in the order
/// [`GATEWAY_RESPONSE_CATALOG`] holds them — which is the order `GetGatewayResponses`
/// reports the collection in.
pub fn gateway_response_catalog() -> impl Iterator<Item = &'static str> {
    GATEWAY_RESPONSE_CATALOG.iter().map(|(kind, _)| *kind)
}

/// The status code [`GATEWAY_RESPONSE_CATALOG`] gives `response_type`, or `None` for
/// a type that has none (`DEFAULT_4XX`, `DEFAULT_5XX`) and for a value the catalog does
/// not name.
///
/// Shared by the operations that write and read a gateway response, so the status code a
/// `PutGatewayResponse` that set none stores is the one a read of the uncustomized type
/// would have reported.
pub fn default_gateway_status_code(response_type: &str) -> Option<&'static str> {
    GATEWAY_RESPONSE_CATALOG
        .iter()
        .find(|(kind, _)| *kind == response_type)
        .and_then(|(_, status)| *status)
}

/// The content type of the body template API Gateway generates a gateway response's body
/// from, and that template.
///
/// A gateway response a developer has written no template of its own for still RENDERS a
/// body, from API Gateway's own template: the error message of the refusal, as a JSON
/// object. Source: the API Gateway REST API reference's `GatewayResponse` representation,
/// where this is the `responseTemplates` entry every such response carries;
/// `$context.error.messageString` is the quoted form of `$context.error.message` (the
/// Developer Guide's `$context` variable reference).
pub const DEFAULT_GATEWAY_RESPONSE_TEMPLATE_TYPE: &str = "application/json";

/// The template [`DEFAULT_GATEWAY_RESPONSE_TEMPLATE_TYPE`] names.
pub const DEFAULT_GATEWAY_RESPONSE_TEMPLATE: &str = "{\"message\":$context.error.messageString}";

/// The gateway response an API reports for `response_type` while no customization is
/// stored for it: the one API Gateway generates, so `defaultResponse` is `true`, the
/// status code is the type's own, and nothing of it is a developer's.
pub fn default_gateway_response(response_type: &str) -> GetGatewayResponseOutput {
    GetGatewayResponseOutput {
        response_type: Some(response_type.to_string()),
        status_code: default_gateway_status_code(response_type).map(str::to_string),
        response_parameters: None,
        response_templates: None,
        default_response: true,
    }
}

/// The two class defaults, and which status class each stands in for: a gateway response
/// type whose own status code is in that class and that a developer has not customized
/// individually is served by the class default's customization.
///
/// Source: the API Gateway Developer Guide's gateway response table, where `DEFAULT_4XX`
/// and `DEFAULT_5XX` are the rows that stand for "all other 4xx" and "all other 5xx"
/// responses rather than for one condition.
const GATEWAY_RESPONSE_CLASS_DEFAULTS: [(&str, char); 2] =
    [("DEFAULT_4XX", '4'), ("DEFAULT_5XX", '5')];

/// The class default standing in for `response_type`: the one whose status class the
/// type's own status code belongs to, and `None` for a type with no status code of its own
/// — which is what the two class defaults themselves are.
fn gateway_response_class_default(response_type: &str) -> Option<&'static str> {
    let status = default_gateway_status_code(response_type)?;
    let class = status.chars().next()?;
    GATEWAY_RESPONSE_CLASS_DEFAULTS
        .iter()
        .find(|(_, held)| *held == class)
        .map(|(class_default, _)| *class_default)
}

/// The gateway response an API REPORTS for `response_type`, as every reading of one
/// answers it.
///
/// Resolved the way API Gateway serves the response itself: the developer's customization
/// of that very type where there is one, and otherwise the EFFECTIVE response of the type —
/// the customization of the class default that stands in for it (`DEFAULT_4XX` for a 4xx
/// type, `DEFAULT_5XX` for a 5xx one), which is what a response the developer has not
/// customized individually answers with: its parameters, its templates, and the status
/// code it answers with where the class default sets one of its own. Nothing of that
/// stand-in is the type's OWN customization, so the record is still reported with
/// `defaultResponse: true`, and a class default that sets no status code leaves each type
/// the status code of its own response.
///
/// One copy for `GetGatewayResponse` and `GetGatewayResponses`, so a single read of a type
/// and the collection entry of that type are one record.
pub fn reported_gateway_response_of(
    api: &RestApiEntry,
    response_type: &str,
) -> GetGatewayResponseOutput {
    if let Some(customized) = api.gateway_responses.get(response_type) {
        return reported_gateway_response(customized.clone());
    }
    let mut generated = default_gateway_response(response_type);
    if let Some(class_default) = gateway_response_class_default(response_type)
        .and_then(|class_default| api.gateway_responses.get(class_default))
    {
        if class_default.status_code.is_some() {
            generated.status_code = class_default.status_code.clone();
        }
        generated.response_parameters = class_default.response_parameters.clone();
        generated.response_templates = class_default.response_templates.clone();
    }
    reported_gateway_response(generated)
}

/// A gateway response as every REPRESENTATION of one carries it: both of its
/// string-to-string maps are always reported — `responseParameters` empty while the
/// response has no parameter configured, and `responseTemplates` carrying API Gateway's
/// own body template ([`DEFAULT_GATEWAY_RESPONSE_TEMPLATE`]) for a response a developer
/// has written no template of.
///
/// The stored record is the developer's CONFIGURATION — what a put wrote and a patch
/// edited — so the two maps are filled in here, where a gateway response is reported,
/// rather than at the write: one copy for `PutGatewayResponse`,
/// `UpdateGatewayResponse`, `GetGatewayResponse` and `GetGatewayResponses`, so a client
/// reading a gateway response any of those four ways can always tell which parameters and
/// which templates it has.
pub fn reported_gateway_response(
    mut response: GetGatewayResponseOutput,
) -> GetGatewayResponseOutput {
    response
        .response_parameters
        .get_or_insert_with(::std::collections::HashMap::new);
    if response
        .response_templates
        .as_ref()
        .is_none_or(|templates| templates.is_empty())
    {
        response.response_templates = Some(
            [(
                DEFAULT_GATEWAY_RESPONSE_TEMPLATE_TYPE.to_string(),
                DEFAULT_GATEWAY_RESPONSE_TEMPLATE.to_string(),
            )]
            .into_iter()
            .collect(),
        );
    }
    response
}

// ---------------------------------------------------------------------------
// SDK types
//
// The `SdkType` collection is the SERVICE's published catalog of the languages API
// Gateway can generate a client SDK in for a deployed stage. It is the same collection
// for every account, no operation of this service creates, updates or deletes an entry
// of it, and an `SdkType` id is exactly what `GetSdk`'s `sdkType` label names. So the
// catalog below IS the collection: `GetSdkTypes` pages it and `GetSdkType` resolves one
// id out of it, the same way an uncustomized gateway response is derived from the
// catalog above rather than from a stored record.
// ---------------------------------------------------------------------------

/// One configuration property of an SDK type, as the catalog spells it:
/// `(name, friendlyName, description, required)`. `name` is the key a `GetSdk`
/// `parameters` entry carries for that property.
type SdkPropertyRow = (&'static str, &'static str, &'static str, bool);

/// One SDK type of the catalog:
/// `(id, friendlyName, description, configurationProperties)`.
type SdkTypeRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static [SdkPropertyRow],
);

/// The SDK types API Gateway can generate a client SDK for.
///
/// Hand-written because this service's model declares no shape enumerating them and places
/// no constraint on their prose: the catalog is REFERENCE DATA the service authors, so what
/// belongs in it is exactly the generators API Gateway offers today, each spelled with the
/// service's own published text — a console, a CLI or a generator front-end renders these
/// strings to a user verbatim and enumerates the ids to decide which SDKs it may ask
/// `GetSdk` for, so an entry the service does not publish advertises a generator that does
/// not exist and a paraphrased sentence is user-visible drift.
///
/// The set is the two generators the service publishes, `java` and `ruby`, in that order.
/// `GetSdkRequest$sdkType`'s documentation (`smithy_query(query="doc",
/// shape="GetSdkRequest", member="sdkType")`: "The language for the generated SDK.
/// Currently `java`, `javascript`, `android`, `objectivec` (for iOS), `swift` (for iOS),
/// and `ruby` are supported") also names `javascript`, `android`, `objectivec` and `swift`,
/// but the catalog the service answers with holds neither of those four: that model prose
/// is older than the catalog, and the catalog — not the prose — is what a client
/// enumerates, so those ids resolve to nothing here, exactly as `GetSdkType` refuses any
/// other id the catalog does not hold.
///
/// Each entry is the GENERATOR the id names: the `friendlyName` is the label a developer
/// picks the generator by, the `description` names the language it generates a client for,
/// and the configuration properties are the keys a `GetSdk` `parameters` entry carries for
/// that generator — the dotted names those generators publish (`service.name`, `java.*`,
/// `ruby.*`), of which only the service name and the Java package name have no default and
/// so are `required`. Those dotted names SUPERSEDE the `serviceName`/`javaPackageName`
/// `GetSdkRequest$parameters` names for `java`, for the same reason: what the service
/// answers for a generator is what that generator publishes.
///
/// Every string below is the service's published text, including each property's
/// `description` — the sentence that states a property's default is the service's own
/// wording of it, not a paraphrase of the default.
///
/// Nothing here is per account or per request, so the whole collection is this constant.
/// `tests/all/sdk_types.rs` pins it entry for entry over the wire, so a re-worded string or
/// an added id fails there rather than reaching a client.
const SDK_TYPES: &[SdkTypeRow] = &[
    (
        "java",
        "Java SDK",
        "Java SDK generator for API Gateway APIs",
        &[
            (
                "service.name",
                "Service Name",
                "Name of the service which is used to derive the Java interface name for your client",
                true,
            ),
            (
                "java.package-name",
                "Java Package Name",
                "Name of the Java package your code will be generated under",
                true,
            ),
            (
                "java.build-system",
                "Java Build System",
                "Build system to setup for project; Currently supported: maven, gradle",
                false,
            ),
            (
                "java.group-id",
                "Java Group Id",
                "Group id for your Maven or Gradle project. Defaults to package name",
                false,
            ),
            (
                "java.artifact-id",
                "Java Artifact Id",
                "Artifact Id for your Maven project or project name for your Gradle project. Defaults to service name",
                false,
            ),
            (
                "java.artifact-version",
                "Java Artifact Version",
                "Version of your Maven or Gradle project. Defaults to 1.0-SNAPSHOT",
                false,
            ),
            (
                "java.license-text",
                "Source Code License Text",
                "Customer provided license to inject into source file headers",
                false,
            ),
        ],
    ),
    (
        "ruby",
        "Ruby",
        "Ruby SDK generator for API Gateway APIs",
        &[
            (
                "service.name",
                "Service Name",
                "Name of the service which is used to derive the name for your client",
                true,
            ),
            (
                "ruby.gem-name",
                "Ruby Gem Name",
                "Name of the Ruby gem your code will be generated under",
                false,
            ),
            (
                "ruby.gem-version",
                "Ruby Gem Version",
                "Version number for your service gem. Defaults to 1.0.0",
                false,
            ),
        ],
    ),
];

/// Every SDK type's id, in the catalog's order: the stable sequence `GetSdkTypes` pages
/// and a `position` cursor walks, which [`page_of`] cuts a page out of.
pub fn sdk_type_ids() -> Vec<String> {
    SDK_TYPES.iter().map(|(id, ..)| (*id).to_string()).collect()
}

/// The SDK type `id` names, or `None` when the catalog holds no type under that id.
///
/// Shared by `GetSdkType` and `GetSdkTypes`, so one entry of the listed collection and a
/// direct read of the same id are the same record member for member.
pub fn sdk_type(id: &str) -> Option<GetSdkTypeOutput> {
    SDK_TYPES
        .iter()
        .find(|(type_id, ..)| *type_id == id)
        .map(|(type_id, friendly_name, description, properties)| GetSdkTypeOutput {
            id: Some((*type_id).to_string()),
            friendly_name: Some((*friendly_name).to_string()),
            description: Some((*description).to_string()),
            // Always reported, and empty for a generator that takes no configuration:
            // the member says what a `GetSdk` call may set for this generator.
            configuration_properties: Some(
                properties
                    .iter()
                    .map(
                        |(name, property_friendly_name, property_description, required)| {
                            SdkConfigurationProperty {
                                name: Some((*name).to_string()),
                                friendly_name: Some((*property_friendly_name).to_string()),
                                description: Some((*property_description).to_string()),
                                required: *required,
                                // Each property's own default, where it has one, is stated
                                // in its description; no separate default value is
                                // published for any of them.
                                default_value: None,
                            }
                        },
                    )
                    .collect(),
            ),
        })
}

/// The `NotFoundException` message for an `id` that names no SDK type of the catalog.
///
/// API Gateway refuses an unresolvable SDK type by the GENERATOR it has none of, in fixed
/// text that quotes the id the request asked for: there is no SDK generator under that
/// name. The id is the only variable part, taken from the request's own path label.
pub fn no_such_sdk_type(id: &str) -> String {
    format!("No SDK generator for '{id}'")
}


/// API Gateway's documented default page size: "The maximum number of returned results
/// per page. The default value is 25 and the maximum value is 500."
/// (`GetRestApisRequest$limit` and `GetResourcesRequest$limit` in this service's model.)
pub const DEFAULT_PAGE_LIMIT: i32 = 25;

/// API Gateway's documented maximum page size; see [`DEFAULT_PAGE_LIMIT`].
pub const MAX_PAGE_LIMIT: i32 = 500;

/// The `resourceOwner` that selects the records of ANOTHER account — one of the two
/// values the modelled `ResourceOwner` enum declares (`smithy_query(query="shape",
/// shape="ResourceOwner")`: `SELF`, `OTHER_ACCOUNTS`), which
/// [`crate::ops::types::RESOURCE_OWNER_VALUES`] lists on the wire.
///
/// This member selects by OWNER: `SELF` lists the records "owned by your own account"
/// and `OTHER_ACCOUNTS` those "owned by other AWS accounts". This emulator serves
/// exactly one account ([`::emulator_server::emulator_account_id`]), and every record it
/// stores was created by that account's own call, so nothing it holds has another owner:
/// a request for this owner gets an empty page, rather than this account's own records
/// reported as somebody else's. ONE spelling of the rule for every collection the member
/// filters (`GetDomainNames`, `GetDomainNameAccessAssociations`), so two of them cannot
/// drift on it.
pub const OTHER_ACCOUNTS_RESOURCE_OWNER: &str = "OTHER_ACCOUNTS";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit delete,
// so each growth site carries a cap and nothing caller-sized reaches the store
// unchecked. The caps are API Gateway's own published quotas where it publishes one;
// where the service publishes none, the constant says so and the value is this
// emulator's resource guard, set well above any realistic use.
//
// The product is therefore finite: at most MAX_REST_APIS APIs, each with at most
// MAX_RESOURCES_PER_API resources, each with at most one method per verb in the HTTP
// verb catalog, each method's caller-sized maps capped at MAX_COLLECTION_ENTRIES
// entries of at most MAX_STRING_CHARS characters.
// ---------------------------------------------------------------------------

/// API Gateway's default quota "APIs per account per Region: 600".
pub const MAX_REST_APIS: usize = 600;

/// API Gateway's default quota "API keys per account per Region: 10,000".
pub const MAX_API_KEYS: usize = 10_000;

/// API Gateway's default quota "Usage plans per account per Region: 300".
pub const MAX_USAGE_PLANS: usize = 300;

/// Emulator resource guard, NOT an AWS quota: the most API keys this emulator associates
/// with ONE usage plan. API Gateway publishes no "API keys per usage plan" quota — a plan
/// may meter every key the account holds — so the guard is that count, [`MAX_API_KEYS`]:
/// an association names a stored API key, and an account stores at most that many.
///
/// The bound is checked at the growth site (`CreateUsagePlanKey`) rather than inferred
/// from the key store, because an association outlives nothing but is reclaimed with its
/// key: see [`UsagePlanEntry::keys`].
pub const MAX_USAGE_PLAN_KEYS_PER_PLAN: usize = MAX_API_KEYS;

/// API Gateway's default quota "Custom domain names (regional or edge-optimized) per
/// account per Region: 120".
pub const MAX_DOMAIN_NAMES: usize = 120;

/// API Gateway's default quota "Base path mappings per domain name: 200".
pub const MAX_BASE_PATH_MAPPINGS_PER_DOMAIN_NAME: usize = 200;

/// Emulator resource guard, NOT an AWS quota: the most domain name access associations
/// this emulator stores for one account. API Gateway publishes no quota for them, so the
/// bound is the emulator's own, the same order as the sibling per-account guards above.
pub const MAX_DOMAIN_NAME_ACCESS_ASSOCIATIONS: usize = 300;

/// API Gateway's default quota "Resources per API: 300".
pub const MAX_RESOURCES_PER_API: usize = 300;

/// API Gateway's default quota "Documentation parts: 2000 per API".
pub const MAX_DOCUMENTATION_PARTS_PER_API: usize = 2_000;

/// Emulator resource guard, NOT an AWS quota: the most documentation versions this
/// emulator keeps for one API. API Gateway publishes a documentation-PARTS quota (see
/// [`MAX_DOCUMENTATION_PARTS_PER_API`]) but none for the snapshots taken of them, so this
/// bound is the emulator's own, the same order as the deployment guard beside it.
pub const MAX_DOCUMENTATION_VERSIONS_PER_API: usize = 300;

/// API Gateway's default quota "Stages per API: 10".
pub const MAX_STAGES_PER_API: usize = 10;

/// API Gateway's default quota "Authorizers per API: 10".
pub const MAX_AUTHORIZERS_PER_API: usize = 10;

/// Emulator resource guard, NOT an AWS quota: the most deployments this emulator keeps
/// for one API. API Gateway publishes no deployment quota.
pub const MAX_DEPLOYMENTS_PER_API: usize = 300;

/// Emulator resource guard, NOT an AWS quota: the most request validators this emulator
/// keeps for one API. API Gateway publishes no request-validator quota, so this bound is
/// the emulator's own, the same order as the deployment guard above.
pub const MAX_REQUEST_VALIDATORS_PER_API: usize = 300;

/// Emulator resource guard, NOT an AWS quota: the most models this emulator keeps for one
/// API. API Gateway publishes no model quota, so this bound is the emulator's own, the
/// same order as the request-validator guard above.
pub const MAX_MODELS_PER_API: usize = 300;

/// From `CreateStage$stageName`'s own documentation: "Maximum length is 128
/// characters."
pub const MAX_STAGE_NAME_CHARS: usize = 128;

/// How long a stage's cache cluster takes to come up, after which it reports
/// `AVAILABLE`. API Gateway documents no duration for it ("it can take up to 4 minutes
/// to create" is guidance, not a contract), so this span is the emulator's own, chosen
/// so a caller reading straight after `CreateStage` still observes
/// `CREATE_IN_PROGRESS`.
pub const CACHE_CLUSTER_CREATE_SPAN: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

/// The `cacheClusterStatus` of a stage with no cache cluster.
pub const CACHE_CLUSTER_NOT_AVAILABLE: &str = "NOT_AVAILABLE";

/// The `cacheClusterStatus` of a cache cluster that is still coming up.
pub const CACHE_CLUSTER_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The `cacheClusterStatus` of a cache cluster that is up.
pub const CACHE_CLUSTER_AVAILABLE: &str = "AVAILABLE";

/// The `cacheClusterStatus` of a cache cluster whose entries are being flushed.
pub const CACHE_CLUSTER_FLUSH_IN_PROGRESS: &str = "FLUSH_IN_PROGRESS";

/// How long a stage's cache cluster stays `FLUSH_IN_PROGRESS` after
/// `FlushStageCache` empties it, after which it reports `AVAILABLE` again. API Gateway
/// documents no duration for a flush, so this span is the emulator's own, chosen so a
/// caller reading straight after `FlushStageCache` still observes `FLUSH_IN_PROGRESS`.
pub const CACHE_CLUSTER_FLUSH_SPAN: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

/// The `cacheClusterStatus` of a cache cluster that is being torn down, after an
/// `UpdateStage` disabled it.
pub const CACHE_CLUSTER_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// How many responses one stage's cache cluster holds at once.
///
/// A cache cluster is a FIXED-SIZE store — `cacheClusterSize` is "the stage's cache
/// capacity in GB" — so a full cluster makes room for a new entry by dropping one
/// ([`StageEntry::cache_response`]), and what the stage holds is bounded by the stage
/// rather than by how many distinct requests reach it. The COUNT is the emulator's own:
/// AWS publishes a capacity in bytes, not in entries.
pub const MAX_CACHED_RESPONSES_PER_STAGE: usize = 512;

/// How many authorizer results one stage holds at once.
///
/// An authorizer result is cached per identity ([`CachedAuthorizerResult`]), and a stage
/// is reached by as many identities as its callers hold, so what the stage holds is bounded
/// by the stage rather than by how many identities reach it: a full stage makes room for a
/// new result by dropping one ([`StageEntry::cache_authorizer_result`]). The COUNT is the
/// emulator's own — AWS publishes no capacity for the authorizer cache.
pub const MAX_CACHED_AUTHORIZER_RESULTS_PER_STAGE: usize = 512;

/// How long a stage's cache cluster stays `DELETE_IN_PROGRESS` after an `UpdateStage`
/// disabled it, after which the stage reports `NOT_AVAILABLE`. API Gateway documents no
/// duration for the teardown, so this span is the emulator's own, chosen so a caller
/// reading straight after the update still observes `DELETE_IN_PROGRESS`.
pub const CACHE_CLUSTER_DELETE_SPAN: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

/// API Gateway's default quota "VPC links per account per Region: 20".
pub const MAX_VPC_LINKS: usize = 20;

/// The `status` of a VPC link that is still being provisioned, from the modelled
/// `VpcLinkStatus` enum (`smithy_query(query="shape",
/// shape="com.amazonaws.apigateway#VpcLinkStatus")`: `AVAILABLE`, `PENDING`,
/// `DELETING`, `FAILED`).
pub const VPC_LINK_PENDING: &str = "PENDING";

/// The `status` of a VPC link that is provisioned; see [`VPC_LINK_PENDING`].
pub const VPC_LINK_AVAILABLE: &str = "AVAILABLE";

/// The `status` of a VPC link whose provisioning could not succeed; see
/// [`VPC_LINK_PENDING`] and [`VpcLinkEntry::failure`].
pub const VPC_LINK_FAILED: &str = "FAILED";

/// How long a VPC link stays [`VPC_LINK_PENDING`] after `CreateVpcLink`, after which it
/// reports [`VPC_LINK_AVAILABLE`]. API Gateway documents no duration for it
/// ("typically takes 2-4 minutes to complete and become operational" is guidance, not a
/// contract), so this span is the emulator's own, chosen so a caller reading straight
/// after the create still observes `PENDING`.
pub const VPC_LINK_PROVISION_SPAN: ::std::time::Duration = ::std::time::Duration::from_millis(500);

/// How long an edge-optimized custom domain name takes to become
/// [`DOMAIN_NAME_AVAILABLE`]: the CloudFront distribution API Gateway creates for it is
/// not in place when the create call answers. API Gateway documents no duration for it
/// ("it can take up to 40 minutes" is guidance, not a contract), so this span is the
/// emulator's own, chosen short enough not to slow a caller down and long enough that a
/// caller reading straight after `CreateDomainName` still observes
/// [`DOMAIN_NAME_PENDING`].
pub const DOMAIN_NAME_CREATE_SPAN: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

/// How long a custom domain name stays [`DOMAIN_NAME_UPDATING`] after an update moved it
/// to another endpoint type, after which it reports [`DOMAIN_NAME_AVAILABLE`] again. API
/// Gateway documents no duration for the migration either, so this span is the
/// emulator's own, chosen on the grounds [`DOMAIN_NAME_CREATE_SPAN`] gives: a caller
/// reading straight after the update still observes the migration.
pub const DOMAIN_NAME_MIGRATION_SPAN: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

/// From `Authorizer$authorizerResultTtlInSeconds`'s own documentation: "If this field
/// is not set, the default value is 300."
pub const DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS: i32 = 300;

/// From `Authorizer$authorizerResultTtlInSeconds`'s own documentation: "If it equals 0,
/// authorization caching is disabled" — so 0 is the lowest value it takes.
pub const MIN_AUTHORIZER_RESULT_TTL_SECONDS: i32 = 0;

/// From `Authorizer$authorizerResultTtlInSeconds`'s own documentation: "The maximum
/// value is 3600, or 1 hour."
pub const MAX_AUTHORIZER_RESULT_TTL_SECONDS: i32 = 3600;

/// API Gateway's documented resource-policy document size: 8192 characters.
pub const MAX_POLICY_CHARS: usize = 8192;

/// AWS's tagging limit of 50 tags per resource.
pub const MAX_TAGS: usize = 50;

/// From the `tags` member's own documentation: "The tag key can be up to 128
/// characters".
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// From the `tags` member's own documentation: "The tag value can be up to 256
/// characters."
pub const MAX_TAG_VALUE_CHARS: usize = 256;

// ---------------------------------------------------------------------------
// Taggable resources, addressed by ARN
//
// `GetTags`, `TagResource` and `UntagResource` name their target by ARN rather than by
// the path labels every other operation of this service takes, so reading an ARN into
// the record it names lives HERE, in one place all three share: the same string must
// resolve to the same record, and be refused the same way, whichever of them took it.
//
// WHICH resources are taggable comes from the model: a resource of this service is
// taggable exactly when the shape declared for it carries a `tags` member
// (`smithy_query(query="shape", shape="RestApi,Stage,ApiKey,UsagePlan,DomainName,\
// ClientCertificate,VpcLink,DomainNameAccessAssociation")` — all eight do), and the
// resource portion of such a resource's ARN is the URI of the operation that reads it
// (`smithy_query(query="operation", operation="GetRestApi,GetStage,GetApiKey,\
// GetUsagePlan,GetDomainName,GetClientCertificate,GetVpcLink")`: `/restapis/{restApiId}`,
// `/restapis/{restApiId}/stages/{stageName}`, `/apikeys/{apiKey}`, and so on).
// ---------------------------------------------------------------------------

/// The vendor field an ARN of one of this service's resources carries: this service's
/// own `aws.api#service$arnNamespace` (`smithy_query(query="service_info")`:
/// `arnNamespace: apigateway`), which is what says an ARN names a resource this API can
/// address at all.
const ARN_SERVICE: &str = "apigateway";

/// The resource one `resourceArn` names: the stored record a tag collection hangs off,
/// as [`parse_tagged_resource`] read it out of the ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `/restapis/{restApiId}` — a [`RestApiEntry`]'s tags.
    RestApi { rest_api_id: String },
    /// `/restapis/{restApiId}/stages/{stageName}` — a [`StageEntry`]'s tags.
    Stage {
        rest_api_id: String,
        stage_name: String,
    },
    /// `/apikeys/{apiKey}` — an [`ApiKeyEntry`]'s tags.
    ApiKey { api_key_id: String },
    /// `/vpclinks/{vpcLinkId}` — a [`VpcLinkEntry`]'s tags.
    VpcLink { vpc_link_id: String },
    /// `/usageplans/{usagePlanId}` — a [`UsagePlanEntry`]'s tags.
    UsagePlan { usage_plan_id: String },
    /// `/domainnames/{domainName}` — a [`DomainNameEntry`]'s tags, where the path
    /// segment is the record's [`domain_name_key`].
    DomainName { domain_name_key: String },
    /// `/domainnameaccessassociations/{id}` — a [`DomainNameAccessAssociationEntry`]'s
    /// tags, where the path segment is the record's minted id.
    DomainNameAccessAssociation { access_association_id: String },
    /// `/clientcertificates/{clientCertificateId}` — a [`ClientCertificateEntry`]'s tags.
    ClientCertificate { client_certificate_id: String },
}

impl TaggedResource {
    /// The kind of resource this names, as the leading segment of its ARN's resource
    /// portion — which is also the first path segment of the operation that reads it
    /// (`restapis`, `apikeys`, `vpclinks`, `domainnames`, …).
    ///
    /// For an operation that takes an ARN of ONE kind rather than any taggable one
    /// (`RejectDomainNameAccessAssociation`, whose two ARNs name an access association
    /// and a domain name): it reads the ARN through [`parse_tagged_resource`] like every
    /// other ARN of this service and then checks the kind, instead of parsing the value a
    /// second way.
    ///
    /// It is the ARN's FIRST segment only, so it does not distinguish an API from one of
    /// its stages — both answer `restapis`, the segment their ARNs share. A caller that
    /// needs that distinction matches the variant rather than the kind.
    pub fn kind(&self) -> &str {
        match self {
            TaggedResource::RestApi { .. } | TaggedResource::Stage { .. } => "restapis",
            TaggedResource::ApiKey { .. } => "apikeys",
            TaggedResource::VpcLink { .. } => "vpclinks",
            TaggedResource::UsagePlan { .. } => "usageplans",
            TaggedResource::DomainName { .. } => "domainnames",
            TaggedResource::DomainNameAccessAssociation { .. } => "domainnameaccessassociations",
            TaggedResource::ClientCertificate { .. } => "clientcertificates",
        }
    }
}

/// Read `resource_arn` as the ARN of a taggable resource of this service.
///
/// The six ARN fields come from [`::service_common::arn::parse_of`], the ONE parse of a
/// caller-supplied ARN, narrowed to this service's vendor namespace — so what counts as
/// an ARN, and whose resource it names, is what every other service of this emulator
/// says. Only the resource PORTION is read here, because its shape is this service's
/// own.
///
/// `None` for anything that is not the ARN of a taggable resource — a string that is not
/// an ARN, an ARN of another service, or an API Gateway ARN whose resource portion names
/// something this service does not tag (a resource or a deployment of an API, a usage
/// plan key) — which the caller is told is a bad request rather than a missing resource.
///
/// The region and account fields are not matched further: this emulator serves one
/// account, and nothing it stores is keyed by partition or region, so what selects a
/// record is the resource portion.
/// The AWS ACCOUNT the ARN `resource_arn` names as the owner of the resource it addresses
/// — an ARN's fifth field — or `None` when the value is not an ARN of this service at all.
///
/// Read through [`::service_common::arn::parse_of`], the same single parse
/// [`parse_tagged_resource`] reads the resource portion with, so the owner an ARN names and
/// the record it selects are read off one grammar.
///
/// The field is EMPTY on every ARN this service MINTS ([`service_arn`]), because API
/// Gateway's own ARNs carry no account — so two ARNs this emulator handed out name the same
/// (single) account, and only an ARN whose account field a caller filled in differently
/// names another one. That is what lets an operation defined on a CROSS-ACCOUNT
/// relationship (`RejectDomainNameAccessAssociation`) tell the two cases apart: nothing
/// else in a request says whose a resource is.
pub fn arn_account(resource_arn: &str) -> Option<&str> {
    ::service_common::arn::parse_of(resource_arn, ARN_SERVICE).map(|arn| arn.account)
}

pub fn parse_tagged_resource(resource_arn: &str) -> Option<TaggedResource> {
    let arn = ::service_common::arn::parse_of(resource_arn, ARN_SERVICE)?;
    let path: Vec<&str> = arn.resource.strip_prefix('/')?.split('/').collect();
    if path.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    match path.as_slice() {
        ["restapis", rest_api_id] => Some(TaggedResource::RestApi {
            rest_api_id: (*rest_api_id).to_string(),
        }),
        ["restapis", rest_api_id, "stages", stage_name] => Some(TaggedResource::Stage {
            rest_api_id: (*rest_api_id).to_string(),
            stage_name: (*stage_name).to_string(),
        }),
        ["apikeys", api_key_id] => Some(TaggedResource::ApiKey {
            api_key_id: (*api_key_id).to_string(),
        }),
        ["vpclinks", vpc_link_id] => Some(TaggedResource::VpcLink {
            vpc_link_id: (*vpc_link_id).to_string(),
        }),
        ["usageplans", usage_plan_id] => Some(TaggedResource::UsagePlan {
            usage_plan_id: (*usage_plan_id).to_string(),
        }),
        ["domainnames", domain_name_key] => Some(TaggedResource::DomainName {
            domain_name_key: (*domain_name_key).to_string(),
        }),
        ["domainnameaccessassociations", access_association_id] => {
            Some(TaggedResource::DomainNameAccessAssociation {
                access_association_id: (*access_association_id).to_string(),
            })
        }
        ["clientcertificates", client_certificate_id] => {
            Some(TaggedResource::ClientCertificate {
                client_certificate_id: (*client_certificate_id).to_string(),
            })
        }
        _ => None,
    }
}

/// The resource PORTION of the ARN of the resource a [`TaggedResource`] names — the
/// inverse of [`parse_tagged_resource`], so an ARN this service hands out is one that
/// same function reads back into the record it came from.
///
/// It is the read URI of the resource's own `Get*` operation, which is what the section
/// comment above cites the `smithy_query` calls for.
pub fn tagged_resource_path(resource: &TaggedResource) -> String {
    match resource {
        TaggedResource::RestApi { rest_api_id } => format!("/restapis/{rest_api_id}"),
        TaggedResource::Stage {
            rest_api_id,
            stage_name,
        } => format!("/restapis/{rest_api_id}/stages/{stage_name}"),
        TaggedResource::ApiKey { api_key_id } => format!("/apikeys/{api_key_id}"),
        TaggedResource::VpcLink { vpc_link_id } => format!("/vpclinks/{vpc_link_id}"),
        TaggedResource::UsagePlan { usage_plan_id } => format!("/usageplans/{usage_plan_id}"),
        TaggedResource::DomainName { domain_name_key } => format!("/domainnames/{domain_name_key}"),
        TaggedResource::DomainNameAccessAssociation {
            access_association_id,
        } => format!("/domainnameaccessassociations/{access_association_id}"),
        TaggedResource::ClientCertificate {
            client_certificate_id,
        } => format!("/clientcertificates/{client_certificate_id}"),
    }
}

/// Emulator resource guard, NOT an AWS quota: the most entries this emulator will store
/// for a caller-sized list or map whose size the service publishes no limit for.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied string this
/// emulator will store for a member whose length the service publishes no limit for.
pub const MAX_STRING_CHARS: usize = 512;

/// Emulator resource guard, NOT an AWS quota: the longest single path segment.
pub const MAX_PATH_PART_CHARS: usize = 128;

/// Emulator resource guard, NOT an AWS quota: the longest materialised resource path,
/// which also bounds how deeply resources may nest.
pub const MAX_PATH_CHARS: usize = 1024;

/// Refuse a caller-sized collection whose entry count would grow the store past `max`.
///
/// `Err` carries the message, naming the member and the bound it broke, for the caller
/// to fix the request from.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        return Err(format!(
            "{member} may carry at most {max} entries, got {count}"
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} may be at most {max} characters, got {length}"
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied `domainName` that is not a domain name.
///
/// A custom domain name is a DNS name a client resolves and then presents in its TLS
/// handshake, so what this service can serve is bounded by what DNS allows: dot-separated
/// labels of ASCII letters, digits and hyphens that neither begin nor end with a hyphen,
/// at most [`MAX_DOMAIN_LABEL_CHARS`] each and [`MAX_DOMAIN_NAME_CHARS`] in total, with a
/// leading `*` label allowed because AWS serves wildcard custom domain names. The model
/// declares no constraint on the member, so the check lives here and the refusal names
/// what was rejected.
///
/// `Err` carries the message, for the caller to fix the request from; a refused name
/// never reaches the store.
pub fn bound_domain_name(domain_name: &str) -> Result<(), String> {
    bound_chars("domainName", domain_name, MAX_DOMAIN_NAME_CHARS)?;
    let labels: Vec<&str> = domain_name.split('.').collect();
    let well_formed = !domain_name.is_empty()
        && labels.len() > 1
        && labels.iter().enumerate().all(|(index, label)| {
            let wildcard = index == 0 && *label == "*";
            wildcard
                || (!label.is_empty()
                    && label.chars().count() <= MAX_DOMAIN_LABEL_CHARS
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-'))
        });
    if !well_formed {
        return Err(format!(
            "domainName must be a domain name of dot-separated labels of letters, digits \
             and inner hyphens, got {domain_name:?}"
        ));
    }
    Ok(())
}

/// The longest a domain name can be: 253 characters, the DNS limit on a fully qualified
/// name (RFC 1035's 255-octet wire form, less its length and root bytes).
pub const MAX_DOMAIN_NAME_CHARS: usize = 253;

/// Refuse a caller-supplied `basePath` outside the character class API Gateway admits for
/// one.
///
/// A base path is "the base path name that callers of the API must provide as part of the
/// URL after the domain name" (`BasePathMapping$basePath`), and API Gateway admits one
/// character class for it: letters, numbers and the safe punctuation
/// [`BASE_PATH_CHARACTERS`] — which is what its refusal states ([`INVALID_BASE_PATH`]).
/// A value carrying anything else — a non-ASCII character, a space, a URL delimiter — is
/// not a base path this service will store, so it is refused before any record is touched;
/// the model declares no constraint on the member, so the check lives here.
///
/// [`NONE_BASE_PATH`] is inside the class anyway (its parentheses are among the admitted
/// characters), and it is the spelling the member itself documents for the mapping that
/// has no base path at all.
///
/// `Err` carries the message, for the caller to fix the request from; a refused base path
/// never reaches the store.
pub fn bound_base_path(base_path: &str) -> Result<(), String> {
    let admitted = !base_path.is_empty()
        && base_path.chars().all(|character| {
            character.is_ascii_alphanumeric() || BASE_PATH_CHARACTERS.contains(character)
        });
    if !admitted {
        return Err(INVALID_BASE_PATH.to_string());
    }
    Ok(())
}

/// The punctuation a `basePath` may carry beside letters and numbers, in the order
/// [`INVALID_BASE_PATH`] states it.
pub const BASE_PATH_CHARACTERS: &str = "$-_.+!*'(),";

/// The `BadRequestException` message for a `basePath` outside the class
/// [`bound_base_path`] admits: ONE sentence of entirely fixed text, which states the class
/// and quotes back neither the value nor the member that carried it.
pub const INVALID_BASE_PATH: &str =
    "A base path may contain only letters, numbers and one of $-_.+!*'(),";

/// The longest one dot-separated label of a domain name can be: 63 characters (RFC 1035).
pub const MAX_DOMAIN_LABEL_CHARS: usize = 63;

/// Whether `code` is an HTTP status code, as the model's `StatusCode` shape is
/// constrained to be (`^[1-5]\d\d$`): a gateway response's, a method response's and an
/// integration response's status code are all that one shape.
///
/// A request member typed `StatusCode` is refused against the pattern at the boundary, so
/// this is for a status code that reaches a handler as a BARE string — a `patchOperations`
/// value, or one read out of an imported API definition — which no boundary check covers.
pub fn is_status_code(code: &str) -> bool {
    code.len() == 3
        && code.starts_with(['1', '2', '3', '4', '5'])
        && code.bytes().all(|byte| byte.is_ascii_digit())
}

/// A caller-supplied map as it is STORED and reported: absent when the caller set no
/// entry, so an empty map in the request does not become an empty map in every later
/// read.
///
/// One copy for every operation that stores a caller-sized map (an integration's request
/// parameters and templates, a method response's response parameters and models), so two
/// of them cannot drift on what an empty map means.
pub fn non_empty_map<V>(
    map: Option<::std::collections::HashMap<String, V>>,
) -> Option<::std::collections::HashMap<String, V>> {
    map.filter(|entries| !entries.is_empty())
}

/// A stored caller-supplied map as the shared map-patch helpers take it: the modelled
/// shapes carry `HashMap`s that are absent when empty, while a patch is applied to a
/// `BTreeMap` that is present-or-empty, so this is the one place the two spellings meet.
pub fn ordered_map<V>(
    stored: Option<::std::collections::HashMap<String, V>>,
) -> BTreeMap<String, V> {
    stored.unwrap_or_default().into_iter().collect()
}

/// The inverse of [`ordered_map`]: a patched map as it is STORED and reported — absent
/// once its last entry is removed, which is what [`non_empty_map`] means by an empty
/// caller-supplied map.
pub fn stored_map<V>(
    patched: BTreeMap<String, V>,
) -> Option<::std::collections::HashMap<String, V>> {
    non_empty_map(Some(patched.into_iter().collect()))
}

/// One page of an ordered cursor sequence, as a `position`/`limit` paginated collection
/// reports it.
#[derive(Debug, Clone)]
pub struct Page {
    /// The cursors on this page, in the sequence's own order.
    pub ids: Vec<String>,
    /// The `position` this answer reports: the cursor of the first element AFTER this
    /// page, which is where the next page RESUMES. Absent once nothing follows the page,
    /// and that absence is what ends a walk of the collection.
    pub next_position: Option<String>,
    /// The page size the REQUEST asked for; see [`Page::echoed_limit`].
    requested_limit: Option<i32>,
    /// Whether the sequence this page was cut out of holds no element at all; see
    /// [`Page::reported_items`].
    sequence_empty: bool,
}

impl Page {
    /// Cut the `limit`-sized page that begins at index `start` out of `ordered`.
    ///
    /// `limit` defaults to [`DEFAULT_PAGE_LIMIT`] and is clamped into the documented
    /// `1..=`[`MAX_PAGE_LIMIT`] range. `start` beyond the last element is the empty final
    /// page, not a failure: a caller that walks a collection to its end is served the
    /// page that holds nothing rather than a client error.
    pub fn cut(ordered: &[String], start: usize, limit: Option<i32>) -> Self {
        let size = limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
        let window = usize::try_from(size).unwrap_or(1);
        let start = start.min(ordered.len());
        let end = start.saturating_add(window).min(ordered.len());
        let ids = ordered[start..end].to_vec();
        Self {
            // The cursor a client sends back names where to CARRY ON, so it is the first
            // element the page did not report and the page that follows begins AT it. A
            // page the sequence holds nothing after reports no cursor at all, which is
            // what tells a caller the collection has ended.
            next_position: ordered.get(end).cloned(),
            ids,
            requested_limit: limit,
            sequence_empty: ordered.is_empty(),
        }
    }

    /// The page size this answer echoes: the one the REQUEST asked for, for a page the
    /// collection carries on after — the member states what the caller asked of a
    /// collection it has not finished walking, so a request that asked for no size is
    /// told none. The page that ends the collection paged nothing further, and reports
    /// neither a size nor a cursor.
    pub fn echoed_limit(&self) -> Option<i32> {
        self.requested_limit.filter(|_| self.next_position.is_some())
    }

    /// The page of elements as the answer's collection member REPORTS it: absent when
    /// the collection being listed holds no element at all, so a listing of nothing is
    /// not a present-but-empty page.
    ///
    /// The empty page of a collection that does hold elements — the window a caller that
    /// walked to the end asks for — stays present and empty: it reports a page of a
    /// collection, and that page is empty. One copy for every `position`/`limit`
    /// collection, so two listings cannot disagree about what an empty one looks like.
    pub fn reported_items<Element>(&self, items: Vec<Element>) -> Option<Vec<Element>> {
        if self.sequence_empty {
            None
        } else {
            Some(items)
        }
    }
}

/// Cut the page a caller asked for out of `ordered`, a stable sequence of cursors.
///
/// `position` is the cursor the collection carries on AT — the first element the previous
/// page did not report — so the page begins at it; `None` begins at the first element.
/// The sequence's order is its own (creation order, for the collections of this service
/// whose cursor is a minted id), so a cursor is resolved by naming an element of it.
///
/// `Err` carries [`INVALID_POSITION`] for an unresolvable cursor: a `position` naming no
/// element of the sequence, which the caller must be told about rather than served an
/// arbitrary page.
pub fn page_of(
    ordered: &[String],
    position: Option<&str>,
    limit: Option<i32>,
) -> Result<Page, String> {
    let start = match position {
        None => 0,
        Some(cursor) => ordered
            .iter()
            .position(|id| id == cursor)
            .ok_or_else(|| INVALID_POSITION.to_string())?,
    };
    Ok(Page::cut(ordered, start, limit))
}

/// The `BadRequestException` message for a `position` this service did not issue for the
/// collection being listed.
///
/// A fixed sentence naming the member that is wrong, which quotes the rejected cursor back
/// nowhere. One copy for every `position`/`limit` collection, so an unusable cursor is
/// refused the same way whichever listing took it.
pub const INVALID_POSITION: &str = "Invalid position parameter";

// ---------------------------------------------------------------------------
// Patch operations
//
// Every `Update*` operation of this service is an HTTP PATCH carrying
// `patchOperations`: a list of JSON-Pointer addressed edits that the service applies to
// the stored resource, in request order, before reporting the updated resource. What an
// `op` means, how a `path` is read and how a single-valued member, a string set or a map
// member is edited are shared here, so no two handlers can disagree about it: a handler
// only maps a supported path to the member of its own record that the path names.
// ---------------------------------------------------------------------------

impl ComAmazonawsApigatewayBackplanecontrolserviceState {
    /// Mint a fresh id of the shape API Gateway assigns a record of the ACCOUNT — a
    /// `RestApi`, an `ApiKey`, a `UsagePlan`, and the root resource a `RestApi` is created
    /// with — ten lowercase alphanumeric characters.
    ///
    /// Every id is DRAWN character by character ([`identifier::drawn_id`]), so it is a
    /// function of nothing a caller can see: not of this record, not of how many records
    /// were created before it. The caller keeps it by storing it with the record.
    pub fn mint_id(&mut self) -> String {
        identifier::drawn_id(identifier::ACCOUNT_RECORD_ID_LEN)
    }

    /// Mint a fresh id of the shape API Gateway assigns a record created INSIDE one REST
    /// API — a `Resource`, a `Model`, a `Deployment` — six lowercase alphanumeric
    /// characters; see [`Self::mint_id`] for how an id is drawn.
    pub fn mint_sub_resource_id(&mut self) -> String {
        identifier::drawn_id(identifier::SUB_RESOURCE_ID_LEN)
    }

    /// The next creation rank. Stamped on a record when it is created, so a listing
    /// reports creation order even though the records are keyed by id.
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence_counter = self.sequence_counter.wrapping_add(1);
        self.sequence_counter
    }

    /// Mint `count` ids at once, so a handler that needs several can take the store
    /// lock once and then do its work — building a cloned resource tree, say — with the
    /// lock released.
    pub fn reserve_ids(&mut self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.mint_id()).collect()
    }

    /// Mint `count` ids for records held inside one REST API at once; see
    /// [`Self::reserve_ids`] and [`Self::mint_sub_resource_id`].
    pub fn reserve_sub_resource_ids(&mut self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.mint_sub_resource_id()).collect()
    }

    /// Drop every usage plan association that names `key_id`, answering how many plans
    /// stopped metering it.
    ///
    /// Called when an API key is DELETED: a key that no longer exists is metered by no
    /// plan, so its associations go with it rather than sitting in the plans for the life
    /// of the process. This is what keeps the association count bounded by the stores it
    /// joins ([`MAX_API_KEYS`] x [`MAX_USAGE_PLANS`]) rather than by request count.
    pub fn forget_api_key(&mut self, key_id: &str) -> usize {
        let mut unmetered = 0;
        for plan in self.usage_plans.values_mut() {
            if plan.keys.remove(key_id).is_some() {
                unmetered += 1;
            }
        }
        // The throttle accounting goes with the association: a key no plan meters has no
        // request rate to keep, and a key created later must not inherit this one's. The
        // quota accounting goes for the same reason: the requests a deleted key sent are
        // counted against no plan, and a key created later starts its periods empty.
        let metered_by_this_key = format!("/{key_id}");
        self.throttle_buckets
            .retain(|key, _| !key.ends_with(&metered_by_this_key));
        self.usage_logs
            .retain(|key, _| !key.ends_with(&metered_by_this_key));
        unmetered
    }

    /// Whether the usage plan `usage_plan_id`'s throttle admits one more request from the
    /// API key `key_id` at `now`, RECORDING the request it admits.
    ///
    /// This is the one place a usage plan's throttle is accounted, so the data plane and
    /// anything else that meters a request share one bucket per pair; a plan whose
    /// `throttle` declares no positive limit ([`ThrottleLimits::declared`]) is not metered
    /// at all and admits every request.
    pub fn throttle_admits(
        &mut self,
        usage_plan_id: &str,
        key_id: &str,
        limits: ThrottleLimits,
        now: ::std::time::Instant,
    ) -> bool {
        self.throttle_buckets
            .entry(metering_key(usage_plan_id, key_id))
            .or_insert_with(|| ThrottleBucket::full(limits, now))
            .admits(limits, now)
    }

    /// The documentation version a `(restApiId, documentationVersion)` pair addresses, or
    /// the message that pair is refused with.
    ///
    /// The pair is resolved as ONE lookup for the snapshot the request addresses, so the
    /// refusal reports the DOCUMENTATION VERSION — the record the URI's leaf label names —
    /// whether the snapshot is missing from a stored API or the API that would hold it is
    /// not stored either: a snapshot cannot exist without its API, so an unresolvable
    /// `restApiId` leaves the addressed version unresolvable too and is not reported as an
    /// API identifier failure. One copy for every operation addressed by that label, so the
    /// read, the update and the delete say the same thing about the same pair.
    pub fn documentation_version(
        &self,
        rest_api_id: &str,
        documentation_version: &str,
    ) -> Result<&DocumentationVersionEntry, String> {
        self.rest_apis
            .get(rest_api_id)
            .and_then(|api| api.documentation_versions.get(documentation_version))
            .ok_or_else(|| NO_SUCH_DOCUMENTATION_VERSION.to_string())
    }

    /// The stage a `(restApiId, stageName)` pair addresses, with the API that holds it, or
    /// the message that pair is refused with.
    ///
    /// The pair is resolved as ONE lookup for the stage the request addresses, the way
    /// [`Self::documentation_version`] resolves a snapshot: a stage cannot exist without
    /// its API, so an unresolvable `restApiId` leaves the addressed stage unresolvable too
    /// and the refusal reports the STAGE ([`NO_SUCH_STAGE`]) rather than the API identifier.
    /// The API is handed back beside the stage because a stage alone renders nothing: what
    /// a stage serves is a deployment of that API.
    pub fn api_and_stage(
        &self,
        rest_api_id: &str,
        stage_name: &str,
    ) -> Result<(&RestApiEntry, &StageEntry), String> {
        self.rest_apis
            .get(rest_api_id)
            .and_then(|api| api.stages.get(stage_name).map(|stage| (api, stage)))
            .ok_or_else(|| NO_SUCH_STAGE.to_string())
    }

    /// The same lookup as [`Self::api_and_stage`], for a call that WRITES on the stage it
    /// addresses — a cache flush, a patch.
    pub fn stage_mut(
        &mut self,
        rest_api_id: &str,
        stage_name: &str,
    ) -> Result<&mut StageEntry, String> {
        self.rest_apis
            .get_mut(rest_api_id)
            .and_then(|api| api.stages.get_mut(stage_name))
            .ok_or_else(|| NO_SUCH_STAGE.to_string())
    }

    /// The resource a `(restApiId, resourceId)` pair addresses, or the message that pair is
    /// refused with.
    ///
    /// The pair is refused by naming the identifier that did not resolve: an API this
    /// account does not hold is reported as the API ([`no_such_rest_api`], which names the
    /// identifier it could not resolve), and only an API that IS held but holds no such
    /// resource is reported as the resource ([`NO_SUCH_RESOURCE`]). A stage's own lookup
    /// ([`Self::api_and_stage`]) reports the STAGE either way, which is that operation's own
    /// recorded contract rather than a disagreement with this one.
    pub fn resource(&self, rest_api_id: &str, resource_id: &str) -> Result<&ResourceEntry, String> {
        self.rest_apis
            .get(rest_api_id)
            .ok_or_else(|| no_such_rest_api(rest_api_id))?
            .resources
            .get(resource_id)
            .ok_or_else(|| NO_SUCH_RESOURCE.to_string())
    }

    /// The same lookup as [`Self::resource`], for a call that WRITES under the resource it
    /// addresses — a method put on it, a child resource created under it.
    pub fn resource_mut(
        &mut self,
        rest_api_id: &str,
        resource_id: &str,
    ) -> Result<&mut ResourceEntry, String> {
        self.rest_apis
            .get_mut(rest_api_id)
            .ok_or_else(|| no_such_rest_api(rest_api_id))?
            .resources
            .get_mut(resource_id)
            .ok_or_else(|| NO_SUCH_RESOURCE.to_string())
    }

    /// The `UsagePlanKey` a `(usagePlanId, keyId)` pair addresses: the association held
    /// on the plan, paired with the API key record whose name and value the
    /// `UsagePlanKey` body reports.
    ///
    /// `None` when the plan holds no association for that key id, AND when it holds one
    /// whose API key is no longer stored. `DeleteApiKey` drops those associations itself
    /// (see [`ComAmazonawsApigatewayBackplanecontrolserviceState::forget_api_key`]), so
    /// the join is the invariant rather than the reclamation: no read can report a key
    /// that no longer exists. The two records are returned together because neither alone
    /// renders a `UsagePlanKey`.
    pub fn usage_plan_key(
        &self,
        usage_plan_id: &str,
        key_id: &str,
    ) -> Option<(&UsagePlanKeyEntry, &ApiKeyEntry)> {
        let entry = self.usage_plans.get(usage_plan_id)?.keys.get(key_id)?;
        let api_key = self.api_keys.get(&entry.key_id)?;
        Some((entry, api_key))
    }

    /// The ids of every stored API key in creation order, the stable sequence
    /// [`page_of`] cuts a `GetApiKeys` page out of.
    ///
    /// The sequence is fixed when each key is created — by the creation rank
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`] stamps on the
    /// record, with the key's own id breaking a tie — so nothing a caller can change
    /// moves a key in the collection: renaming, describing, enabling or disabling a key
    /// leaves it where it was, and the `position` cursor a page handed back keeps naming
    /// the same point in the sequence.
    pub fn api_key_ids_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&ApiKeyEntry> = self.api_keys.values().collect();
        ordered.sort_by_key(|key| (key.sequence, key.id.clone()));
        ordered.into_iter().map(|key| key.id.clone()).collect()
    }

    /// The ids of every stored usage plan in creation order, the stable sequence
    /// [`page_of`] cuts a `GetUsagePlans` page out of.
    pub fn usage_plan_ids_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&UsagePlanEntry> = self.usage_plans.values().collect();
        ordered.sort_by_key(|plan| (plan.sequence, plan.id.clone()));
        ordered.into_iter().map(|plan| plan.id.clone()).collect()
    }

    /// The ids of every stored VPC link in creation order, the stable sequence
    /// [`page_of`] cuts a `GetVpcLinks` page out of.
    ///
    /// The rank is fixed when each link is created, with the link's own id breaking a
    /// tie, so nothing a caller can change moves a link in the collection: renaming,
    /// describing or retargeting a link leaves it where it was, and the `position` cursor
    /// a page handed back keeps naming the same point in the sequence.
    pub fn vpc_link_ids_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&VpcLinkEntry> = self.vpc_links.values().collect();
        ordered.sort_by_key(|link| (link.sequence, link.id.clone()));
        ordered.into_iter().map(|link| link.id.clone()).collect()
    }

    /// The ids of every stored API in creation order, the stable sequence [`page_of`]
    /// cuts a `GetRestApis` page out of.
    pub fn rest_api_ids_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&RestApiEntry> = self.rest_apis.values().collect();
        ordered.sort_by_key(|api| (api.sequence, api.id.clone()));
        ordered.into_iter().map(|api| api.id.clone()).collect()
    }

    /// The keys of every stored custom domain name in creation order, the stable
    /// sequence [`page_of`] cuts a `GetDomainNames` page out of. See
    /// [`domain_name_key`] for what one key is.
    pub fn domain_name_keys_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&DomainNameEntry> = self.domain_names.values().collect();
        ordered.sort_by_key(|domain| (domain.sequence, domain.key()));
        ordered.into_iter().map(DomainNameEntry::key).collect()
    }

    /// The ids of every stored domain name access association in creation order, the
    /// stable sequence [`page_of`] cuts a `GetDomainNameAccessAssociations` page out of.
    pub fn domain_name_access_association_ids_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&DomainNameAccessAssociationEntry> =
            self.domain_name_access_associations.values().collect();
        ordered.sort_by_key(|association| (association.sequence, association.id.clone()));
        ordered
            .into_iter()
            .map(|association| association.id.clone())
            .collect()
    }

    /// The stored access association of `access_association_source` with the domain name
    /// `domain_name_arn`, if this account already has one.
    ///
    /// An access association IS that pair — it is what lets one source invoke one private
    /// custom domain name — so the pair is what identifies an existing association, and
    /// this is the one lookup that decides whether a create would duplicate one.
    pub fn access_association_of(
        &self,
        domain_name_arn: &str,
        access_association_source: &str,
    ) -> Option<&DomainNameAccessAssociationEntry> {
        self.domain_name_access_associations
            .values()
            .find(|association| {
                association.domain_name_arn == domain_name_arn
                    && association.access_association_source == access_association_source
            })
    }

    /// Whether any stored access association names the domain name `domain_name_arn`.
    ///
    /// The dependency lookup, beside the pair lookup above: an association is created only
    /// against a STORED private custom domain name, so one naming a domain name DEPENDS on
    /// that record, and this is the one query that decides whether anything still does.
    pub fn access_associations_depend_on(&self, domain_name_arn: &str) -> bool {
        self.domain_name_access_associations
            .values()
            .any(|association| association.domain_name_arn == domain_name_arn)
    }

    /// The base path mapping that still routes to the API `rest_api_id` — to the stage
    /// `stage_name` of it when one is named, or to the API at all when none is — or `None`
    /// when none does.
    ///
    /// A `BasePathMapping` names "the string identifier of the associated RestApi" and
    /// "the name of the associated stage", so a mapping carrying those is a REFERENCE to
    /// that API and that stage — the same kind of reference
    /// [`Self::integration_referencing_vpc_link`] finds for a VPC link. A mapping is a
    /// record of a custom domain name and a domain name belongs to the ACCOUNT, so the
    /// walk covers every stored domain name; both walks are in key order, so a refusal
    /// NAMES the same mapping on every call for one set of records.
    ///
    /// A mapping whose `stage` is absent, or is the `(none)` spelling that leaves the
    /// stage to the caller, names no stage and so references none — while still
    /// referencing its API.
    pub fn base_path_mapping_referencing_api(
        &self,
        rest_api_id: &str,
        stage_name: Option<&str>,
    ) -> Option<BasePathMappingReference> {
        self.domain_names.values().find_map(|domain| {
            domain
                .base_path_mappings
                .values()
                .find(|mapping| {
                    mapping.rest_api_id == rest_api_id
                        && stage_name.is_none_or(|stage| mapping.stage.as_deref() == Some(stage))
                })
                .map(|mapping| BasePathMappingReference {
                    domain_name: domain.key(),
                    base_path: mapping.base_path.clone(),
                })
        })
    }

    /// The method whose integration still reaches the VPC link `vpc_link_id`, with the
    /// id of the API that method belongs to, or `None` when none does.
    ///
    /// An `Integration` names the link it reaches a VPC through in its `connectionId`
    /// ("The ID of the VpcLink used for the integration when `connectionType=VPC_LINK`"),
    /// so an integration carrying that identifier is a reference to the link. A VPC link
    /// belongs to the ACCOUNT rather than to one API, so the walk covers every stored API
    /// — the same kind of reference [`RestApiEntry::method_referencing_authorizer`] finds
    /// inside one API. The APIs are walked in id order and each one's methods in the
    /// order that lookup fixes, so a refusal NAMES the same reference on every call for
    /// one set of records.
    pub fn integration_referencing_vpc_link(
        &self,
        vpc_link_id: &str,
    ) -> Option<(String, MethodReference)> {
        self.rest_apis.values().find_map(|api| {
            api.referencing_method(|method| {
                method
                    .integration
                    .as_ref()
                    .and_then(|integration| integration.connection_id.as_deref())
                    == Some(vpc_link_id)
            })
            .map(|reference| (api.id.clone(), reference))
        })
    }

    /// The tag collection stored on the resource a `resourceArn` named, as
    /// [`parse_tagged_resource`] read it.
    ///
    /// One lookup for every tag operation, so what a tag read reports and what a tag
    /// write edits can never be two different records. `Err` carries the
    /// `NotFoundException` message for an ARN that is well formed and that no stored
    /// record answers to, in the wording this service already uses for the same
    /// unresolvable identifier: the record the ARN NAMES, which for a stage ARN is the
    /// stage ([`Self::api_and_stage`]) and not the API half of its resource portion — only
    /// an ARN whose own record IS the API reports the API identifier.
    pub fn tags_of(&self, resource: &TaggedResource) -> Result<&BTreeMap<String, String>, String> {
        match resource {
            TaggedResource::RestApi { rest_api_id } => self
                .rest_apis
                .get(rest_api_id)
                .map(|api| &api.tags)
                .ok_or_else(|| no_such_rest_api(rest_api_id)),
            TaggedResource::Stage {
                rest_api_id,
                stage_name,
            } => self
                .api_and_stage(rest_api_id, stage_name)
                .map(|(_, stage)| &stage.tags),
            TaggedResource::ApiKey { api_key_id } => self
                .api_keys
                .get(api_key_id)
                .map(|key| &key.tags)
                .ok_or_else(|| NO_SUCH_API_KEY.to_string()),
            TaggedResource::VpcLink { vpc_link_id } => self
                .vpc_links
                .get(vpc_link_id)
                .map(|link| &link.tags)
                .ok_or_else(|| NO_SUCH_VPC_LINK.to_string()),
            TaggedResource::UsagePlan { usage_plan_id } => self
                .usage_plans
                .get(usage_plan_id)
                .map(|plan| &plan.tags)
                .ok_or_else(|| tagged_usage_plan_not_found(usage_plan_id)),
            TaggedResource::DomainName { domain_name_key } => self
                .domain_names
                .get(domain_name_key)
                .map(|domain| &domain.tags)
                .ok_or_else(|| NO_SUCH_DOMAIN_NAME.to_string()),
            TaggedResource::DomainNameAccessAssociation {
                access_association_id,
            } => self
                .domain_name_access_associations
                .get(access_association_id)
                .map(|association| &association.tags)
                .ok_or_else(|| NO_SUCH_ACCESS_ASSOCIATION.to_string()),
            TaggedResource::ClientCertificate {
                client_certificate_id,
            } => self
                .client_certificates
                .get(client_certificate_id)
                .map(|certificate| &certificate.tags)
                .ok_or_else(|| NO_SUCH_CLIENT_CERTIFICATE.to_string()),
        }
    }

    /// Whether this emulator HOLDS the record `resource` names, for an operation that
    /// resolves an ARN without reading its tags (`RejectDomainNameAccessAssociation`).
    ///
    /// Reached through [`Self::tags_of`], the one lookup that resolves an ARN to a stored
    /// record, so an operation that resolves an ARN can never disagree with the tag
    /// operations about whether that ARN names anything. `Err` carries the same
    /// `NotFoundException` message those operations answer with.
    pub fn holds(&self, resource: &TaggedResource) -> Result<(), String> {
        self.tags_of(resource).map(|_| ())
    }

    /// The same collection for WRITING, so `TagResource` and `UntagResource` edit the
    /// very record [`Self::tags_of`] reports; see there for the failure.
    pub fn tags_of_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Result<&mut BTreeMap<String, String>, String> {
        match resource {
            TaggedResource::RestApi { rest_api_id } => self
                .rest_apis
                .get_mut(rest_api_id)
                .map(|api| &mut api.tags)
                .ok_or_else(|| no_such_rest_api(rest_api_id)),
            TaggedResource::Stage {
                rest_api_id,
                stage_name,
            } => self
                .stage_mut(rest_api_id, stage_name)
                .map(|stage| &mut stage.tags),
            TaggedResource::ApiKey { api_key_id } => self
                .api_keys
                .get_mut(api_key_id)
                .map(|key| &mut key.tags)
                .ok_or_else(|| NO_SUCH_API_KEY.to_string()),
            TaggedResource::VpcLink { vpc_link_id } => self
                .vpc_links
                .get_mut(vpc_link_id)
                .map(|link| &mut link.tags)
                .ok_or_else(|| NO_SUCH_VPC_LINK.to_string()),
            TaggedResource::UsagePlan { usage_plan_id } => self
                .usage_plans
                .get_mut(usage_plan_id)
                .map(|plan| &mut plan.tags)
                .ok_or_else(|| tagged_usage_plan_not_found(usage_plan_id)),
            TaggedResource::DomainName { domain_name_key } => self
                .domain_names
                .get_mut(domain_name_key)
                .map(|domain| &mut domain.tags)
                .ok_or_else(|| NO_SUCH_DOMAIN_NAME.to_string()),
            TaggedResource::DomainNameAccessAssociation {
                access_association_id,
            } => self
                .domain_name_access_associations
                .get_mut(access_association_id)
                .map(|association| &mut association.tags)
                .ok_or_else(|| NO_SUCH_ACCESS_ASSOCIATION.to_string()),
            TaggedResource::ClientCertificate {
                client_certificate_id,
            } => self
                .client_certificates
                .get_mut(client_certificate_id)
                .map(|certificate| &mut certificate.tags)
                .ok_or_else(|| NO_SUCH_CLIENT_CERTIFICATE.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Tagging: the tags this service will STORE on the resource an ARN named.
// Which resource that is comes from `parse_tagged_resource` above, the one place
// this service reads a `resourceArn`; what follows is the one place it reads a tag.
// ---------------------------------------------------------------------------

/// The prefix AWS reserves for its OWN tag keys. From the `tags` member's own
/// documentation: a tag key "must not start with aws:".
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The `BadRequestException` message for a tag whose key or value is outside the tag
/// contract.
///
/// ONE sentence for every violation of that contract — a key longer than a key may be, a
/// value longer than a value may be, a character outside the published set — and it is
/// ENTIRELY FIXED TEXT: it states the contract (the permitted characters and the two
/// length bounds) and names neither the offending key, nor its length, nor which of the
/// rules it broke, so a caller cannot tell the three apart from the message and treats
/// them as the one tag-contract refusal AWS defines. The bounds are spelled from
/// [`MAX_TAG_KEY_CHARS`] / [`MAX_TAG_VALUE_CHARS`], so the sentence and the check it
/// answers for cannot disagree.
pub fn invalid_tag() -> String {
    format!(
        "Invalid tag key/value. Valid characters are letters, numbers, spaces, and the \
         following: _ . : / = + - @. Tag key can be up to {MAX_TAG_KEY_CHARS} characters. \
         Tag value can be up to {MAX_TAG_VALUE_CHARS} characters."
    )
}

/// Refuse one caller-supplied tag this service will not store.
///
/// Every bound comes from the `tags` member's own documentation — "The tag key can be
/// up to 128 characters and must not start with aws:. The tag value can be up to 256
/// characters." — so the one check serves every operation that takes tags, whether the
/// tags ride on a create (`CreateRestApi`, `CreateStage`, `CreateApiKey`) or on
/// `TagResource`. A key or a value outside the contract — an empty key as much as a key or
/// a value past its bound — earns the one tag-contract sentence
/// [`invalid_tag`] states, whichever rule it broke. The member's documented "valid
/// character set is [a-zA-Z+-=._:/]" is NOT enforced: AWS accepts a key and a value
/// outside it (the recorded units `apigateway.tag-resource-unicode-key-value-round-trip`
/// and `apigateway.tag-value-unicode-round-trip` round-trip non-ASCII tags), so refusing
/// them would be stricter than the service.
pub fn bound_tag(key: &str, value: &str) -> Result<(), String> {
    // A key is what names a tag, so a tag has to carry one: an EMPTY key names nothing and
    // is outside the contract exactly as a key past its bound is, which is why it earns the
    // same sentence rather than one of this emulator's own.
    if key.is_empty()
        || key.chars().count() > MAX_TAG_KEY_CHARS
        || value.chars().count() > MAX_TAG_VALUE_CHARS
    {
        return Err(invalid_tag());
    }
    // The reserved namespace is AWS's own, and a prefix is a prefix whatever its case.
    if key
        .to_ascii_lowercase()
        .starts_with(RESERVED_TAG_KEY_PREFIX)
    {
        return Err(reserved_tag_key_prefix());
    }
    Ok(())
}

/// The refusal for a tag key in AWS's own reserved namespace.
///
/// A fixed sentence: it names the reserved prefix and that the rule holds whatever the
/// key's case, and does NOT quote the offending key back. One copy for every operation
/// that takes tags.
pub fn reserved_tag_key_prefix() -> String {
    format!(
        "Invalid tag key. Tag keys cannot start with '{RESERVED_TAG_KEY_PREFIX}' \
         (case insensitive)"
    )
}

/// Refuse a `ThrottleSettings` whose limits are not a request rate API Gateway can
/// meter.
///
/// `burstLimit` is a count of requests ("The API target request burst rate limit") and
/// `rateLimit` a requests-per-second rate ("The API target request rate limit") —
/// `ThrottleSettings`' own members — so neither can be negative, and a rate must be a
/// finite number. Real AWS answers 400 `BadRequestException` for a negative or otherwise
/// invalid throttle rate, with ONE fixed sentence that covers both members and quotes
/// neither value back: [`INVALID_THROTTLE_SETTINGS`].
///
/// One check for every place this shape rides on a request — a usage plan's own
/// `throttle`, the per-method throttle map on each of its `apiStages` entries, and any
/// later operation that takes the shape — so one value is refused the same way wherever
/// it appears.
pub fn bound_throttle_settings(throttle: &ThrottleSettings) -> Result<(), String> {
    if !accepts_throttle_burst(throttle.burst_limit) || !accepts_throttle_rate(throttle.rate_limit)
    {
        return Err(INVALID_THROTTLE_SETTINGS.to_string());
    }
    Ok(())
}

/// Whether `burst_limit` is a request burst API Gateway can meter: "The API target request
/// burst rate limit" (`ThrottleSettings$burstLimit`) is a COUNT of requests, so it cannot be
/// negative. The rule lives here once, so the member is judged the same whether a whole
/// structure rides on a request or a patch names the one member
/// ([`bound_throttle_settings`]).
pub fn accepts_throttle_burst(burst_limit: i32) -> bool {
    burst_limit >= 0
}

/// Whether `rate_limit` is a request rate API Gateway can meter: "The API target request
/// rate limit" (`ThrottleSettings$rateLimit`) is requests per second, so it has to be a
/// finite, non-negative number. See [`accepts_throttle_burst`].
pub fn accepts_throttle_rate(rate_limit: f64) -> bool {
    rate_limit.is_finite() && rate_limit >= 0.0
}

/// The refusal for a usage plan's throttle settings, whichever of the two limits is not a
/// rate: one fixed sentence naming both members and the rule they break.
pub const INVALID_THROTTLE_SETTINGS: &str =
    "Usage Plan throttle rate limit and burst limit must be numeric and greater than zero";

/// Refuse a `QuotaSettings` whose counts are not a quota API Gateway can meter.
///
/// `limit` is "The target maximum number of requests that can be made in a given time
/// period" and `offset` "The number of requests subtracted from the given limit in the
/// initial time period" (`QuotaSettings`' own members): both count requests, so neither
/// can be negative, and real AWS answers 400 `BadRequestException` for a negative quota
/// count with a fixed sentence per member that names the member and the rule, and quotes
/// no value back (see [`invalid_quota_count`]).
///
/// `period` is checked for the WINDOW it gives the offset: a value outside the modelled
/// `QuotaPeriodType` enum is refused at the boundary before any handler runs, but a value
/// INSIDE it fixes how large an initial-period offset can be — see
/// [`max_quota_offset`].
pub fn bound_quota_settings(quota: &QuotaSettings) -> Result<(), String> {
    for (count_member, count) in [("limit", quota.limit), ("offset", quota.offset)] {
        if count < 0 {
            return Err(invalid_quota_count(count_member));
        }
    }
    if let Some(period) = quota.period.as_deref() {
        if let Some(max) = max_quota_offset(period) {
            if quota.offset > max {
                return Err(invalid_quota_offset(period, max));
            }
        }
    }
    Ok(())
}

/// The largest `offset` a usage plan's quota may carry in the metering period `period`,
/// or `None` for a period this service knows no window for.
///
/// `offset` is "the number of requests subtracted from the given limit in the INITIAL
/// time period" (`QuotaSettings$offset`), and AWS counts that offset in the period's own
/// units — days: it shifts the start of the first window, so it has to fall inside the
/// window it shifts. A `DAY` period is one day long and so admits no shift at all; a
/// `WEEK` is seven days and a `MONTH` twenty-eight, so the offset may name any day of
/// them but the last, which is the next period's first.
pub fn max_quota_offset(period: &str) -> Option<i32> {
    match period {
        "DAY" => Some(0),
        "WEEK" => Some(6),
        "MONTH" => Some(27),
        _ => None,
    }
}

/// The refusal for an offset outside the window its metering period gives it: the fixed
/// sentence AWS answers a nonzero offset in the `DAY` period with, stated over the period
/// that fixed the window and the bound it broke.
pub fn invalid_quota_offset(period: &str, max: i32) -> String {
    if max == 0 {
        return format!("Usage Plan quota offset must be zero in the {period} period");
    }
    format!("Usage Plan quota offset must be between 0 and {max} in the {period} period")
}

/// The refusal for one negative count of a usage plan's quota: the fixed sentence AWS
/// answers a negative quota `limit` with, over the member that carried it.
pub fn invalid_quota_count(count_member: &str) -> String {
    format!("Usage Plan quota {count_member} must be a non-negative numeric")
}

/// The `BadRequestException` message for a usage plan quota `period` outside the set the
/// modelled `QuotaPeriodType` declares.
///
/// API Gateway answers this condition with ONE sentence of entirely fixed text: it names
/// the setting that was refused and enumerates the periods it takes — in the order the
/// model declares them, which is the period's own length
/// (`smithy_query(query="shape", shape="QuotaPeriodType")`: DAY, WEEK, MONTH) rather than
/// the alphabetical order the generated value list holds — and quotes neither the rejected
/// value nor the patch path that carried it. One copy for every member a caller can set a
/// quota period through, so the same value is refused the same way whichever one carried
/// it.
pub const INVALID_QUOTA_PERIOD: &str =
    "Invalid Usage Plan quota period specified. Must be one of [DAY, WEEK, MONTH]";

/// The smallest share of a stage's traffic a canary may take: "The percent (0-100) of
/// traffic diverted to a canary deployment" (`CanarySettings$percentTraffic`).
pub const MIN_CANARY_PERCENT_TRAFFIC: i32 = 0;

/// The largest share of a stage's traffic a canary may take; see
/// [`MIN_CANARY_PERCENT_TRAFFIC`].
pub const MAX_CANARY_PERCENT_TRAFFIC: i32 = 100;

/// The canary traffic share a stage may be stored with, or the reason the requested one
/// is refused.
///
/// One check for every operation that sets canary settings — the stage's own
/// `canarySettings`, the ones a deployment is created with, and a patch of either — so
/// one percentage is refused the same way wherever it rides.
pub fn accepted_percent_traffic(requested: f64) -> Result<f64, String> {
    let window = f64::from(MIN_CANARY_PERCENT_TRAFFIC)..=f64::from(MAX_CANARY_PERCENT_TRAFFIC);
    if !requested.is_finite() || !window.contains(&requested) {
        return Err(invalid_percent_traffic());
    }
    Ok(requested)
}

/// The refusal for a canary traffic share outside that window: a fixed sentence naming
/// the stage's canary and the window's bounds, which quotes the rejected share back
/// nowhere.
pub fn invalid_percent_traffic() -> String {
    format!(
        "Invalid traffic settings for stage canary. Must be between \
         {MIN_CANARY_PERCENT_TRAFFIC} and {MAX_CANARY_PERCENT_TRAFFIC}"
    )
}

/// The `NotFoundException` message for a `restApiId` that names no stored API.
///
/// API Gateway does not state a fixed sentence here: it names the identifier it could
/// not resolve, qualified by the account the lookup ran in —
/// `Invalid API identifier specified <accountId>:<restApiId>` — so the message is BUILT
/// FROM THE REQUEST. The account segment is the emulator's one account
/// ([`::emulator_server::emulator_account_id`]), never a literal. Shared so every
/// operation that resolves an API says the same thing about the same failure.
pub fn no_such_rest_api(rest_api_id: &str) -> String {
    format!(
        "Invalid API identifier specified {}:{rest_api_id}",
        ::emulator_server::emulator_account_id()
    )
}

/// The `BadRequestException` message for a `restApiId` that resolves to no stored API on
/// an operation that reads the identifier while VALIDATING the submitted request, rather
/// than while resolving the record the request addresses.
///
/// Unlike [`no_such_rest_api`], this is a SENTENCE OF ITS OWN: nothing is interpolated
/// from the request, so the rejected identifier is not quoted back and the account is not
/// named. Which operations classify an unresolvable API this way is each operation's own
/// contract — a read or a write of a record under the API answers the identifier-naming
/// [`no_such_rest_api`] as a `NotFoundException` instead — so this sentence is shared by
/// the operations whose contract says a bad API identifier is a bad request.
pub const INVALID_REST_API_IDENTIFIER: &str = "Invalid REST API identifier specified";

/// The `NotFoundException` message for a `resourceId` that names no stored resource.
pub const NO_SUCH_RESOURCE: &str = "Invalid Resource identifier specified";

/// The `NotFoundException` message for an `httpMethod` with no method on the resource.
pub const NO_SUCH_METHOD: &str = "Invalid Method identifier specified";

/// The `NotFoundException` message for a method that carries no integration.
pub const NO_SUCH_INTEGRATION: &str = "Invalid Integration identifier specified";

/// The `BadRequestException` message for a SIMULATED invocation of a method that carries
/// no integration: simulating an invocation invokes the method's integration, so a method
/// with none leaves the call nothing to invoke and the call itself is refused.
///
/// A SENTENCE OF ITS OWN, like [`INVALID_REST_API_IDENTIFIER`]: nothing is interpolated
/// from the request, so neither the API, the resource nor the verb is named.
pub const NO_INTEGRATION_DEFINED: &str = "No integration defined for method";

/// The `NotFoundException` message for a `statusCode` that names no response of the
/// method at that code — the method response `GetMethodResponse` resolves, or the
/// integration response `GetIntegrationResponse` resolves on the method's integration.
/// Both are keyed by the same `StatusCode`, so both say the same thing about it.
pub const NO_SUCH_METHOD_RESPONSE: &str = "Invalid Response status code specified";

/// The `NotFoundException` message for a `stageName` that names no stage of the API.
pub const NO_SUCH_STAGE: &str = "Invalid stage identifier specified";

/// The `ConflictException` message for a `stageName` the addressed API already has a stage
/// under: a stage name names ONE stage of an API, so creating a second one under that name
/// is refused rather than replacing the stage the first one stored.
///
/// A SENTENCE OF ITS OWN, like [`METHOD_RESPONSE_ALREADY_EXISTS`]: API Gateway states this
/// collision as entirely fixed text and quotes back neither the name the request sent nor
/// the API it was sent to. One copy, so every call that refuses a stage-name collision
/// words it the same way.
pub const STAGE_ALREADY_EXISTS: &str = "Stage already exists";

/// The `NotFoundException` message for a `deploymentId` that names no stored deployment.
/// The resource type is CAPITALISED here, as it is in the `Resource` / `Method` /
/// `Integration` siblings above and unlike the `stage` one below: that is the spelling the
/// service answers for a deployment, and one sentence serves every operation that resolves
/// one, so all of them say the same thing about the same failure.
pub const NO_SUCH_DEPLOYMENT: &str = "Invalid Deployment identifier specified";

/// The `BadRequestException` message for a `CreateDeployment` of an API that carries no
/// method on any of its resources.
///
/// A deployment is the snapshot of the API's methods a stage serves, so an API with no
/// method has nothing to deploy. A SENTENCE OF ITS OWN, like
/// [`INVALID_REST_API_IDENTIFIER`]: nothing is interpolated from the request, so the API
/// is not named back.
pub const NO_METHODS_TO_DEPLOY: &str = "The REST API doesn't contain any methods";

/// The `NotFoundException` message for an `apiKey` that names no stored API key.
pub const NO_SUCH_API_KEY: &str = "Invalid API Key identifier specified";

/// The `NotFoundException` message for a `usagePlanId` that names no stored usage plan,
/// in the shape of the sibling identifier messages above. Shared by every operation that
/// resolves a usage plan, so all of them say the same thing about the same failure.
///
/// Also the answer for a `(usagePlanId, keyId)` pair that names no stored association:
/// a `UsagePlanKey` is addressed under the plan that meters it, so a pair that resolves
/// to nothing resolves to no usage plan key of that plan.
pub const NO_SUCH_USAGE_PLAN: &str = "Invalid Usage Plan ID specified";

/// The `NotFoundException` message for a usage plan ARN that no stored plan answers to,
/// as the TAG operations word it.
///
/// The tag operations resolve a resource ARN rather than a `usagePlanId` path label, and
/// for a usage plan they report the record they could not find by its modelled resource
/// type and the id the ARN carried, instead of the fixed identifier sentence
/// [`NO_SUCH_USAGE_PLAN`] the usage-plan operations answer. One copy, reached through
/// [`ComAmazonawsApigatewayBackplanecontrolserviceState::tags_of`] /
/// [`ComAmazonawsApigatewayBackplanecontrolserviceState::tags_of_mut`], so every tag
/// operation says the same thing about the same ARN.
pub fn tagged_usage_plan_not_found(usage_plan_id: &str) -> String {
    format!("UsagePlan {usage_plan_id} was not found")
}

/// The `NotFoundException` message for an `apiStages` entry whose API resolves and that
/// names no stage OF that API.
///
/// The entry addresses one stage of one API, so this refusal names the association that
/// does not exist — `<apiId>:<stageName>`, the spelling an `apiStages` entry is addressed
/// by. Reached only through [`resolve_api_stage`], which resolves the API identifier
/// first, so the pair is never named for an identifier that resolves to no API at all.
pub fn no_such_api_stage(api_id: &str, stage_name: &str) -> String {
    format!("API Stage not found: {api_id}:{stage_name}")
}

/// Resolve one `apiStages` entry against the account's stored REST APIs, in the order the
/// pair's two identifiers are nested in.
///
/// An entry names a stage OF an API, so the API identifier is what is resolved first: an
/// identifier no stored API answers to is refused as the account-qualified identifier
/// sentence this service states for one wherever it resolves an API ([`no_such_rest_api`]),
/// naming that identifier alone; only an API that DOES resolve has its stages looked at,
/// and a stage it does not carry is [`no_such_api_stage`]. Shared by every operation that
/// associates a usage plan with a stage, so the create and the update say the same thing
/// about the same unresolvable entry.
pub fn resolve_api_stage(
    rest_apis: &BTreeMap<String, RestApiEntry>,
    api_id: &str,
    stage_name: &str,
) -> Result<(), String> {
    let api = rest_apis
        .get(api_id)
        .ok_or_else(|| no_such_rest_api(api_id))?;
    if !api.stages.contains_key(stage_name) {
        return Err(no_such_api_stage(api_id, stage_name));
    }
    Ok(())
}

/// The `BadRequestException` message for a `DeleteUsagePlan` of a plan that is still
/// associated with API stages.
///
/// The association is what makes the plan gate traffic on that stage, so the plan cannot
/// go while it stands: the caller has to disassociate the stages first (an
/// `UpdateUsagePlan` patch that removes them). Built FROM STORED STATE the way
/// [`domain_name_has_access_associations`] is — the plan the request addressed, and one of
/// the associations standing in its way, in the `<apiId>:<stageName>` spelling an
/// `apiStages` entry is addressed by — so the caller can see which call to make first.
pub fn usage_plan_has_api_stages(usage_plan_id: &str, api_id: &str, stage_name: &str) -> String {
    format!(
        "Cannot delete Usage Plan {usage_plan_id} because there are API Stages associated \
         with it, including {api_id}:{stage_name}"
    )
}

/// The `NotFoundException` message for a `domainName` (with its `domainNameId`, for a
/// private one) that names no stored custom domain name, in the shape of the sibling
/// identifier messages above. Shared by every operation that resolves a domain name, so
/// all of them say the same thing about the same failure.
pub const NO_SUCH_DOMAIN_NAME: &str = "Invalid domain name identifier specified";

/// The `BadRequestException` message for a `domainName` this account already holds a
/// non-private custom domain name under.
///
/// `CreateDomainName` declares both `BadRequestException` (400) and `ConflictException`
/// (409) (`smithy_query(query="operation", operation="CreateDomainName")`), and API Gateway
/// answers THIS condition with the 400: a domain name is a globally unique name, so a
/// second create under one name is a request that cannot be served rather than a transient
/// conflict a caller retries. A SENTENCE OF ITS OWN, ending in a full stop and quoting back
/// neither the name the request sent nor the record it collided with.
pub const DOMAIN_NAME_ALREADY_EXISTS: &str = "The domain name you provided already exists.";

/// The `NotFoundException` message for a `basePath` the addressed custom domain name
/// carries no mapping for, in the shape of the sibling identifier messages above. Shared
/// by every operation that resolves a base path mapping, so all of them say the same thing
/// about the same failure.
pub const NO_SUCH_BASE_PATH_MAPPING: &str = "Invalid base path mapping identifier specified";

/// The `ConflictException` message for a base path the addressed custom domain name
/// already carries a mapping for: the base path "must be unique for all of the mappings
/// across a single API" of the domain name, so the second create of one base path is
/// refused rather than replacing the mapping the first one stored (replacing it is
/// `UpdateBasePathMapping`'s own call).
pub const BASE_PATH_MAPPING_ALREADY_EXISTS: &str = "Base path already exists for this domain name";

/// The `NotFoundException` message for a domain name access association ARN that names no
/// stored association.
///
/// API Gateway words this condition as a SENTENCE OF ITS OWN rather than as an identifier
/// refusal like the siblings above, because an access association is only ever reached
/// through the private custom domain name it is for: an ARN that answers to no stored
/// record is a domain name that has no such association. Shared by every operation that
/// resolves one — the delete, the rejection and the tag operations — so all of them say the
/// same thing about the same failure.
pub const NO_SUCH_ACCESS_ASSOCIATION: &str =
    "This private custom domain name doesn't have any domain name access associations.";

/// The `BadRequestException` message for a `(domain name, access association source)` pair
/// this account already holds a domain name access association for.
///
/// An access association IS that pair, so the second create of one is refused rather than
/// stored twice — and API Gateway refuses it as a BAD REQUEST (the request names a resource
/// that is already there, which no retry can change) rather than as a conflict. The
/// sentence is ENTIRELY FIXED TEXT: it names the two things the pair is made of by their
/// kind — the VPC endpoint the access association source is ("For a VPCE, the value is the
/// VPC endpoint ID", `accessAssociationSource`, whose source type enum declares `VPCE`
/// alone) and the domain name — and quotes back neither the source the request named nor
/// the domain name it was resolved against, as a recorded refusal of a repeated pair shows.
pub const ACCESS_ASSOCIATION_ALREADY_EXISTS: &str =
    "The domain name access association between this VPC endpoint and domain name already \
     exists.";

/// The prefix every VPC endpoint id carries, which is also what tells it apart from the
/// OTHER ids EC2 mints under a longer prefix of its own (an endpoint SERVICE's
/// `vpce-svc-…`, an endpoint connection's `vpce-conn-…`): those carry a second hyphen
/// where an endpoint id carries only hexadecimal digits.
pub const VPC_ENDPOINT_ID_PREFIX: &str = "vpce-";

/// The widths of the hexadecimal part of a VPC endpoint id, from EC2's own id pattern
/// (`^vpce-([0-9a-f]{8}|[0-9a-f]{17})$`): the two EC2 mints.
pub const VPC_ENDPOINT_ID_HEX_WIDTHS: &[usize] = &[8, 17];

/// Whether `value` is the id of a VPC endpoint: [`VPC_ENDPOINT_ID_PREFIX`] and one of the
/// hexadecimal widths EC2 mints ([`VPC_ENDPOINT_ID_HEX_WIDTHS`]), in lower case.
///
/// The ONE reading of that form in this crate, so a member that names a VPC endpoint reads
/// it the same way wherever it is read: the `accessAssociationSource` of a `VPCE` access
/// association ("For a VPCE, the value is the VPC endpoint ID",
/// `CreateDomainNameAccessAssociation$accessAssociationSource`) and the first label of an
/// interface endpoint's own `execute-api` DNS name, which the data plane resolves a private
/// api's source endpoint from ([`crate::service::invoke`]).
#[must_use]
pub fn is_vpc_endpoint_id(value: &str) -> bool {
    let Some(hex) = value.strip_prefix(VPC_ENDPOINT_ID_PREFIX) else {
        return false;
    };
    VPC_ENDPOINT_ID_HEX_WIDTHS.contains(&hex.len())
        && hex
            .chars()
            .all(|glyph| glyph.is_ascii_hexdigit() && !glyph.is_ascii_uppercase())
}

/// The `BadRequestException` message for an `accessAssociationSource` that is not the id of
/// a VPC endpoint of the account, which is what a `VPCE` access association's source must be
/// ("For a VPCE, the value is the VPC endpoint ID").
///
/// VERBATIM from the recorded refusal, which asserts the sentence exactly: the VARIABLE part
/// is the identifier the request sent, and the fixed text is " is not a valid VPC endpoint
/// id". ONE sentence for the whole class — a value that is not shaped like an endpoint id and
/// one that is but names no endpoint of the account are the same fact about the same member,
/// so both are refused with it.
pub fn invalid_access_association_source(access_association_source: &str) -> String {
    format!("{access_association_source} is not a valid VPC endpoint id")
}

/// The `ConflictException` message for a custom domain name a stored
/// `DomainNameAccessAssociation` still depends on, which is what `DeleteDomainName`
/// refuses while the dependency stands (see
/// [`ComAmazonawsApigatewayBackplanecontrolserviceState::access_associations_depend_on`]).
///
/// AWS publishes no fixed sentence for this condition, so the message is BUILT FROM
/// STORED STATE the way [`no_such_rest_api`] is: it names the record the request
/// addressed and the dependent resource kind, in the modelled spelling of both
/// (`DomainName`, `DomainNameAccessAssociation`), so the caller can see which call to
/// make first.
pub fn domain_name_has_access_associations(domain_name: &str) -> String {
    format!(
        "The domain name {domain_name} has domain name access associations. Delete the \
         domain name access associations before deleting the domain name."
    )
}

/// The `BadRequestException` message for a custom domain name that still holds a
/// `BasePathMapping`, which is what `DeleteDomainName` refuses while the mapping stands.
///
/// A mapping sends the calls arriving on its base path of this domain name to a REST API,
/// so removing the domain name would take a mapping a client still reaches an API through
/// with it. AWS publishes no fixed sentence for this condition, so the message is BUILT
/// FROM STORED STATE the way the sibling dependency sentences above are: it names the
/// record the request addressed and the base path of the mapping standing in its way, so
/// the caller can see which `DeleteBasePathMapping` to make first.
pub fn domain_name_has_base_path_mappings(domain_name: &str, base_path: &str) -> String {
    format!(
        "The domain name {domain_name} has base path mapping {base_path}. Delete the base \
         path mappings before deleting the domain name."
    )
}

/// The `BadRequestException` message for a stage a `BasePathMapping` still names, which is
/// what `DeleteStage` refuses while the reference stands (see
/// [`ComAmazonawsApigatewayBackplanecontrolserviceState::base_path_mapping_referencing_api`]).
///
/// BUILT FROM STATE, in the shape of this service's sibling in-use sentences
/// ([`authorizer_in_use`], [`vpc_link_in_use`]): the fixed text names the condition and
/// the variable parts are the stage's own name and the referencing mapping
/// ([`BasePathMappingReference`]), so a client is told which record has to go first.
pub fn stage_in_use_by_base_path_mapping(
    stage_name: &str,
    referencing: &BasePathMappingReference,
) -> String {
    format!("Cannot delete stage '{stage_name}', is referenced in base path mapping: {referencing}")
}

/// The `BadRequestException` message for a REST API a `BasePathMapping` still names, which
/// is what `DeleteRestApi` refuses while the reference stands.
///
/// The same condition one level up: a mapping names the API as well as the stage of it, so
/// deleting the API would leave the mapping pointing at an API that does not exist. Worded
/// as [`stage_in_use_by_base_path_mapping`] words it over the stage, so the two refusals of
/// one reference differ only in which record they name.
pub fn rest_api_in_use_by_base_path_mapping(
    rest_api_id: &str,
    referencing: &BasePathMappingReference,
) -> String {
    format!(
        "Cannot delete REST API '{rest_api_id}', is referenced in base path mapping: \
         {referencing}"
    )
}

/// The `BadRequestException` message for ANY caller-supplied value this service could
/// not read as an ARN — a `resourceArn` the tag operations take (the refusal
/// [`parse_tagged_resource`]'s `None` earns), a `targetArns` entry, a domain name or an
/// access-association ARN, and the `cloudwatchRoleArn` an account's patch carries.
///
/// ONE sentence and one spelling of it, and ENTIRELY FIXED TEXT: it states that the
/// request carried a value this API could not read as an ARN and quotes NEITHER the value
/// nor the member that carried it, as a recorded refusal of an unreadable `resourceArn`
/// shows. One copy for every member an ARN rides on, so the same unreadable value is
/// refused the same way whichever one carried it.
pub const INVALID_ARN: &str = "Invalid ARN specified in the request";

/// The `BadRequestException` message for a value carried on a member typed as the ARN of
/// a PRIVATE CUSTOM DOMAIN NAME that this service could not READ as one — the
/// `domainNameArn` the access-association operations take, and the
/// `domainNameAccessAssociationArn` whose own resource portion names the domain name the
/// association is for.
///
/// API Gateway words this condition as a sentence OF ITS OWN rather than as the generic
/// unreadable-ARN sentence ([`INVALID_ARN`]) a `resourceArn` earns, and the sentence is
/// ENTIRELY FIXED TEXT: it names the member's kind of ARN and quotes back neither the
/// value nor the member that carried it, as recorded refusals of both a value that is no
/// ARN at all and an ARN whose resource portion is no domain name reference show. One copy
/// for every member a domain name's ARN rides on, so the same unreadable value is refused
/// the same way whichever one carried it.
pub const INVALID_DOMAIN_NAME_ARN: &str = "Invalid domain name ARN specified.";

/// The prefix every ARN begins with: "arn:", the first field of the Amazon Resource Name
/// format, which is what a value has to carry to be read as one at all.
pub const ARN_PREFIX: &str = "arn:";

/// The `BadRequestException` message for a value set on an ARN-valued member that is no
/// ARN at all: it does not begin with [`ARN_PREFIX`].
///
/// The service's own unreadable-ARN sentence ([`INVALID_ARN`]), and then — for the one
/// condition it can state in full — the rule the value broke and the value exactly as the
/// request spelled it, which is the only variable part.
pub fn invalid_arn_prefix(value: &str) -> String {
    format!("{INVALID_ARN}. ARNs must start with '{ARN_PREFIX}': {value}")
}

/// Refuse a value an ARN-valued member was set to that is no ARN: every ARN begins with
/// [`ARN_PREFIX`], so a value that does not is not one, whichever member carried it.
pub fn require_arn(value: &str) -> Result<(), String> {
    if value.starts_with(ARN_PREFIX) {
        return Ok(());
    }
    Err(invalid_arn_prefix(value))
}

/// Refuse a value set on a member that names an Amazon Web Services-managed CERTIFICATE.
///
/// `certificateArn` and `regionalCertificateArn` are "the reference to an Amazon Web
/// Services-managed certificate … Certificate Manager is the only supported source", and
/// `ownershipVerificationCertificateArn` is "the ARN of the public certificate issued by
/// ACM", so such a member names ONE kind of resource: a certificate in Certificate
/// Manager. What that ARN looks like comes from [`::service_common::arn::ACM_CERTIFICATE`]
/// — this emulator's one definition of the form — so no service of it disagrees about what
/// the ARN of a certificate is. A value that is not one (a string that is no ARN at all,
/// an ARN of another vendor, an ARN of another ACM resource) names no certificate the
/// domain name could ever present, and is refused with this service's fixed
/// unreadable-ARN sentence ([`INVALID_ARN`]) before anything is stored.
pub fn require_certificate_arn(value: &str) -> Result<(), String> {
    if ::service_common::arn::ACM_CERTIFICATE.admits(value) {
        return Ok(());
    }
    Err(INVALID_ARN.to_string())
}

/// The `NotFoundException` message for a `documentationPartId` that names no stored
/// documentation part of the API.
pub const NO_SUCH_DOCUMENTATION_PART: &str = "Invalid Documentation part identifier specified";

/// The `NotFoundException` message for a documentation version identifier that names no
/// snapshot of the API, in the shape of the sibling identifier messages above.
pub const NO_SUCH_DOCUMENTATION_VERSION: &str = "Invalid Documentation version specified";

/// The `NotFoundException` message for an `authorizerId` that names no stored authorizer
/// of the API.
pub const NO_SUCH_AUTHORIZER: &str = "Invalid Authorizer identifier specified";

/// The `ConflictException` message for an authorizer a `Method` of the same API still
/// names in its `authorizerId`, which is what `DeleteAuthorizer` refuses while the
/// reference stands (see [`RestApiEntry::method_referencing_authorizer`]).
///
/// BUILT FROM STATE: the fixed text names the condition and the variable parts are the
/// authorizer's own name and the referencing method, so tooling parsing the refusal can
/// find which method still has to be updated or deleted.
pub fn authorizer_in_use(authorizer_name: &str, referencing: &MethodReference) -> String {
    format!("Cannot delete authorizer '{authorizer_name}', is referenced in method: {referencing}")
}

/// The `NotFoundException` message for a `modelName` that names no stored model of the
/// API.
///
/// A model is addressed by its NAME rather than by a minted id, so API Gateway names the
/// name back in the refusal instead of stating the fixed identifier sentence its sibling
/// resources earn: the message is BUILT FROM THE REQUEST, naming the one value the caller
/// has to change.
pub fn no_such_model(model_name: &str) -> String {
    format!("Invalid model name specified: {model_name}")
}

/// The `ConflictException` message for a model name this API already holds a model under:
/// a model is keyed by its name, so the second create under one name is refused rather
/// than replacing the model the first one stored (replacing it is `UpdateModel`'s own
/// call).
///
/// API Gateway words this condition as ONE sentence of entirely fixed text, naming the
/// resource class and the SCOPE a model name is unique within — the REST API that holds
/// the model — and quoting back neither the name the request sent nor the API it was sent
/// to, in the shape of the sibling collision sentences this service answers with
/// ([`BASE_PATH_MAPPING_ALREADY_EXISTS`]).
pub const MODEL_NAME_ALREADY_EXISTS: &str = "Model name already exists for this REST API";

/// The `ConflictException` message for a model a `Method` of the same API still names in
/// its `requestModels`, or one of that method's responses in its `responseModels`, which
/// is what `DeleteModel` refuses while the reference stands (see
/// [`RestApiEntry::reference_to_model`]).
///
/// BUILT FROM STATE: the fixed text names the condition and which SITE still holds the
/// reference — the method request or the method response — and the variable parts are the
/// model's own name and that site, which API Gateway identifies by the referencing
/// method's resource path and verb ([`MethodReference::in_method_request`]) plus, for a
/// response, the status code it is keyed by ([`MethodReference::in_method_response`]) — so
/// a client is told which record still has to be updated.
pub fn model_in_use(model_name: &str, referencing: &ModelReference) -> String {
    let (site, at) = match referencing {
        ModelReference::MethodRequest(method) => ("request", method.in_method_request()),
        ModelReference::MethodResponse(method, status_code) => {
            ("response", method.in_method_response(status_code))
        }
    };
    format!("Cannot delete model '{model_name}', is referenced in method {site}: {at}")
}

/// The `BadRequestException` message for a `name` that is not a model name: "The name of
/// the model. Must be alphanumeric." (`CreateModel$name`), so a name carrying anything
/// else names no model. BUILT FROM THE REQUEST — it names the value that was rejected, in
/// the shape of the sibling invalid-value messages above.
pub fn invalid_model_name(name: &str) -> String {
    format!("Invalid model name specified {name}; a model name must be alphanumeric")
}

/// The `BadRequestException` message for a `contentType` that is not a media type: "The
/// content-type for the model" (`CreateModel$contentType`) is the media type of the
/// payload the model describes, so a value that is not one types nothing. BUILT FROM THE
/// REQUEST, in the shape of the sibling invalid-value messages above.
pub fn invalid_model_content_type(content_type: &str) -> String {
    format!("Invalid content type specified: {content_type}")
}

/// The `BadRequestException` message for a `schema` that is not a schema document for the
/// content type the model is typed with.
///
/// API Gateway answers this whole condition with ONE sentence of entirely fixed text — the
/// result of validating the document, with the one error that the schema is not a schema —
/// naming neither the rejected document, nor the member that carried it, nor how the
/// document failed to read, as a recorded refusal of a malformed schema shows. One copy
/// for every operation that stores a model's schema, so a client classifying this refusal
/// against the operation's other bad requests reads the same sentence whether a create or
/// an update made it.
pub const INVALID_MODEL_SCHEMA: &str = "Invalid model specified: Validation Result: \
     warnings : [], errors : [Invalid model schema specified]";

/// The `BadRequestException` message for a `targetArns` entry a VPC-link create could not
/// read as an ARN.
///
/// A VPC link's target is "The ARN of the network load balancer of the VPC targeted by the
/// VPC link" (`VpcLink$targetArns`), and API Gateway states this member's own sentence for
/// a value that is no ARN: fixed text saying the ARN is not well formed, then the offending
/// value exactly as the request spelled it, which is the only variable part. It is NOT the
/// service-wide [`INVALID_ARN`] sentence — that one is what the members an ARN of a STORED
/// resource rides on answer (a `resourceArn`, a patch value), as their own recorded refusals
/// show.
pub fn target_arn_is_not_well_formed(target_arn: &str) -> String {
    format!("Invalid ARN. ARN is not well formed {target_arn}")
}

/// The `NotFoundException` message for a `vpcLinkId` that names no stored VPC link, in
/// the shape of the sibling identifier messages above.
///
/// The resource kind is spelled `Vpc Link` — two capitalised words — which is the spelling
/// API Gateway answers for a VPC link wherever one is resolved, the path label of a VPC
/// link operation and a `resourceArn` naming one alike. One copy, so no two of them word
/// the same failure differently.
pub const NO_SUCH_VPC_LINK: &str = "Invalid Vpc Link identifier specified";

/// The `BadRequestException` message for a `CreateVpcLink` whose `targetArns` names no
/// target: a VPC link reaches a VPC through a load balancer, so a link with no target
/// reaches nothing.
///
/// API Gateway states a FIXED sentence here and quotes back neither the member name nor
/// the (empty) value it received. `targetArns` is `@required` on the request shape, so a
/// request that leaves the member out never reaches this sentence: the framework refuses
/// it as a missing required member first.
pub const VPC_LINK_NEEDS_A_TARGET: &str = "Need at least one target arn to create a vpc link";

/// The `BadRequestException` message for a VPC link an `Integration` of some API still
/// reaches through its `connectionId`, which is what `DeleteVpcLink` refuses while the
/// reference stands (see
/// [`ComAmazonawsApigatewayBackplanecontrolserviceState::integration_referencing_vpc_link`]).
///
/// BUILT FROM STATE: the fixed text names the condition and the FORMAT the reference is
/// stated in, and the variable parts are the link's own id — the identifier the caller
/// addressed it by — and the reference as that format spells it: the id of the API, the
/// method's verb and the id of the resource the method sits on, so a client is told which
/// integration has to be changed first and can address each part of it.
pub fn vpc_link_in_use(
    vpc_link_id: &str,
    rest_api_id: &str,
    referencing: &MethodReference,
) -> String {
    format!(
        "Cannot delete vpc link. Vpc link '{vpc_link_id}', is referenced in \
         [{rest_api_id}:{}:{}] in format of [RESTApiId:Method:Resource].",
        referencing.http_method, referencing.resource_id
    )
}

/// The `statusMessage` of a VPC link whose provisioning FAILED because one of its target
/// ARNs does not name a network load balancer.
///
/// `VpcLink$statusMessage` is "a description about the VPC link status" and AWS publishes
/// no fixed sentence for a failed one, so the description is BUILT FROM THE REQUEST: it
/// names the target that could not be provisioned, which is the one value the caller has
/// to change.
pub fn vpc_link_target_is_not_a_load_balancer(target_arn: &str) -> String {
    format!(
        "Unable to provision the VPC link: {target_arn} does not name a network load balancer"
    )
}

/// The `NotFoundException` message for a `requestValidatorId` that names no stored
/// request validator of the API, in the shape of the sibling identifier messages above.
pub const NO_SUCH_REQUEST_VALIDATOR: &str = "Invalid Request Validator identifier specified";

/// The `ConflictException` message for a request validator a `Method` of the same API
/// still names in its `requestValidatorId`, which is what `DeleteRequestValidator` refuses
/// while the reference stands (see
/// [`RestApiEntry::method_referencing_request_validator`]).
///
/// BUILT FROM STATE: the fixed text names the condition and the variable parts are the
/// validator's own name and the referencing method, so tooling parsing the refusal can
/// find which method still has to be updated or deleted. A validator stored without a name
/// has none to name, so the quoted name is empty for it — `name` is optional on
/// `CreateRequestValidator`, and nothing invents one per refusal.
pub fn request_validator_in_use(
    validator_name: Option<&str>,
    referencing: &MethodReference,
) -> String {
    let validator_name = validator_name.unwrap_or_default();
    format!("Cannot delete Request Validator '{validator_name}' referenced in method: {referencing}")
}

/// The `NotFoundException` message for a `responseType` the API carries no CUSTOMIZED
/// gateway response for — the record `DeleteGatewayResponse` clears and
/// `PutGatewayResponse` writes. Every modelled type always has the response API Gateway
/// generates itself, so what is missing is the customization and not the type: a fixed
/// sentence saying the type is not defined on the api, which names neither the type nor
/// the API, in the shape of the sibling identifier messages above.
pub const NO_SUCH_GATEWAY_RESPONSE: &str = "Gateway response type not defined on api";

/// The `NotFoundException` message for a `patchOperations` `remove` naming a
/// `responseParameters` entry the gateway response does not carry.
///
/// A `remove` names ONE configured response parameter, so a name the response has no
/// parameter under removes nothing: API Gateway refuses the request rather than reporting
/// a removal that never happened. The sentence is entirely fixed — it names neither the
/// parameter, nor the patch path that carried it, nor the response type — in the shape of
/// the sibling fixed refusals above.
pub const INVALID_RESPONSE_PARAMETER_NAME: &str = "Invalid parameter name specified";

/// The `target` every audit record this service emits carries, so a consumer can select
/// the access trail without reading the rest of the emulator's traces.
///
/// A state-changing operation emits ONE record, at `INFO`, naming the operation, the
/// caller's presented access key id, the resource it changed and the outcome. Metadata
/// only — never a request body, a tag value or a secret, so an API key's `value` is
/// never among the fields.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// The identity a request presents, for an audit record: the access key id in its SigV4
/// credential scope. `emulator_server::extract_akid` answers `"default"` when the
/// request carries no scope. An access key id NAMES a caller and is not a secret — the
/// signature and the secret key are never read here.
pub fn request_caller(parts: &http::request::Parts) -> String {
    ::emulator_server::extract_akid(&parts.headers)
}

/// The Region a request is scoped to: the Region of its SigV4 credential scope, or
/// [`DEFAULT_REGION`] for a request that carries none.
///
/// A custom domain name's ARN and its regional endpoint name both name the Region the
/// record was created in, which is the Region the caller addressed.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region a request that carries no SigV4 credential scope is treated as addressing:
/// `us-east-1`, AWS's own default Region for a client configured with none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of a stored resource of this service, whose resource portion is the URI of
/// the operation that reads it (see the "Taggable resources" section above).
///
/// API Gateway's own ARNs carry an EMPTY account field —
/// `arn:aws:apigateway:{region}::/domainnames/{domainName}` — so this is not
/// [`::emulator_server::emulator_arn`], which always writes the account in. The
/// partition and the vendor are the ones [`parse_tagged_resource`] reads back, so an ARN
/// this service REPORTS is one it can resolve again.
pub fn service_arn(region: &str, resource_path: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}:{region}::{resource_path}")
}

/// The ARN of the stage `stage_name` of the api `rest_api_id`, created in `region`: the
/// ARN a web ACL is put in front of a stage BY, and the one every reader of that stage
/// resolves its web ACL through.
pub fn stage_arn(region: &str, rest_api_id: &str, stage_name: &str) -> String {
    tagged_resource_arn(
        region,
        &TaggedResource::Stage {
            rest_api_id: rest_api_id.to_string(),
            stage_name: stage_name.to_string(),
        },
    )
}

/// The ARN of the web ACL WAF currently has in front of one stage of this service, or
/// `None` when no web ACL is associated with it.
///
/// `Stage$webAclArn` is "The ARN of the WebAcl associated with the Stage", and that
/// association is WAFv2's own record — `AssociateWebACL` binds a web ACL to the stage's
/// ARN and `DisassociateWebACL` takes it away — so it is READ from there on every report
/// rather than copied onto the stage: a stage cannot then report a binding WAF no longer
/// holds, and a binding made before or after the stage was read needs nothing written
/// here. It is the same record the data plane asks for its decision
/// (`crate::service::invoke`), so the report and the edge cannot disagree.
///
/// Call it with NO lock of this service's store held: it locks the wafv2 store, and that
/// service holds no record of this one, so the two are never held at the same time.
pub fn associated_web_acl_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    rest_api_id: &str,
    stage_name: &str,
) -> Option<String> {
    let arn = stage_arn(region, rest_api_id, stage_name);
    let waf = ::service_com_amazonaws_wafv2_awswaf_20190729::state::service_state(engine)?;
    let store = waf.lock();
    store.web_acl_associations.get(&arn).cloned()
}

/// The ARN of the stored custom domain name addressed by `key` (a [`domain_name_key`]).
pub fn domain_name_arn(region: &str, key: &str) -> String {
    tagged_resource_arn(
        region,
        &TaggedResource::DomainName {
            domain_name_key: key.to_string(),
        },
    )
}

/// The ARN, in `region`, of the resource a [`TaggedResource`] names.
///
/// The inverse of [`parse_tagged_resource`]: the resource portion comes from
/// [`tagged_resource_path`] and the other five fields from [`service_arn`], so every ARN
/// this service reports — in a response member, or to a cross-service reader of the
/// taggable records below — parses back to the record it was built from.
pub fn tagged_resource_arn(region: &str, resource: &TaggedResource) -> String {
    service_arn(region, &tagged_resource_path(resource))
}

/// The ARN of the stored domain name access association addressed by `id`.
///
/// The resource portion is the one [`parse_tagged_resource`] reads back, so the ARN this
/// service REPORTS from a create is the ARN its readers, its tag operations and
/// `RejectDomainNameAccessAssociation` resolve again.
pub fn domain_name_access_association_arn(region: &str, id: &str) -> String {
    tagged_resource_arn(
        region,
        &TaggedResource::DomainNameAccessAssociation {
            access_association_id: id.to_string(),
        },
    )
}

/// The length of an API Gateway-minted API key value: 40 characters, as observed from
/// AWS. A caller-supplied `CreateApiKey$value` is stored as given instead.
const API_KEY_VALUE_LEN: usize = 40;

/// A fresh API key value, for `CreateApiKey` when the request supplies none.
///
/// The value is a secret handed to a caller — a client presents it in `x-api-key` — but
/// it is NOT a credential of this emulator and must never be accepted as one. Nothing in
/// a service may construct a secret itself, so the value is cut from a secret minted by
/// the one component that generates secrets, the emulator's credential store, which is
/// then removed from that store again: the caller gets the secret, the emulator keeps no
/// credential it would accept, and nothing accumulates per call. Only the alphanumeric
/// characters of that secret are kept, because an API key value carries no others while
/// the store's secret alphabet also holds `+` and `/`.
///
/// `None` when the store minted no secret, or one with fewer than
/// [`API_KEY_VALUE_LEN`] usable characters, which leaves the operation to refuse rather
/// than report a value it made up.
pub fn mint_api_key_value(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let value: String = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(API_KEY_VALUE_LEN)
        .collect();
    // The key value is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    (value.chars().count() == API_KEY_VALUE_LEN).then_some(value)
}

/// The instant a record is stamped with, at the resolution this surface reports a
/// timestamp in: WHOLE epoch seconds.
///
/// API Gateway's REST-JSON responses carry `Timestamp` members as whole-second epoch
/// numbers, so a stored instant is truncated to the second when it is taken rather than
/// when it is serialized: a client that parses the member as an integer epoch, and
/// anything that compares or hashes the serialized value, then reads the same value the
/// service reports.
pub fn now() -> ::aws_smithy_types::DateTime {
    let seconds = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(seconds).unwrap_or(0))
}

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's 501
/// rather than a 5xx because there is nothing to retry: no engine of this process will
/// ever serve the call.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the com-amazonaws-apigateway-backplanecontrolservice emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApigatewayBackplanecontrolserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApigatewayBackplanecontrolserviceState>)
}

/// What a cross-service reader of this account's tagged resources reads this service by.
/// Declared here, at the end of the file, because it is the one surface another crate
/// uses and nothing in this module depends on it.
pub mod tag_index;

/// The JSON-Patch grammar every `Update*` operation of this service shares, in a file of
/// its own; re-exported whole, so a handler still reaches each name as `crate::state::…`.
mod patches;
pub use patches::*;

/// The `Model` records every created REST API already holds; see that module.
pub mod default_models;

/// The VPC endpoints the account has, as Amazon EC2 reports them — what an access
/// association's source identifier is resolved against; see that module.
pub mod vpc_endpoints;

/// The usage a usage plan's QUOTA is metered against — the requests each of its API keys
/// sent on each day, and the daily log of used and remaining quota a read reports from
/// them; re-exported whole, so a handler still reaches each name as `crate::state::…`.
mod usage;
pub use usage::*;

/// The `ClientCertificate` records of the account — the material one carries, how long it
/// is valid for and the reference a stage holds on it; re-exported whole, so a handler
/// still reaches each name as `crate::state::…`.
mod client_certificates;
pub use client_certificates::*;

/// The ADDRESS a regional custom domain name publishes as its `regionalDomainName`, and the
/// socket that answers there; re-exported whole, so a handler still reaches each name as
/// `crate::state::…`.
mod custom_domain_endpoint;
pub use custom_domain_endpoint::*;

/// How the identifiers this service ASSIGNS are drawn, and the width each kind of record's
/// id is spelled at; see that module.
pub mod identifier;
