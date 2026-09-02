#!/bin/sh
set -eu

workspace=${1:-.}
release_dir="$workspace/target/release"
artifact_dir="$workspace/dist"
smoke_dir=$(mktemp -d "${TMPDIR:-/tmp}/termi9ne-package.XXXXXX")
trap 'rm -rf "$smoke_dir"' EXIT INT TERM

for executable in termi9ne termi9ne-server termi9ne-desktop; do
    test -x "$release_dir/$executable" || {
        echo "missing release executable: $release_dir/$executable" >&2
        exit 1
    }
done

mkdir -p "$artifact_dir"
if test "${TERMI9NE_USE_PREGENERATED_METADATA:-0}" = 1; then
    test -s "$artifact_dir/termi9ne.spdx.json"
    test -s "$artifact_dir/THIRD_PARTY_NOTICES.txt"
else
    python3 "$workspace/ci/release-metadata.py" generate \
        --workspace "$workspace" --output "$artifact_dir"
fi
"$release_dir/termi9ne" --help >/dev/null
"$release_dir/termi9ne-server" --help >/dev/null

case "$(uname -s)" in
    Darwin)
        app="$smoke_dir/termi9ne.app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/terminfo" \
            "$app/Contents/Resources/licenses"
        install -m 0755 "$release_dir/termi9ne-desktop" "$app/Contents/MacOS/termi9ne"
        install -m 0755 "$release_dir/termi9ne-server" "$app/Contents/MacOS/termi9ne-server"
        install -m 0755 "$release_dir/termi9ne" "$app/Contents/MacOS/termi9ne-cli"
        install -m 0644 "$workspace/packaging/macos/Info.plist" "$app/Contents/Info.plist"
        install -m 0644 "$workspace/LICENSE" "$app/Contents/Resources/licenses/LICENSE"
        install -m 0644 "$artifact_dir/THIRD_PARTY_NOTICES.txt" \
            "$app/Contents/Resources/licenses/THIRD_PARTY_NOTICES.txt"
        install -m 0644 "$artifact_dir/termi9ne.spdx.json" \
            "$app/Contents/Resources/termi9ne.spdx.json"
        tic -x -o "$app/Contents/Resources/terminfo" \
            "$workspace/packaging/terminfo/xterm-ghostty.terminfo"
        infocmp -A "$app/Contents/Resources/terminfo" xterm-ghostty >/dev/null
        plutil -lint "$app/Contents/Info.plist" >/dev/null
        codesign --force --deep --sign - "$app"
        codesign --verify --deep --strict "$app"
        COPYFILE_DISABLE=1 ditto -c -k --norsrc --keepParent \
            "$app" "$artifact_dir/termi9ne-macos-smoke.zip"
        test -s "$artifact_dir/termi9ne-macos-smoke.zip"
        echo "macOS app-bundle smoke PASS"
        ;;
    Linux)
        appdir="$smoke_dir/termi9ne.AppDir"
        mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
            "$appdir/usr/share/termi9ne/terminfo" "$appdir/usr/share/doc/termi9ne" \
            "$appdir/usr/share/metainfo"
        install -m 0755 "$release_dir/termi9ne-desktop" "$appdir/usr/bin/termi9ne-desktop"
        install -m 0755 "$release_dir/termi9ne" "$appdir/usr/bin/termi9ne"
        install -m 0755 "$release_dir/termi9ne-server" "$appdir/usr/bin/termi9ne-server"
        install -m 0644 "$workspace/packaging/linux/com.termi9ne.desktop.desktop" \
            "$appdir/usr/share/applications/com.termi9ne.desktop.desktop"
        install -m 0755 "$workspace/packaging/linux/AppRun" "$appdir/AppRun"
        install -m 0644 "$workspace/packaging/linux/com.termi9ne.desktop.desktop" \
            "$appdir/termi9ne.desktop"
        install -m 0644 "$workspace/packaging/linux/termi9ne.svg" "$appdir/termi9ne.svg"
        install -m 0644 "$workspace/packaging/linux/com.termi9ne.metainfo.xml" \
            "$appdir/usr/share/metainfo/com.termi9ne.metainfo.xml"
        install -m 0644 "$workspace/LICENSE" "$appdir/usr/share/doc/termi9ne/LICENSE"
        install -m 0644 "$artifact_dir/THIRD_PARTY_NOTICES.txt" \
            "$appdir/usr/share/doc/termi9ne/THIRD_PARTY_NOTICES.txt"
        install -m 0644 "$artifact_dir/termi9ne.spdx.json" \
            "$appdir/usr/share/doc/termi9ne/termi9ne.spdx.json"
        tic -x -o "$appdir/usr/share/termi9ne/terminfo" \
            "$workspace/packaging/terminfo/xterm-ghostty.terminfo"
        infocmp -A "$appdir/usr/share/termi9ne/terminfo" xterm-ghostty >/dev/null
        : "${TERMI9NE_LINUXDEPLOY:?set TERMI9NE_LINUXDEPLOY to the pinned linuxdeploy AppRun}"
        test -x "$TERMI9NE_LINUXDEPLOY"
        "$TERMI9NE_LINUXDEPLOY" --appdir "$appdir"
        if ldd "$appdir/usr/bin/termi9ne-desktop" | grep -q 'not found'; then
            echo "Linux package has unresolved shared libraries" >&2
            exit 1
        fi
        tar -C "$smoke_dir" -czf "$artifact_dir/termi9ne-linux-smoke.tar.gz" termi9ne.AppDir
        test -s "$artifact_dir/termi9ne-linux-smoke.tar.gz"

        : "${TERMI9NE_APPIMAGETOOL:?set TERMI9NE_APPIMAGETOOL to the pinned appimagetool AppRun}"
        : "${TERMI9NE_APPIMAGE_RUNTIME:?set TERMI9NE_APPIMAGE_RUNTIME to the pinned type-2 runtime}"
        test -x "$TERMI9NE_APPIMAGETOOL"
        test -s "$TERMI9NE_APPIMAGE_RUNTIME"
        architecture=$(dpkg --print-architecture)
        case "$architecture" in
            amd64) appimage_arch=x86_64 ;;
            arm64) appimage_arch=aarch64 ;;
            *) echo "unsupported AppImage architecture: $architecture" >&2; exit 1 ;;
        esac
        appimage="$artifact_dir/termi9ne-linux-$appimage_arch.AppImage"
        ARCH="$appimage_arch" VERSION=0.1.0 \
            "$TERMI9NE_APPIMAGETOOL" --runtime-file "$TERMI9NE_APPIMAGE_RUNTIME" \
            "$appdir" "$appimage"
        chmod 0755 "$appimage"
        test -s "$appimage"
        "$appimage" --appimage-offset >/dev/null

        debroot="$smoke_dir/termi9ne-deb"
        mkdir -p "$debroot/DEBIAN" "$debroot/usr/bin" "$debroot/usr/share/applications" \
            "$debroot/usr/share/termi9ne/terminfo" "$debroot/usr/share/doc/termi9ne"
        sed "s/ARCHITECTURE/$architecture/" "$workspace/packaging/linux/control" \
            >"$debroot/DEBIAN/control"
        install -m 0755 "$release_dir/termi9ne-desktop" "$debroot/usr/bin/termi9ne-desktop"
        install -m 0755 "$release_dir/termi9ne" "$debroot/usr/bin/termi9ne"
        install -m 0755 "$release_dir/termi9ne-server" "$debroot/usr/bin/termi9ne-server"
        install -m 0644 "$workspace/packaging/linux/com.termi9ne.desktop.desktop" \
            "$debroot/usr/share/applications/com.termi9ne.desktop.desktop"
        cp -R "$appdir/usr/share/termi9ne/terminfo/." \
            "$debroot/usr/share/termi9ne/terminfo/"
        cp -R "$appdir/usr/share/doc/termi9ne/." "$debroot/usr/share/doc/termi9ne/"
        dpkg-deb --root-owner-group --build "$debroot" "$artifact_dir/termi9ne-linux.deb" >/dev/null
        dpkg-deb --info "$artifact_dir/termi9ne-linux.deb" >/dev/null
        test -s "$artifact_dir/termi9ne-linux.deb"
        echo "Linux AppImage, AppDir archive, and .deb smoke PASS"
        ;;
    *)
        echo "unsupported packaging host: $(uname -s)" >&2
        exit 1
        ;;
esac

python3 "$workspace/ci/release-metadata.py" checksums --output "$artifact_dir"
