// SPDX-License-Identifier: CC0-1.0

//! The ACCESS POINTS that front this store's buckets: the named data paths a request can
//! reach a bucket's objects through, and the policy each of them is judged by.
//!
//! "Amazon S3 access points are named network endpoints that are attached to buckets that
//! you can use to perform S3 object operations" (Amazon S3 User Guide, *Managing data
//! access with Amazon S3 access points*). An access point is CREATED through S3 Control
//! (`CreateAccessPoint`), and it is USED here: a data-plane request addressed to the access
//! point reaches the bucket it fronts, and is authorized against the access point's own
//! policy as well as the identity's and the bucket's.
//!
//! Both halves need the same record, so it lives in ONE place — this store, which is the
//! store the data plane serves from. S3 Control writes it on the calls that create an
//! access point, remove it, and put or delete its policy, exactly as it writes a bucket's
//! tag set into this store rather than keeping a copy of its own
//! (`service-com-amazonaws-s3control`'s `change_bucket_tags`). A second copy in that
//! service's store would mean the access point a client created and the access point this
//! data plane resolves were different records.
//!
//! WHICH addresses reach one access point is the published addressing of its endpoint,
//! and all of them resolve to the same record ([`AccessPointRoute::addresses`]):
//!
//! * its ARN, which is what the `Bucket` field of a data-plane call takes ("you can use the
//!   access point ARN in place of a bucket name");
//! * its ALIAS, which "you can use … instead of an access point ARN for any access point
//!   data plane operation" and which is shaped like a bucket name;
//! * the first label of its endpoint host, `<access-point-name>-<account-id>`, which is the
//!   form every AWS SDK rewrites an access point ARN into before it sends the request
//!   (`https://<name>-<account-id>.s3-accesspoint.<region>.amazonaws.com/<key>`) — so a
//!   client that addresses an access point by ARN arrives here addressing it by this label.
//!
//! A spelling this store holds a BUCKET under is that bucket's and reaches no access point
//! ([`ComAmazonawsS3Amazons3State::addressed_access_point`]) — an alias and an endpoint label
//! are both shaped like a bucket name, so one spelling never reaches two resources.
//!
//! The REQUEST-SCOPED note at the bottom of this file is how the authorization step learns
//! that the request it is judging came through an access point, recorded where the address
//! is resolved (`service::wire`) and read where the decision is taken
//! (`service::request_authorization`). It is a note about the request being served and not
//! state, under the same discipline as [`super::addressed_bucket`]'s: every request this
//! service deserializes records one — `None` for a request that addresses no access point —
//! so the note a decision reads is its own request's.

use std::cell::RefCell;

use super::*;

/// How many access points this store holds routes for at once.
///
/// An EMULATOR ceiling, not an AWS quota (the service that creates them enforces its own,
/// `ACCESS_POINT_QUOTA`): these records live in memory for the lifetime of the process, so
/// a caller creating access points in a loop must not be able to grow this map without
/// end. It is set above that quota, so a create the owning service admits is always
/// recorded here, and past it a further route is not recorded rather than evicting one a
/// live workflow is using.
pub const MAX_ACCESS_POINT_ROUTES: usize = 10_000;

/// The separator between an access point's name and the account id in the first label of
/// its endpoint host (`<name>-<account-id>.s3-accesspoint.<region>.amazonaws.com`).
const ENDPOINT_LABEL_SEPARATOR: char = '-';

