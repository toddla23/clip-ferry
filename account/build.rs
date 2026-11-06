// `sqlx::migrate!`는 컴파일할 때 migrations/를 실행 파일에 넣는다. 새 파일을 더해도
// cargo가 다시 컴파일하지 않으므로 폴더가 바뀌면 다시 빌드하게 한다 (sqlx 문서 권장).
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
