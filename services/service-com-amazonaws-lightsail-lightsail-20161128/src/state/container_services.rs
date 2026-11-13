// SPDX-License-Identifier: CC0-1.0

//! Container services: the published power catalog a service is sized from, the
//! stored service with its deployments and custom domains, and the account's
//! container image registry.

use super::*;

/// `ContainerServiceState` values this slice uses, from the modelled enum and the
/// `ContainerService$state` documentation: `PENDING` while the service is being
/// created, `READY` once it runs without an active deployment, `DEPLOYING` while
/// it launches one, `RUNNING` once a deployment is active, `UPDATING` while "the
/// container service capacity or its custom domains are being updated", and
/// `DISABLED` for one that "is disabled, and its active deployment and
/// containers, if any, are shut down".
pub const CONTAINER_SERVICE_PENDING: &str = "PENDING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_READY: &str = "READY";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_DEPLOYING: &str = "DEPLOYING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_RUNNING: &str = "RUNNING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_UPDATING: &str = "UPDATING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_DISABLED: &str = "DISABLED";

/// The `operationType` the log entry of a `CreateContainerService` call carries,
/// the modelled `OperationType` enum's own `CreateContainerService` member.
///
/// Every mutating container-service call writes the account's operation log the
/// same way the rest of this service's mutating calls do (see
/// [`ComAmazonawsLightsailLightsail20161128State::start_operation`]), so `GetOperations` and
/// `GetOperationsForResource` report it. The entry names the modelled
/// `ResourceType` enum's [`RESOURCE_TYPE_CONTAINER_SERVICE`] member and the
/// service's own name — including for the image-scoped calls, whose resource the
/// enum has no member for.
pub const OPERATION_TYPE_CREATE_CONTAINER_SERVICE: &str = "CreateContainerService";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_UPDATE_CONTAINER_SERVICE: &str = "UpdateContainerService";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_DELETE_CONTAINER_SERVICE: &str = "DeleteContainerService";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_CREATE_CONTAINER_SERVICE_DEPLOYMENT: &str =
    "CreateContainerServiceDeployment";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_REGISTER_CONTAINER_IMAGE: &str = "RegisterContainerImage";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_DELETE_CONTAINER_IMAGE: &str = "DeleteContainerImage";

/// `ContainerServiceDeploymentState` values this slice uses, from the modelled
/// enum and the `ContainerServiceDeployment$state` documentation: `ACTIVATING`
/// while the deployment is being created, `ACTIVE` once it is running (at most
/// one at a time) and `INACTIVE` for one that ran before.
pub const DEPLOYMENT_STATE_ACTIVATING: &str = "ACTIVATING";
/// See [`DEPLOYMENT_STATE_ACTIVATING`].
pub const DEPLOYMENT_STATE_ACTIVE: &str = "ACTIVE";
/// See [`DEPLOYMENT_STATE_ACTIVATING`].
pub const DEPLOYMENT_STATE_INACTIVE: &str = "INACTIVE";

/// The `ContainerServiceStateDetail$code` reported while a deployment is being
/// created, from that member's own documentation: `CREATING_DEPLOYMENT` is one of
/// the codes "possible if your container service is in a `DEPLOYING` or
/// `UPDATING` state".
pub const STATE_DETAIL_CREATING_DEPLOYMENT: &str = "CREATING_DEPLOYMENT";

/// The most container services this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. A create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_CONTAINER_SERVICES: usize = 100;

/// The most public domain names one container service may carry.
///
/// From the modelled `publicDomainNames` documentation: "You can specify up to
/// four public domain names for a container service."
pub const MAX_PUBLIC_DOMAIN_NAMES: usize = 4;

/// The most containers one deployment may describe.
///
/// Emulator resource guard: the modelled `ContainerMap` declares no length, so
/// this is where the bound on a caller-sized map lives.
pub const MAX_CONTAINERS_PER_DEPLOYMENT: usize = 10;

/// How many deployments of one container service the store keeps.
///
/// A service accumulates one deployment per create, so the history a caller can
/// drive has to be bounded — and the service itself bounds it: the
/// `GetContainerServiceDeployments` documentation states that "a set number of
/// deployments are kept before the oldest one is replaced with the newest one",
/// and points at the service's published quotas for the number. The number is
/// not in the model, so the figure here is the emulator's; what it implements is
/// the documented rule that the OLDEST deployment is the one dropped.
pub const MAX_DEPLOYMENTS_PER_SERVICE: usize = 50;

/// The hardware and price one container service power provides.
///
/// The figures are each plan's OWN, from the container service plan table
/// Lightsail publishes — the table the modelled `power` member points
/// `GetContainerServicePowers` at ("The power specifies the amount of RAM, the
/// number of vCPUs, and the base price of the container service"). The plans do
/// NOT step in lockstep: `micro` and `small` carry the same RAM and differ in
/// vCPUs and price, so no entry's figure may be derived from a neighbouring
/// plan's. The `power_id` form is the one the modelled
/// `ContainerServicePower$powerId` documentation shows (`nano-1`).
#[derive(Debug, Clone, Copy)]
pub struct ContainerPowerSpec {
    /// The modelled `ContainerServicePowerName` value that selects this power.
    pub name: &'static str,
    pub power_id: &'static str,
    /// `ContainerServicePower$price`, the base monthly price in USD.
    pub price: f32,
    pub cpu_count: f32,
    pub ram_size_in_gb: f32,
}

/// Every container service power this slice serves, cheapest first. See
/// [`ContainerPowerSpec`] for the source: `nano` with 0.25 vCPUs and 0.5 GB for
/// $7 a month, `micro` with 0.25 vCPUs and 1 GB for $10, `small` with 0.5 vCPUs
/// and 1 GB for $15, `medium` with 1 vCPU and 2 GB for $40, `large` with
/// 2 vCPUs and 4 GB for $80 and `xlarge` with 4 vCPUs and 8 GB for $160.
pub const CONTAINER_SERVICE_POWERS: &[ContainerPowerSpec] = &[
    ContainerPowerSpec {
        name: "nano",
        power_id: "nano-1",
        price: 7.0,
        cpu_count: 0.25,
        ram_size_in_gb: 0.5,
    },
    ContainerPowerSpec {
        name: "micro",
        power_id: "micro-1",
        price: 10.0,
        cpu_count: 0.25,
        ram_size_in_gb: 1.0,
    },
    ContainerPowerSpec {
        name: "small",
        power_id: "small-1",
        price: 15.0,
        cpu_count: 0.5,
        ram_size_in_gb: 1.0,
    },
    ContainerPowerSpec {
        name: "medium",
        power_id: "medium-1",
        price: 40.0,
        cpu_count: 1.0,
        ram_size_in_gb: 2.0,
    },
    ContainerPowerSpec {
        name: "large",
        power_id: "large-1",
        price: 80.0,
        cpu_count: 2.0,
        ram_size_in_gb: 4.0,
    },
    ContainerPowerSpec {
        name: "xlarge",
        power_id: "xlarge-1",
        price: 160.0,
        cpu_count: 4.0,
        ram_size_in_gb: 8.0,
    },
];

/// The power `power` names, or `None` when it is not one this service serves.
pub fn container_service_power(power: &str) -> Option<ContainerPowerSpec> {
    CONTAINER_SERVICE_POWERS
        .iter()
        .find(|spec| spec.name == power)
        .copied()
}

impl ContainerPowerSpec {
    /// This power as the modelled `ContainerServicePower`.
    ///
    /// `name` is "The friendly name of the power (`nano`)" and `powerId` "The ID
    /// of the power (`nano-1`)", so the two are reported from the catalog's own
    /// columns rather than derived from one another.
    pub fn view(&self) -> ContainerServicePower {
        ContainerServicePower {
            power_id: Some(self.power_id.to_string()),
            name: Some(self.name.to_string()),
            price: Some(self.price),
            cpu_count: Some(self.cpu_count),
            ram_size_in_gb: Some(self.ram_size_in_gb),
            // `isActive` is "whether the power is active and can be specified for
            // container services", which is exactly what
            // [`container_service_power`] answers: that lookup is the one a
            // create or an update resolves a requested `power` against, so the
            // flag is read off it rather than restated as a column nothing
            // consults. A power this service stopped accepting therefore reports
            // itself inactive without anything else having to be kept in step.
            is_active: Some(container_service_power(self.name).is_some()),
        }
    }
}

/// The powers a `GetContainerServicePowers` call reports out of `specs`.
///
/// Every entry is reported — the result shape carries no filter and the operation
/// takes no member, so which powers can be SPECIFIED is said by each entry's
/// `isActive` rather than by leaving one out — in `specs`' own order, so two reads
/// answer identically.
pub fn container_service_powers_from(specs: &[ContainerPowerSpec]) -> Vec<ContainerServicePower> {
    specs.iter().map(ContainerPowerSpec::view).collect()
}

