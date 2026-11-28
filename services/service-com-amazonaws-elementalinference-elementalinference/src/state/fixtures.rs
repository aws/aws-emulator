// SPDX-License-Identifier: CC0-1.0

//! The FIXTURE catalog: the sports events the external data source holds, as this
//! emulator holds them, and the window arithmetic a search selects them with.
//!
//! No operation of this service creates, updates or deletes a fixture — `SearchFixtures`
//! and `GetFixture` are both `@readonly` — so the catalog is FILLED when the store is
//! constructed ([`default_fixture_catalog`]) and afterwards only through the in-process
//! seam ([`ComAmazonawsElementalinferenceElementalinferenceState::seed_fixtures`],
//! [`ComAmazonawsElementalinferenceElementalinferenceState::clear_fixtures`]). Nothing a
//! request carries can grow it.

use super::*;

impl ComAmazonawsElementalinferenceElementalinferenceState {
    /// Install fixtures into the data-source catalog, keyed by their `FixtureId`.
    ///
    /// THE SEEDING SEAM, and the only way an entry reaches [`Self::fixtures`]: this
    /// service models no operation that creates a fixture, because AWS relays
    /// fixtures from an external sports data source, so an emulator that is to report
    /// any has to be handed them in process. A fixture whose id is already held is
    /// REPLACED, so re-seeding an updated record neither duplicates it nor moves it in
    /// the page order.
    pub fn seed_fixtures<I: IntoIterator<Item = Fixture>>(&mut self, fixtures: I) {
        for fixture in fixtures {
            self.fixtures.insert(fixture.id.clone(), fixture);
        }
    }

    /// Drop every held fixture — the seam's other half, for a data source that holds
    /// nothing.
    pub fn clear_fixtures(&mut self) {
        self.fixtures.clear();
    }

    /// The one fixture a `FixtureId` names, or `None` when the catalog holds no such
    /// entry.
    pub fn fixture(&self, fixture_id: &str) -> Option<&Fixture> {
        self.fixtures.get(fixture_id)
    }

    /// Every held fixture of `sport` scheduled on a UTC day from `start_day` through
    /// `end_day` (both ends included, as a search window is), in `FixtureId` order.
    ///
    /// This is the SELECTION a search reports from: a fixture of another sport, or one
    /// scheduled outside the window, is not in it, so a window the catalog holds no
    /// event for selects nothing at all.
    pub fn fixtures_in_window(
        &self,
        sport: DataSourceSport,
        start_day: i64,
        end_day: i64,
    ) -> impl Iterator<Item = &Fixture> {
        self.fixtures.values().filter(move |fixture| {
            fixture.sport == sport && (start_day..=end_day).contains(&fixture.scheduled_day())
        })
    }
}

/// Seconds in a UTC day. A `FixtureDate` names a UTC day ("in UTC",
/// `SearchFixturesRequest$startDate`), and UTC days have no offset changes, so a day
/// is exactly this many seconds and an epoch-second count divides into day numbers.
pub(super) const SECONDS_PER_DAY: i64 = 86_400;

/// The widest search window `SearchFixtures` accepts, in days, counting both ends:
/// "The window from startDate through endDate must not exceed seven days"
/// (`SearchFixturesRequest$endDate`).
pub const MAX_SEARCH_WINDOW_DAYS: i64 = 7;

/// A fixture whose scheduled start is still ahead.
///
/// `GetFixtureResponse$status` is the lifecycle status "as provided by the data
/// source. For example, Scheduled or Completed", so the member is a free-form string
/// whose values belong to the data source: this constant and
/// [`FIXTURE_STATUS_COMPLETED`] are the two the model's own documentation names, and a
/// held fixture carries whatever status it was given.
pub const FIXTURE_STATUS_SCHEDULED: &str = "Scheduled";

/// A fixture that has been played — the other status the model's documentation names
/// (see [`FIXTURE_STATUS_SCHEDULED`]).
pub const FIXTURE_STATUS_COMPLETED: &str = "Completed";

/// The sports the data source covers: exactly the `DataSourceSport` enum
/// (`smithy_query(query="shape",shape="DataSourceSport")` — `basketball`,
/// `american-football`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSourceSport {
    /// `basketball`.
    Basketball,
    /// `american-football`.
    AmericanFootball,
}

