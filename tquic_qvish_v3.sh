#!/usr/bin/env bash
# tquic_qvis_labels.sh
# qlog 0.4 JSON-SEQ -> qlog 0.3 JSON for qvis. Multi-trace with unique titles.
#
# Usage:
#   ./tquic_qvis_labels.sh <qlog_sqlog_file> [minimal]
#     <qlog_sqlog_file>  : path to *.sqlog[.N] file
#     [minimal]          : true|false|1|0|yes|no|on|off (default: true)
#
# When minimal=true, the script drops fields/events not used by qvis and keeps the
# same subset as the Rust-side minimalizer:
#   - drops "quic:stream_data_moved"
#   - keeps errors, closes, path_* and *:parameters_set
#   - keeps only recovery:packet_lost from recovery:*
#   - for quic:packet_{sent,received}: keep minimal header/raw/frames
#   - removes any .data.raw and frame-level raw/payload_length/length_in_bytes elsewhere

set -euo pipefail
command -v jq >/dev/null || { echo "Install jq: https://jqlang.github.io/jq/download/"; exit 1; }

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "Usage: $0 <qlog_sqlog_file> [minimal:true|false]"; exit 1
fi

IN="$1"; [ -f "$IN" ] || { echo "Error: File '$IN' not found."; exit 1; }

# 2nd arg: minimal toggle (default: true)
MINIMAL_IN="${2:-true}"
case "${MINIMAL_IN,,}" in
  true|1|yes|on)  MINIMAL=true ;;
  false|0|no|off) MINIMAL=false ;;
  *) echo "Error: minimal must be true|false (got: '$MINIMAL_IN')" ; exit 1 ;;
esac

# Output path: drop any trailing ".sqlog*" so qvis doesn't mis-detect JSON-SEQ
BASE="$(basename "$IN")"
OUT="$(dirname "$IN")/${BASE%.sqlog*}.qvis.json"

# Convert JSON-SEQ (records separated by RS) to a single JSON with qvis-friendly schema.
# Also (optionally) prune noisy fields/events.
sed 's/^\x1e//' "$IN" \
| jq -s --argjson minimal "$MINIMAL" '
  def strip_seq: if type=="string" then sub("\\s*\\(JSON-SEQ\\)";"") else . end;

  # Safe string helpers (never throw on non-strings)
  def as_str: if type=="string" then . else "" end;
  def has_prefix($p): (as_str | startswith($p));
  def has_suffix($s): (as_str | endswith($s));
  def has_sub($s):    (as_str | contains($s));

  # Per-event minimization when $minimal==true:
  # - drop quic:stream_data_moved completely
  # - keep errors/closed/path_* and *:parameters_set
  # - keep only recovery:packet_lost from recovery namespace
  # - for packet_* keep compact header/raw/frames
  # - remove generic .data.raw and frame-level raw/payload fields elsewhere
  def prune_event:
    if $minimal != true then . else
      . as $e
      | ($e.name | as_str) as $n
      | if ($n == "quic:stream_data_moved") then
          empty
        elif ($n | has_prefix("meta:") or $n | has_prefix("loglevel:")) then
          if has("data") and (.data|type)=="object" then .data |= del(.raw) else . end
        elif ($n | has_suffix(":parameters_set")) then
          .
        elif ($n | has_prefix("recovery:")) then
          if $n == "recovery:packet_lost" then . else empty end
        elif ( ($n | has_sub("error"))
            or ($n | has_sub("closed"))
            or ($n | has_prefix("quic:path_"))
            or ($n | has_sub("connection_lost")) ) then
          if has("data") and (.data|type)=="object" then .data |= del(.raw) else . end
        elif ($n=="quic:packet_sent" or $n=="quic:packet_received") then
          if has("data") and (.data|type)=="object" then
            .data |= (
              if has("header") and (.header|type)=="object"
              then .header |= ({packet_type,packet_number,scil,dcil})
              else . end
            )
            | .data |= (
                if has("raw") and (.raw|type)=="object"
                then .raw |= ({length, payload_length})
                else . end
              )
            | .data |= (
                if has("frames") and (.frames|type)=="array"
                then .frames |= map(
                      if type=="object"
                      then {frame_type, stream_id}
                      else .
                      end
                    )
                else . end
              )
          else . end
        else
          if has("data") and (.data|type)=="object" then
            .data |= del(.raw)
            | ( if has("frames") and (.frames|type)=="array"
                then .frames |= map( del(.raw, .payload_length, .length_in_bytes) )
                else . end )
          else . end
        end
    end;

  . as $docs
  | $docs[0] as $h
  | ($docs[1:]
      | map(select(type=="object"))
      | sort_by(.group_id // "unknown")
      | group_by(.group_id // "unknown")
    ) as $groups
  | {
      qlog_version: "0.3",
      qlog_format: "JSON",
      title: ($h.title | strip_seq),
      description: ($h.description | strip_seq),
      traces: (
        $groups
        | map(
            . as $grp
            | ($grp[0].group_id // "unknown") as $gid
            | ( $grp
                | map(select(.name=="meta:connection") | .data)
                | first
              ) as $m
            | {
                title: (
                  if $m then
                    ($m.host // "unknown") + " -> " + ($m.peer // "unknown")
                    + ( if ($m|has("alpn") and ($m.alpn|type)=="string" and ($m.alpn != "<none>"))
                        then " [" + $m.alpn + "]" else "" end )
                  else
                    "gid=" + ($gid|tostring)
                  end
                ),
                description: (($h.trace.description // $h.description) | strip_seq),
                vantage_point: $h.trace.vantage_point,
                common_fields: (
                  ($h.trace.common_fields // {})
                  | .group_id = $gid
                ),
                events: (
                  $grp
                  | map(
                      del(.group_id)
                      | prune_event
                      | (if (.name|type)=="string" then (.name |= sub("^quic:";"transport:")) else . end)
                    )
                  | sort_by(.time)
                )
              }
          )
      )
    }
' > "$OUT"

# quick sanity check
jq -e '.qlog_version=="0.3" and .qlog_format=="JSON" and (.traces|type)=="array"' "$OUT" >/dev/null
echo "Wrote $OUT (minimal=$MINIMAL)"
