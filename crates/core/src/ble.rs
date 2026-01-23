use core::str::FromStr;

use defmt::{Debug2Format, error, info, warn};
use embassy_futures::select::select;
use embassy_time::Timer;
use esp_radio::ble::controller::BleConnector;
use trouble_host::{
    Address, Host, HostResources,
    gap::{GapConfig, PeripheralConfig},
    prelude::*,
};

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 2;

#[gatt_server]
struct Server {
    // device_info: DeviceInformation,
    battery_service: BatteryService,
}

// #[gatt_service(uuid = "7d4ad3b7-0ca8-41c3-8e19-dd5cbe2f780c")]
// struct Settings {
// }

// b7a709de-2d41-4e84-a898-70551e33cb71

#[gatt_service(uuid = service::DEVICE_INFORMATION)]
struct DeviceInformation {
    #[characteristic(uuid = characteristic::MANUFACTURER_NAME_STRING, read, value = HeaplessString::from_str("Gentoid Sp. z O.O.").unwrap())]
    manufacturer_name: HeaplessString<32>,

    #[characteristic(uuid = characteristic::MODEL_NUMBER_STRING, read, value = HeaplessString::from_str("ESP32-C6 sensor node").unwrap())]
    model_number: HeaplessString<32>,

    #[characteristic(uuid = characteristic::FIRMWARE_REVISION_STRING, read, value = HeaplessString::from_str(env!("CARGO_PKG_VERSION")).unwrap())]
    firmware_revision: HeaplessString<16>,

    #[characteristic(uuid = characteristic::SERIAL_NUMBER_STRING, read, value = HeaplessString::from_str("1234567890ABC").unwrap())]
    serial_number: HeaplessString<16>,
}

/// Battery service
#[gatt_service(uuid = service::BATTERY)]
struct BatteryService {
    /// Battery Level
    #[descriptor(uuid = descriptors::VALID_RANGE, read, value = [0, 100])]
    #[descriptor(uuid = descriptors::MEASUREMENT_DESCRIPTION, name = "hello", read, value = "Battery Level")]
    #[characteristic(uuid = characteristic::BATTERY_LEVEL, read, notify, value = 10)]
    level: u8,
    #[characteristic(uuid = "408813df-5dd4-1f87-ec11-cdb001100000", write, read, notify)]
    status: bool,
}


#[embassy_executor::task]
pub async fn task(controller: ExternalController<BleConnector<'static>, 20>) -> ! {
    info!("[ BLE ] Started async task");
    let addr = Address::random([0xff, 0x8f, 0x1a, 0x05, 0xe4, 0xff]);
    info!("BLE: address = {:?}", Debug2Format(&addr));

    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(addr);

    let Host {
        mut peripheral,
        runner,
        ..
    } = stack.build();

    info!("BLE: Starting advertising and GATT service");

    let config = GapConfig::Peripheral(PeripheralConfig {
        appearance: &appearance::sensor::TEMPERATURE_SENSOR,
        name: "ESP sensor",
    });

    let server = Server::new_with_config(config).unwrap();

    let _ = embassy_futures::join::join(ble_task(runner), async {
        loop {
            match advertise("ESP32 text instance", &mut peripheral, &server).await {
                Ok(conn) => {
                    // set up tasks when the connection is established to a central, so they don't run when no one is connected.
                    let task_a = gatt_events_task(&server, &conn);
                    let task_b = custom_task(&server, &conn, &stack);

                    // run until any task ends (usually because the connection has been closed),
                    // then return to advertising state.
                    select(task_a, task_b).await;
                }
                Err(_) => todo!(),
            }
        }
    })
    .await;

    loop {}
}

