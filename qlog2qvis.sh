#!/usr/bin/env bash
# qlog2qvis.sh
# qlog 0.4 JSON-SEQ -> qlog 0.3 JSON for qvis. Multi-trace with unique titles.
#
# Usage:
#   ./qlog2qvis.sh <qlog_sqlog_file> [minimal]
#     <qlog_sqlog_file>  : path to *.sqlog[.N] file
#     [minimal]          : true|false|1|0|yes|no|on|off (default: true)
#
# When minimal=true, events/fields are pruned exactly like the Rust-side minimalizer:
#   - drops "quic:stream_data_moved"
#   - keeps only "recovery:packet_lost" from the recovery namespace
#   - keeps meta:* and loglevel:* (but removes data.raw)
#   - keeps *:parameters_set unchanged
#   - keeps errors/closed/quic:path_*/connection_lost (but removes data.raw)
#   - for quic:packet_{sent,received}:
#       header: only {packet_type, packet_number, scil, dcil}
#       raw:    only {length, payload_length}
#       frames: only {frame_type, stream_id}
#   - for all other events:
#       removes data.raw
#       in data.frames[]:
#         removes raw/payload_length/length_in_bytes
#         if frame_type or stream_id present:
#           reduces frame object to {frame_type, stream_id}

set -euo pipefail
command -v jq >/dev/null || { echo "Install jq: https://jqlang.github.io/jq/download/"; exit 1; }

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "Usage: $0 <qlog_sqlog_file> [minimal:true|false]"; exit 1
fi

IN="$1"; [ -f "$IN" ] || { echo "Error: File '$IN' not found."; exit 1; }

# 2nd argument: minimal toggle (default: true)
MINIMAL_IN="${2:-true}"
case "${MINIMAL_IN,,}" in
  true|1|yes|on)  MINIMAL=true ;;
  false|0|no|off) MINIMAL=false ;;
  *) echo "Error: minimal must be true|false (got: '$MINIMAL_IN')" ; exit 1 ;;
esac

# Output path: strip trailing ".sqlog*" so qvis does not mis-detect JSON-SEQ
BASE="$(basename "$IN")"
OUT="$(dirname "$IN")/${BASE%.sqlog*}.qvis.json"

# Convert JSON-SEQ (records separated by RS) into a single qlog-0.3 JSON document for qvis.
sed 's/^\x1e//' "$IN" \
| jq -s --argjson minimal "$MINIMAL" '
  def strip_seq: if type=="string" then sub("\\s*\\(JSON-SEQ\\)";"") else . end;

  # Safe string helpers
  def as_str: if type=="string" then . else "" end;
  def has_prefix($p): (as_str | startswith($p));
  def has_suffix($s): (as_str | endswith($s));
  def has_sub($s):    (as_str | contains($s));

  # Event minimalizer that mirrors the Rust implementation
  def prune_event:
    if $minimal != true then . else
      . as $e
      | ($e.name | as_str) as $n
      | if ($n | has_prefix("meta:") or $n | has_prefix("loglevel:")) then
          # meta:* / loglevel:* -> keep, but remove data.raw
          if has("data") and (.data|type)=="object" then .data |= del(.raw) else . end

        elif ($n | has_suffix(":parameters_set")) then
          # *:parameters_set unchanged
          .

        else
          # "errory": names with "error"/"closed"/"connection_lost" or quic:path_*
          if ( ($n | has_sub("error"))
               or ($n | has_sub("closed"))
               or ($n | has_prefix("quic:path_"))
               or ($n | has_sub("connection_lost")) ) then
            if has("data") and (.data|type)=="object" then .data |= del(.raw) else . end

          elif ($n | has_prefix("recovery:")) then
            # recovery:* -> only recovery:packet_lost is kept
            if $n == "recovery:packet_lost" then . else empty end

          elif ($n == "quic:stream_data_moved") then
            # drop entirely
            empty

          elif ($n=="quic:packet_sent" or $n=="quic:packet_received") then
            # Packet events: minimize header/raw/frames
            if has("data") and (.data|type)=="object" then
              .data |= (
                if has("header") and (.header|type)=="object"
                then .header |= (
                  . as $h
                  | ( {}
                      | (if $h|has("packet_type")   then . + {packet_type:   $h.packet_type}   else . end)
                      | (if $h|has("packet_number") then . + {packet_number: $h.packet_number} else . end)
                      | (if $h|has("scil")          then . + {scil:          $h.scil}          else . end)
                      | (if $h|has("dcil")          then . + {dcil:          $h.dcil}          else . end)
                    )
                )
                else . end
              )
              | .data |= (
                  if has("raw") and (.raw|type)=="object"
                  then .raw |= (
                    . as $r
                    | ( {}
                        | (if $r|has("length")         then . + {length:         $r.length}         else . end)
                        | (if $r|has("payload_length") then . + {payload_length: $r.payload_length} else . end)
                      )
                    )
                  else . end
                )
              | .data |= (
                  if has("frames") and (.frames|type)=="array"
                  then .frames |= map(
                        if type=="object"
                        then ( . as $f
                               | ( {}
                                   | (if $f|has("frame_type") then . + {frame_type: $f.frame_type} else . end)
                                   | (if $f|has("stream_id")  then . + {stream_id:  $f.stream_id}  else . end)
                                 )
                             )
                        else .
                        end
                      )
                  else . end
                )
            else . end

          else
            # Default branch: remove data.raw and prune frames
            if has("data") and (.data|type)=="object" then
              .data |= del(.raw)
              | .data |= (
                  if has("frames") and (.frames|type)=="array"
                  then .frames |= map(
                        if type=="object"
                        then (
                          . as $f
                          | (
                              del(.raw, .payload_length, .length_in_bytes)
                              | . as $g
                              | (
                                  if ($g|has("frame_type") or $g|has("stream_id"))
                                  then
                                    ( {}
                                      | (if $g|has("frame_type") then . + {frame_type: $g.frame_type} else . end)
                                      | (if $g|has("stream_id")  then . + {stream_id:  $g.stream_id}  else . end)
                                    )
                                  else
                                    $g
                                  end
                                )
                            )
                        )
                        else .
                        end
                      )
                  else . end
                )
            else . end
          end
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

# Quick sanity check
jq -e '.qlog_version=="0.3" and .qlog_format=="JSON" and (.traces|type)=="array"' "$OUT" >/dev/null
echo "Wrote $OUT (minimal=$MINIMAL)"
