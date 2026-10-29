# proto

모든 와이어 포맷의 원본은 `clip_ferry.proto`다.
`core/build.rs`가 순수 Rust 컴파일러(`protox`)와 `prost-build`로 Rust 타입을 만든다(`protoc` 설치 불필요).
