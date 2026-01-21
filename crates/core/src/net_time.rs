use core::{
    net::Ipv4Addr,
    sync::atomic::{AtomicU32, Ordering},
};

use defmt::{info, warn};
use embassy_net::{IpAddress, IpEndpoint, udp::PacketMetadata};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::Instant;

pub static TIME_STATE: Mutex<CriticalSectionRawMutex, TimeState> = Mutex::new(TimeState::new());

pub struct TimeState {
    unit_at_sync: AtomicU32,
    uptime_at_sync: AtomicU32,
}

impl TimeState {
    pub const fn new() -> Self {
        Self {
            unit_at_sync: AtomicU32::new(0),
            uptime_at_sync: AtomicU32::new(0),
        }
    }

    pub fn set(&self, unix: u32) {
        let uptime = Instant::now().as_secs() as u32;
        self.unit_at_sync.store(unix, Ordering::Relaxed);
        self.uptime_at_sync.store(uptime, Ordering::Relaxed);
    }

    pub fn now(&self) -> Option<u32> {
        let base = self.unit_at_sync.load(Ordering::Relaxed);

        if base == 0 {
            return None;
        }

        let uptime_base = self.uptime_at_sync.load(Ordering::Relaxed);
        let uptime_now = Instant::now().as_secs() as u32;

        Some(base + uptime_now - uptime_base)
    }

    pub fn now_or_uptime(&self) -> u32 {
        self.now().unwrap_or_else(|| Instant::now().as_secs() as u32)
    }
}

#[embassy_executor::task]
pub async fn sync_task(
    stack: embassy_net::Stack<'static>,
) -> ! {
    loop {
        stack.wait_config_up().await;

        match sync_time(stack).await {
            Ok(secs) => {
                info!("Received seconds: {}", secs);
                let time_state = TIME_STATE.lock().await;
                time_state.set(secs);
            }
            Err(_) => {},
        }

        embassy_time::Timer::after_secs(60 * 60 * 6).await;
    }
}

#[allow(dead_code)]
enum NtpError {
    Bind(embassy_net::udp::BindError),
    Send(embassy_net::udp::SendError),
    Recv(embassy_net::udp::RecvError),
    Other,
}

async fn sync_time(stack: embassy_net::Stack<'_>) -> Result<u32, NtpError> {
    use embassy_net::udp::UdpSocket;

    info!("Getting NTP time");

    let mut rx_meta = [PacketMetadata::EMPTY];
    let mut rx_buf = [0u8; 48];
    let mut tx_meta = [PacketMetadata::EMPTY];
    let mut tx_buf = [0u8; 48];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    let addr = Ipv4Addr::new(91, 212, 242, 19);

    if let Err(err) = socket.bind(0) {
        warn!("Cannot bind to a socket");
        return Err(NtpError::Bind(err));
    };

    let endpoint = IpEndpoint {
        addr: IpAddress::Ipv4(addr),
        port: 123,
    };

    let mut packet = [0u8; 48];
    packet[0] = 0b11100011;

    if let Err(err) = socket.send_to(&mut packet, endpoint).await {
        warn!("Error getting NTP time: {}", err);
        return Err(NtpError::Send(err));
    };

    let mut recv_buf = [0u8; 48];
    let size = match socket.recv_from(&mut recv_buf).await {
        Ok((size, metadata)) => {
            info!(
                "Received NTP package. size = {}, metadata = {}",
                size, metadata
            );
            size
        }
        Err(err) => {
            warn!("Error receiving NTP: {}", err);
            return Err(NtpError::Recv(err));
        }
    };

    if size < 48 {
        info!("Too short package");
        return Err(NtpError::Other);
    }

    let secs = u32::from_be_bytes([recv_buf[40], recv_buf[41], recv_buf[42], recv_buf[43]]);

    const NTP_UNIX_OFFSET: u32 = 2_208_988_800;

    Ok(secs - NTP_UNIX_OFFSET)
}
