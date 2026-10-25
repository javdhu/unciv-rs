# civ-server: the private-room match host. Build context is the repo root.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release --locked -p civ-server

FROM debian:bookworm-slim
COPY --from=build /src/target/release/civ-server /usr/local/bin/civ-server
# The SQLite database lives in /data; mount a volume there to keep it.
ENV CIV_BIND=0.0.0.0:8080 \
    CIV_DATABASE=/data/civ-server.sqlite3 \
    CIV_SECURE_COOKIE=1
EXPOSE 8080
CMD ["civ-server"]
