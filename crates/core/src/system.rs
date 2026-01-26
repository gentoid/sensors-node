use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;

pub static STATE: Signal<CriticalSectionRawMutex, State> = Signal::new();
pub static NEED_REBOOT: AtomicBool = AtomicBool::new(false);

#[derive(Default, defmt::Format)]
pub enum State {
    #[default]
    Booting,
    Ble,
    WifiConnecting,
    Dhcp,
    NtpSync,
    MqttConnecting,
    Sensors,
    Ok,
    Panic,
}

pub fn set_state(state: State) {
    STATE.signal(state);
}

#[embassy_executor::task]
pub async fn reboot_on_request() -> ! {
    loop{
        if NEED_REBOOT.load(Ordering::SeqCst) {
            Timer::after_millis(500).await;
            esp_hal::system::software_reset();
        }

        Timer::after_secs(1).await;
    }
}
