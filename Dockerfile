# Builds the Rust app inside Docker (Linux), so it does not depend on this PC's Windows settings, and is the image that is deployed.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home app
WORKDIR /app
COPY --from=build /src/target/release/florida-3pl /app/florida-3pl
COPY web ./web
RUN mkdir -p /app/data && chown -R app:app /app/data
ENV HOST=0.0.0.0 PORT=8765 WEB_DIR=/app/web
EXPOSE 8765
USER app
CMD ["/app/florida-3pl"]
