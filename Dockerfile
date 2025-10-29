ARG PROJECT_ENTRYPOINT=runner

FROM rust:slim AS build
ARG PROJECT_ENTRYPOINT

WORKDIR /app

# System deps for builds that use OpenSSL / quiche / tquic, etc.
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential cmake ninja-build perl python3 git \
    pkg-config clang llvm make ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Cache dependencies and config first
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo

COPY crates ./crates
RUN cargo fetch

# Build
RUN cargo build --release -p ${PROJECT_ENTRYPOINT}


# Runtime image
FROM debian:trixie-slim AS runtime
ARG PROJECT_ENTRYPOINT
ARG PROJECT_NAME=quic-lab

# Minimal runtime deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates tzdata nginx-light tini && \
    rm -rf /var/lib/apt/lists/*

# App layout
WORKDIR /app
RUN useradd -r -u 10001 appuser && mkdir -p /app/in && mkdir -p /app/out && chown -R appuser:appuser /app
COPY --from=build /app/target/release/${PROJECT_ENTRYPOINT} /app/${PROJECT_NAME}

# index.html for opt out
COPY ./index.html /var/www/html/index.html

# simple supervisor script
COPY ./docker-entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

ENV SSLKEYLOGFILE=/app/out/sslkeylogfile.txt

EXPOSE 80
ENTRYPOINT ["tini","-g","--","/app/entrypoint.sh"]
