# The export worker: `guiderecorder` (the Rust CLI) and the Node worker that
# feeds it (worker/export.ts). Build from the repository root:
#   docker build -f web/worker/Dockerfile -t guiderecorder-export-worker .
# Needs the web app's DATABASE_URL and S3_* variables.

FROM rust:1-bookworm AS cli
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates crates
# Only the CLI is built; the desktop app isn't part of this context.
RUN sed -i '/"app\/src-tauri",/d' Cargo.toml \
 && cargo build --release -p guiderecorder-cli

FROM node:24-bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates zip \
 && rm -rf /var/lib/apt/lists/* \
 && corepack enable
WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY app/package.json app/
COPY packages packages
COPY web web
RUN pnpm install --frozen-lockfile --filter @guiderecorder/web...
COPY --from=cli /src/target/release/guiderecorder /usr/local/bin/guiderecorder

ENV NODE_ENV=production GUIDERECORDER_CLI=/usr/local/bin/guiderecorder
WORKDIR /app/web
USER node
CMD ["node_modules/.bin/tsx", "worker/export.ts"]