/// What the DATA PLANE needs to know about one access point: which bucket it fronts, the
/// addresses that reach it, and the policy in force on it.
///
/// Not the whole resource — the creation date, the tags, the network origin and the
/// public-access settings are reported by the service that owns the resource and are kept
/// there. This is the part a request addressed to the access point is served and judged by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessPointRoute {
    /// The access point's ARN, which is the resource an access point policy is written
    /// about and the one a refusal about this path names.
    pub arn: String,
    /// The access point's name, within its account.
    pub name: String,
    /// The account that owns the access point.
    pub account_id: String,
    /// The access point's alias — a bucket-shaped name that addresses it.
    pub alias: String,
    /// The bucket the access point fronts: the bucket every request addressed to this
    /// access point reaches.
    pub bucket: String,
    /// The VPC this access point is CONFINED to, and `None` for one that answers requests
    /// from anywhere — the difference between a network origin of `VPC` and one of
    /// `Internet`.
    ///
    /// "If this field is specified, this access point will only allow connections from the
    /// specified VPC ID" (`VpcConfiguration$VpcId`), which is a restriction on the DATA PATH
    /// and not a document: it decides every request addressed to the access point, so the
    /// store that serves that path is where it is held, beside the policy it overrides.
    pub vpc_id: Option<String>,
    /// The document the last `PutAccessPointPolicy` stored, byte for byte, and `None` for
    /// an access point carrying no policy — which is what it carries when it is created and
    /// again once `DeleteAccessPointPolicy` removes one.
    pub policy: Option<String>,
}

impl AccessPointRoute {
    /// Every address OTHER THAN its ARN that reaches this access point — its alias and the
    /// first label of its endpoint host (see the module docs for the three and where each
    /// comes from).
    ///
    /// The ARN is the key the route itself is held under, so it needs no index entry; these
    /// two are what [`ComAmazonawsS3Amazons3State::record_access_point`] indexes, which is
    /// the one place the set is stated.
    pub fn addresses(&self) -> [String; 2] {
        [self.alias.clone(), self.endpoint_label()]
    }

    /// Whether this access point answers only requests that arrive from the VPC it is bound
    /// to — its network origin being `VPC` rather than `Internet` (see [`Self::vpc_id`]).
    pub fn confined_to_a_vpc(&self) -> bool {
        self.vpc_id.is_some()
    }

    /// The first label of this access point's endpoint host: `<name>-<account-id>`.
    pub fn endpoint_label(&self) -> String {
        format!("{}{ENDPOINT_LABEL_SEPARATOR}{}", self.name, self.account_id)
    }

    /// The resource an IAM decision about `resource` is taken over when the request came
    /// through this access point.
    ///
    /// An access point policy is written about the access point, not about the bucket: "the
    /// resource is the access point ARN" for a bucket-level action, and
    /// `<access-point-arn>/object/<key>` for an object-level one (Amazon S3 User Guide,
    /// *Access point policy examples*). `resource` is the bucket-scoped ARN the operation
    /// asked about — `arn:aws:s3:::<bucket>` or `arn:aws:s3:::<bucket>/<key>` — so the key
    /// is read off it and nothing else has to be threaded through the decision.
    ///
    /// A resource this access point does NOT front is left exactly as it is: an operation
    /// that asks about more than one resource (the source a copy reads) asks about one the
    /// access point says nothing about, and stating that one as the access point's would let
    /// its policy decide a request about another bucket's object.
    pub fn resource_for(&self, resource: &str) -> String {
        let fronted = bucket_arn(&self.bucket);
        match resource.strip_prefix(&fronted) {
            Some(key) if key.starts_with('/') => format!("{}/object{key}", self.arn),
            // The bucket itself: the access point ARN is what a decision about the path is
            // taken over.
            Some("") => self.arn.clone(),
            _ => resource.to_string(),
        }
    }
}

impl ComAmazonawsS3Amazons3State {
    /// The access point `address` reaches, or `None` when no access point this store holds
    /// a route for is addressed by it.
    ///
    /// Read-only. Asked of the address EVERY request carries, which is why the ARN — the
    /// key this map is held by — is answered by one lookup, and every other address through
    /// the INDEX of the addresses that reach one, so the address every request carries costs
    /// one lookup rather than a scan.
    pub fn addressed_access_point(&self, address: &str) -> Option<&AccessPointRoute> {
        // A spelling this store holds a BUCKET under addresses that bucket: the data plane
        // resolves a bucket before an access point ([`super::addressed_bucket_in`]), so a
        // name that is both reaches the bucket here too rather than two resources at once.
        if self.bucket(address).is_some() {
            return None;
        }
        if let Some(route) = self.access_points.get(address) {
            return Some(route);
        }
        let arn = self.access_point_addresses.get(address)?;
        self.access_points.get(arn)
    }