impl DataSourceSport {
    /// The sport a `DataSourceSport` value names, or `None` for a value that enum
    /// does not declare.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "basketball" => Some(Self::Basketball),
            "american-football" => Some(Self::AmericanFootball),
            _ => None,
        }
    }

    /// The value this sport takes on the wire, as the `DataSourceSport` enum spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Basketball => "basketball",
            Self::AmericanFootball => "american-football",
        }
    }

    /// Every sport the enum declares, in model order.
    pub const ALL: &'static [Self] = &[Self::Basketball, Self::AmericanFootball];
}

/// One fixture the data source holds: a sports event, as the store keeps it.
///
/// Every member is the data source's own — the emulator derives none of them — so the
/// `FixtureSummary` `SearchFixtures` lists and the `GetFixture` response for the same
/// `fixtureId` are two projections of this ONE record and agree member for member.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The `FixtureId` (modelled length 1..128): the key this fixture is held under
    /// and the id `GetFixture` answers for.
    pub id: String,
    /// The sport this fixture belongs to, which is what a search's required `sport`
    /// selects on.
    pub sport: DataSourceSport,
    /// The fixture's name, "as provided by the data source. For example, the names of
    /// the two competing teams" (`FixtureSummary$name`).
    pub name: String,
    /// The competition the fixture belongs to; `None` for a fixture the data source
    /// gives no group for ("The data source doesn't provide this information for
    /// every fixture", `GetFixtureResponse$fixtureGroup`).
    pub group: Option<String>,
    /// The scheduled start, in seconds since the Unix epoch. Its UTC day is what a
    /// search's date window selects on ("The search includes fixtures that are
    /// scheduled on this day", `SearchFixturesRequest$startDate`), so a held fixture
    /// always has one.
    pub scheduled_start_secs: i64,
    /// The lifecycle status "as provided by the data source"
    /// (`FixtureSummary$status`) — see [`FIXTURE_STATUS_SCHEDULED`].
    pub status: String,
    /// The competitors (`CompetitorList`, modelled length ..30).
    pub competitors: Vec<Competitor>,
}

impl Fixture {
    /// The UTC day this fixture is scheduled on, as a day number since the Unix
    /// epoch — the figure a search's window is compared against.
    pub fn scheduled_day(&self) -> i64 {
        self.scheduled_start_secs.div_euclid(SECONDS_PER_DAY)
    }

    /// The scheduled start as the modelled `Timestamp`.
    pub fn scheduled_start(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from_secs(self.scheduled_start_secs)
    }

    /// The `FixtureSummary` projection `SearchFixtures` reports.
    pub fn summary(&self) -> FixtureSummary {
        FixtureSummary {
            fixture_id: self.id.clone(),
            name: self.name.clone(),
            fixture_group: self.group.clone(),
            scheduled_start: Some(self.scheduled_start()),
            status: self.status.clone(),
            competitors: self.competitors.clone(),
        }
    }

    /// Whether this fixture satisfies a `COMPETITOR` filter over `values`: true when
    /// one of its competitors' names contains one of the values, compared without
    /// regard to case. An empty value list matches nothing, since a fixture has to
    /// "match at least one of the values" (`SearchFixturesRequest$filters`).
    ///
    /// Containment rather than equality is a DELIBERATE choice, not an oversight: the
    /// model says only that a fixture must match one of the values, and a caller that
    /// knows a competitor by part of its name ("Hawks") is the case the filter exists
    /// for. Equality would be the stricter reading; nothing in the model picks one.
    pub fn matches_competitor(&self, values: &[String]) -> bool {
        self.competitors.iter().any(|competitor| {
            let name = competitor.name.as_deref().unwrap_or_default().to_lowercase();
            values
                .iter()
                .any(|value| name.contains(&value.to_lowercase()))
        })
    }
}

/// Parse a `FixtureDate` (`^\d{4}-\d{2}-\d{2}$`) into a day number since the Unix
/// epoch, or `None` when the text is not a real UTC calendar date.
///
/// The modelled pattern admits text that is not a date at all (`2026-02-31`), so the
/// date is parsed as an RFC-3339 instant by `aws-smithy-types` — the same crate the
/// framework parses modelled timestamps with — and then rendered back and compared,
/// which rejects anything the parser normalised instead of accepting.
pub fn parse_fixture_date(date: &str) -> Option<i64> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{date}T00:00:00Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    let epoch_day = parsed.secs().div_euclid(SECONDS_PER_DAY);
    (fixture_date_string(epoch_day)?.as_str() == date).then_some(epoch_day)
}

