#!/bin/sh
set -eu

workspace=${1:-.}
release_dir="$workspace/target/release"
artifact_dir="$workspace/dist"
smoke_dir=$(mktemp -d "${TMPDIR:-/tmp}/superplexr-package.XXXXXX")
trap 'rm -rf "$smoke_dir"' EXIT INT TERM

for executable in superplexr superplexr-server superplexr-desktop; do
    test -x "$release_dir/$executable" || {
        echo "missing release executable: $release_dir/$executable" >&2
        exit 1
    }
done

mkdir -p "$artifact_dir"
if test "${SUPERPLEXR_USE_PREGENERATED_METADATA:-0}" = 1; then
    test -s "$artifact_dir/superplexr.spdx.json"
    test -s "$artifact_dir/THIRD_PARTY_NOTICES.txt"
else
    python3 "$workspace/ci/release-metadata.py" generate \
        --workspace "$workspace" --output "$artifact_dir"
fi
"$release_dir/superplexr" --help >/dev/null
"$release_dir/superplexr-server" --help >/dev/null

case "$(uname -s)" in
    Darwin)
        app="$smoke_dir/superplexr.app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/terminfo" \
            "$app/Contents/Resources/licenses"
        install -m 0755 "$release_dir/superplexr-desktop" "$app/Contents/MacOS/superplexr"
        install -m 0755 "$release_dir/superplexr-server" "$app/Contents/MacOS/superplexr-server"
        install -m 0755 "$release_dir/superplexr" "$app/Contents/MacOS/superplexr-cli"
        install -m 0644 "$workspace/packaging/macos/Info.plist" "$app/Contents/Info.plist"
        install -m 0644 "$workspace/packaging/macos/SuperPlexr.icns" \
            "$app/Contents/Resources/SuperPlexr.icns"
        install -m 0644 "$workspace/LICENSE" "$app/Contents/Resources/licenses/LICENSE"
        install -m 0644 "$artifact_dir/THIRD_PARTY_NOTICES.txt" \
            "$app/Contents/Resources/licenses/THIRD_PARTY_NOTICES.txt"
        install -m 0644 "$artifact_dir/superplexr.spdx.json" \
            "$app/Contents/Resources/superplexr.spdx.json"
        tic -x -o "$app/Contents/Resources/terminfo" \
            "$workspace/packaging/terminfo/xterm-ghostty.terminfo"
        infocmp -A "$app/Contents/Resources/terminfo" xterm-ghostty >/dev/null
        plutil -lint "$app/Contents/Info.plist" >/dev/null
        codesign --force --deep --sign - "$app"
        codesign --verify --deep --strict "$app"
        COPYFILE_DISABLE=1 ditto -c -k --norsrc --keepParent \
            "$app" "$artifact_dir/superplexr-macos-smoke.zip"
        test -s "$artifact_dir/superplexr-macos-smoke.zip"
        echo "macOS app-bundle smoke PASS"
        ;;
    Linux)
        appdir="$smoke_dir/superplexr.AppDir"
        mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
            "$appdir/usr/share/superplexr/terminfo" "$appdir/usr/share/doc/superplexr" \
            "$appdir/usr/share/metainfo"
        install -m 0755 "$release_dir/superplexr-desktop" "$appdir/usr/bin/superplexr-desktop"
        install -m 0755 "$release_dir/superplexr" "$appdir/usr/bin/superplexr"
        install -m 0755 "$release_dir/superplexr-server" "$appdir/usr/bin/superplexr-server"
        install -m 0644 "$workspace/packaging/linux/com.superplexr.desktop.desktop" \
            "$appdir/usr/share/applications/com.superplexr.desktop.desktop"
        install -m 0755 "$workspace/packaging/linux/AppRun" "$appdir/AppRun"
        install -m 0644 "$workspace/packaging/linux/com.superplexr.desktop.desktop" \
            "$appdir/superplexr.desktop"
        install -m 0644 "$workspace/packaging/linux/superplexr.svg" "$appdir/superplexr.svg"
        install -m 0644 "$workspace/packaging/linux/com.superplexr.metainfo.xml" \
            "$appdir/usr/share/metainfo/com.superplexr.metainfo.xml"
        install -m 0644 "$workspace/LICENSE" "$appdir/usr/share/doc/superplexr/LICENSE"
        install -m 0644 "$artifact_dir/THIRD_PARTY_NOTICES.txt" \
            "$appdir/usr/share/doc/superplexr/THIRD_PARTY_NOTICES.txt"
        install -m 0644 "$artifact_dir/superplexr.spdx.json" \
            "$appdir/usr/share/doc/superplexr/superplexr.spdx.json"
        tic -x -o "$appdir/usr/share/superplexr/terminfo" \
            "$workspace/packaging/terminfo/xterm-ghostty.terminfo"
        infocmp -A "$appdir/usr/share/superplexr/terminfo" xterm-ghostty >/dev/null
        : "${SUPERPLEXR_LINUXDEPLOY:?set SUPERPLEXR_LINUXDEPLOY to the pinned linuxdeploy AppRun}"
        test -x "$SUPERPLEXR_LINUXDEPLOY"
        "$SUPERPLEXR_LINUXDEPLOY" --appdir "$appdir"
        if ldd "$appdir/usr/bin/superplexr-desktop" | grep -q 'not found'; then
            echo "Linux package has unresolved shared libraries" >&2
            exit 1
        fi
        tar -C "$smoke_dir" -czf "$artifact_dir/superplexr-linux-smoke.tar.gz" superplexr.AppDir
        test -s "$artifact_dir/superplexr-linux-smoke.tar.gz"

        : "${SUPERPLEXR_APPIMAGETOOL:?set SUPERPLEXR_APPIMAGETOOL to the pinned appimagetool AppRun}"
        : "${SUPERPLEXR_APPIMAGE_RUNTIME:?set SUPERPLEXR_APPIMAGE_RUNTIME to the pinned type-2 runtime}"
        test -x "$SUPERPLEXR_APPIMAGETOOL"
        test -s "$SUPERPLEXR_APPIMAGE_RUNTIME"
        architecture=$(dpkg --print-architecture)
        case "$architecture" in
            amd64) appimage_arch=x86_64 ;;
            arm64) appimage_arch=aarch64 ;;
            *) echo "unsupported AppImage architecture: $architecture" >&2; exit 1 ;;
        esac
        appimage="$artifact_dir/superplexr-linux-$appimage_arch.AppImage"
        ARCH="$appimage_arch" VERSION=0.1.0 \
            "$SUPERPLEXR_APPIMAGETOOL" --runtime-file "$SUPERPLEXR_APPIMAGE_RUNTIME" \
            "$appdir" "$appimage"
        chmod 0755 "$appimage"
        test -s "$appimage"
        "$appimage" --appimage-offset >/dev/null

        debroot="$smoke_dir/superplexr-deb"
        mkdir -p "$debroot/DEBIAN" "$debroot/usr/bin" "$debroot/usr/share/applications" \
            "$debroot/usr/share/superplexr/terminfo" "$debroot/usr/share/doc/superplexr"
        sed "s/ARCHITECTURE/$architecture/" "$workspace/packaging/linux/control" \
            >"$debroot/DEBIAN/control"
        install -m 0755 "$release_dir/superplexr-desktop" "$debroot/usr/bin/superplexr-desktop"
        install -m 0755 "$release_dir/superplexr" "$debroot/usr/bin/superplexr"
        install -m 0755 "$release_dir/superplexr-server" "$debroot/usr/bin/superplexr-server"
        install -m 0644 "$workspace/packaging/linux/com.superplexr.desktop.desktop" \
            "$debroot/usr/share/applications/com.superplexr.desktop.desktop"
        cp -R "$appdir/usr/share/superplexr/terminfo/." \
            "$debroot/usr/share/superplexr/terminfo/"
        cp -R "$appdir/usr/share/doc/superplexr/." "$debroot/usr/share/doc/superplexr/"
        dpkg-deb --root-owner-group --build "$debroot" "$artifact_dir/superplexr-linux.deb" >/dev/null
        dpkg-deb --info "$artifact_dir/superplexr-linux.deb" >/dev/null
        test -s "$artifact_dir/superplexr-linux.deb"
        echo "Linux AppImage, AppDir archive, and .deb smoke PASS"
        ;;
    *)
        echo "unsupported packaging host: $(uname -s)" >&2
        exit 1
        ;;
esac

python3 "$workspace/ci/release-metadata.py" checksums --output "$artifact_dir"
