use core::fmt::Write;
use defmt::{info, warn};
use embassy_futures::select::{Either3, select3};
use embassy_net::{Stack, tcp};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Ticker, Timer};
use heapless::String;
use rust_mqtt::{
    Bytes,
    buffer::AllocBuffer,
    client::{
        Client,
        options::{ConnectOptions, PublicationOptions},
    },
    config::{KeepAlive, SessionExpiryInterval},
    types::{MqttString, QoS, TopicName},
};

use crate::{sensors, wifi};

pub static READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static DOWN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[embassy_executor::task]
pub async fn task(stack: Stack<'static>) -> ! {
    info!("MQTT task started");

    let broker_addr = smoltcp::wire::IpAddress::v4(192, 168, 1, 11);
    let broker_port = 1883;

    let mut backoff = 1u64;

    loop {
        info!("MQTT: waiting for WiFi...");
        wifi::UP.wait().await;
        info!("MQTT: WiFi is up");

        let mut rx_buf = [0u8; 1024];
        let mut tx_buf = [0u8; 1024];
        let mut tcp_socket = tcp::TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);

        tcp_socket.set_timeout(None);

        if let Err(err) = tcp_socket.connect((broker_addr, broker_port)).await {
            warn!("MQTT: TCP connect failed: {}", err);
            Timer::after_secs(backoff).await;
            backoff = (backoff * 2).min(30);
            continue;
        }

        info!("MQTT: TCP connected. Connecting to broker...");

        let mut buffer = AllocBuffer;
        let mut mqtt_client: Client<'_, tcp::TcpSocket<'_>, AllocBuffer, 4, 4, 4> =
            Client::new(&mut buffer);

        let options = ConnectOptions {
            clean_start: true,
            keep_alive: KeepAlive::Seconds(120),
            password: None,
            session_expiry_interval: SessionExpiryInterval::default(),
            user_name: None,
            will: None,
        };

        if let Err(err) = mqtt_client
            .connect(
                tcp_socket,
                &options,
                Some(MqttString::from_slice("esp32s3-test").unwrap()),
            )
            .await
        {
            warn!("MQTT: connect failed: {}", err);
            Timer::after_secs(backoff).await;
            backoff = (backoff * 2).min(30);
            continue;
        }

        info!("MQTT: connected");
        READY.signal(());
        backoff = 1;

        let mut ticker = Ticker::every(Duration::from_secs(90));

        loop {
            match select3(sensors::HAS_DATA.wait(), ticker.next(), mqtt_client.poll()).await {
                Either3::First(_) => {}
                Either3::Second(_) => {
                    info!("keep alive ping");
                    if let Err(err) = mqtt_client.ping().await {
                        warn!("MQTT ping error: {}", err);
                        DOWN.signal(());
                        break;
                    }
                }
                Either3::Third(poll) => match poll {
                    Ok(resp) => {
                        info!("Poll response: {}", resp);
                        ticker.reset();
                    }
                    Err(err) => {
                        warn!("MQTT poll error: {}", err);
                        DOWN.signal(());
                        break;
                    }
                },
            }

            loop {
                let sample = { sensors::QUEUE.lock().await.dequeue() };

                if let Some(sample) = sample {
                    let mut payload = String::<256>::new();

                    write!(
                        payload,
                        "{{ \"ts\": {}, \"temperature\": {}, \"pressure\": {}, \"humidity\": {}, \"gas_ohm\": {}, \"lux\": {}, \"aiq_score\": {} }}",
                        sample.timestamp,
                        sample.temperature,
                        sample.pressure,
                        sample.humidity,
                        sample.gas_ohm,
                        sample.lux,
                        sample.aiq_score,
                    ).ok();

                    if let Err(err) = mqtt_client
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
                            Bytes::Borrowed(payload.as_bytes()),
                        )
                        .await
                    {
                        warn!("MQTT: publish failed: {}", err);
                        {
                            let mut queue = sensors::QUEUE.lock().await;
                            if let Err(_) = queue.enqueue(sample) {
                                warn!("Could not put sample back to the queue");
                            };
                        }
                        break; // reconnect
                    }

                    ticker.reset();
                    info!("MQTT: data published");
                } else {
                    break;
                }
            }
        }

        info!("MQTT disconnected, retrying...");
    }
}
