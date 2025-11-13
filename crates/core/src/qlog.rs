use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::rotate::{NewFileHook, RotatingWriter};

const BASE_NAME: &str = "quic-lab.sqlog";
const MAX_SQLOG_BYTES: u64 = 256 * 1024 * 1024;
const RS: u8 = 0x1E;
const LF: u8 = b'\n';
const FLUSH_EVERY: u32 = 2000; // flush every N records

/// When true, keep only fields/events that qvis + some custom stats.
/// When false, write the full events as received.
pub const MINIMIZE_QLOG: bool = true;

#[derive(Clone)]
struct QlogHeaderHook {
    title: String,
    description: String,
    vp_name: String,
    vp_type: String,
    reference_time_ms: f64,
}

impl QlogHeaderHook {
    fn with_epoch(epoch: SystemTime) -> Self {
        let ms = epoch.duration_since(UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0;
        Self {
            title: "quic-lab session".into(),
            description: "Aggregated multi-connection log".into(),
            vp_name: "quic-lab".into(),
            vp_type: "client".into(),
            reference_time_ms: ms,
        }
    }
}

impl NewFileHook for QlogHeaderHook {
    fn on_new_file(
        &mut self,
        _path: &std::path::Path,
        file: &mut std::fs::File,
    ) -> std::io::Result<()> {
        // Single JSON-SEQ header at the start of each .sqlog
        let header = json!({
          "qlog_version": "0.4",
          "qlog_format":  "JSON-SEQ",
          "title": self.title,
          "description": self.description,
          "trace": {
            "common_fields": {
              "time_format": "relative",
              "reference_time": self.reference_time_ms
            },
            "vantage_point": { "name": self.vp_name, "type": self.vp_type }
          }
        });
        file.write_all(&[RS])?;
        serde_json::to_writer(&mut *file, &header)?;
        file.write_all(&[LF])?;
        file.flush()?;
        Ok(())
    }
}

#[inline]
fn ms_since(then: SystemTime) -> f64 {
    let now = SystemTime::now();
    match now.duration_since(then) {
        Ok(d) => d.as_secs_f64() * 1000.0,
        Err(e) => -(e.duration().as_secs_f64() * 1000.0),
    }
}

struct Inner {
    bufw: BufWriter<RotatingWriter<QlogHeaderHook>>,
    epoch: SystemTime,
    since_flush: u32,
    // last emitted time per group_id to keep traces strictly monotonic
    last_t: HashMap<String, f64>,
}

pub struct QlogMux {
    inner: Mutex<Inner>,
}

static GLOBAL: OnceLock<QlogMux> = OnceLock::new();

impl QlogMux {
    fn new(out_dir: &str) -> std::io::Result<Self> {
        let dir = PathBuf::from(out_dir).join("qlog_files");
        std::fs::create_dir_all(&dir)?;
        let epoch = SystemTime::now();
        let hook = QlogHeaderHook::with_epoch(epoch);
        let writer = RotatingWriter::new(&dir, BASE_NAME, MAX_SQLOG_BYTES, Some(hook))?;
        Ok(Self {
            inner: Mutex::new(Inner {
                bufw: BufWriter::with_capacity(256 * 1024, writer),
                epoch,
                since_flush: 0,
                last_t: HashMap::new(),
            }),
        })
    }

    fn append_record(&self, record: &[u8]) -> std::io::Result<()> {
        // Drop any per-connection JSON-SEQ headers; keep only events
        if is_header_frame(record) {
            return Ok(());
        }
        let mut g = self.inner.lock().unwrap();
        g.bufw.write_all(record)?;
        g.since_flush += 1;
        if g.since_flush >= FLUSH_EVERY {
            g.bufw.flush()?;
            g.since_flush = 0;
        }
        Ok(())
    }

    pub fn append_event<D: Serialize>(
        &self,
        group_id: &str,
        name: &str,
        data: &D,
    ) -> std::io::Result<()> {
        let mut g = self.inner.lock().unwrap();

        // make time strictly monotonic per group_id
        let mut t_ms = ms_since(g.epoch);
        if let Some(prev) = g.last_t.get(group_id) {
            if t_ms <= *prev {
                t_ms = prev + 1e-6;
            }
        }
        g.last_t.insert(group_id.to_string(), t_ms);

        let ev = json!({ "time": t_ms, "name": name, "group_id": group_id, "data": data });
        g.bufw.write_all(&[RS])?;
        serde_json::to_writer(&mut g.bufw, &ev)?;
        g.bufw.write_all(&[LF])?;
        g.since_flush += 1;
        if g.since_flush >= FLUSH_EVERY {
            g.bufw.flush()?;
            g.since_flush = 0;
        }
        Ok(())
    }

