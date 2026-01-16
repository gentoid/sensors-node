use core::cell::RefCell;

use bh1750::BH1750;
use bme680::{Bme680, I2CAddress, IIRFilterSize, PowerMode, SettingsBuilder};
use defmt::{error, info};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex, signal::Signal};
use embedded_hal_bus::i2c::RefCellDevice;
use esp_hal::{
    Blocking,
    i2c::{self, master::I2c},
};
use heapless::spsc::Queue;

use crate::air_quality;

pub static HAS_DATA: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static QUEUE: Mutex<CriticalSectionRawMutex, Queue<Sample, 64>> = Mutex::new(Queue::new());

pub struct Sample {
    pub temperature: f32,
    pub pressure: f32,
    pub humidity: f32,
    pub gas_ohm: u32,
    pub lux: f32,
    pub aiq_score: u32,
}

#[embassy_executor::task]
pub async fn sensors_task(
    i2c: RefCell<I2c<'static, Blocking>>,
    i2c_bh1750: I2c<'static, Blocking>,
) -> ! {
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
        .with_gas_measurement(core::time::Duration::from_millis(150), 320, 21)
        .with_run_gas(true)
        .build();

    bme_dev.set_sensor_settings(&mut delayer, settings).unwrap();

    info!("Setting forced power modes");
    bme_dev
        .set_sensor_mode(&mut delayer, PowerMode::ForcedMode)
        .unwrap();

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

    let mut skip: u8 = 10;

    loop {
        let start = embassy_time::Instant::now();

        bme_dev
            .set_sensor_mode(&mut delayer, PowerMode::ForcedMode)
            .unwrap();
        let (data, _state) = bme_dev.get_sensor_data(&mut delayer).unwrap();

        let lux = bh1750
            .get_one_time_measurement(bh1750::Resolution::High2)
            .unwrap();

        if skip > 0 {
            skip -= 1;
            info!("Skip measurement. {} more to skip", skip);
            embassy_time::Timer::after(embassy_time::Duration::from_secs(3)).await;
            continue;
        }

        let humidity = data.humidity_percent();
        let gas_ohm = data.gas_resistance_ohm();

        let (aiq_score, aiq) = air_quality::calculate(humidity, gas_ohm);

        let sample = Sample {
            aiq_score,
            gas_ohm,
            humidity,
            lux,
            pressure: data.pressure_hpa(),
            temperature: data.temperature_celsius(),
        };
        info!(
            "{{ \"temperature\": {}, \"pressure\": {}, \"humidity\": {}, \"gas_ohm\": {}, \"lux\": {}, \"aiq_score\": {}, \"aiq\": \"{}\" }}",
            sample.temperature,
            sample.pressure,
            sample.humidity,
            sample.gas_ohm,
            sample.lux,
            sample.aiq_score,
            aiq,
        );

        {
            let mut queue = QUEUE.lock().await;
            queue.enqueue(sample).ok();
        }
        HAS_DATA.signal(());

        let delay = embassy_time::Duration::from_secs(60) - (embassy_time::Instant::now() - start);

        embassy_time::Timer::after(delay).await;
    }
}
