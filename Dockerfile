# syntax=docker/dockerfile:1
ARG RUST_IMAGE=rust:1-alpine
FROM ${RUST_IMAGE} AS build
RUN apk add --no-cache build-base pkgconf
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src ./src
COPY web ./web
COPY prompts ./prompts
COPY deploy/desktop ./deploy/desktop
RUN cargo build --locked --release

FROM node:24-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl docker.io git python3 tini \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/dotbot /usr/local/bin/dotbot
COPY LICENSE NOTICE THIRD_PARTY.md /usr/share/doc/dotbot/
COPY licenses /usr/share/doc/dotbot/licenses
COPY deploy/docker/entrypoint.sh /usr/local/bin/dotbot-entrypoint
RUN chmod 755 /usr/local/bin/dotbot-entrypoint
ENV DOTBOT_HOME=/srv/dotbot
WORKDIR /srv/dotbot
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s \
    CMD curl --silent --fail --output /dev/null http://127.0.0.1:4870/login || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/dotbot-entrypoint"]
CMD ["serve", "--workspace", "/srv/dotbot/workspace"]
