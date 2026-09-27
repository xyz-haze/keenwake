FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY presets presets
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd -r -u 10001 alertsift && mkdir /data && chown alertsift /data
COPY --from=build /src/target/release/alertsift /usr/local/bin/alertsift
USER alertsift
WORKDIR /data
EXPOSE 8080
ENTRYPOINT ["alertsift"]
CMD ["--config", "/etc/alertsift/alertsift.toml", "serve"]
