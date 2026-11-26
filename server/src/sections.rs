// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The rest of the radio's configuration, and its modules, field by field as
//! the protobufs describe them: what each is called, what it takes, its unit
//! and limits, and whether it is a secret. Nothing here names a field of its
//! own accord, so a firmware's new settings show up as soon as its protobufs
//! are vendored.
//!
//! Left out: what has a page of its own (LoRa, the role) and what could lock
//! the owner out of their radio (security: keys, admin keys, managed mode).

use meshctl_api::{FieldKind, FixedPosition, SettingsField, SettingsSection};
use meshctl_core::mesh::Settings;
use meshctl_core::radio::{self, SettingsChange};
use meshctl_proto::descriptors;
use meshctl_proto::meshtastic as pb;
use prost::Message;
use prost_reflect::{
    DynamicMessage, ExtensionDescriptor, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage,
    Value,
};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// The parts of the configuration not offered here, and why.
const LEFT_OUT: &[&str] = &[
    // A page of its own, with its own checks.
    "lora",
    // Keys, admin keys and managed mode: a slip locks the owner out.
    "security",
    // Not settings: bookkeeping, and the device UI's own state.
    "version",
    "sessionkey",
    "device_ui",
];

/// Modules not offered here, being elsewhere: the status message, with the
/// owner.
const MODULES_LEFT_OUT: &[&str] = &["statusmessage"];

/// Single settings not offered here, as `section.field`.
const FIELDS_LEFT_OUT: &[&str] = &[
    // On the page of its own, with the LoRa settings.
    "config.device.role",
];

/// Why a section cannot be changed so: the field, or the section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

fn invalid(what: impl Into<String>) -> Invalid {
    Invalid(what.into())
}

fn message(name: &str) -> MessageDescriptor {
    descriptors()
        .get_message_by_name(name)
        .expect("these protobufs describe it")
}

fn metadata_extension() -> ExtensionDescriptor {
    descriptors()
        .get_extension_by_name("meshtastic.field_metadata")
        .expect("these protobufs describe field metadata")
}

/// What the protobufs say of a field, as far as it matters here.
#[derive(Debug, Default)]
struct Said {
    label: Option<String>,
    description: Option<String>,
    unit: Option<String>,
    min: Option<f64>,
    max: Option<f64>,
    secret: bool,
    diy_only: bool,
    deprecated: bool,
}

fn said(field: &FieldDescriptor) -> Said {
    let mut said = said_by_protobufs(field);
    // Limits the protobufs do not say, which the firmware holds to.
    if let Some((least, most)) = limits(field.full_name()) {
        said.min.get_or_insert(least);
        said.max.get_or_insert(most);
    }
    said
}

/// Limits of fields the firmware keeps within a range the protobufs do not
/// say, by their full names.
fn limits(field: &str) -> Option<(f64, f64)> {
    match field {
        // The MQTT map's position precision, in bits: any other, the
        // firmware takes as its default, 14.
        "meshtastic.ModuleConfig.MapReportSettings.position_precision" => Some((12.0, 15.0)),
        _ => None,
    }
}

/// What the protobufs say of a field.
fn said_by_protobufs(field: &FieldDescriptor) -> Said {
    let options = field.options();
    let deprecated_option = options
        .get_field_by_name("deprecated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let extension = metadata_extension();
    if !options.has_extension(&extension) {
        return Said {
            deprecated: deprecated_option,
            ..Said::default()
        };
    }
    let Value::Message(metadata) = options.get_extension(&extension).into_owned() else {
        return Said::default();
    };
    let text = |name: &str| {
        metadata
            .has_field_by_name(name)
            .then(|| metadata.get_field_by_name(name))
            .flatten()
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|s| !s.is_empty())
    };
    let number = |name: &str| {
        metadata
            .has_field_by_name(name)
            .then(|| metadata.get_field_by_name(name))
            .flatten()
            .and_then(|v| v.as_f64())
    };
    let flag = |name: &str| {
        metadata
            .get_field_by_name(name)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    Said {
        label: text("label"),
        description: text("description"),
        unit: text("unit"),
        min: number("min_value"),
        max: number("max_value"),
        secret: flag("sensitive"),
        diy_only: flag("diy_only"),
        deprecated: deprecated_option || flag("deprecated"),
    }
}

/// The range a protobuf integer holds.
fn integer_range(kind: &Kind) -> Option<(f64, f64)> {
    match kind {
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
            Some((f64::from(i32::MIN), f64::from(i32::MAX)))
        }
        Kind::Uint32 | Kind::Fixed32 => Some((0.0, f64::from(u32::MAX))),
        // Wider than an f64 holds exactly, which no setting comes near.
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Some((-9.0e15, 9.0e15)),
        Kind::Uint64 | Kind::Fixed64 => Some((0.0, 9.0e15)),
        _ => None,
    }
}

