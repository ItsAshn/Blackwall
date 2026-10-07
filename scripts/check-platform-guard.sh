#!/usr/bin/env sh
# OS-specific code lives only in crates/bw-platform (PLAN §3.2).
set -eu
hits=$(grep -rn --include='*.rs' 'target_os' crates | grep -v '^crates/bw-platform/' || true)
if [ -n "$hits" ]; then
  echo "cfg(target_os) outside bw-platform:" >&2
  echo "$hits" >&2
  exit 1
fi
echo "platform guard: ok"
