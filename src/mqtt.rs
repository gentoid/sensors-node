use core::fmt::Write;
use defmt::{info, warn};
use embassy_net::{Stack, tcp};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;
use heapless::String;
use rust_mqtt::{
    Bytes,
    buffer::AllocBuffer,
    client::{
        Client,
        options::{ConnectOptions, DisconnectOptions, PublicationOptions},
    },
    config::SessionExpiryInterval,
    types::{MqttString, QoS, TopicName},
};

use crate::{sensors, wifi};

pub static READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static DOWN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[embassy_executor::task]
pub async fn mqtt_task(stack: Stack<'static>) -> ! {
    info!("Setting up MQTT client");

    let broker_addr = smoltcp::wire::IpAddress::v4(192, 168, 1, 11);
    let broker_port = 1883;

    let mut backoff = 1u64;

    loop {
        sensors::HAS_DATA.wait().await;
        wifi::UP.wait().await;

        info!("Establishing TCP connection...");
        let mut rx_buf = [0u8; 1024];
        let mut tx_buf = [0u8; 1024];
        let mut tcp_socket = tcp::TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);

        tcp_socket.set_timeout(Some(embassy_time::Duration::from_secs(5)));

        match tcp_socket.connect((broker_addr, broker_port)).await {
            Ok(_) => info!("Connected to MQTT by IP/TCP"),
            Err(err) => {
                warn!("Error connecting IP/TCP: {}", err);
                Timer::after_secs(backoff).await;
                backoff = (backoff * 2).min(30);
                continue;
            }
        }

        let mut buffer = AllocBuffer;
        let mut mqtt_client: Client<'_, tcp::TcpSocket<'_>, AllocBuffer, 4, 4, 4> =
            Client::new(&mut buffer);

        let options = ConnectOptions {
            clean_start: true,
            keep_alive: rust_mqtt::config::KeepAlive::Seconds(30),
            password: None,
            session_expiry_interval: SessionExpiryInterval::default(),
            user_name: None,
            will: None,
        };

        info!("MQTT: connecting...");
        match mqtt_client
            .connect(
                tcp_socket,
                &options,
                Some(MqttString::from_slice("esp32s3-test").unwrap()),
            )
            .await
        {
            Ok(info) => {
                info!("Connected to broker: {}", info);
                READY.signal(());
                backoff = 1;

                let sample = {
                    let mut queue = sensors::QUEUE.lock().await;
                    queue.dequeue()
                };

                if let Some(sample) = sample {
                    let mut data = String::<256>::new();
                    write!(
                        data,
                        "{{ \"temperature\": {}, \"pressure\": {}, \"humidity\": {}, \"gas_ohm\": {}, \"lux\": {}, \"aiq_score\": {} }}",
                        sample.temperature,
                        sample.pressure,
                        sample.humidity,
                        sample.gas_ohm,
                        sample.lux,
                        sample.aiq_score,
                    ).ok();
                    mqtt_client
                        .publish(
                            &PublicationOptions {
                                qos: QoS::AtLeastOnce,
                                retain: true,
                                topic: unsafe {
                                    TopicName::new_unchecked(MqttString::from_slice_unchecked(
                                        "sensors/living_room/esp-01/all",
                                    ))
                                },
                            },
                            Bytes::Borrowed(&data.as_bytes()),
                        )
                        .await
                        .ok();
                    info!("MQTT: data published");
                };

                mqtt_client
                    .disconnect(&DisconnectOptions {
                        publish_will: true,
                        session_expiry_interval: None,
                    })
                    .await
                    .ok();

                info!("MQTT: disconnected");
            }
            Err(err) => {
                warn!("Could not connect to MQTT: {}", err);
                DOWN.signal(());
                Timer::after_secs(backoff).await;
                backoff = (backoff * 2).min(30);
            }
        }
    }
}
