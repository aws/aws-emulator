// SPDX-License-Identifier: CC0-1.0

//! The LISTENER half of this store: the record one listener is persisted as, and the
//! modelled shapes a write's reply and a read answer it with.

use super::*;

/// A persisted listener.
#[derive(Debug, Clone, Default)]
pub struct ListenerRecord {
    pub arn: String,
    pub load_balancer_arn: String,
    pub port: Option<i32>,
    pub protocol: Option<String>,
    pub ssl_policy: Option<String>,
    /// The listener's DEFAULT certificate — "[HTTPS and TLS listeners] The default
    /// certificate for the listener. You must provide exactly one certificate." — which
    /// is the one `CreateListener` and `ModifyListener` set and the only one a `Listener`
    /// shape reports.
    pub certificate_arns: Vec<String>,
    /// The listener's CERTIFICATE LIST: the additional certificates
    /// `AddListenerCertificates` puts on an HTTPS or TLS listener for SNI, in the order
    /// they were added. Separate from the default certificate above, because the two are
    /// managed by different operations and `DescribeListenerCertificates` reports them
    /// apart (`Certificate$IsDefault`).
    pub list_certificate_arns: Vec<String>,
    pub default_actions: Vec<Action>,
    pub alpn_policy: Vec<String>,
    /// The MUTUAL AUTHENTICATION the listener was configured with, as the create or the
    /// modification that set it carried it, and `None` for a listener nothing configured it
    /// on. A listener outside [`MUTUAL_AUTHENTICATION_PROTOCOLS`] carries none at all.
    pub mutual_authentication: Option<MutualAuthenticationAttributes>,
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    /// The resource id of this listener's DEFAULT rule, minted with the listener: every
    /// listener has one, it carries the listener's default actions, and it is what
    /// `DescribeRules` reports for a listener nothing else has added a rule to.
    pub default_rule_id: String,
    /// The rules a caller added to this listener with `CreateRule`, in the order they
    /// were added; a read orders them by priority
    /// ([`ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State::rules_of`]).
    pub rules: Vec<RuleRecord>,
}

impl ListenerRecord {
    /// The settings `CreateListener` is idempotent over. Actions are compared by the
    /// members this slice models — their type and the target group they forward to —
    /// because the whole action tree is stored but not all of it is comparable.
    pub fn has_settings_of(&self, other: &ListenerRecord) -> bool {
        self.protocol == other.protocol
            && self.ssl_policy == other.ssl_policy
            && sorted(&self.certificate_arns) == sorted(&other.certificate_arns)
            && self.alpn_policy == other.alpn_policy
            // The client certificate handling is one of the listener's settings, so two
            // listeners differing only in it are not "the same settings": they are compared
            // by what each is EFFECTIVELY under, so a request naming the mode the service
            // defaults is the idempotent repeat of one that named none.
            && MutualAuthenticationSettings::of(self.effective_mutual_authentication().as_ref())
                == MutualAuthenticationSettings::of(
                    other.effective_mutual_authentication().as_ref(),
                )
            && action_summary(&self.default_actions) == action_summary(&other.default_actions)
    }

    pub fn to_shape(&self) -> Listener {
        Listener {
            listener_arn: Some(self.arn.clone()),
            load_balancer_arn: Some(self.load_balancer_arn.clone()),
            port: self.port,
            protocol: self.protocol.clone(),
            certificates: Some(
                self.certificate_arns
                    .iter()
                    .map(|arn| Certificate {
                        certificate_arn: Some(arn.clone()),
                        // `Certificate$IsDefault`: "This value is not included in the
                        // output when describing a listener, but is included when
                        // describing listener certificates."
                        is_default: None,
                    })
                    .collect(),
            ),
            ssl_policy: self.ssl_policy.clone(),
            default_actions: Some(reported_actions(&self.default_actions)),
            alpn_policy: self.reported_alpn_policy(),
            // A WRITE answers with the listener it left behind as the request configured
            // it, so the member is there when a request set it and absent when none did;
            // a READ answers the listener's effective configuration
            // ([`Self::described_shape`]). The record holds a configuration only for a
            // listener the member belongs to — the write that sets it refuses it on any
            // other, and the modification that moves a listener off that protocol clears
            // it — so there is nothing to mask here.
            mutual_authentication: self.mutual_authentication.clone(),
        }
    }

