//! Random choices for games and original fixtures. Never silently substitutes
//! time or a fixed seed when the operating system cannot provide entropy.
use std::io::{self, Read};

pub trait Entropy {
    /// # Errors
    /// Returns source failure; callers must not replace it with a fixed seed.
    fn next_u64(&mut self) -> io::Result<u64>;
    /// Unbiased choice in 0..upper. A broken source cannot loop indefinitely.
    /// # Errors
    /// Refuses an empty range, exhausted source or repeated rejected samples.
    fn below(&mut self, upper: u64) -> io::Result<u64> {
        if upper == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty choice"));
        }
        let threshold = upper.wrapping_neg() % upper;
        for _ in 0..1024 {
            let value = self.next_u64()?;
            if value >= threshold {
                return Ok(value % upper);
            }
        }
        Err(io::Error::other(
            "entropy source did not produce an admissible choice",
        ))
    }
}
/// Opens the operating system source once; failure is returned to the app.
#[derive(Debug)]
pub struct SystemEntropy(std::fs::File);
impl SystemEntropy {
    /// # Errors
    /// Returns an error when the operating system entropy source is unavailable.
    pub fn open() -> io::Result<Self> {
        std::fs::File::open("/dev/urandom").map(Self)
    }
}
impl Entropy for SystemEntropy {
    fn next_u64(&mut self) -> io::Result<u64> {
        let mut bytes = [0; 8];
        self.0.read_exact(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }
}
/// Explicit deterministic fixture source. Public seeds must never generate
/// credentials, pairing secrets, tokens or security-sensitive identifiers.
#[derive(Clone, Debug)]
pub struct FixtureEntropy {
    seed: u64,
    counter: u64,
}
impl FixtureEntropy {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { seed, counter: 0 }
    }
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }
}
impl Entropy for FixtureEntropy {
    fn next_u64(&mut self) -> io::Result<u64> {
        let next = self
            .counter
            .checked_add(1)
            .ok_or_else(|| io::Error::other("fixture sequence exhausted"))?;
        let mut input = [0; 16];
        input[..8].copy_from_slice(&self.seed.to_le_bytes());
        input[8..].copy_from_slice(&self.counter.to_le_bytes());
        let digest = kobo_net::sha256::hex_digest(&input);
        let value = u64::from_str_radix(&digest[..16], 16).map_err(io::Error::other)?;
        self.counter = next;
        Ok(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures_replay_and_different_seeds_differ() {
        let mut a = FixtureEntropy::new(7);
        let mut replay = a.clone();
        let mut b = FixtureEntropy::new(8);
        let sequence: Vec<_> = (0..64).map(|_| a.below(6).unwrap()).collect();
        assert_eq!(
            sequence,
            (0..64)
                .map(|_| replay.below(6).unwrap())
                .collect::<Vec<_>>()
        );
        assert_ne!(
            sequence,
            (0..64).map(|_| b.below(6).unwrap()).collect::<Vec<_>>()
        );
        assert!(sequence.iter().all(|&value| value < 6));
        assert!(a.below(0).is_err());
    }
    #[test]
    fn rejection_removes_modulo_bias_and_bad_sources_are_bounded() {
        struct Source(u64);
        impl Entropy for Source {
            fn next_u64(&mut self) -> io::Result<u64> {
                self.0 += 1;
                Ok(self.0 - 1)
            }
        }
        struct Broken;
        impl Entropy for Broken {
            fn next_u64(&mut self) -> io::Result<u64> {
                Ok(0)
            }
        }
        let mut source = Source(0);
        assert_eq!(source.below(6).unwrap(), 4); // 2^64 mod 6 = 4: 0..3 rejected.
        assert!(Broken.below(6).is_err());
    }
}
