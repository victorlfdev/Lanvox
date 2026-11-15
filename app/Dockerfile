# Lanvox host: runs every server, friends and private calls on one machine.
#   docker build -t lanvox-host .
#   docker run -d -p 47800:47800 -v lanvox-data:/data lanvox-host
FROM rust:1-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./build.rs
COPY assets ./assets
COPY vendor ./vendor
COPY src ./src
# Only the host: no window, audio, camera or screen-capture libraries needed.
RUN cargo build --locked --release --no-default-features

FROM debian:stable-slim
RUN groupadd --gid 10001 lanvox && useradd --system --uid 10001 --gid 10001 --create-home --home-dir /data lanvox
COPY --from=build /src/target/release/lanvox /usr/local/bin/lanvox
ENV LANVOX_NAME="Lanvox Hub" LANVOX_BIND=0.0.0.0 LANVOX_PORT=47800 LANVOX_DATA=/data
USER 10001:10001
VOLUME /data
EXPOSE 47800/tcp
EXPOSE 47800/udp
STOPSIGNAL SIGTERM
ENTRYPOINT ["lanvox", "--server"]