/// The container service powers a `GetContainerServicePowers` call reports:
/// [`container_service_powers_from`] over this service's published catalog,
/// [`CONTAINER_SERVICE_POWERS`].
pub fn container_service_powers() -> Vec<ContainerServicePower> {
    container_service_powers_from(CONTAINER_SERVICE_POWERS)
}

/// The power names [`container_service_power`] accepts, for a refusal that has to
/// tell the caller what to send instead.
pub fn container_service_power_names() -> String {
    CONTAINER_SERVICE_POWERS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Container API metadata
// ---------------------------------------------------------------------------

/// The key one container-API metadatum carries its NAME under.
///
/// `ContainerServiceMetadataEntry` is modelled as a free `map<string, string>`
/// (`smithy_query(query="shape",shape="ContainerServiceMetadataEntry")`), so the
/// keys are not in the model: Lightsail publishes each metadatum as the pair
/// `name`/`value`, which is the pair the documented way of reading this catalog
/// selects on (`--query "metadata[?name=='lightsailctlVersion'].value"`).
pub const CONTAINER_API_METADATA_NAME_KEY: &str = "name";

/// The key one container-API metadatum carries its VALUE under. See
/// [`CONTAINER_API_METADATA_NAME_KEY`].
pub const CONTAINER_API_METADATA_VALUE_KEY: &str = "value";

/// The metadatum that reports the current version of the Lightsail Control
/// (lightsailctl) plugin — the one the modelled `metadata` member calls out
/// ("Metadata about Lightsail containers, such as the current version of the
/// Lightsail Control (lightsailctl) plugin").
pub const CONTAINER_API_METADATA_LIGHTSAILCTL_VERSION: &str = "lightsailctlVersion";

/// The lightsailctl plugin release this emulator's container API publishes, as
/// that plugin's `major.minor.patch` version. It is the version a client compares
/// its locally installed plugin against before pushing a container image.
pub const LIGHTSAILCTL_VERSION: &str = "1.0.7";

/// The container-API metadata catalog, as `(name, value)` pairs in the order the
/// service reports them.
///
/// Service-wide published data, like the bundle and blueprint tables above: it is
/// the same for every caller and no operation of this service creates, changes or
/// retires an entry, so it lives here rather than in a per-account record.
pub const CONTAINER_API_METADATA_SPECS: &[(&str, &str)] = &[(
    CONTAINER_API_METADATA_LIGHTSAILCTL_VERSION,
    LIGHTSAILCTL_VERSION,
)];

/// `specs` as the modelled entry list: one entry per metadatum, each a map of its
/// name and its value under [`CONTAINER_API_METADATA_NAME_KEY`] and
/// [`CONTAINER_API_METADATA_VALUE_KEY`].
pub fn container_api_metadata_from(specs: &[(&str, &str)]) -> Vec<HashMap<String, String>> {
    specs
        .iter()
        .map(|(name, value)| {
            HashMap::from([
                (
                    CONTAINER_API_METADATA_NAME_KEY.to_string(),
                    (*name).to_string(),
                ),
                (
                    CONTAINER_API_METADATA_VALUE_KEY.to_string(),
                    (*value).to_string(),
                ),
            ])
        })
        .collect()
}

/// The published container-API metadata catalog, as the modelled entry list.
pub fn container_api_metadata() -> Vec<HashMap<String, String>> {
    container_api_metadata_from(CONTAINER_API_METADATA_SPECS)
}

/// Documented defaults and bounds of a public endpoint's health check, each from
/// its own member's documentation on `ContainerServiceHealthCheckConfig`:
/// "The default value is `2`" (healthy and unhealthy thresholds, and the
/// timeout), "The default value is `5`" (the interval), "The default value is
/// `/`" (the path), and the success codes "between `200` and `499`".
pub(super) const HEALTH_CHECK_DEFAULT_THRESHOLD: i32 = 2;
/// See [`HEALTH_CHECK_DEFAULT_THRESHOLD`].
pub(super) const HEALTH_CHECK_DEFAULT_TIMEOUT_SECONDS: i32 = 2;
/// See [`HEALTH_CHECK_DEFAULT_THRESHOLD`].
pub(super) const HEALTH_CHECK_DEFAULT_INTERVAL_SECONDS: i32 = 5;
/// See [`HEALTH_CHECK_DEFAULT_THRESHOLD`].
pub(super) const HEALTH_CHECK_DEFAULT_PATH: &str = "/";
/// See [`HEALTH_CHECK_DEFAULT_THRESHOLD`].
pub(super) const HEALTH_CHECK_DEFAULT_SUCCESS_CODES: &str = "200-499";
/// "You can specify between 2 and 60 seconds" (`timeoutSeconds`).
pub(super) const HEALTH_CHECK_TIMEOUT_SECONDS_RANGE: (i32, i32) = (2, 60);
/// "You can specify between 5 and 300 seconds" (`intervalSeconds`).
pub(super) const HEALTH_CHECK_INTERVAL_SECONDS_RANGE: (i32, i32) = (5, 300);
/// "You can specify values between `200` and `499`" (`successCodes`).
pub(super) const HEALTH_CHECK_SUCCESS_CODES_RANGE: (i32, i32) = (200, 499);
/// What separates the values of a success-code list: "You can specify multiple
/// values (for example, `200,202`)" (`successCodes`).
pub(super) const SUCCESS_CODES_VALUE_SEPARATOR: char = ',';
/// What separates the two bounds of a success-code range: "or a range of values
/// (for example, `200-299`)" (`successCodes`).
pub(super) const SUCCESS_CODES_RANGE_SEPARATOR: char = '-';

/// How many characters the per-service token of a container service's public
/// address carries, from the example in the modelled `serviceName`
/// documentation (`https://container-service-1.ur4EXAMPLE2uq.us-east-2.cs.amazonlightsail.com`).
pub(super) const DOMAIN_TOKEN_LEN: usize = 13;

/// The alphabet that token is drawn from: the mixed-case alphanumerics the same
/// example shows.
pub(super) const DOMAIN_TOKEN_ALPHABET: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The path space every container service's public address is published under,
/// beneath the endpoint the emulator is reached on.
///
/// AWS publishes that address as a DNS name under its own
/// `cs.amazonlightsail.com` zone, and the modelled `serviceName` documentation
/// gives the textual form `https://<service>.<token>.<region>.cs.amazonlightsail.com`.
/// The HOSTNAME is environment-specific — no caller may assume it — while the
/// PROMISE the field makes is not: `ContainerService$url` is "The publicly
/// accessible URL of the container service", so it has to be an address the
/// caller can actually reach. This emulator owns no DNS zone, so the only
/// address it can be reached on is the endpoint the request arrived at, and the
/// three labels AWS puts in the hostname (region, service name, per-service
/// token) are carried in the path instead. The prefix names this service and
/// this resource type, so it cannot collide with another service's modelled URI
/// space.
pub const CONTAINER_SERVICE_PUBLIC_PATH_PREFIX: &str = "/lightsail-container-service";

/// The suffix of a container service's private domain name, which the Lightsail
/// container services guide gives as `<service name>.service.local` \u2014 resolvable
/// only inside the account's default VPC, as the modelled `privateDomainName`
/// documentation says.
pub(super) const PRIVATE_DOMAIN_SUFFIX: &str = "service.local";

/// The ARN of the container service `id` in `region`.
pub fn container_service_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONTAINER_SERVICE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The publicly accessible URL of the container service `name` in `region`,
/// whose public address carries `token`, as announced to a caller that reached
/// this emulator at `endpoint` (its `Host`/`:authority`).
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on. See
/// [`CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`] for why the address is a path under
/// the request's own endpoint rather than an AWS hostname.
pub fn container_service_url(endpoint: &str, region: &str, name: &str, token: &str) -> String {
    format!("https://{endpoint}{CONTAINER_SERVICE_PUBLIC_PATH_PREFIX}/{region}/{name}/{token}")
}

/// Which container service a request to a published public URL addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerServicePublicTarget {
    pub region: String,
    pub name: String,
    /// The per-service token the announced URL carried, which must be the one
    /// that service was minted with: a URL is addressed to ONE service.
    pub domain_token: String,
}

/// The container service a public-URL request path addresses, or `None` when the
/// path is not one [`container_service_url`] publishes.
///
/// Anything the caller appended BENEATH those three segments is a path within the
/// deployed container: it names no part of this service, and nothing here routes
/// it (no container runs), so it identifies the same service as the announced URL
/// itself and is not carried.
pub fn parse_container_service_public_path(path: &str) -> Option<ContainerServicePublicTarget> {
    let rest = path
        .strip_prefix(CONTAINER_SERVICE_PUBLIC_PATH_PREFIX)?
        .strip_prefix('/')?;
    let mut segments = rest.splitn(4, '/');
    let (Some(region), Some(name), Some(token)) =
        (segments.next(), segments.next(), segments.next())
    else {
        return None;
    };
    if [region, name, token].iter().any(|segment| segment.is_empty()) {
        return None;
    }
    Some(ContainerServicePublicTarget {
        region: region.to_string(),
        name: name.to_string(),
        domain_token: token.to_string(),
    })
}

