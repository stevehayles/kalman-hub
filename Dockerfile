FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked && mkdir -p /out/data && cp target/release/kalman-hub /out/

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build --chown=65532:65532 /out/data /data
COPY --from=build /out/kalman-hub /usr/local/bin/kalman-hub
ENV HUB_DB=/data/hub.db \
    HUB_BIND=0.0.0.0:8080 \
    HUB_PUBLIC_BIND=0.0.0.0:8081 \
    HUB_TIMEZONE=Europe/London
EXPOSE 8080 8081
VOLUME ["/data"]
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/kalman-hub"]