/// A protobuf value as JSON.
fn to_json(value: &Value, kind: &Kind) -> Json {
    match (value, kind) {
        (Value::Bool(b), _) => Json::Bool(*b),
        (Value::I32(n), _) => Json::from(*n),
        (Value::I64(n), _) => Json::from(*n),
        (Value::U32(n), _) => Json::from(*n),
        (Value::U64(n), _) => Json::from(*n),
        (Value::F32(n), _) => Json::from(f64::from(*n)),
        (Value::F64(n), _) => Json::from(*n),
        (Value::String(s), _) => Json::String(s.clone()),
        (Value::EnumNumber(n), Kind::Enum(e)) => e
            .get_value(*n)
            .map_or_else(|| Json::from(*n), |v| Json::String(v.name().to_owned())),
        _ => Json::Null,
    }
}

/// Whether a value is anything but empty: for a secret, whether one is set.
fn is_set(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::I32(n) | Value::EnumNumber(n) => *n != 0,
        Value::I64(n) => *n != 0,
        Value::U32(n) => *n != 0,
        Value::U64(n) => *n != 0,
        Value::F32(n) => *n != 0.0,
        Value::F64(n) => *n != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Bytes(b) => !b.is_empty(),
        _ => true,
    }
}

/// The settings that are IPv4 addresses, kept as a number with the address's
/// bytes in order from its lowest one: shown and taken as `192.168.1.10`.
const ADDRESSES: &[&str] = &[
    "config.network.ipv4_config.ip",
    "config.network.ipv4_config.gateway",
    "config.network.ipv4_config.subnet",
    "config.network.ipv4_config.dns",
];

/// The settings that are flags kept as bits of a number, and the enum that
/// names each bit.
const FLAGS: &[(&str, &str)] = &[
    (
        "config.network.enabled_protocols",
        "meshtastic.Config.NetworkConfig.ProtocolFlags",
    ),
    (
        "config.position.position_flags",
        "meshtastic.Config.PositionConfig.PositionFlags",
    ),
    ("config.power.powermon_enables", "meshtastic.PowerMon.State"),
    (
        "module.mesh_beacon.flags",
        "meshtastic.ModuleConfig.MeshBeaconConfig.Flags",
    ),
];

/// The flags a field's bits are, by the enum that names them: each one's
/// code and bit, but the none.
fn flags_of(id: &str) -> Option<Vec<(String, u64)>> {
    let (_, name) = FLAGS.iter().find(|(field, _)| *field == id)?;
    let flags = descriptors().get_enum_by_name(name)?;
    Some(
        flags
            .values()
            .filter(|v| v.number() > 0)
            .map(|v| (v.name().to_owned(), v.number() as u64))
            .collect(),
    )
}

/// The bits a list of flags' codes makes, with those of the number before
/// that are no flag known here kept.
fn flags_number(
    json: &Json,
    flags: &[(String, u64)],
    before: u64,
    name: &str,
) -> Result<Json, Invalid> {
    let codes = json.as_array().ok_or_else(|| invalid(name))?;
    let known: u64 = flags
        .iter()
        .map(|(_, bit)| bit)
        .fold(0, |all, bit| all | bit);
    let mut number = before & !known;
    for code in codes {
        let code = code.as_str().ok_or_else(|| invalid(name))?;
        let (_, bit) = flags
            .iter()
            .find(|(c, _)| c == code)
            .ok_or_else(|| invalid(name))?;
        number |= bit;
    }
    Ok(Json::from(number))
}

/// A number of bits, of whichever size the field keeps it in.
fn bits(value: &Value) -> u64 {
    match value {
        Value::U32(n) => u64::from(*n),
        Value::U64(n) => *n,
        _ => 0,
    }
}

/// A value given for a field as the field takes it: an address as its
/// number, flags as their bits; any other as it is.
fn prepared(id: &str, path: &str, json: &Json, part: &DynamicMessage) -> Result<Json, Invalid> {
    let field_id = format!("{id}.{path}");
    if ADDRESSES.contains(&field_id.as_str()) {
        address_number(json, path)
    } else if let Some(flags) = flags_of(&field_id) {
        let before = part
            .get_field_by_name(path)
            .map_or(0, |value| bits(value.as_ref()));
        flags_number(json, &flags, before, path)
    } else {
        Ok(json.clone())
    }
}

/// An address as it is written, from the number it is kept as; empty for
/// none.
fn address(number: u32) -> String {
    if number == 0 {
        String::new()
    } else {
        std::net::Ipv4Addr::from(number.to_le_bytes()).to_string()
    }
}

/// The number an address is kept as, from how it is written; 0 for none.
fn address_number(json: &Json, name: &str) -> Result<Json, Invalid> {
    let text = json.as_str().ok_or_else(|| invalid(name))?.trim();
    if text.is_empty() {
        return Ok(Json::from(0));
    }
    let address: std::net::Ipv4Addr = text.parse().map_err(|_| invalid(name))?;
    Ok(Json::from(u32::from_le_bytes(address.octets())))
}