/// This is a background task that is required to run forever alongside any other BLE tasks.
///
/// ## Alternative
///
/// If you didn't require this to be generic for your application, you could statically spawn this with i.e.
///
/// ```rust,ignore
///
/// #[embassy_executor::task]
/// async fn ble_task(mut runner: Runner<'static, SoftdeviceController<'static>>) {
///     runner.run().await;
/// }
///
/// spawner.must_spawn(ble_task(runner));
/// ```
async fn ble_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) -> ! {
    loop {
        if let Err(err) = runner.run().await {
            error!("BLE: runner error: {}", Debug2Format(&err));
            Timer::after_secs(2).await;
        }
    }
}

/// Create an advertiser to use to connect to a BLE Central, and wait for it to connect.
async fn advertise<'values, 'server, C: Controller>(
    name: &'values str,
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut advertiser_data = [0; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ServiceUuids16(&[[0x0f, 0x18]]),
            AdStructure::CompleteLocalName(name.as_bytes()),
        ],
        &mut advertiser_data[..],
    )?;

    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &advertiser_data[0..len],
                scan_data: &[],
            },
        )
        .await?;

    info!("BLE: Advertizing");
    let conn = advertiser.accept().await?.with_attribute_server(server)?;

    info!("BLE: connection established");
    Ok(conn)
}

/// Stream Events until the connection closes.
///
/// This function will handle the GATT events and process them.
/// This is how we interact with read and write requests.
async fn gatt_events_task<P: PacketPool>(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, P>,
) -> Result<(), Error> {
    let level = &server.battery_service.level;
    
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => break reason,
            // GattConnectionEvent::PhyUpdated { tx_phy, rx_phy } => todo!(),
            // GattConnectionEvent::ConnectionParamsUpdated { conn_interval, peripheral_latency, supervision_timeout } => todo!(),
            // GattConnectionEvent::RequestConnectionParams { min_connection_interval, max_connection_interval, max_latency, supervision_timeout } => todo!(),
            // GattConnectionEvent::DataLengthUpdated { max_tx_octets, max_tx_time, max_rx_octets, max_rx_time } => todo!(),
            GattConnectionEvent::Gatt { event } => {
                match &event {
                    GattEvent::Read(event) => {
                        if event.handle() == level.handle {
                            let value = server.get(level);
                            info!("GATT: Read Event to Level Characteristic: {:?}", Debug2Format(&value));
                        }
                    }
                    GattEvent::Write(event) => {
                        if event.handle() == level.handle {
                            info!(
                                "[ GATT ] Write Event to Level Characteristic: {:?}",
                                event.data()
                            );
                        }
                    }
                    // GattEvent::Other(other_event) => todo!(),
                    _ => {}
                };

                // This step is also performed at drop(), but writing it explicitly is necessary
                // in order to ensure reply is sent.
                match event.accept() {
                    Ok(reply) => reply.send().await,
                    Err(err) => warn!("[ GATT ] error sending response: {:?}", Debug2Format(&err)),
                }
            }
            _ => {}
        }
    };

    info!("[ GATT ] disconnected: {:?}", reason);
    Ok(())
}

/// Example task to use the BLE notifier interface.
/// This task will notify the connected central of a counter value every 2 seconds.
/// It will also read the RSSI value every 2 seconds.
/// and will stop when the connection is closed by the central or an error occurs.
async fn custom_task<C: Controller, P: PacketPool>(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, P>,
    stack: &Stack<'_, C, P>,
) {
    let mut tick: u8 = 0;
    let level = server.battery_service.level;

    loop {
        tick = tick.wrapping_add(1);
        info!("[custom_task] notifying connection of tick {}", tick);

        if level.notify(conn, &tick).await.is_err() {
            info!("[custom_task] error notifying connection");
            break;
        }

        // read RSSI (Received Signal Strength Indicator) of the connection.
        if let Ok(rssi) = conn.raw().rssi(stack).await {
            info!("[custom_task] RSSI: {:?}", rssi);
        } else {
            info!("[custom_task] error getting RSSI");
            break;
        };

        Timer::after_secs(2).await;
    }
}
