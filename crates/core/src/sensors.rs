use core::cell::RefCell;

use bh1750::BH1750;
use bme680::{Bme680, I2CAddress, IIRFilterSize, PowerMode, SettingsBuilder};
use defmt::{error, info, warn};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use embedded_hal_bus::i2c::RefCellDevice;
use esp_hal::{Async, delay::Delay, i2c};
use heapless::spsc::Queue;
use serde::{Deserialize, Serialize};
use uom::si::{pressure::hectopascal, thermodynamic_temperature::degree_celsius};

use crate::{air_quality, net_time};

pub static HAS_DATA: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static QUEUE: mutex::Mutex<CriticalSectionRawMutex, Queue<Sample, 64>> =
    mutex::Mutex::new(Queue::new());

#[derive(Default, Serialize, Deserialize)]
enum SampleVersion {
    #[default]
    V1,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Sample {
    version: SampleVersion,
    pub timestamp: u32,
    pub temperature: Option<f32>,
    pub pressure: Option<f32>,
    pub humidity: Option<f32>,
    pub hum_sht40: Option<f32>,
    pub temp_sht40: Option<f32>,
    pub press_bmp390: Option<f32>,
    pub temp_bmp390: Option<f32>,
    pub gas_ohm: Option<u32>,
    pub lux_veml7700: Option<f32>,
    pub lux_bh1750: Option<f32>,
    pub aiq_score: Option<u32>,
}

type I2C<'a> = i2c::master::I2c<'a, Async>;
type RefCellDevI2C<'a> = RefCellDevice<'a, I2C<'a>>;

#[embassy_executor::task]
pub async fn task(i2c: I2C<'static>) -> ! {
    let refcell_i2c = RefCell::new(i2c);

    Timer::after(Duration::from_secs(1)).await;

    let mut veml = if check_i2c_address(&refcell_i2c, 0x10).await {
        info!("I2C: VEML7700 detected");
        create_veml7700(&refcell_i2c)
    } else {
        None
    };
    
    // Timer::after(Duration::from_secs(1)).await;

    let mut sht40 = create_sht40(&refcell_i2c);

    // Timer::after(Duration::from_secs(1)).await;

    let mut bme680 = if check_i2c_address(&refcell_i2c, 0x76).await {
        info!("I2C: BME680 detected");
        create_bme680(&refcell_i2c)
    } else {
        None
    };

    // Timer::after(Duration::from_secs(1)).await;

    let mut bh1750 = if check_i2c_address(&refcell_i2c, 0x23).await {
        info!("I2C: BH1750 detected");
        create_bh1750(&refcell_i2c)
    } else {
        None
    };

    // Timer::after(Duration::from_secs(1)).await;

    let mut bmp390 = create_bmp390(&refcell_i2c);

    // Timer::after(Duration::from_secs(1)).await;

    let mut skip: u8 = 10;

    loop {
        let start = Instant::now();

        let lux_veml7700 = veml.as_mut().and_then(|device| match device.read_lux() {
            Ok(lux) => Some(lux),
            Err(_) => {
                warn!("Could not read value out of VEML7700");
                None
            }
        });

        let bme680_data = bme680.as_mut().and_then(|(bme, delayer)| {
            // bme.set_sensor_mode(&mut delayer, PowerMode::ForcedMode)
            //     .ok()?;
            let (data, _state) = bme.get_sensor_data(delayer).ok()?;

            Some((
                data.humidity_percent(),
                data.pressure_hpa(),
                data.temperature_celsius(),
                data.gas_resistance_ohm(),
            ))
        });

        let lux_bh1750 = bh1750
            .as_mut()
            .and_then(|bh| bh.get_one_time_measurement(bh1750::Resolution::High2).ok());

        let sht40_data = sht40.as_mut().and_then(|(device, delay)| {
            device
                .measure(sht4x::Precision::High, delay)
                .inspect_err(|err| warn!("Could not measure with SHT40: {}", err))
                .ok()
        });

        let bmp390_data = bmp390.as_mut().and_then(|device| device.measure().ok());

        if skip > 0 {
            skip -= 1;
            info!("Skip measurement. {} more to skip", skip);
            Timer::after(embassy_time::Duration::from_secs(3)).await;
            continue;
        }

        let timestamp = { net_time::TIME_STATE.lock().await.now_or_uptime() };

        let mut sample = Sample {
            timestamp,
            lux_bh1750,
            lux_veml7700,
            ..Default::default()
        };

        bme680_data.map(|data| {
            let (aiq_score, _) = air_quality::calculate(data.0, data.3);

            sample.humidity = Some(data.0);
            sample.pressure = Some(data.1);
            sample.temperature = Some(data.2);
            sample.aiq_score = Some(aiq_score);
            sample.gas_ohm = Some(data.3);
        });

        sht40_data.map(|data| {
            sample.hum_sht40 = Some(data.humidity_milli_percent() as f32 / 1000.0);
            sample.temp_sht40 = Some(data.temperature_milli_celsius() as f32 / 1000.0);
        });

        bmp390_data.map(|data| {
            sample.temp_bmp390 = Some(data.temperature.get::<degree_celsius>());
            sample.press_bmp390 = Some(data.pressure.get::<hectopascal>());
        });

        {
            let mut queue = QUEUE.lock().await;
            queue.enqueue(sample).ok();
        }
        HAS_DATA.signal(());

        let delay = embassy_time::Duration::from_secs(60) - (Instant::now() - start);

        Timer::after(delay).await;
    }
}

async fn check_i2c_address<'a>(i2c: &RefCell<I2C<'a>>, addr: u8) -> bool {
    Timer::after(Duration::from_secs(1)).await;

    let mut data = [0u8; 22];
    i2c.borrow_mut()
        .write_read(addr, &[0x00], &mut data)
        .map_err(|err| warn!("I2C: Error scanning at 0x{:X}: {}", addr, err))
        .ok()
        .is_some()
}

