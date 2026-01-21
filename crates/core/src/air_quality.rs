#[derive(defmt::Format)]
pub enum AirQuality {
    Good,
    Moderate,
    UnhealthyForSensitiveGroups,
    Unhealthy,
    VeryUnhealthy,
    Hazardous,
}

fn aiq_from_score(score: u32) -> AirQuality {
    match score {
        0..50 => AirQuality::Good,
        50..150 => AirQuality::Moderate,
        150..175 => AirQuality::UnhealthyForSensitiveGroups,
        175..200 => AirQuality::Unhealthy,
        200..300 => AirQuality::VeryUnhealthy,
        _ => AirQuality::Hazardous,
    }
}

pub fn calculate(humidity: f32, gas: u32) -> (u32, AirQuality) {
    const HUM_REF: f32 = 40.0;

    let hum_score: u32 = match humidity {
        0.0..38.0 => 25 * (humidity / HUM_REF) as u32,
        38.0..=42.0 => 25,
        _ => 41 + 25 * (humidity / (100.0 - HUM_REF)) as u32,
    };

    const GAS_LOWER_LIMIT: u32 = 5000;
    const GAS_UPPER_LIMIT: u32 = 50000;
    const GAS_LIMITS_DIFF: u32 = GAS_UPPER_LIMIT - GAS_LOWER_LIMIT;

    let gas_ref = match gas {
        0..GAS_LOWER_LIMIT => GAS_LOWER_LIMIT,
        GAS_LOWER_LIMIT..GAS_UPPER_LIMIT => GAS_UPPER_LIMIT / 2,
        _ => GAS_UPPER_LIMIT,
    };

    let gas_score = 75 * (gas_ref - GAS_LOWER_LIMIT) / GAS_LIMITS_DIFF;

    let score = hum_score + gas_score;

    (score, aiq_from_score(score))
}
