use core::sync::atomic::Ordering;
use defmt::Debug2Format;
use embassy_net::Stack;
use picoserve::{AppBuilder, AppRouter, extract::Form, response::File};
use static_cell::StaticCell;

use crate::{config::SettingsEnum, kv_storage};

extern crate alloc;

pub const WEB_TASK_POOL_SIZE: usize = 2;
static INDEX_PAGE: StaticCell<alloc::string::String> = StaticCell::new();

pub struct App {
    pub db: &'static kv_storage::Db,
    settings: SettingsEnum,
}

impl App {
    pub fn new(db: &'static kv_storage::Db, settings: SettingsEnum) -> Self {
        Self { db, settings }
    }
}

impl picoserve::AppBuilder for App {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> {
        let db = self.db;
        let template = include_str!("../../../html/index.html");
        let settings = self.settings.to_filled_in_with_default();

        let index_page = template
            .replace("%_wifi_ssid_%", &settings.wifi_ssid)
            .replace("%_wifi_password_%", &settings.wifi_password)
            .replace("%_mqtt_broker_%", &settings.mqtt_broker)
            .replace("%_mqtt_client_id_%", &settings.mqtt_client_id)
            .replace("%_mqtt_topic_%", &settings.mqtt_topic);

        let page: &'static str = INDEX_PAGE.init(index_page).as_str();

        picoserve::Router::new()
            .route("/", picoserve::routing::get_service(File::html(&page)))
            .route(
                "/save",
                picoserve::routing::post(
                    move |Form(data): Form<crate::config::Settings>| async move {
                        match crate::config::save_settings(db, &data).await {
                            Err(err) => {
                                defmt::error!("Saving error: {}", err);
                                Debug2Format(&data);
                            }
                            Ok(_) => {
                                defmt::info!("Saved!");
                                crate::system::NEED_REBOOT.store(true, Ordering::SeqCst);
                            }
                        }
                    },
                ),
            )
    }
}

pub struct WebApp {
    pub router: &'static picoserve::Router<<App as picoserve::AppBuilder>::PathRouter>,
    pub config: &'static picoserve::Config,
}

impl WebApp {
    pub fn new(db: &'static kv_storage::Db, settings: SettingsEnum) -> Self {
        let router = picoserve::make_static!(AppRouter<App>, App::new(db, settings).build_app());

        let config = picoserve::make_static!(
            picoserve::Config,
            picoserve::Config::const_default().keep_connection_alive()
        );

        Self { router, config }
    }
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
