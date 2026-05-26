#!/bin/bash
# Walk every JSON in the IG export (except per-thread messages, sampled separately)
# and emit: path | shape | length | first-entry keys.
# Designed to be re-runnable against any IG export to detect schema drift.

set -u
ROOT="${1:-/Users/gati3478/Desktop/social-network-project/ig/ig-exported-data}"
cd "$ROOT" || exit 1

emit_one() {
  local f="$1"
  jq -c --arg path "$f" '
    def shape: if type == "object" then "obj"
              elif type == "array" then "arr"
              else "scalar" end;
    def first_keys:
      if type == "object" then (keys | sort)
      elif type == "array" and length > 0 and (.[0] | type) == "object" then (.[0] | keys | sort)
      else null end;
    def wrapped:
      if type == "object" and (keys | length) == 1 then
        keys[0] as $k
        | { wrapper_key: $k,
            inner_shape: (.[$k] | shape),
            inner_len: (.[$k] | if type == "array" then length else null end),
            first_entry_keys: (.[$k] | if type == "array" then (.[0] | if type == "object" then (keys | sort) else null end) else (if type == "object" then (keys | sort) else null end) end) }
      else null end;
    {
      path: $path,
      shape: shape,
      array_len: (if type == "array" then length else null end),
      obj_keys: (if type == "object" then (keys | sort) else null end),
      first_entry_keys: first_keys,
      wrapped: wrapped
    }' "$f" 2>/dev/null || echo "{\"path\":\"$f\",\"error\":\"parse-failed\"}"
}

echo "## All top-level JSON files (excluding per-thread messages)"
find . -type f -name '*.json' \
    ! -path './your_instagram_activity/messages/inbox/*' \
    ! -path './your_instagram_activity/messages/message_requests/*' \
  | sort \
  | while read -r f; do
      emit_one "$f"
    done

echo
echo "## DM thread sample (5 threads from inbox)"
ls your_instagram_activity/messages/inbox/ | head -5 | while read -r thread; do
  for msg_json in your_instagram_activity/messages/inbox/"$thread"/message_*.json; do
    [ -f "$msg_json" ] || continue
    jq -c --arg path "$msg_json" '{
      path: $path,
      top_keys: keys,
      message_count: (.messages | length),
      participants_count: (.participants | length),
      participant_names: [.participants[].name],
      first_msg_keys: (.messages[0] | keys),
      last_msg_keys: (.messages[-1] | keys),
      distinct_msg_keys: ([.messages[] | keys] | add | unique | sort),
      msg_subtype_counts: ([.messages[] | (if has("photos") then "photo" elif has("videos") then "video" elif has("share") then "share" elif has("audio_files") then "audio" elif has("call_duration") then "call" elif has("sticker") then "sticker" elif has("content") then "text" elif has("is_unsent") and .is_unsent == true then "unsent" else "other" end)] | group_by(.) | map({(.[0]): length}) | add)
    }' "$msg_json" 2>/dev/null
  done
done

echo
echo "## Inbox thread counts (folder count + message_*.json multi-part check)"
echo "total_inbox_threads: $(ls your_instagram_activity/messages/inbox | wc -l)"
echo "threads with multiple message_*.json files (multi-part):"
for t in your_instagram_activity/messages/inbox/*/; do
  n=$(ls "$t"/message_*.json 2>/dev/null | wc -l)
  if [ "$n" -gt 1 ]; then echo "  $(basename "$t"): $n parts"; fi
done | head -20
echo "message_requests/ count: $(ls your_instagram_activity/messages/message_requests 2>/dev/null | wc -l)"
echo "ai_conversations.json shape:"
jq 'if type=="object" then keys else type end' your_instagram_activity/messages/ai_conversations.json 2>/dev/null
echo "secret_conversations.json shape:"
jq 'if type=="object" then keys else type end' your_instagram_activity/messages/secret_conversations.json 2>/dev/null

echo
echo "## media/ inventory"
echo "top-level entries: $(ls media | wc -l)"
ls media/
echo "media/posts subdirs:"
ls media/posts
echo "sample post month (first): $(ls media/posts | head -1)"
ls "media/posts/$(ls media/posts | head -1)" | head
echo "any JSON in media/?"
find media -name '*.json' | head -5

echo
echo "## personal_information/ inventory"
find personal_information -name '*.json' | while read -r f; do
  echo "--- $f ---"
  jq -c 'if type=="object" then {keys, sample_label_values: (.label_values | if . then .[0:3] else null end)} else {type:type, len:length} end' "$f" 2>/dev/null
done

echo
echo "## connections/contacts inventory"
find connections/contacts -name '*.json' | while read -r f; do
  echo "--- $f ---"
  jq -c 'if type=="object" then ([keys[] as $k | {wrapper:$k, len: (.[$k] | if type=="array" then length else "scalar" end)}][0]) else {type:type,len:length} end' "$f"
done
