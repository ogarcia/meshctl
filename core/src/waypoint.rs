// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Meshtastic's waypoints: points on the map shared with the mesh, with a
//! name, an emoji and, maybe, a fence around them that tells when a node
//! goes in or out.
//!
//! On the air a waypoint is all of it, every time: changing one is sending
//! it again with the same id, and deleting it for everyone, sending it with
//! [`DELETED`] for when it expires, as the official apps and the firmware do.
//! One locked to a node is that node's alone to change or delete.

use meshctl_proto::meshtastic as pb;

/// What `expire` says of a waypoint deleted: the firmware's and the apps'
/// way. Zero is one that never expires.
pub const DELETED: u32 = 1;

/// The most a name takes, in bytes: what the firmware keeps, its end
/// included, is 30.
pub const NAME_BYTES: usize = 29;

/// The most a description takes, in bytes, as the name.
pub const DESCRIPTION_BYTES: usize = 99;

/// 📍, the emoji of one with none.
pub const DEFAULT_ICON: u32 = 0x1F4CD;

/// A waypoint, as it goes on the air.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Waypoint {
    /// Its own, which names it across the mesh: not the packet's.
    pub id: u32,
    pub latitude: f64,
    pub longitude: f64,
    /// When it expires, in seconds since the epoch; 0, never; [`DELETED`],
    /// gone.
    pub expire: u32,
    /// The node it is locked to, the only one that may change it; 0 for
    /// anyone.
    pub locked_to: u32,
    pub name: String,
    pub description: String,
    /// Its emoji, as a Unicode code point; 0 for none.
    pub icon: u32,
    /// A circle around it that fences it, its radius in metres; 0 for none.
    pub fence_radius: u32,
    /// A box that fences it, instead of the circle or besides it.
    pub fence_box: Option<FenceBox>,
    /// Whether nodes going in or out of the fence are told of; for the
    /// favourites only, maybe. Each receiver decides for itself whether to
    /// heed them: the firmware does only for those it made.
    pub notify_on_enter: bool,
    pub notify_on_exit: bool,
    pub notify_favorites_only: bool,
}

/// A box on the map, by its edges, in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FenceBox {
    pub south: f64,
    pub west: f64,
    pub north: f64,
    pub east: f64,
}

impl Waypoint {
    /// Whether it says it is gone, deleted or expired by `now`.
    pub fn is_gone(&self, now: u32) -> bool {
        self.expire == DELETED || (self.expire > DELETED && self.expire <= now)
    }

    /// Whether a node may change or delete it: anyone, unless it is locked
    /// to another.
    pub fn changeable_by(&self, node: u32) -> bool {
        self.locked_to == 0 || self.locked_to == node
    }

    /// Whether it has a fence, of either shape.
    pub fn is_fenced(&self) -> bool {
        self.fence_radius > 0 || self.fence_box.is_some()
    }

    /// Its emoji, or 📍 for one with none or one that is not a character.
    pub fn emoji(&self) -> char {
        char::from_u32(self.icon)
            .filter(|_| self.icon != 0)
            .or_else(|| char::from_u32(DEFAULT_ICON))
            .unwrap_or('📍')
    }

    /// The same, deleted for everyone: as it was, but for when it expires.
    pub fn deleted(&self) -> Self {
        Self {
            expire: DELETED,
            ..self.clone()
        }
    }

    /// Its name and description cut to what the firmware keeps, at a
    /// character's edge.
    pub fn fitted(mut self) -> Self {
        self.name = cut(&self.name, NAME_BYTES);
        self.description = cut(&self.description, DESCRIPTION_BYTES);
        self
    }
}

