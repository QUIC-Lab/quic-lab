#!/usr/bin/env python3
import argparse
import gzip
import io
import json
import math
import re
import sqlite3
import sys
import tempfile
import time
import urllib.request
from collections import defaultdict
from contextlib import contextmanager
from pathlib import Path
from typing import Iterable, Optional, Set, Tuple, Iterator


# --- Progressbar & timers (stdlib-only) ---
def fmt_hhmmss(seconds: float) -> str:
    m, s = divmod(int(seconds), 60)
    h, m = divmod(m, 60)
    return f"{h:02d}:{m:02d}:{s:02d}"


def fmt_rate(n: float) -> str:
    """Format a rate like 15320.4 -> '15.3k' (no unit)."""
    if n < 1000:
        return f"{n:.0f}"
    for unit in ("k", "M", "B", "T"):
        n /= 1000.0
        if abs(n) < 1000:
            return f"{n:.1f}{unit}"
    return f"{n:.1f}P"


class Progress:
    """
    Console progress without external libs.
    - Determinate (when total known) or indeterminate spinner (when total unknown).
    - 'compact' for a minimal, fast rendering.
    - Throttled redraws to avoid slow TTY updates.
    """
    SPINNER = "|/-\\"

    def __init__(self, prefix: str = "", total: int | None = None, width: int = 10, min_interval: float = 0.25, compact: bool = False, ):
        self.prefix = prefix
        self.total = total
        self.width = width
        self.min_interval = min_interval
        self.compact = compact
        self.start = time.perf_counter()
        self.last_draw = 0.0
        self.count = 0
        self._spin_idx = 0
        self._done = False
        self.extra = ""  # extra text appended (e.g., "215k/s")

    def set_extra(self, text: str):
        self.extra = text

    def update(self, n: int = 1):
        self.count += n
        now = time.perf_counter()
        if now - self.last_draw >= self.min_interval:
            self._draw(now)
            self.last_draw = now

    def _draw(self, now: float):
        elapsed = now - self.start
        if self.total is not None and self.total > 0:
            ratio = min(1.0, self.count / self.total)
            if self.compact:
                msg = f"{self.prefix} {ratio * 100:6.2f}%  ELAPSED {fmt_hhmmss(elapsed)}"
            else:
                filled = int(self.width * ratio)
                bar = "█" * filled + "·" * (self.width - filled)
                rate = self.count / elapsed if elapsed > 0 else 0
                remaining = (self.total - self.count) / rate if rate > 0 else math.inf
                msg = (f"{self.prefix} [{bar}] {ratio * 100:6.2f}% "
                       f"{self.count:,}/{self.total:,} | {rate:,.0f}/s "
                       f"ETA {fmt_hhmmss(remaining) if math.isfinite(remaining) else '--:--:--'} "
                       f"ELAPSED {fmt_hhmmss(elapsed)}")
        else:
            spin = Progress.SPINNER[self._spin_idx % len(Progress.SPINNER)]
            self._spin_idx += 1
            rate = self.count / elapsed if elapsed > 0 else 0
            if self.compact:
                msg = f"{self.prefix} {spin} ELAPSED {fmt_hhmmss(elapsed)}"
            else:
                msg = f"{self.prefix} {spin} processed {self.count:,} ({rate:,.0f}/s) ELAPSED {fmt_hhmmss(elapsed)}"
        if self.extra:
            msg += "  " + self.extra
        sys.stdout.write("\r" + msg[:shutil_get_terminal_width()])
        sys.stdout.flush()

    def close(self):
        if self._done:
            return
        self._done = True
        now = time.perf_counter()
        self._draw(now)
        sys.stdout.write("\n")
        sys.stdout.flush()


@contextmanager
def timer(label: str):
    t0 = time.perf_counter()
    print(f"[START] {label}")
    try:
        yield
    finally:
        dt = time.perf_counter() - t0
        print(f"[DONE ] {label} in {fmt_hhmmss(dt)}")


def shutil_get_terminal_width(default=120) -> int:
    try:
        import shutil
        cols = shutil.get_terminal_size((default, 20)).columns
        return max(40, cols)
    except Exception:
        return default


# ----------------------------
# Helpers
# ----------------------------

