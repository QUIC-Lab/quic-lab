from __future__ import annotations

import json
import logging
import re
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import Set

from tqdm import tqdm

ERROR_CODE_RE = re.compile(r"error_code=(\d+)")


@dataclass
class RecorderAnalyzer:
    total_records: int = 0
    handshake_ok_counts: Counter = field(default_factory=Counter)
    enable_multipath_counts: Counter = field(default_factory=Counter)
    alpn_counts: Counter = field(default_factory=Counter)
    peer_close_error_codes: Counter = field(default_factory=Counter)
    local_close_error_codes: Counter = field(default_factory=Counter)
    group_ids: Set[str] = field(default_factory=set)

    def process_directory(self, recorder_dir: Path) -> None:
        if not recorder_dir.exists():
            logging.warning("Recorder directory %s does not exist, skipping.", recorder_dir)
            return

        files = sorted(recorder_dir.glob("quic-lab-recorder.jsonl*"))
        if not files:
            logging.warning("No recorder files found in %s", recorder_dir)
            return

        logging.info("Processing %d recorder files ...", len(files))
        for path in tqdm(files, desc="Recorder files"):
            self.process_file(path)

        logging.info(
            "Recorder: %d records, %d unique group_ids",
            self.total_records,
            len(self.group_ids),
        )

    def process_file(self, path: Path) -> None:
        with path.open("r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue

                key = rec.get("key")
                if isinstance(key, str):
                    self.group_ids.add(key)

                value = rec.get("value") or {}
                self.total_records += 1

                # handshake_ok
                self.handshake_ok_counts[value.get("handshake_ok", None)] += 1

                # enable_multipath
                self.enable_multipath_counts[value.get("enable_multipath", None)] += 1

                # ALPN
                alpn = value.get("alpn")
                if alpn is None:
                    alpn = "<none>"
                self.alpn_counts[alpn] += 1

                # peer_close / local_close error codes
                peer_close = value.get("peer_close")
                if isinstance(peer_close, str):
                    m = ERROR_CODE_RE.search(peer_close)
                    if m:
                        self.peer_close_error_codes[int(m.group(1))] += 1

                local_close = value.get("local_close")
                if isinstance(local_close, str):
                    m = ERROR_CODE_RE.search(local_close)
                    if m:
                        self.local_close_error_codes[int(m.group(1))] += 1

    def to_dict(self) -> dict:
        return {
            "total_records": self.total_records,
            "handshake_ok_counts": dict(self.handshake_ok_counts),
            "enable_multipath_counts": dict(self.enable_multipath_counts),
            "alpn_counts": dict(self.alpn_counts),
            "peer_close_error_codes": dict(self.peer_close_error_codes),
            "local_close_error_codes": dict(self.local_close_error_codes),
            "unique_group_ids": len(self.group_ids),
        }

    def write_summary(self, out_dir: Path) -> None:
        out_dir.mkdir(parents=True, exist_ok=True)

        summary_path = out_dir / "recorder_summary.json"
        with summary_path.open("w", encoding="utf-8") as f:
            json.dump(self.to_dict(), f, indent=2, sort_keys=True)
        logging.info("Wrote recorder summary to %s", summary_path)

        self._write_counter_csv(
            out_dir / "recorder_alpn_counts.csv",
            self.alpn_counts,
            ["alpn", "count"],
            )
        self._write_counter_csv(
            out_dir / "recorder_peer_close_error_codes.csv",
            self.peer_close_error_codes,
            ["error_code", "count"],
            )
        self._write_counter_csv(
            out_dir / "recorder_local_close_error_codes.csv",
            self.local_close_error_codes,
            ["error_code", "count"],
            )

    @staticmethod
    def _write_counter_csv(path: Path, counter: Counter, header: list[str]) -> None:
        if not counter:
            return
        with path.open("w", encoding="utf-8") as f:
            f.write(",".join(header) + "\n")
            for key, count in sorted(counter.items(), key=lambda x: (-x[1], x[0])):
                f.write(f"{key},{count}\n")
