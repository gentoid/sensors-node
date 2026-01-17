use core::fmt::Write;
use defmt::{info, warn};
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_net::{Stack, tcp};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Ticker, Timer};
use heapless::String;
use rust_mqtt::{
    Bytes,
    buffer::AllocBuffer,
    client::{
        Client, MqttError, event::Event, options::{ConnectOptions, PublicationOptions}
    },
    config::{KeepAlive, SessionExpiryInterval},
    types::{MqttString, QoS, TopicName},
};

use crate::{net_time::TIME_STATE, sensors, wifi};

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
            keep_alive: KeepAlive::Seconds(30),
            password: None,
            session_expiry_interval: SessionExpiryInterval::default(),
            user_name: None,
            will: None,
        };

        if let Err(err) = mqtt_client
            .connect(
                tcp_socket,
                &options,
                Some(MqttString::from_slice("esp32s3-test-2").unwrap()),
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

        let mut ticker = Ticker::every(Duration::from_secs(15));

        loop {
            match select3(sensors::HAS_DATA.wait(), ticker.next(), mqtt_client.poll()).await {
                Either3::First(_) => {
                    info!("Got data to send");
                }
                Either3::Second(_) => {
                    info!("keep alive ping");
                    if let Err(err) = mqtt_client.ping().await {
                        warn!("MQTT: ping error: {}", err);
                        DOWN.signal(());
                        break;
                    }
                }
                Either3::Third(poll) => {
                    info!("Poll response");
                    ticker.reset();
                    match poll {
                        Ok(event) => match event {
                            Event::Pingresp => info!("MQTT resp: Pingresp"),
                            Event::Publish(publish) => info!("MQTT resp: Publish"),
                            Event::Suback(suback) => info!("MQTT resp: Suback"),
                            Event::Unsuback(suback) => info!("MQTT resp: Unsuback"),
                            Event::PublishRejected(pubrej) => info!("MQTT resp: PublishRejected"),
                            Event::PublishAcknowledged(puback) => info!("MQTT resp: PublishAcknowledged"),
                            Event::PublishReceived(puback) => info!("MQTT resp: PublishReceived"),
                            Event::PublishReleased(puback) => info!("MQTT resp: PublishReleased"),
                            Event::PublishComplete(puback) => info!("MQTT resp: PublishComplete"),
                            Event::Ignored => info!("MQTT resp: Ignored"),
                        },
                        Err(err) => todo!(),
                    }
                }
            }

            // 2. Check if we have data
            let sample = {
                let mut queue = sensors::QUEUE.lock().await;
                queue.dequeue()
            };

            if let Some(sample) = sample {
                let mut payload = String::<256>::new();
                let ts = {
                    let time = TIME_STATE.lock().await;
                    time.now().unwrap_or(0)
                };

                write!(
                    payload,
                    "{{ \"ts\": {}, \"temperature\": {}, \"pressure\": {}, \"humidity\": {}, \"gas_ohm\": {}, \"lux\": {}, \"aiq_score\": {} }}",
                    ts,
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
                                    "sensors/living_room/esp-02/all",
                                ))
                            },
                        },
                        Bytes::Borrowed(payload.as_bytes()),
                    )
                    .await
                {
                    warn!("MQTT: publish failed: {}", err);
                    // @todo keep the data
                    break; // reconnect
                }

                ticker.reset();
                info!("MQTT: data published");
            }

            // 3. Don't busy-loop
            Timer::after_millis(10).await;
        }

        info!("MQTT disconnected, retrying...");
    }
}
