use core::fmt::Write;
use core::net::Ipv4Addr;
use defmt::{Debug2Format, info, warn};
use embassy_futures::join::join3;
use embassy_futures::select;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Stack, tcp};
use embassy_sync::channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use heapless::String;

use mqtt_client::packet::QoS;
use mqtt_client::time::EmbassyClock;
use mqtt_client::{ConnectOptions, Event, PublishMsg, SubscribeOptions};
use static_cell::StaticCell;

use crate::{Command, config, kv_storage, sensors, wifi};

extern crate alloc;

type MqttClient<'c, 't> = mqtt_client::Client<'c, EmbassyClock, TcpSocket<'t>, 1, 4, 1, 4>;

type SampleSender = Sender<'static, CriticalSectionRawMutex, sensors::Sample, PUBLISH_QUEUE_SIZE>;
type SampleReceiver =
    Receiver<'static, CriticalSectionRawMutex, sensors::Sample, PUBLISH_QUEUE_SIZE>;

type CommandSender = Sender<'static, CriticalSectionRawMutex, Command, SUBSCRIBE_QUEUE_SIZE>;
type CommandReceiver = Receiver<'static, CriticalSectionRawMutex, Command, SUBSCRIBE_QUEUE_SIZE>;

pub static READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static DOWN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

const PUBLISH_QUEUE_SIZE: usize = 8;
const SUBSCRIBE_QUEUE_SIZE: usize = 8;
const PUBLISH_BURST: usize = 4;
const IO_POLL_TIMEOUT_MS: u64 = 6_000;
const CONNECT_TIMEOUT_SECS: u64 = 10;

static PUBLISH_QUEUE: Channel<CriticalSectionRawMutex, sensors::Sample, PUBLISH_QUEUE_SIZE> =
    Channel::new();
static SUBSCRIBE_QUEUE: Channel<CriticalSectionRawMutex, Command, SUBSCRIBE_QUEUE_SIZE> =
    Channel::new();

static COMMANDS_TOPIC_BASE: &'static str = "sensors/command";

#[embassy_executor::task]
pub async fn task(
    db: &'static kv_storage::Db,
    stack: Stack<'static>,
    broker_addr: Ipv4Addr,
    client_id: &'static str,
    topic: &'static str,
) -> ! {
    info!("MQTT task started");

    let publish_sender = PUBLISH_QUEUE.sender();
    let publish_receiver = PUBLISH_QUEUE.receiver();

    let subscribe_sender = SUBSCRIBE_QUEUE.sender();
    let subscribe_receiver = SUBSCRIBE_QUEUE.receiver();

    join3(
        publisher_loop(publish_sender),
        command_execution_loop(db, subscribe_receiver),
        mqtt_loop(stack, broker_addr, client_id, topic, publish_receiver, subscribe_sender),
    )
    .await;

    unreachable!()
}
async fn command_execution_loop(db: &'static kv_storage::Db, receiver: CommandReceiver) -> ! {
    loop {
        match receiver.receive().await {
            Command::RebootToReconfigure => {
                info!("Reboot requested");
                if let Err(err) = config::set_reboot(db).await {
                    warn!("Could not set settings to reboot: {:?}", err);
                };
            }
        }
    }
}

async fn publisher_loop(sender: SampleSender) -> ! {
    loop {
        sensors::HAS_DATA.wait().await;

        while let Some(sample) = { sensors::QUEUE.lock().await.dequeue() } {
            match sender.try_send(sample) {
                Ok(()) => {}
                Err(TrySendError::Full(sample)) => {
                    warn!("MQTT: publish queue full, waiting for capacity");
                    sender.send(sample).await;
                }
            }
        }
    }
}

fn command_topic(client_id: &str) -> alloc::string::String {
    alloc::format!("{COMMANDS_TOPIC_BASE}/{client_id}")
}

