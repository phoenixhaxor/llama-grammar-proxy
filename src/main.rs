//! llama-grammar-proxy — Ultra-lightweight reverse proxy for llama-server
//!
//! Auto-injects GBNF grammar into every /v1/chat/completions request.
//! Smart comment stripping for code blocks to reduce token usage.
//! Handles tool calling gracefully (skips grammar when tools present).
//! Multi-backend switching via /admin/switch endpoint.
//!
//! Architecture:
//!   Client → :8081 (this proxy) → :8082 or :8083 (llama-server backends)

mod filter;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use bytes::Bytes;
use clap::Parser;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};

// ── CLI Args ───────────────────────────────────────────────

#[derive(Parser, Debug, Clone)]
#[command(name = "llama-grammar-proxy", version)]
#[command(about = "Lightweight proxy for llama-server with auto GBNF grammar injection and multi-backend switching")]
struct Args {
    #[arg(short, long, default_value_t = 8081)]
    port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    backend_host: String,
    /// Primary backend port (default, e.g. 8082)
    #[arg(long, default_value_t = 8082)]
    backend_port: u16,
    /// Secondary backend port (switchable via /admin/switch, e.g. 8083)
    #[arg(long)]
    secondary_backend_port: Option<u16>,
    #[arg(long, default_value = "/Users/andre/models/grammars/advanced.gbnf")]
    grammar: Option<String>,
    #[arg(long)]
    no_grammar: bool,
    /// Disable smart comment filtering
    #[arg(long)]
    no_filter: bool,
    #[arg(short, long)]
    verbose: bool,
}

// ── Switch Request (admin API) ─────────────────────────────

#[derive(Deserialize)]
struct SwitchRequest {
    /// "primary" or "secondary" (or port number as string)
    backend: String,
}

#[derive(Serialize)]
struct SwitchResponse {
    active_port: u16,
    message: String,
}

#[derive(Serialize)]
struct StatusResponse {
    listen_port: u16,
    active_backend: String,
    active_port: u16,
    primary_port: u16,
    secondary_port: Option<u16>,
    grammar_enabled: bool,
    filter_enabled: bool,
}

// ── App State ─────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    backend_host: String,
    primary_port: u16,
    secondary_port: Option<u16>,
    active_port: Arc<AtomicU16>,
    client: Client,
    grammar_content: Option<Arc<String>>,
    filter_enabled: bool,
    verbose: bool,
}

impl AppState {
    fn get_backend_url(&self) -> String {
        let port = self.active_port.load(Ordering::Relaxed);
        format!("http://{}:{}", self.backend_host, port)
    }
}

// ── Main ───────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(if args.verbose { "debug" } else { "info" })
        .with_target(false)
        .init();

    // Load grammar
    let grammar_content = if args.no_grammar {
        None
    } else {
        match &args.grammar {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(content) => {
                    info!(bytes = content.len(), %path, "Grammar loaded");
                    Some(Arc::new(content))
                }
                Err(e) => {
                    error!(error = %e, %path, "Failed to load grammar");
                    None
                }
            },
            None => None,
        }
    };

    let filter_enabled = !args.no_filter;
    let primary_port = args.backend_port;
    let secondary_port = args.secondary_backend_port;

    let state = AppState {
        backend_host: args.backend_host.clone(),
        primary_port,
        secondary_port,
        active_port: Arc::new(AtomicU16::new(primary_port)),
        client: Client::builder()
            .timeout(std::time::Duration::from_secs(900))
            .build()
            .expect("Failed to build HTTP client"),
        grammar_content,
        filter_enabled,
        verbose: args.verbose,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // Admin endpoints
        .route("/admin/switch", axum::routing::post(switch_backend))
        .route("/admin/status", axum::routing::get(get_status))
        // All other requests go to proxy
        .fallback(any(proxy_handler))
        .with_state(state.clone())
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));

    println!();
    println!("═══ llama-grammar-proxy (Rust) ═══");
    println!("  Listen:     0.0.0.0:{}", args.port);
    println!("  Primary:    {}:{}", args.backend_host, primary_port);
    if let Some(sec) = secondary_port {
        println!("  Secondary:  {}:{}", args.backend_host, sec);
    }
    println!(
        "  Grammar:    {}",
        match &state.grammar_content {
            Some(g) => format!("ENABLED ({} bytes)", g.len()),
            None => "DISABLED".into(),
        }
    );
    println!(
        "  Filter:     {}",
        if filter_enabled { "ENABLED (smart comment strip)" } else { "DISABLED" }
    );
    println!();
    println!("  Admin endpoints:");
    println!("    POST /admin/switch  {{\"backend\": \"primary\"|\"secondary\"|\"8083\"}}");
    println!("    GET  /admin/status");
    println!();

    if state.grammar_content.is_some() {
        println!("  ✓ Grammar loaded — auto-injecting into /v1/chat/completions");
        println!("  ✓ Tool calling aware — skips grammar when 'tools' field present");
    } else {
        println!("  ⚠ Passthrough mode (no grammar)");
    }

    if filter_enabled {
        println!("  ✓ Smart filter — strips comments in code blocks (```...```)");
        println!("  ✓ Safe: keeps TODO/FIXME/HACK, 'why' comments, long explanations");
    }

    println!("\n  Press Ctrl+C to stop\n");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("Failed to bind");
    info!(%addr, "Proxy listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .ok();
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.expect("Ctrl-C handler failed");
    info!("Shutting down...");
}

