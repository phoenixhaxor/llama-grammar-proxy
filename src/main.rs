//! llama-grammar-proxy — Ultra-lightweight reverse proxy for llama-server
//!
//! Auto-injects GBNF grammar into every /v1/chat/completions request.
//! Handles tool calling gracefully (skips grammar when tools present).
//!
//! Architecture:
//!   Client → :8081 (this proxy) → :8082 (llama-server)

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use bytes::Bytes;
use clap::Parser;
use reqwest::Client;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};

// ── CLI Args ───────────────────────────────────────────────

#[derive(Parser, Debug, Clone)]
#[command(name = "llama-grammar-proxy", version)]
#[command(about = "Lightweight proxy for llama-server with auto GBNF grammar injection")]
struct Args {
    #[arg(short, long, default_value_t = 8081)]
    port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    backend_host: String,
    #[arg(long, default_value_t = 8082)]
    backend_port: u16,
    #[arg(long, default_value = "/Users/andre/llama-server-config/grammars/advanced.gbnf")]
    grammar: Option<String>,
    #[arg(long)]
    no_grammar: bool,
    #[arg(short, long)]
    verbose: bool,
}

// ── App State ─────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    backend_url: String,
    client: Client,
    grammar_content: Option<Arc<String>>,
    verbose: bool,
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

    let state = AppState {
        backend_url: format!("http://{}:{}", args.backend_host, args.backend_port),
        client: Client::builder()
            .timeout(std::time::Duration::from_secs(900))
            .build()
            .expect("Failed to build HTTP client"),
        grammar_content,
        verbose: args.verbose,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new().fallback(any(proxy_handler).with_state(state.clone())).layer(cors);
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));

    println!();
    println!("═══ llama-grammar-proxy (Rust) ═══");
    println!("  Listen:     0.0.0.0:{}", args.port);
    println!("  Backend:    {}:{}", args.backend_host, args.backend_port);
    println!(
        "  Grammar:    {}",
        match &state.grammar_content {
            Some(g) => format!("ENABLED ({} bytes)", g.len()),
            None => "DISABLED".into(),
        }
    );
    println!();

    if state.grammar_content.is_some() {
        println!("  ✓ Grammar loaded — auto-injecting into /v1/chat/completions");
        println!("  ✓ Tool calling aware — skips grammar when 'tools' field present");
    } else {
        println!("  ⚠ Passthrough mode (no grammar)");
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
    let backend_path = if query.is_empty() {
        format!("{}{}", state.backend_url, path)
    } else {
        format!("{}{}?{}", state.backend_url, path, query)
    };

    // ── Grammar Injection ──
    let mut body_bytes = body;
    let mut injected = false;

    if method == Method::POST
        && path.contains("/chat/completions")
        && !body_bytes.is_empty()
        && state.grammar_content.is_some()
    {
        if let Ok(body_str) = std::str::from_utf8(&body_bytes) {
            if let Ok(mut data) = serde_json::from_str::<Value>(body_str) {
                let has_grammar = data.get("grammar").is_some();
                let has_tools = data.get("tools").is_some();

                if !has_grammar && !has_tools {
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert(
                            "grammar".to_string(),
                            Value::String(state.grammar_content.as_ref().unwrap().as_str().to_string()),
                        );
                    }
                    match serde_json::to_vec(&data) {
                        Ok(new_body) => {
                            body_bytes = Bytes::from(new_body);
                            injected = true;
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to serialize injected body");
                        }
                    }
                } else if has_tools {
                    let count: usize = data["tools"].as_array().map(|a| a.len()).unwrap_or(0);
                    info!(tool_count = count, "Skipping grammar — tools present (tool calling mode)");
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

            if injected {
                info!(req_size = resp_body_bytes.len(), resp_len = resp_body_bytes.len(), %path, "✓ Grammar injected");
            }

            // Build final response — use axum's body conversion
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