OWNER_TOKEN_RE = re.compile(r"^\s*([^\s;]+)")
ORIGIN_RE = re.compile(r"^\s*\$ORIGIN\s+([^\s;]+)")
TTL_RE = re.compile(r"^\s*\$TTL\b")
COMMENT_OR_EMPTY_RE = re.compile(r"^\s*(;|$)")
HTTP_URL_RE = re.compile(r"^https?://", re.I)

# Firebog index URLs
FIREBOG_NOCROSS = "https://v.firebog.net/hosts/lists.php?type=nocross"
FIREBOG_ADULT = "https://v.firebog.net/hosts/lists.php?type=adult"


def get_tld_from_filename(p: Path) -> str:
    """
    Heuristic: take the base name and strip .txt, .gz, .txt.gz in that order.
    e.g., 'com.txt.gz' -> 'com'  |  'com.txt' -> 'com'  |  'org.gz' -> 'org'
    """
    name = p.name.lower()
    for suf in (".txt.gz", ".txt", ".gz"):
        if name.endswith(suf):
            name = name[: -len(suf)]
            break
    return name.strip(".")


def normalize_fqdn(s: str) -> str:
    s = s.strip().lower()
    if s.endswith("."):
        s = s[:-1]
    return s


def is_relative(name: str) -> bool:
    return not name.endswith(".")


def fqdn_to_sld(fqdn: str, tld: str) -> Optional[str]:
    """
    Reduce an FQDN ending with <tld> to the immediate-registered domain:
    e.g., 'www.api.example.com' -> 'example.com'
          'foo.bar' (no tld match) -> None
          'com' (apex) -> None
    This assumes the zone is exactly for the given TLD.
    """
    fqdn = normalize_fqdn(fqdn)
    labels = fqdn.split(".")
    tld_labels = tld.split(".")
    if len(labels) < len(tld_labels) + 1:
        return None
    if labels[-len(tld_labels):] != tld_labels:
        return None
    sld_label_index = -len(tld_labels) - 1
    if abs(sld_label_index) > len(labels):
        return None
    sld = ".".join([labels[sld_label_index]] + tld_labels)
    return sld


def open_zone_file(path: Path) -> Iterable[str]:
    """Open .txt, .gz, or .txt.gz as a text stream (utf-8 with replacement)."""
    if path.suffix == ".gz" or path.name.endswith(".txt.gz"):
        with gzip.open(path, "rt", encoding="utf-8", errors="replace") as ftxt:
            for line in ftxt:
                yield line
    else:
        with path.open("r", encoding="utf-8", errors="replace") as f:
            for line in f:
                yield line


@contextmanager
def domain_db(tmp_dir: Optional[Path] = None):
    """
    SQLite-backed 'set' for unique domains at large scale.
    Uses a single table with UNIQUE(domain).
    """
    tmp_dir = tmp_dir or Path(tempfile.gettempdir())
    db_path = tmp_dir / "domains_cache.sqlite3"
    if db_path.exists():
        db_path.unlink()
    conn = sqlite3.connect(str(db_path))
    try:
        cur = conn.cursor()
        cur.executescript("""
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = OFF;
            PRAGMA temp_store = MEMORY;
            CREATE TABLE IF NOT EXISTS domains (
                domain TEXT PRIMARY KEY,
                tld    TEXT
            );
            """)
        conn.commit()
        yield conn
    finally:
        conn.close()
        try:
            db_path.unlink()
        except Exception:
            pass


def batch_insert(conn: sqlite3.Connection, rows: Iterable[Tuple[str, str]]):
    cur = conn.cursor()
    cur.executemany("INSERT OR IGNORE INTO domains(domain, tld) VALUES(?, ?)", rows)


def dump_domains(conn: sqlite3.Connection, out_path: Path) -> int:
    """Write domains to file with a determinate progress bar."""
    cur = conn.cursor()
    (n_total,) = cur.execute("SELECT COUNT(*) FROM domains").fetchone()
    pb = Progress(prefix=f"Writing {out_path.name}", total=n_total, compact=True)
    total = 0
    with out_path.open("w", encoding="utf-8") as out:
        for (d,) in cur.execute("SELECT domain FROM domains ORDER BY domain ASC"):
            out.write(d + "\n")
            total += 1
            if (total & 0x3FF) == 0:  # update every ~1024 rows
                pb.update(1024)
    # fix final increment if not multiple of 1024
    remainder = n_total - (total - (total & 0x3FF))
    if remainder > 0:
        pb.update(remainder)
    pb.close()
    return total