// ── Admin Handlers ─────────────────────────────────────────

async fn switch_backend(
    State(state): State<AppState>,
    Json(req): Json<SwitchRequest>,
) -> Response {
    let new_port = match req.backend.to_lowercase().as_str() {
        "primary" | "p" | "1" => {
            info!("Switching to PRIMARY backend (port {})", state.primary_port);
            state.primary_port
        }
        "secondary" | "sec" | "s" | "2" => {
            match state.secondary_port {
                Some(port) => {
                    info!("Switching to SECONDARY backend (port {})", port);
                    port
                }
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(SwitchResponse {
                            active_port: state.active_port.load(Ordering::Relaxed),
                            message: "No secondary backend configured".into(),
                        }),
                    ).into_response();
                }
            }
        }
        // Allow direct port number
        other => {
            if let Ok(port) = other.parse::<u16>() {
                info!("Switching to backend port {}", port);
                port
            } else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SwitchResponse {
                        active_port: state.active_port.load(Ordering::Relaxed),
                        message: format!("Invalid backend: '{}'. Use 'primary', 'secondary', or port number.", other),
                    }),
                ).into_response();
            }
        }
    };

    let old_port = state.active_port.swap(new_port, Ordering::Relaxed);
    info!("Backend switched: {} → {}", old_port, new_port);

    (
        StatusCode::OK,
        Json(SwitchResponse {
            active_port: new_port,
            message: format!("Switched from port {} to {}", old_port, new_port),
        }),
    ).into_response()
}

async fn get_status(State(state): State<AppState>) -> Response {
    let active = state.active_port.load(Ordering::Relaxed);
    let label = if active == state.primary_port {
        "primary"
    } else if Some(active) == state.secondary_port {
        "secondary"
    } else {
        "custom"
    };

    Json(StatusResponse {
        listen_port: 8081, // We don't store this in state, but it's always 8081 for now
        active_backend: label.to_string(),
        active_port: active,
        primary_port: state.primary_port,
        secondary_port: state.secondary_port,
        grammar_enabled: state.grammar_content.is_some(),
        filter_enabled: state.filter_enabled,
    }).into_response()
}

// ── Proxy Handler ───────────────────────────────────────────

