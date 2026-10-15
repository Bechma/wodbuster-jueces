FROM lukemathwalker/cargo-chef:latest AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked --bin wodbuster-jueces \
    && strip target/release/wodbuster-jueces

FROM debian:latest AS runtime
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 --shell /usr/sbin/nologin app \
    && install -d -o app -g app /home/app/.config/wodbuster-jueces
COPY --from=builder /app/target/release/wodbuster-jueces /usr/local/bin/wodbuster-jueces
ENV HOST=0.0.0.0 PORT=3000 HOME=/home/app
WORKDIR /app
USER 10001:10001
EXPOSE 3000
CMD ["wodbuster-jueces"]