/// The private domain name of the container service `name`. See
/// [`PRIVATE_DOMAIN_SUFFIX`].
pub fn container_service_private_domain_name(name: &str) -> String {
    format!("{name}.{PRIVATE_DOMAIN_SUFFIX}")
}

/// The principal ARN of the container service `name` in `region`, and the ARN of
/// the role it pulls private container images with.
///
/// The ARN's PATH is emulator-chosen: AWS documents what the principal ARN is
/// FOR (a trust relationship between the caller's own account and the service)
/// but not how its string is built. It is an IAM role ARN carrying the service's
/// region, name and its own domain token, so two services never share one and a
/// value is stable across reads.
pub fn container_service_principal_arn(region: &str, name: &str, token: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("role/amazon/lightsail/{region}/containers/{name}/{token}"),
    )
}

/// See [`container_service_principal_arn`]: the same role, suffixed so the ECR
/// image puller principal is distinguishable from the service's own.
pub fn ecr_image_puller_principal_arn(region: &str, name: &str, token: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("role/amazon/lightsail/{region}/containers/{name}/{token}/ecr-image-puller"),
    )
}

// ---------------------------------------------------------------------------
// The container image registry of the account
// ---------------------------------------------------------------------------

/// How long one set of container image registry log in credentials stays valid.
///
/// A duration the service DOCUMENTS, not a chosen one: the modelled
/// `ContainerServiceRegistryLogin$expiresAt` states it itself — "The log in
/// credentials expire 12 hours after they are created, at which point you will
/// need to create a new set of log in credentials using the
/// `CreateContainerServiceRegistryLogin` action."
pub const REGISTRY_LOGIN_LIFETIME_MS: i64 = 12 * 60 * 60 * 1_000;

/// The path space the container image registry of this account is published
/// under, beneath the endpoint the emulator is reached on.
///
/// `ContainerServiceRegistryLogin$registry` is "The address to use to push
/// container images to the container image registry of a Lightsail account", so
/// it is a promise that the caller which read it can reach that address. AWS
/// publishes the registry as a DNS name of its own; the HOSTNAME is
/// environment-specific while the promise is not, this emulator owns no DNS
/// zone, and a literal AWS hostname would send the next client out of the
/// emulator entirely (see [`request_endpoint`]). So the registry is published
/// beneath the request's own endpoint, with the region AWS puts in its hostname
/// carried in the path instead — exactly as
/// [`CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`] carries a container service's
/// labels. The prefix names this service and this resource, so it cannot
/// collide with another service's modelled URI space.
pub const CONTAINER_IMAGE_REGISTRY_PATH_PREFIX: &str = "/lightsail-container-image-registry";

/// The address a `docker login` — and the pushes after it — are pointed at for
/// this account's registry in `region`, as announced to a caller that reached
/// this emulator at `endpoint` (its `Host`/`:authority`).
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on.
pub fn container_image_registry_url(endpoint: &str, region: &str) -> String {
    format!("https://{endpoint}{CONTAINER_IMAGE_REGISTRY_PATH_PREFIX}/{region}")
}

/// Which registry a request to a published registry address addresses, and what
/// inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerImageRegistryTarget {
    /// The region whose registry the announced address names.
    pub region: String,
    /// What the client appended BENEATH that address: the registry protocol's
    /// own request target (`/v2/…` for a container image registry client), its
    /// leading `/` included, or the empty string where the client appended
    /// nothing at all.
    pub path: String,
}

/// The registry a request to a published registry address addresses, or `None`
/// when the path is not one [`container_image_registry_url`] publishes.
///
/// Anything the caller appended BENEATH the region is the registry protocol's
/// own path (`/v2/…` for a container image registry client): it names no part of
/// this service's modelled URI space, so it is carried through to the registry
/// the address names.
pub fn parse_container_image_registry_path(path: &str) -> Option<ContainerImageRegistryTarget> {
    let rest = path
        .strip_prefix(CONTAINER_IMAGE_REGISTRY_PATH_PREFIX)?
        .strip_prefix('/')?;
    let (region, beneath) = match rest.find('/') {
        Some(cut) => rest.split_at(cut),
        None => (rest, ""),
    };
    (!region.is_empty()).then(|| ContainerImageRegistryTarget {
        region: region.to_string(),
        path: beneath.to_string(),
    })
}

/// The most sets of registry log in credentials this store keeps per region.
///
/// Emulator resource guard, NOT an AWS quota: AWS caps the number of live sets
/// nowhere, and the clock already forgets every set
/// [`REGISTRY_LOGIN_LIFETIME_MS`] after it was created, which bounds the store
/// for any ordinary caller. This bounds it for one that asks for more than that
/// within a single window; the set dropped is the OLDEST, which is the one
/// closest to expiring anyway.
pub const MAX_REGISTRY_LOGINS_PER_REGION: usize = 500;

/// One set of container image registry log in credentials this account was
/// handed: the username it logs in with, what a presented password is checked
/// against, and when the set expires.
///
/// The password itself is NOT kept. It was minted by the emulator's credential
/// store, carried to its caller in the one response that creates it, and only
/// its SHA-256 digest is stored — so nothing can read a usable password back out
/// of this store, the same reason a bucket access key keeps only its id here
/// (see [`StoredBucketAccessKey`]).
#[derive(Debug, Clone)]
pub struct StoredRegistryLogin {
    /// The username this set logs in with, which is also what identifies it: the
    /// id the credential store minted for it.
    pub username: String,
    /// The SHA-256 digest of the password, hex — see [`StoredRegistryLogin`].
    pub(super) password_digest: String,
    pub created_at_ms: i64,
    /// The instant this set expires: [`REGISTRY_LOGIN_LIFETIME_MS`] after it was
    /// created.
    pub expires_at_ms: i64,
}

impl StoredRegistryLogin {
    /// The set created at `created_at_ms` that logs in as `username` with
    /// `password` — which is hashed here and not retained.
    pub fn new(username: String, password: &str, created_at_ms: i64) -> Self {
        Self {
            username,
            password_digest: registry_password_digest(password),
            created_at_ms,
            expires_at_ms: created_at_ms.saturating_add(REGISTRY_LOGIN_LIFETIME_MS),
        }
    }

    /// Whether `password` is the password this set logs in with.
    pub fn accepts(&self, password: &str) -> bool {
        // DIGESTS are compared, not secrets: the stored value is the SHA-256 of
        // a 712-character random token, so what the comparison can tell a
        // guesser is at most how far a digest matched, which nobody can turn
        // back into the password that produced it.
        self.password_digest == registry_password_digest(password)
    }
}

