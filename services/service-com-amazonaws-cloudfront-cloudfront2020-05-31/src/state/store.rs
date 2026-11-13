// SPDX-License-Identifier: CC0-1.0

//! The STORE's own API: the inherent methods over
//! [`ComAmazonawsCloudfrontCloudfront20200531State`].
//!
//! The struct and the maps it holds live in the parent module; everything a handler does
//! THROUGH the store — settling the clock, reading a record, allocating an identifier,
//! inserting, updating and releasing one — lives here, so that surface can grow without
//! touching the records themselves. Nothing is re-exported: every name here is reached as a
//! method on the store the handlers lock.

use super::*;

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// Apply every transition the clock owes at `now`, across the whole store.
    pub fn settle(&mut self, now: SystemTime) {
        for distribution in self.distributions.values_mut() {
            distribution.settle(now);
        }
        for key_value_store in self.key_value_stores.values_mut() {
            key_value_store.settle(now);
        }
        for anycast_ip_list in self.anycast_ip_lists.values_mut() {
            anycast_ip_list.settle(now);
        }
        for tenant in self.distribution_tenants.values_mut() {
            tenant.settle(now);
        }
        for connection_group in self.connection_groups.values_mut() {
            connection_group.settle(now);
        }
        for streaming_distribution in self.streaming_distributions.values_mut() {
            streaming_distribution.settle(now);
        }
        for trust_store in self.trust_stores.values_mut() {
            trust_store.settle(now);
        }
        for vpc_origin in self.vpc_origins.values_mut() {
            vpc_origin.settle(now);
        }
        for anycast_ip_list in self.anycast_ip_lists.values_mut() {
            anycast_ip_list.settle(now);
        }
    }

    /// The key value store `name` names, or `None` when the store holds none.
    pub fn key_value_store(&self, name: &str) -> Option<&StoredKeyValueStore> {
        self.key_value_stores.get(name)
    }

    /// The resource policy attached to the resource `arn` names, or `None` when that
    /// resource carries none.
    pub fn resource_policy(&self, arn: &str) -> Option<&str> {
        self.resource_policies.get(arn).map(String::as_str)
    }

    /// Attach `document` to the resource `arn` names as its resource policy, replacing
    /// whatever policy that resource carried before.
    ///
    /// One policy per resource: the surface's write is a PUT, so a second write under the
    /// same ARN supersedes the first rather than adding a second policy beside it.
    pub fn put_resource_policy(&mut self, arn: &str, document: &str) {
        self.resource_policies
            .insert(arn.to_string(), document.to_string());
    }

    /// Remove the resource policy attached to the resource `arn` names, answering whether
    /// that resource carried one.
    ///
    /// `false` is the whole "there was nothing to remove" answer, which is what lets the
    /// delete tell an accepted ARN carrying no policy apart from one it removed a policy
    /// from; nothing else in the store is touched either way, since a policy is keyed by the
    /// ARN it was attached to alone.
    pub fn delete_resource_policy(&mut self, arn: &str) -> bool {
        self.resource_policies.remove(arn).is_some()
    }

    /// The origin access identity `id` names, or `None` when the store holds none.
    pub fn origin_access_identity(&self, id: &str) -> Option<&StoredOriginAccessIdentity> {
        self.origin_access_identities.get(id)
    }

    /// The origin access identity created under `caller_reference`, or `None` when no
    /// identity was.
    ///
    /// A scan over the identities rather than a second index beside them: the stored
    /// records are the single source of truth for which references are taken, so a scan
    /// cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_ORIGIN_ACCESS_IDENTITIES`].
    pub fn origin_access_identity_by_caller_reference(
        &self,
        caller_reference: &str,
    ) -> Option<&StoredOriginAccessIdentity> {
        self.origin_access_identities
            .values()
            .find(|identity| identity.config.caller_reference == caller_reference)
    }

    /// The id of the distribution that attaches the origin access identity `id` to one of
    /// its Amazon S3 origins, or `None` when nothing stored names it.
    ///
    /// `CloudFrontOriginAccessIdentityInUse` is documented as "The Origin Access Identity
    /// specified is already in use", and a stored configuration names an identity in
    /// exactly two places: `S3OriginConfig$OriginAccessIdentity` on an origin of a
    /// distribution, and `S3Origin$OriginAccessIdentity` on a streaming distribution. Both
    /// carry the reference form [`origin_access_identity_reference`] builds, and both are
    /// documented EMPTY when the configuration names no identity — so what makes an
    /// identity in use is a stored S3 origin of either family naming it, and the answer is
    /// the distribution that does.
    ///
    /// The scan reads the STORED configurations rather than a second index beside them, for
    /// the same reason
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_origin_access_control`]
    /// scans: it cannot drift out of step with what a read of those distributions reports,
    /// and both collections are bounded ([`MAX_DISTRIBUTIONS`] and
    /// [`MAX_STREAMING_DISTRIBUTIONS`], each record with its own bounded origin list).
    ///
    /// The holder's ID rather than its record, because the two families are stored as
    /// different types and only the id is common to both.
    pub fn distribution_using_origin_access_identity(&self, id: &str) -> Option<&str> {
        if id.is_empty() {
            return None;
        }
        let reference = origin_access_identity_reference(id);
        self.distributions
            .values()
            .find(|distribution| {
                distribution.config.origins.items.iter().any(|origin| {
                    origin
                        .s3_origin_config
                        .as_ref()
                        .is_some_and(|s3| s3.origin_access_identity.trim() == reference)
                })
            })
            .map(|distribution| distribution.id.as_str())
            .or_else(|| {
                self.streaming_distributions
                    .values()
                    .find(|distribution| {
                        distribution.config.s3_origin.origin_access_identity.trim() == reference
                    })
                    .map(|distribution| distribution.id.as_str())
            })
    }

    /// The streaming distribution `id` names, or `None` when the store holds none.
    pub fn streaming_distribution(&self, id: &str) -> Option<&StoredStreamingDistribution> {
        self.streaming_distributions.get(id)
    }

    /// The streaming distribution created under `caller_reference`, or `None` when no
    /// streaming distribution was.
    ///
    /// A scan over the stored records rather than a second index beside them, for the
    /// reason [`ComAmazonawsCloudfrontCloudfront20200531State::origin_access_identity_by_caller_reference`]
    /// is one: the records are the single source of truth for which references are taken,
    /// so a scan cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_STREAMING_DISTRIBUTIONS`].
    pub fn streaming_distribution_by_caller_reference(
        &self,
        caller_reference: &str,
    ) -> Option<&StoredStreamingDistribution> {
        self.streaming_distributions
            .values()
            .find(|distribution| distribution.config.caller_reference == caller_reference)
    }

    /// The distribution `id` names, or `None` when the store holds none.
    pub fn distribution(&self, id: &str) -> Option<&StoredDistribution> {
        self.distributions.get(id)
    }

    /// The distribution whose edge domain name is `domain_name`, or `None` when the store
    /// holds none.
    ///
    /// A domain name is allocated to one distribution at creation and is stable for its
    /// whole life, so it addresses at most one record. This is how a resource that names a
    /// distribution by its DOMAIN name rather than by its id — a continuous deployment
    /// policy's `StagingDistributionDnsNames` — resolves to the distribution it routes
    /// traffic to. DNS names are case-insensitive, so the comparison is ([`same_domain`]).
    pub fn distribution_by_domain_name(&self, domain_name: &str) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| same_domain(&distribution.domain_name, domain_name))
    }

    /// The continuous deployment policy `id` names, or `None` when the store holds none.
    pub fn continuous_deployment_policy(
        &self,
        id: &str,
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        self.continuous_deployment_policies.get(id)
    }

    /// The distribution that attaches the continuous deployment policy `id`, or `None` when
    /// no stored distribution does.
    ///
    /// `ContinuousDeploymentPolicyInUse` is documented as "You cannot delete a continuous
    /// deployment policy that is associated with a primary distribution", and a distribution
    /// attaches one through its `ContinuousDeploymentPolicyId` — so what makes a policy in
    /// use is a stored distribution naming it. The SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for the policies a cache behaviour attaches: the scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those distributions reports, and the collection is bounded
    /// ([`MAX_DISTRIBUTIONS`]). A distribution that attaches no policy carries the empty
    /// string rather than an id, so an id matches only a distribution that really names it.
    pub fn distribution_using_continuous_deployment_policy(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        self.distributions.values().find(|distribution| {
            distribution
                .config
                .continuous_deployment_policy_id
                .as_deref()
                .is_some_and(|attached| attached == id)
        })
    }

    /// The policy that already routes traffic to one of `dns_names`, or `None` when no
    /// stored policy names any of them.
    ///
    /// `StagingDistributionInUse` is documented as "A continuous deployment policy for
    /// this staging distribution already exists", so a staging distribution takes part in
    /// at most ONE policy and this is what decides whether a create may claim it.
    ///
    /// The request's names are indexed into a set ONCE, so the scan costs one set lookup
    /// per stored name rather than a walk of the request's list per stored name: the work
    /// is linear in the two lengths, each of which is bounded
    /// ([`MAX_STAGING_DISTRIBUTION_DNS_NAMES`] per policy,
    /// [`MAX_CONTINUOUS_DEPLOYMENT_POLICIES`] policies).
    pub fn policy_using_staging_distribution(
        &self,
        dns_names: &[String],
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        let claimed: BTreeSet<&str> = dns_names.iter().map(String::as_str).collect();
        self.continuous_deployment_policies.values().find(|policy| {
            policy
                .staging_distribution_dns_names()
                .iter()
                .any(|held| claimed.contains(held.as_str()))
        })
    }

    /// The policy whose stored configuration is `config`, or `None` when none is.
    ///
    /// `ContinuousDeploymentPolicyAlreadyExists` is documented as "A continuous deployment
    /// policy with this configuration already exists", so this is what separates a create
    /// that repeats a policy already in the store from one that claims a staging
    /// distribution another policy holds.
    pub fn policy_with_config(
        &self,
        config: &ContinuousDeploymentPolicyConfig,
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        self.continuous_deployment_policies
            .values()
            .find(|policy| same_policy_config(&policy.config, config))
    }

    /// The distribution tenant `identifier` names, or `None` when the store holds none.
    ///
    /// A distribution tenant operation names its tenant by "the ARN, ID, or name of the
    /// distribution tenant", so all three resolve to the one record: the id is the store's
    /// key, and the ARN and the name are compared against the STORED values rather than
    /// re-derived out of the request. The three namespaces cannot overlap — see
    /// [`DISTRIBUTION_TENANT_ID_PREFIX`] — so at most one of them matches.
    pub fn distribution_tenant(&self, identifier: &str) -> Option<&StoredDistributionTenant> {
        if let Some(tenant) = self.distribution_tenants.get(identifier) {
            return Some(tenant);
        }
        self.distribution_tenants
            .values()
            .find(|tenant| tenant.arn == identifier || tenant.name == identifier)
    }

    /// The distribution tenant `domain` routes to, or `None` when no tenant this store
    /// holds serves it.
    ///
    /// "A domain name associated with the target distribution tenant": the association is
    /// the tenant's own `Domains`, and a domain routes to at most one CloudFront resource
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::domain_in_use`]), so at most one
    /// tenant answers. DNS names are case-insensitive, so the lookup is too
    /// ([`StoredDistributionTenant::serves_domain`]).
    ///
    /// This is the HOST-to-tenant map a viewer's request is resolved through, so it holds a
    /// domain only once CloudFront can serve it: a domain still waiting for the tenant's
    /// CloudFront managed certificate to be validated is not in it yet
    /// ([`StoredDistributionTenant::domain_pending_validation`]), which is why a by-domain
    /// read of it answers "no such entity" while the certificate is pending. It is also the
    /// index a second claim on the domain is refused against
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::domain_in_use`]): a claim is
    /// enforced from the moment it can be served, not from the moment it is written down.
    ///
    /// Several tenants may therefore hold the same domain while none of their claims is
    /// served yet, and the validation that resolves such a race decides ONE of them: the
    /// EARLIEST claim validated for a domain is the one it routes to, so the map answers the
    /// first-created serving tenant and a later duplicate never takes a domain away from
    /// it. The id breaks a tie, so the answer is a property of the store rather than of the
    /// walk.
    pub fn distribution_tenant_by_domain(&self, domain: &str) -> Option<&StoredDistributionTenant> {
        self.distribution_tenants
            .values()
            .filter(|tenant| tenant.serves_domain(domain))
            .min_by(|left, right| {
                (left.created_time, left.id.as_str()).cmp(&(right.created_time, right.id.as_str()))
            })
    }

    /// Whether `domain` is already SERVED by a CloudFront resource this store holds: a
    /// domain of a distribution tenant other than `except_tenant_id` that the tenant can
    /// already be reached at, or an alias of a distribution.
    ///
    /// "The CNAME specified is already defined for CloudFront": one domain name routes to
    /// at most one CloudFront resource, so the check spans every resource kind that can
    /// claim one. `except_tenant_id` is the tenant a mutation is rewriting, whose own
    /// domains are no conflict with itself.
    ///
    /// A claim counts only once the certificate it is served over has been VALIDATED, which
    /// is exactly when the domain enters the host-to-tenant map
    /// ([`StoredDistributionTenant::serves_domain`]): until then nothing resolves the domain
    /// to the resource, so there is no route for a second claim to collide with and the
    /// claim is not enforced. A tenant whose CloudFront managed certificate is still
    /// pending validation therefore does not take a domain, while a tenant whose
    /// certificate has been issued — and a distribution's alias, which cannot be declared
    /// without a custom viewer certificate that covers it
    /// ([`alias_certificate_mismatch`]) — does.
    pub fn domain_in_use(&self, domain: &str, except_tenant_id: Option<&str>) -> bool {
        let served_by_tenant = self.distribution_tenants.values().any(|tenant| {
            Some(tenant.id.as_str()) != except_tenant_id && tenant.serves_domain(domain)
        });
        served_by_tenant
            || self.distributions.values().any(|distribution| {
                distribution
                    .config
                    .aliases
                    .as_ref()
                    .and_then(|aliases| aliases.items.as_deref())
                    .unwrap_or_default()
                    .iter()
                    .any(|alias| same_domain(alias, domain))
            })
    }

    /// Every domain association this store holds that conflicts with the domain
    /// `requested`, as the `DomainConflict` each is reported as.
    ///
    /// A conflict is an association of a name that cannot be served beside `requested`:
    /// the same name, or a wildcard alternate domain name that covers it — or that it
    /// covers ([`domains_conflict`]). The associations CloudFront's domain surface
    /// addresses are a standard distribution's alternate domain names and a distribution
    /// tenant's domains, which is also the pair `DistributionResourceType` admits, so the
    /// alias of a legacy RTMP distribution is none of them: there is no resource type to
    /// report one under.
    ///
    /// One association is reported ONCE — the conflicts are the SET of (domain, resource)
    /// associations — and they are ordered by the conflicting domain and then by the id of
    /// the resource holding it, so the order is a property of the STORE rather than of the
    /// walk and a resume point into it names the same place on every call.
    pub fn domain_conflicts(&self, requested: &str) -> Vec<DomainConflict> {
        let account_id = ::engine::emulator_account_id();
        let mut conflicts: Vec<DomainConflict> = Vec::new();
        let mut push = |domain: &str, resource_type: &str, resource_id: &str| {
            conflicts.push(DomainConflict {
                domain: domain.to_string(),
                resource_type: resource_type.to_string(),
                resource_id: resource_id.to_string(),
                account_id: account_id.to_string(),
            });
        };
        for distribution in self.distributions.values() {
            let aliases = distribution
                .config
                .aliases
                .as_ref()
                .and_then(|aliases| aliases.items.as_deref())
                .unwrap_or_default();
            for alias in aliases {
                if domains_conflict(alias, requested) {
                    push(
                        alias,
                        DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION,
                        &distribution.id,
                    );
                }
            }
        }
        for tenant in self.distribution_tenants.values() {
            for domain in &tenant.domains {
                if domains_conflict(domain, requested) {
                    push(domain, DISTRIBUTION_RESOURCE_TYPE_TENANT, &tenant.id);
                }
            }
        }
        conflicts.sort_by_key(conflict_order);
        conflicts.dedup_by_key(|conflict| conflict_order(conflict));
        conflicts
    }

    /// The CloudFront endpoint the DNS name `name` resolves to, or `None` when this
    /// service holds no record that resolves it.
    ///
    /// CloudFront answers for the names it MINTS — the `d<label>.cloudfront.net` endpoints
    /// it allocates: a connection group's routing endpoint ("also known as the DNS name
    /// ... such as `d111111abcdef8.cloudfront.net`") and a distribution's own
    /// `DomainName`. Each of those names resolves to itself, and that is the whole of what
    /// this emulator can resolve: a name in a CUSTOMER's zone (the domain of a
    /// distribution tenant, an alias of a distribution) is published by the customer's own
    /// DNS, which this emulator neither runs nor reaches — so it holds no record for one,
    /// exactly as [`DOMAIN_STATUS_ACTIVE`] says of the association it does hold.
    ///
    /// [`MULTI_TENANT_DOMAIN_NAME`] is excluded: it is the fixed placeholder a
    /// multi-tenant distribution reports in place of an edge domain, not a name that
    /// resolves anywhere.
    pub fn resolved_cloudfront_endpoint(&self, name: &str) -> Option<String> {
        if let Some(group) = self.connection_group_by_routing_endpoint(name) {
            return Some(group.routing_endpoint.clone());
        }
        if let Some(distribution) = self.distributions.values().find(|distribution| {
            let reported = distribution.reported_domain_name();
            reported != MULTI_TENANT_DOMAIN_NAME && same_domain(&reported, name)
        }) {
            return Some(distribution.reported_domain_name());
        }
        self.streaming_distributions
            .values()
            .find(|distribution| same_domain(&distribution.domain_name, name))
            .map(|distribution| distribution.domain_name.clone())
    }

    /// The first domain a CloudFront managed ACM certificate `request` would cover whose
    /// ownership this service cannot verify, or `None` when the request asks for no
    /// verification or every covered domain is verified.
    ///
    /// `ManagedCertificateRequest$ValidationTokenHost` decides WHO serves the HTTP
    /// validation token: [`VALIDATION_TOKEN_HOST_CLOUDFRONT`] asks CloudFront to serve it at
    /// the domain itself, which it can only do once that domain resolves to a CloudFront
    /// resource — so the request is admitted only for a domain this service resolves to one,
    /// and the other mode ("you serve the validation token from your existing
    /// infrastructure") leaves the validation to the caller and demands nothing of DNS.
    ///
    /// The DNS view is the one and only one this service has
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::resolved_cloudfront_endpoint`]), so
    /// the names a certificate request is admitted for are exactly the names a verification
    /// of the DNS configuration can read a record for. The covered domains are the ones the
    /// resource claims plus the `PrimaryDomainName` the request names, if any: CloudFront
    /// has to serve a token at every one of them.
    pub fn unverifiable_certificate_domain(
        &self,
        request: &ManagedCertificateRequest,
        domains: &[String],
    ) -> Option<String> {
        if request.validation_token_host != VALIDATION_TOKEN_HOST_CLOUDFRONT {
            return None;
        }
        self.unproven_domain(
            domains.iter().map(String::as_str).chain(
                request
                    .primary_domain_name
                    .as_deref()
                    .filter(|domain| !domain.is_empty()),
            ),
        )
        .map(str::to_string)
    }

    /// Whether `id` names a connection group this store holds — an id no connection group
    /// carries names no entity.
    pub fn holds_connection_group(&self, id: &str) -> bool {
        self.connection_groups.contains_key(id)
    }

    /// Whether any distribution tenant this store holds is routed through the connection
    /// group `id`.
    ///
    /// A connection group is "what a distribution tenant's viewers are routed through", and
    /// the attachment lives on the tenant as its `ConnectionGroupId`, so the question is
    /// answered against the STORED tenants rather than against any request — the way
    /// [`Self::key_value_store_is_associated`] answers the same question for the other
    /// attachment this store holds. It is what decides whether a group may be deleted.
    pub fn connection_group_is_associated(&self, id: &str) -> bool {
        self.distribution_tenants
            .values()
            .any(|tenant| tenant.connection_group_id == id)
    }

    /// The first of `domains` this service has no proof the account controls, or `None`
    /// when every one of them is proven.
    ///
    /// A domain is where a distribution tenant's viewers reach it, so CloudFront binds one
    /// only to a domain the account's control of which it can see established. The one
    /// proof this service can read is its own DNS view
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::resolved_cloudfront_endpoint`]):
    /// a domain that already resolves to a CloudFront resource in this account is a domain
    /// whose DNS the account steers. A name in a customer's zone is published by DNS this
    /// emulator neither runs nor reaches, so nothing here proves it — and an ACM
    /// certificate the caller names by ARN proves nothing either, since a certificate
    /// imported into ACM carries no domain validation at all. The remaining way to bind
    /// such a domain is to ask CloudFront to validate it, which is what a
    /// `ManagedCertificateRequest` on the call does.
    ///
    /// ONE rule for every call that binds a domain to a tenant, and the ONE place the DNS
    /// view is consulted per domain — the certificate-request check above answers from it
    /// too — so an admission and a later association cannot drift apart.
    pub fn unproven_domain<'a, I>(&self, domains: I) -> Option<&'a str>
    where
        I: IntoIterator<Item = &'a str>,
    {
        domains
            .into_iter()
            .find(|domain| self.resolved_cloudfront_endpoint(domain).is_none())
    }

    /// Whether `id` names an Anycast static IP list this store holds.
    ///
    /// Every list in the store was put there by `CreateAnycastIpList`, so an id no stored
    /// list carries is no argument this service can resolve — which is what the
    /// `InvalidArgument` the connection group create and listing answer for one says
    /// ([`invalid_anycast_ip_list`]). The ONE place that resolves an `AnycastIpListId`, so
    /// every operation that takes one agrees on which ids exist.
    pub fn holds_anycast_ip_list(&self, id: &str) -> bool {
        self.anycast_ip_lists.contains_key(id)
    }

    /// The Anycast static IP list `id` names, or `None` when the store holds none.
    pub fn anycast_ip_list(&self, id: &str) -> Option<&StoredAnycastIpList> {
        self.anycast_ip_lists.get(id)
    }

    /// The Anycast static IP list whose STORED name is `name`, or `None` when none is.
    ///
    /// A scan over the stored records rather than a second index beside them, for the
    /// reason [`ComAmazonawsCloudfrontCloudfront20200531State::origin_access_identity_by_caller_reference`]
    /// is one: the records are the single source of truth for which names are taken, and
    /// the collection is bounded by the address pool every list is allocated from
    /// ([`ANYCAST_IP_POOL`]).
    pub fn anycast_ip_list_named(&self, name: &str) -> Option<&StoredAnycastIpList> {
        self.anycast_ip_lists
            .values()
            .find(|list| list.name == name)
    }

    /// The connection group `identifier` names, or `None` when the store holds none.
    ///
    /// `GetConnectionGroup$Identifier` is "The ID, name, or Amazon Resource Name (ARN) of
    /// the connection group", so all three resolve to the one record: the id is the
    /// store's key, and the ARN and the name are compared against the STORED values
    /// rather than re-derived out of the request. The three namespaces cannot overlap —
    /// an id carries [`CONNECTION_GROUP_ID_PREFIX`] and an ARN starts with `arn:` — so at
    /// most one of them matches.
    pub fn connection_group(&self, identifier: &str) -> Option<&StoredConnectionGroup> {
        if let Some(group) = self.connection_groups.get(identifier) {
            return Some(group);
        }
        self.connection_groups
            .values()
            .find(|group| group.arn == identifier || group.name == identifier)
    }

    /// The connection group whose STORED name is `name`, or `None` when none is.
    ///
    /// `CreateConnectionGroup$Name` is "a friendly identifier that is unique within your
    /// Amazon Web Services account", so at most one group answers, and this is what makes
    /// a repeated name a conflict rather than a second group.
    pub fn connection_group_named(&self, name: &str) -> Option<&StoredConnectionGroup> {
        self.connection_groups
            .values()
            .find(|group| group.name == name)
    }

    /// The connection group whose STORED routing endpoint is `endpoint`, or `None` when
    /// none is.
    ///
    /// `ConnectionGroup$RoutingEndpoint` is "the routing endpoint (also known as the DNS
    /// name) that is assigned to the connection group", and this service assigns a fresh
    /// one to each group, so at most one group answers. The comparison is the one every
    /// DNS name in this store is compared by ([`same_domain`]): a name is the same name in
    /// any case.
    pub fn connection_group_by_routing_endpoint(
        &self,
        endpoint: &str,
    ) -> Option<&StoredConnectionGroup> {
        self.connection_groups
            .values()
            .find(|group| same_domain(&group.routing_endpoint, endpoint))
    }

    /// The response headers policy `id` names, or `None` when the store holds none.
    ///
    /// The ACCOUNT's own policies only — what a mutation addresses, since nothing in this
    /// account changes a policy AWS published. A READ addresses both, through
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::readable_response_headers_policy`].
    pub fn response_headers_policy(&self, id: &str) -> Option<&StoredResponseHeadersPolicy> {
        self.response_headers_policies.get(id)
    }

    /// The response headers policy `id` names wherever it came from: one this account
    /// created with `CreateResponseHeadersPolicy`, or one AWS published
    /// ([`MANAGED_RESPONSE_HEADERS_POLICIES`]).
    ///
    /// The same rule [`Self::readable_cache_policy`] states for the cache policy family.
    pub fn readable_response_headers_policy(
        &self,
        id: &str,
    ) -> Option<Cow<'_, StoredResponseHeadersPolicy>> {
        match self.response_headers_policies.get(id) {
            Some(stored) => Some(Cow::Borrowed(stored)),
            None => {
                managed_response_headers_policy(id).map(|managed| Cow::Owned(managed.to_stored()))
            }
        }
    }

    /// The response headers policy whose STORED configuration carries `name`, or `None`.
    ///
    /// `ResponseHeadersPolicyConfig$Name`: "The name must be unique for response headers
    /// policies in this Amazon Web Services account" — so at most one policy answers,
    /// and this is what makes a repeated name a conflict rather than a second policy.
    /// The comparison is against the stored configuration, never against a request.
    pub fn response_headers_policy_named(
        &self,
        name: &str,
    ) -> Option<&StoredResponseHeadersPolicy> {
        self.response_headers_policies
            .values()
            .find(|policy| policy.config.name == name)
    }

    /// The distribution that attaches the response headers policy `id` to one of its cache
    /// behaviours, or `None` when no stored distribution does.
    ///
    /// `ResponseHeadersPolicyInUse` is documented as "Cannot delete the response headers
    /// policy because it is attached to one or more cache behaviors in a CloudFront
    /// distribution", and a behaviour attaches a policy through its
    /// `ResponseHeadersPolicyId` — so what makes a policy in use is a stored behaviour
    /// naming it, which is the rule [`references_response_headers_policy`] states for the
    /// whole configuration. The SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for the other policies a cache behaviour attaches, over the same bounded
    /// collections.
    pub fn distribution_using_response_headers_policy(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_response_headers_policy(&distribution.config, id))
    }

    /// The origin access control `id` names, or `None` when the store holds none.
    pub fn origin_access_control(&self, id: &str) -> Option<&StoredOriginAccessControl> {
        self.origin_access_controls.get(id)
    }

    /// The origin access control whose STORED configuration carries `name`, or `None` when
    /// no control does.
    ///
    /// `OriginAccessControlConfig$Name` is "A name to identify the origin access control",
    /// and the service refuses a create under a name an existing control holds with "An
    /// origin access control with the same name already exists" — so at most one control
    /// answers, and a repeated name is a conflict rather than a second control, whatever
    /// the rest of the parameters say.
    ///
    /// A scan over the controls rather than a second index beside them: the stored
    /// records are the single source of truth for which names are taken, so a scan
    /// cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_ORIGIN_ACCESS_CONTROLS`].
    pub fn origin_access_control_named(&self, name: &str) -> Option<&StoredOriginAccessControl> {
        self.origin_access_controls
            .values()
            .find(|control| control.config.name == name)
    }

    /// The distribution that attaches the origin access control `id` to one of its
    /// origins, or `None` when no stored distribution does.
    ///
    /// `OriginAccessControlInUse` is documented as "Cannot delete the origin access
    /// control because it's in use by one or more distributions", and `Origin$
    /// OriginAccessControlId` is the only place a configuration names a control, so what
    /// makes a control in use is a stored origin naming it. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of
    /// step with what a read of those distributions reports; both collections are bounded
    /// ([`MAX_DISTRIBUTIONS`] distributions, each with its own bounded origin list).
    ///
    /// An empty `id` names no control — [`resolve_distribution_config`] stores the empty
    /// string for an origin that named none — so it never matches.
    pub fn distribution_using_origin_access_control(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        if id.is_empty() {
            return None;
        }
        self.distributions.values().find(|distribution| {
            distribution
                .config
                .origins
                .items
                .iter()
                .any(|origin| origin.origin_access_control_id.as_deref() == Some(id))
        })
    }

    /// The origin access control a configuration's origins name that this account does
    /// NOT hold, or `None` when every control they name exists.
    ///
    /// An origin reaches its bucket WITH the credentials of the origin access control the
    /// configuration names, so a configuration can only be stored while every control it
    /// names exists: the reference is resolved against the stored controls on the way IN,
    /// on every path that stores a `DistributionConfig`, and an id the store does not hold
    /// — never created, or created and since deleted — is refused with the modelled
    /// `InvalidOriginAccessControl` before anything is written. The check is the mirror of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_origin_access_control`],
    /// which is what keeps a control a stored origin names from being deleted, so the two
    /// together are why no stored origin can name a control that is gone.
    ///
    /// `Origin$OriginAccessControlId` is the only place a configuration names a control,
    /// and an empty id names none — [`resolve_distribution_config`] stores the empty string
    /// for an origin that named no control — so an absent or empty member is not a
    /// reference and resolves to nothing.
    pub fn unknown_origin_access_control<'config>(
        &self,
        config: &'config DistributionConfig,
    ) -> Option<&'config str> {
        config
            .origins
            .items
            .iter()
            .filter_map(|origin| origin.origin_access_control_id.as_deref())
            .filter(|id| !id.is_empty())
            .find(|id| self.origin_access_control(id).is_none())
    }

    /// The origin request policy `id` names, or `None` when the store holds none.
    ///
    /// The ACCOUNT's own policies only — what a mutation addresses, since nothing in this
    /// account changes a policy AWS published. A READ addresses both, through
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::readable_origin_request_policy`].
    pub fn origin_request_policy(&self, id: &str) -> Option<&StoredOriginRequestPolicy> {
        self.origin_request_policies.get(id)
    }

    /// The origin request policy `id` names wherever it came from: one this account created
    /// with `CreateOriginRequestPolicy`, or one AWS published
    /// ([`MANAGED_ORIGIN_REQUEST_POLICIES`]).
    ///
    /// The same rule [`Self::readable_cache_policy`] states for the cache policy family: an
    /// origin request policy id addresses ONE policy whichever of the two created it, so
    /// every read of a policy by id resolves both and an id outside both names no policy at
    /// all. The account's store is consulted first, so a record it holds is always what
    /// answers.
    pub fn readable_origin_request_policy(
        &self,
        id: &str,
    ) -> Option<Cow<'_, StoredOriginRequestPolicy>> {
        match self.origin_request_policies.get(id) {
            Some(stored) => Some(Cow::Borrowed(stored)),
            None => managed_origin_request_policy(id).map(|managed| Cow::Owned(managed.to_stored())),
        }
    }

    /// The distribution that attaches the origin request policy `id` to one of its cache
    /// behaviours, or `None` when no stored distribution does.
    ///
    /// `OriginRequestPolicyInUse` is documented as "Cannot delete the origin request policy
    /// because it is attached to one or more cache behaviors", and a behaviour attaches a
    /// policy through its `OriginRequestPolicyId` — so what makes a policy in use is a stored
    /// behaviour naming it, which is the rule [`references_origin_request_policy`] states for
    /// the whole configuration. The SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for the other policy a cache behaviour attaches. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those distributions reports; both collections are bounded
    /// ([`MAX_DISTRIBUTIONS`] distributions, each with a bounded behaviour list). A behaviour
    /// that attaches no policy carries no `OriginRequestPolicyId` at all rather than an empty
    /// one, so an id matches only a behaviour that really names it.
    pub fn distribution_using_origin_request_policy(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_origin_request_policy(&distribution.config, id))
    }

    /// The cache policy `id` names, or `None` when the store holds none.
    ///
    /// The ACCOUNT's own policies only — what a mutation addresses, since nothing in this
    /// account changes a policy AWS published. A READ addresses both, through
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::readable_cache_policy`].
    pub fn cache_policy(&self, id: &str) -> Option<&StoredCachePolicy> {
        self.cache_policies.get(id)
    }

    /// The cache policy `id` names wherever it came from: one this account created with
    /// `CreateCachePolicy`, or one AWS published ([`MANAGED_CACHE_POLICIES`]).
    ///
    /// A cache policy id addresses ONE policy whichever of the two created it — "The unique
    /// identifier for the cache policy" — so every read of a policy by id resolves both,
    /// and an id outside both names no policy at all. The account's store is consulted
    /// first, so a record it holds is always what answers.
    pub fn readable_cache_policy(&self, id: &str) -> Option<Cow<'_, StoredCachePolicy>> {
        match self.cache_policies.get(id) {
            Some(stored) => Some(Cow::Borrowed(stored)),
            None => managed_cache_policy(id).map(|managed| Cow::Owned(managed.to_stored())),
        }
    }

    /// The distribution that attaches the cache policy `id` to one of its cache
    /// behaviours, or `None` when no stored distribution does.
    ///
    /// `CachePolicyInUse` is documented as "Cannot delete the cache policy because it is
    /// attached to one or more cache behaviors", and a behaviour attaches a policy through
    /// its `CachePolicyId` — so what makes a policy in use is a stored behaviour naming it,
    /// which is the rule [`references_cache_policy`] states for the whole configuration.
    /// The scan reads the STORED configurations rather than a second index beside them, so
    /// it cannot drift out of step with what a read of those distributions reports; both
    /// collections are bounded ([`MAX_DISTRIBUTIONS`] distributions, each with a bounded
    /// behaviour list). A behaviour that attaches no policy carries no `CachePolicyId` at
    /// all rather than an empty one, so an id matches only a behaviour that really names it.
    pub fn distribution_using_cache_policy(&self, id: &str) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_cache_policy(&distribution.config, id))
    }

    /// The public key `id` names, or `None` when the store holds none.
    pub fn public_key(&self, id: &str) -> Option<&StoredPublicKey> {
        self.public_keys.get(id)
    }

    /// The public key uploaded under `caller_reference`, or `None` when no key was.
    ///
    /// `PublicKeyConfig$CallerReference` is "A string included in the request to help make
    /// sure that the request can't be replayed", so at most one key answers and this is
    /// what tells a replayed create from a second key under a reference already taken.
    ///
    /// A scan over the keys rather than a second index beside them: the stored records are
    /// the single source of truth for which references are taken, so a scan cannot drift
    /// out of step with them, and the collection is bounded by [`MAX_PUBLIC_KEYS`].
    pub fn public_key_by_caller_reference(&self, caller_reference: &str) -> Option<&StoredPublicKey> {
        self.public_keys
            .values()
            .find(|key| key.config.caller_reference == caller_reference)
    }

    /// The public key whose STORED configuration carries `name`, or `None`.
    ///
    /// `PublicKeyConfig$Name` is "A name to help identify the public key", and the create
    /// models `PublicKeyAlreadyExists` for a name another key of the account already holds
    /// — so at most one key answers, and this is what makes a repeated name a conflict
    /// rather than a second key. The SAME scan
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::key_group_named`] makes for the
    /// neighbouring resource: the stored records are the single source of truth for which
    /// names are taken, and the collection is bounded by [`MAX_PUBLIC_KEYS`].
    pub fn public_key_named(&self, name: &str) -> Option<&StoredPublicKey> {
        self.public_keys
            .values()
            .find(|key| key.config.name == name)
    }

    /// The key group whose STORED configuration lists the public key `public_key_id` names,
    /// or `None` when no stored group does.
    ///
    /// "A key group contains a list of public keys that you can use with CloudFront signed
    /// URLs and signed cookies", and that list is `KeyGroupConfig$Items` — "A list of the
    /// identifiers of the public keys in the key group" — so what puts a public key in use is
    /// a stored group naming it, whichever position of whichever group's list names it. The
    /// SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for a policy a cache behaviour attaches. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those groups reports, and both collections are bounded
    /// ([`MAX_KEY_GROUPS`] groups, each with a bounded key list).
    pub fn key_group_using_public_key(&self, public_key_id: &str) -> Option<&StoredKeyGroup> {
        self.key_groups
            .values()
            .find(|group| group.config.items.iter().any(|id| id == public_key_id))
    }

    /// The field-level encryption profile whose STORED configuration names the public key
    /// `public_key_id` names in one of its encryption entities, or `None` when no stored
    /// profile does.
    ///
    /// An encryption entity names the key its matching fields are encrypted with through
    /// `EncryptionEntity$PublicKeyId` — "The public key associated with a set of field-level
    /// encryption patterns, to be used when encrypting the fields that match the patterns" —
    /// so what puts a public key in use is a stored profile naming it, which is the same rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::key_group_using_public_key`] states
    /// for the other way a stored record reaches a public key. The entity list is optional on
    /// the wire, so a profile carrying none names no key at all rather than an unknown one,
    /// and the scan reads the stored configurations rather than a second index beside them.
    pub fn field_level_encryption_profile_using_public_key(
        &self,
        public_key_id: &str,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        self.field_level_encryption_profiles.values().find(|stored| {
            stored
                .config
                .encryption_entities
                .items
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|entity| entity.public_key_id == public_key_id)
        })
    }

    /// The key group `id` names, or `None` when the store holds none.
    pub fn key_group(&self, id: &str) -> Option<&StoredKeyGroup> {
        self.key_groups.get(id)
    }

    /// The distribution that references the key group `id` from one of its cache
    /// behaviours, or `None` when no stored distribution does.
    ///
    /// "You cannot delete a key group that is referenced in a cache behavior", and a
    /// behaviour references a group through its `TrustedKeyGroups` list — so what makes a
    /// group in use is a stored behaviour naming it, which is the rule
    /// [`references_key_group`] states for the whole configuration. The SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for a policy a cache behaviour attaches. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those distributions reports; both collections are bounded
    /// ([`MAX_DISTRIBUTIONS`] distributions, each with a bounded behaviour list).
    pub fn distribution_using_key_group(&self, id: &str) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_key_group(&distribution.config, id))
    }

    /// The key group whose STORED configuration carries `name`, or `None`.
    ///
    /// `KeyGroupConfig$Name` is "A name to identify the key group", and the create models
    /// `KeyGroupAlreadyExists` for a name another group already holds — so at most one
    /// group answers, and this is what makes a repeated name a conflict rather than a
    /// second group. The comparison is against the stored configuration, never against a
    /// request, and the collection is bounded by [`MAX_KEY_GROUPS`].
    pub fn key_group_named(&self, name: &str) -> Option<&StoredKeyGroup> {
        self.key_groups
            .values()
            .find(|group| group.config.name == name)
    }

    /// The field-level encryption profile `id` names, or `None` when the store holds none.
    pub fn field_level_encryption_profile(
        &self,
        id: &str,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        self.field_level_encryption_profiles.get(id)
    }

    /// The field-level encryption profile whose STORED configuration carries `name`, or
    /// `None`.
    ///
    /// `FieldLevelEncryptionProfileConfig$Name` is "Profile name for the field-level
    /// encryption profile", and the create models
    /// `FieldLevelEncryptionProfileAlreadyExists` for a name another profile of the account
    /// already holds — so at most one profile answers, and this is what makes a repeated
    /// name a conflict rather than a second profile. The SAME scan
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::public_key_named`] makes, bounded by
    /// [`MAX_FIELD_LEVEL_ENCRYPTION_PROFILES`].
    pub fn field_level_encryption_profile_named(
        &self,
        name: &str,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        self.field_level_encryption_profiles
            .values()
            .find(|stored| stored.config.name == name)
    }

    /// The field-level encryption profile created under `caller_reference`, or `None` when
    /// no profile was.
    ///
    /// `FieldLevelEncryptionProfileConfig$CallerReference` is "A unique number that ensures
    /// that the request can't be replayed", so at most one profile answers and this is what
    /// tells a replayed create from a second profile under a reference already taken.
    ///
    /// A scan over the profiles rather than a second index beside them, for the same reason
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::public_key_by_caller_reference`]
    /// scans: the stored records are the single source of truth for which references are
    /// taken, so a scan cannot drift out of step with them, and the collection is bounded
    /// by [`MAX_FIELD_LEVEL_ENCRYPTION_PROFILES`].
    pub fn field_level_encryption_profile_by_caller_reference(
        &self,
        caller_reference: &str,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        self.field_level_encryption_profiles
            .values()
            .find(|profile| profile.config.caller_reference == caller_reference)
    }

    /// The field-level encryption configuration `id` names, or `None` when the store holds
    /// none.
    pub fn field_level_encryption_config(
        &self,
        id: &str,
    ) -> Option<&StoredFieldLevelEncryptionConfig> {
        self.field_level_encryption_configs.get(id)
    }

    /// The field-level encryption configuration created under `caller_reference`, or `None`
    /// when no configuration was.
    ///
    /// `FieldLevelEncryptionConfig$CallerReference` is "A unique number that ensures the
    /// request can't be replayed", so at most one configuration answers and this is what
    /// tells a replayed create from a second configuration under a reference already taken.
    ///
    /// A scan over the configurations rather than a second index beside them, for the same
    /// reason
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::field_level_encryption_profile_by_caller_reference`]
    /// scans: the stored records are the single source of truth for which references are
    /// taken, so a scan cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_FIELD_LEVEL_ENCRYPTION_CONFIGS`].
    pub fn field_level_encryption_config_by_caller_reference(
        &self,
        caller_reference: &str,
    ) -> Option<&StoredFieldLevelEncryptionConfig> {
        self.field_level_encryption_configs
            .values()
            .find(|config| config.config.caller_reference == caller_reference)
    }

    /// The distribution that uses the field-level encryption configuration `id` names for
    /// one of its cache behaviours, or `None` when no stored distribution does.
    ///
    /// A cache behaviour names the configuration its matching fields are encrypted under
    /// through its `FieldLevelEncryptionId` — "the value of `ID` for the field-level
    /// encryption configuration that you want CloudFront to use for encrypting specific
    /// fields of data for this cache behavior" — so what puts a configuration in use is a
    /// stored behaviour naming it, which is the rule
    /// [`references_field_level_encryption_config`] states for the whole configuration. The
    /// SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for a policy a cache behaviour attaches. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those distributions reports, and both collections are bounded
    /// ([`MAX_DISTRIBUTIONS`] distributions, each with a bounded behaviour list).
    pub fn distribution_using_field_level_encryption_config(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_field_level_encryption_config(&distribution.config, id))
    }

    /// The field-level encryption configuration that uses the profile `id` names in one of
    /// its mappings, or `None` when no stored configuration does.
    ///
    /// A configuration names the profile its matching fields are encrypted under through a
    /// mapping's `ProfileId` — `QueryArgProfile$ProfileId` is "ID of profile to use for
    /// field-level encryption query argument-profile mapping" and
    /// `ContentTypeProfile$ProfileId` is "The profile ID for a field-level encryption content
    /// type-profile mapping" — so what puts a profile in use is a stored configuration naming
    /// it, which is the rule [`references_field_level_encryption_profile`] states for the whole
    /// configuration. The SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_field_level_encryption_config`]
    /// states for the configuration a cache behaviour names. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those configurations reports, and both mapping lists are bounded
    /// ([`MAX_FIELD_LEVEL_ENCRYPTION_CONFIGS`] configurations, each with bounded mapping
    /// lists).
    pub fn field_level_encryption_config_using_profile(
        &self,
        id: &str,
    ) -> Option<&StoredFieldLevelEncryptionConfig> {
        self.field_level_encryption_configs
            .values()
            .find(|stored| references_field_level_encryption_profile(&stored.config, id))
    }

    /// The cache policy whose STORED configuration carries `name`, or `None`.
    ///
    /// `CachePolicyConfig$Name` is "A unique name to identify the cache policy", and the
    /// create models `CachePolicyAlreadyExists` for a name another policy already holds —
    /// so at most one policy answers, and this is what makes a repeated name a conflict
    /// rather than a second policy. The comparison is against the stored configuration,
    /// never against a request.
    pub fn cache_policy_named(&self, name: &str) -> Option<&StoredCachePolicy> {
        self.cache_policies
            .values()
            .find(|policy| policy.config.name == name)
    }

    /// The origin request policy whose STORED configuration carries `name`, or `None`.
    ///
    /// `OriginRequestPolicyConfig$Name` is "A unique name to identify the origin request
    /// policy", and the create models `OriginRequestPolicyAlreadyExists` for a name
    /// another policy already holds — so at most one policy answers, and this is what
    /// makes a repeated name a conflict rather than a second policy. The comparison is
    /// against the stored configuration, never against a request.
    pub fn origin_request_policy_named(
        &self,
        name: &str,
    ) -> Option<&StoredOriginRequestPolicy> {
        self.origin_request_policies
            .values()
            .find(|policy| policy.config.name == name)
    }

    /// The tag set of the resource `arn` names, across every resource kind this store
    /// holds, or `None` when it holds no such resource.
    ///
    /// A tagging call names its resource by ARN rather than by id or name, and the ARN a
    /// resource carries is the one minted when it was created, so the lookup compares
    /// against the STORED value rather than re-deriving an identifier out of the
    /// request. An ARN naming a kind of CloudFront resource this emulator does not hold
    /// matches nothing, which is exactly what makes it a resource that does not exist.
    pub fn tags_for_resource(&self, arn: &str) -> Option<&TagSet> {
        if let Some(distribution) = self
            .distributions
            .values()
            .find(|distribution| distribution.arn == arn)
        {
            return Some(&distribution.tags);
        }
        if let Some(store) = self.key_value_store_by_arn(arn) {
            return Some(&store.tags);
        }
        if let Some(group) = self
            .connection_groups
            .values()
            .find(|group| group.arn == arn)
        {
            return Some(&group.tags);
        }
        if let Some(function) = self.function_by_arn(arn) {
            return Some(&function.tags);
        }
        if let Some(function) = self.connection_function_by_arn(arn) {
            return Some(&function.tags);
        }
        if let Some(distribution) = self
            .streaming_distributions
            .values()
            .find(|distribution| distribution.arn == arn)
        {
            return Some(&distribution.tags);
        }
        if let Some(list) = self.anycast_ip_lists.values().find(|list| list.arn == arn) {
            return Some(&list.tags);
        }
        if let Some(trust_store) = self
            .trust_stores
            .values()
            .find(|trust_store| trust_store.arn == arn)
        {
            return Some(&trust_store.tags);
        }
        if let Some(vpc_origin) = self
            .vpc_origins
            .values()
            .find(|vpc_origin| vpc_origin.arn == arn)
        {
            return Some(&vpc_origin.tags);
        }
        self.distribution_tenants
            .values()
            .find(|tenant| tenant.arn == arn)
            .map(|tenant| &tenant.tags)
    }

    /// Whether this store holds the CloudFront resource `arn` names, whatever its type.
    ///
    /// The question an operation that addresses a resource by ARN has to answer before it
    /// can say anything about that resource — so it is resolved the way every other ARN
    /// lookup here resolves one, against the STORED record rather than by re-deriving an
    /// identifier out of the request:
    ///
    /// * a kind that mints an ARN of its own is matched on the ARN it carries, the very
    ///   comparison [`Self::tags_for_resource`] makes, so no two lookups can disagree about
    ///   which resource an ARN names;
    /// * a kind that mints none is matched on the id its own routes address it by, read out
    ///   of the resource part of an ARN of this service ([`cloudfront_arn_resource`]). A
    ///   policy family also answers for the policies AWS publishes, since an id addresses
    ///   one policy whichever of the two created it.
    ///
    /// `false` is "this emulator holds no such resource", whatever the ARN's type: a type
    /// this store holds nothing of at all, another service's ARN, and an ARN carrying no
    /// resource part all answer it.
    pub fn holds_resource(&self, arn: &str) -> bool {
        if self.tags_for_resource(arn).is_some() || self.realtime_log_config_by_arn(arn).is_some() {
            return true;
        }
        let Some((kind, id)) = cloudfront_arn_resource(arn) else {
            return false;
        };
        match kind {
            "cache-policy" => self.readable_cache_policy(id).is_some(),
            "continuous-deployment-policy" => self.continuous_deployment_policies.contains_key(id),
            "field-level-encryption-config" => self.field_level_encryption_configs.contains_key(id),
            "field-level-encryption-profile" => {
                self.field_level_encryption_profiles.contains_key(id)
            }
            "key-group" => self.key_groups.contains_key(id),
            "origin-access-control" => self.origin_access_controls.contains_key(id),
            "origin-access-identity" => self.origin_access_identities.contains_key(id),
            "origin-request-policy" => self.readable_origin_request_policy(id).is_some(),
            "public-key" => self.public_keys.contains_key(id),
            "response-headers-policy" => self.readable_response_headers_policy(id).is_some(),
            _ => false,
        }
    }

    /// The tag set of the resource `arn` names, MUTABLY, so a tagging call writes the
    /// very set a tag read reports rather than a second copy of it.
    ///
    /// Resolves the ARN exactly as [`ComAmazonawsCloudfrontCloudfront20200531State::tags_for_resource`]
    /// does — across every resource kind this store holds, comparing against the STORED
    /// ARN — so a tagging call and a tag read can never disagree about which resource an
    /// ARN names. `None` is an ARN this emulator holds no resource for.
    pub fn tags_for_resource_mut(&mut self, arn: &str) -> Option<&mut TagSet> {
        if let Some(distribution) = self
            .distributions
            .values_mut()
            .find(|distribution| distribution.arn == arn)
        {
            return Some(&mut distribution.tags);
        }
        if let Some(store) = self
            .key_value_stores
            .values_mut()
            .find(|store| store.arn == arn)
        {
            return Some(&mut store.tags);
        }
        if let Some(group) = self
            .connection_groups
            .values_mut()
            .find(|group| group.arn == arn)
        {
            return Some(&mut group.tags);
        }
        // The FIELD rather than the [`Self::function_by_arn`] lookup the tag READ goes
        // through: a method taking `&mut self` would borrow the whole store, which the
        // arms above already borrow a field of. The comparison is the same one.
        if let Some(function) = self
            .functions
            .values_mut()
            .find(|function| function.arn == arn)
        {
            return Some(&mut function.tags);
        }
        // The connection function arm, resolved against the STORED ARN exactly as the tag
        // read resolves it, and against the field for the same borrow reason as above.
        if let Some(function) = self
            .connection_functions
            .values_mut()
            .find(|function| function.arn == arn)
        {
            return Some(&mut function.tags);
        }
        // The streaming distribution arm, resolved against the STORED ARN exactly as the
        // tag read resolves it, and against the field for the same borrow reason as above.
        if let Some(distribution) = self
            .streaming_distributions
            .values_mut()
            .find(|distribution| distribution.arn == arn)
        {
            return Some(&mut distribution.tags);
        }
        // The Anycast static IP list arm: a list carries the tags its create submitted, so
        // it is one of the resource kinds a tagging call addresses by ARN.
        if let Some(list) = self
            .anycast_ip_lists
            .values_mut()
            .find(|list| list.arn == arn)
        {
            return Some(&mut list.tags);
        }
        // The trust store arm: a trust store carries the tags its create submitted, so it
        // is one of the resource kinds a tagging call addresses by ARN.
        if let Some(trust_store) = self
            .trust_stores
            .values_mut()
            .find(|trust_store| trust_store.arn == arn)
        {
            return Some(&mut trust_store.tags);
        }
        // The VPC origin arm: a VPC origin carries the tags its create submitted, so it is
        // one of the resource kinds a tagging call addresses by ARN.
        if let Some(vpc_origin) = self
            .vpc_origins
            .values_mut()
            .find(|vpc_origin| vpc_origin.arn == arn)
        {
            return Some(&mut vpc_origin.tags);
        }
        // The distribution tenant arm, so a tagging call and a tag read agree about every
        // resource kind this store holds and tags.
        self.distribution_tenants
            .values_mut()
            .find(|tenant| tenant.arn == arn)
            .map(|tenant| &mut tenant.tags)
    }

    /// Whether this store already holds as many distributions as it can
    /// ([`MAX_DISTRIBUTIONS`]), so the next create is refused.
    ///
    /// Read by [`Self::create_distribution`], which is what refuses the create, and by the
    /// handlers BEFORE they open the listener a new distribution's edge would be served on:
    /// the bound is what keeps a caller from making this emulator hold more state than it
    /// can, so a create it refuses must not have cost a host resource on the way.
    pub fn at_distribution_capacity(&self) -> bool {
        self.distributions.len() >= MAX_DISTRIBUTIONS
    }

    /// Persist a new distribution carrying `config` and `tags`, and report the record
    /// that was stored — or, when the caller reference already names a distribution
    /// carrying this very configuration, report THAT record and store nothing.
    ///
    /// The ONE create path. `CreateDistribution` and `CreateDistributionWithTags` create
    /// the same resource under the same rules and differ only in whether the request
    /// carries a tag set, so the configuration check, the caller-reference rule, the
    /// store bound, the mint and the insert all live here and cannot drift apart.
    ///
    /// Every refusal is decided BEFORE anything is claimed, so a refused create leaves
    /// the store exactly as it was: no id, domain name, version token or caller
    /// reference is spent on it.
    pub fn create_distribution(
        &mut self,
        config: DistributionConfig,
        tags: TagSet,
        now: SystemTime,
        edge_endpoint: Option<EdgeEndpoint>,
    ) -> Result<CreatedDistribution, CreateRefusal> {
        // "The value of Quantity and the size of Items don't match", and then the published
        // per-list quotas: the refusals a configuration carries BY ITSELF, read through the
        // one statement of them a handler answers first as well, so a malformed or oversized
        // configuration is refused before any id or caller reference is claimed for it and
        // before anything of it is resolved against another service.
        if let Some(refusal) = config_refusal_without_state(&config) {
            return Err(refusal);
        }

        // An alternate domain name is served only under a certificate that validates it,
        // so a configuration whose aliases and viewer certificate disagree is refused
        // before any distribution exists to carry the alias.
        if let Some(message) = alias_certificate_mismatch(&config) {
            return Err(CreateRefusal::InvalidViewerCertificate(message));
        }

        // A CloudFront function runs on a VIEWER event, so a configuration whose function
        // association names an origin-facing one is refused before any distribution is
        // created carrying an association CloudFront could not invoke.
        if let Some(message) = function_association_event_refusal(&config) {
            return Err(CreateRefusal::InvalidArgument(message));
        }

        // A Lambda@Edge association invokes ONE published version of a function, so a
        // configuration whose association names no version is refused before any
        // distribution is created carrying it.
        if let Some(message) = lambda_association_without_version(&config) {
            return Err(CreateRefusal::InvalidLambdaFunctionAssociation(message));
        }

        // A cache behaviour runs the code a CloudFront function's `LIVE` stage holds, so a
        // configuration whose function association names a function nothing has published
        // to that stage is refused before any distribution is created referencing code that
        // does not exist at the edge.
        if let Some(arn) = self.function_association_without_live_stage(&config) {
            return Err(CreateRefusal::InvalidArgument(
                invalid_function_association_message(arn),
            ));
        }

        // An origin reaches a private bucket EITHER with the credentials of an origin
        // access control OR as an origin access identity, never both: a configuration whose
        // origin names both says nothing about which signs it, so it is refused before any
        // distribution is created from it.
        if origin_with_both_access_mechanisms(&config) {
            return Err(CreateRefusal::IllegalOriginAccessConfiguration(
                ILLEGAL_ORIGIN_ACCESS_CONFIGURATION_MESSAGE.to_string(),
            ));
        }

        // An origin signs its requests to the bucket with the credentials of the origin
        // access control it names, so a configuration can only be stored while every
        // control it names exists: a reference to one this account does not hold is
        // refused before any distribution is created from it.
        if self.unknown_origin_access_control(&config).is_some() {
            return Err(CreateRefusal::InvalidOriginAccessControl(
                INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE.to_string(),
            ));
        }

        // A multi-tenant distribution serves its tenants, not its own traffic: the members
        // the model documents as standard-distribution-only — or as a connection group's to
        // carry — are no members of its configuration, so one that states any of them is
        // refused before a distribution is created that CloudFront would not serve.
        if let Some(member) = multi_tenant_direct_only_member(&config) {
            return Err(CreateRefusal::InvalidArgument(
                multi_tenant_direct_only_message(member),
            ));
        }

        // A multi-tenant distribution's configuration is a TEMPLATE: the members that
        // address the origin may name one of the parameters its `TenantConfig` declares, and
        // nothing else may. A reference to a parameter the configuration does not declare —
        // or any reference at all in a configuration that serves traffic directly, which
        // declares none — names a value CloudFront could never substitute, so it is refused
        // before any distribution is created carrying it.
        if let Some(message) = tenant_parameter_refusal(&config) {
            return Err(CreateRefusal::InvalidArgument(message));
        }

        // A caller reference identifies ONE distribution for its whole life, and what it
        // takes part in is this: repeating a create under a reference the store already
        // holds is either the SAME request again or a second distribution under a taken
        // reference. What separates the two is the content of the configuration, which is
        // what the recorded service does on both branches: a repeat whose configuration is
        // the one that reference already names is answered with that distribution, its
        // version and its location (creating nothing), and a repeat that changes the
        // configuration is refused — "The caller reference you attempted to create the
        // distribution with is associated with another distribution."
        // [`ComAmazonawsCloudfrontCloudfront20200531State::create_invalidation`]'s caller
        // reference follows the same rule for an invalidation batch.
        //
        // The replay is an AS-CREATED privilege: a reference names the create it was sent
        // with, so it answers a repeat of that create only while the distribution still
        // holds the configuration that create stored. Once the distribution has been
        // reconfigured ([`StoredDistribution::reconfigured`]) the reference is associated
        // with a distribution no create under it describes, and every create under it is
        // refused — including one resending the distribution's own current configuration,
        // which is a configuration no create ever submitted.
        let caller_reference = config.caller_reference.clone();
        // The configuration as the service will run it: the members submitted plus every
        // member the service itself supplies. Resolved BEFORE the caller-reference
        // question so the comparison below is one between like and like — the stored
        // record is resolved too — and a repeat that states a default explicitly and one
        // that leaves it out are the same request, as they describe the same distribution.
        let config = resolve_distribution_config(config);
        // The reference index and the store are written together — a delete releases both
        // — so an entry that names no stored distribution cannot arise; were one to, the
        // reference it indexes is associated with nothing and this create claims it.
        if let Some(existing_id) = self.caller_references.get(&caller_reference).cloned() {
            if let Some(existing) = self.distribution(&existing_id) {
                if !existing.reconfigured && same_config(&existing.config, &config) {
                    // The same request again: the record the first create left behind, at
                    // its current version. Nothing is minted and nothing is stored — so a
                    // tag set this repeat carries changes nothing either, the distribution
                    // keeping the tags it was created with.
                    return Ok(CreatedDistribution {
                        id: existing.id.clone(),
                        etag: existing.etag.clone(),
                        distribution: existing.to_distribution(&self.key_groups),
                    });
                }
                return Err(CreateRefusal::DistributionAlreadyExists(
                    distribution_already_exists_message(&existing_id),
                ));
            }
        }

        // "Processing your request would cause you to exceed the maximum number of
        // distributions allowed." The store grows only here and a distribution cannot be
        // evicted (a later Get has to answer with what this create returned), so the
        // bound is an entry count refused BEFORE anything is claimed. The handlers read
        // the same bound through [`Self::at_distribution_capacity`] before they open a
        // listener for the create, so a request this refuses costs no host resource either.
        if self.at_distribution_capacity() {
            return Err(too_many_distributions());
        }

        let id = self.allocate_distribution_id();
        // The domain name a VIEWER is handed: the address of the listener this
        // distribution's edge is served on, so a client that dials what it was told reaches
        // this distribution ([`EdgeEndpoint`]). A distribution with no listener of its own —
        // a multi-tenant one, which reports no domain at all, and a create on an engine that
        // vends no listeners — carries the name CloudFront's own zone would give it.
        let domain_name = match &edge_endpoint {
            Some(endpoint) => endpoint.authority().to_string(),
            None => self.allocate_domain_name(),
        };
        let etag = self.next_etag();
        let stored = StoredDistribution {
            arn: distribution_arn(&id),
            id: id.clone(),
            domain_name,
            // A distribution CloudFront has just accepted is not yet propagated to the
            // edge locations, so it reports the transient status until the clock owes it
            // the terminal one.
            status: STATUS_IN_PROGRESS.to_string(),
            deployed_due: now + PROPAGATION_DELAY,
            last_modified_time: now,
            etag: etag.clone(),
            // Stored whole, with every member the service itself supplies resolved into
            // it: a later read answers from this record — the configuration the
            // distribution actually runs under — never from the request that was served.
            config,
            // The configuration this record holds is the one this create stored, so the
            // caller reference's replay still answers a repeat of it.
            reconfigured: false,
            tags,
            invalidations: BTreeMap::new(),
            // Additional CloudWatch metrics are enabled by a call of their own, so a
            // distribution this create minted carries no monitoring subscription until
            // `CreateMonitoringSubscription` puts one on it.
            monitoring_subscription: None,
            // Nothing is cached at the edge until the edge serves a request.
            edge_cache: BTreeMap::new(),
        };
        let created = CreatedDistribution {
            id: id.clone(),
            etag,
            distribution: stored.to_distribution(&self.key_groups),
        };
        self.caller_references.insert(caller_reference, id.clone());
        // The listener now has a distribution to serve, and its handle is held for as long
        // as that distribution is: dropping it closes the port.
        if let Some(endpoint) = edge_endpoint {
            endpoint.serves(&id);
            self.edge_endpoints.insert(id.clone(), endpoint);
        }
        self.distributions.insert(id, stored);
        Ok(created)
    }

    /// A fresh value from the mint: a scrambled counter rendered in the id alphabet.
    ///
    /// The scramble is a bijection on `u64` and 36^13 exceeds 2^64, so distinct counter
    /// values always render as distinct tokens — the mint never repeats itself.
    pub(super) fn mint(&mut self, len: usize) -> String {
        render_token(self.mint_u64(), len)
    }

    /// One fresh scrambled counter value — the single draw every mint is rendered from.
    pub(super) fn mint_u64(&mut self) -> u64 {
        self.next_token_seq = self.next_token_seq.wrapping_add(1);
        scramble(self.next_token_seq)
    }

    /// Allocate a distribution id no stored distribution holds: `E` + 13 uppercase
    /// alphanumerics, the shape CloudFront distribution ids take.
    pub fn allocate_distribution_id(&mut self) -> String {
        loop {
            let candidate = format!("E{}", self.mint(ID_BODY_LEN));
            if !self.distributions.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a streaming distribution id no stored streaming distribution holds.
    ///
    /// `StreamingDistribution$Id` is "The identifier for the RTMP distribution. For
    /// example: `EGTXBD79EXAMPLE`": the same shape a distribution id takes (`E` +
    /// [`ID_BODY_LEN`] uppercase alphanumerics), drawn from the same mint, so no two
    /// streaming distributions are minted the same id.
    pub fn allocate_streaming_distribution_id(&mut self) -> String {
        loop {
            let candidate = format!("E{}", self.mint(ID_BODY_LEN));
            if !self.streaming_distributions.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a key value store id no stored key value store holds.
    ///
    /// `KeyValueStore$Id` is "The unique Id for the key value store", and the identifier
    /// CloudFront mints for one is a UUID — the id that forms the resource part of the
    /// store's ARN, `arn:aws:cloudfront::<account>:key-value-store/<uuid>`, which is how
    /// the key value store DATA plane addresses the store. So this renders two draws of
    /// the mint (128 bits) as the 8-4-4-4-12 lowercase hex a UUID takes; distinct draws
    /// render distinctly, so the mint never repeats an id.
    pub fn allocate_key_value_store_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self
                .key_value_stores
                .values()
                .any(|store| store.id == candidate)
            {
                break candidate;
            }
        }
    }

    /// Allocate an Anycast static IP list id no stored list holds.
    ///
    /// `AnycastIpList$Id` is "The ID of the Anycast static IP list", and the model puts no
    /// pattern on it, so the shape is the one CloudFront writes such an id in:
    /// [`ANYCAST_IP_LIST_ID_PREFIX`] followed by [`ANYCAST_IP_LIST_ID_HEX_LEN`] lowercase
    /// hex digits. It is rendered from ONE draw of the mint, zero-padded to that width, so
    /// distinct draws render distinctly and no two lists are minted the same id.
    pub fn allocate_anycast_ip_list_id(&mut self) -> String {
        loop {
            let candidate = format!(
                "{ANYCAST_IP_LIST_ID_PREFIX}{:0width$x}",
                self.mint_u64(),
                width = ANYCAST_IP_LIST_ID_HEX_LEN
            );
            if !self.anycast_ip_lists.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate `ip_count` Anycast static IP address SLOTS, or `None` when the pool cannot
    /// serve that many more.
    ///
    /// "The static IP addresses that are allocated to the Anycast static IP list": the
    /// slots are allocated ONCE, here, and the list holds exactly as many of them as its
    /// `IpCount` — "The number of IP addresses in the Anycast static IP list". Which
    /// FAMILY each slot is reported in follows the list's address type and is decided where
    /// the list is read ([`anycast_ips_of`]), so a list that changes its address type keeps
    /// the allocation it already holds.
    ///
    /// Each slot comes off [`ComAmazonawsCloudfrontCloudfront20200531State::next_anycast_ip`],
    /// so no address is allocated to two lists; `None` once that counter has walked the
    /// whole of [`ANYCAST_IP_POOL`], which the caller reports as the modelled
    /// `EntityLimitExceeded`.
    ///
    /// The count a caller asks for never sizes the allocation on its own: the pool check
    /// below runs BEFORE anything is allocated, so the loop that follows runs at most
    /// [`ANYCAST_IP_POOL`] times whatever the request named, and a count that does not even
    /// fit the counter is refused there rather than allocated for.
    pub fn allocate_anycast_ip_hosts(&mut self, ip_count: i32) -> Option<Vec<u32>> {
        let wanted = u32::try_from(ip_count).ok()?;
        if self.next_anycast_ip.checked_add(wanted)? > ANYCAST_IP_POOL {
            return None;
        }
        let hosts = (0..wanted)
            .map(|_| {
                self.next_anycast_ip += 1;
                self.next_anycast_ip
            })
            .collect();
        Some(hosts)
    }

    /// Allocate a continuous deployment policy id no stored policy holds.
    ///
    /// The same UUID mint [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`]
    /// serves every resource CloudFront identifies by one: the policy-shaped resources of
    /// its control plane (cache policies, origin request policies, continuous deployment
    /// policies) carry a UUID rather than the `E`+13 token a distribution carries.
    pub fn allocate_continuous_deployment_policy_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self
                .continuous_deployment_policies
                .contains_key(&candidate)
            {
                break candidate;
            }
        }
    }

    /// Allocate a distribution tenant id no stored tenant holds:
    /// [`DISTRIBUTION_TENANT_ID_PREFIX`] followed by a minted token.
    pub fn allocate_distribution_tenant_id(&mut self) -> String {
        loop {
            let candidate = format!("{DISTRIBUTION_TENANT_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.distribution_tenants.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate an origin access identity id no stored identity holds.
    ///
    /// `CloudFrontOriginAccessIdentity$Id` is "The ID for the origin access identity, for
    /// example, `E74FTE3AJFJ256A`": the same shape a distribution id and a version token
    /// take (`E` + [`ID_BODY_LEN`] uppercase alphanumerics), drawn from the same mint, so
    /// no two identities are minted the same id.
    pub fn allocate_origin_access_identity_id(&mut self) -> String {
        loop {
            let candidate = format!("E{}", self.mint(ID_BODY_LEN));
            if !self.origin_access_identities.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a response headers policy id no stored policy holds.
    ///
    /// `ResponseHeadersPolicy$Id` is the identifier every response headers policy
    /// operation addresses one by, and CloudFront mints a UUID for one (the same shape
    /// its cache and origin request policy ids take), so this renders one draw of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`] — distinct draws
    /// render distinctly, so the mint never repeats an id.
    pub fn allocate_response_headers_policy_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self.response_headers_policies.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate an origin access control id no stored control holds.
    ///
    /// `OriginAccessControl$Id` is "The unique identifier of the origin access control",
    /// and CloudFront mints one in the shape every CloudFront entity id takes (`E` +
    /// [`ID_BODY_LEN`] uppercase alphanumerics, as `--origin-access-control-id
    /// E127EXAMPLE51Z` shows), drawn from the same mint as every other id here so no two
    /// controls are minted the same id.
    pub fn allocate_origin_access_control_id(&mut self) -> String {
        loop {
            let candidate = format!("E{}", self.mint(ID_BODY_LEN));
            if !self.origin_access_controls.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate an origin request policy id no stored policy holds.
    ///
    /// `OriginRequestPolicy$Id` is the identifier every origin request policy operation
    /// addresses one by, and CloudFront mints a UUID for one (the same shape its cache and
    /// response headers policy ids take), so this renders one draw of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`] — distinct draws render
    /// distinctly, so the mint never repeats an id.
    pub fn allocate_origin_request_policy_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self.origin_request_policies.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a public key id no stored public key holds.
    ///
    /// `PublicKey$Id` is "The identifier of the public key", and CloudFront mints one in
    /// the shape its own answers show (`K2K8NC4HVFE3MO`): a `K` followed by
    /// [`ID_BODY_LEN`] uppercase alphanumerics, the id alphabet every other entity id here
    /// is drawn from — so no two keys are minted the same id and the namespace is
    /// distinguishable from a distribution's `E`-prefixed one.
    pub fn allocate_public_key_id(&mut self) -> String {
        loop {
            let candidate = format!("{PUBLIC_KEY_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.public_keys.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a key group id no stored key group holds.
    ///
    /// `KeyGroup$Id` is "The identifier for the key group", and CloudFront mints a UUID for
    /// one (the shape its own `create-key-group` answers show, the same one its cache,
    /// origin request and response headers policy ids take), so this renders one draw of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`] — distinct draws render
    /// distinctly, so the mint never repeats an id.
    pub fn allocate_key_group_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self.key_groups.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a field-level encryption profile id no stored profile holds.
    ///
    /// `FieldLevelEncryptionProfile$Id` is "The ID for a field-level encryption profile
    /// configuration", and CloudFront mints one in the shape its own answers show
    /// (`P280MFCLSYOCVU`): a [`FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX`] followed by
    /// [`ID_BODY_LEN`] uppercase alphanumerics, the id alphabet every other entity id here
    /// is drawn from — so no two profiles are minted the same id and the namespace is
    /// distinguishable from a distribution's `E`-prefixed one.
    pub fn allocate_field_level_encryption_profile_id(&mut self) -> String {
        loop {
            let candidate = format!(
                "{FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX}{}",
                self.mint(ID_BODY_LEN)
            );
            if !self.field_level_encryption_profiles.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a field-level encryption configuration id no stored configuration holds.
    ///
    /// `FieldLevelEncryption$Id` is "The configuration ID for a field-level encryption
    /// configuration", and CloudFront mints one in the shape its own answers show: a
    /// [`FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX`] followed by [`ID_BODY_LEN`] uppercase
    /// alphanumerics, the id alphabet every other entity id here is drawn from — so no two
    /// configurations are minted the same id and the namespace is distinguishable from the
    /// profile's `P`-prefixed one.
    pub fn allocate_field_level_encryption_config_id(&mut self) -> String {
        loop {
            let candidate = format!(
                "{FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX}{}",
                self.mint(ID_BODY_LEN)
            );
            if !self.field_level_encryption_configs.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a cache policy id no stored policy holds.
    ///
    /// `CachePolicy$Id` is the identifier every cache policy operation addresses one by,
    /// and CloudFront mints a UUID for one (the same shape its origin request and response
    /// headers policy ids take), so this renders one draw of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`] — distinct draws render
    /// distinctly, so the mint never repeats an id.
    pub fn allocate_cache_policy_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self.cache_policies.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a connection group id no stored connection group holds:
    /// [`CONNECTION_GROUP_ID_PREFIX`] followed by a minted token.
    pub fn allocate_connection_group_id(&mut self) -> String {
        loop {
            let candidate = format!("{CONNECTION_GROUP_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.connection_groups.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// The id of the account's default connection group, creating the group on the first
    /// call that needs one — which is what "CloudFront will automatically create a default
    /// connection group for you" means. Every later call reads that one id back, and the
    /// record it names is what a read and a listing of that group answer from.
    ///
    /// The group this creates is the only one carrying
    /// [`StoredConnectionGroup::is_default`], and it is created exactly as
    /// `CreateConnectionGroup` creates one: a minted id, the address of the listener its
    /// routing endpoint is served on and a minted version token, `InProgress` until the
    /// clock owes it `Deployed`.
    ///
    /// `endpoint` is that listener, opened by the handler before it took this lock (the
    /// same way a distribution's is), or `None` on an engine that vends none — in which
    /// case the group carries the name CloudFront's own zone would give it. A call that
    /// finds the default group already created keeps that group's address and drops this
    /// one, which closes the port it had opened.
    pub fn default_connection_group(
        &mut self,
        now: SystemTime,
        endpoint: Option<EdgeEndpoint>,
    ) -> String {
        if let Some(group) = self.connection_groups.values().find(|group| group.is_default) {
            return group.id.clone();
        }
        let id = self.allocate_connection_group_id();
        // The automatic group's name is [`DEFAULT_CONNECTION_GROUP_NAME`] unless a create
        // already took it: a name identifies ONE group in the account, so the fallback is
        // derived from this group's own minted id, which no other group holds.
        let name = match self.connection_group_named(DEFAULT_CONNECTION_GROUP_NAME) {
            None => DEFAULT_CONNECTION_GROUP_NAME.to_string(),
            Some(_) => format!("{DEFAULT_CONNECTION_GROUP_NAME}-{id}"),
        };
        let routing_endpoint = self.minted_routing_endpoint(endpoint.as_ref());
        let etag = self.next_etag();
        let stored = StoredConnectionGroup {
            arn: connection_group_arn(&id),
            id: id.clone(),
            name,
            routing_endpoint,
            // Nothing asked for an Anycast static IP list, so the automatic group names
            // none; every other member takes the same default a create that named none
            // would get.
            anycast_ip_list_id: None,
            ipv6_enabled: DEFAULT_CONNECTION_GROUP_IPV6_ENABLED,
            enabled: DEFAULT_CONNECTION_GROUP_ENABLED,
            is_default: true,
            status: STATUS_IN_PROGRESS.to_string(),
            deployed_due: now + PROPAGATION_DELAY,
            created_time: now,
            last_modified_time: now,
            etag,
            tags: TagSet::new(),
        };
        self.connection_groups.insert(id.clone(), stored);
        // The listener now has a group to serve, and its handle is held for as long as that
        // group is: dropping it closes the port.
        if let Some(endpoint) = endpoint {
            endpoint.serves(&id);
            self.connection_group_endpoints.insert(id.clone(), endpoint);
        }
        id
    }

    /// The routing endpoint a connection group PUBLISHES: the address of the listener it is
    /// served on ([`EdgeEndpoint`]), so a viewer that dials what the group announced reaches
    /// this emulator — and the name CloudFront's own zone would give it on an engine that
    /// vends no listener.
    ///
    /// The ONE derivation, read by `CreateConnectionGroup` and by the automatic creation of
    /// the account's default group, so the two cannot publish addresses of different kinds.
    pub fn minted_routing_endpoint(&mut self, endpoint: Option<&EdgeEndpoint>) -> String {
        match endpoint {
            Some(endpoint) => endpoint.authority().to_string(),
            None => self.allocate_domain_name(),
        }
    }

    /// Allocate the Amazon S3 canonical user id of an origin access identity, in the
    /// [`S3_CANONICAL_USER_ID_LEN`] lowercase hex characters S3 writes one in.
    ///
    /// Drawn from the same mint as every other generated value here (four draws, 256
    /// bits, rendered as hex), and checked against the stored identities so no two
    /// identities share the canonical user id a bucket policy names them by.
    pub fn allocate_s3_canonical_user_id(&mut self) -> String {
        loop {
            let mut candidate = String::with_capacity(S3_CANONICAL_USER_ID_LEN);
            while candidate.len() < S3_CANONICAL_USER_ID_LEN {
                candidate.push_str(&format!("{:016x}", self.mint_u64()));
            }
            if !self
                .origin_access_identities
                .values()
                .any(|identity| identity.s3_canonical_user_id == candidate)
            {
                break candidate;
            }
        }
    }

    /// Mint the CloudFront managed ACM certificate a `ManagedCertificateRequest` asks
    /// for, covering `domains`.
    ///
    /// The identity of the certificate — its ARN and the validation token of each domain
    /// — is drawn from the one mint every other generated value here comes from, so no
    /// two certificates and no two tokens are minted the same. The certificate starts
    /// [`CERTIFICATE_STATUS_PENDING_VALIDATION`] and the CLOCK owes it its issuance, so
    /// nothing a later call does decides when it is issued.
    pub fn request_managed_certificate(
        &mut self,
        request: ManagedCertificateRequest,
        domains: &[String],
        now: SystemTime,
    ) -> StoredManagedCertificate {
        let arn = managed_certificate_arn(&self.mint_uuid());
        let validation_tokens = domains
            .iter()
            .map(|domain| StoredValidationToken {
                domain: domain.clone(),
                token: self.mint_validation_token(),
            })
            .collect();
        StoredManagedCertificate {
            arn,
            status: CERTIFICATE_STATUS_PENDING_VALIDATION.to_string(),
            issued_due: now + CERTIFICATE_VALIDATION_DELAY,
            request,
            validation_tokens,
        }
    }

    /// One fresh HTTP validation token name: two draws of the mint (128 bits) rendered as
    /// the lowercase hex an opaque token file name is written in.
    pub(super) fn mint_validation_token(&mut self) -> String {
        format!("{:016x}{:016x}", self.mint_u64(), self.mint_u64())
    }

    /// One fresh UUID from the mint: two draws (128 bits) rendered as the 8-4-4-4-12
    /// lowercase hex a UUID takes. ONE rendering for every resource CloudFront identifies
    /// by UUID, so no two of them mint ids in different shapes.
    pub(super) fn mint_uuid(&mut self) -> String {
        let high = self.mint_u64();
        let low = self.mint_u64();
        let hex = format!("{high:016x}{low:016x}");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Allocate the domain name a distribution is served on: a generated label under
    /// `cloudfront.net`, in the lowercase DNS labels take.
    ///
    /// What a distribution REPORTS is [`StoredDistribution::reported_domain_name`]: a
    /// multi-tenant distribution has no domain of its own, so it reports the placeholder
    /// instead of the label allocated here.
    ///
    /// A connection group's `RoutingEndpoint` is "also known as the DNS name ... such as
    /// `d111111abcdef8.cloudfront.net`" — the same generated name in the same shape, so it
    /// is drawn from here too rather than from a second rendering of the mint.
    pub fn allocate_domain_name(&mut self) -> String {
        let label = self.mint(ID_BODY_LEN).to_lowercase();
        format!("d{label}{CLOUDFRONT_DOMAIN_SUFFIX}")
    }

    /// Allocate an invalidation id no invalidation of the named distribution tenant holds.
    pub fn allocate_tenant_invalidation_id(&mut self, tenant_id: &str) -> String {
        loop {
            let candidate = format!("I{}", self.mint(ID_BODY_LEN));
            let taken = self
                .distribution_tenants
                .get(tenant_id)
                .is_some_and(|tenant| tenant.invalidations.contains_key(&candidate));
            if !taken {
                break candidate;
            }
        }
    }

    /// Allocate an invalidation id no invalidation of the named distribution holds.
    pub fn allocate_invalidation_id(&mut self, distribution_id: &str) -> String {
        loop {
            let candidate = format!("I{}", self.mint(ID_BODY_LEN));
            let taken = self
                .distributions
                .get(distribution_id)
                .is_some_and(|distribution| distribution.invalidations.contains_key(&candidate));
            if !taken {
                break candidate;
            }
        }
    }

    /// Mint the version token a mutation hands back. Every call yields a fresh one, so
    /// an `If-Match` carrying the version from before a mutation no longer matches.
    ///
    /// Same shape as a distribution id (`E` + 13), which is the shape CloudFront's own
    /// `ETag` values take; it is drawn from the same counter, so no token this service
    /// mints is ever minted twice.
    pub fn next_etag(&mut self) -> String {
        format!("E{}", self.mint(ID_BODY_LEN))
    }

    /// Mint the identifier the edge stamps on one request it serves.
    ///
    /// `X-Amz-Cf-Id` "identifies the request that you're interested in" — one opaque
    /// token per request, so it is drawn fresh from the same counter every other
    /// generated value comes from and is never derived from the request. Rendered as 32
    /// lowercase hex characters (two draws, 128 bits): an opaque token, which is all a
    /// client may read it as.
    pub fn mint_edge_request_id(&mut self) -> String {
        format!("{:016x}{:016x}", self.mint_u64(), self.mint_u64())
    }

    /// Replace the configuration of the distribution `id` names, and report the record
    /// as it now stands.
    ///
    /// The stored configuration becomes the one submitted with the service's own members
    /// resolved into it ([`resolve_distribution_config`]), `LastModifiedTime` becomes
    /// `now`, the version token is freshly minted, and the distribution goes back to
    /// [`STATUS_IN_PROGRESS`] with a new propagation deadline — the new configuration has
    /// to reach the edge locations before it is deployed again.
    ///
    /// `None` when the store holds no such distribution, checked BEFORE the mint so a
    /// version token is never spent on a distribution that is not there.
    ///
    /// The record is marked RECONFIGURED: the configuration it now holds is no longer the
    /// one its create stored, so the caller reference that created it no longer answers a
    /// repeat of that create ([`StoredDistribution::reconfigured`]).
    pub fn apply_config(
        &mut self,
        id: &str,
        config: DistributionConfig,
        now: SystemTime,
    ) -> Option<&StoredDistribution> {
        if !self.distributions.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.distributions.get_mut(id)?;
        stored.config = resolve_distribution_config(config);
        stored.reconfigured = true;
        stored.last_modified_time = now;
        stored.etag = etag;
        stored.status = STATUS_IN_PROGRESS.to_string();
        stored.deployed_due = now + PROPAGATION_DELAY;
        Some(stored)
    }

    /// The `Distribution` payload of the record `id` names, beside the version token that
    /// record carries, or `None` when the store holds no such distribution.
    ///
    /// ONE place a `Distribution` payload is built from this store, because the projection
    /// reads TWO parts of it: the distribution record itself and the account's key groups,
    /// whose public keys the active key group set reports
    /// ([`StoredDistribution::active_trusted_key_groups`]). Every operation that answers
    /// with a `Distribution` goes through it, so they cannot report different things for
    /// the same state.
    pub fn distribution_payload(&self, id: &str) -> Option<(Distribution, String)> {
        let stored = self.distributions.get(id)?;
        Some((stored.to_distribution(&self.key_groups), stored.etag.clone()))
    }

    /// Set or clear the WAF web ACL customization of the distribution tenant `id` names,
    /// and report the record as it now stands.
    ///
    /// `Customizations$WebAcl` is "the WAF web ACL" a tenant overrides or disables, and it
    /// is the ONE member a web ACL association of a tenant writes: `Some` stores the
    /// customization, `None` takes it off the record, and a customization set that is then
    /// empty is dropped altogether rather than reported as an element describing nothing.
    /// The tenant's other customizations are carried over untouched, because the
    /// association says nothing about them.
    ///
    /// The write versions the tenant exactly as any other change to its configuration
    /// does — `LastModifiedTime` becomes `now`, a fresh version token is minted so the
    /// token the caller matched on no longer satisfies an `If-Match`, and the tenant goes
    /// back to [`STATUS_IN_PROGRESS`] with a new propagation deadline, since the new
    /// configuration has to reach the edge locations before it is deployed again.
    ///
    /// `None` when the store holds no such tenant, checked BEFORE the mint so a version
    /// token is never spent on a tenant that is not there.
    pub fn apply_tenant_web_acl(
        &mut self,
        id: &str,
        web_acl: Option<WebAclCustomization>,
        now: SystemTime,
    ) -> Option<&StoredDistributionTenant> {
        if !self.distribution_tenants.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let tenant = self.distribution_tenants.get_mut(id)?;
        let mut customizations = tenant.customizations.take().unwrap_or_default();
        customizations.web_acl = web_acl;
        tenant.customizations = retained_customizations(customizations);
        tenant.last_modified_time = now;
        tenant.etag = etag;
        tenant.status = STATUS_IN_PROGRESS.to_string();
        tenant.deployed_due = now + PROPAGATION_DELAY;
        Some(tenant)
    }

    /// Apply an `UpdateDistributionTenant` to the tenant `id` names, and report the record
    /// as it now stands.
    ///
    /// Every member the update submits is optional and each is documented as what the
    /// tenant is updated TO ("the domains to update", "updated to an enabled state"), so a
    /// member the request leaves out is left exactly as it was and one it states REPLACES
    /// what the record held — the same rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_anycast_ip_list_update`]
    /// follows. A stated customization set that names nothing is retained as none at all,
    /// so the record never carries an element describing nothing.
    ///
    /// A `ManagedCertificateRequest` on the update MINTS a certificate, over the domains
    /// the tenant holds once the update has landed, and replaces whatever certificate the
    /// record carried; an update that asks for none leaves the existing one alone.
    ///
    /// The write versions the tenant exactly as every other change to its configuration
    /// does: `LastModifiedTime` becomes `now`, a fresh version token is minted so the token
    /// the caller matched on no longer satisfies an `If-Match`, and the tenant goes back to
    /// [`STATUS_IN_PROGRESS`] with a new propagation deadline, since the configuration it
    /// now runs under has to reach the edge locations before it is deployed again. The
    /// tenant's identity — its id, its ARN, its name and its creation time — is untouched:
    /// an update submits none of them, and "[the name] can't be updated after you create the
    /// distribution tenant".
    ///
    /// `None` when the store holds no such tenant, checked BEFORE the mint so a version
    /// token is never spent on a tenant that is not there.
    pub fn apply_distribution_tenant_update(
        &mut self,
        id: &str,
        update: DistributionTenantUpdate,
        now: SystemTime,
    ) -> Option<&StoredDistributionTenant> {
        if !self.distribution_tenants.contains_key(id) {
            return None;
        }
        let DistributionTenantUpdate {
            distribution_id,
            domains,
            customizations,
            parameters,
            connection_group_id,
            enabled,
            managed_certificate_request,
        } = update;
        // The certificate covers the domains the tenant holds AFTER this update, so it is
        // minted from the effective list rather than from the one the record carried.
        let certificate_domains = domains.clone().unwrap_or_else(|| {
            self.distribution_tenants
                .get(id)
                .map(|tenant| tenant.domains.clone())
                .unwrap_or_default()
        });
        let etag = self.next_etag();
        let managed_certificate = managed_certificate_request
            .map(|request| self.request_managed_certificate(request, &certificate_domains, now));
        let tenant = self.distribution_tenants.get_mut(id)?;
        if let Some(distribution_id) = distribution_id {
            tenant.distribution_id = distribution_id;
        }
        if let Some(domains) = domains {
            tenant.domains = domains;
        }
        if let Some(customizations) = customizations {
            tenant.customizations = retained_customizations(customizations);
        }
        if let Some(parameters) = parameters {
            tenant.parameters = parameters;
        }
        if let Some(connection_group_id) = connection_group_id {
            tenant.connection_group_id = connection_group_id;
        }
        if let Some(enabled) = enabled {
            tenant.enabled = enabled;
        }
        if let Some(certificate) = managed_certificate {
            tenant.managed_certificate = Some(certificate);
        }
        tenant.last_modified_time = now;
        tenant.etag = etag;
        tenant.status = STATUS_IN_PROGRESS.to_string();
        tenant.deployed_due = now + PROPAGATION_DELAY;
        Some(tenant)
    }

    /// Replace the configuration of the cache policy `id` names, and report the record as
    /// it now stands.
    ///
    /// `UpdateCachePolicy` replaces the configuration WHOLE — "When you update a cache
    /// policy configuration, all the fields are updated with the values provided in the
    /// request. You cannot update some fields independent of others." — so the stored
    /// configuration becomes the one submitted, down to the members the request left out;
    /// `LastModifiedTime` becomes `now` ("the date and time when the cache policy was last
    /// modified"), and the version token is freshly minted, so the token the caller matched
    /// on no longer satisfies an `If-Match` against this policy. Nothing else about a cache
    /// policy changes: the id is the one it was created under, and a policy carries no
    /// status and no propagation deadline.
    ///
    /// `None` when the store holds no such policy, checked BEFORE the mint so a version
    /// token is never spent on a policy that is not there.
    pub fn apply_cache_policy_config(
        &mut self,
        id: &str,
        config: CachePolicyConfig,
        now: SystemTime,
    ) -> Option<&StoredCachePolicy> {
        if !self.cache_policies.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.cache_policies.get_mut(id)?;
        // Stored RESOLVED, exactly as the create stores it: the members CloudFront supplies
        // for the ones this payload left out ([`resolve_cache_policy_config`]).
        stored.config = resolve_cache_policy_config(config);
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the response headers policy `id` names, and report the
    /// record as it now stands.
    ///
    /// The same write [`Self::apply_cache_policy_config`] performs for the cache policy, on
    /// the response headers policy record: `UpdateResponseHeadersPolicy` replaces the
    /// configuration WHOLE — "When you update a response headers policy, the entire policy is
    /// replaced. You cannot update some policy fields independent of others." — so the stored
    /// configuration becomes the one submitted, down to the members the request left out;
    /// `LastModifiedTime` becomes `now` ("the date and time when the response headers policy
    /// was last modified"), and the version token is freshly minted, so the token the caller
    /// matched on no longer satisfies an `If-Match` against this policy. Nothing else about a
    /// response headers policy changes: the id is the one it was created under, and a policy
    /// carries no status and no propagation deadline.
    ///
    /// `None` when the store holds no such policy, checked BEFORE the mint so a version token
    /// is never spent on a policy that is not there.
    pub fn apply_response_headers_policy_config(
        &mut self,
        id: &str,
        config: ResponseHeadersPolicyConfig,
        now: SystemTime,
    ) -> Option<&StoredResponseHeadersPolicy> {
        if !self.response_headers_policies.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.response_headers_policies.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the origin request policy `id` names, and report the
    /// record as it now stands.
    ///
    /// `UpdateOriginRequestPolicy` replaces the configuration WHOLE — "When you update an
    /// origin request policy configuration, all the fields are updated with the values
    /// provided in the request. You cannot update some fields independent of others." — so
    /// the stored configuration becomes the one submitted, down to the members the request
    /// left out; `LastModifiedTime` becomes `now` ("the date and time when the origin
    /// request policy was last modified"), and the version token is freshly minted, so the
    /// token the caller matched on no longer satisfies an `If-Match` against this policy.
    /// Nothing else about an origin request policy changes: the id is the one it was created
    /// under, and a policy carries no status and no propagation deadline.
    ///
    /// `None` when the store holds no such policy, checked BEFORE the mint so a version
    /// token is never spent on a policy that is not there.
    pub fn apply_origin_request_policy_config(
        &mut self,
        id: &str,
        config: OriginRequestPolicyConfig,
        now: SystemTime,
    ) -> Option<&StoredOriginRequestPolicy> {
        if !self.origin_request_policies.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.origin_request_policies.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the origin access control `id` names, and report the
    /// record as it now stands.
    ///
    /// `UpdateOriginAccessControl` submits a whole `OriginAccessControlConfig` as its
    /// payload, so the stored configuration becomes the one submitted, down to the members
    /// the request left out, and the version token is freshly minted — "the new version of
    /// the origin access control after it has been updated" — so the token the caller
    /// matched on no longer satisfies an `If-Match` against this control. Nothing else
    /// about a control changes: the id is the one it was created under, and a control
    /// carries no status, no last-modified time and no propagation deadline.
    ///
    /// `None` when the store holds no such control, checked BEFORE the mint so a version
    /// token is never spent on a control that is not there.
    pub fn apply_origin_access_control_config(
        &mut self,
        id: &str,
        config: OriginAccessControlConfig,
    ) -> Option<&StoredOriginAccessControl> {
        if !self.origin_access_controls.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.origin_access_controls.get_mut(id)?;
        stored.config = config;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the comment of the key value store `name` names, and report the record as
    /// it now stands.
    ///
    /// `KeyValueStore$Comment` is the only member `UpdateKeyValueStore` submits, so the
    /// stored comment becomes the one submitted, `LastModifiedTime` becomes `now` — "when
    /// the resource last changed" — and the version token is freshly minted, so the token
    /// the caller sent no longer satisfies an `If-Match` against this store.
    ///
    /// The store's identity (name, id, ARN) and its PROVISIONING progress are untouched:
    /// changing a comment neither re-provisions the store's data plane nor renames it, so
    /// [`StoredKeyValueStore::status`] and [`StoredKeyValueStore::ready_due`] stay as the
    /// create left them and the clock still owes the same transition at the same instant.
    ///
    /// `None` when the store holds no such key value store, checked BEFORE the mint so a
    /// version token is never spent on a store that is not there.
    pub fn apply_key_value_store_comment(
        &mut self,
        name: &str,
        comment: String,
        now: SystemTime,
    ) -> Option<&StoredKeyValueStore> {
        if !self.key_value_stores.contains_key(name) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.key_value_stores.get_mut(name)?;
        stored.comment = comment;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Apply an `UpdateAnycastIpList` to the list `id` names, and report the record as it
    /// now stands.
    ///
    /// `IpAddressType` ("The IP address type for the Anycast static IP list") and
    /// `IpamCidrConfigs` ("A list of IPAM CIDR configurations … for updating the Anycast
    /// static IP list") are the only members the update submits, and each is optional, so a
    /// member the request leaves out is left exactly as it was. What the update changes:
    ///
    /// * the address type becomes the one submitted, and the list's own address SLOTS are
    ///   re-reported in the families that type admits ([`anycast_ips_of`]) — the allocation
    ///   is the list's for its whole life, so nothing more is taken out of the pool and no
    ///   address can land on two lists;
    /// * the IPAM CIDR configurations become the ones submitted, each carrying the status
    ///   the SERVICE puts on a range it has just been asked for, which the clock then owes
    ///   the provisioned one;
    /// * the list re-enters the transient status, since the addresses and ranges it now
    ///   runs under have to reach the edge locations before they are in effect, and the
    ///   CLOCK owes it the terminal status again;
    /// * `LastModifiedTime` becomes `now` and the version token is freshly minted, so the
    ///   token the caller matched on no longer satisfies an `If-Match` against this list.
    ///
    /// The list's identity (id, ARN, name), its `IpCount` and its tag set are untouched: an
    /// update submits none of them.
    ///
    /// `None` when the store holds no such list, checked BEFORE the mint so a version token
    /// is never spent on a list that is not there.
    pub fn apply_anycast_ip_list_update(
        &mut self,
        id: &str,
        ip_address_type: Option<String>,
        ipam_cidr_configs: Option<Vec<IpamCidrConfig>>,
        now: SystemTime,
    ) -> Option<&StoredAnycastIpList> {
        if !self.anycast_ip_lists.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.anycast_ip_lists.get_mut(id)?;
        if let Some(ip_address_type) = ip_address_type {
            stored.ip_address_type = ip_address_type;
        }
        if let Some(configs) = ipam_cidr_configs {
            stored.ipam_cidr_configs = configs
                .into_iter()
                .map(|config| IpamCidrConfig {
                    status: Some(IPAM_CIDR_STATUS_PROVISIONING.to_string()),
                    ..config
                })
                .collect();
        }
        stored.status = ANYCAST_IP_LIST_STATUS_DEPLOYING.to_string();
        stored.deployed_due = now + PROPAGATION_DELAY;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Apply an update to the connection group `id` names, and report the record as it now
    /// stands.
    ///
    /// Every member `UpdateConnectionGroup` submits beside the group's id is optional and
    /// is documented as what the group is updated TO ("whether the connection group is
    /// enabled", "enable IPv6 for the connection group", "the ID of the Anycast static IP
    /// list"), so a member the request leaves out is left exactly as it was and one it
    /// states REPLACES what the record held — the same rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_anycast_ip_list_update`] and
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_distribution_tenant_update`]
    /// follow.
    ///
    /// The write versions the group exactly as every other change to it does:
    /// `LastModifiedTime` becomes `now`, a fresh version token is minted so the token the
    /// caller matched on no longer satisfies an `If-Match`, and the group goes back to
    /// [`STATUS_IN_PROGRESS`] with a new propagation deadline, since the settings it now
    /// runs under have to reach the edge locations before they are in effect. The group's
    /// identity — its id, its ARN, its name, its routing endpoint, its creation time and
    /// whether it is the account's default one — is untouched: an update submits none of
    /// them, and the name "can't be updated after you create the connection group".
    ///
    /// `None` when the store holds no such group, checked BEFORE the mint so a version
    /// token is never spent on a group that is not there.
    pub fn apply_connection_group_update(
        &mut self,
        id: &str,
        anycast_ip_list_id: Option<String>,
        enabled: Option<bool>,
        ipv6_enabled: Option<bool>,
        now: SystemTime,
    ) -> Option<&StoredConnectionGroup> {
        if !self.connection_groups.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.connection_groups.get_mut(id)?;
        if let Some(anycast_ip_list_id) = anycast_ip_list_id {
            stored.anycast_ip_list_id = Some(anycast_ip_list_id);
        }
        if let Some(enabled) = enabled {
            stored.enabled = enabled;
        }
        if let Some(ipv6_enabled) = ipv6_enabled {
            stored.ipv6_enabled = ipv6_enabled;
        }
        stored.status = STATUS_IN_PROGRESS.to_string();
        stored.deployed_due = now + PROPAGATION_DELAY;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the origin access identity `id` names, and report the
    /// record as it now stands.
    ///
    /// The submitted configuration becomes the stored one WHOLE, and the version token is
    /// freshly minted, so the token the caller matched on no longer satisfies an
    /// `If-Match` against this identity. Nothing else about an identity changes: the id it
    /// was created under and the Amazon S3 canonical user id minted with it are fixed for
    /// its whole life, and an identity carries neither a status nor a propagation deadline,
    /// so the clock owes it nothing.
    ///
    /// `None` when the store holds no such identity, checked BEFORE the mint so a version
    /// token is never spent on an identity that is not there.
    pub fn apply_origin_access_identity_config(
        &mut self,
        id: &str,
        config: CloudFrontOriginAccessIdentityConfig,
    ) -> Option<&StoredOriginAccessIdentity> {
        if !self.origin_access_identities.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.origin_access_identities.get_mut(id)?;
        stored.config = config;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the public key `id` names, and report the record as it
    /// now stands.
    ///
    /// `UpdatePublicKey` submits a whole `PublicKeyConfig` as its payload, so the stored
    /// configuration becomes the submitted one and the version token is freshly minted, so
    /// the token the caller matched on no longer satisfies an `If-Match` against this key.
    /// Nothing else about a key changes: the id it was uploaded under and the instant it was
    /// uploaded are fixed for its whole life, and a key carries neither a status nor a
    /// propagation deadline, so the clock owes it nothing. WHICH members an update may move
    /// is the separate rule [`immutable_public_key_change`] states, decided by the caller
    /// before it reaches here.
    ///
    /// `None` when the store holds no such key, checked BEFORE the mint so a version token
    /// is never spent on a key that is not there.
    pub fn apply_public_key_config(
        &mut self,
        id: &str,
        config: PublicKeyConfig,
    ) -> Option<&StoredPublicKey> {
        if !self.public_keys.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.public_keys.get_mut(id)?;
        // Stored RESOLVED, exactly as the create stores it ([`resolve_public_key_config`]).
        stored.config = resolve_public_key_config(config);
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the key group `id` names, and report the record as it
    /// now stands.
    ///
    /// `UpdateKeyGroup` replaces the configuration WHOLE — "When you update a key group, all
    /// the fields are updated with the values provided in the request. You cannot update some
    /// fields independent of others." — so the stored configuration becomes the one
    /// submitted, down to the members the request left out; `LastModifiedTime` becomes `now`
    /// ("the date and time when the key group was last modified"), and the version token is
    /// freshly minted, so the token the caller matched on no longer satisfies an `If-Match`
    /// against this group. Nothing else about a key group changes: the id is the one it was
    /// created under, and a group carries no status and no propagation deadline.
    ///
    /// `None` when the store holds no such key group, checked BEFORE the mint so a version
    /// token is never spent on a group that is not there.
    pub fn apply_key_group_config(
        &mut self,
        id: &str,
        config: KeyGroupConfig,
        now: SystemTime,
    ) -> Option<&StoredKeyGroup> {
        if !self.key_groups.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.key_groups.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the continuous deployment policy `id` names, and
    /// report the record as it now stands.
    ///
    /// The submitted configuration becomes the stored one WHOLE, `LastModifiedTime`
    /// becomes `now` — "the date and time the policy was last modified" — and the version
    /// token is freshly minted, so the token the caller matched on no longer satisfies an
    /// `If-Match` against this policy. Nothing else about a policy changes: the id is the
    /// one it was created under, and a policy carries neither a status nor a propagation
    /// deadline, so the clock owes it nothing.
    ///
    /// `None` when the store holds no such policy, checked BEFORE the mint so a version
    /// token is never spent on a policy that is not there.
    pub fn apply_continuous_deployment_policy_config(
        &mut self,
        id: &str,
        config: ContinuousDeploymentPolicyConfig,
        now: SystemTime,
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        if !self.continuous_deployment_policies.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.continuous_deployment_policies.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the field-level encryption profile `id` names, and
    /// report the record as it now stands.
    ///
    /// `UpdateFieldLevelEncryptionProfile` submits a whole `FieldLevelEncryptionProfileConfig`
    /// as its payload, so the stored configuration becomes the submitted one WHOLE,
    /// `LastModifiedTime` becomes `now` — "the last time the field-level encryption profile
    /// was updated" — and the version token is freshly minted, so the token the caller matched
    /// on no longer satisfies an `If-Match` against this profile. Nothing else about a profile
    /// changes: the id is the one the create minted, and a profile carries neither a status nor
    /// a propagation deadline, so the clock owes it nothing. WHICH members an update may move
    /// is the caller's rule, decided before it reaches here.
    ///
    /// `None` when the store holds no such profile, checked BEFORE the mint so a version
    /// token is never spent on a profile that is not there.
    pub fn apply_field_level_encryption_profile_config(
        &mut self,
        id: &str,
        config: FieldLevelEncryptionProfileConfig,
        now: SystemTime,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        if !self.field_level_encryption_profiles.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.field_level_encryption_profiles.get_mut(id)?;
        // Stored RESOLVED, exactly as the create stores it
        // ([`resolve_field_level_encryption_profile_config`]).
        stored.config = resolve_field_level_encryption_profile_config(config);
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the field-level encryption CONFIGURATION `id` names, and
    /// report the record as it now stands.
    ///
    /// The same write [`Self::apply_field_level_encryption_profile_config`] performs for the
    /// profile, on the configuration record: `UpdateFieldLevelEncryptionConfig` submits a whole
    /// `FieldLevelEncryptionConfig` as its payload, so the stored configuration becomes the
    /// submitted one WHOLE, `LastModifiedTime` becomes `now` — "the last time the field-level
    /// encryption configuration was changed" — and the version token is freshly minted, so the
    /// token the caller matched on no longer satisfies an `If-Match` against this
    /// configuration. Nothing else about a configuration changes: the id is the one the create
    /// minted, and a configuration carries neither a status nor a propagation deadline, so the
    /// clock owes it nothing. WHICH members an update may move is the caller's rule, decided
    /// before it reaches here.
    ///
    /// `None` when the store holds no such configuration, checked BEFORE the mint so a version
    /// token is never spent on a configuration that is not there.
    pub fn apply_field_level_encryption_config(
        &mut self,
        id: &str,
        config: FieldLevelEncryptionConfig,
        now: SystemTime,
    ) -> Option<&StoredFieldLevelEncryptionConfig> {
        if !self.field_level_encryption_configs.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.field_level_encryption_configs.get_mut(id)?;
        // Stored RESOLVED, exactly as the create stores it
        // ([`resolve_field_level_encryption_config`]).
        stored.config = resolve_field_level_encryption_config(config);
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }
}