async fn mqtt_loop(
    stack: Stack<'static>,
    broker_addr: Ipv4Addr,
    client_id: &'static str,
    topic: &'static str,
    publish_receiver: SampleReceiver,
    command_sender: CommandSender,
) -> ! {
    let broker_port = 1883;
    let keep_alive_secs: u16 = 120;

    let mut backoff = 1u64;

    let cmd_topic: &'static alloc::string::String = {
        static CMD_TOPIC: StaticCell<alloc::string::String> = StaticCell::new();
        CMD_TOPIC.init(command_topic(client_id))
    };

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

        let rx_buf = &mut [0u8; 1024];
        let tx_buf = &mut [0u8; 1024];

        let clock = mqtt_client::time::EmbassyClock::default();
        let keep_alive = mqtt_client::time::KeepAlive::from_sec(keep_alive_secs as u64);

        let mut client: MqttClient =
            mqtt_client::Client::try_new(clock, keep_alive, tcp_socket, rx_buf, tx_buf).unwrap();

        if let Err(err) = client.schedule_connect(options) {
            warn!("MQTT: connect failed: {:?}", Debug2Format(&err));
            Timer::after_secs(backoff).await;
            backoff = (backoff * 2).min(30);

            continue;
        }

        if let Err(err) = wait_for_connect(&mut client).await {
            warn!("MQTT: connect poll error: {:?}", Debug2Format(&err));
            Timer::after_secs(backoff).await;
            backoff = (backoff * 2).min(30);

            continue;
        };

        info!("MQTT: connected");
        READY.signal(());
        backoff = 1;

        let subscribe_options = SubscribeOptions {
            qos: Some(QoS::AtMostOnce),
            topic: &cmd_topic,
        };

        if let Err(err) = client.schedule_subscribe(subscribe_options) {
            warn!("Error when subscribe scheduled: {:?}", err);
        }

        'connected: loop {
            if let Err(err) = client.poll_timers() {
                warn!("MQTT poll timers error: {:?}", Debug2Format(&err));
                DOWN.signal(());
                break;
            }

            match select::select(
                publish_receiver.receive(),
                poll_io_with_timeout(&mut client),
            )
            .await
            {
                select::Either::First(sample) => {
                    if !publish_sample(&mut client, topic, sample).await {
                        // @todo put sample back, or is it ok to drop it?
                        DOWN.signal(());
                        break;
                    }

                    for _ in 0..PUBLISH_BURST {
                        match publish_receiver.try_receive() {
                            Ok(sample) => {
                                if !publish_sample(&mut client, topic, sample).await {
                                    // @todo put sample back, or is it ok to drop it?
                                    DOWN.signal(());
                                    break 'connected;
                                }
                            }
                            Err(TryReceiveError::Empty) => break,
                        }
                    }
                }
                select::Either::Second(poll) => {
                    if !handle_poll_result(client_id, poll, command_sender) {
                        DOWN.signal(());
                        break;
                    }
                }
            }
        }

        info!("MQTT disconnected, retrying...");
    }
}

async fn wait_for_connect(client: &mut MqttClient<'_, '_>) -> Result<(), mqtt_client::Error> {
    let deadline = Instant::now() + Duration::from_secs(CONNECT_TIMEOUT_SECS);

    loop {
        if Instant::now() >= deadline {
            return Err(mqtt_client::Error::TimedOut);
        }

        match poll_io_with_timeout(client).await? {
            Some(Event::Connected) => return Ok(()),
            Some(Event::Disconnected) => return Err(mqtt_client::Error::TransportError),
            _ => {}
        }
    }
}

async fn poll_io_with_timeout<'a>(
    client: &'a mut MqttClient<'_, '_>,
) -> Result<Option<Event<'a>>, mqtt_client::Error> {
    match select::select(client.poll_io(), Timer::after_millis(IO_POLL_TIMEOUT_MS)).await {
        select::Either::First(event) => event,
        select::Either::Second(_) => Ok(None),
    }
}

async fn publish_sample(
    client: &mut MqttClient<'_, '_>,
    topic: &'static str,
    sample: sensors::Sample,
) -> bool {
    let payload = build_payload(&sample);

    let msg = PublishMsg {
        qos: QoS::AtLeastOnce,
        retain: false,
        topic,
        payload: payload.as_bytes(),
    };

    if let Err(err) = client.schedule_publish(msg) {
        warn!("MQTT: publish failed: {:?}", Debug2Format(&err));

        let result = { sensors::QUEUE.lock().await.enqueue(sample) };

        match result {
            Ok(()) => {}
            Err(_sample) => {
                warn!("Could not put sample back to the queue");
            }
        }

        return false;
    }

    true
}

fn handle_poll_result(
    client_id: &str,
    poll_result: Result<Option<Event<'_>>, mqtt_client::Error>,
    sender: CommandSender,
) -> bool {
    match poll_result {
        Ok(Some(event)) => match event {
            Event::Connected => info!("MQTT: connected"),
            Event::Received(msg) => {
                info!("MQTT: message received: {:?}", msg);

                let cmd_topic = command_topic(client_id);
                if msg.topic.as_bytes() == cmd_topic.as_bytes() {
                    match Command::try_from(msg) {
                        Ok(command) => {
                            if let Err(err) = sender.try_send(command) {
                                warn!("Could not apply command: {:?}", err);
                            }
                        }
                        Err(err) => warn!("Error while converting payload to Command: {:?}", err),
                    }
                } else {
                    warn!("Unknown packet arrived: {:?}", msg);
                }
            }
            Event::Subscribed => info!("MQTT: subscribed"),
            Event::SubscribeFailed => warn!("MQTT: subscribe failed"),
            Event::Unsubscribed => info!("MQTT: unsubscribed"),
            Event::Published => info!("MQTT: published"),
            Event::Disconnected => {
                warn!("MQTT: disconnected");
                return false;
            }
        },
        Ok(None) => {}
        Err(err) => {
            warn!("MQTT poll error: {:?}", Debug2Format(&err));
            return false;
        }
    }

    true
}

fn build_payload(sample: &sensors::Sample) -> String<256> {
    let mut payload = String::<256>::new();

    write!(payload, "{{\"ts\":{}", sample.timestamp).ok();
    sample.temp_bme680.inspect(|value| {
        write!(payload, ",\"temp_bme680\":{}", value).ok();
    });
    sample.press_bme680.inspect(|value| {
        write!(payload, ",\"press_bme680\":{}", value).ok();
    });
    sample.hum_bme680.inspect(|value| {
        write!(payload, ",\"hum_bme680\":{}", value).ok();
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
    write!(payload, "}}").ok();

    payload
}