/// The `FixtureDate` text (`YYYY-MM-DD`) of a day number since the Unix epoch, or
/// `None` for a day number no calendar date corresponds to.
pub fn fixture_date_string(epoch_day: i64) -> Option<String> {
    let rendered = ::aws_smithy_types::DateTime::from_secs(epoch_day.checked_mul(SECONDS_PER_DAY)?)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    // `Format::DateTime` renders `YYYY-MM-DDThh:mm:ssZ`, so the date is its first ten
    // characters — and a rendering that is shorter than that is not one.
    rendered.get(..10).map(str::to_string)
}

/// The epoch-second instant of `hour`:`minute` UTC on `epoch_day`, or `None` when
/// that day number is not a day the calendar has, or the time of day is not a time a
/// UTC day has (`hour` outside 0..=23, `minute` outside 0..=59).
///
/// The seam's companion to [`parse_fixture_date`]: a fixture is held with the instant
/// it starts at, and this is how a caller that knows the day and the kick-off time
/// builds that instant without reimplementing the UTC day arithmetic. Refusing a time
/// of day outside the day is what keeps the instant ON the day the caller named, since
/// that day is what a search window selects on; every step is checked, so no input
/// wraps or panics.
pub fn fixture_start_secs(epoch_day: i64, hour: i64, minute: i64) -> Option<i64> {
    fixture_date_string(epoch_day)?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    epoch_day
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)
}

/// How many days either side of the day the catalog is built its schedule covers.
///
/// A search window is at most [`MAX_SEARCH_WINDOW_DAYS`] days wide, so a span of two
/// weeks each way means every window a caller can ask for around the present day holds
/// events, which is what the recorded service answers for a recent window. It also
/// BOUNDS the catalog: the schedule is `2 * CATALOG_SPAN_DAYS + 1` days of
/// [`FIXTURES_PER_SPORT_PER_DAY`] fixtures for each sport the `DataSourceSport` enum
/// declares, and nothing on the wire can add to it.
pub const CATALOG_SPAN_DAYS: i64 = 14;

/// How many fixtures of one sport the schedule holds on one day: a double-header, so a
/// single day's search reports more than one row and paging over a window has something
/// to page.
pub const FIXTURES_PER_SPORT_PER_DAY: usize = 2;

/// The UTC hour the first fixture of a day starts at, and the gap to the next one.
///
/// The data source's own figures; they only have to put every fixture of a day ON that
/// day, since the day is what a search window selects on.
pub(super) const FIRST_KICK_OFF_HOUR: i64 = 18;
pub(super) const KICK_OFF_SPACING_HOURS: i64 = 3;

/// Now, in seconds since the Unix epoch — the instant the catalog is built around and
/// the one a held fixture's status is decided against.
pub fn current_epoch_secs() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs() as i64)
        .unwrap_or_default()
}

/// The teams of `sport`, as this emulator's data source holds them.
///
/// They are the EMULATOR's own clubs, not any real competition's: the modelled members
/// they fill (`FixtureSummary$name` "as provided by the data source", `Competitor$name`)
/// are free-form strings whose values belong to the data source, and this emulator
/// reaches no sports data source to relay another's. Each sport has its own list, so no
/// fixture pairs a basketball side against a football one, and both lists are long
/// enough that the pairing below never has a team play itself.
pub(super) fn sport_teams(sport: DataSourceSport) -> &'static [&'static str] {
    match sport {
        DataSourceSport::Basketball => &[
            "Harbor Hawks",
            "Summit Suns",
            "Delta Dynamos",
            "Granite Giants",
            "Cobalt Comets",
            "Prairie Pumas",
            "Ember Eagles",
            "Lakeside Lynx",
        ],
        DataSourceSport::AmericanFootball => &[
            "Ironside Bisons",
            "Copper Cardinals",
            "Northgate Nomads",
            "Sandstone Stallions",
            "Riverbend Rangers",
            "Vantage Vipers",
            "Quarry Quakes",
            "Beacon Bulldogs",
        ],
    }
}

/// The competition `sport`'s fixtures belong to, for the fixtures the data source gives
/// a group for ("The group that the fixture belongs to, such as the competition, league,
/// or tournament", `FixtureSummary$fixtureGroup`).
pub(super) fn sport_competition(sport: DataSourceSport) -> &'static str {
    match sport {
        DataSourceSport::Basketball => "Continental Basketball League",
        DataSourceSport::AmericanFootball => "Continental Gridiron League",
    }
}

