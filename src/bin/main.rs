#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]
#![feature(ip_from)]

use core::time;
use core::{cell::RefCell, net::Ipv4Addr};

use bh1750::BH1750;
use bme680::{Bme680, I2CAddress, IIRFilterSize, PowerMode, SettingsBuilder};
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_net::{Ipv4Cidr, StackResources, StaticConfigV4};
use embedded_hal_bus::i2c::RefCellDevice;
use esp_hal::clock::CpuClock;
use esp_hal::i2c;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::{AuthMethod, ClientConfig, WifiDevice};
use esp_rtos::main;
use heapless::Vec;
use sensors_node::air_quality;
use sensors_node::wifi::print_wifi_error;
use static_cell::StaticCell;
use {esp_backtrace as _, esp_println as _};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

// struct EspClock;

// impl embedded_time::Clock for EspClock {
//     type T = u32;

//     const SCALING_FACTOR: embedded_time::rate::Fraction = Fraction::new(1, 1);

//     fn try_now(&self) -> Result<embedded_time::Instant<Self>, embedded_time::clock::Error> {
//         Ok(embedded_time::Instant::new(
//             Instant::now().duration_since_epoch().as_secs() as u32,
//         ))
//     }
// }

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

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz);
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    // COEX needs more RAM - so we've added some more
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    let radio_init =
        RADIO.init(esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller"));

    let (mut wifi_controller, interfaces) =
        esp_radio::wifi::new(radio_init, peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");

    info!("Setting up WiFi");
    let wifi_config = esp_radio::wifi::ModeConfig::Client(
        ClientConfig::default()
            .with_ssid(env!("WIFI_SSID").into())
            .with_password(env!("WIFI_PASSWORD").into())
            .with_protocols(esp_radio::wifi::Protocol::P802D11BGN.into())
            .with_auth_method(AuthMethod::Wpa2Wpa3Personal)
            // .with_channel(1)
            .with_scan_method(esp_radio::wifi::ScanMethod::AllChannels),
    );

    info!("  Setting up WiFi power saving");
    if let Err(err) = wifi_controller.set_power_saving(esp_radio::wifi::PowerSaveMode::None) {
        print_wifi_error(err);
    };

    // info!("  Setting up WiFi mode STA");
    // if let Err(err) = wifi_controller.set_mode(esp_radio::wifi::WifiMode::Sta) {
    //     print_wifi_error(err);
    // };

    if let Err(err) = wifi_controller.set_config(&wifi_config) {
        print_wifi_error(err);
    };

    info!("  Starting up the WiFi controller");
    if let Err(err) = wifi_controller.start_async().await {
        print_wifi_error(err);
    } else {
        info!("  Started: {}", wifi_controller.is_started().ok());
    }

    let net_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::from_netmask(
            Ipv4Addr::from_octets([192, 168, 1, 210]),
            Ipv4Addr::from_octets([255, 255, 255, 0]),
        ).unwrap(),
        dns_servers: Vec::new(),
        gateway: None
    });
    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        net_config,
        RESOURCES.init(StackResources::new()),
        embassy_time::Instant::now().as_millis(),
    );

    info!("Connecting to a WiFi network");
    if let Err(err) = wifi_controller.connect_async().await {
        print_wifi_error(err);
    }

    spawner.must_spawn(net_task(runner));

    info!("  Waiting for network...");
    stack.wait_config_up().await;

    info!("Network is up!");
    info!("IP address: {:?}", stack.config_v4());

    // let _connector = BleConnector::new(&radio_init, peripherals.BT, Default::default());

    // static mut SOCKET_STORAGE: [SocketStorage; 8] = [SocketStorage::EMPTY; 8];
    // let mut sockets = unsafe { SocketSet::new(&mut SOCKET_STORAGE[..]) };

    // let dhcp_socket = dhcpv4::Socket::new();
    // let dhcp_handle = sockets.add(dhcp_socket);

    // let wifi_device = interfaces.sta;

    // let iface_config = smoltcp::iface::Config::new(smoltcp::wire::HardwareAddress::Ethernet(
    //     EthernetAddress::from_bytes(&wifi_device.mac_address()),
    // ));

    // let mut iface = smoltcp::iface::Interface::new(
    //     iface_config,
    //     &mut wifi_device,
    //     smoltcp::time::Instant::from_millis(
    //         i64::try_from(Instant::now().duration_since_epoch().as_millis()).unwrap(),
    //     ),
    // );

    //  else {
    //     let delay = Delay::new();
    //     let mut repeats = 20;
    //     while repeats > 0 {
    //         match wifi_controller.is_started() {
    //             Ok(started) => {
    //                 info!("Started: {}", started);
    //                 if started {
    //                     break;
    //                 }
    //             }
    //             Err(err) => print_wifi_error(err),
    //         }
    //         info!("Waiting for starting for {} secs", repeats);
    //         repeats -= 1;
    //         delay.delay_millis(1000);
    //     }
    // };

    // wait_for_wifi(
    //     &mut iface,
    //     &mut wifi_device,
    //     &mut sockets,
    //     &mut wifi_controller,
    //     20,
    // )
    // .await;

    // if let Err(err) = wifi_controller.connect() {
    //     print_wifi_error(err);
    // } else {
    //     let delay = Delay::new();
    //     let mut repeats = 20;
    //     while repeats > 0 {
    //         let now = smoltcp::time::Instant::from_millis(
    //             i64::try_from(Instant::now().duration_since_epoch().as_millis()).unwrap(),
    //         );
    //         iface.poll(now, &mut wifi_device, &mut sockets);

    //         // wifi_controller

    //         {
    //             let dhcp_socket: &mut dhcpv4::Socket = sockets.get_mut(dhcp_handle);
    //             if let Some(event) = dhcp_socket.poll() {
    //                 match event {
    //                     dhcpv4::Event::Deconfigured => info!("DHCP deconfigured"),
    //                     dhcpv4::Event::Configured(config) => info!("DHCP config: {}", config),
    //                 }
    //             } else {
    //                 warn!("DHCP poll failed");
    //             };
    //         }

    //         match wifi_controller.is_connected() {
    //             Ok(connected) => {
    //                 info!("Connected: {}", connected);

    //                 if connected {
    //                     break;
    //                 }
    //             }
    //             Err(err) => print_wifi_error(err),
    //         }

    //         info!("Waiting for connection for {} secs", repeats);
    //         // while delay_start.elapsed() < Duration::from_secs(1) {}
    //         delay.delay_millis(1000);

    //         repeats -= 1;
    //     }
    // };

    // if let (Ok(info), Ok(started), Ok(connected)) = (
    //     wifi_controller.capabilities(),
    //     wifi_controller.is_started(),
    //     wifi_controller.is_connected(),
    // ) {
    //     info!("WiFi capabilities: {}", info);
    //     info!("WiFi started: {}", started);
    //     info!("WiFi connected: {}", connected);
    // } else {
    //     warn!("Error getting WiFi capabilities");
    // }

    // info!("Setting up MQTT client");

    // let stack = NetworkStack::new(iface, wifi_device, sockets, EspClock);
    // let broker = IpBroker::new(core::net::IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)));
    // let mut buffer = [0u8; 1024];
    // let config = ConfigBuilder::new(broker, &mut buffer);
    // minimq::Minimq::new(stack, EspClock, config);

    info!("Setting up I2C for BME680");
    let i2c = i2c::master::I2c::new(peripherals.I2C0, i2c::master::Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO1)
        .with_scl(peripherals.GPIO2);

    let i2c = RefCell::new(i2c);

    info!("Setting up BME680");
    let mut delayer = esp_hal::delay::Delay::new();
    let mut bme_dev = Bme680::init(
        RefCellDevice::new(&i2c),
        &mut delayer,
        I2CAddress::Primary, /* 0x76 */
    )
    .map_err(|err| match err {
        bme680::Error::I2C(err) => {
            error!("BME init error: I2C");
            match err {
                i2c::master::Error::FifoExceeded => error!("  I2C error: FifoExceeded"),
                i2c::master::Error::AcknowledgeCheckFailed(err) => {
                    error!("  I2C error: AcknowledgeCheckFailed");
                    match err {
                        i2c::master::AcknowledgeCheckFailedReason::Address => error!("    Address"),
                        i2c::master::AcknowledgeCheckFailedReason::Data => error!("    Data"),
                        i2c::master::AcknowledgeCheckFailedReason::Unknown => error!("    Unknown"),
                        _ => error!("    ????"),
                    }
                }
                i2c::master::Error::Timeout => error!("  I2C error: Timeout"),
                i2c::master::Error::ArbitrationLost => error!("  I2C error: ArbitrationLost"),
                i2c::master::Error::ExecutionIncomplete => {
                    error!("  I2C error: ExecutionIncomplete")
                }
                i2c::master::Error::CommandNumberExceeded => {
                    error!("  I2C error: CommandNumberExceeded")
                }
                i2c::master::Error::ZeroLengthInvalid => error!("  I2C error: ZeroLengthInvalid"),
                i2c::master::Error::AddressInvalid(i2c_address) => {
                    error!("  I2C error: AddressInvalid: {}", i2c_address)
                }
                _ => todo!(),
            };
        }
        bme680::Error::Delay => error!("BME init error: Delay"),
        bme680::Error::DeviceNotFound => error!("BME init error: DeviceNotFound"),
        bme680::Error::InvalidLength => error!("BME init error: InvalidLength"),
        bme680::Error::DefinePwrMode => error!("BME init error: DefinePwrMode"),
        bme680::Error::NoNewData => error!("BME init error: NoNewData"),
        bme680::Error::BoundaryCheckFailure(_) => error!("BME init error: BoundaryCheckFailure"),
    })
    .unwrap();

    info!("Setting up settings for BME680");
    let settings = SettingsBuilder::new()
        .with_temperature_oversampling(bme680::OversamplingSetting::OS2x)
        .with_pressure_oversampling(bme680::OversamplingSetting::OS4x)
        .with_humidity_oversampling(bme680::OversamplingSetting::OS2x)
        .with_temperature_filter(IIRFilterSize::Size3)
        .with_gas_measurement(time::Duration::from_millis(150), 320, 21)
        .with_run_gas(true)
        .build();

    bme_dev.set_sensor_settings(&mut delayer, settings).unwrap();

    info!("Setting forced power modes");
    bme_dev
        .set_sensor_mode(&mut delayer, PowerMode::ForcedMode)
        .unwrap();

    info!("Setting up I2C for BH1750");
    let i2c_bh1750 = i2c::master::I2c::new(peripherals.I2C1, i2c::master::Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO42)
        .with_scl(peripherals.GPIO41);

    let mut delayer_bh1750 = esp_hal::delay::Delay::new();
    let mut bh1750 = BH1750::new(i2c_bh1750, &mut delayer_bh1750, false);

    info!(
        "Lux measurement time for HIGH2: {} ms",
        bh1750.get_typical_measurement_time_ms(bh1750::Resolution::High2)
    );
    info!(
        "Lux measurement time for HIGH:  {} ms",
        bh1750.get_typical_measurement_time_ms(bh1750::Resolution::High)
    );
    info!(
        "Lux measurement time for LOW:    {} ms",
        bh1750.get_typical_measurement_time_ms(bh1750::Resolution::Low)
    );

    let _ = spawner;

    loop {
        // let delay_start = Instant::now();

        bme_dev
            .set_sensor_mode(&mut delayer, PowerMode::ForcedMode)
            .unwrap();
        let (data, _state) = bme_dev.get_sensor_data(&mut delayer).unwrap();

        let lux = bh1750
            .get_one_time_measurement(bh1750::Resolution::High2)
            .unwrap();

        let humidity = data.humidity_percent();
        let gas = data.gas_resistance_ohm();

        let (aiq_score, aiq) = air_quality::calculate(humidity, gas);
        info!(
            "{{ \"temperature\": {}, \"pressure\": {}, \"humidity\": {}, \"gas_ohm\": {}, \"lux\": {}, \"aiq_score\": {}, \"aiq\": \"{}\" }}",
            data.temperature_celsius(),
            data.pressure_hpa(),
            humidity,
            gas,
            lux,
            aiq_score,
            aiq,
        );
        embassy_time::Timer::after(embassy_time::Duration::from_secs(10)).await;
        // while delay_start.elapsed() < Duration::from_millis(60_000) {}
    }
    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0/examples
}