/// Every setting of a message, nested ones by their dotted path.
fn fields(message: &DynamicMessage, prefix: &str, section: &str, out: &mut Vec<SettingsField>) {
    for field in message.descriptor().fields() {
        let name = format!("{prefix}{}", field.name());
        if field.is_list() || field.is_map() {
            continue;
        }
        if FIELDS_LEFT_OUT.contains(&format!("{section}.{name}").as_str()) {
            continue;
        }
        let said = said(&field);
        if said.deprecated {
            continue;
        }
        let kind = field.kind();
        let value = message.get_field(&field);
        let (field_kind, choices, range) = match &kind {
            Kind::Bool => (FieldKind::Bool, Vec::new(), None),
            Kind::Float | Kind::Double => (FieldKind::Number, Vec::new(), None),
            Kind::String => (FieldKind::Text, Vec::new(), None),
            Kind::Enum(e) => (
                FieldKind::Choice,
                e.values().map(|v| v.name().to_owned()).collect(),
                None,
            ),
            Kind::Message(_) => {
                if let Value::Message(nested) = value.as_ref() {
                    fields(nested, &format!("{name}."), section, out);
                }
                continue;
            }
            // Keys and the like: not something to type into a form.
            Kind::Bytes => continue,
            integer => (FieldKind::Integer, Vec::new(), integer_range(integer)),
        };
        // Flags, as the list of those set.
        if let Some(flags) = flags_of(&format!("{section}.{name}")) {
            let number = bits(value.as_ref());
            let set: Vec<Json> = flags
                .iter()
                .filter(|(_, bit)| number & bit != 0)
                .map(|(code, _)| Json::from(code.clone()))
                .collect();
            out.push(SettingsField {
                name,
                label: said.label,
                description: said.description,
                unit: None,
                kind: FieldKind::Flags,
                min: None,
                max: None,
                choices: flags.into_iter().map(|(code, _)| code).collect(),
                secret: false,
                diy_only: said.diy_only,
                value: Some(Json::Array(set)),
                set: number != 0,
            });
            continue;
        }
        // An address, as it is written.
        if ADDRESSES.contains(&format!("{section}.{name}").as_str()) {
            let number = match value.as_ref() {
                Value::U32(n) => *n,
                _ => 0,
            };
            out.push(SettingsField {
                name,
                label: said.label,
                description: said.description,
                unit: None,
                kind: FieldKind::Text,
                min: None,
                max: None,
                choices: Vec::new(),
                secret: false,
                diy_only: said.diy_only,
                value: Some(Json::String(address(number))),
                set: number != 0,
            });
            continue;
        }
        out.push(SettingsField {
            name,
            label: said.label,
            description: said.description,
            unit: said.unit,
            kind: field_kind,
            min: said.min.or(range.map(|r| r.0)),
            max: said.max.or(range.map(|r| r.1)),
            choices,
            secret: said.secret,
            diy_only: said.diy_only,
            value: (!said.secret).then(|| to_json(&value, &kind)),
            set: is_set(&value),
        });
    }
}

fn section(id: String, name: &str, module: bool, message: &DynamicMessage) -> SettingsSection {
    let mut out = Vec::new();
    fields(message, "", &id, &mut out);
    SettingsSection {
        id,
        name: name.to_owned(),
        module,
        fields: out,
    }
}

/// The radio's configuration as a message described by its descriptor.
fn local_config(settings: &Settings) -> DynamicMessage {
    DynamicMessage::decode(
        message("meshtastic.LocalConfig"),
        settings.config.encode_to_vec().as_slice(),
    )
    .expect("a configuration these protobufs encoded decodes with them")
}

/// A module's configuration, as its own message.
fn module_part(module: &pb::ModuleConfig, name: &str) -> Option<DynamicMessage> {
    let whole = DynamicMessage::decode(
        message("meshtastic.ModuleConfig"),
        module.encode_to_vec().as_slice(),
    )
    .ok()?;
    match whole.get_field_by_name(name)?.into_owned() {
        Value::Message(part) => Some(part),
        _ => None,
    }
}

/// The bit each module has in what the radio says it was built without.
fn excluded_bit(module: &str) -> Option<u32> {
    use pb::ExcludedModules as E;
    Some(match module {
        "mqtt" => E::MqttConfig,
        "serial" => E::SerialConfig,
        "external_notification" => E::ExtnotifConfig,
        "store_forward" => E::StoreforwardConfig,
        "range_test" => E::RangetestConfig,
        "telemetry" => E::TelemetryConfig,
        "canned_message" => E::CannedmsgConfig,
        "audio" => E::AudioConfig,
        "remote_hardware" => E::RemotehardwareConfig,
        "neighbor_info" => E::NeighborinfoConfig,
        "ambient_lighting" => E::AmbientlightingConfig,
        "detection_sensor" => E::DetectionsensorConfig,
        "paxcounter" => E::PaxcounterConfig,
        "bluetooth" => E::BluetoothConfig,
        "network" => E::NetworkConfig,
        "statusmessage" => E::StatusmessageConfig,
        "traffic_management" => E::TrafficmanagementConfig,
        "tak" => E::TakConfig,
        "mesh_beacon" => E::MeshbeaconConfig,
        _ => return None,
    } as u32)
}

