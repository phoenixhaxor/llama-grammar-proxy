# 🦀 llama-grammar-proxy

> Ultra-lightweight **Rust (axum)** reverse proxy for llama-server with **auto GBNF grammar injection**, **smart comment stripping**, and **multi-backend switching**.

[![Rust](https://img.shields.io/badge/Rust-1.77+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Binary Size](https://img.shields.io/badge/Binary-3.6MB-green.svg)](#build)

---

## 📖 Table of Contents

- [Architecture](#-architecture)
- [Why This Exists](#-why-this-exists)
- [Features](#-features)
- [Performance](#-performance)
- [Quick Start](#-quick-start)
- [Configuration](#-configuration)
- [Admin API](#-admin-api)
- [Grammar: Structured Thinking (QMKRV)](#-grammar-structured-thinking-qmkrv)
- [Smart Comment Filter](#-smart-comment-filter)
- [Benchmark Results](#-benchmark-results)
- [Production Deployment (macOS launchd)](#-production-deployment-macos-launchd)
- [API Reference](#-api-reference)
- [Troubleshooting](#-troubleshooting)
- [Project Structure](#-project-structure)

---

## 🏗 Architecture

```
                        ┌──────────────────────────────┐
                        │     llama-grammar-proxy       │
                        │        (Rust / axum)          │
  Client Request ──────►│  :8081                       │
  (chat/completions)    │                              │
                        │  ┌────────────────────────┐  │
                        │  │ 1. Smart Comment Filter │  │
                        │  │    (strip noise in code │  │
                        │  │     blocks, keep TODOs, │  │
                        │  │     WHY comments, URLs) │  │
                        │  └──────────┬─────────────┘  │
                        │             │                 │
                        │  ┌──────────▼─────────────┐  │
                        │  │ 2. Grammar Injection    │  │
                        │  │    (auto-inject GBNF    │  │
                        │  │     into every request) │  │
                        │  └──────────┬─────────────┘  │
                        │             │                 │
                        │  ┌──────────▼─────────────┐  │
                        │  │ 3. Backend Router       │  │
                        │  │    primary (:8082) or   │  │
                        │  │    secondary (:8083)    │  │
                        │  └──────────┬─────────────┘  │
                        └─────────────┼────────────────┘
                                      │
                          ┌───────────┴───────────┐
                          │                       │
                    ┌─────▼─────┐          ┌──────▼──────┐
                    │  :8082    │          │   :8083     │
                    │ Primary   │          │ Secondary   │
                    │ (legacy)  │          │ (Qwopus/Qwen)│
                    └───────────┘          └─────────────┘
```

---

## 🤔 Why This Exists

### The Problem

Modern MoE (Mixture of Experts) models like **Qwen3.6-35B** have a thinking/reasoning mode. Without constraints, the model produces **verbose, unstructured thinking** that:

1. **Wastes tokens** — 1,500+ chars of English rambling per request
2. **Consumes the entire token budget** — model thinks but never answers
3. **Increases latency** — more tokens = more decode time
4. **Increases cost** — more tokens = more compute

**Example without grammar:**
```
<thinkWe need to analyze the question carefully. The user is asking about
Umrah packages. Let me think about this step by step. First, we should
consider the different package tiers available. The Silver package costs
Rp148,000,000 and includes... [continues for 1,500+ chars until max_tokens reached]</think

[NO OUTPUT — model ran out of tokens just thinking!]
```

### The Solution

This proxy **auto-injects a GBNF grammar** that forces the model's thinking into a structured, ultra-compact format:

```
<thinkQ=solve
M=test
K=answer
R=answer
V=verify
</thinkBaik Pak/Bu, berikut informasi lengkap paket umroh kami...
[Full 1,200 char answer with correct pricing and details]
```

**Result: 97% thinking compression, model actually answers the question!**

---

## ✨ Features

| Feature | Description |
|---|---|
| **Auto Grammar Injection** | Automatically injects GBNF grammar into every `/v1/chat/completions` request |
| **Tool Calling Aware** | Skips grammar injection when `tools` field is present (preserves function calling) |
| **Smart Comment Filter** | Strips noise comments from code blocks while keeping TODOs, FIXMEs, WHY comments |
| **Multi-Backend Switching** | Switch between primary and secondary backends on-the-fly without restart |
| **Zero-Downtime Swap** | Atomic backend switching via `/admin/switch` endpoint |
| **Ultra-Lightweight** | 3.6 MB binary, ~4 MB RSS, <0.1% CPU overhead |
| **900s Timeout** | Handles large context (128K+) requests without premature timeout |
| **CORS Support** | Full CORS headers for browser-based clients |
| **Hot-Switchable** | Change backends at runtime without restarting the proxy |

---

## ⚡ Performance

| Metric | Value |
|---|---|
| **Binary Size** | 3.6 MB (LTO + stripped) |
| **RSS (Memory)** | ~4 MB |
| **Proxy Overhead** | 0-5% on decode speed |
| **Startup Time** | <50ms |
| **Grammar Injection** | ~0.1ms per request |
| **Comment Filtering** | ~0.5ms per request |

**Benchmarked on:** Mac M4 Pro, 24GB RAM, Qwen3.6-35B MoE @ 128K context

---

## 🚀 Quick Start

### Prerequisites

- Rust 1.77+ (for building)
- A running [llama.cpp](https://github.com/ggerganov/llama.cpp) or [ik_llama.cpp](https://github.com/ikawrakow/ik_llama.cpp) server

### Build

```bash
git clone https://github.com/phoenixhaxor/llama-grammar-proxy.git
cd llama-grammar-proxy
cargo build --release

# Binary will be at:
ls -lh target/release/llama_grammar_proxy
# -rwxr-xr-x  3.6M llama_grammar_proxy
```

### Install

```bash
sudo cp target/release/llama_grammar_proxy /usr/local/bin/llama-grammar-proxy
```

### Run

```bash
# Basic — proxy on :8081 → llama-server on :8082
llama-grammar-proxy --port 8081 --backend-port 8082

# With grammar injection
llama-grammar-proxy --port 8081 --backend-port 8082 \
  --grammar ./grammars/advanced.gbnf

# With multi-backend switching
llama-grammar-proxy --port 8081 --backend-port 8082 \
  --secondary-backend-port 8083 \
  --grammar ./grammars/advanced.gbnf

# Passthrough mode (no grammar, just proxy)
llama-grammar-proxy --port 8081 --backend-port 8082 --no-grammar

# Verbose logging
llama-grammar-proxy --port 8081 --backend-port 8082 --grammar ./grammars/advanced.gbnf --verbose
```

---

## ⚙ Configuration

### CLI Flags

| Flag | Default | Description |
|---|---|---|
| `-p, --port` | `8081` | Port to listen on |
| `--backend-host` | `127.0.0.1` | Backend llama-server hostname |
| `--backend-port` | `8082` | Primary backend port |
| `--secondary-backend-port` | — | Secondary backend port (for switching) |
| `--grammar` | `~/models/grammars/advanced.gbnf` | Path to GBNF grammar file |
| `--no-grammar` | `false` | Disable grammar injection (passthrough) |
| `--no-filter` | `false` | Disable smart comment filtering |
| `-v, --verbose` | `false` | Enable debug logging |

### Example Configurations

**Single backend with grammar:**
```bash
llama-grammar-proxy \
  --port 8081 \
  --backend-port 8082 \
  --grammar /path/to/grammar.gbnf
```

**Dual backend with switching:**
```bash
llama-grammar-proxy \
  --port 8081 \
  --backend-port 8082 \
  --secondary-backend-port 8083 \
  --grammar /path/to/grammar.gbnf
```

---

## 🔀 Admin API

The proxy exposes admin endpoints for runtime management. These are **not forwarded** to the backend.

### Switch Backend

```bash
# Switch to secondary backend
curl -X POST http://127.0.0.1:8081/admin/switch \
  -H 'Content-Type: application/json' \
  -d '{"backend":"secondary"}'

# Switch to primary backend
curl -X POST http://127.0.0.1:8081/admin/switch \
  -H 'Content-Type: application/json' \
  -d '{"backend":"primary"}'

# Switch by port number directly
curl -X POST http://127.0.0.1:8081/admin/switch \
  -H 'Content-Type: application/json' \
  -d '{"backend":"8083"}'

# Aliases
# "primary", "p", "1"   → primary backend
# "secondary", "sec", "s", "2" → secondary backend
# Any number             → that port directly
```

**Response:**
```json
{
  "active_port": 8083,
  "message": "Switched from port 8082 to port 8083"
}
```

### Get Status

```bash
curl http://127.0.0.1:8081/admin/status
```

**Response:**
```json
{
  "listen_port": 8081,
  "active_backend": "secondary",
  "active_port": 8083,
  "primary_port": 8082,
  "secondary_port": 8083,
  "grammar_enabled": true,
  "filter_enabled": true
}
```

---

## 🧠 Grammar: Structured Thinking (QMKRV)

The included `grammars/advanced.gbnf` forces the model to produce structured thinking using the **QMKRV** format:

```
<thinkQ=calc
M=case
K=ca
R=ca
V=candidate
</think

[actual answer here]
```

### Field Definitions

| Field | Name | Purpose | Allowed Values |
|---|---|---|---|
| **Q** | Question Type | Classify what kind of question | `solve`, `prove`, `route`, `debug`, `patch`, `code`, `calc`, `compare`, `explain` |
| **M** | Method | How to approach it | `case`, `enum`, `check`, `derive`, `edit`, `test`, `trace`, `rank` |
| **K** | Key Concepts | Important keywords (max 8) | Any `[A-Za-z][A-Za-z0-9_.!<>=/-]{0,18}` |
| **R** | Reasoning Steps | Key steps taken (max 8) | Same as K |
| **V** | Verdict | Final assessment | `ok`, `fail`, `done`, `blocked`, `candidate`, `verify` |

### Why This Works

1. **GBNF grammar** is a formal context-free grammar that llama.cpp enforces during token generation
2. The model **cannot** produce free-form verbose thinking — it must follow the strict QMKRV structure
3. Each field is **1-2 tokens max** — total thinking is ~35 chars vs 1,500+ chars unstructured
4. The `out` rule uses `[\x09\x0A\x0D\x20-\x7E]+` which allows any printable ASCII for the actual answer

### How It's Injected

The proxy intercepts every `POST /v1/chat/completions` request and:

1. Parses the JSON body
2. Checks if `grammar` field already exists (skip if present)
3. Checks if `tools` field exists (skip if present — tool calling mode)
4. Injects the grammar content into the JSON body
5. Forwards the modified request to the backend

---

## 🔍 Smart Comment Filter

The proxy includes a **smart comment stripping filter** that removes noise comments from code blocks in LLM message content, reducing token usage.

### What It Does

- **Only operates inside markdown code blocks** (`` ```...``` ``)
- **Never touches** plain text, tool results, or content outside code blocks
- When in doubt, **keeps** the comment (false negatives > false positives)

### What Gets Stripped

```rust
/// User's display name          ← short doc comment (stripped)
// ============================  ← decorative separator (stripped)
// This is a simple note         ← generic comment (stripped)
```

### What Gets Kept

```rust
// TODO: fix race condition      ← has keep prefix
// Using RwLock because read-heavy ← contains "because" (WHY keyword)
// See https://example.com/docs   ← contains URL
// Important: must handle overflow ← contains WHY keyword
// This is a very long explanation about why we need to use this particular
// algorithm instead of the standard approach... (>100 chars)  ← long explanation
```

### Supported Languages

| Language | Line Comments | Block Comments |
|---|---|---|
| Rust | `//` | `/* */` |
| Python | `#` | `""" """` |
| Go | `//` | `/* */` |
| JavaScript/TypeScript | `//` | `/* */` |
| C/C++/Java | `//` | `/* */` |
| Shell/Bash | `#` | — |
| Others | — (no stripping) | — |

### Safety Keywords (Kept)

`because`, `due to`, `workaround`, `hack`, `todo`, `fixme`, `important`, `note`, `bug`, `constraint`, `warning`, `caution`, `must`, `required`, `cannot`, `critical`, `unsafe`, `careful`, `beware`, `ensure`, `remember`, `security`, `race condition`, `deadlock`, `memory leak`, `gotcha`, `pitfall`, `side effect`, `non-obvious`, `intentional`, `deliberate`, `by design`, and more.

---

## 📊 Benchmark Results

### Test Environment

- **Hardware:** Mac M4 Pro, 24GB Unified RAM, 12 CPU threads
- **Model:** Qwen3.6-35B MoE Q3_K_XL (16 GB) via ik_llama.cpp
- **Context:** 128K (131,072 tokens)
- **Backend:** CPU-only (`-ngl 0`), port 8083

### Short Context (~500 tokens)

| Metric | Direct (no grammar) | Proxy (QMKRV) |
|---|---|---|
| **Thinking chars** | ~950 verbose English | **37 chars** structured |
| **Completion tokens** | 300 | **271** (saved 29) |
| **Decode speed** | 33.2 tok/s | **33.7 tok/s** |
| **Output** | ❌ No answer (all tokens in thinking) | ✅ Full correct answer |

### Long Context (~60K tokens)

| Metric | Direct (no grammar) | Proxy (QMKRV) |
|---|---|---|
| **Prefill** | 75.0 tok/s (209s cold) | 14.5 tok/s (cached) |
| **Decode** | 17.3 tok/s | **16.8 tok/s** |
| **Wall time** | 3m 52s | **24s** (10× faster) |
| **Thinking** | 1,449 chars | **39 chars** (97% compression) |
| **Output** | ❌ No answer | ✅ 1,267 chars correct answer |

### Long Context (~78K tokens)

| Metric | Direct (no grammar) | Proxy (QMKRV) |
|---|---|---|
| **Prefill** | 15.9 tok/s | 23.9 tok/s |
| **Decode** | 13.6 tok/s | **13.3 tok/s** |
| **Wall time** | 30s | **47s** (includes cold prefill) |
| **Thinking** | 1,532 chars | **43 chars** (97% compression) |
| **Output** | ❌ No answer | ✅ Full answer with correct math |

### Key Findings

1. **Proxy overhead is negligible** — 0-5% on decode speed
2. **Grammar saves responses from total failure** — without it, the model consumes all tokens thinking and never answers
3. **97% thinking compression** — from 1,500+ chars to ~40 chars
4. **Smart filter saves additional tokens** by stripping noise code comments

---

## 🍎 Production Deployment (macOS launchd)

### LaunchAgent for the Proxy

Create `/Users/andre/Library/LaunchAgents/com.panglima.grammar-proxy.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" 
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.panglima.grammar-proxy</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/llama-grammar-proxy</string>
        <string>--port</string>
        <string>8081</string>
        <string>--backend-port</string>
        <string>8082</string>
        <string>--secondary-backend-port</string>
        <string>8083</string>
        <string>--grammar</string>
        <string>/Users/andre/models/grammars/advanced.gbnf</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/Users/andre/logs/grammar-proxy.log</string>
    <key>StandardErrorPath</key>
    <string>/Users/andre/logs/grammar-proxy.err</string>
</dict>
</plist>
```

### Start Service

```bash
launchctl load ~/Library/LaunchAgents/com.panglima.grammar-proxy.plist
```

### Verify

```bash
curl http://127.0.0.1:8081/admin/status
```

---

## 📡 API Reference

### Proxied Endpoints

All requests (except `/admin/*`) are forwarded to the active backend:

| Method | Path | Notes |
|---|---|---|
| `*` | `/v1/chat/completions` | Grammar injected, comments filtered |
| `*` | `/v1/completions` | Grammar injected (if applicable) |
| `*` | `/v1/models` | Passthrough |
| `*` | `/v1/embeddings` | Passthrough |
| `*` | Any other path | Passthrough |

### Admin Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/admin/status` | Get current proxy status |
| `POST` | `/admin/switch` | Switch active backend |

### Error Responses

**Backend unavailable (502):**
```json
{
  "error": {
    "message": "Backend unavailable: connection refused",
    "type": "proxy_error",
    "code": 502
  }
}
```

**Invalid switch request (400):**
```json
{
  "active_port": 8082,
  "message": "No secondary backend configured"
}
```

---

## 🔧 Troubleshooting

### Proxy returns 502

```
Backend unavailable: connection refused
```

**Cause:** The backend llama-server is not running.

**Fix:** Start llama-server first, or switch to a running backend:
```bash
curl -X POST http://127.0.0.1:8081/admin/switch \
  -d '{"backend":"8083"}'
```

### Grammar not being injected

**Cause:** Request has `tools` field or existing `grammar` field.

**Fix:** This is by design. Grammar is skipped when:
- `tools` array is present (tool calling mode)
- `grammar` field already exists in the request body

### ik_llama.cpp returns "tools param requires --jinja flag"

**Cause:** ik_llama.cpp requires `--jinja` flag to support tool calling.

**Fix:** Add `--jinja` to your llama-server startup command.

### High latency on first request

**Cause:** First request after backend switch may trigger cold prefill.

**Fix:** This is normal. Subsequent requests will benefit from KV cache.

---

## 📁 Project Structure

```
llama-grammar-proxy/
├── Cargo.toml              # Dependencies & build config
├── .gitignore
├── grammars/
│   └── advanced.gbnf       # QMKRV structured thinking grammar
├── src/
│   ├── main.rs             # Proxy server, admin API, routing
│   └── filter.rs           # Smart comment stripping filter
└── target/
    └── release/
        └── llama_grammar_proxy  # Compiled binary (3.6 MB)
```

### Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `axum` | 0.8 | HTTP framework |
| `tokio` | 1 | Async runtime |
| `reqwest` | 0.12 | HTTP client for backend requests |
| `serde` / `serde_json` | 1 | JSON serialization |
| `clap` | 4 | CLI argument parsing |
| `tower-http` | 0.6 | CORS middleware |
| `regex` | 1 | Comment pattern matching |
| `tracing` | 0.1 | Structured logging |
| `once_cell` | 1.21 | Lazy static initialization |
| `bytes` | 1 | Efficient byte handling |
| `hyper` | 1 | Low-level HTTP |

### Build Profile

```toml
[profile.release]
opt-level = 3        # Maximum optimization
lto = true           # Link-Time Optimization
codegen-units = 1    # Single codegen unit (smaller binary)
strip = true         # Strip debug symbols
```

---

## 📝 License

MIT License — see [LICENSE](LICENSE) for details.

---

## 🙏 Credits

Built with:
- [axum](https://github.com/tokio-rs/axum) — Ergonomic and modular web framework
- [llama.cpp](https://github.com/ggerganov/llama.cpp) — LLM inference in C/C++
- [ik_llama.cpp](https://github.com/ikawrakow/ik_llama.cpp) — Optimized llama.cpp fork

---

*Maintained by [PT. Panglima Ekspres](https://panglimaekspres.co.id) — Licensed Umrah & Hajj Travel Agency, Surabaya since 1990.*
