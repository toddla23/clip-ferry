# 테스트 전용 키

`test-google-a.pem`, `test-google-b.pem`은 **테스트에서 구글 대신 ID 토큰에 서명하는 RSA 키**다(2026-11-06에 `openssl genpkey`로 만듦). 어떤 실제 서비스에도 쓰지 않는다.

- `a`: 테스트 서버가 믿는 "구글" 키. 공개키는 `test-google-a.jwks.json`(kid `test-kid-a`)
- `b`: 믿지 않는 키. 위조 토큰을 만드는 데 쓴다