    /// Record `route` as the access point it names, replacing any route this store held
    /// under the same ARN.
    ///
    /// Called by the service that CREATES an access point, so the data plane resolves the
    /// access point a client just created. Answers whether the route was recorded: past
    /// [`MAX_ACCESS_POINT_ROUTES`] it is not, and nothing a live workflow is using is
    /// dropped to make room.
    pub fn record_access_point(&mut self, route: AccessPointRoute) -> bool {
        if self.access_points.len() >= MAX_ACCESS_POINT_ROUTES
            && !self.access_points.contains_key(&route.arn)
        {
            return false;
        }
        // The index is rebuilt for this ARN first, so no address is left resolving to a route
        // that is no longer the one it reaches.
        self.forget_access_point_addresses(&route.arn);
        for address in route.addresses() {
            self.access_point_addresses
                .insert(address, route.arn.clone());
        }
        self.access_points.insert(route.arn.clone(), route);
        true
    }

    /// Forget the access point whose ARN is `arn`, reporting whether this store held one.
    ///
    /// Called by the service that DELETES an access point: the addresses that reached it
    /// stop resolving at that moment, exactly as the resource stops being reported.
    pub fn forget_access_point(&mut self, arn: &str) -> bool {
        self.forget_access_point_addresses(arn);
        self.access_points.remove(arn).is_some()
    }

    /// Drop every indexed address that reaches the access point whose ARN is `arn`.
    ///
    /// The addresses are the ROUTE's own ([`AccessPointRoute::addresses`]), so they are read
    /// off the record this store holds under that ARN and removed by key — the index is never
    /// scanned, which is what keeps recording and forgetting as cheap as the lookup they
    /// serve.
    fn forget_access_point_addresses(&mut self, arn: &str) {
        let Some(held) = self.access_points.get(arn) else {
            return;
        };
        for address in held.addresses() {
            self.access_point_addresses.remove(&address);
        }
    }

    /// Put `policy` on the access point whose ARN is `arn` — the document a
    /// `PutAccessPointPolicy` stored, or `None` for one a `DeleteAccessPointPolicy`
    /// removed — reporting whether this store held that access point.
    pub fn set_access_point_policy(&mut self, arn: &str, policy: Option<String>) -> bool {
        match self.access_points.get_mut(arn) {
            Some(route) => {
                route.policy = policy;
                true
            }
            None => false,
        }
    }

    /// How many access point routes this store holds.
    pub fn access_point_routes(&self) -> usize {
        self.access_points.len()
    }

    /// How many ADDRESSES other than an ARN this store resolves an access point by — the
    /// index [`Self::addressed_access_point`] reads, which holds exactly the addresses of the
    /// routes it holds.
    pub fn access_point_addresses(&self) -> usize {
        self.access_point_addresses.len()
    }
}

/// The access point `address` reaches, read out of the store in `engine`.
///
/// For the seams that resolve an ADDRESS and hold no guard of their own
/// (`crate::service`'s addressing and the wire layer's): it takes this store's guard, copies
/// the route out and hands back an owned value, so nothing holds the guard across another
/// store's.
pub fn addressed_access_point_in<Eng: Send + Sync + 'static>(
    engine: &Eng,
    address: &str,
) -> Option<AccessPointRoute> {
    crate::state::service_state(engine)
        .and_then(|state| state.lock().addressed_access_point(address).cloned())
}

thread_local! {
    /// The access point the request this thread is serving was addressed THROUGH, for the
    /// length of that one request. See the module docs for the discipline.
    static ADDRESSED: RefCell<Option<AccessPointRoute>> = const { RefCell::new(None) };
}

