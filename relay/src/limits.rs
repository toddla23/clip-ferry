use std::time::Instant;

/// 토큰 버킷. `capacity`까지 모이고 초당 `per_sec`개씩 채워진다.
pub struct Bucket {
    capacity: f64,
    tokens: f64,
    per_sec: f64,
    last: Instant,
}

impl Bucket {
    pub fn new(capacity: f64, per_sec: f64) -> Self {
        Self {
            capacity,
            tokens: capacity,
            per_sec,
            last: Instant::now(),
        }
    }

    /// `n`개를 쓸 수 있으면 쓰고 `true`.
    pub fn take(&mut self, n: f64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.per_sec).min(self.capacity);
        if self.tokens >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_runs_out_then_refuses() {
        let mut b = Bucket::new(3.0, 0.0);
        assert!(b.take(1.0));
        assert!(b.take(2.0));
        assert!(!b.take(1.0));
    }
}
