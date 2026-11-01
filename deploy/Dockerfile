# clip-ferry 릴레이 이미지 (멀티스테이지). 빌드한 기기의 아키텍처로 만들어진다 (Pi 5 = arm64).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p clip-ferry-relay --locked

# 런타임: 셸·패키지 관리자가 없는 최소 이미지, 비루트 사용자.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/clip-ferry-relay /usr/local/bin/clip-ferry-relay
ENV CF_RELAY_LISTEN=0.0.0.0:8787
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/clip-ferry-relay"]
