# renovate: datasource=node depName=node
ARG NODE_VERSION=20.9.0

FROM node:${NODE_VERSION}-alpine

# renovate: datasource=npm depName=pnpm
ARG PNPM_VERSION=8.10.2

# Install `pnpm`
RUN npm install --global pnpm@$PNPM_VERSION

WORKDIR /app

COPY pnpm-lock.yaml /app/

# Download all locked dependencies
RUN pnpm fetch

COPY . /app

# Install dependencies from previously downloaded pnpm store
RUN pnpm install --offline

ENTRYPOINT ["pnpm", "start:staging"]
