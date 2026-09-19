#!/usr/bin/env bash
# Build the Angular client, compress it, and push it to the board.
#
#   ./deploy.sh                 # build, then deploy to whichever port is found
#   ./deploy.sh -p COM4         # name the port
#   ./deploy.sh -s              # skip the build, push what is in dist/
#   ./deploy.sh -n              # skip the reset, to poke about in the REPL
#
# The Git Bash twin of deploy.ps1.

set -euo pipefail
cd "$(dirname "$0")"

PORT=""
SKIP_BUILD=0
NO_RESET=0

while [ $# -gt 0 ]; do
    case "$1" in
        -p|--port)       PORT="$2"; shift 2 ;;
        -s|--skip-build) SKIP_BUILD=1; shift ;;
        -n|--no-reset)   NO_RESET=1; shift ;;
        -h|--help) sed -n '2,9p' "$0" | sed 's/^# \?//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
cyan()  { printf '\033[36m%s\033[0m\n' "$*"; }

ANGULAR_DIR="../nanacoin/angular"

# Angular 22 puts the actual site under dist/<project>/browser/, alongside
# build metadata (3rdpartylicenses.txt, prerendered-routes.json) that is not
# part of the site. Publishing the parent would put index.html at
# /www/browser/index.html, where static.py does not look for it, and ship
# 18KB of licence text to a board with 4MB of flash.
DIST_ROOT="$ANGULAR_DIR/dist/nanacoin-web"
DIST_DIR="$DIST_ROOT/browser"

find_python() {
    local candidates=()
    command -v python >/dev/null 2>&1 && candidates+=("$(command -v python)")
    candidates+=("/c/Users/matth/AppData/Local/Programs/Python/Python312/python.exe")
    candidates+=("/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe")

    local py
    for py in "${candidates[@]}"; do
        [ -x "$py" ] || continue
        if "$py" -m mpremote --version >/dev/null 2>&1; then
            printf '%s' "$py"
            return 0
        fi
    done
    return 1
}

PY="$(find_python)" || {
    red "Could not find a Python with mpremote installed."
    echo "  python -m pip install mpremote esptool pyserial"
    exit 1
}

# --- build ------------------------------------------------------------------

if [ "$SKIP_BUILD" -ne 1 ]; then
    cyan "building the Angular client..."
    ( cd "$ANGULAR_DIR" && npm run build )
fi

if [ ! -d "$DIST_DIR" ]; then
    # Older Angular layouts put the site straight in dist/<project>.
    if [ -f "$DIST_ROOT/index.html" ]; then
        DIST_DIR="$DIST_ROOT"
    else
        red "No build at $DIST_DIR"
        echo "  Run without -s, or build it by hand first."
        exit 1
    fi
fi

if [ ! -f "$DIST_DIR/index.html" ]; then
    red "No index.html in $DIST_DIR"
    echo "  The build looks incomplete; check 'npm run build' output."
    exit 1
fi

# --- compress ---------------------------------------------------------------
#
# Gzipping happens here, on a PC with a spare CPU, rather than on a board with
# 4MB of flash and one core. The board never compresses anything at runtime -
# it serves the .gz when the browser says it accepts it.
#
# Only text compresses usefully. A .png or .woff2 is already compressed and a
# .gz of one is usually larger, costing flash twice.

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

MANIFEST="$STAGE/.manifest"
: > "$MANIFEST"

original_bytes=0
shipped_bytes=0

size_of() { stat -c %s "$1"; }

while IFS= read -r -d '' file; do
    rel="${file#"$DIST_DIR"/}"
    bytes="$(size_of "$file")"
    original_bytes=$(( original_bytes + bytes ))

    # Source maps are the largest thing in the build and exist for debugging.
    # On 4MB of flash they do not earn their place.
    case "$rel" in *.map) continue ;; esac

    target="$STAGE/$rel"
    mkdir -p "$(dirname "$target")"

    case "${rel##*.}" in
        html|js|css|json|svg|txt)
            gzip -9 -c "$file" > "$target.gz"
            gz_bytes="$(size_of "$target.gz")"

            # index.html ships BOTH ways, which is the one exception to
            # "smaller copy only".
            #
            # It is the only file a client that refuses gzip ever asks for:
            # assets are fetched by a browser that has already parsed this
            # HTML, and every browser sends Accept-Encoding: gzip. Without a
            # plain copy, such a client makes the board inflate a file into
            # RAM to answer - which works, but is real work for a 240MHz core
            # with 2MB of heap. A ~1.7KB second copy makes that path
            # effectively dead code, kept only as a correctness backstop.
            if [ "$(basename "$rel")" = "index.html" ]; then
                cp "$file" "$target"
                shipped_bytes=$(( shipped_bytes + bytes + gz_bytes ))
                printf '%s	%s
