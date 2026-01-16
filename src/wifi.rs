use defmt::{error, info};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{WifiController, WifiError};
use smoltcp::iface::Interface;

pub fn print_wifi_error(err: WifiError) {
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
