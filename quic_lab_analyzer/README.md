# QUIC Lab Analyzer

This subproject provides an analysis pipeline for QUIC / HTTP/3 measurement data produced by the main scanning framework. It ingests recorder
output, qlog traces, and application logs, and generates JSON/CSV summaries and PNG plots for further inspection. It is part of the measurement setup
for the master’s thesis *Design and Implementation of Novel Transport and Application Layer Measurement Techniques*.

---

## Features

- Aggregates **recorder output** (connection-level metadata, ALPN, error codes, multipath flags)
- Aggregates **qlog 0.4 JSON-SEQ** traces:
    - Event/name frequencies
    - Frame and packet type distributions
    - Transport parameter distributions (per parameter)
    - Per-direction byte counters
- Aggregates **Rust application logs**:
    - DNS lookup errors
    - Connect errors
    - Other error occurrences
- Computes **cross-dataset statistics**:
    - Overlap of `group_id`/key between recorder and qlog
- Produces:
    - JSON summaries (`*_summary.json`)
    - CSV tables (error distributions, transport parameters, etc.)
    - PNG plots (histograms, bar charts) for key metrics

---

## Input Data Layout

The tool expects a root directory with the following structure:

```text
<data_root>/
  recorder_files/
    quic-lab-recorder.jsonl*
  qlog_files/
    quic-lab.sqlog*
  log_files/
    quic-lab.log*
```

* `recorder_files/`
  Line-delimited JSON (`*.jsonl*`) with keys and values from the recorder module.
  Used for:

    * Handshake success/failure
    * `enable_multipath` flag
    * ALPN distribution
    * Peer/local close error codes
    * Recorder-side `group_id` set

* `qlog_files/`
  QLOG 0.4 JSON-SEQ files (`*.sqlog*`) as produced by the scanning framework.
  Used for:

    * Event, frame, and packet type statistics
    * Transport parameter distributions (remote/“owner=remote”)
    * Path and error event counts
    * Total bytes sent/received
    * QLOG-side `group_id` set

* `log_files/`
  Application logs (`quic-lab.log*`) from the scanner.
  Used for:

    * DNS resolution errors
    * Connect errors
    * Fallback “other” error counts

By default, a local `data_input/` directory (ignored in `.gitignore`) is used as `<data_root>`.

---

## Installation

1. Ensure **Python 3.10+** is available (required for modern typing features).
2. (Recommended) Create and activate a virtual environment.
3. Install dependencies:

```bash
pip install -r requirements.txt
```

---

## Usage

From the directory containing `cli.py`, run:

```bash
python cli.py \
  --root ./data_input \
  --out ./analysis_output
```

### Command-line options

* `--root PATH`
  Root directory containing the three input subdirectories:

    * `recorder_files/`
    * `qlog_files/`
    * `log_files/`
      Default: `./data_input/`

* `--out PATH`
  Output directory for summaries and plots.
  Default: `./analysis_output`
  (This directory is ignored in `.gitignore`.)

* `--workers N`
  Number of worker processes for **qlog** parsing (`ProcessPoolExecutor`).

    * `N > 1`: multi-process qlog parsing
    * `N` omitted or `None`: uses `os.cpu_count()` (or `1` as fallback)

* `--no-plots`
  If present, disables PNG plot generation. Only JSON/CSV summaries are written.

Example:

```bash
python cli.py \
  --root /path/to/data_root \
  --out /path/to/analysis_output \
  --workers 8
```

---

## Output

All outputs are written into the directory passed via `--out`:

### Recorder summaries

* `recorder_summary.json`

    * `total_records`
    * `handshake_ok_counts`
    * `enable_multipath_counts`
    * `alpn_counts`
    * `peer_close_error_codes`
    * `local_close_error_codes`
    * `unique_group_ids`

* `recorder_alpn_counts.csv`

* `recorder_peer_close_error_codes.csv`

* `recorder_local_close_error_codes.csv`

If plotting is enabled:

* `recorder_handshake_ok.png`
* `recorder_alpn.png`
* `recorder_peer_close_error_codes.png`
* `recorder_local_close_error_codes.png`

### QLOG summaries

* `qlog_summary.json`

    * `total_events`, `invalid_events`
    * `event_name_counts`
    * `frame_type_counts`
    * `packet_type_counts` (keyed by `direction:packet_type`)
    * `path_event_counts`, `error_event_counts`
    * `transport_params` (per parameter value distribution)
    * `unique_group_ids`
    * `total_bytes_sent`, `total_bytes_received`

* `qlog_event_name_counts.csv`

* `qlog_frame_type_counts.csv`

* `qlog_packet_type_counts.csv`

* `qlog_transport_param_<param>.csv` (one CSV per observed transport parameter)

If plotting is enabled:

* `qlog_event_names_top20.png`
* `qlog_frame_types_top20.png`
* `qlog_packet_types_sent_top20.png`
* `qlog_packet_types_received_top20.png`
* `qlog_param_max_idle_timeout_hist.png` (if numeric values present)
* `qlog_param_initial_max_data_hist.png` (if numeric values present)

### Log summaries

* `logs_summary.json`

    * `error_counts` (DNS/connect/other)
    * `dns_error_counts`
    * `connect_error_counts`

* `logs_dns_error_counts.csv`

* `logs_connect_error_counts.csv`

If plotting is enabled:

* `logs_dns_errors_top10.png`
* `logs_connect_errors_top10.png`

### Cross-dataset statistics

If any `group_id`/key is observed in recorder or qlog data:

* `cross_summary.json`

    * `recorder_group_ids`
    * `qlog_group_ids`
    * `group_ids_both`
    * `group_ids_only_recorder`
    * `group_ids_only_qlog`

If plotting is enabled:

* `cross_group_id_overlap.png`