/// The SHA-256 of a registry password, hex — the only form of one this store
/// holds. See [`StoredRegistryLogin`].
pub(super) fn registry_password_digest(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// What a push put IN the container image registry of the account
// ---------------------------------------------------------------------------

/// The algorithm the registry addresses its content by, which is the one the
/// registry protocol's digests name.
pub const REGISTRY_DIGEST_ALGORITHM: &str = "sha256";

/// The number of hex characters a [`REGISTRY_DIGEST_ALGORITHM`] digest is
/// written with: SHA-256 is 32 bytes, two characters each.
pub(super) const REGISTRY_DIGEST_HEX_LEN: usize = 64;

/// The digest of `content`, spelled the way the registry protocol spells one:
/// the algorithm, a colon, and the hex of the digest over exactly those bytes.
///
/// This is computed over the content that arrived, never taken from what a
/// client claimed: it is what makes the registry content-addressed, so a stored
/// blob or manifest is reachable only under the digest of its own bytes.
pub fn registry_digest(content: &[u8]) -> String {
    format!(
        "{REGISTRY_DIGEST_ALGORITHM}:{}",
        hex::encode(Sha256::digest(content))
    )
}

/// Whether `digest` is spelled the way this registry addresses content: the
/// algorithm it holds content under, a colon, and that algorithm's number of
/// lower-case hex characters.
pub fn is_registry_digest(digest: &str) -> bool {
    digest
        .strip_prefix(REGISTRY_DIGEST_ALGORITHM)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|hex| {
            hex.len() == REGISTRY_DIGEST_HEX_LEN
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// The most bytes one blob may put in the registry.
///
/// An emulator resource guard, NOT a registry limit: the registry protocol caps
/// a blob nowhere, and a real image layer is far larger than this. The registry
/// here holds every byte it accepts in this process's memory, so what one caller
/// can make it hold has to be bounded — a push past this is refused rather than
/// half-stored.
pub const MAX_REGISTRY_BLOB_BYTES: usize = 32 * 1024 * 1024;

/// The most bytes one region's registry holds at once — blobs, manifests and
/// in-flight uploads together. The same guard as [`MAX_REGISTRY_BLOB_BYTES`],
/// for the total rather than one blob.
pub const MAX_REGISTRY_BYTES_PER_REGION: usize = 128 * 1024 * 1024;

/// The most blob uploads one region's registry keeps in flight at once.
///
/// An emulator resource guard as well: an upload session holds what has been
/// sent to it so far, and a client that starts sessions without finishing them
/// would otherwise keep them all. The session dropped is the OLDEST, which is
/// the one most likely already abandoned; a client whose session is gone is
/// answered that it is unknown and can start another.
pub const MAX_REGISTRY_UPLOADS_PER_REGION: usize = 64;

/// The most container images one container service's registry holds at once.
///
/// An emulator resource guard, not an AWS quota: a service accumulates one
/// entry per registration, so the count a caller can drive has to be bounded.
/// The entry dropped is the oldest registration.
pub const MAX_CONTAINER_IMAGES_PER_SERVICE: usize = 500;

/// What ONE entry of a region's registry costs this process beyond the bytes it
/// holds: the map node it lives in, the headers of its owned `String`s and
/// `Vec`s, and its fixed fields.
///
/// Deliberately a generous over-estimate of that overhead (a one-byte blob
/// really costs this process on the order of 140 bytes: a 71-character digest
/// key plus two allocation headers, a timestamp and a tree node), so the
/// accounting errs towards refusing rather than towards holding more than
/// [`MAX_REGISTRY_BYTES_PER_REGION`] says. Charging it is what makes that bound
/// a bound on MEMORY and on the entry COUNT: every entry costs at least this
/// much, so one region's registry holds at most
/// `MAX_REGISTRY_BYTES_PER_REGION / REGISTRY_ENTRY_OVERHEAD_BYTES` entries
/// however small the content pushed into them is.
pub const REGISTRY_ENTRY_OVERHEAD_BYTES: usize = 128;

/// What an entry whose owned bytes are `parts` costs a region's registry: those
/// bytes together with [`REGISTRY_ENTRY_OVERHEAD_BYTES`].
///
/// The ONE place an entry's cost is worked out, so what
/// [`StoredImageRegistry::bytes_held`] accounts for and what a write is refused
/// for passing cannot drift apart.
pub(super) fn registry_entry_bytes(parts: &[usize]) -> usize {
    parts.iter().fold(REGISTRY_ENTRY_OVERHEAD_BYTES, |total, part| {
        total.saturating_add(*part)
    })
}

/// One blob the registry holds: the bytes that were pushed, and when they
/// arrived.
#[derive(Debug, Clone)]
pub struct StoredRegistryBlob {
    /// The bytes themselves, held verbatim so a later read answers what was
    /// pushed rather than something built to look like it.
    pub content: Vec<u8>,
    pub pushed_at_ms: i64,
}

/// One image manifest the registry holds: the bytes that were pushed, the media
/// type they were pushed as, the repository they were pushed to, and when.
#[derive(Debug, Clone)]
pub struct StoredRegistryManifest {
    pub content: Vec<u8>,
    /// The media type the manifest is held in, which is what a read of it
    /// answers with.
    ///
    /// One of the manifest media types the registry stores, as the plane that
    /// accepted the push resolved it — never the spelling a client sent, so a
    /// value held here is always a media type this registry chose.
    pub media_type: String,
    pub repository: String,
    pub pushed_at_ms: i64,
}

/// One blob upload in flight: the repository it was started against, what has
/// been sent to it so far, and when it started.
#[derive(Debug, Clone)]
pub struct StoredBlobUpload {
    pub repository: String,
    pub content: Vec<u8>,
    pub started_at_ms: i64,
}

/// What the registry answered to content offered to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryWrite {
    /// The content is held, addressed by this digest.
    Stored(String),
    /// Holding it would take this region's registry past `limit` bytes.
    OutOfSpace { limit: usize },
}

/// What the registry answered to bytes offered to an upload session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadAppend {
    /// The session now holds `held` bytes.
    Appended { held: usize },
    /// No such upload session: never started, started against another
    /// repository, already finished, or dropped to keep the region within
    /// [`MAX_REGISTRY_UPLOADS_PER_REGION`].
    Unknown,
    /// Taking them would pass `limit` bytes.
    OutOfSpace { limit: usize },
}

/// What the registry answered to a request finishing an upload session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadFinish {
    /// The whole blob is held, addressed by this digest.
    Stored(String),
    /// What the session holds has this digest, not the one the request claimed,
    /// so nothing was stored and the session is untouched.
    DigestMismatch { computed: String },
    /// No such upload session — see [`UploadAppend::Unknown`].
    Unknown,
    /// Holding it would pass `limit` bytes.
    OutOfSpace { limit: usize },
}

/// The container image registry of the account in ONE region: the blobs and
/// image manifests pushed to it, the tag each repository's tags resolve to, and
/// the uploads in flight.
///
/// Content-addressed, as the registry protocol is: a blob and a manifest are
/// held under the digest of their own bytes, so the registry can only answer
/// with what was actually pushed to it.
#[derive(Debug, Default)]
pub struct StoredImageRegistry {
    /// Every blob, keyed by its digest. Keyed registry-wide and not per
    /// repository because the digest IS the identity of the content.
    pub blobs: BTreeMap<String, StoredRegistryBlob>,
    /// Every image manifest, keyed by its digest — the digest a `docker push`
    /// reports and `RegisterContainerImage` takes.
    pub manifests: BTreeMap<String, StoredRegistryManifest>,
    /// The manifest digest each repository's tags name, keyed by the repository
    /// and the tag.
    pub tags: BTreeMap<(String, String), String>,
    /// The blob uploads in flight, keyed by the session id the registry minted
    /// for each.
    pub uploads: BTreeMap<String, StoredBlobUpload>,
}

impl StoredImageRegistry {
    /// How many bytes this registry holds: its blobs, its manifests, the tags
    /// pointing at them, and what its in-flight uploads have taken so far.
    ///
    /// Every entry is charged what it costs this PROCESS rather than only the
    /// content it was pushed: the string it is keyed by (a digest, or a
    /// repository and a tag), the owned strings on the record itself, and
    /// [`REGISTRY_ENTRY_OVERHEAD_BYTES`] for the rest of it. Otherwise a client
    /// pushing a stream of distinct one-byte blobs — each one a digest of its
    /// own, so none of them is the blob already there — would be charged a byte
    /// for an entry costing a hundred times that, and
    /// [`MAX_REGISTRY_BYTES_PER_REGION`] would bound neither this process's
    /// memory nor the number of entries one caller can make it hold.
    ///
    /// A tag is counted for the reason it always was: a tag pointing at a
    /// manifest the registry already holds costs no manifest bytes, so without
    /// counting it a client could grow this registry by tagging one manifest
    /// over and over.
    pub fn bytes_held(&self) -> usize {
        let blobs: usize = self
            .blobs
            .iter()
            .map(|(digest, blob)| registry_entry_bytes(&[digest.len(), blob.content.len()]))
            .fold(0, usize::saturating_add);
        let manifests: usize = self
            .manifests
            .iter()
            .map(|(digest, manifest)| {
                registry_entry_bytes(&[
                    digest.len(),
                    manifest.content.len(),
                    manifest.repository.len(),
                    manifest.media_type.len(),
                ])
            })
            .fold(0, usize::saturating_add);
        let tags: usize = self
            .tags
            .iter()
            .map(|((repository, tag), digest)| {
                registry_entry_bytes(&[repository.len(), tag.len(), digest.len()])
            })
            .fold(0, usize::saturating_add);
        let uploads: usize = self
            .uploads
            .iter()
            .map(|(session, upload)| {
                registry_entry_bytes(&[
                    session.len(),
                    upload.repository.len(),
                    upload.content.len(),
                ])
            })
            .fold(0, usize::saturating_add);
        blobs
            .saturating_add(manifests)
            .saturating_add(tags)
            .saturating_add(uploads)
    }

    /// Whether this registry holds the blob `digest` names.
    pub fn has_blob(&self, digest: &str) -> bool {
        self.blobs.contains_key(digest)
    }

    /// The blob `digest` names.
    pub fn blob(&self, digest: &str) -> Option<&StoredRegistryBlob> {
        self.blobs.get(digest)
    }

    /// Whether this registry holds the image manifest `digest` names.
    pub fn has_manifest(&self, digest: &str) -> bool {
        self.manifests.contains_key(digest)
    }

    /// The digest the `reference` of `repository` resolves to: a reference that
    /// IS a digest names content directly, and any other reference is a tag,
    /// which names whatever manifest was last pushed under it.
    ///
    /// A DIGEST is resolved registry-wide, by design: the registry is the
    /// ACCOUNT's and content-addressed, so a digest names the same content
    /// whichever repository path addresses it (the property that lets a client
    /// mount content it has already pushed instead of sending it again). A TAG
    /// is the repository's own, so it resolves only under the repository it was
    /// pushed to.
    pub fn resolve_reference(&self, repository: &str, reference: &str) -> Option<String> {
        if self.manifests.contains_key(reference) {
            return Some(reference.to_string());
        }
        self.tags
            .get(&(repository.to_string(), reference.to_string()))
            .cloned()
    }