/// Record `route` as the access point the request this thread is serving was addressed
/// through, `None` for a request addressed to a bucket directly.
///
/// Called for EVERY request this service deserializes, so the note a decision reads is
/// always its own request's: a `None` overwrites whatever the previous request on this
/// thread recorded rather than leaving it to be read by the next decision.
pub(crate) fn remember(route: Option<AccessPointRoute>) {
    ADDRESSED.with(|note| {
        *note.borrow_mut() = route;
    });
}

/// The access point recorded for the request being served, READ rather than taken: one
/// request's decision may be taken more than once (an operation that authorizes an action
/// per resource), and the note is replaced by the next request's own.
pub(crate) fn addressed() -> Option<AccessPointRoute> {
    ADDRESSED.with(|note| note.borrow().clone())
}

/// The access point the request being served came through, when that access point answers
/// only requests from a VPC this request did not arrive from — the one state in which the
/// access point refuses the request whatever the documents in force say.
///
/// A request reaching this emulator reached its one internet-facing data plane, and no VPC
/// carries traffic into it, so a request that arrives here arrived from outside every VPC:
/// an access point confined to one ([`AccessPointRoute::confined_to_a_vpc`]) does not answer
/// it. ONE place, so the decision taken for a CREDENTIALED caller
/// (`crate::service::request_authorization::denial`) and the one taken for the ANONYMOUS
/// caller ([`super::Bucket::anonymous_object_read_refusal`]) answer it alike.
pub(crate) fn addressed_outside_its_vpc() -> Option<AccessPointRoute> {
    addressed().filter(AccessPointRoute::confined_to_a_vpc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One access point over `bucket`, as S3 Control records one. The ARN is the form
    /// `crate::state::is_s3_access_point_arn` reads:
    /// `arn:aws:s3:<region>:<account-id>:accesspoint/<name>`.
    fn route(name: &str, bucket: &str) -> AccessPointRoute {
        AccessPointRoute {
            arn: format!("arn:aws:s3:us-east-1:000000000000:accesspoint/{name}"),
            name: name.to_string(),
            account_id: "000000000000".to_string(),
            alias: format!("{name}-0123456789abcdef-s3alias"),
            bucket: bucket.to_string(),
            vpc_id: None,
            policy: None,
        }
    }

    #[test]
    fn every_published_address_of_an_access_point_reaches_it() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        let held = route("reports-ap", "reports-bucket");
        store.record_access_point(held.clone());
        // Each row is one published address of the same access point (see the module docs).
        let rows: [(&str, String); 3] = [
            ("its ARN", held.arn.clone()),
            ("its alias", held.alias.clone()),
            ("its endpoint host label", held.endpoint_label()),
        ];
        for (case, address) in rows {
            assert_eq!(
                store.addressed_access_point(&address),
                Some(&held),
                "case {case}: {address:?} must reach the access point, got {:?}",
                store.addressed_access_point(&address)
            );
        }
        // And an address that is the fronted BUCKET's own name, another access point's, or
        // another bucket's ARN reaches it not at all.
        for address in [
            "reports-bucket",
            "other-ap-000000000000",
            "other-ap-0123456789abcdef-s3alias",
            "arn:aws:s3:::reports-bucket",
        ] {
            assert!(
                store.addressed_access_point(address).is_none(),
                "case {address:?}: must reach no access point, got {:?}",
                store.addressed_access_point(address)
            );
        }
    }

    #[test]
    fn a_decision_through_an_access_point_is_taken_over_the_access_points_own_resource() {
        let held = route("reports-ap", "reports-bucket");
        // (case, the bucket-scoped resource the operation asked about, the resource the
        // access point path is judged over)
        let rows: [(&str, String, String); 4] = [
            (
                "an object of the bucket it fronts",
                format!("{}/data/report.txt", bucket_arn("reports-bucket")),
                format!("{}/object/data/report.txt", held.arn),
            ),
            (
                "a key carrying no prefix",
                format!("{}/report.txt", bucket_arn("reports-bucket")),
                format!("{}/object/report.txt", held.arn),
            ),
            (
                "the bucket itself",
                bucket_arn("reports-bucket"),
                held.arn.clone(),
            ),
            (
                "another bucket's object",
                format!("{}/data/report.txt", bucket_arn("other-bucket")),
                format!("{}/data/report.txt", bucket_arn("other-bucket")),
            ),
        ];
        for (case, asked, expected) in rows {
            assert_eq!(
                held.resource_for(&asked),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                held.resource_for(&asked)
            );
        }
    }

    #[test]
    fn a_recorded_route_resolves_until_it_is_forgotten() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        let held = route("reports-ap", "reports-bucket");
        assert!(
            store.record_access_point(held.clone()),
            "the route must be recorded, got a refusal"
        );
        assert_eq!(
            store.addressed_access_point(&held.endpoint_label()),
            Some(&held),
            "the endpoint label must resolve the recorded route, got {:?}",
            store.addressed_access_point(&held.endpoint_label())
        );

        // A policy put on it is what the data plane judges the path by.
        assert!(
            store.set_access_point_policy(&held.arn, Some("{}".to_string())),
            "the policy must be put on the recorded route"
        );
        assert_eq!(
            store
                .addressed_access_point(&held.arn)
                .and_then(|route| route.policy.clone()),
            Some("{}".to_string()),
            "the stored policy must be the one the decision reads"
        );

        // And once the access point is gone, none of its addresses resolves, and the index
        // holds nothing for it either.
        assert!(
            store.forget_access_point(&held.arn),
            "the route must be forgotten, got no record"
        );
        for address in [held.arn.clone(), held.alias.clone(), held.endpoint_label()] {
            assert!(
                store.addressed_access_point(&address).is_none(),
                "case {address:?}: a forgotten access point must resolve nothing, got {:?}",
                store.addressed_access_point(&address)
            );
        }
        assert_eq!(
            (store.access_point_routes(), store.access_point_addresses()),
            (0, 0),
            "a forgotten access point must leave neither a route nor an address behind, got \
             {} route(s) and {} address(es)",
            store.access_point_routes(),
            store.access_point_addresses()
        );
    }

    #[test]
    fn a_route_recorded_again_leaves_no_address_reaching_what_it_replaced() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        let first = route("reports-ap", "reports-bucket");
        store.record_access_point(first.clone());
        // The same access point, re-recorded over another bucket and under another alias:
        // what it used to be reached by must not resolve any more.
        let moved = AccessPointRoute {
            alias: "reports-ap-9999999999999999999999999-s3alias".to_string(),
            bucket: "archive-bucket".to_string(),
            ..first.clone()
        };
        store.record_access_point(moved.clone());
        assert_eq!(
            store
                .addressed_access_point(&moved.alias)
                .map(|route| route.bucket.clone()),
            Some(moved.bucket.clone()),
            "the current alias must reach the bucket the route now fronts, got {:?}",
            store.addressed_access_point(&moved.alias)
        );
        assert!(
            store.addressed_access_point(&first.alias).is_none(),
            "the alias it was recorded under before must reach nothing, got {:?}",
            store.addressed_access_point(&first.alias)
        );
        assert_eq!(
            (store.access_point_routes(), store.access_point_addresses()),
            (1, 2),
            "one access point must hold one route and its two non-ARN addresses, got {} and \
             {}",
            store.access_point_routes(),
            store.access_point_addresses()
        );
    }

    #[test]
    fn the_ceiling_bounds_what_one_caller_can_record() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        for index in 0..MAX_ACCESS_POINT_ROUTES {
            assert!(
                store.record_access_point(route(&format!("ap-{index:06}"), "reports-bucket")),
                "case route {index}: expected it recorded under the ceiling"
            );
        }
        assert!(
            !store.record_access_point(route("ap-past-the-ceiling", "reports-bucket")),
            "case one past the ceiling: expected the route refused, got {} recorded",
            store.access_point_routes()
        );
        // Nothing a workflow is using was dropped to make room.
        assert_eq!(
            store.access_point_routes(),
            MAX_ACCESS_POINT_ROUTES,
            "case one past the ceiling: expected the ceiling held, got {}",
            store.access_point_routes()
        );
        let first = route("ap-000000", "reports-bucket");
        assert_eq!(
            store.addressed_access_point(&first.arn),
            Some(&first),
            "case one past the ceiling: expected the oldest route kept"
        );
    }

    #[test]
    fn a_name_this_store_holds_a_bucket_under_reaches_that_bucket_and_no_access_point() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        let held = route("reports-ap", "reports-bucket");
        store.record_access_point(held.clone());
        // An access point's ENDPOINT LABEL is shaped like a bucket name, so a bucket can be
        // held under it. Before such a bucket exists, the label reaches the access point.
        let shared = held.endpoint_label();
        assert_eq!(
            store.addressed_access_point(&shared),
            Some(&held),
            "case no bucket of that name: {shared:?} must reach the access point, got {:?}",
            store.addressed_access_point(&shared)
        );
        store.create_bucket(&shared, "us-east-1");
        assert!(
            store.addressed_access_point(&shared).is_none(),
            "case a bucket of that name: {shared:?} must reach the bucket and no access point, \
             got {:?}",
            store.addressed_access_point(&shared)
        );
        // Every other address of the same access point still reaches it.
        for address in [held.arn.clone(), held.alias.clone()] {
            assert_eq!(
                store.addressed_access_point(&address),
                Some(&held),
                "case {address:?}: must still reach the access point, got {:?}",
                store.addressed_access_point(&address)
            );
        }
    }

    #[test]
    fn only_a_route_confined_to_a_vpc_is_one_the_request_arrived_outside_of() {
        // (case, the VPC the access point is bound to, whether a request through it arrived
        // outside that VPC) — this emulator's data plane carries no VPC traffic, so every
        // request through a VPC-confined access point did.
        let rows: [(&str, Option<&str>, bool); 3] = [
            ("an Internet-origin access point", None, false),
            (
                "a VPC-origin access point",
                Some("vpc-0a1b2c3d4e5f67890"),
                true,
            ),
            ("another VPC's access point", Some("vpc-00000000000000001"), true),
        ];
        for (case, vpc_id, outside) in rows {
            let held = AccessPointRoute {
                vpc_id: vpc_id.map(str::to_string),
                ..route("reports-ap", "reports-bucket")
            };
            remember(Some(held.clone()));
            assert_eq!(
                addressed_outside_its_vpc().is_some(),
                outside,
                "case {case}: expected outside-its-VPC {outside}, got {:?}",
                addressed_outside_its_vpc()
            );
            assert_eq!(
                held.confined_to_a_vpc(),
                vpc_id.is_some(),
                "case {case}: expected confined {}, got {}",
                vpc_id.is_some(),
                held.confined_to_a_vpc()
            );
        }
        // And a request that came through no access point is outside nothing.
        remember(None);
        assert!(
            addressed_outside_its_vpc().is_none(),
            "case no access point: expected nothing, got {:?}",
            addressed_outside_its_vpc()
        );
    }

    #[test]
    fn a_request_records_its_own_access_point_and_nothing_else() {
        let held = route("reports-ap", "reports-bucket");
        remember(Some(held.clone()));
        assert_eq!(
            addressed(),
            Some(held),
            "the recorded access point must be the one a decision reads"
        );
        // A request addressed to a bucket directly clears the slot rather than leaving the
        // previous request's note behind.
        remember(None);
        assert_eq!(
            addressed(),
            None,
            "a request addressing no access point must read no note, got {:?}",
            addressed()
        );
    }
}
