use heapless::String;
use serde::Deserialize;

use crate::kv_storage;

static WIFI_SSID_KEY: &'static str = "wifi.ssid";
static WIFI_PASSWORD_KEY: &'static str = "wifi.password";
static MQTT_BROKER_KEY: &'static str = "mqtt.broker";
static MQTT_CLIENT_ID_KEY: &'static str = "mqtt.client_id";
static MQTT_TOPIC_KEY: &'static str = "mqtt.topic";
static SYSTEM_REBOOT_TO_RECONFIGURE: &'static str = "system.reconfig";

pub struct OptionalSettings {
    pub wifi_ssid: Option<String<32>>,
    pub wifi_password: Option<String<64>>,
    pub mqtt_broker: Option<String<64>>,
    pub mqtt_client_id: Option<String<32>>,
    pub mqtt_topic: Option<String<64>>,
    pub reboot_to_reconfigure: Option<bool>,
}

impl OptionalSettings {
    pub fn needs_reconfiguration(&self) -> bool {
        !self.is_complete() || self.reboot_to_reconfigure.unwrap_or(false)
    }

    fn is_complete(&self) -> bool {
        self.wifi_ssid.is_some()
            && self.wifi_password.is_some()
            && self.mqtt_broker.is_some()
            && self.mqtt_client_id.is_some()
            && self.mqtt_topic.is_some()
    }
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub wifi_ssid: String<32>,
    pub wifi_password: String<64>,
    pub mqtt_broker: String<64>,
    pub mqtt_client_id: String<32>,
    pub mqtt_topic: String<64>,
    pub reboot_to_reconfigure: bool,
}

pub enum SettingsEnum {
    Optional(OptionalSettings),
    FilledIn(Settings),
}

impl SettingsEnum {
    pub fn transmute(self) -> Self {
        match self {
            Self::Optional(settings) => {
                if !settings.is_complete() {
                    return Self::Optional(settings);
                }

                if let Some(wifi_ssid) = settings.wifi_ssid
                    && let Some(wifi_password) = settings.wifi_password
                    && let Some(mqtt_broker) = settings.mqtt_broker
                    && let Some(mqtt_client_id) = settings.mqtt_client_id
                    && let Some(mqtt_topic) = settings.mqtt_topic
                {
                    return Self::FilledIn(Settings {
                        wifi_ssid,
                        wifi_password,
                        mqtt_broker,
                        mqtt_client_id,
                        mqtt_topic,
                        reboot_to_reconfigure: settings.reboot_to_reconfigure.unwrap_or(false),
                    });
                }

                unreachable!()
            }
            Self::FilledIn(settings) => Self::Optional(OptionalSettings {
                wifi_ssid: Some(settings.wifi_ssid),
                wifi_password: Some(settings.wifi_password),
                mqtt_broker: Some(settings.mqtt_broker),
                mqtt_client_id: Some(settings.mqtt_client_id),
                mqtt_topic: Some(settings.mqtt_topic),
                reboot_to_reconfigure: Some(settings.reboot_to_reconfigure),
            }),
        }
    }

    pub fn to_filled_in_with_default(self) -> Settings {
        match self {
            Self::Optional(settings) => Settings {
                wifi_ssid: settings.wifi_ssid.unwrap_or_default(),
                wifi_password: settings.wifi_password.unwrap_or_default(),
                mqtt_broker: settings.mqtt_broker.unwrap_or_default(),
                mqtt_client_id: settings.mqtt_client_id.unwrap_or_default(),
                mqtt_topic: settings.mqtt_topic.unwrap_or_default(),
                reboot_to_reconfigure: settings.reboot_to_reconfigure.unwrap_or_default(),
            },
            Self::FilledIn(settings) => settings,
        }
    }
}

pub async fn get_initial_settings<'a>(
    db: &'static kv_storage::Db,
) -> kv_storage::DbResult<SettingsEnum> {
    let mut tx = db.read_transaction().await;
    let settings = SettingsEnum::Optional(OptionalSettings {
        wifi_ssid: kv_storage::read_string(&mut tx, WIFI_SSID_KEY).await?,
        wifi_password: kv_storage::read_string(&mut tx, WIFI_PASSWORD_KEY).await?,
        mqtt_broker: kv_storage::read_string(&mut tx, MQTT_BROKER_KEY).await?,
        mqtt_client_id: kv_storage::read_string(&mut tx, MQTT_CLIENT_ID_KEY).await?,
        mqtt_topic: kv_storage::read_string(&mut tx, MQTT_TOPIC_KEY).await?,
        reboot_to_reconfigure: kv_storage::read_bool(&mut tx, SYSTEM_REBOOT_TO_RECONFIGURE).await?,
    })
    .transmute();

    Ok(settings)
}

pub async fn save_settings(
    db: &'static kv_storage::Db,
    settings: &Settings,
) -> kv_storage::DbResult<()> {
    let mut tx = db.write_transaction().await;

    kv_storage::write_string(&mut tx, MQTT_BROKER_KEY, &settings.mqtt_broker).await?;
    kv_storage::write_string(&mut tx, MQTT_CLIENT_ID_KEY, &settings.mqtt_client_id).await?;
    kv_storage::write_string(&mut tx, MQTT_TOPIC_KEY, &settings.mqtt_topic).await?;
    kv_storage::write_bool(
        &mut tx,
        SYSTEM_REBOOT_TO_RECONFIGURE,
        settings.reboot_to_reconfigure,
    )
    .await?;
    kv_storage::write_string(&mut tx, WIFI_PASSWORD_KEY, &settings.wifi_password).await?;
    kv_storage::write_string(&mut tx, WIFI_SSID_KEY, &settings.wifi_ssid).await?;

    tx.commit().await?;

    Ok(())
}

pub async fn set_reboot(db: &'static kv_storage::Db) -> kv_storage::DbResult<()> {
    let mut tx = db.write_transaction().await;
    kv_storage::write_bool(&mut tx, SYSTEM_REBOOT_TO_RECONFIGURE, true).await?;
    tx.commit().await?;

    esp_hal::system::software_reset();
}