def read_lines(path: Path) -> Iterable[str]:
    with path.open("r", encoding="utf-8", errors="replace") as f:
        for line in f:
            yield line


# ----------------------------
# Bytes-based line iterators (deterministic per-file progress, throttled)
# ----------------------------

def _iter_lines_bytes_progress_txt(path: Path, pb: "Progress", min_bytes_step: int = 8 * 1024 * 1024, min_time_step: float = 0.25):
    # Binary read so .tell() reports real bytes
    with path.open("rb") as fb:
        with io.TextIOWrapper(fb, encoding="utf-8", errors="replace") as ftxt:
            last_bytes = 0
            last_t = time.perf_counter()
            for line in ftxt:
                now = time.perf_counter()
                # sample only when enough time or bytes have passed
                if (now - last_t) >= min_time_step:
                    pos = fb.tell()
                    delta = pos - last_bytes
                    if delta >= min_bytes_step or (now - last_t) >= min_time_step:
                        pb.update(delta)
                        last_bytes = pos
                        last_t = now
                yield line
            # final advance to eof
            pos = fb.tell()
            if pos > last_bytes:
                pb.update(pos - last_bytes)


def _iter_lines_bytes_progress_gz(path: Path, pb: "Progress", min_time_step: float = 0.25, ) -> Iterator[str]:
    # Open in text mode; this returns a TextIOWrapper
    with gzip.open(path, mode="rt", encoding="utf-8", errors="replace") as ftxt:
        # Underlying gzip.GzipFile
        gz = getattr(ftxt, "buffer", None)  # <- GzipFile
        comp = getattr(gz, "fileobj", None) if gz is not None else None  # <- compressed file handle

        last_bytes = 0
        last_t = time.perf_counter()

        for line in ftxt:
            now = time.perf_counter()
            if (now - last_t) >= min_time_step:
                try:
                    pos = comp.tell() if comp is not None else last_bytes
                except OSError:
                    pos = last_bytes
                delta = pos - last_bytes
                if delta > 0:
                    pb.update(delta)
                    last_bytes = pos
                last_t = now
            yield line

        # final advance
        try:
            pos = comp.tell() if comp is not None else last_bytes
        except OSError:
            pos = last_bytes
        if pos > last_bytes:
            pb.update(pos - last_bytes)


def parse_slds_from_lines(lines: Iterable[str], tld: str) -> Iterable[str]:
    """
    Fast-path parser: avoids regex except for $ORIGIN.
    Yields SLDs (immediate child of TLD).
    """
    origin = tld
    for raw in lines:
        s = raw.lstrip()
        if not s or s[0] == ';':  # comment/empty
            continue
        if s.startswith("$TTL"):
            continue
        if s.startswith("$ORIGIN"):
            m_or = ORIGIN_RE.match(s)
            if m_or:
                origin = normalize_fqdn(m_or.group(1))
            continue

        # FAST owner token: up to first whitespace or ';'
        end = len(s)
        semi = s.find(';')
        if semi != -1:
            end = semi  # drop inline comment
        for i, ch in enumerate(s[:end]):
            if ch.isspace():
                end = i
                break
        if end == 0:
            continue
        owner = s[:end]

        fqdn = origin if owner == "@" else (f"{owner}.{origin}" if is_relative(owner) else owner)
        sld = fqdn_to_sld(fqdn, tld)
        if sld:
            yield sld


# ----------------------------
# Blacklist fetching & parsing (stdlib urllib)
# ----------------------------

