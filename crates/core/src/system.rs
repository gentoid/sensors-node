use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};

pub static STATE: Signal<CriticalSectionRawMutex, State> = Signal::new();

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