/// Whether the radio has a section at all, as it says: the modules it was
/// built with, Bluetooth if it has it, the network if Wi-Fi or Ethernet.
/// Everything, for one that does not say.
pub fn has(settings: &Settings, name: &str) -> bool {
    let Some(metadata) = &settings.metadata else {
        return true;
    };
    if excluded_bit(name).is_some_and(|bit| metadata.excluded_modules & bit != 0) {
        return false;
    }
    match name {
        "bluetooth" => metadata.has_bluetooth,
        "network" => metadata.has_wifi || metadata.has_ethernet,
        _ => true,
    }
}

/// Whether the radio has what a field is for: Wi-Fi's and Ethernet's own,
/// in the network, only if it has them.
fn has_field(settings: &Settings, section: &str, field: &str) -> bool {
    let Some(metadata) = &settings.metadata else {
        return true;
    };
    match (section, field) {
        ("config.network", "wifi_enabled" | "wifi_ssid" | "wifi_psk") => metadata.has_wifi,
        ("config.network", "eth_enabled") => metadata.has_ethernet,
        _ => true,
    }
}

/// Every section offered: the configuration's, then the modules', each as
/// the radio sent it, and only those the radio has.
pub fn sections(settings: &Settings) -> Vec<SettingsSection> {
    let config = local_config(settings);
    let mut sections: Vec<SettingsSection> = config
        .descriptor()
        .fields()
        .filter(|f| !LEFT_OUT.contains(&f.name()) && config.has_field(f) && has(settings, f.name()))
        .filter_map(|f| match config.get_field(&f).into_owned() {
            Value::Message(part) => Some(section(
                format!("config.{}", f.name()),
                f.name(),
                false,
                &part,
            )),
            _ => None,
        })
        .collect();
    for section in &mut sections {
        let id = section.id.clone();
        section.fields.retain(|f| has_field(settings, &id, &f.name));
    }
    sections.extend(settings.modules.iter().filter_map(|(name, module)| {
        if MODULES_LEFT_OUT.contains(&name.as_str()) || !has(settings, name) {
            return None;
        }
        let part = module_part(module, name)?;
        Some(section(format!("module.{name}"), name, true, &part))
    }));
    sections
}

/// A secret of a section offered, for the one who asks for it: a text field
/// the protobufs mark sensitive, by its dotted path. `None` for any other.
pub fn secret(settings: &Settings, section: &str, path: &str) -> Option<String> {
    let part = match section.split_once('.')? {
        ("config", name) if !LEFT_OUT.contains(&name) => {
            match local_config(settings).get_field_by_name(name)?.into_owned() {
                Value::Message(part) => part,
                _ => return None,
            }
        }
        ("module", name) if !MODULES_LEFT_OUT.contains(&name) => {
            module_part(settings.modules.get(name)?, name)?
        }
        _ => return None,
    };
    let mut message = part;
    let mut steps = path.split('.').peekable();
    while let Some(step) = steps.next() {
        let field = message.descriptor().get_field_by_name(step)?;
        let value = message.get_field(&field).into_owned();
        if steps.peek().is_some() {
            let Value::Message(nested) = value else {
                return None;
            };
            message = nested;
            continue;
        }
        return match (said(&field).secret, value) {
            (true, Value::String(text)) => Some(text),
            _ => None,
        };
    }
    None
}

