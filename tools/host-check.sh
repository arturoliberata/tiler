#!/usr/bin/env bash
# Type-check and lint the Windows binary from a Linux/macOS host.
#
# The `windows` crates are `#![cfg(windows)]`, so a normal `cargo check` on a
# non-Windows host sees no Win32 API at all. This script copies the workspace to
# a scratch directory and compiles only the windows-* crates with `--cfg windows`
# (nightly-only profile rustflags, enabled with RUSTC_BOOTSTRAP). Nothing is linked
# or run; on Windows itself, just use `cargo build`.
set -euo pipefail

src="$(cd "$(dirname "$0")/.." && pwd)"
dst="${TMPDIR:-/tmp}/tiler-host-check"
mkdir -p "$dst"
if command -v rsync >/dev/null; then
  rsync -a --delete --exclude target "$src/" "$dst/ws/"
else
  rm -rf "$dst/ws" && mkdir -p "$dst/ws"
  (cd "$src" && tar --exclude=./target -cf - .) | (cd "$dst/ws" && tar -xf -)
fi

python3 - "$dst/ws/Cargo.toml" <<'EOF'
import sys
p = sys.argv[1]
s = open(p).read()
s = 'cargo-features = ["profile-rustflags"]\n' + s
flags = '["--cfg", "windows", "-A", "explicit_builtin_cfgs_in_flags", "-Zcrate-attr=feature(raw_dylib_elf)", "-Aincomplete_features"]'
for c in ["windows", "windows-core", "windows-future", "windows-collections", "windows-numerics",
          "windows-link", "windows-result", "windows-strings", "windows-threading"]:
    s += f'\n[profile.dev.package.{c}]\nrustflags = {flags}\n'
open(p, "w").write(s)
EOF

cd "$dst/ws"
CARGO_TARGET_DIR="$dst/target" RUSTC_BOOTSTRAP=1 cargo clippy -p tiler -p tiler-core --all-targets -- -D warnings
echo "host check OK"
