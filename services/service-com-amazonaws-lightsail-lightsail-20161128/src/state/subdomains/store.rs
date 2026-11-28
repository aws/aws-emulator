// SPDX-License-Identifier: CC0-1.0

//! How the store reaches its subdomains: the records a reporting operation
//! renders, the quotas a create and a reservation are bounded by, the door a
//! subdomain is created and removed through, and the resource each one routes
//! traffic to.

use super::*;

impl ComAmazonawsLightsailLightsail20161128State {
    /// The subdomain `name` names, for a reporting operation.
    pub fn subdomain(&self, name: &str) -> Option<&StoredSubdomain> {
        self.subdomains.get(name)
    }

    /// The subdomain `name` names, for an operation the spec says mutates it.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation renames a subdomain.
    pub fn subdomain_mut(&mut self, name: &str) -> Option<&mut StoredSubdomain> {
        self.subdomains.get_mut(name)
    }

    /// How many of this account's subdomains count toward [`MAX_SUBDOMAINS`]:
    /// every record but the implicit ones, which "don't count toward this
    /// quota".
    pub fn subdomains_toward_quota(&self) -> usize {
        self.subdomains
            .values()
            .filter(|subdomain| !subdomain.is_implicit())
            .count()
    }

    /// Whether the subdomain `name` is available for this account to create.
    ///
    /// A name this account already holds is not available — except an IMPLICIT
    /// one, which is exactly the name "you can … create … with the
    /// `CreateSubdomain` action". This emulator serves one account
    /// (`engine::emulator_account_id`), so no other account's subdomain or
    /// reservation can hold a name here.
    pub fn subdomain_is_available(&self, name: &str) -> bool {
        self.subdomain(name)
            .is_none_or(StoredSubdomain::is_implicit)
    }

    /// When this account's reservation of the subdomain `name` expires, and
    /// `None` when it holds none — the `reservationExpiresAt` a check reports,
    /// which "is returned only when your account holds an active reservation for
    /// the subdomain".
    ///
    /// Read under the TOP-LEVEL name a reservation is held by
    /// ([`subdomain_reserved_name`]), so a nested name reports the reservation
    /// that holds it. Every expired reservation is already gone, because the one
    /// call that reads and writes reservations releases them first
    /// (`CheckSubdomainAvailability`).
    pub fn subdomain_reservation_expires_at(&self, name: &str) -> Option<i64> {
        self.subdomain_reservations
            .get(subdomain_reserved_name(name))
            .copied()
    }

    /// How many reservations of a free subdomain this account holds now — what
    /// the call that makes one checks against [`MAX_SUBDOMAIN_RESERVATIONS`]
    /// before it writes. Every entry counted is still holding a name, because
    /// that same call releases the expired ones first.
    pub fn subdomain_reservations_held(&self) -> usize {
        self.subdomain_reservations.len()
    }

    /// Hold the subdomain `name` for this account for
    /// [`SUBDOMAIN_RESERVATION_MILLIS`] from `now_ms`, answering when the
    /// reservation expires.
    ///
    /// The reservation is held under the top-level name, because "for a nested
    /// subdomain, such as `blog.mysite`, the top-level subdomain (`mysite`) is
    /// reserved", so this map holds at most ONE entry per top-level name.
    /// Checking a subdomain the account already holds a reservation of answers
    /// that reservation unchanged and writes nothing: "Checking the subdomain
    /// again doesn't extend the reservation."
    ///
    /// How many reservations the account may hold at a time
    /// ([`MAX_SUBDOMAIN_RESERVATIONS`]) is checked by the CALL that makes one,
    /// before it asks — the growth site owns its bound, so no request can grow
    /// this map past the published quota.
    pub fn reserve_subdomain(&mut self, name: &str, now_ms: i64) -> i64 {
        let reserved = subdomain_reserved_name(name).to_string();
        if let Some(expires_at_ms) = self.subdomain_reservations.get(&reserved) {
            return *expires_at_ms;
        }
        let expires_at_ms = now_ms.saturating_add(SUBDOMAIN_RESERVATION_MILLIS);
        self.subdomain_reservations.insert(reserved, expires_at_ms);
        expires_at_ms
    }