/// A JSON value as a protobuf value of a field's kind, checked against what
/// the field takes.
fn from_json(json: &Json, field: &FieldDescriptor, name: &str) -> Result<Value, Invalid> {
    let said = said(field);
    let kind = field.kind();
    let within = |n: f64| -> Result<f64, Invalid> {
        let (low, high) = integer_range(&kind).unwrap_or((f64::MIN, f64::MAX));
        // The type's own range always holds.
        if n < low || n > high {
            return Err(invalid(name));
        }
        // The protobufs' limits too, but for zero: throughout the firmware, a
        // setting left at zero takes its default, whatever the floor for
        // setting it by hand.
        let floor = said.min.unwrap_or(low);
        let ceiling = said.max.unwrap_or(high);
        if n != 0.0 && (n < floor || n > ceiling) {
            return Err(invalid(name));
        }
        Ok(n)
    };
    let whole = |json: &Json| -> Result<f64, Invalid> {
        let n = json.as_f64().ok_or_else(|| invalid(name))?;
        if n.fract() != 0.0 {
            return Err(invalid(name));
        }
        within(n)
    };
    // The casts below are from numbers just checked to fit.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let value = match &kind {
        Kind::Bool => Value::Bool(json.as_bool().ok_or_else(|| invalid(name))?),
        Kind::String => Value::String(json.as_str().ok_or_else(|| invalid(name))?.to_owned()),
        Kind::Float => Value::F32(within(json.as_f64().ok_or_else(|| invalid(name))?)? as f32),
        Kind::Double => Value::F64(within(json.as_f64().ok_or_else(|| invalid(name))?)?),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Value::I32(whole(json)? as i32),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(whole(json)? as i64),
        Kind::Uint32 | Kind::Fixed32 => Value::U32(whole(json)? as u32),
        Kind::Uint64 | Kind::Fixed64 => Value::U64(whole(json)? as u64),
        Kind::Enum(e) => {
            let code = json.as_str().ok_or_else(|| invalid(name))?;
            Value::EnumNumber(
                e.get_value_by_name(code)
                    .ok_or_else(|| invalid(name))?
                    .number(),
            )
        }
        Kind::Bytes | Kind::Message(_) => return Err(invalid(name)),
    };
    Ok(value)
}

/// Sets a field by its dotted path, into the messages it nests in.
fn set(message: &mut DynamicMessage, path: &str, json: &Json, full: &str) -> Result<(), Invalid> {
    let (head, rest) = match path.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (path, None),
    };
    let field = message
        .descriptor()
        .get_field_by_name(head)
        .ok_or_else(|| invalid(full))?;
    if field.is_list() || field.is_map() {
        return Err(invalid(full));
    }
    match rest {
        Some(rest) => {
            let Kind::Message(_) = field.kind() else {
                return Err(invalid(full));
            };
            let mut nested = match message.get_field(&field).into_owned() {
                Value::Message(nested) => nested,
                _ => return Err(invalid(full)),
            };
            set(&mut nested, rest, json, full)?;
            message.set_field(&field, Value::Message(nested));
        }
        None => {
            let value = from_json(json, &field, full)?;
            message.set_field(&field, value);
        }
    }
    Ok(())
}

/// What the radio is to be sent to change some settings of a section, built
/// on what it has.
pub fn change(
    settings: &Settings,
    id: &str,
    values: &BTreeMap<String, Json>,
) -> Result<SettingsChange, Invalid> {
    let (group, name) = id.split_once('.').ok_or_else(|| invalid(id))?;
    for path in values.keys() {
        if FIELDS_LEFT_OUT.contains(&format!("{id}.{path}").as_str()) {
            return Err(invalid(path.clone()));
        }
    }
    let mut change = SettingsChange::default();
    match group {
        "config" => {
            if LEFT_OUT.contains(&name) {
                return Err(invalid(id));
            }
            let config = local_config(settings);
            let field = config
                .descriptor()
                .get_field_by_name(name)
                .filter(|f| config.has_field(f))
                .ok_or_else(|| invalid(id))?;
            let Value::Message(mut part) = config.get_field(&field).into_owned() else {
                return Err(invalid(id));
            };
            for (path, json) in values {
                let json = prepared(id, path, json, &part)?;
                set(&mut part, path, &json, path)?;
            }
            change.configs.push(wrap("meshtastic.Config", name, part)?);
        }
        "module" => {
            let module = settings.modules.get(name).ok_or_else(|| invalid(id))?;
            let mut part = module_part(module, name).ok_or_else(|| invalid(id))?;
            for (path, json) in values {
                let json = prepared(id, path, json, &part)?;
                set(&mut part, path, &json, path)?;
            }
            change
                .module_configs
                .push(wrap("meshtastic.ModuleConfig", name, part)?);
        }
        _ => return Err(invalid(id)),
    }
    Ok(change)
}

/// The section whose fixed position can be given along with it.
const POSITION: &str = "config.position";

/// What a change to a section does to the radio's fixed position: set it
/// where given, which takes the position fixed, as it is or as changed
/// along; or let it go, as the position stops being fixed.
pub fn fixed(
    settings: &Settings,
    id: &str,
    values: &BTreeMap<String, Json>,
    at: Option<&FixedPosition>,
) -> Result<Option<radio::FixedPosition>, Invalid> {
    let was = settings
        .config
        .position
        .as_ref()
        .is_some_and(|p| p.fixed_position);
    let will = match values.get("fixed_position") {
        Some(Json::Bool(will)) if id == POSITION => *will,
        _ => was,
    };
    let Some(at) = at else {
        return Ok((was && !will).then_some(radio::FixedPosition::Remove));
    };
    let inside = (-90.0..=90.0).contains(&at.latitude) && (-180.0..=180.0).contains(&at.longitude);
    if id != POSITION || !will || !inside {
        return Err(invalid("fixed_position"));
    }
    // In ten millionths of a degree, as the protobufs carry it: well
    // within an i32 for any degree in range.
    #[allow(clippy::cast_possible_truncation)]
    let degrees = |d: f64| Some((d * 1e7).round() as i32);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(0));
    Ok(Some(radio::FixedPosition::Set(pb::Position {
        latitude_i: degrees(at.latitude),
        longitude_i: degrees(at.longitude),
        altitude: at.altitude,
        time,
        location_source: pb::position::LocSource::LocManual as i32,
        ..Default::default()
    })))
}

