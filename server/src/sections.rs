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

use meshctl_api::{FieldKind, SettingsField, SettingsSection};
use meshctl_core::mesh::Settings;
use meshctl_core::radio::SettingsChange;
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

/// Every section offered: the configuration's, then the modules', each as
/// the radio sent it.
pub fn sections(settings: &Settings) -> Vec<SettingsSection> {
    let config = local_config(settings);
    let mut sections: Vec<SettingsSection> = config
        .descriptor()
        .fields()
        .filter(|f| !LEFT_OUT.contains(&f.name()) && config.has_field(f))
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
    sections.extend(settings.modules.iter().filter_map(|(name, module)| {
        if MODULES_LEFT_OUT.contains(&name.as_str()) {
            return None;
        }
        let part = module_part(module, name)?;
        Some(section(format!("module.{name}"), name, true, &part))
    }));
    sections
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
                set(&mut part, path, json, path)?;
            }
            change.configs.push(wrap("meshtastic.Config", name, part)?);
        }
        "module" => {
            let module = settings.modules.get(name).ok_or_else(|| invalid(id))?;
            let mut part = module_part(module, name).ok_or_else(|| invalid(id))?;
            for (path, json) in values {
                set(&mut part, path, json, path)?;
            }
            change
                .module_configs
                .push(wrap("meshtastic.ModuleConfig", name, part)?);
        }
        _ => return Err(invalid(id)),
    }
    Ok(change)
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
