#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]
#![feature(ip_from)]

use core::cell::RefCell;

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use esp_hal::clock::CpuClock;
use esp_hal::i2c;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::WifiDevice;
use esp_rtos::main;
use sensors_node::{net_time, storage};
use static_cell::StaticCell;
use {esp_backtrace as _, esp_println as _};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

static RADIO: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, WifiDevice<'static>>) -> ! {
    runner.run().await;
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
async fn main(spawner: Spawner) -> ! {
    info!("Starting up");
    // generator version: 1.2.0

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz);
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    // COEX needs more RAM - so we've added some more
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    let radio_init =
        RADIO.init(esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller"));

    let (wifi_controller, interfaces) =
        esp_radio::wifi::new(radio_init, peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");

    if let Err(err) = storage::init(peripherals.FLASH).await {
        warn!("Couldn't initialize storage. It won't be available. Error: {}", err);
    } else {
        // @todo spawn a storage task
    };

    spawner.must_spawn(sensors_node::wifi::task(wifi_controller));

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

    spawner.must_spawn(sensors_node::mqtt::task(stack));

    // let _connector = BleConnector::new(&radio_init, peripherals.BT, Default::default());

    info!("Setting up I2C for BME680");
    let i2c_bme680 = i2c::master::I2c::new(peripherals.I2C0, i2c::master::Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO1)
        .with_scl(peripherals.GPIO2);

    let i2c_bme680 = RefCell::new(i2c_bme680);

    info!("Setting up I2C for BH1750");
    let i2c_bh1750 = i2c::master::I2c::new(peripherals.I2C1, i2c::master::Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO42)
        .with_scl(peripherals.GPIO41);

    spawner.must_spawn(sensors_node::sensors::task(i2c_bme680, i2c_bh1750));

    loop {
        let forever = embassy_sync::signal::Signal::<NoopRawMutex, ()>::new();
        forever.wait().await;
    }
}
