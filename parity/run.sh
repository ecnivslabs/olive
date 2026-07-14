#!/bin/bash
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PARITY_DIR="parity"
PARITY_BIN="parity/bin"

echo "Building Olive compiler and stdlib (release)..."
cargo build --release -q
cargo build --release -p olive_std -q

# Stage libolive_std.so where find_library_dir() can find it
mkdir -p grove/release
cp target/release/libolive_std.so grove/release/
mkdir -p "$PARITY_BIN"

PIT="./target/release/pit"
NUMPY_OK=1
python3 -c "import numpy" >/dev/null 2>&1 || NUMPY_OK=0

PASS=0
FAIL=0
SKIP=0

for py_file in "$PARITY_DIR"/*.py; do
    name=$(basename "$py_file" .py)
    liv_file="$PARITY_DIR/${name}.liv"

    if [ ! -f "$liv_file" ]; then
        echo "SKIP  $name (no .liv twin)"
        continue
    fi

    if [[ "$name" == *numpy* ]] && [ "$NUMPY_OK" -eq 0 ]; then
        echo "SKIP  $name (numpy absent)"
        SKIP=$((SKIP + 1))
        continue
    fi

    py_out=$(python3 "$py_file" 2>&1)
    jit_out=$("$PIT" run "$liv_file" 2>&1)

    liv_bin="$PARITY_BIN/${name}_liv"
    if ! build_err=$("$PIT" build --release "$liv_file" -o "$liv_bin" 2>&1); then
        echo "FAIL  $name (AOT build error)"
        echo "$build_err" | sed 's/^/      /'
        FAIL=$((FAIL + 1))
        continue
    fi
    aot_out=$("$liv_bin" 2>&1)

    if [ "$py_out" == "$jit_out" ] && [ "$py_out" == "$aot_out" ]; then
        echo "PASS  $name"
        PASS=$((PASS + 1))
    else
        echo "FAIL  $name"
        diff <(echo "$py_out") <(echo "$jit_out") | sed 's/^/      jit: /'
        diff <(echo "$py_out") <(echo "$aot_out") | sed 's/^/      aot: /'
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "================================================================="
echo "$PASS passed, $FAIL failed, $SKIP skipped"
echo "================================================================="

if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
