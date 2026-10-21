#!/bin/bash
# Build every program as SBPFv3, plus its IDL and TS types — a drop-in for
# `anchor build`.
#
# `anchor build` alone is not enough: cargo-build-sbf's default arch differs by
# release (v0 up to 4.3, v3 from 4.4), and `anchor build -- --arch v3` also hands
# the flag to the IDL build's `cargo test`, which rejects it. So the bytecode and
# the IDL are built in two steps. The arch must match the verifiable build's
# `--arch` (VERIFY.md) and SBF_ARCH in integration-tests/build.rs.
set -euo pipefail

ARCH=v3

cd "$(dirname "$0")/.."

anchor build --no-idl -- --arch "$ARCH"

mkdir -p target/idl target/types
for dir in programs/*/; do
    prog=$(basename "$dir" | tr '-' '_')
    anchor idl build -p "$prog" -o "target/idl/${prog}.json" -t "target/types/${prog}.ts"

    # e_flags (ELF offset 0x30) carries the SBPF version.
    so="target/deploy/${prog}.so"
    version=$(od -An -t u1 -j 48 -N 1 "$so" | tr -d ' ')
    if [ "$version" != "${ARCH#v}" ]; then
        echo "❌ $so is SBPF v$version, expected $ARCH" >&2
        exit 1
    fi
done
