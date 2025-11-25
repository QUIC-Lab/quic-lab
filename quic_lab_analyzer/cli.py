import argparse
import json
import logging
import os
from pathlib import Path

from log_analyzer import LogAnalyzer
from qlog_analyzer import QlogAnalyzer
from recorder_analyzer import RecorderAnalyzer
from visualize import Visualizer


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze QUIC / HTTP/3 measurements (recorder + qlogs + logs)."
    )
    parser.add_argument(
        "--root",
        type=str,
        default="./data_input/",
        help="Root directory containing recorder_files, qlog_files, log_files.",
    )
    parser.add_argument(
        "--out",
        type=str,
        default="./analysis_output",
        help="Output directory for summaries and plots.",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=None,
        help="Number of worker processes for qlog parsing (default: CPU count).",
    )
    parser.add_argument(
        "--no-plots",
        action="store_true",
        help="Disable plot generation.",
    )

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(message)s",
    )

    root = Path(args.root)
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    recorder_dir = root / "recorder_files"
    qlog_dir = root / "qlog_files"
    log_dir = root / "log_files"

    logging.info("Root: %s", root)
    logging.info("Output: %s", out_dir)

    # 1) Recorder
    recorder = RecorderAnalyzer()
    recorder.process_directory(recorder_dir)
    recorder.write_summary(out_dir)

    # 2) QLOG (multiprocessing)
    qlog = QlogAnalyzer(workers=args.workers or os.cpu_count() or 1)
    qlog.process_directory(qlog_dir)
    qlog.write_summary(out_dir)

    # 3) Rust logs
    logs = LogAnalyzer()
    logs.process_directory(log_dir)
    logs.write_summary(out_dir)

    # 4) Cross-set stats (group_id overlap recorder <-> qlog)
    cross_summary = {}
    if recorder.group_ids or qlog.group_ids:
        r_ids = recorder.group_ids
        q_ids = qlog.group_ids
        cross_summary = {
            "recorder_group_ids":      len(r_ids),
            "qlog_group_ids":          len(q_ids),
            "group_ids_both":          len(r_ids & q_ids),
            "group_ids_only_recorder": len(r_ids - q_ids),
            "group_ids_only_qlog":     len(q_ids - r_ids),
        }
        cross_path = out_dir / "cross_summary.json"
        with cross_path.open("w", encoding="utf-8") as f:
            json.dump(cross_summary, f, indent=2, sort_keys=True)
        logging.info("Wrote cross summary to %s", cross_path)

    # 5) Plots
    if not args.no_plots:
        vis = Visualizer(out_dir)
        vis.plot_recorder(recorder)
        vis.plot_qlog(qlog)
        vis.plot_logs(logs)
        vis.plot_cross(cross_summary)