def fetch_text(url: str):
    """Yield lines from a URL using only stdlib, with byte progress if Content-Length present."""
    req = urllib.request.Request(url, headers={"User-Agent": "zone-extractor/1.0"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        total = None
        try:
            total = int(resp.headers.get("Content-Length") or 0)
        except Exception:
            total = None

        pb = Progress(prefix=f"Downloading: {url}", total=total, compact=True)
        buf = b""
        for chunk in iter(lambda: resp.read(1 << 14), b""):
            if not chunk:
                break
            pb.update(len(chunk))
            buf += chunk
            while True:
                nl = buf.find(b"\n")
                if nl == -1:
                    break
                line = buf[:nl].decode("utf-8", errors="replace").rstrip("\r\n")
                buf = buf[nl + 1:]
                yield line
        if buf:
            yield buf.decode("utf-8", errors="replace").rstrip("\r\n")
        pb.close()


def list_urls_from_firebog(index_url: str) -> set[str]:
    urls = set()
    for line in fetch_text(index_url):
        line = line.strip()
        if line.lower().startswith("http"):
            urls.add(line)
    return urls


def parse_hosts_line(line: str) -> Optional[str]:
    """
    Parse a hosts-entry style line into a domain.
    Supports formats like:
      - "0.0.0.0 example.com"
      - "127.0.0.1 example.com"
      - "example.com"
      - "# comment" (ignored)
    Returns the domain in lowercase or None.
    """
    line = line.strip()
    if not line or line.startswith("#"):
        return None
    if "#" in line:
        line = line.split("#", 1)[0].strip()
    if not line:
        return None
    parts = line.split()
    if len(parts) == 1:
        dom = parts[0]
    else:
        dom = parts[1]  # likely "IP domain"
    dom = normalize_fqdn(dom)
    if re.match(r"^\d{1,3}(\.\d{1,3}){3}$", dom):
        return None
    if ":" in dom:  # IPv6
        return None
    if "." not in dom:
        return None
    return dom


def build_blacklist(sources: Iterable[str]) -> Set[str]:
    """
    Download each source (hosts list) and collect domains into a set.
    Shows a sources counter progress bar.
    """
    src_list = list(sources)
    pb = Progress(prefix="Building blacklist (sources)", total=len(src_list), compact=True)
    bl: Set[str] = set()
    for src in src_list:
        try:
            for line in fetch_text(src):
                dom = parse_hosts_line(line)
                if dom:
                    bl.add(dom)
        except Exception as e:
            print(f"[WARN] Failed to fetch {src}: {e}", file=sys.stderr)
        finally:
            pb.update(1)
    pb.close()
    return bl


def suffix_blacklisted(domain: str, blacklist: Set[str]) -> bool:
    """
    True if 'domain' itself or any of its parent domains are in the blacklist.
    e.g., blacklist has 'example.com' -> blocks 'example.com' and 'www.example.com'.
    """
    # exact match first
    if domain in blacklist:
        return True
    # walk dots right-to-left, no allocations of full split list
    i = domain.find(".")
    while i != -1:
        cand = domain[i + 1:]
        if cand in blacklist:
            return True
        i = domain.find(".", i + 1)
    return False


# ----------------------------
# Zone parsing (legacy generator kept; not used for bytes progress)
# ----------------------------

def extract_domains_from_zone(path: Path, tld: str) -> Iterable[str]:
    """
    Stream a zone file and yield SLDs for the given TLD.
    Handles $ORIGIN for relative owners.
    """
    origin = tld  # default origin is the zone itself
    for raw in open_zone_file(path):
        # super fast skips before regex:
        s = raw.lstrip()
        if not s or s[0] == ';':  # comment/empty
            continue
        if s.startswith("$TTL"):
            continue
        if s.startswith("$ORIGIN"):
            m_or = ORIGIN_RE.match(s)
            if m_or:
                origin = normalize_fqdn(m_or.group(1))
            continue
        m_own = OWNER_TOKEN_RE.match(s)
        if not m_own:
            continue
        owner = m_own.group(1)
        if owner == "@":
            fqdn = origin
        else:
            fqdn = f"{owner}.{origin}" if is_relative(owner) else owner
        sld = fqdn_to_sld(fqdn, tld)
        if sld:
            yield sld


# ----------------------------
# Main modes
# ----------------------------

def mode_extract_and_filter(folder: Path, out_file: Path, metrics_path: Path, include_adult: bool, include_nocross: bool) -> None:
    files = sorted([p for p in folder.iterdir() if p.is_file() and (p.suffix in (".gz", ".txt") or p.name.endswith(".txt.gz"))])
    if not files:
        print(f"No .txt/.gz files found in {folder}", file=sys.stderr)
        sys.exit(1)

    # Build blacklist
    with timer("Blacklist build"):
        index_urls = []
        if include_nocross:
            index_urls.append(FIREBOG_NOCROSS)
        if include_adult:
            index_urls.append(FIREBOG_ADULT)
        sources: Set[str] = set()
        for idx in index_urls:
            urls = list_urls_from_firebog(idx)
            print(f"[INFO] Index {idx} -> {len(urls)} sources")
            sources |= urls
        print(f"[INFO] Fetching {len(sources)} hosts lists for blacklist…")
        blacklist = build_blacklist(sources)
        print(f"[INFO] Blacklist domains loaded: {len(blacklist):,}")

    # Process zones (bytes-based determinate progress per file)
    per_tld_counts = defaultdict(lambda: {"extracted": 0, "kept": 0, "filtered": 0})
    tld_times: dict[str, float] = defaultdict(float)

    with domain_db() as conn:
        batch: list[Tuple[str, str]] = []
        BATCH_SIZE = 250_000

        for fp in files:
            tld = get_tld_from_filename(fp)
            if not tld:
                print(f"[WARN] Could not infer TLD from filename: {fp.name}", file=sys.stderr)
                continue

            t0 = time.perf_counter()
            with timer(f"Processing {fp.name} (TLD={tld})"):
                pb = Progress(prefix=f"Parsing {fp.name}", total=fp.stat().st_size, min_interval=0.25, compact=True)
                extracted = kept = filtered = 0
                rate_last_update = 0.0

                # choose the correct byte-progress iterator
                if fp.suffix == ".gz" or fp.name.endswith(".txt.gz"):
                    line_iter = _iter_lines_bytes_progress_gz(fp, pb)
                else:
                    line_iter = _iter_lines_bytes_progress_txt(fp, pb)

                for sld in parse_slds_from_lines(line_iter, tld):
                    extracted += 1

                    # compute and display domains/sec only on redraw cadence
                    now = time.perf_counter()
                    if now - rate_last_update >= 0.5:
                        elapsed = max(1e-9, now - t0)
                        pb.set_extra(f"{fmt_rate(extracted / elapsed)}/s")
                        pb.update(0)  # just redraw
                        rate_last_update = now

                    per_tld_counts[tld]["extracted"] += 1
                    if suffix_blacklisted(sld, blacklist):
                        filtered += 1
                        per_tld_counts[tld]["filtered"] += 1
                        continue
                    kept += 1
                    per_tld_counts[tld]["kept"] += 1
                    batch.append((sld, tld))
                    if len(batch) >= BATCH_SIZE:
                        batch_insert(conn, batch)
                        conn.commit()
                        batch.clear()

                # final redraw with rate
                elapsed = max(1e-9, time.perf_counter() - t0)
                pb.set_extra(f"{fmt_rate(extracted / elapsed)}/s")
                pb.update(0)
                pb.close()

            tld_times[tld] += (time.perf_counter() - t0)

        if batch:
            batch_insert(conn, batch)
            conn.commit()

        with timer(f"Writing output {out_file.name}"):
            total_written = dump_domains(conn, out_file)

    # Metrics (add times and throughput)
    totals = {"extracted": 0, "kept": 0, "filtered": 0}
    for tld, d in per_tld_counts.items():
        for k in totals:
            totals[k] += d[k]

    # enrich per_tld with time + rates
    times_per_tld = {}
    for tld, d in per_tld_counts.items():
        tsec = tld_times.get(tld, 0.0)
        times_per_tld[tld] = tsec
        d["time_seconds"] = round(tsec, 3)
        d["extracted_per_second"] = round((d["extracted"] / tsec) if tsec > 0 else 0.0, 3)
        d["kept_per_second"] = round((d["kept"] / tsec) if tsec > 0 else 0.0, 3)

    total_seconds = round(sum(times_per_tld.values()), 3)
    totals["time_seconds"] = total_seconds
    totals["extracted_per_second"] = round((totals["extracted"] / total_seconds) if total_seconds > 0 else 0.0, 3)
    totals["kept_per_second"] = round((totals["kept"] / total_seconds) if total_seconds > 0 else 0.0, 3)

    metrics = {"per_tld": per_tld_counts, "totals": totals, "times": {"per_tld_seconds": times_per_tld, "total_seconds": total_seconds},
               "output_file": str(out_file)}
    metrics_path.write_text(json.dumps(metrics, indent=2), encoding="utf-8")

    print("\n=== Metrics ===")
    for tld in sorted(per_tld_counts):
        d = per_tld_counts[tld]
        print(f"{tld}: extracted={d['extracted']:,} kept={d['kept']:,} filtered={d['filtered']:,} "
              f"time={d['time_seconds']}s kept/s={d['kept_per_second']:.1f}")
    print(f"TOTAL: extracted={totals['extracted']:,} kept={totals['kept']:,} filtered={totals['filtered']:,} "
          f"time={totals['time_seconds']}s kept/s={totals['kept_per_second']:.1f}")
    print(f"\nWrote {total_written:,} unique domains to {out_file}")
    print(f"Metrics JSON: {metrics_path}")


def mode_extract_only(folder: Path, out_file: Path, metrics_path: Path) -> None:
    files = sorted([p for p in folder.iterdir() if p.is_file() and (p.suffix in (".gz", ".txt") or p.name.endswith(".txt.gz"))])
    if not files:
        print(f"No .txt/.gz files found in {folder}", file=sys.stderr)
        sys.exit(1)

    per_tld_counts = defaultdict(lambda: {"extracted": 0, "kept": 0, "filtered": 0})
    tld_times: dict[str, float] = defaultdict(float)

    with domain_db() as conn:
        batch: list[Tuple[str, str]] = []
        BATCH_SIZE = 250_000

        for fp in files:
            tld = get_tld_from_filename(fp)
            if not tld:
                print(f"[WARN] Could not infer TLD from filename: {fp.name}", file=sys.stderr)
                continue

            t0 = time.perf_counter()
            with timer(f"Processing {fp.name} (TLD={tld})"):
                pb = Progress(prefix=f"Parsing {fp.name}", total=fp.stat().st_size, min_interval=0.25, compact=True)
                extracted = kept = 0
                rate_last_update = 0.0

                if fp.suffix == ".gz" or fp.name.endswith(".txt.gz"):
                    line_iter = _iter_lines_bytes_progress_gz(fp, pb)
                else:
                    line_iter = _iter_lines_bytes_progress_txt(fp, pb)

                for sld in parse_slds_from_lines(line_iter, tld):
                    extracted += 1
                    now = time.perf_counter()
                    if now - rate_last_update >= 0.5:
                        elapsed = max(1e-9, now - t0)
                        pb.set_extra(f"{fmt_rate(extracted / elapsed)}/s")
                        pb.update(0)
                        rate_last_update = now

                    per_tld_counts[tld]["extracted"] += 1
                    per_tld_counts[tld]["kept"] += 1
                    kept += 1
                    batch.append((sld, tld))
                    if len(batch) >= BATCH_SIZE:
                        batch_insert(conn, batch)
                        conn.commit()
                        batch.clear()

                elapsed = max(1e-9, time.perf_counter() - t0)
                pb.set_extra(f"{fmt_rate(extracted / elapsed)}/s")
                pb.update(0)
                pb.close()

            tld_times[tld] += (time.perf_counter() - t0)

        if batch:
            batch_insert(conn, batch)
            conn.commit()

        with timer(f"Writing output {out_file.name}"):
            total_written = dump_domains(conn, out_file)

    # Metrics (+times and throughput)
    totals = {"extracted": 0, "kept": 0, "filtered": 0}
    for tld, d in per_tld_counts.items():
        for k in totals:
            totals[k] += d[k]

    times_per_tld = {}
    for tld, d in per_tld_counts.items():
        tsec = tld_times.get(tld, 0.0)
        times_per_tld[tld] = tsec
        d["time_seconds"] = round(tsec, 3)
        d["extracted_per_second"] = round((d["extracted"] / tsec) if tsec > 0 else 0.0, 3)
        d["kept_per_second"] = round((d["kept"] / tsec) if tsec > 0 else 0.0, 3)

    total_seconds = round(sum(times_per_tld.values()), 3)
    totals["time_seconds"] = total_seconds
    totals["extracted_per_second"] = round((totals["extracted"] / total_seconds) if total_seconds > 0 else 0.0, 3)
    totals["kept_per_second"] = round((totals["kept"] / total_seconds) if total_seconds > 0 else 0.0, 3)

    metrics = {"per_tld": per_tld_counts, "totals": totals, "times": {"per_tld_seconds": times_per_tld, "total_seconds": total_seconds},
               "output_file": str(out_file)}
    metrics_path.write_text(json.dumps(metrics, indent=2), encoding="utf-8")

    print("\n=== Metrics ===")
    for tld in sorted(per_tld_counts):
        d = per_tld_counts[tld]
        print(f"{tld}: extracted={d['extracted']:,} kept={d['kept']:,} time={d['time_seconds']}s kept/s={d['kept_per_second']:.1f}")
    print(f"TOTAL: extracted={totals['extracted']:,} kept={totals['kept']:,} time={totals['time_seconds']}s kept/s={totals['kept_per_second']:.1f}")
    print(f"\nWrote {total_written:,} unique domains to {out_file}")
    print(f"Metrics JSON: {metrics_path}")


def mode_filter_only(folder: Path, in_file: Path, out_file: Path, metrics_path: Path, include_adult: bool, include_nocross: bool) -> None:
    if not in_file.exists():
        print(f"Input domains file not found: {in_file}", file=sys.stderr)
        sys.exit(1)

    # Build blacklist
    with timer("Blacklist build"):
        index_urls = []
        if include_nocross:
            index_urls.append(FIREBOG_NOCROSS)
        if include_adult:
            index_urls.append(FIREBOG_ADULT)
        sources: Set[str] = set()
        for idx in index_urls:
            urls = list_urls_from_firebog(idx)
            print(f"[INFO] Index {idx} -> {len(urls)} sources")
            sources |= urls
        print(f"[INFO] Fetching {len(sources)} hosts lists for blacklist…")
        blacklist = build_blacklist(sources)
        print(f"[INFO] Blacklist domains loaded: {len(blacklist):,}")

    per_tld_counts = defaultdict(lambda: {"extracted": 0, "kept": 0, "filtered": 0})
    t0_total = time.perf_counter()

    def infer_tld(domain: str) -> str:
        parts = domain.split(".")
        return parts[-1] if len(parts) >= 2 else ""

    with domain_db() as conn:
        batch: list[Tuple[str, str]] = []
        BATCH_SIZE = 250_000

        with timer(f"Filtering {in_file.name}"):
            pb = Progress(prefix=f"Filtering {in_file.name}", total=in_file.stat().st_size, min_interval=0.25, compact=True)
            processed = 0
            rate_last = 0.0
            t0 = time.perf_counter()

            # bytes-progress for plain text input, throttled
            for line in _iter_lines_bytes_progress_txt(in_file, pb):
                domain = normalize_fqdn(line)
                if not domain or "." not in domain:
                    continue
                processed += 1

                now = time.perf_counter()
                if now - rate_last >= 0.5:
                    elapsed = max(1e-9, now - t0)
                    pb.set_extra(f"{fmt_rate(processed / elapsed)}/s")
                    pb.update(0)
                    rate_last = now

                tld = infer_tld(domain)
                per_tld_counts[tld]["extracted"] += 1
                if suffix_blacklisted(domain, blacklist):
                    per_tld_counts[tld]["filtered"] += 1
                    continue
                per_tld_counts[tld]["kept"] += 1
                batch.append((domain, tld))
                if len(batch) >= BATCH_SIZE:
                    batch_insert(conn, batch)
                    conn.commit()
                    batch.clear()

            elapsed = max(1e-9, time.perf_counter() - t0)
            pb.set_extra(f"{fmt_rate(processed / elapsed)}/s")
            pb.update(0)
            pb.close()

        if batch:
            batch_insert(conn, batch)
            conn.commit()

        with timer(f"Writing output {out_file.name}"):
            total_written = dump_domains(conn, out_file)

    # Metrics (+throughput for totals; per-TLD time not measured here individually)
    totals = {"extracted": 0, "kept": 0, "filtered": 0}
    for tld, d in per_tld_counts.items():
        for k in totals:
            totals[k] += d[k]

    total_seconds = round(time.perf_counter() - t0_total, 3)
    totals["time_seconds"] = total_seconds
    totals["extracted_per_second"] = round((totals["extracted"] / total_seconds) if total_seconds > 0 else 0.0, 3)
    totals["kept_per_second"] = round((totals["kept"] / total_seconds) if total_seconds > 0 else 0.0, 3)

    metrics = {"per_tld": per_tld_counts, "totals": totals, "times": {"per_tld_seconds": {},  # not tracked in filter-only mode
                                                                      "total_seconds": total_seconds}, "output_file": str(out_file)}
    metrics_path.write_text(json.dumps(metrics, indent=2), encoding="utf-8")

    print("\n=== Metrics ===")
    for tld in sorted(per_tld_counts):
        d = per_tld_counts[tld]
        print(f"{tld}: input={d['extracted']:,} kept={d['kept']:,} filtered={d['filtered']:,}")
    print(f"TOTAL: input={totals['extracted']:,} kept={totals['kept']:,} filtered={totals['filtered']:,} "
          f"time={totals['time_seconds']}s kept/s={totals['kept_per_second']:.1f}")
    print(f"\nWrote {total_written:,} unique domains to {out_file}")
    print(f"Metrics JSON: {metrics_path}")


# ----------------------------
# CLI
# ----------------------------

def normalize_mode(m: str) -> str:
    """
    Accepts: '', 'default', '1' -> extract+filter
             'extract', '2'     -> extract only
             'filter', '3'      -> filter only
    Returns one of: 'extract+filter' | 'extract' | 'filter'
    """
    m = (m or "").strip().lower()
    if m in ("", "default", "1", "extract+filter"):
        return "extract+filter"
    if m in ("2", "extract"):
        return "extract"
    if m in ("3", "filter"):
        return "filter"
    raise SystemExit(f"Unknown --mode '{m}'. Use '', 'extract', 'filter' or 1/2/3.")


def parse_args():
    p = argparse.ArgumentParser(
        description="Extract SLDs from TLD zone files (.txt / .gz), optionally apply Firebog blacklists, and write to domains.txt with metrics.")
    p.add_argument("folder", type=Path, help="Folder containing .txt/.gz zone files OR domains.txt (for filter mode).")
    p.add_argument("--mode", default="",
                   help="Modes: '' or 1 = extract+filter (default), 'extract' or 2 = extract only, 'filter' or 3 = filter only.")
    p.add_argument("--output", type=Path, default=None, help="Output file path. Defaults depend on mode.")
    p.add_argument("--metrics", type=Path, default=None, help="Metrics JSON path (default: metrics.json in folder).")
    p.add_argument("--include-adult", action="store_true", default=True, help="Include Firebog 'adult' lists.")
    p.add_argument("--no-include-adult", dest="include_adult", action="store_false", help="Exclude Firebog 'adult' lists.")
    p.add_argument("--include-nocross", action="store_true", default=True, help="Include Firebog 'nocross' lists.")
    p.add_argument("--no-include-nocross", dest="include_nocross", action="store_false", help="Exclude Firebog 'nocross' lists.")
    return p.parse_args()


def main():
    args = parse_args()
    folder: Path = args.folder
    if not folder.exists() or not folder.is_dir():
        print(f"Folder not found or not a directory: {folder}", file=sys.stderr)
        sys.exit(1)

    mode = normalize_mode(args.mode)
    metrics_path = args.metrics or (folder / "metrics.json")

    with timer("Total run"):
        if mode == "extract+filter":
            out_file = args.output or (folder / "domains.txt")
            mode_extract_and_filter(folder, out_file, metrics_path, include_adult=args.include_adult, include_nocross=args.include_nocross)
        elif mode == "extract":
            out_file = args.output or (folder / "domains.txt")
            mode_extract_only(folder, out_file, metrics_path)
        else:
            in_file = folder / "domains.txt"
            out_file = args.output or (folder / "domains_filtered.txt")
            mode_filter_only(folder, in_file, out_file, metrics_path, include_adult=args.include_adult, include_nocross=args.include_nocross)


if __name__ == "__main__":
    main()