    /// The manifest the `reference` of `repository` resolves to.
    pub fn manifest(&self, repository: &str, reference: &str) -> Option<&StoredRegistryManifest> {
        let digest = self.resolve_reference(repository, reference)?;
        self.manifests.get(&digest)
    }

    /// The tags `repository` holds, in one stable order.
    pub fn tags_of(&self, repository: &str) -> Vec<String> {
        self.tags
            .keys()
            .filter(|(held, _)| held == repository)
            .map(|(_, tag)| tag.clone())
            .collect()
    }

    /// Whether anything was pushed to `repository` — a manifest of its own, or
    /// a tag naming one.
    pub fn holds_repository(&self, repository: &str) -> bool {
        self.manifests
            .values()
            .any(|manifest| manifest.repository == repository)
            || self.tags.keys().any(|(held, _)| held == repository)
    }

    /// Hold `content` as a blob, addressed by the digest of its own bytes.
    ///
    /// Content the registry already holds costs it nothing: the digest names
    /// those exact bytes, so a re-push of them is the blob that is already
    /// there.
    pub fn put_blob(&mut self, content: Vec<u8>, now_ms: i64) -> RegistryWrite {
        let digest = registry_digest(&content);
        if self.blobs.contains_key(&digest) {
            return RegistryWrite::Stored(digest);
        }
        // What the ENTRY costs, not only what was pushed: the same accounting
        // `bytes_held` reports, so a push is refused exactly when holding it
        // would take this region past its bound.
        if !self.can_hold(registry_entry_bytes(&[digest.len(), content.len()])) {
            return RegistryWrite::OutOfSpace {
                limit: MAX_REGISTRY_BYTES_PER_REGION,
            };
        }
        self.blobs.insert(
            digest.clone(),
            StoredRegistryBlob {
                content,
                pushed_at_ms: now_ms,
            },
        );
        RegistryWrite::Stored(digest)
    }

    /// Hold `content` as an image manifest of `repository`, addressed by the
    /// digest of its own bytes, and point `tag` at it when the push named one.
    ///
    /// What the write needs is judged for the manifest and the tag TOGETHER,
    /// before either is held: content the registry already holds and a tag it
    /// already has cost it nothing.
    pub fn put_manifest(
        &mut self,
        repository: &str,
        tag: Option<&str>,
        content: Vec<u8>,
        media_type: &str,
        now_ms: i64,
    ) -> RegistryWrite {
        let digest = registry_digest(&content);
        let fresh_manifest = !self.manifests.contains_key(&digest);
        let tag_key = tag.map(|tag| (repository.to_string(), tag.to_string()));
        let fresh_tag = tag_key
            .as_ref()
            .is_some_and(|key| !self.tags.contains_key(key));
        let needed = if fresh_manifest {
            registry_entry_bytes(&[
                digest.len(),
                content.len(),
                repository.len(),
                media_type.len(),
            ])
        } else {
            0
        }
        .saturating_add(
            tag_key
                .as_ref()
                .filter(|_| fresh_tag)
                .map_or(0, |(repository, tag)| {
                    registry_entry_bytes(&[repository.len(), tag.len(), digest.len()])
                }),
        );
        if !self.can_hold(needed) {
            return RegistryWrite::OutOfSpace {
                limit: MAX_REGISTRY_BYTES_PER_REGION,
            };
        }
        if fresh_manifest {
            self.manifests.insert(
                digest.clone(),
                StoredRegistryManifest {
                    content,
                    media_type: media_type.to_string(),
                    repository: repository.to_string(),
                    pushed_at_ms: now_ms,
                },
            );
        }
        if let Some(key) = tag_key {
            // A tag is a MOVING reference: pushing to one that is taken points
            // it at the manifest just pushed, which is what the protocol says a
            // tag is.
            self.tags.insert(key, digest.clone());
        }
        RegistryWrite::Stored(digest)
    }

    /// Record one blob upload started against `repository` under the session id
    /// `session`, dropping the oldest session once this region holds
    /// [`MAX_REGISTRY_UPLOADS_PER_REGION`].
    pub fn start_upload(&mut self, session: String, repository: &str, now_ms: i64) {
        self.uploads.insert(
            session,
            StoredBlobUpload {
                repository: repository.to_string(),
                content: Vec::new(),
                started_at_ms: now_ms,
            },
        );
        while self.uploads.len() > MAX_REGISTRY_UPLOADS_PER_REGION {
            let oldest = self
                .uploads
                .iter()
                .min_by(|left, right| {
                    left.1
                        .started_at_ms
                        .cmp(&right.1.started_at_ms)
                        .then_with(|| left.0.cmp(right.0))
                })
                .map(|(id, _)| id.clone());
            let Some(oldest) = oldest else { break };
            // A session dropped early stops accepting the bytes its client was
            // sending it, so the eviction is traceable rather than a silent
            // surprise. Only the session id is logged; no uploaded byte is.
            ::tracing::warn!(
                session = %oldest,
                limit = MAX_REGISTRY_UPLOADS_PER_REGION,
                "the oldest container image blob upload was dropped to keep this region's \
                 registry within the store's bound"
            );
            self.uploads.remove(&oldest);
        }
    }

    /// The upload session `session` of `repository` names.
    ///
    /// Scoped by the repository too: a session was started against ONE
    /// repository, so the same id under another repository's path names no
    /// session of it.
    pub fn upload(&self, session: &str, repository: &str) -> Option<&StoredBlobUpload> {
        self.uploads
            .get(session)
            .filter(|upload| upload.repository == repository)
    }

    /// Take `content` into the upload session `session` of `repository`.
    pub fn append_to_upload(
        &mut self,
        session: &str,
        repository: &str,
        content: &[u8],
    ) -> UploadAppend {
        if self.upload(session, repository).is_none() {
            return UploadAppend::Unknown;
        }
        let held_elsewhere = self
            .bytes_held()
            .saturating_sub(self.uploads.get(session).map_or(0, |u| u.content.len()));
        let Some(upload) = self.uploads.get_mut(session) else {
            return UploadAppend::Unknown;
        };
        let would_hold = upload.content.len().saturating_add(content.len());
        // Both bounds are judged BEFORE the bytes are taken, so a session that
        // cannot take them keeps exactly what it had.
        if would_hold > MAX_REGISTRY_BLOB_BYTES {
            return UploadAppend::OutOfSpace {
                limit: MAX_REGISTRY_BLOB_BYTES,
            };
        }
        if held_elsewhere.saturating_add(would_hold) > MAX_REGISTRY_BYTES_PER_REGION {
            return UploadAppend::OutOfSpace {
                limit: MAX_REGISTRY_BYTES_PER_REGION,
            };
        }
        upload.content.extend_from_slice(content);
        UploadAppend::Appended {
            held: upload.content.len(),
        }
    }

    /// Finish the upload session `session` of `repository` with the trailing
    /// `content` a client sent on the finishing request, holding the whole blob
    /// as one only when `claimed` is the digest of everything that session
    /// holds.
    ///
    /// The session SURVIVES every answer but a stored one — a claimed digest
    /// that is not the content's leaves the bytes where they were, so the
    /// client can carry on with that session rather than start again.
    pub fn finish_upload(
        &mut self,
        session: &str,
        repository: &str,
        content: &[u8],
        claimed: &str,
        now_ms: i64,
    ) -> UploadFinish {
        let Some(upload) = self.upload(session, repository) else {
            return UploadFinish::Unknown;
        };
        let would_hold = upload.content.len().saturating_add(content.len());
        let held_elsewhere = self.bytes_held().saturating_sub(upload.content.len());
        if would_hold > MAX_REGISTRY_BLOB_BYTES {
            return UploadFinish::OutOfSpace {
                limit: MAX_REGISTRY_BLOB_BYTES,
            };
        }
        if held_elsewhere.saturating_add(would_hold) > MAX_REGISTRY_BYTES_PER_REGION {
            return UploadFinish::OutOfSpace {
                limit: MAX_REGISTRY_BYTES_PER_REGION,
            };
        }
        // The digest of everything the blob would be, taken over the session's
        // own bytes and the trailing ones in order and WITHOUT joining them
        // first: the digest of the content is what decides, so it is computed
        // before anything is stored or dropped.
        let mut hasher = Sha256::new();
        hasher.update(&upload.content);
        hasher.update(content);
        let computed = format!(
            "{REGISTRY_DIGEST_ALGORITHM}:{}",
            hex::encode(hasher.finalize())
        );
        if computed != claimed {
            return UploadFinish::DigestMismatch { computed };
        }
        let Some(mut upload) = self.uploads.remove(session) else {
            return UploadFinish::Unknown;
        };
        upload.content.extend_from_slice(content);
        match self.put_blob(upload.content, now_ms) {
            RegistryWrite::Stored(digest) => UploadFinish::Stored(digest),
            RegistryWrite::OutOfSpace { limit } => UploadFinish::OutOfSpace { limit },
        }
    }