    /// The ALPN policy every reply reporting this listener carries — the member itself for
    /// a listener that HOLDS a policy, and nothing at all for one that holds none.
    ///
    /// `Listener$AlpnPolicy` is "[TLS listener] The name of the Application-Layer Protocol
    /// Negotiation (ALPN) policy", and the write that sets it takes "one policy name"
    /// (`CreateListenerInput$AlpnPolicy`), so the member names the policy a listener is
    /// configured with. Its list shape declares no empty form and the member no default, so
    /// a listener with no ALPN policy — one of a protocol the member is not bracketed to,
    /// and a TLS listener no request named one on — has no such member in the reply rather
    /// than an empty one: an empty list reads as "a policy is configured, holding no name",
    /// which is a different answer from "no policy is configured" and the one a client
    /// branching on whether ALPN is configured, or round-tripping a reported listener back
    /// into the modification, acts on.
    fn reported_alpn_policy(&self) -> Option<Vec<String>> {
        (!self.alpn_policy.is_empty()).then(|| self.alpn_policy.clone())
    }

    /// This listener as a READ of it reports it: [`Self::to_shape`] carrying the mutual
    /// authentication the listener EFFECTIVELY handles client certificates under
    /// ([`Self::effective_mutual_authentication`]).
    pub fn described_shape(&self) -> Listener {
        Listener {
            mutual_authentication: self.effective_mutual_authentication(),
            ..self.to_shape()
        }
    }

    /// The mutual authentication this listener EFFECTIVELY handles client certificates
    /// under: the configuration it was given, with every setting the configuration left
    /// out carried at the value the service carries it at — the mode a listener nothing
    /// configured mTLS on handles client certificates under being
    /// [`DEFAULT_MUTUAL_AUTHENTICATION_MODE`].
    ///
    /// A listener the member does not belong to has none at all
    /// ([`handles_client_certificates`]).
    pub fn effective_mutual_authentication(&self) -> Option<MutualAuthenticationAttributes> {
        if !self.handles_client_certificates() {
            return None;
        }
        let mut effective = self.mutual_authentication.clone().unwrap_or_default();
        if effective.mode.is_none() {
            effective.mode = Some(DEFAULT_MUTUAL_AUTHENTICATION_MODE.to_string());
        }
        Some(effective)
    }

    /// Whether this listener carries a mutual authentication configuration at all.
    pub fn handles_client_certificates(&self) -> bool {
        handles_client_certificates(self.protocol.as_deref())
    }

    /// Whether this listener terminates TLS, and so carries certificates at all.
    pub fn terminates_tls(&self) -> bool {
        terminates_tls(self.protocol.as_deref())
    }

    /// What `DescribeListenerCertificates` reports: the default certificate, then the
    /// certificate list.
    ///
    /// The operation is documented as describing "the default certificate and the
    /// certificate list for the specified HTTPS or TLS listener", and `IsDefault`
    /// distinguishes them, so a certificate that is in both appears twice — once as the
    /// default and once as a list entry — rather than being collapsed.
    pub fn certificate_shapes(&self) -> Vec<Certificate> {
        self.certificate_arns
            .iter()
            .map(|arn| Certificate {
                certificate_arn: Some(arn.clone()),
                is_default: Some(true),
            })
            .chain(self.list_certificate_arns.iter().map(|arn| Certificate {
                certificate_arn: Some(arn.clone()),
                is_default: Some(false),
            }))
            .collect()
    }

    /// The ARN a rule of this listener carrying the resource id `rule_id` has.
    ///
    /// A listener rule's ARN is its listener's with the resource type `listener-rule` and
    /// the rule's own id appended — the segments this service's ARNs carry
    /// (`listener/<type>/<name>/<lb-id>/<id>` and
    /// `listener-rule/<type>/<name>/<lb-id>/<listener-id>/<id>`), so it is derived from
    /// the stored listener ARN rather than rebuilt from parts. Every rule of a listener,
    /// the default one included, is named through here, so one place decides the form.
    pub fn rule_arn(&self, rule_id: &str) -> String {
        format!(
            "{}/{rule_id}",
            self.arn.replacen(":listener/", ":listener-rule/", 1)
        )
    }

    /// The ARN of this listener's default rule.
    pub fn default_rule_arn(&self) -> String {
        self.rule_arn(&self.default_rule_id)
    }

    /// This listener's default rule: the rule that forwards everything the listener's
    /// own rules do not match, which is why its actions ARE the listener's default
    /// actions and it matches on no condition.
    pub fn default_rule(&self) -> Rule {
        Rule {
            rule_arn: Some(self.default_rule_arn()),
            // The one priority AWS reports for a default rule; every other rule's is a
            // number.
            priority: Some("default".to_string()),
            conditions: Some(Vec::new()),
            actions: Some(reported_actions(&self.default_actions)),
            is_default: Some(true),
            // A default rule carries no transform, and reports that as the empty list
            // exactly as every other rule does ([`RuleRecord::to_shape`]).
            transforms: Some(Vec::new()),
        }
    }

