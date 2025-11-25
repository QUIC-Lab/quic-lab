from __future__ import annotations

import json
import logging
from collections import Counter, defaultdict
from concurrent.futures import ProcessPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Set, Any

from tqdm import tqdm


def _extract_packet_size(event: dict) -> Optional[int]:
    data = event.get("data") or {}
    header = data.get("header") or {}

    for container in (header, data):
        for key in ("packet_size", "payload_length", "length"):
            val = container.get(key)
            if isinstance(val, int):
                return val

    return None


def _process_qlog_file(path_str: str) -> dict:
    path = Path(path_str)
    result = {
        "file": str(path),
        "total_events": 0,
        "invalid_events": 0,
        "event_name_counts": Counter(),
        "packet_type_counts": Counter(),  # key: (direction, packet_type)
        "frame_type_counts": Counter(),
        "path_event_counts": Counter(),
        "error_event_counts": Counter(),
        "transport_param_counts": defaultdict(Counter),  # param_name -> Counter(value)
        "group_ids": set(),
        "total_bytes_sent": 0,
        "total_bytes_received": 0,
    }

    with path.open("r", encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            if line.startswith("\x1e"):
                line = line[1:]
            if not line:
                continue

            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                result["invalid_events"] += 1
                continue

            result["total_events"] += 1

            group_id = ev.get("group_id")
            if isinstance(group_id, str):
                result["group_ids"].add(group_id)

            name = ev.get("name")
            if not isinstance(name, str):
                continue

            result["event_name_counts"][name] += 1

            lname = name.lower()
            if "error" in lname or "closed" in lname or "connection_lost" in lname:
                result["error_event_counts"][name] += 1
            if name.startswith("quic:path_"):
                result["path_event_counts"][name] += 1

            # Packet-level analysis
            if name in ("quic:packet_sent", "quic:packet_received"):
                direction = "sent" if name.endswith("sent") else "received"
                data = ev.get("data") or {}
                header = data.get("header") or {}

                pkt_type = header.get("packet_type") or "<unknown>"
                result["packet_type_counts"][(direction, pkt_type)] += 1

                size = _extract_packet_size(ev)
                if size is None:
                    size = 0

                if direction == "sent":
                    result["total_bytes_sent"] += size
                else:
                    result["total_bytes_received"] += size

                # Frame types
                frames = data.get("frames") or []
                if isinstance(frames, list):
                    for fr in frames:
                        if not isinstance(fr, dict):
                            continue
                        ft = fr.get("frame_type")
                        if ft is None:
                            continue
                        result["frame_type_counts"][ft] += 1

            # Transport parameters (server side)
            if name == "quic:parameters_set":
                data = ev.get("data") or {}
                if data.get("owner") != "remote":
                    continue
                for key, val in data.items():
                    if key in ("owner",):
                        continue
                    if val is None:
                        continue
                    if isinstance(val, (bool, int, float, str)):
                        v_key: Any = val
                    else:
                        v_key = str(val)
                    result["transport_param_counts"][key][v_key] += 1

    # Convert sets/defaultdict for pickling
    result["group_ids"] = list(result["group_ids"])
    result["transport_param_counts"] = {
        k: dict(v) for k, v in result["transport_param_counts"].items()
    }

    return result


@dataclass
class QlogAnalyzer:
    workers: int = 1

    total_events: int = 0
    invalid_events: int = 0
    event_name_counts: Counter = field(default_factory=Counter)
    packet_type_counts: Counter = field(default_factory=Counter)
    frame_type_counts: Counter = field(default_factory=Counter)
    path_event_counts: Counter = field(default_factory=Counter)
    error_event_counts: Counter = field(default_factory=Counter)
    transport_param_counts: Dict[str, Counter] = field(default_factory=dict)
    group_ids: Set[str] = field(default_factory=set)
    total_bytes_sent: int = 0
    total_bytes_received: int = 0

    def process_directory(self, qlog_dir: Path) -> None:
        if not qlog_dir.exists():
            logging.warning("QLOG directory %s does not exist, skipping.", qlog_dir)
            return

        files = sorted(qlog_dir.glob("quic-lab.sqlog*"))
        if not files:
            logging.warning("No qlog files found in %s", qlog_dir)
            return

        logging.info(
            "Processing %d qlog files with %d workers ...",
            len(files),
            self.workers,
        )

        if self.workers <= 1:
            for path in tqdm(files, desc="QLOG files"):
                res = _process_qlog_file(str(path))
                self._merge_result(res)
        else:
            with ProcessPoolExecutor(max_workers=self.workers) as ex:
                futures = {ex.submit(_process_qlog_file, str(p)): p for p in files}
                for fut in tqdm(as_completed(futures), total=len(files), desc="QLOG files"):
                    res = fut.result()
                    self._merge_result(res)

        logging.info(
            "QLOG: %d valid events, %d invalid events, %d unique group_ids",
            self.total_events,
            self.invalid_events,
            len(self.group_ids),
        )

    def _merge_result(self, res: dict) -> None:
        self.total_events += res.get("total_events", 0)
        self.invalid_events += res.get("invalid_events", 0)
        self.total_bytes_sent += res.get("total_bytes_sent", 0)
        self.total_bytes_received += res.get("total_bytes_received", 0)

        self.group_ids.update(res.get("group_ids") or [])

        self.event_name_counts.update(res.get("event_name_counts") or {})
        self.packet_type_counts.update(res.get("packet_type_counts") or {})
        self.frame_type_counts.update(res.get("frame_type_counts") or {})
        self.path_event_counts.update(res.get("path_event_counts") or {})
        self.error_event_counts.update(res.get("error_event_counts") or {})

        for param, counts in (res.get("transport_param_counts") or {}).items():
            c = self.transport_param_counts.setdefault(param, Counter())
            c.update(counts)

    def to_dict(self) -> dict:
        return {
            "total_events": self.total_events,
            "invalid_events": self.invalid_events,
            "event_name_counts": dict(self.event_name_counts),
            "packet_type_counts": {
                f"{direction}:{ptype}": count
                for (direction, ptype), count in self.packet_type_counts.items()
            },
            "frame_type_counts": dict(self.frame_type_counts),
            "path_event_counts": dict(self.path_event_counts),
            "error_event_counts": dict(self.error_event_counts),
            "transport_params": {
                name: dict(counter) for name, counter in self.transport_param_counts.items()
            },
            "unique_group_ids": len(self.group_ids),
            "total_bytes_sent": self.total_bytes_sent,
            "total_bytes_received": self.total_bytes_received,
        }

    def write_summary(self, out_dir: Path) -> None:
        out_dir.mkdir(parents=True, exist_ok=True)

        import json

        summary_path = out_dir / "qlog_summary.json"
        with summary_path.open("w", encoding="utf-8") as f:
            json.dump(self.to_dict(), f, indent=2, sort_keys=True)
        logging.info("Wrote qlog summary to %s", summary_path)

        self._write_counter_csv(
            out_dir / "qlog_event_name_counts.csv",
            self.event_name_counts,
            ["event_name", "count"],
            )
        self._write_counter_csv(
            out_dir / "qlog_frame_type_counts.csv",
            self.frame_type_counts,
            ["frame_type", "count"],
            )
        self._write_counter_csv(
            out_dir / "qlog_packet_type_counts.csv",
            self.packet_type_counts,
            ["direction", "packet_type", "count"],
            is_pair=True,
            )
        # Transport parameters: one CSV per parameter
        for param, counter in self.transport_param_counts.items():
            safe_name = param.replace(" ", "_")
            path = out_dir / f"qlog_transport_param_{safe_name}.csv"
            self._write_counter_csv(path, counter, [param, "count"])

    @staticmethod
    def _write_counter_csv(
            path: Path,
            counter: Counter,
            header: List[str],
            is_pair: bool = False,
    ) -> None:
        if not counter:
            return
        with path.open("w", encoding="utf-8") as f:
            f.write(",".join(header) + "\n")
            if is_pair:
                for (direction, pkt_type), count in sorted(
                        counter.items(), key=lambda x: (-x[1], str(x[0]))
                ):
                    f.write(f"{direction},{pkt_type},{count}\n")
            else:
                for key, count in sorted(counter.items(), key=lambda x: (-x[1], str(x[0]))):
                    f.write(f"{key},{count}\n")
