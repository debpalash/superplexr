#!/bin/sh
set -eu

workspace=${1:-.}
audit_dir=$(mktemp -d "${TMPDIR:-/tmp}/ultraplexr-license-audit.XXXXXX")
trap 'rm -rf "$audit_dir"' EXIT INT TERM

cd "$workspace"
workspace_root=$(pwd -P)
tree_file="$audit_dir/normal-build-tree.txt"

cargo tree \
    --package ultraplexr-desktop \
    --edges normal,build \
    --locked \
    --format '{p}|{l}' \
    --prefix none >"$tree_file"

if grep -E '\|$' "$tree_file"; then
    echo "normal/build dependency graph contains a package without a declared license" >&2
    exit 1
fi

if grep -E '\|.*LicenseRef' "$tree_file"; then
    echo "normal/build dependency graph contains an unrecognized license reference" >&2
    exit 1
fi

grep -E '\|.*(AGPL|GPL|LGPL|SSPL)' "$tree_file" >"$audit_dir/copyleft-candidates.txt" || true
while IFS='|' read -r package license; do
    case "$license" in
        *" OR "*)
            case "$license" in
                *MIT*|*Apache-2.0*|*BSD-*|*ISC*|*Zlib*|*Unicode-*|*Unlicense*|*CC0-*)
                    continue
                    ;;
            esac
            ;;
    esac

    echo "$package|$license" >&2
    echo "normal/build dependency graph contains a forbidden copyleft license" >&2
    exit 1
done <"$audit_dir/copyleft-candidates.txt"

for package in gpui_shared_string gpui_util ztracing; do
    resolution=$(cargo tree \
        --package ultraplexr-desktop \
        --edges normal,build \
        --invert "$package" \
        --depth 0 \
        --locked \
        --prefix none)

    case "$resolution" in
        *"$workspace_root/crates/"*) ;;
        *)
            echo "$package did not resolve to a ultraplexr compatibility crate" >&2
            exit 1
            ;;
    esac

    case "$resolution" in
        *https://*|*git+*)
            echo "$package unexpectedly resolved from a remote source" >&2
            exit 1
            ;;
    esac
done

resolution=$(cargo tree --package ultraplexr-desktop --edges normal,build \
    --invert libghostty-vt-sys --depth 0 --locked --prefix none)
case "$resolution" in
    *"$workspace_root/vendor/libghostty-vt-sys"*) ;;
    *)
        echo "libghostty-vt-sys did not resolve to the reviewed native build patch" >&2
        exit 1
        ;;
esac

echo "normal/build dependency license audit PASS"
