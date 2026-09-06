# A hosted ultraplexr runtime in a container. Build from the repository root:
#   docker build -f ci/daemon.Dockerfile -t ultraplexr-daemon .
#   docker run -d --name ultraplexr -p 7373:7373 -v ultraplexr-state:/var/lib/ultraplexr ultraplexr-daemon
#   docker exec -u ultraplexr ultraplexr ultraplexr --socket /tmp/ultraplexr/control.sock device-pair --label laptop
# The build stage is the same toolchain CI uses (Rust + Zig for the VT engine).
FROM rust:1.97.1-bookworm AS build
ARG TARGETARCH
RUN apt-get update && apt-get install --yes --no-install-recommends build-essential clang cmake curl pkg-config xz-utils && rm -rf /var/lib/apt/lists/*
RUN set -eux; case "${TARGETARCH}" in arm64) zig_arch="aarch64";; amd64) zig_arch="x86_64";; *) echo "unsupported ${TARGETARCH}"; exit 1;; esac; \
    curl -fsSL "https://ziglang.org/download/0.14.1/zig-linux-${zig_arch}-0.14.1.tar.xz" -o /tmp/zig.tar.xz; \
    mkdir -p /opt/zig && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1 && ln -s /opt/zig/zig /usr/local/bin/zig && rm /tmp/zig.tar.xz
WORKDIR /src
COPY . .
RUN cargo build --release -p ultraplexr-daemon -p ultraplexr-cli -j 2

FROM debian:bookworm-slim
RUN apt-get update && apt-get install --yes --no-install-recommends ca-certificates curl bash procps && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /var/lib/ultraplexr --shell /bin/bash ultraplexr
COPY --from=build /src/target/release/ultraplexr-daemon /usr/local/bin/ultraplexr-daemon
COPY --from=build /src/target/release/ultraplexr /usr/local/bin/ultraplexr
USER ultraplexr
ENV HOME=/var/lib/ultraplexr SHELL=/bin/bash
VOLUME ["/var/lib/ultraplexr"]
EXPOSE 7373
ENTRYPOINT ["/usr/local/bin/ultraplexr-daemon", "--socket", "/tmp/ultraplexr/control.sock", "--state-dir", "/var/lib/ultraplexr", "--gateway", "0.0.0.0:7373"]