    /// Take the upload session `session` of `repository` out of this registry —
    /// what cancelling an upload does with it.
    pub fn take_upload(&mut self, session: &str, repository: &str) -> Option<StoredBlobUpload> {
        // The session has to be one of THIS repository's; `?` answers `None`
        // for any other, which is what a caller asking for one reads.
        self.upload(session, repository)?;
        self.uploads.remove(session)
    }

    /// Whether this registry can take `additional` more bytes and stay within
    /// [`MAX_REGISTRY_BYTES_PER_REGION`].
    pub(super) fn can_hold(&self, additional: usize) -> bool {
        self.bytes_held().saturating_add(additional) <= MAX_REGISTRY_BYTES_PER_REGION
    }
}

/// The name a container service assigns an image registered to it as the
/// `version`-th of `label`.
///
/// The form is the one the modelled `RegisterContainerImageRequest$label`
/// documentation gives: "if the name of your container service is
/// `mycontainerservice`, and the label that you specify is `mystaticwebsite`,
/// then the name of the registered container image will be
/// `:mycontainerservice.mystaticwebsite.1`".
pub fn container_image_name(service_name: &str, label: &str, version: i32) -> String {
    format!(":{service_name}.{label}.{version}")
}

/// One container image registered to a container service: the label it was
/// registered under, which version of that label it is, the digest that was
/// pushed, and when it was registered.
///
/// The assigned NAME is not stored beside them: it is
/// [`container_image_name`] of the holding service's name, the label and the
/// version, so one record cannot disagree with the name it is reported under.
#[derive(Debug, Clone)]
pub struct StoredContainerImage {
    /// The label the registering request named.
    pub label: String,
    /// Which version of that label this image is — 1 for the first image
    /// registered under it, and one more for each after, as the modelled
    /// `RegisterContainerImageRequest$label` documentation describes.
    pub version: i32,
    /// The digest of the pushed image, kept verbatim.
    pub digest: String,
    pub created_at_ms: i64,
}

impl StoredContainerImage {
    /// The name the container service `service_name` assigned this image.
    pub fn name(&self, service_name: &str) -> String {
        container_image_name(service_name, &self.label, self.version)
    }

    /// This record as the modelled `ContainerImage`, as the container service
    /// `service_name` reports it.
    pub fn view(&self, service_name: &str) -> ContainerImage {
        ContainerImage {
            image: Some(self.name(service_name)),
            digest: Some(self.digest.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
        }
    }
}

/// One stored deployment of a container service: the containers it launches, the
/// public endpoint that fronts them, and where it is in its own lifecycle.
#[derive(Debug, Clone)]
pub struct StoredDeployment {
    /// 1 for a service's first deployment, incremented per deployment after.
    pub version: i32,
    pub state: &'static str,
    /// Whether this deployment ever BECAME ACTIVE, i.e. whether the service
    /// launched the containers it declares.
    ///
    /// `INACTIVE` covers two deployments the service tells apart: one that ran
    /// and stepped down when a later deployment replaced it, and one that was
    /// replaced while it was still `ACTIVATING` and so never launched anything.
    /// This is the bit that says which, so the containers that "previously ran"
    /// are only the ones a deployment that did activate declares.
    pub activated: bool,
    /// A `BTreeMap` so the containers are stored in one stable order, whatever
    /// order the request's map arrived in.
    pub containers: BTreeMap<String, Container>,
    pub public_endpoint: Option<ContainerServiceEndpoint>,
    pub created_at_ms: i64,
}

impl StoredDeployment {
    /// This record as the modelled `ContainerServiceDeployment`.
    pub fn view(&self) -> ContainerServiceDeployment {
        ContainerServiceDeployment {
            version: Some(self.version),
            state: Some(self.state.to_string()),
            containers: Some(
                self.containers
                    .iter()
                    .map(|(name, container)| (name.clone(), container.clone()))
                    .collect::<HashMap<String, Container>>(),
            ),
            public_endpoint: self.public_endpoint.clone(),
            created_at: Some(timestamp(self.created_at_ms)),
        }
    }
}

/// A phase change the clock owes a container service: the state it settles into,
/// and whether reaching it makes the waiting deployment the active one.
#[derive(Debug, Clone, Copy)]
pub struct OwedPhase {
    /// Epoch milliseconds at which the change became (or becomes) due.
    pub at_ms: i64,
    pub state: &'static str,
    pub activates_next_deployment: bool,
}

/// One stored container service: what the caller asked for (name, power, scale,
/// tags, public domain names, private registry access and the deployment, if
/// any) plus what the service assigns (ARN, creation time, default domain,
/// principal ARNs and the lifecycle it is walking).
#[derive(Debug, Clone)]
pub struct StoredContainerService {
    pub name: String,
    pub arn: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    pub power: String,
    pub power_id: String,
    pub scale: i32,
    pub state: &'static str,
    pub is_disabled: bool,
    /// The per-service token in the service's public address, minted once so
    /// every read reports the same URL.
    pub domain_token: String,
    pub principal_arn: String,
    /// A `BTreeMap` so the certificate/domain pairs are stored in one stable
    /// order.
    pub public_domain_names: BTreeMap<String, Vec<String>>,
    pub ecr_image_puller_role_active: bool,
    /// Every deployment this service has been given, NEWEST VERSION FIRST.
    ///
    /// The ONE place a deployment of this service lives: the record that is
    /// `ACTIVE` is what `ContainerService$currentDeployment` reports and the one
    /// that is `ACTIVATING` is its `nextDeployment`, so a deployment is never
    /// held twice and the history, the current and the waiting deployment can
    /// never disagree. Bounded by [`MAX_DEPLOYMENTS_PER_SERVICE`].
    pub deployments: VecDeque<StoredDeployment>,
    /// The version the NEXT deployment of this service will carry.
    pub next_deployment_version: i32,
    /// The container images registered to this service, in registration order —
    /// the service's own registry, which `GetContainerImages` reports.
    ///
    /// A `Vec` and not a map: the modelled `ContainerImageList` documents no
    /// order, so registration order is the one stable total order the store has,
    /// and the label and version of each entry already key it uniquely.
    ///
    /// An entry gets here only through `RegisterContainerImage`, which takes
    /// the digest of an image that was PUSHED to the container image registry
    /// of the account (`StoredImageRegistry`) — so a registered image always
    /// names content this emulator holds.
    pub images: Vec<StoredContainerImage>,
    /// The phases the clock still owes this service, soonest first.
    pub owed: VecDeque<OwedPhase>,
}

impl StoredContainerService {
    /// Register the pushed image `digest` to this service under `label`, and
    /// answer the record that was stored.
    ///
    /// The version assigned is one past the highest this label's registered
    /// images carry, which is the rule the modelled
    /// `RegisterContainerImageRequest$label` documentation states: "If you push
    /// and register another container image to the same Lightsail container
    /// service, with the same label, then the version number for the new
    /// registered container image will be `2`. If you push and register another
    /// container image, the version number will be `3`, and so on." Two labels
    /// are counted independently, each starting at 1, and the version keeps the
    /// name of every image this registry holds distinct.
    ///
    /// The registry is bounded by [`MAX_CONTAINER_IMAGES_PER_SERVICE`]: past
    /// that, the oldest registration is dropped.
    pub fn register_image(
        &mut self,
        label: &str,
        digest: &str,
        now_ms: i64,
    ) -> StoredContainerImage {
        let version = self
            .images
            .iter()
            .filter(|image| image.label == label)
            .map(|image| image.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let registered = StoredContainerImage {
            label: label.to_string(),
            version,
            digest: digest.to_string(),
            created_at_ms: now_ms,
        };
        self.images.push(registered.clone());
        if let Some(excess) = self.images.len().checked_sub(MAX_CONTAINER_IMAGES_PER_SERVICE) {
            for evicted in self.images.drain(..excess) {
                // An image the store had to forget stops being reported before
                // anything deleted it, so the eviction is traceable rather than
                // a silent surprise.
                ::tracing::warn!(
                    service = %self.name,
                    image = %evicted.name(&self.name),
                    limit = MAX_CONTAINER_IMAGES_PER_SERVICE,
                    "the oldest registered container image was dropped to keep this container \
                     service within the store's bound"
                );
            }
        }
        registered
    }

    /// This record as the modelled `ContainerService`, announced to a caller that
    /// reached this emulator at `endpoint` (its `Host`/`:authority`, as
    /// [`request_endpoint`] reads it).
    ///
    /// `url` is the one member that depends on the request: it is the address the
    /// caller is told to reach this service's public endpoint on, so it has to be
    /// the endpoint THIS request arrived at (see
    /// [`CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`]). A request that names no host at
    /// all names no endpoint to announce, so `url` is omitted rather than filled
    /// with an address nothing serves.
    pub fn view(&self, endpoint: Option<&str>) -> ContainerService {
        ContainerService {
            container_service_name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_CONTAINER_SERVICE.to_string()),
            tags: Some(self.tags.clone()),
            power: Some(self.power.clone()),
            power_id: Some(self.power_id.clone()),
            state: Some(self.state.to_string()),
            state_detail: self.state_detail(),
            scale: Some(self.scale),
            current_deployment: self.current_deployment().map(StoredDeployment::view),
            next_deployment: self.next_deployment().map(StoredDeployment::view),
            is_disabled: Some(self.is_disabled),
            principal_arn: Some(self.principal_arn.clone()),
            private_domain_name: Some(container_service_private_domain_name(&self.name)),
            public_domain_names: Some(
                self.public_domain_names
                    .iter()
                    .map(|(certificate, domains)| (certificate.clone(), domains.clone()))
                    .collect::<HashMap<String, Vec<String>>>(),
            ),
            url: endpoint.map(|endpoint| {
                container_service_url(
                    endpoint,
                    &self.location.region_name,
                    &self.name,
                    &self.domain_token,
                )
            }),
            private_registry_access: Some(PrivateRegistryAccess {
                ecr_image_puller_role: Some(ContainerServiceECRImagePullerRole {
                    is_active: Some(self.ecr_image_puller_role_active),
                    // The role has a principal only while it is active: an
                    // inactive role grants nothing, so there is nothing for a
                    // repository policy to name.
                    principal_arn: self.ecr_image_puller_role_active.then(|| {
                        ecr_image_puller_principal_arn(
                            &self.location.region_name,
                            &self.name,
                            &self.domain_token,
                        )
                    }),
                }),
            }),
        }
    }

