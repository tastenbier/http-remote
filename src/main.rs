use std::{
    env,
    ffi::OsStr,
    fmt::Write as _,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    net::UdpSocket,
    process::Command,
    sync::Arc,
};

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use tokio::{sync::RwLock, task};
use uuid::Uuid;

#[derive(Deserialize, Debug, Clone)]
struct Config {
    session_id: Option<String>,
    port: Option<u32>,
    #[serde(default)]
    title: String,
    action: Vec<Action>,
}

#[derive(Deserialize, Debug, Clone)]
struct Action {
    display_name: String,
    cmd: String,
    #[serde(skip)]
    cmd_hash: String,
}

#[tokio::main]
async fn main() {
    setup();

    let config = Config::read("./config.toml");
    // Randomize port by default (port '0')
    let port = config.port.unwrap_or(0);
    let session_id = config
        .session_id
        .clone()
        .unwrap_or(Uuid::new_v4().to_string());

    let app = Router::new()
        .route("/static/{subdir}/{static_asset}", get(static_asset_handler))
        .route(&format!("/{session_id}/"), get(index_handler))
        .route(
            &format!("/{session_id}/control/{{control}}"),
            get(control_handler),
        )
        .with_state(Arc::new(RwLock::new(config)));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .unwrap();

    if let Some(ip) = get_local_ip() {
        let url = format!(
            "http://{ip}:{}/{session_id}/",
            listener.local_addr().unwrap().port()
        );
        println!(
            "Listening on {url}\n{}",
            qr2term::generate_qr_string(&url).unwrap()
        );
    } else {
        println!(
            "Could not determine local IP!\nListening on http://{}/{session_id}/",
            listener.local_addr().unwrap(),
        );
    }

    let server_task = task::spawn(async {
        axum::serve(listener, app).await.unwrap();
    });
    let cmd = &env::args().skip(1).collect::<Vec<_>>().join(" ");

    if cmd.is_empty() {
        // Run server indefinitely
        server_task.await.unwrap();
    } else {
        // Run the provided command, the server will shut down after the command
        // finished.
        let _ = Command::new("sh").arg("-c").arg(cmd).output();
    }
}

/// Create default dirs and configs if necessary
fn setup() {
    fs::create_dir_all("./static/css/").expect("Failed to create './static/css/'");

    let path = std::path::Path::new("./static/css/main.css");
    if !path.exists() {
        fs::write(path, include_str!("../static/css/main.css"))
            .unwrap_or_else(|_| panic!("Failed to create '{}'", path.display()));
    }

    let path = std::path::Path::new("./config.toml");
    if !path.exists() {
        fs::write(path, include_str!("../config.toml"))
            .unwrap_or_else(|_| panic!("Failed to create '{}'", path.display()));
    }
}

async fn static_asset_handler(Path((subdir, static_asset)): Path<(String, String)>) -> Response {
    let safe_path = {
        let canonical_unsafe_path =
            std::path::Path::new(&format!("./static/{subdir}/{static_asset}"))
                .canonicalize()
                .unwrap();
        let base_path = std::path::Path::new("./static/").canonicalize().unwrap();

        if canonical_unsafe_path.starts_with(base_path) {
            canonical_unsafe_path
        } else {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let headers = match safe_path.extension().and_then(OsStr::to_str) {
        Some("css") => [("content-type", "text/css")],
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    let res = fs::read_to_string(&safe_path);
    if let Ok(res) = res {
        (StatusCode::OK, headers, res)
    } else {
        println!("Error reading '{}'", safe_path.display());

        (StatusCode::NOT_FOUND, Default::default(), String::default())
    }
    .into_response()
}

async fn index_handler(State(state): State<Arc<RwLock<Config>>>) -> Html<String> {
    let buttons: String =
        state
            .read()
            .await
            .action
            .iter()
            .fold(String::new(), |mut output, action| {
                let _ = write!(
                    output,
                    r#"<button type="button" onclick="fetch('control/{}')">{}</button>"#,
                    action.cmd_hash, action.display_name
                );
                output
            });

    Html(format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="stylesheet" href="/static/css/main.css" />
</head>
<body>
    <h1>{}</h1>
    <div class="controls">
        {buttons}
    </div>
</body>
</html>
"#,
        &state.read().await.title
    ))
}

async fn control_handler(
    Path(control): Path<String>,
    State(state): State<Arc<RwLock<Config>>>,
) -> impl IntoResponse {
    print!("handling {control:>25}");

    let action = state
        .read()
        .await
        .action
        .iter()
        .find(|action| action.cmd_hash.eq(&control))
        .cloned();

    if let Some(action) = action {
        println!(" ({})", action.display_name);

        let _ = Command::new("/bin/sh").arg("-c").arg(action.cmd).spawn();

        StatusCode::ACCEPTED
    } else {
        StatusCode::NOT_FOUND
    }
}

fn get_local_ip() -> Option<std::net::IpAddr> {
    // There might be better/cleaner solutions, but this is good enough for us.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("10.0.0.0:80").ok()?;

    Some(socket.local_addr().ok()?.ip())
}

impl Config {
    fn read<P>(path: P) -> Self
    where
        P: AsRef<std::path::Path>,
    {
        let mut config: Config = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();

        // Generate cmd hashes
        let mut hasher = DefaultHasher::new();
        config.action.iter_mut().for_each(|action| {
            action.cmd.hash(&mut hasher);
            action.cmd_hash = format!("{}", hasher.finish());
        });

        config
    }
}
