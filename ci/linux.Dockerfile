FROM rust:1.97.1-bookworm

ARG TARGETARCH

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        build-essential \
        bubblewrap \
        clang \
        cmake \
        curl \
        dbus-daemon \
        fonts-dejavu-core \
        libfontconfig-dev \
        libglib2.0-dev \
        libssl-dev \
        libvulkan1 \
        libwayland-dev \
        libx11-xcb-dev \
        libxkbcommon-x11-dev \
        mesa-vulkan-drivers \
        ncurses-bin \
        pkg-config \
        python3 \
        weston \
        xauth \
        xvfb \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    case "${TARGETARCH}" in \
        arm64) \
            zig_arch="aarch64"; \
            zig_sha="ea4b09bfb22ec6f6c6ceac57ab63efb6b46e17ab08d21f69f3a48b38e1534f17" \
            ;; \
        amd64) \
            zig_arch="x86_64"; \
            zig_sha="70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00" \
            ;; \
        *) \
            echo "unsupported Docker architecture: ${TARGETARCH}" >&2; \
            exit 1 \
            ;; \
    esac; \
    zig_archive="zig-${zig_arch}-linux-0.16.0.tar.xz"; \
    curl --fail --location --proto '=https' --tlsv1.2 \
        --output "/tmp/${zig_archive}" \
        "https://ziglang.org/download/0.16.0/${zig_archive}"; \
    echo "${zig_sha}  /tmp/${zig_archive}" | sha256sum --check; \
    mkdir /opt/zig; \
    tar --extract --xz --file "/tmp/${zig_archive}" --strip-components=1 --directory /opt/zig; \
    rm "/tmp/${zig_archive}"

RUN set -eux; \
    case "${TARGETARCH}" in \
        arm64) \
            appimage_arch="aarch64"; \
            tool_sha="f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158"; \
            runtime_sha="00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444" \
            ;; \
        amd64) \
            appimage_arch="x86_64"; \
            tool_sha="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"; \
            runtime_sha="2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d" \
            ;; \
        *) \
            echo "unsupported Docker architecture: ${TARGETARCH}" >&2; \
            exit 1 \
            ;; \
    esac; \
    tool="/tmp/appimagetool-${appimage_arch}.AppImage"; \
    curl --fail --location --proto '=https' --tlsv1.2 --output "$tool" \
        "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-${appimage_arch}.AppImage"; \
    echo "${tool_sha}  ${tool}" | sha256sum --check; \
    chmod 0755 "$tool"; \
    mkdir -p /opt/appimagetool; \
    cd /opt/appimagetool; \
    "$tool" --appimage-extract >/dev/null; \
    curl --fail --location --proto '=https' --tlsv1.2 \
        --output /opt/appimage-runtime \
        "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-${appimage_arch}"; \
    echo "${runtime_sha}  /opt/appimage-runtime" | sha256sum --check; \
    chmod 0755 /opt/appimage-runtime; \
    rm "$tool"

RUN set -eux; \
    case "${TARGETARCH}" in \
        arm64) \
            linuxdeploy_arch="aarch64"; \
            linuxdeploy_sha="620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff" \
            ;; \
        amd64) \
            linuxdeploy_arch="x86_64"; \
            linuxdeploy_sha="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d" \
            ;; \
        *) \
            echo "unsupported Docker architecture: ${TARGETARCH}" >&2; \
            exit 1 \
            ;; \
    esac; \
    tool="/tmp/linuxdeploy-${linuxdeploy_arch}.AppImage"; \
    curl --fail --location --proto '=https' --tlsv1.2 --output "$tool" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-${linuxdeploy_arch}.AppImage"; \
    echo "${linuxdeploy_sha}  ${tool}" | sha256sum --check; \
    chmod 0755 "$tool"; \
    mkdir -p /opt/linuxdeploy; \
    cd /opt/linuxdeploy; \
    "$tool" --appimage-extract >/dev/null; \
    rm "$tool"

RUN curl --fail --location --proto '=https' --tlsv1.2 \
        --output /tmp/ghostty.tar.gz \
        https://github.com/ghostty-org/ghostty/archive/22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018.tar.gz \
    && echo "5fdb21d3744ce76dfaefb0e25fd113540906939752627a4193b88520e126c389  /tmp/ghostty.tar.gz" \
        | sha256sum --check \
    && mkdir /opt/ghostty-source \
    && tar --extract --gzip --file /tmp/ghostty.tar.gz \
        --strip-components=1 --directory /opt/ghostty-source \
    && rm /tmp/ghostty.tar.gz

ENV PATH="/opt/zig:${PATH}"
ENV CARGO_BUILD_JOBS=2
ENV GHOSTTY_SOURCE_DIR=/opt/ghostty-source

RUN rustup component add clippy rustfmt

WORKDIR /workspace
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/workspace/target \
    rustc --version \
    && cargo --version \
    && zig version \
    && cargo fmt --all --check \
    && ./ci/audit-normal-licenses.sh . \
    && cargo test --workspace --locked \
    && cargo clippy --workspace --all-targets --locked -- -D warnings \
    && python3 ci/release-metadata.py generate --workspace . --output dist

# All source acquisition is complete above. Build release artifacts and run the
# package smoke with the build step's network namespace disabled.
RUN --network=none \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/workspace/target \
    cargo build --workspace --release --locked --offline \
    && TERMI9NE_USE_PREGENERATED_METADATA=1 \
        TERMI9NE_APPIMAGETOOL=/opt/appimagetool/squashfs-root/AppRun \
        TERMI9NE_APPIMAGE_RUNTIME=/opt/appimage-runtime \
        TERMI9NE_LINUXDEPLOY=/opt/linuxdeploy/squashfs-root/AppRun \
        ./ci/package-smoke.sh . \
    && TERMI9NE_SOAK_SECONDS=5 TERMI9NE_SOAK_SESSIONS=12 ./ci/runtime-soak.sh . \
    && ./ci/linux-window-smoke.sh target/release/termi9ne-desktop \
    && APPIMAGE_EXTRACT_AND_RUN=1 \
        ./ci/linux-window-smoke.sh "dist/termi9ne-linux-$(uname -m).AppImage"