    /// The service's `stateDetail`, present only while a deployment is being
    /// created.
    ///
    /// The modelled `ContainerServiceStateDetail$code` documentation lists
    /// progress codes only for a `DEPLOYING` or `UPDATING` service, and reserves
    /// the two codes of a `PENDING` one for failures; nothing has failed here, so
    /// every other state reports no detail at all rather than an invented one.
    pub(super) fn state_detail(&self) -> Option<ContainerServiceStateDetail> {
        if self.state != CONTAINER_SERVICE_DEPLOYING {
            return None;
        }
        let version = self.next_deployment().map(|next| next.version);
        Some(ContainerServiceStateDetail {
            code: Some(STATE_DETAIL_CREATING_DEPLOYMENT.to_string()),
            message: Some(match version {
                Some(version) => format!("Creating deployment version {version}"),
                None => "Creating deployment".to_string(),
            }),
        })
    }

    /// The phase the clock owes this service at `at_ms` once the work in flight
    /// on it is applied: the state it settles into, and whether reaching it makes
    /// the deployment that is waiting the running one.
    ///
    /// The ONE rule every writer of [`Self::state`] settles by, from the modelled
    /// `ContainerService$state` documentation: a service the account disabled "is
    /// disabled, and its active deployment and containers, if any, are shut
    /// down", so being disabled outranks having a deployment and nothing is
    /// launched on it; an enabled service is `RUNNING` when it has a deployment
    /// to serve — the one running, or the one still waiting — and `READY` when it
    /// "is running but it does not have an active container deployment".
    ///
    /// A deployment record is left exactly as it stands: the model carries no way
    /// to report a deployment whose containers are shut down, and enabling the
    /// service brings the waiting one up, so nothing here retires one.
    pub fn settling_phase(&self, at_ms: i64) -> OwedPhase {
        let state = if self.is_disabled {
            CONTAINER_SERVICE_DISABLED
        } else if self.current_deployment().is_some() || self.next_deployment().is_some() {
            CONTAINER_SERVICE_RUNNING
        } else {
            CONTAINER_SERVICE_READY
        };
        OwedPhase {
            at_ms,
            state,
            activates_next_deployment: !self.is_disabled && self.next_deployment().is_some(),
        }
    }

    /// Materialise every phase the clock had already made due at `now_ms`.
    ///
    /// Bounded by the number of phases a create enqueues, and the service drops
    /// out of the work as soon as it has none outstanding.
    pub(super) fn advance(&mut self, now_ms: i64) {
        while let Some(phase) = self.owed.front().copied() {
            if phase.at_ms > now_ms {
                break;
            }
            self.owed.pop_front();
            self.state = phase.state;
            if !phase.activates_next_deployment {
                continue;
            }
            let Some(activating) = self
                .deployments
                .iter()
                .position(|deployment| deployment.state == DEPLOYMENT_STATE_ACTIVATING)
            else {
                continue;
            };
            // "The container service can have only one deployment in an active
            // state at a time": whatever was running steps down to the state of
            // a deployment that ran before, and stays in the history.
            for deployment in self.deployments.iter_mut() {
                if deployment.state == DEPLOYMENT_STATE_ACTIVE {
                    deployment.state = DEPLOYMENT_STATE_INACTIVE;
                }
            }
            self.deployments[activating].state = DEPLOYMENT_STATE_ACTIVE;
            // This deployment is launching its containers now, so they are the
            // ones that RAN on this service from here on — including after a
            // later deployment supersedes it, which is what distinguishes it
            // from a deployment that was replaced before it ever activated.
            self.deployments[activating].activated = true;
        }
    }

    /// The deployment currently running on this service, if any — the one in
    /// `ACTIVE`, of which the model admits at most one.
    pub fn current_deployment(&self) -> Option<&StoredDeployment> {
        self.deployments
            .iter()
            .find(|deployment| deployment.state == DEPLOYMENT_STATE_ACTIVE)
    }

    /// The deployment waiting to become the running one, if any — the one in
    /// `ACTIVATING`, of which a service has at most one (see
    /// [`Self::record_deployment`]).
    pub fn next_deployment(&self) -> Option<&StoredDeployment> {
        self.deployments
            .iter()
            .find(|deployment| deployment.state == DEPLOYMENT_STATE_ACTIVATING)
    }

    /// Add `deployment` to this service's history as the deployment WAITING to
    /// become the running one.
    ///
    /// A service launches one deployment at a time, so a deployment that was
    /// still `ACTIVATING` is superseded by this one: it was created, and it is
    /// not running, which is what `INACTIVE` reports. The history is kept
    /// newest-version-first and bounded by [`MAX_DEPLOYMENTS_PER_SERVICE`],
    /// past which the OLDEST deployment is the one dropped.
    pub fn record_deployment(&mut self, deployment: StoredDeployment) {
        for superseded in self.deployments.iter_mut() {
            if superseded.state == DEPLOYMENT_STATE_ACTIVATING {
                superseded.state = DEPLOYMENT_STATE_INACTIVE;
            }
        }
        self.deployments.push_front(deployment);
        while self.deployments.len() > MAX_DEPLOYMENTS_PER_SERVICE {
            if let Some(evicted) = self.deployments.pop_back() {
                // A deployment the store had to forget stops being reported
                // before anything replaced it, so the eviction is traceable
                // rather than a silent surprise.
                ::tracing::warn!(
                    service = %self.name,
                    version = evicted.version,
                    limit = MAX_DEPLOYMENTS_PER_SERVICE,
                    "the oldest deployment was dropped to keep this container service within \
                     the store's bound"
                );
            }
        }
    }

    /// This service's deployments as the modelled `ContainerServiceDeploymentList`.
    ///
    /// Newest version first: the operation's own documentation says "the newest
    /// version is listed at the top of the response".
    pub fn deployments_view(&self) -> Vec<ContainerServiceDeployment> {
        self.deployments
            .iter()
            .map(StoredDeployment::view)
            .collect()
    }

    /// Every deployment of this service that BECAME ACTIVE and the store still
    /// holds, newest version first: the one running now and the ones it
    /// superseded.
    ///
    /// A deployment still `ACTIVATING` is NOT among them, and neither is one
    /// that was replaced before it ever activated: neither launched a container.
    pub fn deployments_that_ran(&self) -> impl Iterator<Item = &StoredDeployment> {
        self.deployments
            .iter()
            .filter(|deployment| deployment.activated)
    }

    /// Whether `container` is one that is running, or previously ran, on this
    /// service — i.e. one that a deployment of it that became active declares.
    pub fn has_run_container(&self, container: &str) -> bool {
        self.deployments_that_ran()
            .any(|deployment| deployment.containers.contains_key(container))
    }