/// A text cut to so many bytes, at a character's edge.
fn cut(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Degrees from the 1e-7 the protobufs carry, and back.
fn degrees(scaled: i32) -> f64 {
    f64::from(scaled) * 1e-7
}

fn scaled(degrees: f64) -> i32 {
    // Within an i32 for any place on Earth; clamped for anything else.
    (degrees * 1e7)
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

impl From<pb::Waypoint> for Waypoint {
    fn from(w: pb::Waypoint) -> Self {
        Self {
            id: w.id,
            latitude: degrees(w.latitude_i.unwrap_or_default()),
            longitude: degrees(w.longitude_i.unwrap_or_default()),
            expire: w.expire,
            locked_to: w.locked_to,
            name: w.name,
            description: w.description,
            icon: w.icon,
            fence_radius: w.geofence_radius,
            fence_box: w.bounding_box.map(|b| FenceBox {
                south: degrees(b.latitude_south_i),
                west: degrees(b.longitude_west_i),
                north: degrees(b.latitude_north_i),
                east: degrees(b.longitude_east_i),
            }),
            notify_on_enter: w.notify_on_enter,
            notify_on_exit: w.notify_on_exit,
            notify_favorites_only: w.notify_favorites_only,
        }
    }
}

impl From<&Waypoint> for pb::Waypoint {
    fn from(w: &Waypoint) -> Self {
        Self {
            id: w.id,
            latitude_i: Some(scaled(w.latitude)),
            longitude_i: Some(scaled(w.longitude)),
            expire: w.expire,
            locked_to: w.locked_to,
            name: w.name.clone(),
            description: w.description.clone(),
            icon: w.icon,
            geofence_radius: w.fence_radius,
            bounding_box: w.fence_box.map(|b| pb::BoundingBox {
                longitude_west_i: scaled(b.west),
                latitude_south_i: scaled(b.south),
                longitude_east_i: scaled(b.east),
                latitude_north_i: scaled(b.north),
            }),
            notify_on_enter: w.notify_on_enter,
            notify_on_exit: w.notify_on_exit,
            notify_favorites_only: w.notify_favorites_only,
        }
    }
}

/// A waypoint as heard: who sent it, where and when.
#[derive(Debug, Clone, PartialEq)]
pub struct HeardWaypoint {
    /// The packet's id.
    pub packet_id: u32,
    pub from: u32,
    pub to: u32,
    /// The channel's slot on the radio that heard it.
    pub channel: u32,
    /// When it was heard, in seconds since the epoch.
    pub at: u32,
    pub via_mqtt: bool,
    pub waypoint: Waypoint,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_goes_and_comes_back_as_it_was() {
        let waypoint = Waypoint {
            id: 0xdead_beef,
            latitude: 43.362_345_6,
            longitude: -8.411_234_5,
            expire: 1_800_000_000,
            locked_to: 0x1234_abcd,
            name: "Cumbre".into(),
            description: "Where the antenna is".into(),
            icon: '⛰'.into(),
            fence_radius: 250,
            fence_box: Some(FenceBox {
                south: 43.3,
                west: -8.5,
                north: 43.4,
                east: -8.3,
            }),
            notify_on_enter: true,
            notify_on_exit: false,
            notify_favorites_only: true,
        };
        let back = Waypoint::from(pb::Waypoint::from(&waypoint));
        assert_eq!(back.id, waypoint.id);
        assert!((back.latitude - waypoint.latitude).abs() < 1e-7);
        assert!((back.longitude - waypoint.longitude).abs() < 1e-7);
        assert_eq!(back.fence_radius, 250);
        assert_eq!(
            back.fence_box.map(|b| (b.south * 10.0).round()),
            Some(433.0)
        );
        assert_eq!(back.emoji(), '⛰');
        assert!(back.notify_on_enter && back.notify_favorites_only && !back.notify_on_exit);
    }

    #[test]
    fn gone_is_deleted_or_past_its_time() {
        let at = |expire| Waypoint {
            expire,
            ..Default::default()
        };
        assert!(!at(0).is_gone(1000), "never expires");
        assert!(at(DELETED).is_gone(1000), "deleted");
        assert!(at(999).is_gone(1000));
        assert!(at(1000).is_gone(1000));
        assert!(!at(1001).is_gone(1000));
        assert!(at(5).deleted().is_gone(0));
    }

    #[test]
    fn locked_it_is_its_owners_alone() {
        let open = Waypoint::default();
        assert!(open.changeable_by(7));
        let locked = Waypoint {
            locked_to: 7,
            ..Default::default()
        };
        assert!(locked.changeable_by(7));
        assert!(!locked.changeable_by(8));
    }

    #[test]
    fn names_are_cut_at_a_characters_edge() {
        let long = Waypoint {
            // 14 two-byte letters and one more: 29 bytes fit 14 and a half.
            name: "ñññññññññññññññ".into(),
            description: "x".repeat(120),
            ..Default::default()
        }
        .fitted();
        assert_eq!(long.name, "ññññññññññññññ");
        assert_eq!(long.description.len(), DESCRIPTION_BYTES);
        assert_eq!(Waypoint::default().emoji(), '📍');
    }
}
