# The web app's production image. Build from the repository root (the app
# uses the workspace packages in packages/):
#
#   docker build -f web/Dockerfile -t guiderecorder-web .
#
# Targets: `runtime` (default) serves the app; `build` has the full toolchain
# and runs migrations (`pnpm -C web db:migrate`).

FROM node:24-alpine AS build
RUN corepack enable
WORKDIR /repo
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY app/package.json app/
COPY web/package.json web/
COPY packages/guide-schema/package.json packages/guide-schema/
COPY packages/sync/package.json packages/sync/
COPY packages/ui/package.json packages/ui/
RUN pnpm install --frozen-lockfile --filter "@guiderecorder/web..."
COPY packages packages
COPY web web
RUN pnpm -C web build

FROM node:24-alpine AS runtime
ENV NODE_ENV=production PORT=3000
WORKDIR /app
COPY --from=build --chown=node:node /repo/web/.output ./.output
USER node
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD node -e "fetch('http://127.0.0.1:' + process.env.PORT + '/api/health').then((r) => process.exit(r.ok ? 0 : 1), () => process.exit(1))"
CMD ["node", ".output/server/index.mjs"]
