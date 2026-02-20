#![no_std]
#![no_main]
#![feature(addr_parse_ascii)]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::cell::RefCell;
use core::net::Ipv4Addr;

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_net::{Runner, StackResources};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::Timer;
use esp_hal::clock::CpuClock;
use esp_hal::i2c;
use esp_hal::peripherals::Peripherals;
use esp_hal::rmt::Rmt;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal_smartled::{SmartLedsAdapter, smart_led_buffer};
use esp_radio::wifi::AccessPointConfig;
use esp_radio::{
    ble::controller::BleConnector,
    wifi::{self, WifiController, WifiDevice},
};
use esp_rtos::main;
use panic_rtt_target as _;
use sensors_node_core::config::{self, SettingsEnum};
use sensors_node_core::wifi::print_wifi_error;
use sensors_node_core::{
    ble,
    config::{Settings, get_initial_settings},
    kv_storage, led, net_time, system, web,
};
use sensors_node_core::{dhcp, display, sensors};
use static_cell::StaticCell;

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

static RADIO: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static RESOURCES: StaticCell<StackResources<16>> = StaticCell::new();
static FLASH_KV_START: usize = 0x600_000;

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, wifi::WifiDevice<'static>>) -> ! {
    runner.run().await;
}

#[embassy_executor::task]
pub async fn led_task() -> ! {
    let mut led_buf = smart_led_buffer!(1);
    let peripherals = unsafe { Peripherals::steal() };

    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).unwrap();
    let led = SmartLedsAdapter::new(rmt.channel0, peripherals.GPIO8, &mut led_buf);

    led::run(led).await
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

    info!("Setting up I2C");
    let i2c = i2c::master::I2c::new(peripherals.I2C0, i2c::master::Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO0)
        .with_scl(peripherals.GPIO1)
        .into_async();

    let i2c: &'static RefCell<sensors::I2C> = {
        static I2C_STATIC: StaticCell<RefCell<sensors::I2C>> = StaticCell::new();
        I2C_STATIC.init(RefCell::new(i2c))
    };
    spawner.must_spawn(display(&i2c));

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

    let kv_db = match kv_storage::init(peripherals.FLASH, FLASH_KV_START).await {
        Ok(db) => db,
        Err(err) => panic!(
            "Couldn't initialize storage. It won't be available. Error: {:?}",
            err
        ),
    };

    match get_initial_settings(kv_db).await {
        Ok(settings) => match settings {
            SettingsEnum::Optional(settings) => {
                init_start(
                    spawner,
                    wifi_controller,
                    interfaces.ap,
                    kv_db,
                    SettingsEnum::Optional(settings),
                )
                .await
            }
            SettingsEnum::FilledIn(settings) => {
                info!("###    WiFi SSID:        {}", settings.wifi_ssid);
                info!("###    MQTT broker:      {}", settings.mqtt_broker);
                info!("###    MQTT client id:   {}", settings.mqtt_client_id);
                info!("###    MQTT topic:       {}", settings.mqtt_topic);
                info!(
                    "###    Reconfigure:      {:?}",
                    settings.reboot_to_reconfigure
                );

                if settings.reboot_to_reconfigure {
                    init_start(
                        spawner,
                        wifi_controller,
                        interfaces.ap,
                        kv_db,
                        SettingsEnum::FilledIn(settings),
                    )
                    .await
                } else {
                    run(
                        spawner,
                        kv_db,
                        wifi_controller,
                        interfaces.sta,
                        &i2c,
                        settings,
                    )
                    .await
                }
            }
        },

        Err(err) => panic!("Could not get initial settings: {:?}", err),
    }
}

#[embassy_executor::task]
async fn display(i2c: &'static RefCell<sensors::I2C<'static>>) {
    display::run(i2c).await;
}

async fn run(
    spawner: Spawner,
    db: &'static kv_storage::Db,
    wifi_controller: WifiController<'static>,
    device: WifiDevice<'static>,
    i2c: &'static RefCell<sensors::I2C<'static>>,
    settings: Settings,
) -> ! {
    let settings = {
        static SETTINGS_STATIC: StaticCell<Settings> = StaticCell::new();
        SETTINGS_STATIC.init(settings)
    };

    spawner.must_spawn(sensors_node_core::wifi::task(
        wifi_controller,
        settings.wifi_ssid.as_str(),
        settings.wifi_password.as_str(),
    ));

    let net_config = embassy_net::Config::dhcpv4(Default::default());

    let (stack, runner) = embassy_net::new(
        device,
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

    let broker_address = match Ipv4Addr::parse_ascii(settings.mqtt_broker.as_bytes()) {
        Err(err) => {
            warn!("Error parsing broker IP: {}", err);
            config::set_reboot(db).await.unwrap();
            unreachable!();
        }
        Ok(address) => address,
    };

    spawner.must_spawn(sensors_node_core::mqtt::task(
        db,
        stack,
        broker_address,
        settings.mqtt_client_id.as_str(),
        settings.mqtt_topic.as_str(),
    ));

    spawner.must_spawn(sensors_node_core::sensors::task(i2c));

    system::set_state(system::State::Ok);
    loop {
        let forever = embassy_sync::signal::Signal::<NoopRawMutex, ()>::new();
        forever.wait().await;
    }
}

async fn init_start(
    spawner: Spawner,
    mut wifi_controller: WifiController<'static>,
    device: WifiDevice<'static>,
    kv_db: &'static kv_storage::Db,
    settings: SettingsEnum,
) -> ! {
    let net_config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(Ipv4Addr::new(192, 168, 1, 1), 24),
        dns_servers: heapless_08::Vec::new(),
        gateway: None,
    });

    let ap_config = AccessPointConfig::default().with_ssid("esp32-setup".into());

    let _ = wifi_controller.set_config(&wifi::ModeConfig::AccessPoint(ap_config));

    let (stack, runner) = embassy_net::new(
        device,
        net_config,
        RESOURCES.init(StackResources::new()),
        embassy_time::Instant::now().as_millis(),
    );

    spawner.must_spawn(net_task(runner));

    loop {
        info!("Starting WIFI");
        if let Err(err) = wifi_controller.start_async().await {
            print_wifi_error(err);
            Timer::after_secs(5).await;
        } else {
            break;
        }
    }

    spawner.must_spawn(dhcp_task(stack));

    info!("Waiting for link...");
    stack.wait_link_up().await;
    info!("  Link is up!");

    info!("Waiting for DHCP...");
    stack.wait_config_up().await;
    info!("  IPv4 config: {:?}", stack.config_v4());

    spawner.must_spawn(system::reboot_on_request());

    info!("Starting up web-server");
    let web_app = {
        static WEB_APP_STATIC: StaticCell<web::WebApp> = StaticCell::new();
        WEB_APP_STATIC.init(web::WebApp::new(kv_db, settings))
    };

    for task_id in 0..web::WEB_TASK_POOL_SIZE {
        spawner.must_spawn(web::task(task_id, stack, web_app.router, web_app.config));
    }

    loop {
        let forever = embassy_sync::signal::Signal::<NoopRawMutex, ()>::new();
        forever.wait().await;
    }
}

#[embassy_executor::task]
async fn dhcp_task(stack: embassy_net::Stack<'static>) -> ! {
    let buffers = edge_nal_embassy::UdpBuffers::<2, 1024, 1024, 8>::new();
    let unbound_socket = edge_nal_embassy::Udp::new(stack, &buffers);

    dhcp::run(unbound_socket).await
}
