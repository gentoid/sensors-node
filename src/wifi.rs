use defmt::{error, info, warn};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;
use esp_radio::wifi::{ClientConfig, PowerSaveMode, WifiError};

pub static UP: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static DOWN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[embassy_executor::task]
pub async fn task(mut wifi: esp_radio::wifi::WifiController<'static>) -> ! {
    setup(&mut wifi).await;

    let mut backoff = 1u64;

    loop {
        if wifi.is_connected().ok().unwrap_or_default() {
            UP.signal(());
            backoff = 1;
            Timer::after_secs(5).await;
            continue;
        }

        info!("WiFi: connecting...");
        match wifi.connect_async().await {
            Ok(_) => {
                info!("WiFI: connected");
                UP.signal(());
                backoff = 1;
            }
            Err(err) => {
                warn!("WiFi error: {:?}", err);
                Timer::after_secs(backoff).await;
                backoff = (backoff * 2).min(30);
            }
        }
    }
}

async fn setup(wifi: &mut esp_radio::wifi::WifiController<'static>) {
    info!("Setting up WiFi");
    let wifi_config = esp_radio::wifi::ModeConfig::Client(
        ClientConfig::default()
            .with_ssid(env!("WIFI_SSID").into())
            .with_password(env!("WIFI_PASSWORD").into())
            .with_failure_retry_cnt(3),
    );

    info!("  Setting up WiFi power saving");
    if let Err(err) = wifi.set_power_saving(PowerSaveMode::None) {
        print_wifi_error(err);
    };

    if let Err(err) = wifi.set_config(&wifi_config) {
        print_wifi_error(err);
    };

    info!("  Starting up the WiFi controller");
    if let Err(err) = wifi.start_async().await {
        print_wifi_error(err);
    } else {
        info!("  Started: {}", wifi.is_started().ok());
    }
}

fn print_wifi_error(err: WifiError) {
    match err {
        esp_radio::wifi::WifiError::NotInitialized => {
            error!("WiFi error: NotInitialized")
        }
        esp_radio::wifi::WifiError::InternalError(err) => {
            error!("WiFi error: InternalError");
            match err {
                esp_radio::wifi::InternalWifiError::NoMem => error!("  => NoMem"),
                esp_radio::wifi::InternalWifiError::InvalidArg => error!("  => InvalidArg"),
                esp_radio::wifi::InternalWifiError::NotInit => error!("  => NotInit"),
                esp_radio::wifi::InternalWifiError::NotStarted => error!("  => NotStarted"),
                esp_radio::wifi::InternalWifiError::NotStopped => error!("  => NotStopped"),
                esp_radio::wifi::InternalWifiError::Interface => error!("  => Interface"),
                esp_radio::wifi::InternalWifiError::Mode => error!("  => Mode"),
                esp_radio::wifi::InternalWifiError::State => error!("  => State"),
                esp_radio::wifi::InternalWifiError::Conn => error!("  => Conn"),
                esp_radio::wifi::InternalWifiError::Nvs => error!("  => Nvs"),
                esp_radio::wifi::InternalWifiError::InvalidMac => error!("  => InvalidMac"),
                esp_radio::wifi::InternalWifiError::InvalidSsid => error!("  => InvalidSsid"),
                esp_radio::wifi::InternalWifiError::InvalidPassword => {
                    error!("  => InvalidPassword")
                }
                esp_radio::wifi::InternalWifiError::Timeout => error!("  => Timeout"),
                esp_radio::wifi::InternalWifiError::WakeFail => error!("  => WakeFail"),
                esp_radio::wifi::InternalWifiError::WouldBlock => error!("  => WouldBlock"),
                esp_radio::wifi::InternalWifiError::NotConnected => error!("  => NotConnected"),
                esp_radio::wifi::InternalWifiError::PostFail => error!("  => PostFail"),
                esp_radio::wifi::InternalWifiError::InvalidInitState => {
                    error!("  => InvalidInitState")
                }
                esp_radio::wifi::InternalWifiError::StopState => error!("  => StopState"),
                esp_radio::wifi::InternalWifiError::NotAssociated => error!("  => NotAssociated"),
                esp_radio::wifi::InternalWifiError::TxDisallowed => error!("  => TxDisallowed"),
                _ => error!("  => Unknown error"),
            }
        }
        esp_radio::wifi::WifiError::Disconnected => error!("WiFi error: Disconnected"),
        esp_radio::wifi::WifiError::UnknownWifiMode => {
            error!("WiFi error: UnknownWifiMode")
        }
        esp_radio::wifi::WifiError::Unsupported => error!("WiFi error: Unsupported"),
        esp_radio::wifi::WifiError::InvalidArguments => {
            error!("WiFi error: InvalidArguments")
        }
        _ => error!("WiFi error: Unknown error"),
    }
}
