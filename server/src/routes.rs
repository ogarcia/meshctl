// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Which node each hop of a MeshCore route most likely is, when its hash
//! alone cannot say: a hash is one to three bytes of a key, and many known
//! nodes may start so, or the only one that does may be the wrong one, a
//! node the radio never heard being the one that repeated it.
//!
//! The route is read as a whole, as the cheapest way through it: the nodes
//! that repeat floods over those that do not, each hop within reach of the
//! last one placed, as far as the hops between can go, the last one heard
//! by the radio. A hop none of whose known nodes fits is told as one not
//! known. It is a guess, and told as one.

/// How far a hop can reach and still be as likely as any nearer, in km.
const REACH_KM: f64 = 150.0;

/// What each km of a hop costs: the nearer of two that fit, the likelier.
const PER_KM: f64 = 0.01;

/// What each km past the reach costs: enough that a node a country away
/// is less likely than one never heard.
const PAST_REACH_PER_KM: f64 = 0.05;

/// What a hop costs as a node not known: more than a known one that fits.
const UNKNOWN: f64 = 1.5;

/// What the same node twice in a row costs: a node does not repeat what it
/// sent itself.
const TWICE: f64 = 5.0;

/// What a last hop costs that the radio is not known to hear directly.
const NOT_HEARD: f64 = 0.5;

/// Whether a node repeats floods, as its kind tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeats {
    /// A repeater.
    Yes,
    /// A room server, which can be set to.
    Maybe,
    /// A companion or a sensor, which do not.
    No,
}

/// A known node a hop's hash may be.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub num: u32,
    /// Where it says it is, latitude and longitude.
    pub at: Option<(f64, f64)>,
    pub repeats: Repeats,
    /// Whether the radio hears it directly.
    pub direct: bool,
}

