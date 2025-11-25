# QUIC Lab

Scalable QUIC / HTTP/3 measurement framework for Internet-wide measurements, built on top of [tencent/tquic](https://github.com/tencent/tquic).

> Originally developed as part of a Master’s thesis on novel transport and application-layer measurement techniques.

[![Docker](https://img.shields.io/badge/docker-ghcr.io-informational)](https://ghcr.io)
[![Rust](https://img.shields.io/badge/Rust-Edition%202024-informational)](https://www.rust-lang.org/)
[![GitHub Actions](https://github.com/QUIC-Lab/quic-lab/actions/workflows/docker-publish-latest.yml/badge.svg)](https://github.com/QUIC-Lab/quic-lab/actions/workflows/docker-publish-latest.yml)


---

## Table of contents

- [Overview](#overview)
- [Features](#features)
- [Repository layout](#repository-layout)
- [Quick start](#quick-start)
    - [Prerequisites](#prerequisites)
    - [Build from source](#build-from-source)
    - [Run with Docker](#run-with-docker)
- [Configuration](#configuration)
    - [`[scheduler]`](#scheduler)
    - [`[io]`](#io)
    - [`[general]`](#general)
    - `[[connection_config]]`
- [Input and output](#input-and-output)
    - [Domain list](#domain-list)
    - [Output files](#output-files)
- [HTTP/3 probe](#http3-probe)
- [Writing custom probes](#writing-custom-probes)
- [Logging, qlog and keylog](#logging-qlog-and-keylog)
- [Ethics and responsible use](#ethics-and-responsible-use)
- [Contributing](#contributing)
- [License](#license)

---

## Overview

`QUIC Lab` is a modular Rust framework for large-scale, automated measurements of QUIC and HTTP/3 deployments on the Internet.

The system:

- reads a list of domains,
- resolves IPv4/IPv6 targets,
- runs QUIC handshakes (optionally with multipath),
- drives an application-layer probe (HTTP/3 by default),
- records per-connection statistics and metadata,
- writes aggregated [qlog 0.4 JSON-SEQ](https://datatracker.ietf.org/doc/html/draft-ietf-quic-qlog-main-schema) traces for further analysis.

The architecture separates:

- **core**: transport, scheduling, logging, qlog, DNS resolution, rate limiting, file rotation, recorder,
- **probes**: application-layer measurement logic (HTTP/3 and a reusable probe template),
- **runner**: the CLI / orchestrator that fans out over many domains and coordinates global rate-limiting and progress reporting.

---

## Features

- QUIC client based on [tencent/tquic](https://github.com/tencent/tquic)
- HTTP/3 GET probe implementation
- IPv4 / IPv6 / auto family selection
- Configurable QUIC transport parameters (max data, streams, ACK delay, payload size, etc.)
- Optional multipath QUIC (enable/disable and algorithm selection)
- Global concurrency and requests-per-second throttling via `governor`
- Rotating log files, key logs, recorder files, and qlog JSON-SEQ files
- Structured per-connection JSONL recorder with compact metadata and stats
- Progress reporting with [indicatif](https://github.com/console-rs/indicatif) for TTY and periodic logging for non-TTY
- Pluggable probe architecture (`AppProtocol` trait), with an annotated `template.rs` for custom probes
- Docker image publishing via GitHub Actions to GitHub Container Registry (GHCR)
- Dependency updates handled via `dependabot.yml`

---

## Repository layout

Workspace (simplified):

```text
.
├── Cargo.toml               # Workspace manifest
├── config.toml              # Optional Cargo aliases (see below)
├── dependabot.yml           # Dependency update configuration
├── .github/
│   └── workflows/
│       └── docker-publish-latest.yml
├── core/                    # Shared library crate
│   ├── Cargo.toml
│   └── src/
│       ├── config.rs        # Runtime config (scheduler, IO, general, connection_config)
│       ├── keylog.rs        # Rotated TLS keylog sink
│       ├── logging.rs       # Tracing + file logger with rotation
│       ├── qlog.rs          # qlog 0.4 JSON-SEQ mux and minimizer
│       ├── recorder.rs      # JSONL recorder (per-trace_id records)
│       ├── resolver.rs      # IPv4 / IPv6 aware DNS resolution helpers
│       ├── rotate.rs        # Generic rotating writer abstraction
│       ├── throttle.rs      # Global RPS limiter (governor)
│       ├── transport/
│       │   ├── mod.rs
│       │   └── quic/        # QUIC transport based on tquic
│       │       ├── mod.rs
│       │       └── quic.rs
│       └── types.rs         # Shared types and result structs
├── probes/                  # Probe implementations (application layer)
│   ├── Cargo.toml
│   └── src/
│       ├── h3.rs            # HTTP/3 GET probe on top of QUIC
│       ├── template.rs      # Template for custom probes
│       └── lib.rs
└── runner/                  # CLI / orchestration crate
    ├── Cargo.toml
    └── src/
        └── main.rs          # Domain fan-out, concurrency & progress reporting
```

Optional Cargo aliases (root `config.toml`):

```toml
[alias]
r = "run -p runner"
b = "build -p runner"
t = "test -p runner"
```

---

## Quick start

### Prerequisites

* Recent stable Rust toolchain with edition 2024 support (via `rustup`)
* A POSIX-like environment (Linux is the primary target; macOS works for development)
* For Docker usage: Docker Engine with Buildx and QEMU (for multi-arch builds) if you build images locally

### Build from source

```bash
# Clone the repository
git clone https://github.com/QUIC-Lab/quic-lab.git
cd quic-lab/

# Build the workspace in release mode
cargo build --release

# Or use the runner alias (if config.toml is active)
cargo r --release
```

By default, the runner expects a runtime configuration file at `in/config.toml`.
You can override this path by passing it as the first CLI argument:

```bash
# Explicit config path
cargo run -p runner --release -- in/config.toml
```

### Run with Docker

The GitHub Actions workflow builds and publishes a multi-arch image to GHCR:

```text
ghcr.io/quic-lab/quic-lab:latest
```

Example `docker-compose.yml` / `docker compose` service:

```yaml
services:
  quic-lab:
    container_name: quic-lab
    image: ghcr.io/quic-lab/quic-lab:latest
    ports:
      - "80:80"      # only needed if your setup exposes HTTP; not required for scans
    dns:
      - "1.1.1.1"    # Cloudflare
      - "2606:4700:4700::1111"
      - "8.8.8.8"    # Google
      - "2001:4860:4860::8888"
    volumes:
      - ./in:/app/in
      - ./out:/app/out
```

The Docker image expects the same `in/` and `out/` folders as the native runner:

* mount your `in/` directory containing `config.toml` and `domains.txt`,
* mount an `out/` directory to collect logs, recorder files and qlogs.

---

## Configuration

At runtime, the runner loads a TOML configuration (default: `in/config.toml`) via `core::config::read_config`.

High-level structure:

```toml
[scheduler]
# concurrency, RPS, burst, etc.

[io]
# input/output directories and domain file name

[general]
# logging, qlog/keylog/session toggles

[[connection_config]]
# one or more connection attempts tried in order
```

### `[scheduler]`

Controls concurrency and rate limiting:

```toml
[scheduler]
# Number of worker threads in the Rayon pool.
# 0 = auto (10 × available_parallelism)
concurrency = 0

# Global maximum requests per second.
# 0 = unlimited.
requests_per_second = 150

# Short-term burst allowance (token bucket size).
# Min. 1. Higher burst allows short spikes above RPS.
burst = 150

# Delay (ms) between attempts for the same domain when
# multiple [[connection_config]] entries are configured.
inter_attempt_delay_ms = 3000
```

### `[io]`

Controls where inputs are read from and where outputs are written:

```toml
[io]
# Directory containing the runtime config and domains list.
in_dir = "in"

# Domain list filename (inside `in_dir`).
domains_file_name = "domains.txt"

# Base output directory. Subdirectories are created as needed.
out_dir = "out"
```

### `[general]`

Controls logging and which artefacts are persisted:

```toml
[general]
# OFF, ERROR, WARN, INFO, DEBUG, TRACE
log_level = "INFO"

save_log_files = true   # rotating logs in out/log_files/
save_recorder_files = true   # JSONL recorder in out/recorder_files/
save_qlog_files = true   # qlog JSON-SEQ in out/qlog_files/
save_keylog_files = false  # TLS keylog files in out/keylog_files/
save_session_files = false  # session resumption blobs in out/session_files/
```

### `[[connection_config]]`

Each `[[connection_config]]` entry describes one attempt. The runner tries them in order until one succeeds (per domain), optionally sleeping
`inter_attempt_delay_ms` between attempts.

Defaults are provided for all fields; you only need to override what you care about.

Minimal example:

```toml
[[connection_config]]
# Application layer
port = 443
path = "/"
user_agent = "QUIC Lab (research; no-harm-intended; opt-out: you@example.org)"

# TLS / ALPN
verify_peer = true
alpn = ["h3"]

# IP family: "auto", "ipv4", or "ipv6"
ip_version = "auto"

# Timeouts (ms)
max_idle_timeout_ms = 30000

# Transport parameters (example values; these are the defaults)
initial_max_data = 10485760
initial_max_stream_data_bidi_local = 5242880
initial_max_stream_data_bidi_remote = 2097152
initial_max_stream_data_uni = 1048576
initial_max_streams_bidi = 200
initial_max_streams_uni = 100
max_ack_delay = 25
active_connection_id_limit = 2
send_udp_payload_size = 1200
max_receive_buffer_size = 65536

# Multipath (tquic extensions)
enable_multipath = false
multipath_algorithm = "minrtt"   # "minrtt", "roundrobin", or "redundant"
```

For multipath experiments, set:

```toml
enable_multipath = true
multipath_algorithm = "redundant"  # or "roundrobin", "minrtt"
```

---

## Input and output

### Domain list

`read_domains_iter` expects a plain text file with one domain per line:

```text
example.com
www.example.org
# Lines starting with '#' are comments and ignored
example.net   # inline comments after '#' are also stripped
```

The file path is `in/<domains_file_name>` (by default `in/domains.txt`).

### Output files

The framework writes all artefacts under `out_dir`:

* `out/log_files/`

    * `quic-lab.log`, `quic-lab.log.1`, …
      Rotating textual logs (configured via `save_log_files`).

* `out/recorder_files/`

    * `quic-lab-recorder.jsonl`, `quic-lab-recorder.jsonl.1`, …
      JSON Lines records of the form:

      ```json
      {"key": "<trace_id>", "value": { ... Probe-specific JSON ... }}
      ```

      For the HTTP/3 probe, this contains `ProbeRecord` with handshake status, HTTP status, IP family, transport stats, multipath flag, and the full
      `ConnectionConfig` used.

* `out/qlog_files/`

    * `quic-lab.sqlog`, `quic-lab.sqlog.1`, …
      Aggregated qlog 0.4 JSON-SEQ logs across all connections. A single global mux (`QlogMux`) writes one record-separated stream, optionally
      minimized for qvis (`MINIMIZE_QLOG = true`).

* `out/keylog_files/`

    * `quic-lab.keylog`, `quic-lab.keylog.1`, …
      Rotated TLS key logs (if `save_keylog_files = true`), suitable for decrypting traffic in Wireshark.

* `out/session_files/`

    * Sharded session resumption blobs `<shard>/<host>.session` (if `save_session_files = true`).

Rotations are handled by a generic `RotatingWriter`:

* new files are created once `max_bytes` for a given artefact is exceeded,
* hooks (`NewFileHook`) are invoked for header setup (e.g. qlog JSON-SEQ header),
* names follow `base`, `base.1`, `base.2`, ….

---

## HTTP/3 probe

The default probe (`probes::h3`) implements a minimal HTTP/3 client:

* performs a QUIC handshake with parameters from `ConnectionConfig`,

* negotiates ALPN (`h3`) via tquic,

* opens a client-initiated stream and sends a GET request:

  ```rust
  Header::new(b":method",  b"GET");
  Header::new(b":scheme",  b"https");
  Header::new(b":authority", host.as_bytes());
  Header::new(b":path",    path.as_bytes());
  Header::new(b"user-agent", user_agent.as_bytes());
  ```

* drains the response body (without storing it),

* records the final HTTP status code and transport stats.

The runner currently invokes the HTTP/3 probe here:

```rust
// runner/src/main.rs
domains.par_iter().for_each( | host| {
if let Err(e) = probes::h3::probe(
host,
& cfg.scheduler,
& cfg.io,
& cfg.general,
& cfg.connection_config,
& rl,
& recorder,
) {
// error handling ...
}
});

```

To use a different probe, adjust this call accordingly (see below).

---

## Writing custom probes

Probes are separate crates in `probes/` and are built on top of the shared QUIC transport via the `AppProtocol` trait:

```rust
pub trait AppProtocol {
    fn on_connected(&mut self, _conn: &mut Connection) {}
    fn on_stream_readable(&mut self, _conn: &mut Connection, _stream_id: u64) {}
    fn on_stream_writable(&mut self, _conn: &mut Connection, _stream_id: u64) {}
    fn on_stream_closed(&mut self, _conn: &mut Connection, _stream_id: u64) {}
    fn on_conn_closed(&mut self, _conn: &mut Connection) {}
}
```

A fully documented example is provided in `probes/src/template.rs`. The general pattern:

1. **Define shared state**

   ```rust
   #[derive(Debug, Default)]
   struct TemplateState {
       trace_id: Option<String>,
       handshake_ok: bool,
       // add fields for your metrics
   }
   ```

2. **Implement `TemplateApp`**

   ```rust
   struct TemplateApp {
       host: String,
       shared: Arc<Mutex<TemplateState>>,
   }

   impl AppProtocol for TemplateApp {
       fn on_connected(&mut self, conn: &mut Connection) {
           let mut st = self.shared.lock().unwrap();
           st.trace_id = Some(conn.trace_id().to_string());
           st.handshake_ok = true;
           // send your frames / requests here
       }

       fn on_stream_readable(&mut self, conn: &mut Connection, stream_id: u64) {
           // read from streams, update state
       }

       fn on_conn_closed(&mut self, conn: &mut Connection) {
           // final updates, error information, etc.
       }
   }
   ```

3. **Record results via `Recorder`**

   The template shows how to construct a `TemplateResult` struct and write it to the JSONL recorder using a stable key (typically the tquic
   `trace_id`).

4. **Expose `probe()`**

   Provide a `probe()` function with the same signature as `h3::probe`, reusing resolution, rate limiting and retry logic.

5. **Hook into the runner**

   In `runner/src/main.rs`, swap:

   ```rust
   probes::h3::probe(...)
   ```

   for your custom probe, e.g.:

   ```rust
   probes::template::probe(...)
   ```

This design allows the core to handle concurrency, rate limiting, logging and qlog/recorder files, while probe authors implement only the
application-layer logic.

---

## Logging, qlog and keylog

* **Logging** (`core::logging`):

    * Uses `tracing-subscriber` with optional `RUST_LOG` overrides.
    * Logs go to stdout/stderr and `out/log_files/quic-lab.log` (rotated).

* **qlog** (`core::qlog`):

    * Aggregates per-connection JSON-SEQ streams into a single `.sqlog` file.
    * Injects `group_id` and enforces strictly monotonic timestamps per connection.
    * Optionally minimizes events and payloads for qvis and custom statistics via `MINIMIZE_QLOG`.

* **Keylog** (`core::keylog`):

    * When enabled, creates `out/keylog_files/quic-lab.keylog[.N]`.
    * Connections get a `PerConnKeylog` writer; Wireshark can use these files for TLS 1.3 decryption.

The HTTP/3 probe hooks qlog and keylog in `on_conn_created`.

---

## Ethics and responsible use

This framework is capable of generating significant amounts of network traffic. Use it responsibly:

* respect local laws, institutional policies and acceptable-use guidelines,
* keep `requests_per_second` and `burst` at conservative values for Internet-wide scans,
* provide a valid contact in `ConnectionConfig.user_agent` (e.g. `"… opt-out: you@example.org"`),
* honour opt-out requests you receive,
* avoid probing networks or hosts where you do not have permission.

The defaults are tuned for research-oriented scanning with an emphasis on safety and observability (logging, recorder, qlog).

---

## Contributing

Contributions are welcome. Typical ways to contribute:

* bug reports or feature requests via GitHub Issues,
* pull requests that:

    * add new probes under `probes/`,
    * improve documentation or configuration examples,
    * extend analysis tooling for qlog / recorder outputs.

Before opening a large PR, align on the intended design via an issue or discussion.

---

## License

This project is open source. The exact terms are defined in the `LICENSE` file shipped with this repository.
