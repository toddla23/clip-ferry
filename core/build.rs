fn main() {
    // protoc 없이 빌드하도록 순수 Rust 컴파일러(protox)를 쓴다.
    let fds = protox::compile(["clip_ferry.proto"], ["../proto"]).expect("proto 컴파일 실패");
    prost_build::Config::new()
        .compile_fds(fds)
        .expect("prost 코드 생성 실패");
    println!("cargo:rerun-if-changed=../proto/clip_ferry.proto");
}
