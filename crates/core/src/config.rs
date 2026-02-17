use heapless::String;
use serde::{Deserialize, Serialize};

use crate::kv_storage;

static WIFI_SSID_KEY: &'static str = "wifi.ssid";
static WIFI_PASSWORD_KEY: &'static str = "wifi.password";
static MQTT_BROKER_KEY: &'static str = "mqtt.broker";
static MQTT_CLIENT_ID_KEY: &'static str = "mqtt.client_id";
static MQTT_TOPIC_KEY: &'static str = "mqtt.topic";

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Settings {
    pub wifi_ssid: String<32>,
    pub wifi_password: String<64>,
    pub mqtt_broker: String<64>,
    pub mqtt_client_id: String<32>,
    pub mqtt_topic: String<64>,
}

pub async fn get_initial_settings<'a>(
    db: &'static kv_storage::Db,
) -> kv_storage::DbResult<Settings> {
    let mut tx = db.read_transaction().await;

    Ok(Settings {
        wifi_ssid: kv_storage::read_string(&mut tx, WIFI_SSID_KEY).await?,
        wifi_password: kv_storage::read_string(&mut tx, WIFI_PASSWORD_KEY).await?,
        mqtt_broker: kv_storage::read_string(&mut tx, MQTT_BROKER_KEY).await?,
        mqtt_client_id: kv_storage::read_string(&mut tx, MQTT_CLIENT_ID_KEY).await?,
        mqtt_topic: kv_storage::read_string(&mut tx, MQTT_TOPIC_KEY).await?,
    })
}

pub async fn save_settings(
    db: &'static kv_storage::Db,
    settings: &Settings,
) -> kv_storage::DbResult<()> {
    let mut tx = db.write_transaction().await;

    kv_storage::write_string(&mut tx, MQTT_BROKER_KEY, &settings.mqtt_broker).await?;
    kv_storage::write_string(&mut tx, MQTT_CLIENT_ID_KEY, &settings.mqtt_client_id).await?;
    kv_storage::write_string(&mut tx, MQTT_TOPIC_KEY, &settings.mqtt_topic).await?;
    kv_storage::write_string(&mut tx, WIFI_PASSWORD_KEY, &settings.wifi_password).await?;
    kv_storage::write_string(&mut tx, WIFI_SSID_KEY, &settings.wifi_ssid).await?;

    tx.commit().await?;

    Ok(())
}
