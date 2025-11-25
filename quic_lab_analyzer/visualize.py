from __future__ import annotations

from pathlib import Path
from typing import Dict, Any

import matplotlib
matplotlib.use("Agg")  # non-interactive backend

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd

from recorder_analyzer import RecorderAnalyzer
from qlog_analyzer import QlogAnalyzer
from log_analyzer import LogAnalyzer


class Visualizer:
    def __init__(self, out_dir: Path) -> None:
        self.out_dir = out_dir

    # ------------- Recorder -------------

    def plot_recorder(self, rec: RecorderAnalyzer) -> None:
        if rec.total_records == 0:
            return

        self._bar_from_counter(
            rec.handshake_ok_counts,
            "Handshake success vs failure",
            "handshake_ok",
            "connections",
            self.out_dir / "recorder_handshake_ok.png",
            )
        self._bar_from_counter(
            rec.alpn_counts,
            "ALPN distribution",
            "ALPN",
            "connections",
            self.out_dir / "recorder_alpn.png",
            )
        self._bar_from_counter(
            rec.peer_close_error_codes,
            "Peer close error codes",
            "error_code",
            "connections",
            self.out_dir / "recorder_peer_close_error_codes.png",
            top_n=20,
            )
        self._bar_from_counter(
            rec.local_close_error_codes,
            "Local close error codes",
            "error_code",
            "connections",
            self.out_dir / "recorder_local_close_error_codes.png",
            top_n=20,
            )

    # ------------- QLOG -------------

    def plot_qlog(self, qlog: QlogAnalyzer) -> None:
        if qlog.total_events == 0:
            return

        self._bar_from_counter(
            qlog.event_name_counts,
            "QLOG event names (top 20)",
            "event_name",
            "count",
            self.out_dir / "qlog_event_names_top20.png",
            top_n=20,
            )
        self._bar_from_counter(
            qlog.frame_type_counts,
            "QLOG frame types (top 20)",
            "frame_type",
            "count",
            self.out_dir / "qlog_frame_types_top20.png",
            top_n=20,
            )

        # Packet types by direction
        if qlog.packet_type_counts:
            rows = []
            for (direction, pkt_type), count in qlog.packet_type_counts.items():
                rows.append({"direction": direction, "packet_type": pkt_type, "count": count})
            df = pd.DataFrame(rows)
            for direction, sub in df.groupby("direction"):
                sub_sorted = sub.sort_values("count", ascending=False).head(20)
                plt.figure(figsize=(10, 5))
                plt.bar(sub_sorted["packet_type"].astype(str), sub_sorted["count"])
                plt.xticks(rotation=45, ha="right")
                plt.tight_layout()
                plt.title(f"Packet types ({direction})")
                plt.xlabel("packet_type")
                plt.ylabel("count")
                out_path = self.out_dir / f"qlog_packet_types_{direction}_top20.png"
                plt.savefig(out_path)
                plt.close()

        # Selected numeric transport params
        for param in ("max_idle_timeout", "initial_max_data"):
            dist = qlog.transport_param_counts.get(param)
            if not dist:
                continue
            numeric = {k: v for k, v in dist.items() if isinstance(k, (int, float))}
            if not numeric:
                # try to coerce
                tmp = {}
                for k, v in dist.items():
                    try:
                        tmp[int(k)] = v
                    except Exception:
                        continue
                numeric = tmp
            if not numeric:
                continue

            values = np.array(list(numeric.keys()), dtype=float)
            weights = np.array(list(numeric.values()), dtype=float)

            plt.figure(figsize=(10, 5))
            plt.hist(values, bins=50, weights=weights)
            plt.title(f"Distribution of {param}")
            plt.xlabel(param)
            plt.ylabel("connections")
            plt.tight_layout()
            out_path = self.out_dir / f"qlog_param_{param}_hist.png"
            plt.savefig(out_path)
            plt.close()

    # ------------- Logs -------------

    def plot_logs(self, logs: LogAnalyzer) -> None:
        if not logs.error_counts:
            return

        self._bar_from_counter(
            logs.dns_error_counts,
            "DNS error messages (top 10)",
            "error_message",
            "count",
            self.out_dir / "logs_dns_errors_top10.png",
            top_n=10,
            )
        self._bar_from_counter(
            logs.connect_error_counts,
            "Connect error messages (top 10)",
            "error_message",
            "count",
            self.out_dir / "logs_connect_errors_top10.png",
            top_n=10,
            )

    # ------------- Cross summary -------------

    def plot_cross(self, cross_summary: Dict[str, Any]) -> None:
        if not cross_summary:
            return
        labels = [
            "recorder_group_ids",
            "qlog_group_ids",
            "group_ids_both",
            "group_ids_only_recorder",
            "group_ids_only_qlog",
        ]
        values = [cross_summary.get(k, 0) for k in labels]
        plt.figure(figsize=(8, 5))
        plt.bar(labels, values)
        plt.xticks(rotation=30, ha="right")
        plt.ylabel("count")
        plt.title("Group ID overlap (recorder vs qlog)")
        plt.tight_layout()
        out_path = self.out_dir / "cross_group_id_overlap.png"
        plt.savefig(out_path)
        plt.close()

    # ------------- Helpers -------------

    def _bar_from_counter(
            self,
            counter,
            title: str,
            xlabel: str,
            ylabel: str,
            out_path: Path,
            top_n: int | None = None,
    ) -> None:
        if not counter:
            return
        items = sorted(counter.items(), key=lambda x: (-x[1], str(x[0])))
        if top_n is not None:
            items = items[:top_n]
        labels = [str(k) for k, _ in items]
        values = [v for _, v in items]

        plt.figure(figsize=(10, 5))
        plt.bar(labels, values)
        plt.xticks(rotation=45, ha="right")
        plt.xlabel(xlabel)
        plt.ylabel(ylabel)
        plt.title(title)
        plt.tight_layout()
        plt.savefig(out_path)
        plt.close()
