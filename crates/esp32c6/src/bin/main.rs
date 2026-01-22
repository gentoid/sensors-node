#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::cell::RefCell;

use defmt::info;
use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use esp_hal::i2c;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{Blocking, clock::CpuClock};
use esp_radio::{ble::controller::BleConnector, wifi};
use esp_rtos::main;
use panic_rtt_target as _;
use sensors_node_core::net_time;
use static_cell::StaticCell;
use trouble_host::prelude::*;

extern crate alloc;

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 1;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

static RADIO: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();
static I2C_BUS: StaticCell<RefCell<i2c::master::I2c<'static, Blocking>>> = StaticCell::new();

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, wifi::WifiDevice<'static>>) -> ! {
    runner.run().await;
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.2.0

    rtt_target::rtt_init_defmt!();

    info!("Starting up");

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);
    // COEX needs more RAM - so we've added some more
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    let radio_init =
        RADIO.init(esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller"));

    let (wifi_controller, interfaces) =
        esp_radio::wifi::new(radio_init, peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");

    // find more examples https://github.com/embassy-rs/trouble/tree/main/examples/esp32
    let transport = BleConnector::new(&radio_init, peripherals.BT, Default::default()).unwrap();
    let ble_controller = ExternalController::<_, 1>::new(transport);
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let _stack = trouble_host::new(ble_controller, &mut resources);

    // let mut db: Option<&'static mut storage::MutexDb> = None;

    // match storage::init(peripherals.FLASH).await {
    //     Ok(db_proxy) => db = Some(db_proxy),
    //     Err(err) => warn!("Couldn't initialize storage. It won't be available. Error: {}", err),
    // }

    const WIFI_SSID: &'static str = env!("WIFI_SSID");
    const WIFI_PASSWORD: &'static str = env!("WIFI_PASSWORD");

    spawner.must_spawn(sensors_node_core::wifi::task(wifi_controller, WIFI_SSID, WIFI_PASSWORD));

    let net_config = embassy_net::Config::dhcpv4(Default::default());

    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        net_config,
        RESOURCES.init(StackResources::new()),
        embassy_time::Instant::now().as_millis(),
    );

    spawner.must_spawn(net_task(runner));

    info!("Waiting for link...");
    stack.wait_link_up().await;
    info!("  Link is up!");

    info!("Waiting for DHCP...");
    stack.wait_config_up().await;
    info!("  IPv4 config: {:?}", stack.config_v4());

    spawner.must_spawn(net_time::sync_task(stack));

    const CLIENT_ID: &'static str = env!("MQTT_CLIENT_ID");
    const MQTT_TOPIC: &'static str = env!("MQTT_TOPIC");

    spawner.must_spawn(sensors_node_core::mqtt::task(stack, CLIENT_ID, MQTT_TOPIC));

    info!("Setting up I2C");
    let i2c = i2c::master::I2c::new(peripherals.I2C0, i2c::master::Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO0)
        .with_scl(peripherals.GPIO1);

    let i2c = I2C_BUS.init(RefCell::new(i2c));

    spawner.must_spawn(sensors_node_core::sensors::task(i2c));

    loop {
        let forever = embassy_sync::signal::Signal::<NoopRawMutex, ()>::new();
        forever.wait().await;
    }
}
