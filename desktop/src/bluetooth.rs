// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Radios by Bluetooth on a desktop, reached by the app for the daemon
//! within it, as Android's app reaches them: by each firmware's service,
//! Meshtastic's to and from the radio, MeshCore's over Nordic's UART, their
//! payloads one by one. Through BlueZ on Linux and WinRT on Windows; the
//! radio paired by the system, which asks for its PIN.

use btleplug::api::{
    Central as _, CentralEvent, CentralState, CharPropFlags, Characteristic, Manager as _,
    Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures_util::StreamExt;
use meshctl_core::link::Framing;
use meshctl_core::link::ble::{BleOpener, Looked, Nearby, Pipe};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use uuid::{Uuid, uuid};

const MESHTASTIC: Uuid = uuid!("6ba1b218-15a8-461f-9fa8-5dcae273eafd");
const TO_RADIO: Uuid = uuid!("f75c76d2-129e-4dad-a1dd-7866124401e7");
const FROM_RADIO: Uuid = uuid!("2c55e69e-4993-11ed-b878-0242ac120002");
const FROM_NUM: Uuid = uuid!("ed9da18c-a800-4f66-a670-aa7547e34453");
const UART: Uuid = uuid!("6e400001-b5a3-f393-e0a9-e50e24dcca9e");
const UART_RX: Uuid = uuid!("6e400002-b5a3-f393-e0a9-e50e24dcca9e");
const UART_TX: Uuid = uuid!("6e400003-b5a3-f393-e0a9-e50e24dcca9e");

/// How long radios nearby are looked for.
const LOOKING: Duration = Duration::from_secs(8);
/// How long a radio has to answer a connection, and to say what it has.
const CONNECTING: Duration = Duration::from_secs(20);

/// Reaches radios by Bluetooth for the daemon.
pub struct Desktop;

/// The first Bluetooth adapter, on: or why not.
async fn adapter() -> io::Result<Adapter> {
    let manager = Manager::new().await.map_err(io::Error::other)?;
    let adapter = manager
        .adapters()
        .await
        .map_err(io::Error::other)?
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Bluetooth adapter"))?;
    if adapter.adapter_state().await.ok() == Some(CentralState::PoweredOff) {
        return Err(io::Error::other("Bluetooth is off"));
    }
    Ok(adapter)
}

/// Looks for what is nearby for a while. BlueZ goes between Bluetooth
/// Classic and LE as it does, which radios speak, hence longer than
/// Android's LE alone.
async fn scan(adapter: &Adapter) -> io::Result<()> {
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(io::Error::other)?;
    tokio::time::sleep(LOOKING).await;
    let _ = adapter.stop_scan().await;
    Ok(())
}

/// Which firmware a radio heard has, by the service it says it has, or
/// else by its name.
fn framing_of(services: &[Uuid], name: Option<&str>) -> Option<Framing> {
    if services.contains(&MESHTASTIC) {
        Some(Framing::Meshtastic)
    } else if services.contains(&UART) {
        Some(Framing::Meshcore)
    } else if name.is_some_and(|n| n.starts_with("Meshtastic")) {
        Some(Framing::Meshtastic)
    } else if name.is_some_and(|n| n.starts_with("MeshCore")) {
        Some(Framing::Meshcore)
    } else {
        None
    }
}

/// The radios heard nearby in a few seconds, the best heard first.
async fn look() -> Vec<Nearby> {
    let adapter = match adapter().await {
        Ok(adapter) => adapter,
        Err(error) => {
            tracing::warn!(%error, "no Bluetooth to look for radios with");
            return Vec::new();
        }
    };
    if let Err(error) = scan(&adapter).await {
        tracing::warn!(%error, "cannot look for radios by Bluetooth");
        return Vec::new();
    }
    let mut heard = Vec::new();
    for peripheral in adapter.peripherals().await.unwrap_or_default() {
        let Ok(Some(properties)) = peripheral.properties().await else {
            continue;
        };
        let name = properties
            .local_name
            .clone()
            .or(properties.advertisement_name.clone())
            .filter(|n| !n.is_empty());
        // Each heard, radio or not, to tell why one is not listed.
        tracing::debug!(
            address = %properties.address,
            name = name.as_deref().unwrap_or(""),
            rssi = ?properties.rssi,
            services = ?properties.services,
            "heard by Bluetooth"
        );
        let Some(framing) = framing_of(&properties.services, name.as_deref()) else {
            continue;
        };
        heard.push(Nearby {
            address: properties.address.to_string().to_uppercase(),
            name,
            rssi: properties.rssi,
            framing: Some(framing),
        });
    }
    heard.sort_by_key(|n| std::cmp::Reverse(n.rssi.unwrap_or(i16::MIN)));
    heard
}

/// A radio by its address: known to the system, as one paired is, or
/// else heard as it is looked for.
async fn find(adapter: &Adapter, address: &str) -> io::Result<Peripheral> {
    let wanted = address.to_uppercase();
    let matching = |peripherals: Vec<Peripheral>| {
        peripherals
            .into_iter()
            .find(|p| p.address().to_string().to_uppercase() == wanted)
    };
    if let Some(found) = matching(adapter.peripherals().await.unwrap_or_default()) {
        return Ok(found);
    }
    scan(adapter).await?;
    matching(adapter.peripherals().await.unwrap_or_default())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the radio is not in reach"))
}

/// One of a radio's characteristics, by its UUID.
fn characteristic(peripheral: &Peripheral, id: Uuid) -> io::Result<Characteristic> {
    peripheral
        .characteristics()
        .into_iter()
        .find(|c| c.uuid == id)
        .ok_or_else(|| io::Error::other("the radio does not have its firmware's service"))
}

/// How what goes to a radio is written: answered, where it takes that.
fn write_type(characteristic: &Characteristic) -> WriteType {
    if characteristic.properties.contains(CharPropFlags::WRITE) {
        WriteType::WithResponse
    } else {
        WriteType::WithoutResponse
    }
}

/// Connects to a radio and passes its payloads both ways, until either
/// side goes.
async fn open(address: String, framing: Framing) -> io::Result<Pipe> {
    let adapter = adapter().await?;
    let peripheral = find(&adapter, &address).await?;
    tokio::time::timeout(CONNECTING, async {
        peripheral.connect().await?;
        peripheral.discover_services().await
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "the radio by Bluetooth did not answer",
        )
    })?
    .map_err(io::Error::other)?;

    let (to_radio, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
    let (incoming, from_radio) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut notifications = peripheral.notifications().await.map_err(io::Error::other)?;
    let mut events = adapter.events().await.map_err(io::Error::other)?;
    let id = peripheral.id();

    match framing {
        // What it has for the client is read once it says so, and after
        // each thing written, until it has nothing more.
        Framing::Meshtastic => {
            let to = characteristic(&peripheral, TO_RADIO)?;
            let from = characteristic(&peripheral, FROM_RADIO)?;
            let number = characteristic(&peripheral, FROM_NUM)?;
            peripheral.subscribe(&number).await.map_err(unpaired)?;
            let ready = Arc::new(Notify::new());
            // Reading.
            tokio::spawn({
                let (peripheral, ready) = (peripheral.clone(), Arc::clone(&ready));
                async move {
                    loop {
                        loop {
                            match peripheral.read(&from).await {
                                Ok(payload) if payload.is_empty() => break,
                                Ok(payload) => {
                                    if incoming.send(payload).is_err() {
                                        return;
                                    }
                                }
                                Err(_) => return,
                            }
                        }
                        tokio::select! {
                            () = ready.notified() => {}
                            notified = notifications.next() => if notified.is_none() { return },
                            event = gone(&mut events, &id) => if event { return },
                        }
                    }
                }
            });
            // Writing.
            tokio::spawn(async move {
                let kind = write_type(&to);
                while let Some(payload) = outgoing.recv().await {
                    if peripheral.write(&to, &payload, kind).await.is_err() {
                        break;
                    }
                    ready.notify_one();
                }
                let _ = peripheral.disconnect().await;
            });
        }
        // Each payload as its own write, and each it sends as it comes.
        Framing::Meshcore => {
            let rx = characteristic(&peripheral, UART_RX)?;
            let tx = characteristic(&peripheral, UART_TX)?;
            peripheral.subscribe(&tx).await.map_err(unpaired)?;
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        notified = notifications.next() => match notified {
                            Some(notified) if notified.uuid == UART_TX => {
                                if incoming.send(notified.value).is_err() {
                                    return;
                                }
                            }
                            Some(_) => {}
                            None => return,
                        },
                        event = gone(&mut events, &id) => if event { return },
                    }
                }
            });
            tokio::spawn(async move {
                let kind = write_type(&rx);
                while let Some(payload) = outgoing.recv().await {
                    if peripheral.write(&rx, &payload, kind).await.is_err() {
                        break;
                    }
                }
                let _ = peripheral.disconnect().await;
            });
        }
    }
    Ok((to_radio, from_radio))
}