    /// The name of every container that is running, or previously ran, on this
    /// service, in one stable order and without repeats (a container of the same
    /// name in two deployments is one container of this service).
    ///
    /// Bounded by [`MAX_CONTAINERS_PER_DEPLOYMENT`] per deployment the store
    /// keeps, so the set is bounded however many deployments the service has had.
    pub fn containers_that_ran(&self) -> Vec<&str> {
        self.deployments_that_ran()
            .flat_map(|deployment| deployment.containers.keys().map(String::as_str))
            .collect::<BTreeSet<&str>>()
            .into_iter()
            .collect()
    }
}

/// The deployment `request` describes, at `version`, or a refusal naming the
/// member that is wrong.
///
/// A deployment launches containers, so it must name at least one, and each must
/// name the image it runs. A public endpoint must name one of those containers,
/// on a port that container opens, its health check is filled in with the
/// documented defaults for whatever the request left out, and every health-check
/// member the request did fill in is checked against the value domain that
/// member's own documentation states.
pub fn deployment_from_request(
    version: i32,
    now_ms: i64,
    request: &ContainerServiceDeploymentRequest,
) -> Result<StoredDeployment, String> {
    let containers: BTreeMap<String, Container> = request
        .containers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    if containers.is_empty() {
        return Err(
            "deployment.containers must describe at least one container to launch".to_string(),
        );
    }
    if containers.len() > MAX_CONTAINERS_PER_DEPLOYMENT {
        return Err(format!(
            "deployment.containers describes {} containers, more than the \
             {MAX_CONTAINERS_PER_DEPLOYMENT} this service accepts in one deployment",
            containers.len()
        ));
    }
    for (name, container) in &containers {
        if container
            .image
            .as_ref()
            .is_none_or(|image| image.trim().is_empty())
        {
            return Err(format!(
                "deployment.containers['{name}'].image must name the image the container runs"
            ));
        }
    }

    let public_endpoint = match &request.public_endpoint {
        None => None,
        Some(endpoint) => {
            let container = containers.get(&endpoint.container_name).ok_or_else(|| {
                format!(
                    "deployment.publicEndpoint.containerName '{}' names no container of this \
                     deployment, which describes [{}]",
                    endpoint.container_name,
                    containers.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            let open_ports = container.ports.clone().unwrap_or_default();
            if !open_ports.contains_key(&endpoint.container_port.to_string()) {
                let mut open: Vec<String> = open_ports.keys().cloned().collect();
                open.sort();
                return Err(format!(
                    "deployment.publicEndpoint.containerPort {} is not open on container '{}', \
                     which opens [{}]",
                    endpoint.container_port,
                    endpoint.container_name,
                    open.join(", ")
                ));
            }
            Some(ContainerServiceEndpoint {
                container_name: Some(endpoint.container_name.clone()),
                container_port: Some(endpoint.container_port),
                health_check: Some(health_check_from_request(endpoint.health_check.as_ref())?),
            })
        }
    };

    Ok(StoredDeployment {
        version,
        state: DEPLOYMENT_STATE_ACTIVATING,
        // A deployment the request just described has not launched anything
        // yet: the clock activates it.
        activated: false,
        containers,
        public_endpoint,
        created_at_ms: now_ms,
    })
}

/// The health check `request` asks for, with every member it left out filled in
/// from that member's documented default, or a refusal naming the member whose
/// value falls outside the domain that member's own documentation states.
///
/// Every member that documents a domain is checked against it: the two second
/// counts against their published ranges, and the success codes against the HTTP
/// code domain and list form their member documents (see
/// [`validate_success_codes`]). The thresholds and the path document a default
/// each and no domain at all, so any value the model admits for them is one the
/// service takes.
pub(super) fn health_check_from_request(
    request: Option<&ContainerServiceHealthCheckConfig>,
) -> Result<ContainerServiceHealthCheckConfig, String> {
    let timeout_seconds = request
        .and_then(|config| config.timeout_seconds)
        .unwrap_or(HEALTH_CHECK_DEFAULT_TIMEOUT_SECONDS);
    let (timeout_min, timeout_max) = HEALTH_CHECK_TIMEOUT_SECONDS_RANGE;
    if !(timeout_min..=timeout_max).contains(&timeout_seconds) {
        return Err(format!(
            "deployment.publicEndpoint.healthCheck.timeoutSeconds must be between \
             {timeout_min} and {timeout_max}, got {timeout_seconds}"
        ));
    }
    let interval_seconds = request
        .and_then(|config| config.interval_seconds)
        .unwrap_or(HEALTH_CHECK_DEFAULT_INTERVAL_SECONDS);
    let (interval_min, interval_max) = HEALTH_CHECK_INTERVAL_SECONDS_RANGE;
    if !(interval_min..=interval_max).contains(&interval_seconds) {
        return Err(format!(
            "deployment.publicEndpoint.healthCheck.intervalSeconds must be between \
             {interval_min} and {interval_max}, got {interval_seconds}"
        ));
    }
    let success_codes = request
        .and_then(|config| config.success_codes.clone())
        .unwrap_or_else(|| HEALTH_CHECK_DEFAULT_SUCCESS_CODES.to_string());
    validate_success_codes(&success_codes)?;
    Ok(ContainerServiceHealthCheckConfig {
        healthy_threshold: Some(
            request
                .and_then(|config| config.healthy_threshold)
                .unwrap_or(HEALTH_CHECK_DEFAULT_THRESHOLD),
        ),
        unhealthy_threshold: Some(
            request
                .and_then(|config| config.unhealthy_threshold)
                .unwrap_or(HEALTH_CHECK_DEFAULT_THRESHOLD),
        ),
        timeout_seconds: Some(timeout_seconds),
        interval_seconds: Some(interval_seconds),
        path: Some(
            request
                .and_then(|config| config.path.clone())
                .unwrap_or_else(|| HEALTH_CHECK_DEFAULT_PATH.to_string()),
        ),
        success_codes: Some(success_codes),
    })
}

/// `codes` read as the documented value domain of a health check's
/// `successCodes`, or a refusal naming what falls outside it.
///
/// The member documents the domain whole: the HTTP codes a container's response
/// is checked against are "values between `200` and `499`", given as "multiple
/// values (for example, `200,202`) or a range of values (for example,
/// `200-299`)". So a value names at least one code; each value it lists is one
/// code or a pair of codes running from the lower one to the higher one, written
/// the way the documented examples write them (decimal digits and the two
/// separators, nothing else); and every code named lies within the documented
/// bounds.
pub(super) fn validate_success_codes(codes: &str) -> Result<(), String> {
    let (min, max) = HEALTH_CHECK_SUCCESS_CODES_RANGE;
    let malformed = || {
        format!(
            "deployment.publicEndpoint.healthCheck.successCodes must be HTTP codes between \
             {min} and {max}, given as multiple values (for example, \
             200{SUCCESS_CODES_VALUE_SEPARATOR}202) or a range of values (for example, \
             200{SUCCESS_CODES_RANGE_SEPARATOR}299), got '{codes}'"
        )
    };
    let code = |text: &str| -> Result<i32, String> {
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(malformed());
        }
        let value: i32 = text.parse().map_err(|_| malformed())?;
        if !(min..=max).contains(&value) {
            return Err(format!(
                "deployment.publicEndpoint.healthCheck.successCodes must be HTTP codes between \
                 {min} and {max}, got {value}"
            ));
        }
        Ok(value)
    };
    for value in codes.split(SUCCESS_CODES_VALUE_SEPARATOR) {
        // A value carrying the range separator is the documented range form, so
        // both of its bounds are codes; anything else is one code on its own.
        match value.split_once(SUCCESS_CODES_RANGE_SEPARATOR) {
            None => {
                code(value)?;
            }
            Some((low, high)) => {
                let (low, high) = (code(low)?, code(high)?);
                if low > high {
                    return Err(format!(
                        "deployment.publicEndpoint.healthCheck.successCodes range \
                         '{low}{SUCCESS_CODES_RANGE_SEPARATOR}{high}' must run from the lower \
                         HTTP code to the higher one"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The public domain names `requested` carries, or a refusal when it carries more
/// than [`MAX_PUBLIC_DOMAIN_NAMES`] of them or one of them is not a domain name.
///
/// This is the ONE owner of the half of the rule the REQUEST decides on its own,
/// whichever operation records those names: how many names the map may carry, and
/// that every name listed under a key is a domain name — the member is "the public
/// domain names to use with the container service, such as example.com and
/// www.example.com" — asked of [`validate_domain_name`], this crate's one checker
/// of that (which is also what refuses the wildcard forms the certificate
/// documentation says are not supported, because `*` is not a character a DNS
/// label carries). Whether the certificate each group of names is keyed by is one
/// the account holds and the certificate authority has issued is decided by the
/// store — see
/// [`ComAmazonawsLightsailLightsail20161128State::unusable_public_domain_certificate`].
///
/// How many names the map carries is bounded BEFORE a single one is examined, so
/// the per-name walk below is bounded by [`MAX_PUBLIC_DOMAIN_NAMES`] rather than
/// by what the caller sent.
pub fn public_domain_names_from_request(
    requested: Option<&HashMap<String, Vec<String>>>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let names: BTreeMap<String, Vec<String>> = requested
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(certificate, mut domains)| {
            domains.sort();
            domains.dedup();
            (certificate, domains)
        })
        .collect();
    let total: usize = names.values().map(Vec::len).sum();
    if total > MAX_PUBLIC_DOMAIN_NAMES {
        return Err(format!(
            "publicDomainNames carries {total} domain names, more than the \
             {MAX_PUBLIC_DOMAIN_NAMES} a container service accepts"
        ));
    }
    for domain in names.values().flatten() {
        validate_domain_name(domain).map_err(|detail| {
            format!(
                "publicDomainNames entry '{domain}' is not a public domain name of a \
                 container service: {detail}"
            )
        })?;
    }
    Ok(names)
}