    /// This listener's WHOLE attribute collection, for a listener of a load balancer in
    /// `family`: every attribute the catalog carries for such a listener, each holding
    /// the value stored for it or the value it is carried at — which is what both a read
    /// and a write of these attributes answers with — and nothing for an attribute this
    /// family's listeners do not carry or that the collection does not carry.
    ///
    /// In [`LISTENER_ATTRIBUTES`] order, which is the order the service reports them in.
    pub fn effective_attributes(&self, family: LoadBalancerFamily) -> Vec<(String, String)> {
        LISTENER_ATTRIBUTES
            .iter()
            .filter(|spec| spec.applies_to(family))
            .filter_map(|spec| {
                let value = self
                    .attributes
                    .get(spec.key)
                    .cloned()
                    .or_else(|| spec.default.map(str::to_string))?;
                Some((spec.key.to_string(), value))
            })
            .collect()
    }

    /// [`Self::effective_attributes`] as the modelled shapes a describe answers with.
    pub fn attribute_shapes(&self, family: LoadBalancerFamily) -> Vec<ListenerAttribute> {
        self.effective_attributes(family)
            .into_iter()
            .map(|(key, value)| ListenerAttribute {
                key: Some(key),
                value: Some(value),
            })
            .collect()
    }

    /// Every target group an ACTION of this listener forwards to: its default actions,
    /// and the actions of every rule a caller added to it.
    ///
    /// One place answers "what does this listener route to", because two questions are
    /// decided by it — which load balancers a target group reports under
    /// `LoadBalancerArns`, and whether a target group is "referenced by any actions" and
    /// so cannot be deleted.
    pub fn forwarded_target_group_arns(&self) -> Vec<String> {
        let mut arns = action_target_group_arns(&self.default_actions);
        for rule in &self.rules {
            arns.extend(action_target_group_arns(&rule.actions));
        }
        arns
    }
}

/// The comparable projection of a listener's mutual authentication configuration: EVERY
/// setting the configuration carries, read off the modelled shape so two configurations can
/// be judged the same or different ([`ListenerRecord::has_settings_of`]).
///
/// The modelled shapes are rendered from the model and carry no comparison of their own, so
/// a configuration is compared through this projection exactly as an action list is compared
/// through its own ([`action_summary`]). One setting per member
/// `MutualAuthenticationAttributes` declares
/// (`smithy_query(query="shape",shape="MutualAuthenticationAttributes")`), so a listener
/// differing in any one of them — the mode, the trust store, whether expired client
/// certificates are ignored, the shared trust store association status, whether the trust
/// store's CA names are advertised, or the client certificate identity filters it admits
/// connections by — is not a listener with the same settings. The filters are projected as
/// a SET, each by the four members `ClientCertificateIdentityFilter` declares and all four
/// of which it requires: "[t]he load balancer admits a connection only if the client
/// certificate matches at least one filter", so the same filters in another order admit
/// exactly the same connections and are the same setting — the reason the default
/// certificates are compared sorted too.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct MutualAuthenticationSettings {
    mode: Option<String>,
    trust_store_arn: Option<String>,
    ignore_client_certificate_expiry: Option<bool>,
    trust_store_association_status: Option<String>,
    advertise_trust_store_ca_names: Option<String>,
    client_certificate_identity_filters: Vec<(String, String, String, String)>,
}

impl MutualAuthenticationSettings {
    /// The settings `mutual_authentication` holds, and `None` for a listener that carries
    /// no mutual authentication configuration at all — which is itself a setting two
    /// listeners can differ in.
    fn of(mutual_authentication: Option<&MutualAuthenticationAttributes>) -> Option<Self> {
        let attributes = mutual_authentication?;
        let mut client_certificate_identity_filters: Vec<(String, String, String, String)> =
            attributes
                .client_certificate_identity_filters
                .iter()
                .flatten()
                .map(|filter| {
                    (
                        filter.field.clone(),
                        filter.action.clone(),
                        filter.match_type.clone(),
                        filter.value.clone(),
                    )
                })
                .collect();
        client_certificate_identity_filters.sort();
        Some(Self {
            mode: attributes.mode.clone(),
            trust_store_arn: attributes.trust_store_arn.clone(),
            ignore_client_certificate_expiry: attributes.ignore_client_certificate_expiry,
            trust_store_association_status: attributes.trust_store_association_status.clone(),
            advertise_trust_store_ca_names: attributes.advertise_trust_store_ca_names.clone(),
            client_certificate_identity_filters,
        })
    }
}
