FROM node:24-bookworm-slim AS web
WORKDIR /build
COPY package.json package-lock.json ./
COPY apps/web/package.json apps/web/package.json
RUN npm ci
COPY apps/web apps/web
RUN npm run build:web

FROM rust:1-bookworm AS server
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release --locked -p deep-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --create-home deep
WORKDIR /app
COPY --from=server /build/target/release/deep-server /usr/local/bin/deep-server
COPY --from=web /build/apps/web/dist /app/web
RUN mkdir /app/data && chown deep:deep /app/data
USER deep
ENV DEEP_BIND=0.0.0.0:4317 DEEP_WEB_DIR=/app/web DEEP_DATA_DIR=/app/data
EXPOSE 4317
CMD ["deep-server"]