async fn proxy_handler(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path();
    let query = uri.query().unwrap_or("");

    // Get current backend URL (may have been switched)
    let backend_url = state.get_backend_url();

    let backend_path = if query.is_empty() {
        format!("{}{}", backend_url, path)
    } else {
        format!("{}{}?{}", backend_url, path, query)
    };

    // ── Process request body ──
    let mut body_bytes = body;
    let mut injected = false;
    let mut filter_stats = None;

    if method == Method::POST
        && path.contains("/chat/completions")
        && !body_bytes.is_empty()
    {
        if let Ok(body_str) = std::str::from_utf8(&body_bytes) {
            if let Ok(mut data) = serde_json::from_str::<Value>(body_str) {
                let original_size = body_bytes.len();

                // Step 1: Smart comment filter (before grammar injection)
                if state.filter_enabled {
                    if let Some(messages) = data.get_mut("messages").and_then(|m| m.as_array_mut()) {
                        let mut total_saved = 0usize;
                        let mut total_stripped = 0usize;

                        for msg in messages.iter_mut() {
                            if let Some(content_val) = msg.get_mut("content") {
                                if let Some(content_str) = content_val.as_str() {
                                    let result = filter::filter_message(content_str);
                                    total_saved += result.chars_saved;
                                    total_stripped += result.comments_stripped;
                                    *content_val = Value::String(result.filtered_content);
                                }
                            }
                        }

                        if total_saved > 0 {
                            filter_stats = Some((total_stripped, total_saved));
                        }
                    }
                }

                // Step 2: Grammar injection (after filtering)
                let has_grammar = data.get("grammar").is_some();
                let has_tools = data.get("tools").is_some();

                if !has_grammar && !has_tools && state.grammar_content.is_some() {
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert(
                            "grammar".to_string(),
                            Value::String(state.grammar_content.as_ref().unwrap().as_str().to_string()),
                        );
                    }
                    injected = true;
                } else if has_tools {
                    let count: usize = data["tools"].as_array().map(|a| a.len()).unwrap_or(0);
                    info!(tool_count = count, "Skipping grammar — tools present (tool calling mode)");
                }

                match serde_json::to_vec(&data) {
                    Ok(new_body) => {
                        let new_size = new_body.len();
                        body_bytes = Bytes::from(new_body);

                        // Log both filter and grammar stats
                        if let Some((stripped, saved)) = filter_stats {
                            info!(
                                filter_saved_chars = saved,
                                filter_stripped = stripped,
                                body_before = original_size,
                                body_after = new_size,
                                grammar = if injected { "injected" } else { "skipped" },
                                "✓ Request processed"
                            );
                        } else if injected {
                            info!(body_size = new_size, "✓ Grammar injected");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to serialize body — using original");
                    }
                }
            }
        }
    }

    // ── Build backend request ──
    let mut builder = state.client.request(method.clone(), &backend_path);
    for (key, value) in &headers {
        match key.as_str().to_lowercase().as_str() {
            "host" | "content-length" | "transfer-encoding"
            | "connection" | "keep-alive" | "upgrade" => {}
            _ => {
                if let Ok(val) = value.to_str() {
                    builder = builder.header(key.as_str(), val);
                }
            }
        }
    }

    if method != Method::GET && method != Method::HEAD && method != Method::OPTIONS {
        builder = builder.body(body_bytes.to_vec());
    }

    match builder.send().await {
        Ok(resp) => {
            let status = resp.status();

            // Build response with forwarded headers
            let mut resp_builder = Response::builder().status(status);
            for (key, value) in resp.headers() {
                match key.as_str().to_lowercase().as_str() {
                    "transfer-encoding" | "content-length" | "connection"
                    | "keep-alive" => {}
                    _ => {
                        resp_builder = resp_builder.header(key, value);
                    }
                }
            }

            let resp_body_bytes: Vec<u8> = match resp.bytes().await {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    error!(error = %e, "Failed to read response body");
                    format!(r#"{{"error":"Failed to read: {}"}}"#, e).into_bytes()
                }
            };

            if injected && filter_stats.is_none() {
                info!(resp_size = resp_body_bytes.len(), %path, "✓ Grammar injected");
            }

            // Build final response
            let final_resp: std::result::Result<axum::http::Response<Body>, axum::http::Error> =
                resp_builder.body(Body::from(resp_body_bytes));
            match final_resp {
                Ok(r) => r.into_response(),
                Err(e) => {
                    error!(error = %e, "Failed to build response");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            error!(error = %e, backend = %backend_path, "Backend unavailable");
            (
                StatusCode::BAD_GATEWAY,
                [("content-type", "application/json")],
                format!(r#"{{"error":{{"message":"Backend unavailable: {}","type":"proxy_error","code":502}}}}"#, e),
            )
                .into_response()
        }
    }
}
