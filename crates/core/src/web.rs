use embassy_net::Stack;
use heapless::String;
use picoserve::{
    AppBuilder, AppRouter,
    extract::Form,
    response::{DebugValue, File},
};
use serde::{Deserialize, Serialize};

pub const WEB_TASK_POOL_SIZE: usize = 2;

pub struct App;

impl picoserve::AppBuilder for App {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> {
        picoserve::Router::new()
            .route(
                "/",
                picoserve::routing::get_service(File::html(include_str!(
                    "../../../html/index.html"
                ))),
            )
            .route(
                "/save",
                picoserve::routing::post(
                    |Form(data): Form<Settings>| async move { DebugValue(data) },
                ),
            )
    }
}

pub struct WebApp {
    pub router: &'static picoserve::Router<<App as picoserve::AppBuilder>::PathRouter>,
    pub config: &'static picoserve::Config,
}

impl Default for WebApp {
    fn default() -> Self {
        let router = picoserve::make_static!(AppRouter<App>, App.build_app());

        let config = picoserve::make_static!(
            picoserve::Config,
            picoserve::Config::const_default().keep_connection_alive()
        );

        Self { router, config }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Settings {
    wifi_ssid: String<32>,
    wifi_password: String<32>,
    mqtt_broker: String<32>,
    mqtt_client_id: String<32>,
    mqtt_topic: String<32>,
}

#[embassy_executor::task(pool_size = WEB_TASK_POOL_SIZE)]
pub async fn task(
    task_id: usize,
    stack: Stack<'static>,
    router: &'static picoserve::AppRouter<App>,
    config: &'static picoserve::Config,
) -> ! {
    let port = 80;
    let mut tcp_rx_buf = [0; 1024];
    let mut tcp_tx_buf = [0; 1024];
    let mut http_buf = [0; 2048];

    picoserve::Server::new(router, &*config, &mut http_buf)
        .listen_and_serve(task_id, stack, port, &mut tcp_rx_buf, &mut tcp_tx_buf)
        .await
        .into_never()
}
