FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY presets presets
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd -r -u 10001 keenwake && mkdir /data && chown keenwake /data
COPY --from=build /src/target/release/keenwake /usr/local/bin/keenwake
USER keenwake
WORKDIR /data
EXPOSE 8080
ENTRYPOINT ["keenwake"]
CMD ["--config", "/etc/keenwake/keenwake.toml", "serve"]
