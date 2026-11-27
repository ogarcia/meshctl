// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Waypoints: points on the map a Meshtastic radio shares with the mesh, or
//! hears shared, each with a name, an emoji and, maybe, a fence that tells
//! when a node goes in or out of it.

use serde::{Deserialize, Serialize};

/// The most a waypoint's name takes, in bytes: what the firmware keeps.
pub const NAME_BYTES: usize = 29;

/// The most a waypoint's description takes, in bytes.
pub const DESCRIPTION_BYTES: usize = 99;

/// The widest a fence's circle can be, in metres: 100 km, more than any
/// mesh's reach.
pub const FENCE_RADIUS_MAX: u32 = 100_000;

/// A waypoint, as meshctl keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Waypoint {
    /// Its own id, which names it across the mesh.
    #[cfg_attr(feature = "openapi", schema(example = 3_735_928_559_u32))]
    pub id: u32,
    pub latitude: f64,
    pub longitude: f64,
    #[cfg_attr(feature = "openapi", schema(example = "Cumbre"))]
    pub name: String,
    pub description: String,
    /// Its emoji.
    #[cfg_attr(feature = "openapi", schema(example = "⛰️"))]
    pub icon: String,
    /// When it expires, in seconds since the epoch; none, never.
    pub expire: Option<i64>,
    /// The node it is locked to, the only one that may change or delete it;
    /// none for anyone.
    pub locked_to: Option<u32>,
    /// The fence around it, if it has one.
    pub fence: Option<Fence>,
    /// Whether meshctl tells of nodes going in or out of its fence: of the
    /// radio's own, as asked when shared; of another's, once asked to.
    #[serde(default)]
    pub watched: bool,
    /// The channel's slot it was last heard or sent on.
    pub channel: u32,
    /// Who shared it first, and who last changed it, by their node numbers.
    pub created_by: u32,
    pub updated_by: u32,
    /// When it was first heard or sent, and when last changed, in seconds
    /// since the epoch.
    pub created_at: i64,
    pub updated_at: i64,
}

/// A fence around a waypoint: a circle, a box, or both. Going in or out of
/// it is told of, as its author asks and whoever hears it heeds.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Fence {
    /// A circle around the waypoint, its radius in metres.
    #[serde(default)]
    pub radius: Option<u32>,
    /// A box on the map.
    #[serde(default)]
    pub area: Option<FenceArea>,
    /// What its author asks to be told of: nodes going in, or out; of the
    /// favourites only, maybe.
    #[serde(default)]
    pub notify_on_enter: bool,
    #[serde(default)]
    pub notify_on_exit: bool,
    #[serde(default)]
    pub notify_favorites_only: bool,
}

/// A box on the map, by its edges, in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FenceArea {
    pub south: f64,
    pub west: f64,
    pub north: f64,
    pub east: f64,
}

/// A waypoint to share, new or changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewWaypoint {
    pub latitude: f64,
    pub longitude: f64,
    /// Up to 29 bytes, not empty.
    pub name: String,
    /// Up to 99 bytes.
    #[serde(default)]
    pub description: String,
    /// One emoji; none for 📍.
    #[serde(default)]
    pub icon: Option<String>,
    /// When it expires, in seconds since the epoch, still to come; none,
    /// never.
    #[serde(default)]
    pub expire: Option<i64>,
    /// Whether it is locked to the radio, so that no other node may change
    /// or delete it.
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub fence: Option<Fence>,
    /// The channel's slot it goes on: whoever has that channel hears it.
    #[serde(default)]
    pub channel: u32,
}

/// Whether a radio's map shows its waypoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MapWaypoints {
    pub shown: bool,
}

impl Fence {
    /// Whether it fences anything: a circle or a box.
    pub fn is_some(&self) -> bool {
        self.radius.is_some_and(|r| r > 0) || self.area.is_some()
    }
}

impl FenceArea {
    /// Whether it is a box on the map: south under north and west before
    /// east, within the world.
    pub fn is_valid(&self) -> bool {
        (-90.0..=90.0).contains(&self.south)
            && (-90.0..=90.0).contains(&self.north)
            && (-180.0..=180.0).contains(&self.west)
            && (-180.0..=180.0).contains(&self.east)
            && self.south < self.north
            && self.west < self.east
    }
}

impl NewWaypoint {
    /// Whether it can be shared as it is, by `now`, in seconds since the
    /// epoch: somewhere on the map (not 0, 0, which says nowhere), a name of
    /// up to 29 bytes and a description of up to 99, one emoji if any, an
    /// expiry still to come, and a fence that fences something sensible.
    pub fn is_valid(&self, now: i64) -> bool {
        let placed = (-90.0..=90.0).contains(&self.latitude)
            && (-180.0..=180.0).contains(&self.longitude)
            && !(self.latitude == 0.0 && self.longitude == 0.0);
        let named = !self.name.trim().is_empty() && self.name.len() <= NAME_BYTES;
        let icon = self.icon.as_deref().is_none_or(|icon| {
            let mut chars = icon
                .chars()
                .filter(|c| !matches!(c, '\u{fe0e}' | '\u{fe0f}'));
            chars.next().is_some() && chars.next().is_none()
        });
        let fence = self.fence.is_none_or(|fence| {
            fence.radius.is_none_or(|r| r <= FENCE_RADIUS_MAX)
                && fence.area.is_none_or(|area| area.is_valid())
        });
        placed
            && named
            && self.description.len() <= DESCRIPTION_BYTES
            && icon
            && self.expire.is_none_or(|at| at > now)
            && fence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> NewWaypoint {
        NewWaypoint {
            latitude: 43.36,
            longitude: -8.41,
            name: "Cumbre".into(),
            description: String::new(),
            icon: Some("⛰️".into()),
            expire: None,
            locked: false,
            fence: None,
            channel: 0,
        }
    }

    #[test]
    fn a_waypoint_is_checked_before_it_goes() {
        assert!(valid().is_valid(1000));
        let check = |change: fn(&mut NewWaypoint)| {
            let mut w = valid();
            change(&mut w);
            w.is_valid(1000)
        };
        assert!(!check(|w| w.name = "  ".into()), "no name");
        assert!(!check(|w| w.name = "x".repeat(30)), "a name too long");
        assert!(
            !check(|w| w.description = "x".repeat(100)),
            "a description too long"
        );
        assert!(
            !check(|w| (w.latitude, w.longitude) = (0.0, 0.0)),
            "nowhere"
        );
        assert!(!check(|w| w.latitude = 91.0), "off the map");
        assert!(!check(|w| w.icon = Some("ab".into())), "two characters");
        assert!(check(|w| w.icon = None), "none is a pin");
        assert!(!check(|w| w.expire = Some(999)), "expired already");
        assert!(check(|w| w.expire = Some(2000)));
        assert!(!check(|w| w.fence = Some(Fence {
            radius: Some(FENCE_RADIUS_MAX + 1),
            ..Default::default()
        })));
        assert!(
            !check(|w| w.fence = Some(Fence {
                area: Some(FenceArea {
                    south: 44.0,
                    west: -9.0,
                    north: 43.0,
                    east: -8.0
                }),
                ..Default::default()
            })),
            "south over north"
        );
        assert!(check(|w| w.fence = Some(Fence {
            radius: Some(250),
            area: Some(FenceArea {
                south: 43.0,
                west: -9.0,
                north: 44.0,
                east: -8.0
            }),
            notify_on_enter: true,
            ..Default::default()
        })));
    }
}
