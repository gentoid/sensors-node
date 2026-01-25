#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_futures::select;
use embassy_net::StackResources;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use esp_hal::clock::CpuClock;
use esp_hal::i2c;
use esp_hal::peripherals::Peripherals;
use esp_hal::rmt::Rmt;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal_smartled::{SmartLedsAdapter, smart_led_buffer};
use esp_radio::{ble::controller::BleConnector, wifi};
use esp_rtos::main;
use panic_rtt_target as _;
use sensors_node_core::{ble, led, net_time, system, web, kv_storage};
use static_cell::StaticCell;

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

static RADIO: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, wifi::WifiDevice<'static>>) -> ! {
    runner.run().await;
}

#[embassy_executor::task]
pub async fn led_task() -> ! {
    let mut led_buf = smart_led_buffer!(1);
    let peripherals = unsafe { Peripherals::steal() };

    let mut led = {
        let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).unwrap();
        let led = SmartLedsAdapter::new(rmt.channel0, peripherals.GPIO8, &mut led_buf);
        sensors_node_core::led::Status::new(led)
    };

    let mut state = system::State::default();

    loop {
        match select::select(system::STATE.wait(), led::pattern(&mut led, &state)).await {
            select::Either::First(new_state) => state = new_state,
            select::Either::Second(_) => {}
        }
    }
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

    spawner.must_spawn(led_task());
    system::set_state(system::State::Booting);

    let radio_init =
        RADIO.init(esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller"));

    let (wifi_controller, interfaces) =
        esp_radio::wifi::new(radio_init, peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");

    info!("[ BLE ] Setting up");
    // find more examples https://github.com/embassy-rs/trouble/tree/main/examples/esp32
    let transport = BleConnector::new(radio_init, peripherals.BT, Default::default()).unwrap();
    let ble_controller = trouble_host::prelude::ExternalController::<_, 20>::new(transport);

    spawner.must_spawn(ble::task(ble_controller));

    

    let mut kv_db = match kv_storage::init(peripherals.FLASH).await {
        Ok(db) => db,
        Err(err) => panic!("Couldn't initialize storage. It won't be available. Error: {:?}", err),
    };

    const WIFI_SSID: &'static str = env!("WIFI_SSID");
    const WIFI_PASSWORD: &'static str = env!("WIFI_PASSWORD");

    spawner.must_spawn(sensors_node_core::wifi::task(
        wifi_controller,
        WIFI_SSID,
        WIFI_PASSWORD,
    ));

    let net_config = embassy_net::Config::dhcpv4(Default::default());

    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        net_config,
        RESOURCES.init(StackResources::new()),
        embassy_time::Instant::now().as_millis(),
    );

    spawner.must_spawn(net_task(runner));

    system::set_state(system::State::WifiConnecting);
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

    info!("Starting up web-server");
    let web_app = {
        static WEB_APP_STATIC: StaticCell<web::WebApp> = StaticCell::new();
        WEB_APP_STATIC.init(web::WebApp::new(kv_db))
    };

    for task_id in 0..web::WEB_TASK_POOL_SIZE {
        spawner.must_spawn(web::task(task_id, stack, web_app.router, web_app.config));
    }

    info!("Setting up I2C");
    let i2c = i2c::master::I2c::new(peripherals.I2C0, i2c::master::Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO0)
        .with_scl(peripherals.GPIO1)
        .into_async();

    spawner.must_spawn(sensors_node_core::sensors::task(i2c));

    system::set_state(system::State::Ok);
    loop {
        let forever = embassy_sync::signal::Signal::<NoopRawMutex, ()>::new();
        forever.wait().await;
    }
}
