//! Yerel semantik vektörler: Ollama embedding yoksa hashed bag-of-words.

const LEXICAL_DIM: usize = 256;

/// Unicode token + bigram feature hashing; L2-normalize.
pub fn lexical_embedding(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; LEXICAL_DIM];
    let tokens = tokenize(text);
    if tokens.is_empty() {
        return vec;
    }

    for token in &tokens {
        bump(&mut vec, fnv1a(token), 1.0);
    }
    for window in tokens.windows(2) {
        let bigram = format!("{} {}", window[0], window[1]);
        bump(&mut vec, fnv1a(&bigram), 0.75);
    }
    l2_normalize(&mut vec);
    vec
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (a, b) in left.iter().zip(right.iter()) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return Some(0.0);
    }
    Some(dot / (left_norm.sqrt() * right_norm.sqrt()))
}

pub fn encode_embedding(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + values.len() * 4);
    bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub fn decode_embedding(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() < 4 {
        return None;
    }
    let dim = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    if dim == 0 || bytes.len() != 4 + dim * 4 {
        return None;
    }
    let mut values = Vec::with_capacity(dim);
    for chunk in bytes[4..].chunks_exact(4) {
        values.push(f32::from_le_bytes(chunk.try_into().ok()?));
    }
    Some(values)
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| token.len() > 1)
        .map(ToOwned::to_owned)
        .collect()
}

fn bump(vec: &mut [f32], hash: u64, weight: f32) {
    let index = (hash as usize) % vec.len();
    vec[index] += weight;
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn l2_normalize(vec: &mut [f32]) {
    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vec {
            *value /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_topics_outrank_unrelated() {
        let query = lexical_embedding("NATS dispatcher dinleyici lounge.task.requested");
        let close = lexical_embedding("dispatcher NATS mesaj dinle task requested");
        let far = lexical_embedding("dead code prune echo embeddings python");
        let close_score = cosine_similarity(&query, &close).unwrap();
        let far_score = cosine_similarity(&query, &far).unwrap();
        assert!(close_score > far_score);
        assert!(close_score > 0.2);
    }

    #[test]
    fn embedding_roundtrip_preserves_values() {
        let original = lexical_embedding("kernel experience store");
        let decoded = decode_embedding(&encode_embedding(&original)).unwrap();
        assert_eq!(original.len(), decoded.len());
        for (left, right) in original.iter().zip(decoded.iter()) {
            assert!((left - right).abs() < 1e-6);
        }
    }
}