/// The (home, away) pairing the schedule gives slot `slot` of day `day`.
///
/// A rotation over the sport's teams, so the same day always yields the same pairing,
/// two slots of one day are two different fixtures, and the away side is never the home
/// side (the offset is drawn from `1..=len-1`, which cannot be a multiple of the team
/// count).
pub(super) fn scheduled_pairing(
    teams: &'static [&'static str],
    day: i64,
    slot: usize,
) -> (&'static str, &'static str) {
    let count = teams.len() as i64;
    let index = day * FIXTURES_PER_SPORT_PER_DAY as i64 + slot as i64;
    let home = index.rem_euclid(count);
    let offset = index.div_euclid(count).rem_euclid(count - 1) + 1;
    let away = (home + offset).rem_euclid(count);
    (teams[home as usize], teams[away as usize])
}

/// The `FixtureId` the schedule gives slot `slot` of day `day` for `sport`.
///
/// Opaque, as a data source's id is, and derived from the three figures that identify
/// the fixture (the same hash the data endpoints' label uses), so one fixture keeps one
/// id for the life of the store while no two fixtures share one, and a client cannot
/// read a date or a sport out of an id it was handed.
pub(super) fn catalog_fixture_id(sport: DataSourceSport, day: i64, slot: usize) -> String {
    let mut seed = fnv1a64(format!("{}|{day}|{slot}", sport.as_wire()).as_bytes());
    format!("{:016x}", splitmix64(&mut seed))
}

/// The schedule the data source holds at `now_secs`: for each sport the
/// `DataSourceSport` enum declares, [`FIXTURES_PER_SPORT_PER_DAY`] fixtures on each UTC
/// day within [`CATALOG_SPAN_DAYS`] of that instant's day.
///
/// This is the catalog a store comes up with (see [`Default`] on the state struct), and
/// the reason a search of a recent window reports events rather than nothing: a client
/// obtains a `fixtureId` only from a search, so a `GetFixture` and a clipping output's
/// `dataSourceConfiguration` have nothing to name until a search can report one. A
/// fixture that has already started carries [`FIXTURE_STATUS_COMPLETED`] and one still
/// ahead [`FIXTURE_STATUS_SCHEDULED`] — the two statuses the model's own documentation
/// names — so the catalog is the snapshot of the schedule as of the instant it was taken.
pub fn default_fixture_catalog(now_secs: i64) -> Vec<Fixture> {
    let center_day = now_secs.div_euclid(SECONDS_PER_DAY);
    let mut catalog = Vec::new();
    for sport in DataSourceSport::ALL.iter().copied() {
        let teams = sport_teams(sport);
        for day in (center_day - CATALOG_SPAN_DAYS)..=(center_day + CATALOG_SPAN_DAYS) {
            for slot in 0..FIXTURES_PER_SPORT_PER_DAY {
                let hour = FIRST_KICK_OFF_HOUR + slot as i64 * KICK_OFF_SPACING_HOURS;
                // A day number the calendar does not have, or a kick-off past the end
                // of the day, is no fixture at all rather than a panic.
                let Some(scheduled_start_secs) = fixture_start_secs(day, hour, 0) else {
                    continue;
                };
                let (home, away) = scheduled_pairing(teams, day, slot);
                catalog.push(Fixture {
                    id: catalog_fixture_id(sport, day, slot),
                    sport,
                    // "For example, the names of the two competing teams"
                    // (`FixtureSummary$name`).
                    name: format!("{away} at {home}"),
                    // "The data source doesn't provide this information for every
                    // fixture": the second fixture of a day carries no group.
                    group: (slot == 0).then(|| sport_competition(sport).to_string()),
                    scheduled_start_secs,
                    status: if scheduled_start_secs <= now_secs {
                        FIXTURE_STATUS_COMPLETED.to_string()
                    } else {
                        FIXTURE_STATUS_SCHEDULED.to_string()
                    },
                    competitors: vec![
                        Competitor {
                            name: Some(home.to_string()),
                            is_home: Some(true),
                        },
                        Competitor {
                            name: Some(away.to_string()),
                            is_home: Some(false),
                        },
                    ],
                });
            }
        }
    }
    catalog
}
