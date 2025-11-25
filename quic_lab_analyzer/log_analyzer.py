from __future__ import annotations

import logging
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import List

from tqdm import tqdm


@dataclass
class LogAnalyzer:
    error_counts: Counter = field(default_factory=Counter)
    dns_error_counts: Counter = field(default_factory=Counter)
    connect_error_counts: Counter = field(default_factory=Counter)

    def process_directory(self, log_dir: Path) -> None:
        if not log_dir.exists():
            logging.warning("Log directory %s does not exist, skipping.", log_dir)
            return

        files = sorted(log_dir.glob("quic-lab.log*"))
        if not files:
            logging.warning("No log files found in %s", log_dir)
            return

        logging.info("Processing %d log files ...", len(files))
        for path in tqdm(files, desc="Log files"):
            self.process_file(path)

        logging.info(
            "Logs: %d DNS errors, %d connect errors",
            sum(self.dns_error_counts.values()),
            sum(self.connect_error_counts.values()),
        )

    def process_file(self, path: Path) -> None:
        with path.open("r", encoding="utf-8", errors="replace") as f:
            for line in f:
                self._process_line(line.rstrip("\n"))

    def _process_line(self, line: str) -> None:
        if "ERROR: failed to lookup address information:" in line:
            self.error_counts["dns_lookup"] += 1
            msg = line.split("ERROR: failed to lookup address information:", 1)[1].strip()
            self.dns_error_counts[msg] += 1
            return

        if " connect " in line and " err:" in line:
            self.error_counts["connect"] += 1
            msg = line.split(" err:", 1)[1].strip()
            self.connect_error_counts[msg] += 1
            return

        if "ERROR" in line:
            self.error_counts["other_error"] += 1

    def to_dict(self) -> dict:
        return {
            "error_counts": dict(self.error_counts),
            "dns_error_counts": dict(self.dns_error_counts),
            "connect_error_counts": dict(self.connect_error_counts),
        }

    def write_summary(self, out_dir: Path) -> None:
        out_dir.mkdir(parents=True, exist_ok=True)

        import json

        summary_path = out_dir / "logs_summary.json"
        with summary_path.open("w", encoding="utf-8") as f:
            json.dump(self.to_dict(), f, indent=2, sort_keys=True)
        logging.info("Wrote logs summary to %s", summary_path)

        self._write_counter_csv(
            out_dir / "logs_dns_error_counts.csv",
            self.dns_error_counts,
            ["dns_error_message", "count"],
            )
        self._write_counter_csv(
            out_dir / "logs_connect_error_counts.csv",
            self.connect_error_counts,
            ["connect_error_message", "count"],
            )

    @staticmethod
    def _write_counter_csv(path: Path, counter: Counter, header: List[str]) -> None:
        if not counter:
            return
        with path.open("w", encoding="utf-8") as f:
            f.write(",".join(header) + "\n")
            for key, count in sorted(counter.items(), key=lambda x: (-x[1], str(x[0]))):
                f.write(f"{key},{count}\n")
