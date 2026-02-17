use core::fmt::Write;
use defmt::{Debug2Format, info, warn};
use embassy_futures::select::{Either3, select3};
use embassy_net::{Stack, tcp};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Ticker, Timer};
use heapless::String;

use mqtt_client::packet::QoS;
use mqtt_client::{ConnectOptions, Event, PublishMsg};

use crate::{sensors, wifi};

pub static READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static DOWN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[embassy_executor::task]
pub async fn task(stack: Stack<'static>, client_id: &'static str, topic: &'static str) -> ! {
    info!("MQTT task started");

    let broker_addr = smoltcp::wire::IpAddress::v4(192, 168, 1, 11);
    let broker_port = 1883;
    let keep_alive_secs: u16 = 120;

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

        let options = ConnectOptions {
            clean_session: true,
            client_id,
            keep_alive: keep_alive_secs,
            password: None,
            username: None,
            will: None,
        };

        let mut rx_buf = &mut [0u8; 1024];
        let mut tx_buf = &mut [0u8; 1024];

        let clock = mqtt_client::time::EmbassyClock::default();
        let keep_alive = mqtt_client::time::KeepAlive::from_sec(keep_alive_secs as u64);

        let mut mqtt_client = mqtt_client::Client::<_, _, 1, 4, 1, 4>::try_new(
            clock, keep_alive, tcp_socket, rx_buf, tx_buf,
        )
        .unwrap();

        if let Err(err) = mqtt_client.schedule_connect(options) {
            warn!("MQTT: connect failed: {:?}", Debug2Format(&err));
            Timer::after_secs(backoff).await;
            backoff = (backoff * 2).min(30);
            continue;
        }

        let mut connected = false;
        while !connected {
            match mqtt_client.poll().await {
                Ok(Some(Event::Connected)) => {
                    connected = true;
                }
                Ok(Some(Event::Disconnected)) => {
                    warn!("MQTT: disconnected during connect");
                    break;
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(err) => {
                    warn!("MQTT: connect poll error: {:?}", Debug2Format(&err));
                    break;
                }
            }
        }

        if !connected {
            Timer::after_secs(backoff).await;
            backoff = (backoff * 2).min(30);
            continue;
        }

        info!("MQTT: connected");
        READY.signal(());
        backoff = 1;

        let tick_secs = (keep_alive_secs as u64 / 2).max(1);
        let mut ticker = Ticker::every(Duration::from_secs(tick_secs));

        loop {
            match select3(sensors::HAS_DATA.wait(), ticker.next(), mqtt_client.poll()).await {
                Either3::First(_) => {}
                Either3::Second(_) => {}
                Either3::Third(poll) => match poll {
                    Ok(Some(event)) => {
                        match event {
                            Event::Disconnected => {
                                warn!("MQTT: disconnected");
                                DOWN.signal(());
                                break;
                            }
                            Event::Received(_) => {
                                info!("MQTT: message received");
                            }
                            Event::Published => {
                                info!("MQTT: published");
                            }
                            Event::Subscribed => {
                                info!("MQTT: subscribed");
                            }
                            Event::SubscribeFailed => {
                                warn!("MQTT: subscribe failed");
                            }
                            Event::Unsubscribed => {
                                info!("MQTT: unsubscribed");
                            }
                            Event::Connected => {}
                        }

                        ticker.reset();
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!("MQTT poll error: {:?}", Debug2Format(&err));
                        DOWN.signal(());
                        break;
                    }
                },
            }

            loop {
                let sample = { sensors::QUEUE.lock().await.dequeue() };

                if let Some(sample) = sample {
                    let payload = build_payload(&sample);

                    let publish_result = mqtt_client.schedule_publish(PublishMsg {
                        qos: QoS::AtLeastOnce,
                        retain: false,
                        topic,
                        payload: payload.as_bytes(),
                    });

                    let publish_result = match publish_result {
                        Ok(()) => mqtt_client.poll_io().await,
                        Err(err) => Err(err),
                    };

                    if let Err(err) = publish_result {
                        warn!("MQTT: publish failed: {:?}", Debug2Format(&err));

                        let stored = {
                            // if let Some(db) = db.as_mut() {
                            //     db.lock()
                            //         .await
                            //         .store(&sample)
                            //         .await
                            //         .map_err(|err| warn!("Could not write to the DB: {}", err))
                            //         .is_err()
                            // } else {
                            //     false
                            // }
                            false
                        };

                        if stored {
                            info!("Sample has been stored to the DB");
                        } else {
                            let mut queue = sensors::QUEUE.lock().await;
                            if let Err(_sample) = queue.enqueue(sample) {
                                warn!("Could not put sample back to the queue");
                            } else {
                                info!("Sample has been put back to the queue");
                            }
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

fn build_payload(sample: &sensors::Sample) -> String<256> {
    let mut payload = String::<256>::new();

    write!(payload, "{{\"ts\":{}", sample.timestamp).ok();
    sample.temperature.inspect(|value| {
        write!(payload, ",\"temperature\":{}", value).ok();
    });
    sample.pressure.inspect(|value| {
        write!(payload, ",\"pressure\":{}", value).ok();
    });
    sample.humidity.inspect(|value| {
        write!(payload, ",\"humidity\":{}", value).ok();
    });
    sample.gas_ohm.inspect(|value| {
        write!(payload, ",\"gas_ohm\":{}", value).ok();
    });
    sample.lux_bh1750.inspect(|value| {
        write!(payload, ",\"lux_bh1750\":{}", value).ok();
    });
    sample.lux_veml7700.inspect(|value| {
        write!(payload, ",\"lux_veml7700\":{}", value).ok();
    });
    sample.temp_bmp390.inspect(|value| {
        write!(payload, ",\"temp_bmp390\":{}", value).ok();
    });
    sample.press_bmp390.inspect(|value| {
        write!(payload, ",\"press_bmp390\":{}", value).ok();
    });
    sample.hum_sht40.inspect(|value| {
        write!(payload, ",\"hum_sht40\":{}", value).ok();
    });
    sample.temp_sht40.inspect(|value| {
        write!(payload, ",\"temp_sht40\":{}", value).ok();
    });
    sample.aiq_score.inspect(|value| {
        write!(payload, ",\"aiq_score\":{}", value).ok();
    });
    write!(payload, "}}").ok();

    payload
}
