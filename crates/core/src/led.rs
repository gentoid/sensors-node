use defmt::info;
use embassy_time::Timer;
use esp_hal_smartled::SmartLedsAdapter;
use rgb::Grb;
use smart_leds::RGB8;

use crate::system;

pub struct Status<L>
where
    L: smart_leds::SmartLedsWrite<Color = Grb<u8>>,
{
    led: L,
    buf: [RGB8; 1],
}

impl<L> Status<L>
where
    L: smart_leds::SmartLedsWrite<Color = Grb<u8>>,
{
    pub fn new(led: L) -> Self {
        Self {
            led,
            buf: [RGB8::default()],
        }
    }

    pub fn off(&mut self) {
        self.set(0, 0, 0);
    }

    pub fn set(&mut self, r: u8, g: u8, b: u8) {
        self.buf[0] = RGB8 { r, g, b };
        let _ = self.led.write(self.buf.iter().copied());
    }
}

pub async fn pattern<const BUFFER_SIZE: usize>(
    led: &mut Status<SmartLedsAdapter<'_, BUFFER_SIZE>>,
    state: &system::State,
) -> ! {
    match state {
        system::State::Booting => pattern_connecting(led).await,
        system::State::WifiConnecting => pattern_connecting(led).await,
        system::State::Dhcp => pattern_connecting(led).await,
        system::State::NtpSync => pattern_ok(led).await,
        system::State::MqttConnecting => pattern_connecting(led).await,
        system::State::Ok => pattern_ok(led).await,
        system::State::Panic => pattern_connecting(led).await,
        system::State::Ble => pattern_ok(led).await,
        system::State::Sensors => pattern_connecting(led).await,
    }
}

async fn pattern_ok<const BUFFER_SIZE: usize>(
    led: &mut Status<SmartLedsAdapter<'_, BUFFER_SIZE>>,
) -> ! {
    let rnd = esp_hal::rng::Rng::new();
    loop {
        let c1 = rnd.random();
        let c2 = rnd.random();
        let c1 = c1 as f32 / u32::MAX as f32;
        let c2 = c2 as f32 / 2.0 / u32::MAX as f32;
        let c2 = 1.0 - c2;

        let r = rnd.random();
        let r = (r as f32 / u32::MAX as f32) * c1 * c2;

        for b in 0..16 {
            let b = (b as f32 / 16.0) * (1.0 - c1) * c2;
            blink_with_blue(led, b, r, c2).await;
        }

        for b in 0..16 {
            let b = ((16 - b) as f32 / 16.0) * (1.0 - c1) * c2;
            blink_with_blue(led, b, r, c2).await;
        }
    }
}

async fn blink_with_blue<const BUFFER_SIZE: usize>(
    led: &mut Status<SmartLedsAdapter<'_, BUFFER_SIZE>>,
    b: f32,
    r: f32,
    c: f32,
) {
    for g in 0..16 {
        set_led(led, r, g, b, c);
        Timer::after_millis(80).await;
    }

    for g in 0..16 {
        set_led(led, r, 16 - g, b, c);
        Timer::after_millis(80).await;
    }
}

fn set_led<const BUFFER_SIZE: usize>(
    led: &mut Status<SmartLedsAdapter<'_, BUFFER_SIZE>>,
    r: f32,
    g: u8,
    b: f32,
    c: f32,
) {
    let i = g as f32;
    let g = (i * c) as u8;
    let r = (r * i / 2.0) as u8;
    let b = (b * i / 4.0) as u8;
    led.set(r, g, b);
}

async fn pattern_connecting<const BUFFER_SIZE: usize>(
    led: &mut Status<SmartLedsAdapter<'_, BUFFER_SIZE>>,
) -> ! {
    loop {
        for i in 0u8..255 {
            led.set((i.wrapping_mul(2)).min(u8::MAX) / 6, i / 6, i / 24);
            Timer::after_millis(20).await;
        }
    }
}