/// Waits for the next of the adapter's events: true once it says the
/// radio went, or it has no more to say.
async fn gone(
    events: &mut Pin<Box<dyn futures_util::Stream<Item = CentralEvent> + Send>>,
    id: &btleplug::platform::PeripheralId,
) -> bool {
    match events.next().await {
        Some(CentralEvent::DeviceDisconnected(went)) => &went == id,
        Some(_) => false,
        None => true,
    }
}

/// A radio that would not let its service be listened to: most likely not
/// paired, which the system does, asking for its PIN.
fn unpaired(error: btleplug::Error) -> io::Error {
    io::Error::other(format!(
        "the radio was not paired: pair it in the system's Bluetooth settings first ({error})"
    ))
}

impl BleOpener for Desktop {
    fn open(
        &self,
        address: String,
        framing: Framing,
    ) -> Pin<Box<dyn Future<Output = io::Result<Pipe>> + Send>> {
        Box::pin(open(address, framing))
    }

    fn look(&self) -> Pin<Box<dyn Future<Output = Looked> + Send>> {
        Box::pin(async { Looked::Heard(look().await) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_radio_is_told_by_its_service_or_its_name() {
        assert_eq!(framing_of(&[MESHTASTIC], None), Some(Framing::Meshtastic));
        assert_eq!(framing_of(&[UART], Some("Phone")), Some(Framing::Meshcore));
        assert_eq!(
            framing_of(&[], Some("Meshtastic_1a2b")),
            Some(Framing::Meshtastic)
        );
        assert_eq!(
            framing_of(&[], Some("MeshCore-Casa")),
            Some(Framing::Meshcore)
        );
        assert_eq!(framing_of(&[], Some("Auriculares")), None);
    }
}
