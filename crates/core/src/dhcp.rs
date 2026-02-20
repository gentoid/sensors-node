use core::net::Ipv4Addr;

use defmt::{error, info};
use edge_nal::UdpBind;
use embassy_time::Timer;

pub async fn run<U: UdpBind>(socket: U) -> ! {
    let mut bound_socket = loop {
        match socket
            .bind(core::net::SocketAddr::V4(core::net::SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                edge_dhcp::io::DEFAULT_SERVER_PORT,
            )))
            .await
        {
            Ok(sock) => break sock,
            Err(_) => {
                error!("DHCP server: failed to bind socket");
                Timer::after_secs(5).await;
                continue;
            }
        };
    };

    let server_ip = Ipv4Addr::new(192, 168, 1, 1);

    let mut server = edge_dhcp::server::Server::<_, 8>::new_with_et(server_ip);
    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];
    let options = edge_dhcp::server::ServerOptions::new(server_ip, Some(&mut gw_buf));
    let mut buf = [0u8; 1024];

    loop {
        info!("Starting DHCP server");
        let _ =
            edge_dhcp::io::server::run(&mut server, &options, &mut bound_socket, &mut buf).await;
        Timer::after_secs(5).await;
    }
}
