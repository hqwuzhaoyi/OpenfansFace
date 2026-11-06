# syntax=docker/dockerfile:1
FROM node:22-slim AS build
COPY --from=oven/bun:1 /usr/local/bin/bun /usr/local/bin/bun
WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN corepack enable && corepack prepare pnpm@10.10.0 --activate && pnpm install --frozen-lockfile
COPY . .
RUN pnpm build

FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends tar && rm -rf /var/lib/apt/lists/*
WORKDIR /app
ENV NODE_ENV=production HOSTNAME=0.0.0.0 FACES_BASE=/data/base MEDIA2_ROOT=/mnt/media2 DOCKER_SOCK=/var/run/docker.sock
COPY --from=build /app/.next/standalone ./
COPY --from=build /app/.next/static ./.next/static
COPY --from=build /app/public ./public
CMD ["node", "server.js"]
