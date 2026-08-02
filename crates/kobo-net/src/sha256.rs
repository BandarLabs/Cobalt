//! A small, dependency-free SHA-256.
//!
//! Hashing is done in process so a downloaded artifact can be verified
//! against its published digest before anything is written to disk, and so
//! the workspace keeps its zero-dependency guarantee.

const ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

const INITIAL_STATE: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// Returns the lowercase hexadecimal SHA-256 digest of `message`.
#[must_use]
pub fn hex_digest(message: &[u8]) -> String {
    let mut state = INITIAL_STATE;
    for chunk in padded(message).chunks_exact(64) {
        compress(&mut state, chunk);
    }

    let mut digest = String::with_capacity(64);
    for word in state {
        for byte in word.to_be_bytes() {
            digest.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
            digest.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
        }
    }
    digest
}

fn padded(message: &[u8]) -> Vec<u8> {
    let bit_length = (message.len() as u64).wrapping_mul(8);
    let mut blocks = message.to_vec();
    blocks.push(0x80);
    while blocks.len() % 64 != 56 {
        blocks.push(0);
    }
    blocks.extend_from_slice(&bit_length.to_be_bytes());
    blocks
}

fn schedule(chunk: &[u8]) -> [u32; 64] {
    let mut words = [0_u32; 64];
    for (index, word) in chunk.chunks_exact(4).enumerate() {
        words[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    }
    for index in 16..64 {
        let previous = words[index - 15];
        let recent = words[index - 2];
        let sigma0 = previous.rotate_right(7) ^ previous.rotate_right(18) ^ (previous >> 3);
        let sigma1 = recent.rotate_right(17) ^ recent.rotate_right(19) ^ (recent >> 10);
        words[index] = words[index - 16]
            .wrapping_add(sigma0)
            .wrapping_add(words[index - 7])
            .wrapping_add(sigma1);
    }
    words
}

fn compress(state: &mut [u32; 8], chunk: &[u8]) {
    let words = schedule(chunk);
    let mut working = *state;
    for (index, word) in words.iter().enumerate() {
        let [alpha, beta, gamma, delta, epsilon, zeta, eta, theta] = working;
        let sum1 = epsilon.rotate_right(6) ^ epsilon.rotate_right(11) ^ epsilon.rotate_right(25);
        let choose = (epsilon & zeta) ^ ((!epsilon) & eta);
        let temp1 = theta
            .wrapping_add(sum1)
            .wrapping_add(choose)
            .wrapping_add(ROUND_CONSTANTS[index])
            .wrapping_add(*word);
        let sum0 = alpha.rotate_right(2) ^ alpha.rotate_right(13) ^ alpha.rotate_right(22);
        let majority = (alpha & beta) ^ (alpha & gamma) ^ (beta & gamma);
        working = [
            temp1.wrapping_add(sum0.wrapping_add(majority)),
            alpha,
            beta,
            gamma,
            delta.wrapping_add(temp1),
            epsilon,
            zeta,
            eta,
        ];
    }
    for (slot, value) in state.iter_mut().zip(working) {
        *slot = slot.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use super::hex_digest;

    #[test]
    fn matches_the_published_test_vectors() {
        assert_eq!(
            hex_digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex_digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex_digest(&vec![0x61_u8; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn digests_are_always_sixty_four_lowercase_hex_characters() {
        for length in [0_usize, 1, 55, 56, 63, 64, 65, 1000] {
            let digest = hex_digest(&vec![0x5a_u8; length]);
            assert_eq!(digest.len(), 64);
            assert!(digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        }
    }
}
