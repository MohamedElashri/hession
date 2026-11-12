FROM rust:1.90-bookworm AS build
WORKDIR /build
COPY . .
RUN cargo build --locked --release -p hession-relay

FROM debian:bookworm-slim
RUN groupadd --gid 10001 hession && useradd --uid 10001 --gid 10001 --no-create-home hession \
    && mkdir /data && chown hession:hession /data
COPY --from=build /build/target/release/hession-relay /usr/local/bin/hession-relay
ENV HESSION_RELAY_DATA_DIR=/data HESSION_RELAY_BIND=0.0.0.0:8744
USER 10001:10001
VOLUME ["/data"]
EXPOSE 8744
ENTRYPOINT ["hession-relay"]
CMD ["serve"]