    pub fn info(&self, group_id: &str, message: &str) {
        let _ = self.append_event(group_id, "loglevel:info", &json!({ "message": message }));
    }
    pub fn error(&self, group_id: &str, message: &str) {
        let _ = self.append_event(group_id, "loglevel:error", &json!({ "message": message }));
    }
}

#[inline]
pub fn qlog() -> Option<&'static QlogMux> {
    GLOBAL.get()
}

#[inline]
pub fn is_enabled() -> bool {
    GLOBAL.get().is_some()
}

pub fn init(out_dir: &str, enabled: bool) -> Result<()> {
    if !enabled {
        return Ok(());
    }
    let _ = GLOBAL.set(QlogMux::new(out_dir)?);
    Ok(())
}

// Detects JSON-SEQ header frames: look for known header keys between RS…LF.
fn is_header_frame(frame: &[u8]) -> bool {
    if frame.first() != Some(&RS) {
        return false;
    }
    let max = frame.len().min(64 * 1024);
    let s = &frame[..max];
    memmem(s, br#""qlog_format""#) || memmem(s, br#""file_schema""#)
}

fn memmem(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

// --------------------
// Minimalizer helpers
// --------------------

#[inline]
fn vobj(v: &mut Value) -> Option<&mut Map<String, Value>> {
    v.as_object_mut()
}

/// Reduce event payload to what qvis + custom stats.
/// Returns `false` to drop the event entirely.
fn qvis_minimize_in_place(ev: &mut Value) -> bool {
    if !MINIMIZE_QLOG {
        return true;
    }

    // Own the name so immutable borrow ends before we mutate `ev`.
    let name: String = ev
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();

    // Always keep meta:* (e.g., meta:connection for labels) and loglevel:*
    if name.starts_with("meta:") || name.starts_with("loglevel:") {
        // Still prune heavy subfields if any
        if let Some(ev_obj) = vobj(ev) {
            if let Some(data) = ev_obj.get_mut("data").and_then(|d| d.as_object_mut()) {
                data.remove("raw");
            }
        }
        return true;
    }

    // Keep transport parameters (both owners; alternatively filter owner=="remote")
    if name.ends_with(":parameters_set") {
        // typically small; leave as is
        return true;
    }

    // Errors / closes / path validation / connection loss: keep
    let looks_errory = name.contains("error")
        || name.contains("closed")
        || name.starts_with("quic:path_")
        || name.contains("connection_lost");
    if looks_errory {
        if let Some(ev_obj) = vobj(ev) {
            if let Some(data) = ev_obj.get_mut("data").and_then(|d| d.as_object_mut()) {
                data.remove("raw");
            }
        }
        return true;
    }

    // Keep only recovery:packet_lost from the recovery namespace.
    if name.starts_with("recovery:") {
        return name == "recovery:packet_lost";
    }

    // Drop very noisy events not needed by qvis & custom stats.
    if name == "quic:stream_data_moved" {
        return false;
    }

    // Packet events: keep header/frames minimal + raw.{length,payload_length}.
    if name == "quic:packet_sent" || name == "quic:packet_received" {
        if let Some(ev_obj) = vobj(ev) {
            if let Some(data) = ev_obj.get_mut("data").and_then(|d| d.as_object_mut()) {
                // header: keep packet_type, packet_number, scil, dcil
                if let Some(h) = data.get_mut("header") {
                    if let Some(hobj) = h.as_object_mut() {
                        let pkt_type = hobj.get("packet_type").cloned();
                        let pkt_num = hobj.get("packet_number").cloned();
                        let scil = hobj.get("scil").cloned();
                        let dcil = hobj.get("dcil").cloned();
                        hobj.clear();
                        if let Some(v) = pkt_type {
                            hobj.insert("packet_type".into(), v);
                        }
                        if let Some(v) = pkt_num {
                            hobj.insert("packet_number".into(), v);
                        }
                        if let Some(v) = scil {
                            hobj.insert("scil".into(), v);
                        }
                        if let Some(v) = dcil {
                            hobj.insert("dcil".into(), v);
                        }
                    }
                }

                // raw: keep only length & payload_length
                if let Some(raw) = data.get_mut("raw") {
                    if let Some(robj) = raw.as_object_mut() {
                        let len = robj.get("length").cloned();
                        let plen = robj.get("payload_length").cloned();
                        robj.clear();
                        if let Some(v) = len {
                            robj.insert("length".into(), v);
                        }
                        if let Some(v) = plen {
                            robj.insert("payload_length".into(), v);
                        }
                    }
                }

                // frames: keep only frame_type (and optionally stream_id if present)
                if let Some(frames) = data.get_mut("frames").and_then(|f| f.as_array_mut()) {
                    for f in frames.iter_mut() {
                        if let Some(fo) = f.as_object_mut() {
                            let ft = fo.get("frame_type").cloned();
                            let sid = fo.get("stream_id").cloned(); // cheap; ok to keep if present
                            fo.clear();
                            if let Some(v) = ft {
                                fo.insert("frame_type".into(), v);
                            }
                            if let Some(v) = sid {
                                fo.insert("stream_id".into(), v);
                            }
                        }
                    }
                }
            }
        }
        return true;
    }

    // Everything else: default pruning (drop any nested "raw" and heavy per-frame blobs).
    if let Some(ev_obj) = vobj(ev) {
        if let Some(data) = ev_obj.get_mut("data").and_then(|d| d.as_object_mut()) {
            data.remove("raw");
            if let Some(frames) = data.get_mut("frames").and_then(|f| f.as_array_mut()) {
                for f in frames.iter_mut() {
                    if let Some(fo) = f.as_object_mut() {
                        fo.remove("raw");
                        fo.remove("payload_length");
                        fo.remove("length_in_bytes");
                        // Keep frame_type by default if already present; otherwise leave as-is.
                        let ft = fo.get("frame_type").cloned();
                        let sid = fo.get("stream_id").cloned();
                        if ft.is_some() || sid.is_some() {
                            fo.clear();
                            if let Some(v) = ft {
                                fo.insert("frame_type".into(), v);
                            }
                            if let Some(v) = sid {
                                fo.insert("stream_id".into(), v);
                            }
                        }
                    }
                }
            }
        }
    }

    true
}

/// Per-connection writer: splits RS…LF and forwards to the mux.
/// Adds a fixed `group_id` if missing and keeps times monotonic per connection.
/// Optionally strips payload to a minimal subset when `QVIS_MINIMAL` is true.
pub struct PerConnSqlog {
    buf: Vec<u8>,
    gid: String,
    last_t: Option<f64>,
}

impl PerConnSqlog {
    pub fn new(group_id: &str) -> Option<Self> {
        if is_enabled() {
            Some(Self {
                buf: Vec::with_capacity(8 * 1024),
                gid: group_id.to_string(),
                last_t: None,
            })
        } else {
            None
        }
    }

    // Forward one complete RS … JSON … LF frame, injecting group_id and fixing time if needed.
    fn forward_frame(&mut self, rec: Vec<u8>) {
        if let Some(mux) = qlog() {
            if rec.len() >= 3 && rec[0] == RS && rec[rec.len() - 1] == LF {
                let payload = &rec[1..rec.len() - 1];
                if let Ok(mut v) = serde_json::from_slice::<Value>(payload) {
                    // ensure group_id
                    if v.get("group_id").is_none() {
                        if let Some(obj) = v.as_object_mut() {
                            obj.insert("group_id".to_string(), Value::String(self.gid.clone()));
                        }
                    }
                    // enforce monotonic time per connection
                    if let Some(t) = v.get("time").and_then(|x| x.as_f64()) {
                        let t_adj = match self.last_t {
                            Some(prev) if t <= prev => prev + 1e-6,
                            _ => t,
                        };
                        if let Some(obj) = v.as_object_mut() {
                            if (t_adj - t).abs() > f64::EPSILON {
                                obj.insert("time".into(), Value::from(t_adj));
                            }
                        }
                        self.last_t = Some(t_adj);
                    }

                    // Optionally reduce to what qvis/custom stats need.
                    if !qvis_minimize_in_place(&mut v) {
                        return; // drop this event entirely
                    }

                    let mut out = Vec::with_capacity(payload.len().min(4096) + 256);
                    out.push(RS);
                    let _ = serde_json::to_writer(&mut out, &v);
                    out.push(LF);
                    let _ = mux.append_record(&out);
                    return;
                }
            }
            let _ = mux.append_record(&rec); // fallback (unparsed or malformed)
        }
    }
}

impl Write for PerConnSqlog {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        loop {
            // Ensure first byte is RS; drop any noise before it
            if let Some(start) = self.buf.iter().position(|&b| b == RS) {
                if start > 0 {
                    self.buf.drain(..start);
                }
            } else {
                break;
            }

            // If we have LF after RS, emit one frame
            if let Some(end_rel) = self.buf[1..].iter().position(|&b| b == LF) {
                let end = 1 + end_rel; // inclusive
                let rec: Vec<u8> = self.buf.drain(..=end).collect();
                self.forward_frame(rec);
                continue;
            } else {
                break;
            }
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Emit only complete RS…LF frames
        loop {
            let start = match self.buf.iter().position(|&b| b == RS) {
                Some(s) => s,
                None => break,
            };
            let rel_end = match self.buf[start + 1..].iter().position(|&b| b == LF) {
                Some(e) => e,
                None => {
                    // No full frame; drop leading noise and stop
                    self.buf.drain(..start);
                    break;
                }
            };
            let end = start + 1 + rel_end; // inclusive LF
            let rec: Vec<u8> = self.buf.drain(start..=end).collect();
            self.forward_frame(rec);
        }
        // Drop any leftovers that are not a full frame
        self.buf.clear();
        Ok(())
    }
}

impl Drop for PerConnSqlog {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
