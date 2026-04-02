use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

pub struct SharedState {
    pub html: String,
    pub js: String,
}

pub struct AppState {
    pub shared: Arc<RwLock<SharedState>>,
    pub port: u16,
    pub tx: broadcast::Sender<String>,
}

impl AppState {
    pub fn new(html: String, js: String, port: u16) -> Self {
        let (tx, _) = broadcast::channel(16);
        AppState {
            shared: Arc::new(RwLock::new(SharedState { html, js })),
            port,
            tx,
        }
    }

    /// Replace the served HTML/JS and broadcast a "reload" signal to all
    /// connected WebSocket clients.
    pub async fn update(&self, html: String, js: String) {
        let mut state = self.shared.write().await;
        state.html = html;
        state.js = js;
        drop(state);
        // Ignore error if no clients are connected
        let _ = self.tx.send("reload".to_string());
    }
}