' "$rel" "$bytes" >> "$MANIFEST"
                printf '%s	%s
' "$rel.gz" "$gz_bytes" >> "$MANIFEST"
                continue
            fi

            # Everything else: ship whichever is smaller. A tiny file can gzip
            # larger than it started, and shipping both wastes the flash twice.
            if [ "$gz_bytes" -lt "$bytes" ]; then
                shipped_bytes=$(( shipped_bytes + gz_bytes ))
                printf '%s\t%s\n' "$rel.gz" "$gz_bytes" >> "$MANIFEST"
            else
                rm -f "$target.gz"
                cp "$file" "$target"
                shipped_bytes=$(( shipped_bytes + bytes ))
                printf '%s\t%s\n' "$rel" "$bytes" >> "$MANIFEST"
            fi
            ;;
        *)
            cp "$file" "$target"
            shipped_bytes=$(( shipped_bytes + bytes ))
            printf '%s\t%s\n' "$rel" "$bytes" >> "$MANIFEST"
            ;;
    esac
done < <(find "$DIST_DIR" -type f -print0)

file_count="$(wc -l < "$MANIFEST" | tr -d ' ')"
cyan "build $(( original_bytes / 1024 )) KB -> shipping $(( shipped_bytes / 1024 )) KB in $file_count files"

# 4MB of flash, most of it already firmware. Refuse rather than half-fill it.
if [ "$shipped_bytes" -gt 1572864 ]; then
    red "That is more than 1.5MB, which is more than this board should hold."
    echo "  Check for source maps or unoptimised assets in the build."
    exit 1
fi

# --- port -------------------------------------------------------------------

if [ -z "$PORT" ]; then
    PORT="$(./find_port.sh || true)"
fi
if [ -z "$PORT" ]; then
    red "No board found."
    echo "  Tap RESET, then try again. The port moves on every reset."
    exit 1
fi

cyan "checking the board on $PORT..."
if ! "$PY" -m mpremote connect "$PORT" eval "1+1" >/dev/null 2>&1; then
    red "No MicroPython answering on $PORT."
    echo "  Flash it first:  ./flash_micropython.sh"
    echo "  Or tap RESET and re-run - the port changes on every reset."
    exit 1
fi

# --- push -------------------------------------------------------------------
#
# Wipe /www first. Angular emits hashed filenames, so a rebuild writes new
# names rather than overwriting the old ones; without this the board silently
# accumulates every build it has ever been given until the flash fills.

cyan "clearing /www..."
"$PY" -m mpremote connect "$PORT" exec '
import os
def rm(d):
    try: entries = os.listdir(d)
    except OSError: return
    for e in entries:
        p = d + "/" + e
        try:
            if os.stat(p)[0] & 0x4000: rm(p); os.rmdir(p)
            else: os.remove(p)
        except OSError as err: print("could not remove", p, err)
rm("/www")
try: os.mkdir("/www")
except OSError: pass
'

cyan "copying the site..."
made_dirs=""
while IFS=$'\t' read -r rel bytes; do
    reldir="$(dirname "$rel")"
    if [ "$reldir" != "." ]; then
        case " $made_dirs " in
            *" $reldir "*) ;;
            *)
                "$PY" -m mpremote connect "$PORT" exec \
                    "import os
try: os.mkdir('/www/$reldir')
except OSError: pass" >/dev/null 2>&1
                made_dirs="$made_dirs $reldir"
                ;;
        esac
    fi
    "$PY" -m mpremote connect "$PORT" fs cp "$STAGE/$rel" ":/www/$rel"
    printf '  %-46s %8s B\n' "$rel" "$bytes"
done < "$MANIFEST"

# The server itself last, so a half-copied site never runs.
for f in static.py config.py main.py; do
    if [ ! -f "$f" ]; then
        if [ "$f" = "config.py" ]; then
            red "config.py missing - copy config_example.py and add your WiFi details"
            exit 1
        fi
        continue
    fi
    "$PY" -m mpremote connect "$PORT" fs cp "$f" ":$f"
    echo "  $f"
done

if [ "$NO_RESET" -ne 1 ]; then
    cyan "resetting..."
    "$PY" -m mpremote connect "$PORT" reset
fi

echo
green "Done. The site should come up at http://nanacoin.local/"
echo "Point it at the API board once:"
echo "  http://nanacoin.local/?api=<nanacoin-s3-ip>"
echo
echo "Watch it boot:  $PY -m mpremote connect $PORT repl"
