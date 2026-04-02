use crate::application::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::header,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Hot-module-reload WebSocket snippet injected into every HTML response.
/// Connects to `/ws` and triggers a full-page reload on "reload" message.
const HMR_SCRIPT: &str = concat!(
    "<script>(function(){",
    "var ws=new WebSocket('ws://'+location.host+'/ws');",
    "ws.onmessage=function(e){if(e.data==='reload')location.reload();};",
    "ws.onclose=function(){setTimeout(function(){location.reload();},1000);};",
    "})();</script>"
);

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(serve_html))
        .route("/app.js", get(serve_js))
        .route("/ws", get(serve_ws))
        .with_state(state)
}

async fn serve_html(State(state): State<Arc<AppState>>) -> Html<String> {
    let shared = state.shared.read().await;
    // Inject HMR script just before </body> so the page auto-reloads on save
    let html = shared
        .html
        .replace("</body>", &format!("{HMR_SCRIPT}\n</body>"));
    Html(html)
}

async fn serve_js(State(state): State<Arc<AppState>>) -> Response {
    let shared = state.shared.read().await;
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        shared.js.clone(),
    )
        .into_response()
}

async fn serve_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    let rx = state.tx.subscribe();
    ws.on_upgrade(|socket| handle_ws(socket, rx))
}

async fn handle_ws(mut socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    while let Ok(msg) = rx.recv().await {
        if socket.send(Message::Text(msg)).await.is_err() {
            break; // client disconnected
        }
    }
}
