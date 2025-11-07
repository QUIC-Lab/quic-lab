# Domain Extractor

Extracts unique **second-level domains (SLDs)** from very large **TLD zone files** (`.txt`, `.gz`, `.txt.gz`), optionally filters them using public *
*blacklists** (Firebog), and writes:

* `domains.txt` (or `domains_filtered.txt` in filter-only mode)
* `metrics.json` with **per-TLD counts, timings, and throughput**, and overall totals

The script is **stdlib-only** (no `pip install`), streams files line-by-line, deduplicates via a temporary **SQLite** store, and shows a **compact,
low-overhead progress line** (percent by bytes + domains/sec + elapsed).

![python](https://img.shields.io/badge/python-3.9%2B-blue) ![platform](https://img.shields.io/badge/platform-linux%20%7C%20macOS%20%7C%20windows-lightgrey) ![license](https://img.shields.io/badge/license-MIT-green)

---

## Table of Contents

* [Features](#features)
* [Quickstart](#quickstart)
* [CLI](#cli)
* [Inputs & Naming](#inputs--naming)
* [Outputs](#outputs)
* [How It Works](#how-it-works)
* [Progress, Timings & Metrics](#progress-timings--metrics)
* [Performance Tips](#performance-tips)
* [Caveats & Assumptions](#caveats--assumptions)
* [Troubleshooting](#troubleshooting)
* [Contributing](#contributing)
* [License](#license)
* [Acknowledgements](#acknowledgements)

---

## Features

* ✅ **Huge files friendly**: streams `.txt` and `.gz` (20 GB+) without loading into memory
* ✅ **Zero dependencies**: pure Python standard library
* ✅ **Compact progress**: **percent by bytes** (compressed bytes for `.gz`) + **domains/sec** + **elapsed** (throttled to minimize overhead)
* ✅ **Blacklist filtering**: expands Firebog indices and blocks domains by **suffix match**
* ✅ **Per-TLD metrics** + totals, including **time\_seconds**, **extracted\_per\_second**, **kept\_per\_second**
* ✅ **Crash-resilient dedup**: on-disk SQLite with `INSERT OR IGNORE` and large batched writes
* ✅ Cross-platform: Linux, macOS, Windows

---

## Quickstart

```bash
# 1) Put your zone files into /data/zones
#    Examples:
#      com.txt.gz, org.txt.gz, net.txt
#      com.txt.gz  -> TLD inferred as "com"

# 2) Default mode: extract + filter -> domains.txt + metrics.json
python domain_extractor.py /data/zones

# Extract only (no blacklist) -> domains.txt
python domain_extractor.py /data/zones --mode extract

# Filter only (reads domains.txt) -> domains_filtered.txt
python domain_extractor.py /data/zones --mode filter
```

Windows PowerShell:

```powershell
python domain_extractor.py data\zones
```

---

## CLI

```
usage: domain_extractor.py [-h] [--mode MODE] [--output OUTPUT] [--metrics METRICS]
                       [--include-adult] [--no-include-adult]
                       [--include-nocross] [--no-include-nocross]
                       folder
```

* **folder** – directory containing input zone files (for default/`extract`) **or** a `domains.txt` (for `filter`).
* **--mode**

    * `""` or `1` (default): **extract + filter** → writes `domains.txt`
    * `extract` or `2`: **extract only** → writes `domains.txt`
    * `filter` or `3`: **filter only** (reads `domains.txt`) → writes `domains_filtered.txt`
* **--output** – custom output path (optional).
* **--metrics** – custom metrics JSON path (default: `metrics.json` in `folder`).
* **--include-adult** / **--no-include-adult** – toggle Firebog “adult” lists (default: included).
* **--include-nocross** / **--no-include-nocross** – toggle Firebog “nocross” lists (default: included).

> The script always uses **bytes-based progress** with a compact display; there’s no flag required.

---

## Inputs & Naming

The script takes **every file** in the folder with extension `.txt`, `.gz`, or `.txt.gz`.

**TLD inference** comes from the filename (portion before `.txt`/`.gz`):

* `com.txt.gz` → `com`
* `net.txt`    → `net`

Inside zone files, `$ORIGIN` and `@` are respected. Owners are reduced to **second-level domains** (immediate children of the TLD), e.g.:

```
www.api.example.com  ->  example.com
@ (apex of TLD)      ->  ignored
```

---

## Outputs

* `domains.txt` – unique SLDs, sorted ascending
* `domains_filtered.txt` – unique SLDs after filtering (filter-only mode)
* `metrics.json` – counts, timings, and throughput

**`metrics.json` example:**

```json
{
  "per_tld": {
    "com": {
      "extracted": 411863888,
      "kept": 410889905,
      "filtered": 973983,
      "time_seconds": 1825.301,
      "extracted_per_second": 225641.683,
      "kept_per_second": 225108.081
    },
    "net": {
      "extracted": 33760376,
      "kept": 33682896,
      "filtered": 77480,
      "time_seconds": 219.803,
      "extracted_per_second": 153593.912,
      "kept_per_second": 153241.414
    },
    "org": {
      "extracted": 31452608,
      "kept": 31419552,
      "filtered": 33056,
      "time_seconds": 199.759,
      "extracted_per_second": 157452.667,
      "kept_per_second": 157287.188
    }
  },
  "totals": {
    "extracted": 477076872,
    "kept": 475992353,
    "filtered": 1084519,
    "time_seconds": 2244.863,
    "extracted_per_second": 212519.371,
    "kept_per_second": 212036.259
  },
  "times": {
    "per_tld_seconds": {
      "com": 1825.3005535999982,
      "net": 219.80282720000105,
      "org": 199.75913089999813
    },
    "total_seconds": 2244.863
  },
  "output_file": "data/zones/domains.txt"
}
```

---

## How It Works

* **Streaming I/O** – files are read line-by-line. For `.gz`, parsing is via `gzip` (single-threaded); progress is based on the **compressed** stream
  position.
* **Parsing** – lightweight path avoids regex in the hot loop (regex only for `$ORIGIN`).
* **Deduplication** – a temporary **SQLite** DB (in your OS temp dir) acts as a set (`PRIMARY KEY(domain)`), with large batched inserts for speed.
* **Blacklist** – Firebog index pages are fetched; each referenced hosts list is parsed; domains are blocked by **suffix match** (a listed domain
  blocks all its subdomains).

---

## Progress, Timings & Metrics

* **Per file (compact line):**

  ```
  Parsing com.txt.gz  73.42%  ELAPSED 00:12:34  215k/s
  ```

    * Percent is by **bytes processed** (compressed bytes for `.gz`, real bytes for `.txt`).
    * `215k/s` = **domains extracted per second** since the file started.
    * Updates are **throttled** to keep overhead minimal.

* **Downloads**: progress shown by bytes when `Content-Length` is known.

* **Timings**: the script prints `[START]` / `[DONE]` blocks around blacklist build, each file, writing output, and the total run.

* **`metrics.json`**: includes **per-TLD counts + timings + throughput** and totals (see example above).

---

## Performance Tips

* **Fastest win:** if disk space allows, **decompress `.gz` to `.txt`** first and run on `.txt` (gzip decompression is CPU-bound and single-threaded).
* **Runs faster under PyPy** for parse-heavy workloads:

  ```bash
  pypy3 domain_extractor.py data/zones
  ```
* **NVMe SSD** helps most; network fetch for blacklists is usually minor.
* **Tune batch size**: the script uses a large `BATCH_SIZE` for SQLite inserts; you can increase/decrease it in code.
* **Progress overhead** is already low; if you want even leaner UI, adjust the throttling constants in `_iter_lines_bytes_progress_{txt,gz}` (
  `min_time_step`, `min_bytes_step`) or the `Progress(min_interval=...)` argument.

---

## Caveats & Assumptions

* **SLD definition:** outputs the **immediate child** of the TLD (no Public Suffix List logic). If you need PSL-aware registrable domains (e.g.,
  `co.uk`), open an issue/PR.
* **Apex lines** (`@` / the TLD itself) are ignored by design.
* **SQLite tmp file** lives in your OS temp directory and is deleted at the end. With `PRAGMA synchronous=OFF`, a sudden crash can lose the current
  batch (re-run is safe).
* **Encoding:** input decoded as UTF-8 with `errors="replace"` to survive odd bytes.
* **Proxy support:** `urllib` honors standard env vars (`HTTP_PROXY`, `HTTPS_PROXY`, etc.).

---

## Troubleshooting

**Progress stuck at 0% on `.gz`**
Make sure you didn’t alter the gzip iterator internals; the script uses the **compressed** stream position for percent. If you changed the code,
ensure that `_iter_lines_bytes_progress_gz` tracks `comp.tell()` and that the progress bar’s `total` is `path.stat().st_size` for `.gz`.

**“No .txt/.gz files found”**
Check the path and permissions.

**CPU at \~1 core**
Expected while parsing `.gz` (gzip is single-threaded). Try pre-decompressing to `.txt`.

**High memory?**
It should stay modest. If you see spikes, lower `BATCH_SIZE` in the source.

**Windows console artifacts**
Use Windows Terminal/PowerShell and ensure UTF-8; the progress line uses `█` and `·` characters only in some code paths (compact mode is mostly plain
text).

---

## Contributing

Issues and PRs welcome! Ideas:

* Threaded blacklist fetch
* PSL-aware extraction or configurable domain depth
* Optional multiprocess parsing for `.txt` via byte-range sharding
* CSV/Parquet export for metrics and domains

To hack on it:

```bash
python -m venv .venv && source .venv/bin/activate    # Windows: .venv\Scripts\activate
# no dependencies; run the script against test fixtures or your own data
```

---

## License

MIT — see `LICENSE`.

---

## Acknowledgements

* Blacklist indices courtesy of **Firebog**:

    * `https://v.firebog.net/hosts/lists.php?type=nocross`
    * `https://v.firebog.net/hosts/lists.php?type=adult`

> Use of third-party blocklists is at your discretion; verify terms and suitability for your use case.

---