    /// Store `subdomain`, holding its parent for the account as an IMPLICIT
    /// subdomain when the account does not hold that name yet, and releasing the
    /// reservation that was holding the name.
    ///
    /// The ONE door a subdomain is created through, so every create keeps those
    /// three rules together: "When you create a nested subdomain, such as
    /// `blog.mysite`, and you don't already have the top-level subdomain
    /// (`mysite`), Lightsail adds the top-level subdomain to your account in the
    /// `IMPLICIT` state", a reservation exists to hold a name until it is
    /// created, and a name the account held implicitly becomes the subdomain the
    /// create asked for.
    pub fn insert_subdomain(&mut self, subdomain: StoredSubdomain) {
        let created_at_ms = subdomain.created_at_ms;
        if let Some(parent) = subdomain_parent_name(&subdomain.name) {
            if !self.subdomains.contains_key(parent) {
                self.subdomains.insert(
                    parent.to_string(),
                    StoredSubdomain::implicit(parent, created_at_ms),
                );
            }
        }
        self.subdomain_reservations
            .remove(subdomain_reserved_name(&subdomain.name));
        self.subdomains.insert(subdomain.name.clone(), subdomain);
    }

    /// The nested subdomains of the subdomain `name`, in the store's own stable
    /// order (by subdomain name).
    pub fn subdomain_children(&self, name: &str) -> Vec<&StoredSubdomain> {
        self.subdomains
            .values()
            .filter(|subdomain| subdomain_parent_name(&subdomain.name) == Some(name))
            .collect()
    }

    /// The subdomain record `subdomain` as `GetSubdomains` reports it when the
    /// request named NO subdomain: with its nested subdomains, and without the
    /// tags that listing does not report.
    ///
    /// Assembling it needs the whole store, because a nested subdomain is a
    /// record of its own that names its parent — the same reason an instance's
    /// view does.
    pub fn subdomain_view_with_children(&self, subdomain: &StoredSubdomain) -> Subdomain {
        let children = self
            .subdomain_children(&subdomain.name)
            .into_iter()
            // A nested subdomain is reported as the resource it is; its own
            // children are the depth this service does not serve
            // ([`MAX_SUBDOMAIN_LABELS`]), so the walk cannot recurse further.
            .map(|child| child.view(None, false))
            .collect();
        subdomain.view(Some(children), false)
    }

    /// Remove the subdomain `name` names, answering the record that was removed
    /// and the IMPLICIT record the account keeps in its place when nested
    /// subdomains of it remain. `None` when nothing matched.
    ///
    /// "When you delete a top-level subdomain that has nested subdomains, the
    /// nested subdomains continue to work, and the top-level subdomain changes
    /// to the `IMPLICIT` state" — so the name is still the account's, with the
    /// instant it came to hold it kept, and it holds no ARN, no tags and no
    /// association any more.
    pub fn remove_subdomain(
        &mut self,
        name: &str,
    ) -> Option<(StoredSubdomain, Option<StoredSubdomain>)> {
        let removed = self.subdomains.remove(name)?;
        if self.subdomain_children(name).is_empty() {
            return Some((removed, None));
        }
        let implicit = StoredSubdomain::implicit(name, removed.created_at_ms);
        self.subdomains.insert(name.to_string(), implicit.clone());
        Some((removed, Some(implicit)))
    }

    /// The subdomain the resource `resource_arn` is reachable at, or is being
    /// made reachable at, and `None` when no subdomain of this account routes
    /// traffic to it.
    ///
    /// A resource "can be associated with only one subdomain at a time", so the
    /// answer is that one subdomain.
    pub fn subdomain_routing_to(&self, resource_arn: &str) -> Option<&StoredSubdomain> {
        self.subdomains
            .values()
            .find(|subdomain| subdomain.routes_to(resource_arn))
    }

    /// The distribution `distribution` as the modelled `LightsailDistribution`,
    /// with the Lightsail subdomain it is reachable at.
    ///
    /// The one way a reporting operation renders a distribution: the
    /// association lives on the SUBDOMAIN record (the subdomain names the
    /// resource it is associated with), so assembling the view needs the whole
    /// store and not just the distribution.
    pub fn distribution_view(&self, distribution: &StoredDistribution) -> LightsailDistribution {
        distribution.view(
            self.subdomain_routing_to(&distribution.arn)
                .and_then(|subdomain| subdomain.resource_subdomain_view(&distribution.arn)),
        )
    }
}
