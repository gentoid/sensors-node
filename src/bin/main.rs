#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::cell::RefCell;
use core::time;

use bh1750::BH1750;
use bme680::{Bme680, I2CAddress, IIRFilterSize, PowerMode, SettingsBuilder};
use defmt::{error, info};
use embedded_hal_bus::i2c::RefCellDevice;
use esp_hal::clock::CpuClock;
use esp_hal::time::{Duration, Instant};
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{i2c, main};
use esp_radio::ble::controller::BleConnector;
use sensors_node::air_quality;
use {esp_backtrace as _, esp_println as _};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    info!("Starting up");
    // generator version: 1.2.0

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz);
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    // COEX needs more RAM - so we've added some more
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);
    let radio_init = esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller");
    let (mut _wifi_controller, _interfaces) =
        esp_radio::wifi::new(&radio_init, peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");
    let _connector = BleConnector::new(&radio_init, peripherals.BT, Default::default());

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

    loop {
        let delay_start = Instant::now();

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

        while delay_start.elapsed() < Duration::from_millis(60_000) {}
    }
    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0/examples
}
