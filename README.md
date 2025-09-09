# quic-lab

*A modular Rust workspace for Internet-scale QUIC/HTTP/3 measurement and research.*

`quic-lab` is a research-grade toolkit for probing modern transport and application layer protocols such as **QUIC** and **HTTP/3**.  
It is designed to be **modular**, **reproducible**, and **ethically responsible**, forming part of the Master’s thesis *Design and Implementation of Novel Transport and Application Layer Measurement Techniques* at the University of Zürich.

---

## Features

- QUIC transport setup and HTTP/3 handshake probing (via [quiche](https://github.com/cloudflare/quiche))
- IPv4/IPv6 family handling (`auto`, `v4`, `v6`, `both`)
- Per-domain configuration overrides (timeouts, ALPN, QUIC parameters)
- Extendable probe system (e.g., connection migration, 0-RTT, Multipath QUIC)
- Structured results (human-readable logs and machine-parsable output planned)

---

## Repository Layout

```

quic-lab/
├── Cargo.toml              # Workspace definition
├── crates/
│   ├── core/               # Shared library: config, resolver, transport, logging, types
│   ├── probes/             # Probe implementations (h3, ...)
│   └── runner/             # CLI entry point
├── config.toml.example     # Example configuration
├── domains.txt.example     # Example domain list
└── tests/                  # Minimal test scaffold

````

- **core**: configuration, DNS resolution, QUIC transport, types, logging
- **probes**: independent measurement modules (e.g. `h3`)
- **runner**: CLI wiring everything together

---

## Quick Start

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable)
- C toolchain + [CMake](https://cmake.org/) (required by `quiche`)
- [NASM](https://www.nasm.us/) assembler
- On Windows: install Visual Studio Build Tools + CMake + NASM

### Setup and Build

```bash
# Clone and enter repository
git clone https://github.com/QUIC-Lab/quic-lab.git
cd quic-lab

# Copy the templates for config and domain list
cp config.toml.example config.toml
cp domains.txt.example domains.txt

# Build the workspace
cargo build --workspace
````

### Run

```bash
# Run the CLI with config and domain list
# If not specified, defaults to config.toml and domains.txt
cargo run -p runner -- config.toml domains.txt

# There are also aliases available for convenience:
# cargo run -p runner
cargo r

# cargo build -p runner
cargo b

# cargo test -p runner
cargo t
```

### Aliases

```bash
# cargo build -p runner
cargo b

# cargo test -p runner
cargo t

# cargo run -p runner
cargo r
````

---

## Example Output

```
[IPv4] Connecting to 104.16.133.229:443 (IPv4)
==> cloudflare.com:443 /
   QUIC handshake: OK (ALPN: h3)
   HTTP/3 support: YES (status 301)
```

---

## Adding a New Probe

1. Create a new module under `crates/probes/src/` and implement probe logic.
2. Create a new module under `core::transport` if another transport layer protocol is needed.
4. Register it in `runner` for CLI selection.

Example skeleton for probe implementation:

```rust
pub fn probe(host: &str, cfg: &DomainConfig) -> Result<()> {
    // Resolve address
    // Run transport logic
    // Collect structured results
    Ok(())
}
```

---

## Research Context

This project is part of a Master’s thesis at the University of Zürich, [Department of Informatics](https://www.ifi.uzh.ch/)  
**Title**: *Design and Implementation of Novel Transport and Application Layer Measurement Techniques*

The toolkit is designed to study advanced QUIC/HTTP/3 features such as:

* Multipath QUIC
* Connection migration (client/server side)
* 0-RTT resumption
* GREASE

---

## Contributing

Contributions are welcome!
Please open an [issue](../../issues) or [pull request](../../pulls) with your suggestions.

---

## License

This project is dual-licensed under either:

* **MIT License** ([LICENSE-MIT](LICENSE-MIT) or [https://opensource.org/licenses/MIT](https://opensource.org/licenses/MIT))
* **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE) or [https://www.apache.org/licenses/LICENSE-2.0](https://www.apache.org/licenses/LICENSE-2.0))

at your option.

---

## Citation

If you use this project in academic work, please cite it:

```bibtex
@software{quic_lab_2025,
  title  = {quic-lab: A Modular QUIC Measurement Suite},
  author = {Mete Polat},
  year   = {2025},
  url    = {https://github.com/QUIC-Lab/quic-lab},
  note   = {Version v0.1.0}
}
```

---

## Acknowledgments

* Built on top of [quiche](https://github.com/cloudflare/quiche)
* Supported by the [Communication Systems Group (CSG)](https://www.csg.uzh.ch/) at UZH
