#!/bin/sh
set -eu

workspace=${1:-.}
release_dir="$workspace/target/release"
artifact_dir="$workspace/dist"
smoke_dir=$(mktemp -d "${TMPDIR:-/tmp}/ultraplexr-package.XXXXXX")
trap 'rm -rf "$smoke_dir"' EXIT INT TERM

for executable in ultraplexr ultraplexr-server ultraplexr-desktop; do
    test -x "$release_dir/$executable" || {
        echo "missing release executable: $release_dir/$executable" >&2
        exit 1
    }
done

mkdir -p "$artifact_dir"
if test "${ULTRAPLEXR_USE_PREGENERATED_METADATA:-0}" = 1; then
    test -s "$artifact_dir/ultraplexr.spdx.json"
    test -s "$artifact_dir/THIRD_PARTY_NOTICES.txt"
else
    python3 "$workspace/ci/release-metadata.py" generate \
        --workspace "$workspace" --output "$artifact_dir"
fi
"$release_dir/ultraplexr" --help >/dev/null
"$release_dir/ultraplexr-server" --help >/dev/null

case "$(uname -s)" in
    Darwin)
        app="$smoke_dir/ultraplexr.app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/terminfo" \
            "$app/Contents/Resources/licenses"
        install -m 0755 "$release_dir/ultraplexr-desktop" "$app/Contents/MacOS/ultraplexr"
        install -m 0755 "$release_dir/ultraplexr-server" "$app/Contents/MacOS/ultraplexr-server"
        install -m 0755 "$release_dir/ultraplexr" "$app/Contents/MacOS/ultraplexr-cli"
        install -m 0644 "$workspace/packaging/macos/Info.plist" "$app/Contents/Info.plist"
        install -m 0644 "$workspace/LICENSE" "$app/Contents/Resources/licenses/LICENSE"
        install -m 0644 "$artifact_dir/THIRD_PARTY_NOTICES.txt" \
            "$app/Contents/Resources/licenses/THIRD_PARTY_NOTICES.txt"
        install -m 0644 "$artifact_dir/ultraplexr.spdx.json" \
            "$app/Contents/Resources/ultraplexr.spdx.json"
        tic -x -o "$app/Contents/Resources/terminfo" \
            "$workspace/packaging/terminfo/xterm-ghostty.terminfo"
        infocmp -A "$app/Contents/Resources/terminfo" xterm-ghostty >/dev/null
        plutil -lint "$app/Contents/Info.plist" >/dev/null
        codesign --force --deep --sign - "$app"
        codesign --verify --deep --strict "$app"
        COPYFILE_DISABLE=1 ditto -c -k --norsrc --keepParent \
            "$app" "$artifact_dir/ultraplexr-macos-smoke.zip"
        test -s "$artifact_dir/ultraplexr-macos-smoke.zip"
        echo "macOS app-bundle smoke PASS"
        ;;
    Linux)
        appdir="$smoke_dir/ultraplexr.AppDir"
        mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
            "$appdir/usr/share/ultraplexr/terminfo" "$appdir/usr/share/doc/ultraplexr" \
            "$appdir/usr/share/metainfo"
        install -m 0755 "$release_dir/ultraplexr-desktop" "$appdir/usr/bin/ultraplexr-desktop"
        install -m 0755 "$release_dir/ultraplexr" "$appdir/usr/bin/ultraplexr"
        install -m 0755 "$release_dir/ultraplexr-server" "$appdir/usr/bin/ultraplexr-server"
        install -m 0644 "$workspace/packaging/linux/com.ultraplexr.desktop.desktop" \
            "$appdir/usr/share/applications/com.ultraplexr.desktop.desktop"
        install -m 0755 "$workspace/packaging/linux/AppRun" "$appdir/AppRun"
        install -m 0644 "$workspace/packaging/linux/com.ultraplexr.desktop.desktop" \
            "$appdir/ultraplexr.desktop"
        install -m 0644 "$workspace/packaging/linux/ultraplexr.svg" "$appdir/ultraplexr.svg"
        install -m 0644 "$workspace/packaging/linux/com.ultraplexr.metainfo.xml" \
            "$appdir/usr/share/metainfo/com.ultraplexr.metainfo.xml"
        install -m 0644 "$workspace/LICENSE" "$appdir/usr/share/doc/ultraplexr/LICENSE"
        install -m 0644 "$artifact_dir/THIRD_PARTY_NOTICES.txt" \
            "$appdir/usr/share/doc/ultraplexr/THIRD_PARTY_NOTICES.txt"
        install -m 0644 "$artifact_dir/ultraplexr.spdx.json" \
            "$appdir/usr/share/doc/ultraplexr/ultraplexr.spdx.json"
        tic -x -o "$appdir/usr/share/ultraplexr/terminfo" \
            "$workspace/packaging/terminfo/xterm-ghostty.terminfo"
        infocmp -A "$appdir/usr/share/ultraplexr/terminfo" xterm-ghostty >/dev/null
        : "${ULTRAPLEXR_LINUXDEPLOY:?set ULTRAPLEXR_LINUXDEPLOY to the pinned linuxdeploy AppRun}"
        test -x "$ULTRAPLEXR_LINUXDEPLOY"
        "$ULTRAPLEXR_LINUXDEPLOY" --appdir "$appdir"
        if ldd "$appdir/usr/bin/ultraplexr-desktop" | grep -q 'not found'; then
            echo "Linux package has unresolved shared libraries" >&2
            exit 1
        fi
        tar -C "$smoke_dir" -czf "$artifact_dir/ultraplexr-linux-smoke.tar.gz" ultraplexr.AppDir
        test -s "$artifact_dir/ultraplexr-linux-smoke.tar.gz"

        : "${ULTRAPLEXR_APPIMAGETOOL:?set ULTRAPLEXR_APPIMAGETOOL to the pinned appimagetool AppRun}"
        : "${ULTRAPLEXR_APPIMAGE_RUNTIME:?set ULTRAPLEXR_APPIMAGE_RUNTIME to the pinned type-2 runtime}"
        test -x "$ULTRAPLEXR_APPIMAGETOOL"
        test -s "$ULTRAPLEXR_APPIMAGE_RUNTIME"
        architecture=$(dpkg --print-architecture)
        case "$architecture" in
            amd64) appimage_arch=x86_64 ;;
            arm64) appimage_arch=aarch64 ;;
            *) echo "unsupported AppImage architecture: $architecture" >&2; exit 1 ;;
        esac
        appimage="$artifact_dir/ultraplexr-linux-$appimage_arch.AppImage"
        ARCH="$appimage_arch" VERSION=0.1.0 \
            "$ULTRAPLEXR_APPIMAGETOOL" --runtime-file "$ULTRAPLEXR_APPIMAGE_RUNTIME" \
            "$appdir" "$appimage"
        chmod 0755 "$appimage"
        test -s "$appimage"
        "$appimage" --appimage-offset >/dev/null

        debroot="$smoke_dir/ultraplexr-deb"
        mkdir -p "$debroot/DEBIAN" "$debroot/usr/bin" "$debroot/usr/share/applications" \
            "$debroot/usr/share/ultraplexr/terminfo" "$debroot/usr/share/doc/ultraplexr"
        sed "s/ARCHITECTURE/$architecture/" "$workspace/packaging/linux/control" \
            >"$debroot/DEBIAN/control"
        install -m 0755 "$release_dir/ultraplexr-desktop" "$debroot/usr/bin/ultraplexr-desktop"
        install -m 0755 "$release_dir/ultraplexr" "$debroot/usr/bin/ultraplexr"
        install -m 0755 "$release_dir/ultraplexr-server" "$debroot/usr/bin/ultraplexr-server"
        install -m 0644 "$workspace/packaging/linux/com.ultraplexr.desktop.desktop" \
            "$debroot/usr/share/applications/com.ultraplexr.desktop.desktop"
        cp -R "$appdir/usr/share/ultraplexr/terminfo/." \
            "$debroot/usr/share/ultraplexr/terminfo/"
        cp -R "$appdir/usr/share/doc/ultraplexr/." "$debroot/usr/share/doc/ultraplexr/"
        dpkg-deb --root-owner-group --build "$debroot" "$artifact_dir/ultraplexr-linux.deb" >/dev/null
        dpkg-deb --info "$artifact_dir/ultraplexr-linux.deb" >/dev/null
        test -s "$artifact_dir/ultraplexr-linux.deb"
        echo "Linux AppImage, AppDir archive, and .deb smoke PASS"
        ;;
    *)
        echo "unsupported packaging host: $(uname -s)" >&2
        exit 1
        ;;
esac

python3 "$workspace/ci/release-metadata.py" checksums --output "$artifact_dir"
