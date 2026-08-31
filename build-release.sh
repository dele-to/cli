#!/bin/sh
set -eu

script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
repo_dir="$(dirname "$script_dir")"
output_dir="$repo_dir/public/cli/latest"

case "${1:-}" in
  "") ;;
  --install-tools)
    if ! command -v brew >/dev/null 2>&1; then
      printf 'error: Homebrew is required to install Zig\n' >&2
      exit 1
    fi
    command -v zig >/dev/null 2>&1 || brew install zig
    command -v cargo-zigbuild >/dev/null 2>&1 || cargo install cargo-zigbuild --version 0.23.2 --locked
    command -v cargo-xwin >/dev/null 2>&1 || cargo install cargo-xwin --version 0.23.1 --locked
    ;;
  *)
    printf 'usage: %s [--install-tools]\n' "$0" >&2
    exit 2
    ;;
esac

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'error: required command not found: %s\n' "$1" >&2
    printf 'run %s --install-tools\n' "$0" >&2
    exit 1
  fi
}

if [ "$(uname -s)" != Darwin ]; then
  printf 'error: this release build currently requires macOS\n' >&2
  exit 1
fi

for command in cargo rustup zig cargo-zigbuild cargo-xwin clang python3 tar zip unzip shasum file strings; do
  require_command "$command"
done

for target in \
  aarch64-apple-darwin \
  x86_64-apple-darwin \
  aarch64-unknown-linux-musl \
  x86_64-unknown-linux-musl \
  aarch64-pc-windows-msvc \
  x86_64-pc-windows-msvc
do
  rustup target add "$target"
done

shim_dir="$(mktemp -d)"
trap 'rm -rf "$shim_dir"' EXIT
trap 'exit 1' HUP INT TERM
export DELETO_REAL_CLANG="$(command -v clang)"
export DELETO_ZIG="$(command -v zig)"

cat > "$shim_dir/clang" <<'PYTHON'
#!/usr/bin/env python3
import os
import sys

arguments = []
index = 1
while index < len(sys.argv):
    argument = sys.argv[index]
    if argument == "/imsvc" and index + 1 < len(sys.argv):
        arguments.extend(["-isystem", sys.argv[index + 1]])
        index += 2
    elif argument.startswith("/imsvc"):
        arguments.extend(["-isystem", argument[6:]])
        index += 1
    else:
        arguments.append(argument)
        index += 1

os.execv(os.environ["DELETO_REAL_CLANG"], ["clang", *arguments])
PYTHON

cat > "$shim_dir/llvm-ar" <<'SH'
#!/bin/sh
exec "$DELETO_ZIG" ar "$@"
SH

cat > "$shim_dir/llvm-lib" <<'PYTHON'
#!/usr/bin/env python3
import os
import sys

output = None
objects = []
for argument in sys.argv[1:]:
    lowered = argument.lower()
    if lowered.startswith("-out:") or lowered.startswith("/out:"):
        output = argument[5:]
    elif lowered in ("-nologo", "/nologo"):
        continue
    elif not output or argument != output:
        objects.append(argument)

if not output:
    raise SystemExit("llvm-lib shim: missing output path")
os.execv(os.environ["DELETO_ZIG"], ["zig", "ar", "rcs", output, *objects])
PYTHON

chmod +x "$shim_dir/clang" "$shim_dir/llvm-ar" "$shim_dir/llvm-lib"
PATH="$shim_dir:$PATH"
export PATH

cd "$script_dir"
cargo test
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
cargo zigbuild --release --target aarch64-unknown-linux-musl
cargo zigbuild --release --target x86_64-unknown-linux-musl
cargo xwin build --release --target aarch64-pc-windows-msvc
cargo xwin build --release --target x86_64-pc-windows-msvc

assert_file_contains() {
  description="$(file "$1")"
  case "$description" in
    *"$2"*) ;;
    *)
      printf 'error: unexpected binary format: %s\n' "$description" >&2
      exit 1
      ;;
  esac
}

assert_file_contains target/aarch64-apple-darwin/release/deleto "Mach-O 64-bit executable arm64"
assert_file_contains target/x86_64-apple-darwin/release/deleto "Mach-O 64-bit executable x86_64"
assert_file_contains target/aarch64-unknown-linux-musl/release/deleto "ARM aarch64"
assert_file_contains target/aarch64-unknown-linux-musl/release/deleto "statically linked"
assert_file_contains target/x86_64-unknown-linux-musl/release/deleto "x86-64"
assert_file_contains target/x86_64-unknown-linux-musl/release/deleto "statically linked"
assert_file_contains target/aarch64-pc-windows-msvc/release/deleto.exe "Aarch64"
assert_file_contains target/x86_64-pc-windows-msvc/release/deleto.exe "x86-64"

for binary in \
  target/aarch64-apple-darwin/release/deleto \
  target/x86_64-apple-darwin/release/deleto \
  target/aarch64-unknown-linux-musl/release/deleto \
  target/x86_64-unknown-linux-musl/release/deleto \
  target/aarch64-pc-windows-msvc/release/deleto.exe \
  target/x86_64-pc-windows-msvc/release/deleto.exe
do
  strings "$binary" | grep -Fq "Reveal delete capability:"
  strings "$binary" | grep -Fq "Get an API key at https://dele.to/developers"
done

for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
  archive="deleto-$target.tar.gz"
  tar -C "$script_dir/target/$target/release" -czf "$output_dir/$archive" deleto
done

for target in aarch64-pc-windows-msvc x86_64-pc-windows-msvc; do
  archive="$output_dir/deleto-$target.zip"
  rm -f "$archive"
  (cd "$script_dir/target/$target/release" && zip -q -j -9 "$archive" deleto.exe)
done

cd "$output_dir"
for archive in *.tar.gz *.zip; do
  shasum -a 256 "$archive" > "$archive.sha256"
done
for checksum in *.sha256; do
  shasum -a 256 -c "$checksum"
done

for archive in *.tar.gz; do
  [ "$(tar -tzf "$archive")" = deleto ]
done
for archive in *.zip; do
  [ "$(unzip -Z1 "$archive")" = deleto.exe ]
done

printf '\nBuilt and verified:\n'
for archive in *.tar.gz *.zip; do
  printf '  %s\n' "$output_dir/$archive"
done