/// The node each hop most likely is, from the sender's side to the radio,
/// by the nodes each may be; none for a hop that is likely one not known.
/// `radio` is where the radio is, if it says.
pub fn likely(hops: &[Vec<Candidate>], radio: Option<(f64, f64)>) -> Vec<Option<u32>> {
    if hops.is_empty() {
        return Vec::new();
    }
    // Each hop's choices: its candidates, then one not known.
    let choices: Vec<Vec<Option<&Candidate>>> = hops
        .iter()
        .map(|hop| hop.iter().map(Some).chain([None]).collect())
        .collect();
    // The cheapest way to each choice of each hop, and the choice before it.
    let mut ways: Vec<Vec<Way>> = Vec::with_capacity(hops.len());
    ways.push(
        choices[0]
            .iter()
            .map(|c| Way {
                cost: own(*c),
                before: 0,
                anchor: place(*c).map(|at| (at, 0)),
            })
            .collect(),
    );
    for at in 1..choices.len() {
        let next = choices[at]
            .iter()
            .map(|choice| {
                let (before, cost) = ways[at - 1]
                    .iter()
                    .zip(&choices[at - 1])
                    .enumerate()
                    .map(|(i, (way, prior))| (i, way.cost + between(way, *prior, *choice)))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap_or((0, 0.0));
                // Where the way was last placed, and how many hops back: a
                // hop that cannot be placed leaves it there.
                let anchor = match place(*choice) {
                    Some(here) => Some((here, 0)),
                    None => ways[at - 1][before].anchor.map(|(a, n)| (a, n + 1)),
                };
                Way {
                    cost: cost + own(*choice),
                    before,
                    anchor,
                }
            })
            .collect();
        ways.push(next);
    }
    // The last, as the radio heard it.
    let last = choices.len() - 1;
    let mut at = ways[last]
        .iter()
        .zip(&choices[last])
        .enumerate()
        .map(|(i, (way, c))| (i, way.cost + heard(way, *c, radio)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(i, _)| i);
    let mut picked = vec![None; choices.len()];
    for hop in (0..choices.len()).rev() {
        picked[hop] = choices[hop][at].map(|c| c.num);
        at = ways[hop][at].before;
    }
    picked
}

/// The cheapest way to a choice: what it costs, the choice before it, and
/// where the way was last placed and how many hops back.
struct Way {
    cost: f64,
    before: usize,
    anchor: Option<((f64, f64), u32)>,
}

/// Where a choice is, if it is a node that says.
fn place(choice: Option<&Candidate>) -> Option<(f64, f64)> {
    choice.and_then(|c| c.at)
}

/// What a choice costs in itself: one not known, or one that would not
/// repeat it.
fn own(choice: Option<&Candidate>) -> f64 {
    match choice.map(|c| c.repeats) {
        None => UNKNOWN,
        Some(Repeats::Yes) => 0.0,
        Some(Repeats::Maybe) => 0.5,
        Some(Repeats::No) => 2.0,
    }
}

/// What it costs to go on to the next choice: how far from where the way
/// was last placed, in as many hops, when the next says where it is.
fn between(way: &Way, prior: Option<&Candidate>, next: Option<&Candidate>) -> f64 {
    if let (Some(a), Some(b)) = (prior, next)
        && a.num == b.num
    {
        return TWICE;
    }
    match (way.anchor, place(next)) {
        (Some((from, back)), Some(to)) => far(km(from, to), back + 1),
        _ => 0.0,
    }
}

/// What it costs that the radio heard the last: nothing for one it hears
/// directly; how far from where the way was last placed, for another.
fn heard(way: &Way, choice: Option<&Candidate>, radio: Option<(f64, f64)>) -> f64 {
    if choice.is_some_and(|c| c.direct) {
        return 0.0;
    }
    let distance = match (way.anchor, radio) {
        (Some((from, back)), Some(radio)) => far(km(from, radio), back + 1),
        _ => 0.0,
    };
    if choice.is_some() {
        NOT_HEARD + distance
    } else {
        distance
    }
}

/// What a stretch this long costs, over so many hops.
fn far(km: f64, hops: u32) -> f64 {
    km * PER_KM + (km - REACH_KM * f64::from(hops)).max(0.0) * PAST_REACH_PER_KM
}

/// How far apart two places are, in km, over the Earth.
fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    const EARTH_KM: f64 = 6371.0;
    let (lat1, lat2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_KM * h.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORUNA: (f64, f64) = (43.37, -8.40);
    const SANTIAGO: (f64, f64) = (42.88, -8.54);
    const LUGO: (f64, f64) = (43.01, -7.56);
    const LISBOA: (f64, f64) = (38.72, -9.14);
    const BRETANA: (f64, f64) = (48.50, -2.90);

    fn node(num: u32, at: (f64, f64), repeats: Repeats, direct: bool) -> Candidate {
        Candidate {
            num,
            at: Some(at),
            repeats,
            direct,
        }
    }

    #[test]
    fn the_nearer_of_two_is_picked() {
        // Two may be the first hop: one by Santiago, one in Lisboa; the
        // second is a repeater the radio hears, in Coruña.
        let hops = vec![
            vec![
                node(1, LISBOA, Repeats::Yes, false),
                node(2, SANTIAGO, Repeats::Yes, false),
            ],
            vec![node(3, CORUNA, Repeats::Yes, true)],
        ];
        assert_eq!(likely(&hops, Some(CORUNA)), [Some(2), Some(3)]);
    }

    #[test]
    fn one_a_country_away_is_one_not_known() {
        // The only one known with the hash is in Brittany, between two in
        // Galicia: it is likely another, not known.
        let hops = vec![
            vec![node(1, LUGO, Repeats::Yes, false)],
            vec![node(2, BRETANA, Repeats::Yes, false)],
            vec![node(3, SANTIAGO, Repeats::Yes, true)],
        ];
        assert_eq!(likely(&hops, Some(CORUNA)), [Some(1), None, Some(3)]);
    }

    #[test]
    fn a_repeater_over_a_companion() {
        let hops = vec![vec![
            node(1, CORUNA, Repeats::No, true),
            node(2, CORUNA, Repeats::Yes, true),
        ]];
        assert_eq!(likely(&hops, Some(CORUNA)), [Some(2)]);
    }

    #[test]
    fn the_last_one_the_radio_hears() {
        // Two repeaters near: the one the radio hears directly.
        let hops = vec![vec![
            node(1, SANTIAGO, Repeats::Yes, false),
            node(2, SANTIAGO, Repeats::Yes, true),
        ]];
        assert_eq!(likely(&hops, Some(CORUNA)), [Some(2)]);
    }

    #[test]
    fn far_past_a_hop_not_known_is_one_not_known() {
        // Between two in Galicia, past one no known node has the hash of,
        // the only one known with the next is in Lisboa: likely another.
        let hops = vec![
            vec![node(1, LUGO, Repeats::Yes, false)],
            vec![],
            vec![node(2, LISBOA, Repeats::Yes, false)],
            vec![],
            vec![node(3, SANTIAGO, Repeats::Yes, true)],
        ];
        assert_eq!(
            likely(&hops, Some(CORUNA)),
            [Some(1), None, None, None, Some(3)]
        );
    }

    #[test]
    fn with_no_places_a_known_one_still() {
        let hops = vec![
            vec![Candidate {
                num: 1,
                at: None,
                repeats: Repeats::Yes,
                direct: false,
            }],
            vec![],
        ];
        assert_eq!(likely(&hops, None), [Some(1), None]);
        assert!(likely(&[], None).is_empty());
    }
}
