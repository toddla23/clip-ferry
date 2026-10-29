# clip-ferry

여러 기기(Windows, macOS, Android, iOS) 사이에서 클립보드의 텍스트와 이미지를 종단간 암호화로 주고받는 서비스.
서버(릴레이)는 암호문을 중계만 하고 아무것도 저장하지 않는다.

설계와 진행 상황은 [`docs/`](docs/)에 있다. 시작은 [`docs/01-overview.md`](docs/01-overview.md).

## 구성

| 경로 | 내용 |
|---|---|
| `core/` | 공용 Rust 코어 (`clip-ferry-core`) |
| `relay/` | 릴레이 서버 (`clip-ferry-relay`) |
| `tools/cli/` | 개발·검증용 CLI (`clip-ferry-cli`) |
| `proto/` | 와이어 포맷 원본 |

## 개발

```
cargo build --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