/// A section, back into the message that carries one: `Config` or
/// `ModuleConfig`, decoded into prost's own type to send.
fn wrap<M: Message + Default>(
    carrier: &str,
    name: &str,
    part: DynamicMessage,
) -> Result<M, Invalid> {
    let mut whole = DynamicMessage::new(message(carrier));
    whole
        .try_set_field_by_name(name, Value::Message(part))
        .map_err(|_| invalid(name))?;
    M::decode(whole.encode_to_vec().as_slice()).map_err(|_| invalid(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::{config, module_config};

    fn settings() -> Settings {
        let mut settings = Settings::default();
        settings.config.device = Some(config::DeviceConfig {
            role: config::device_config::Role::ClientBase as i32,
            node_info_broadcast_secs: 43_200,
            ..Default::default()
        });
        settings.config.lora = Some(config::LoRaConfig::default());
        settings.config.network = Some(config::NetworkConfig {
            wifi_enabled: true,
            wifi_ssid: "spirit".into(),
            wifi_psk: "s3cret".into(),
            ..Default::default()
        });
        settings.modules.insert(
            "mqtt".into(),
            pb::ModuleConfig {
                payload_variant: Some(module_config::PayloadVariant::Mqtt(
                    module_config::MqttConfig {
                        enabled: true,
                        address: "mqtt.lan".into(),
                        password: "large4cats".into(),
                        map_report_settings: Some(module_config::MapReportSettings {
                            publish_interval_secs: 3600,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )),
            },
        );
        settings.modules.insert(
            "telemetry".into(),
            pb::ModuleConfig {
                payload_variant: Some(module_config::PayloadVariant::Telemetry(
                    module_config::TelemetryConfig {
                        device_update_interval: 900,
                        ..Default::default()
                    },
                )),
            },
        );
        settings
    }

    fn find<'a>(sections: &'a [SettingsSection], id: &str) -> &'a SettingsSection {
        sections.iter().find(|s| s.id == id).expect(id)
    }

    fn field<'a>(section: &'a SettingsSection, name: &str) -> &'a SettingsField {
        section.fields.iter().find(|f| f.name == name).expect(name)
    }

    #[test]
    fn sections_are_what_the_radio_sent_less_what_is_left_out() {
        let sections = sections(&settings());
        let ids: Vec<_> = sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "config.device",
                "config.network",
                "module.mqtt",
                "module.telemetry"
            ]
        );
        let device = find(&sections, "config.device");
        assert!(device.fields.iter().all(|f| f.name != "role"));
    }

    #[test]
    fn fields_say_what_they_are_and_secrets_stay_secret() {
        let sections = sections(&settings());
        let network = find(&sections, "config.network");
        let psk = field(network, "wifi_psk");
        assert!(psk.secret && psk.set);
        assert_eq!(psk.value, None);
        assert_eq!(
            field(network, "wifi_ssid").value,
            Some(Json::from("spirit"))
        );

        let mqtt = find(&sections, "module.mqtt");
        assert_eq!(field(mqtt, "password").value, None);
        let nested = field(mqtt, "map_report_settings.publish_interval_secs");
        assert_eq!(nested.value, Some(Json::from(3600)));
        assert_eq!(nested.kind, FieldKind::Integer);
        // The protobufs set its floor: an hour.
        assert_eq!(nested.min, Some(3600.0));
        assert!(
            field(mqtt, "address").label.is_some(),
            "labels come from the protobufs"
        );

        let json = serde_json::to_string(&sections).unwrap();
        assert!(!json.contains("s3cret") && !json.contains("large4cats"));
    }

    #[test]
    fn a_module_changes_field_by_field_and_keeps_the_rest() {
        let values = BTreeMap::from([
            ("enabled".to_owned(), Json::Bool(false)),
            (
                "map_report_settings.publish_interval_secs".to_owned(),
                Json::from(7200),
            ),
        ]);
        let change = change(&settings(), "module.mqtt", &values).unwrap();
        let Some(module_config::PayloadVariant::Mqtt(mqtt)) =
            &change.module_configs[0].payload_variant
        else {
            panic!("an MQTT config");
        };
        assert!(!mqtt.enabled);
        assert_eq!(
            mqtt.map_report_settings
                .as_ref()
                .unwrap()
                .publish_interval_secs,
            7200
        );
        assert_eq!(mqtt.password, "large4cats", "a secret not given is kept");
        assert_eq!(mqtt.address, "mqtt.lan");
    }

    #[test]
    fn addresses_read_and_are_taken_as_written() {
        let mut settings = settings();
        if let Some(network) = settings.config.network.as_mut() {
            network.ipv4_config = Some(pb::config::network_config::IpV4Config {
                ip: u32::from_le_bytes([192, 168, 1, 10]),
                ..Default::default()
            });
        }
        let network = sections(&settings)
            .into_iter()
            .find(|s| s.id == "config.network")
            .unwrap();
        let ip = network
            .fields
            .iter()
            .find(|f| f.name == "ipv4_config.ip")
            .unwrap();
        assert_eq!(ip.kind, FieldKind::Text);
        assert_eq!(ip.value, Some(Json::from("192.168.1.10")));
        let gateway = network
            .fields
            .iter()
            .find(|f| f.name == "ipv4_config.gateway")
            .unwrap();
        assert_eq!(gateway.value, Some(Json::from("")));

        let values =
            BTreeMap::from([("ipv4_config.gateway".to_owned(), Json::from("192.168.1.1"))]);
        let change = change(&settings, "config.network", &values).unwrap();
        let Some(pb::config::PayloadVariant::Network(network)) = &change.configs[0].payload_variant
        else {
            panic!("a network config");
        };
        let ipv4 = network.ipv4_config.unwrap();
        assert_eq!(ipv4.gateway, u32::from_le_bytes([192, 168, 1, 1]));
        assert_eq!(ipv4.ip, u32::from_le_bytes([192, 168, 1, 10]), "kept");
        let bad = BTreeMap::from([("ipv4_config.dns".to_owned(), Json::from("1.2.3"))]);
        assert!(super::change(&settings, "config.network", &bad).is_err());
    }

    #[test]
    fn flags_read_and_are_taken_as_a_list() {
        let mut settings = settings();
        settings.config.position = Some(pb::config::PositionConfig {
            position_flags: 0x0001 | 0x0100 | 0x8000,
            ..Default::default()
        });
        let all = sections(&settings);
        let position = all.iter().find(|s| s.id == "config.position").unwrap();
        let flags = position
            .fields
            .iter()
            .find(|f| f.name == "position_flags")
            .unwrap();
        assert_eq!(flags.kind, FieldKind::Flags);
        assert_eq!(flags.value, Some(Json::from(vec!["ALTITUDE", "HEADING"])));
        assert!(flags.choices.contains(&"SPEED".to_owned()));

        let values = BTreeMap::from([("position_flags".to_owned(), Json::from(vec!["SPEED"]))]);
        let change = change(&settings, "config.position", &values).unwrap();
        let Some(pb::config::PayloadVariant::Position(position)) =
            &change.configs[0].payload_variant
        else {
            panic!("a position config");
        };
        assert_eq!(
            position.position_flags,
            0x0200 | 0x8000,
            "a bit not known here, kept"
        );
        let bad = BTreeMap::from([("position_flags".to_owned(), Json::from(vec!["NOPE"]))]);
        assert!(super::change(&settings, "config.position", &bad).is_err());
    }

    #[test]
    fn a_fixed_position_goes_with_its_setting() {
        let mut settings = settings();
        settings.config.position = Some(config::PositionConfig::default());
        let coruna = FixedPosition {
            latitude: 43.362_3,
            longitude: -8.411_5,
            altitude: Some(54),
        };
        let on = BTreeMap::from([("fixed_position".to_owned(), Json::Bool(true))]);
        let none = BTreeMap::new();

        // Set along with fixing it.
        let Some(radio::FixedPosition::Set(at)) =
            fixed(&settings, "config.position", &on, Some(&coruna)).unwrap()
        else {
            panic!("a position set");
        };
        assert_eq!(at.latitude_i, Some(433_623_000));
        assert_eq!(at.longitude_i, Some(-84_115_000));
        assert_eq!(at.altitude, Some(54));
        assert_eq!(
            at.location_source,
            pb::position::LocSource::LocManual as i32
        );
        // Not on a position that is not fixed, nor elsewhere, nor off the
        // globe.
        assert!(fixed(&settings, "config.position", &none, Some(&coruna)).is_err());
        assert!(fixed(&settings, "config.device", &on, Some(&coruna)).is_err());
        let nowhere = FixedPosition {
            latitude: 91.0,
            ..coruna
        };
        assert!(fixed(&settings, "config.position", &on, Some(&nowhere)).is_err());
        // Fixing it with none given keeps what the radio has.
        assert_eq!(
            fixed(&settings, "config.position", &on, None).unwrap(),
            None
        );

        // Once fixed: moved alone, or let go.
        settings.config.position = Some(config::PositionConfig {
            fixed_position: true,
            ..Default::default()
        });
        assert!(matches!(
            fixed(&settings, "config.position", &none, Some(&coruna)),
            Ok(Some(radio::FixedPosition::Set(_)))
        ));
        let off = BTreeMap::from([("fixed_position".to_owned(), Json::Bool(false))]);
        assert_eq!(
            fixed(&settings, "config.position", &off, None).unwrap(),
            Some(radio::FixedPosition::Remove)
        );
        assert!(fixed(&settings, "config.position", &off, Some(&coruna)).is_err());
    }

    #[test]
    fn the_map_precision_is_kept_as_the_firmware_keeps_it() {
        let all = sections(&settings());
        let mqtt = all.iter().find(|s| s.id == "module.mqtt").unwrap();
        let precision = mqtt
            .fields
            .iter()
            .find(|f| f.name == "map_report_settings.position_precision")
            .unwrap();
        assert_eq!((precision.min, precision.max), (Some(12.0), Some(15.0)));
        let with = |bits: u32| {
            let values = BTreeMap::from([(
                "map_report_settings.position_precision".to_owned(),
                Json::from(bits),
            )]);
            change(&settings(), "module.mqtt", &values).is_ok()
        };
        assert!(with(13));
        // Zero is its default, as anywhere.
        assert!(with(0));
        assert!(!with(20));
        assert!(!with(11));
    }

    #[test]
    fn only_what_the_radio_has_is_offered() {
        let mut settings = settings();
        settings.metadata = Some(pb::DeviceMetadata {
            has_wifi: true,
            has_bluetooth: false,
            has_ethernet: false,
            excluded_modules: pb::ExcludedModules::MqttConfig as u32,
            ..Default::default()
        });
        let all = sections(&settings);
        let network = all.iter().find(|s| s.id == "config.network").unwrap();
        assert!(network.fields.iter().any(|f| f.name == "wifi_ssid"));
        assert!(!network.fields.iter().any(|f| f.name == "eth_enabled"));
        assert!(!all.iter().any(|s| s.id == "config.bluetooth"));
        assert!(!all.iter().any(|s| s.id == "module.mqtt"));
    }

    #[test]
    fn a_secret_is_handed_out_only_if_it_is_one() {
        let settings = settings();
        assert_eq!(
            secret(&settings, "config.network", "wifi_psk").as_deref(),
            Some("s3cret")
        );
        assert_eq!(secret(&settings, "config.network", "wifi_ssid"), None);
        assert_eq!(secret(&settings, "config.security", "private_key"), None);
        assert_eq!(secret(&settings, "config.network", "nothing"), None);
    }

    #[test]
    fn a_secret_is_written_when_given() {
        let values = BTreeMap::from([("wifi_psk".to_owned(), Json::from("n3w"))]);
        let change = change(&settings(), "config.network", &values).unwrap();
        let Some(config::PayloadVariant::Network(network)) = &change.configs[0].payload_variant
        else {
            panic!("a network config");
        };
        assert_eq!(network.wifi_psk, "n3w");
        assert_eq!(network.wifi_ssid, "spirit");
    }

    #[test]
    fn what_a_section_cannot_take_is_refused() {
        let cases: [(&str, &str, Json); 7] = [
            ("module.telemetry", "device_update_interval", Json::from(-1)),
            (
                "module.telemetry",
                "device_update_interval",
                Json::from(1.5),
            ),
            ("module.telemetry", "no_such_field", Json::from(1)),
            ("module.mqtt", "enabled", Json::from("yes")),
            ("config.device", "role", Json::from("ROUTER")),
            ("config.lora", "hop_limit", Json::from(3)),
            ("module.paxcounter", "enabled", Json::Bool(true)),
        ];
        for (section, name, value) in cases {
            let values = BTreeMap::from([(name.to_owned(), value.clone())]);
            assert!(
                change(&settings(), section, &values).is_err(),
                "{section} {name} {value}"
            );
        }
    }

    #[test]
    fn zero_is_the_default_whatever_the_floor() {
        let path = "map_report_settings.publish_interval_secs";
        let zero = BTreeMap::from([(path.to_owned(), Json::from(0))]);
        assert!(change(&settings(), "module.mqtt", &zero).is_ok());
        let below = BTreeMap::from([(path.to_owned(), Json::from(60))]);
        assert!(change(&settings(), "module.mqtt", &below).is_err());
    }

    #[test]
    fn a_choice_takes_a_code() {
        let values = BTreeMap::from([("rebroadcast_mode".to_owned(), Json::from("LOCAL_ONLY"))]);
        let change = change(&settings(), "config.device", &values).unwrap();
        let Some(config::PayloadVariant::Device(device)) = &change.configs[0].payload_variant
        else {
            panic!("a device config");
        };
        assert_eq!(
            device.rebroadcast_mode,
            config::device_config::RebroadcastMode::LocalOnly as i32
        );
        assert_eq!(device.node_info_broadcast_secs, 43_200);
        let bad = BTreeMap::from([("rebroadcast_mode".to_owned(), Json::from("SOMETIMES"))]);
        assert!(super::change(&settings(), "config.device", &bad).is_err());
    }
}
