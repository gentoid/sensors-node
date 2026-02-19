use core::cell::RefCell;

use alloc::format; 
use alloc::string::String; 
use alloc::vec::Vec; 
use embedded_graphics::mono_font::{self, MonoTextStyleBuilder};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::Point;
use embedded_graphics::{Drawable, text};
use ssd1306::mode::{BufferedGraphicsMode, DisplayConfig};
use ssd1306::prelude::{DisplayRotation, I2CInterface};
use ssd1306::size::DisplaySize128x32;

extern crate alloc;
use crate::sensors;

struct Display<'a> {
    display: ssd1306::Ssd1306<
        I2CInterface<sensors::RefCellDevI2C<'a>>,
        DisplaySize128x32,
        BufferedGraphicsMode<DisplaySize128x32>,
    >,
    text_style: mono_font::MonoTextStyle<'a, BinaryColor>,
}

impl<'a> Display<'a> {
    pub fn new(i2c: &'a RefCell<sensors::I2C<'a>>) -> Self {
        let interface = ssd1306::I2CDisplayInterface::new(sensors::RefCellDevice::new(i2c));
        let mut display =
            ssd1306::Ssd1306::new(interface, DisplaySize128x32, DisplayRotation::Rotate0)
                .into_buffered_graphics_mode();

        display.init().unwrap();

        let text_style = MonoTextStyleBuilder::new()
            .font(&mono_font::ascii::FONT_8X13)
            .text_color(BinaryColor::On)
            .build();

        Self {
            display,
            text_style,
        }
    }

    pub fn line_one(&mut self, val1: &str, val2: Option<&str>) {
        text::Text::with_baseline(val1, Point::zero(), self.text_style, text::Baseline::Top)
            .draw(&mut self.display)
            .ok();

        val2.map(|val| {
            text::Text::with_baseline(val, Point::new(64, 0), self.text_style, text::Baseline::Top)
                .draw(&mut self.display)
                .ok()
        });
    }

    pub fn line_two(&mut self, val1: &str, val2: Option<&str>) {
        text::Text::with_baseline(
            val1,
            Point::new(0, 16),
            self.text_style,
            text::Baseline::Top,
        )
        .draw(&mut self.display)
        .ok();

        val2.map(|val| {
            text::Text::with_baseline(
                val,
                Point::new(64, 16),
                self.text_style,
                text::Baseline::Top,
            )
            .draw(&mut self.display)
            .ok()
        });
    }

    pub fn flush(&mut self) {
        self.display.flush().ok();
    }

    pub fn clear_buffer(&mut self) {
        self.display.clear_buffer();
    }
}

pub async fn run(i2c: &'static RefCell<sensors::I2C<'static>>) {
    let mut display = Display::new(i2c);

    display.line_one("Loading", None);
    display.flush();

    loop {
        let sample = sensors::LATEST_SAMPLE.wait().await;
        let mut values: Vec<String> = Vec::new();

        sample
            .temp_sht40
            .or_else(|| sample.temp_bmp390)
            .or_else(|| sample.temperature)
            .inspect(|val| values.push(format!("T {:4.2}", val)));
        sample
            .hum_sht40
            .or_else(|| sample.humidity)
            .inspect(|val| values.push(format!("H {:4.2}", val)));
        sample
            .lux_veml7700
            .or_else(|| sample.lux_bh1750)
            .inspect(|val| values.push(format!("L {:4.2}", val)));
        sample
            .press_bmp390
            .or_else(|| sample.pressure)
            .inspect(|val| values.push(format!("P {:4.2}", val)));

        display.clear_buffer();
        display.flush();

        if !values.is_empty() {
            display.line_one(values[0].as_str(), values.get(1).map(|v| v.as_str()));
        } else {
            display.line_one("---", Some("---"));
        }

        if values.len() > 2 {
            display.line_two(values[2].as_str(), values.get(3).map(|v| v.as_str()));
        } else {
            display.line_two("---", Some("---"));
        }

        display.flush();
    }
}