fn create_veml7700<'a>(i2c: &'a RefCell<I2C<'a>>) -> Option<veml7700::Veml7700<RefCellDevI2C<'a>>> {
    let mut veml = veml7700::Veml7700::new(RefCellDevice::new(i2c));

    veml.set_integration_time(veml7700::IntegrationTime::_100ms)
        .ok()?;
    veml.set_gain(veml7700::Gain::OneQuarter).ok()?;

    if let Err(_err) = veml.enable() {
        warn!("Could not enable VEML7700");
        None
    } else {
        Some(veml)
    }
}

fn create_bme680<'a>(
    i2c: &'a RefCell<I2C<'a>>,
) -> Option<(
    Bme680<RefCellDevI2C<'a>, esp_hal::delay::Delay>,
    esp_hal::delay::Delay,
)> {
    info!("Setting up BME680");
    let mut delayer = esp_hal::delay::Delay::new();
    let mut bme = Bme680::init(RefCellDevice::new(i2c), &mut delayer, I2CAddress::Primary)
        .map_err(bme680_error)
        .ok()?;

    info!("Setting up settings for BME680");
    let settings = SettingsBuilder::new()
        .with_temperature_oversampling(bme680::OversamplingSetting::OS2x)
        .with_pressure_oversampling(bme680::OversamplingSetting::OS4x)
        .with_humidity_oversampling(bme680::OversamplingSetting::OS2x)
        .with_temperature_filter(IIRFilterSize::Size3)
        .with_gas_measurement(core::time::Duration::from_millis(150), 320, 21)
        .with_run_gas(true)
        .build();

    bme.set_sensor_settings(&mut delayer, settings).ok()?;

    info!("Setting forced power modes");
    bme.set_sensor_mode(&mut delayer, PowerMode::ForcedMode)
        .ok()?;

    Some((bme, delayer))
}

fn bme680_error(err: bme680::Error<esp_hal::i2c::master::Error>) {
    match err {
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
    }
}

fn create_bh1750<'a>(
    i2c: &'a RefCell<I2C<'a>>,
) -> Option<BH1750<RefCellDevI2C<'a>, esp_hal::delay::Delay>> {
    let delayer = esp_hal::delay::Delay::new();
    let bh1750 = BH1750::new(RefCellDevice::new(&i2c), delayer, false);

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

    Some(bh1750)
}

fn create_sht40<'a>(
    i2c: &'a RefCell<I2C<'a>>,
) -> Option<(sht4x::Sht4x<RefCellDevI2C<'a>, Delay>, Delay)> {
    let mut delay = Delay::new();

    for addr in [
        sht4x::Address::Address0x44,
        sht4x::Address::Address0x45,
        sht4x::Address::Address0x46,
    ] {
        let mut sht40 = sht4x::Sht4x::new_with_address(RefCellDevice::new(&i2c), addr);
        if sht40.serial_number(&mut delay).is_ok() {
            info!("I2C: SHT40 detected at 0x{:X}", u8::from(addr));
            return Some((sht40, delay));
        }
    }

    None
}

fn create_bmp390<'a>(i2c: &'a RefCell<I2C<'a>>) -> Option<bmp390::sync::Bmp390<RefCellDevI2C<'a>>> {
    use bmp390::{Address, Configuration, sync::Bmp390};

    let delay = esp_hal::delay::Delay::new();
    let config = Configuration::default();

    for addr in [Address::Up, Address::Down] {
        let sensor = Bmp390::try_new(RefCellDevice::new(i2c), addr, delay, &config).ok();

        if sensor.is_some() {
            info!("I2C: BMP390 detected");
            return sensor;
        }
    }

    None
}
