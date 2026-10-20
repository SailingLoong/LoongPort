//! Bounded exact-known-secret screening at a private account owner's output.
//! No protocol parsing, account selection, logging, or secret serialization.
use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::io;
use zeroize::Zeroizing;

const MAX_SECRETS: usize = 4;
const MAX_SECRET_BYTES: usize = 16_384;

struct Pattern {
    bytes: Zeroizing<Vec<u8>>,
    fallback: Vec<usize>,
    matched: usize,
}

/// Exact byte matching only: this is not a general PII/encoding redactor.
/// Callers must reject encoded responses or decode them before this boundary.
/// The owner supplies only its already-known credentials; no external store.
pub(crate) struct KnownSecretGuard {
    patterns: Vec<Pattern>,
    pending: Zeroizing<Vec<u8>>,
    failed: bool,
}

fn blocked() -> io::Error {
    io::Error::other("managed_response.unsafe_body")
}

impl KnownSecretGuard {
    pub(crate) fn new(secrets: &[&[u8]]) -> Result<Self, io::Error> {
        if secrets.is_empty() || secrets.len() > MAX_SECRETS {
            return Err(blocked());
        }
        let mut patterns = Vec::with_capacity(secrets.len());
        for secret in secrets {
            if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
                return Err(blocked());
            }
            let mut fallback = vec![0; secret.len()];
            for index in 1..secret.len() {
                let mut prefix = fallback[index - 1];
                while prefix > 0 && secret[index] != secret[prefix] {
                    prefix = fallback[prefix - 1];
                }
                if secret[index] == secret[prefix] {
                    prefix += 1;
                }
                fallback[index] = prefix;
            }
            patterns.push(Pattern {
                bytes: Zeroizing::new(secret.to_vec()),
                fallback,
                matched: 0,
            });
        }
        Ok(Self {
            patterns,
            pending: Zeroizing::new(Vec::new()),
            failed: false,
        })
    }

    pub(crate) fn feed(&mut self, bytes: Bytes) -> Result<Option<Bytes>, io::Error> {
        if self.failed {
            return Err(blocked());
        }
        // Each byte updates one KMP state per bounded known secret. A complete
        // match is rejected before any portion of THIS chunk is released.
        for byte in bytes.iter() {
            for pattern in &mut self.patterns {
                while pattern.matched > 0 && *byte != pattern.bytes[pattern.matched] {
                    pattern.matched = pattern.fallback[pattern.matched - 1];
                }
                if *byte == pattern.bytes[pattern.matched] {
                    pattern.matched += 1;
                }
                if pattern.matched == pattern.bytes.len() {
                    self.failed = true;
                    self.pending.clear();
                    return Err(blocked());
                }
            }
        }
        // Only a suffix that is a prefix of a known secret can match a later
        // chunk. Keep its longest length, strictly less than MAX_SECRET_BYTES.
        let keep = self.patterns.iter().map(|p| p.matched).max().unwrap_or(0);
        if self.pending.is_empty() {
            let emit = bytes.len() - keep;
            self.pending.extend_from_slice(&bytes[emit..]);
            return Ok((emit > 0).then(|| bytes.slice(..emit)));
        }
        // At most one incoming chunk plus the bounded held suffix is copied;
        // the response is never accumulated across chunks.
        let mut joined = std::mem::take(&mut *self.pending);
        joined.extend_from_slice(&bytes);
        *self.pending = joined.split_off(joined.len() - keep);
        Ok((!joined.is_empty()).then(|| Bytes::from(joined)))
    }

    /// Whether a logical stream suffix is still withheld as a secret prefix.
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Consume only at a successful logical EOF. On an upstream error, drop the
    /// guard instead. A detected match never releases its held suffix.
    pub(crate) fn finish(mut self) -> Option<Bytes> {
        if self.failed {
            return None;
        }
        let tail = std::mem::take(&mut *self.pending);
        (!tail.is_empty()).then(|| Bytes::from(tail))
    }

    pub(crate) fn wrap_stream<S, E>(
        self,
        stream: S,
    ) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Send + 'static,
    {
        futures::stream::unfold(
            (self, Box::pin(stream), false),
            |(mut guard, mut stream, stopped)| async move {
                if stopped {
                    return None;
                }
                loop {
                    match stream.next().await {
                        Some(Ok(bytes)) => match guard.feed(bytes) {
                            Ok(Some(bytes)) => return Some((Ok(bytes), (guard, stream, false))),
                            Ok(None) => continue,
                            Err(error) => return Some((Err(error), (guard, stream, true))),
                        },
                        Some(Err(_)) => {
                            return Some((
                                Err(io::Error::other("managed_response.stream_interrupted")),
                                (guard, stream, true),
                            ))
                        }
                        None => {
                            let tail = std::mem::take(&mut *guard.pending);
                            return (!tail.is_empty())
                                .then(|| (Ok(Bytes::from(tail)), (guard, stream, true)));
                        }
                    }
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_api_flushes_only_success_and_poisoned_guard_stays_closed() {
        let mut normal = KnownSecretGuard::new(&[b"token"]).unwrap();
        assert_eq!(
            normal
                .feed(Bytes::from_static(b"safe tok"))
                .unwrap()
                .unwrap(),
            "safe "
        );
        assert_eq!(normal.finish().unwrap(), "tok");
        let mut denied = KnownSecretGuard::new(&[b"token"]).unwrap();
        assert!(denied.feed(Bytes::from_static(b"tok")).unwrap().is_none());
        assert!(denied.feed(Bytes::from_static(b"en")).is_err());
        assert!(denied.feed(Bytes::from_static(b"later")).is_err());
        assert!(denied.finish().is_none());
    }

    #[test]
    fn rejects_unbounded_or_empty_configuration() {
        assert!(KnownSecretGuard::new(&[]).is_err());
        assert!(KnownSecretGuard::new(&[b""]).is_err());
        assert!(KnownSecretGuard::new(&[b"a", b"b", b"c", b"d", b"e"]).is_err());
        assert!(KnownSecretGuard::new(&[&vec![b'a'; MAX_SECRET_BYTES + 1]]).is_err());
    }

    #[test]
    fn repeated_prefix_and_every_split_match_reference() {
        // Exhaustive small inputs include overlapping KMP prefixes and distinct
        // secrets sharing prefixes. Compare all two-chunk splits to plain search.
        let secrets: [&[u8]; 3] = [b"ababa", b"abb", b"bbaa"];
        for length in 0u32..=7 {
            for mut code in 0..3usize.pow(length) {
                let data: Vec<u8> = (0..length)
                    .map(|_| {
                        let byte = b"abc"[code % 3];
                        code /= 3;
                        byte
                    })
                    .collect();
                let expected = secrets
                    .iter()
                    .any(|secret| data.windows(secret.len()).any(|w| w == *secret));
                for split in 0..=data.len() {
                    let mut guard = KnownSecretGuard::new(&secrets).unwrap();
                    let mut output = Vec::new();
                    let mut blocked = false;
                    for chunk in [&data[..split], &data[split..]] {
                        match guard.feed(Bytes::copy_from_slice(chunk)) {
                            Ok(Some(bytes)) => output.extend_from_slice(&bytes),
                            Ok(None) => {}
                            Err(_) => {
                                blocked = true;
                                break;
                            }
                        }
                        assert!(guard.pending.len() < 5);
                    }
                    assert_eq!(blocked, expected);
                    if !blocked {
                        output.extend_from_slice(&guard.pending);
                        assert_eq!(output, data);
                    } else {
                        assert!(!secrets
                            .iter()
                            .any(|secret| output.windows(secret.len()).any(|w| w == *secret)));
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn normal_stream_is_incremental_and_eof_flushes_only_tail() {
        let input = futures::stream::iter([
            Ok::<_, io::Error>(Bytes::from_static(b"normal ")),
            Ok(Bytes::from_static(b"tok")),
        ]);
        let stream = KnownSecretGuard::new(&[b"token"])
            .unwrap()
            .wrap_stream(input);
        futures::pin_mut!(stream);
        assert_eq!(stream.next().await.unwrap().unwrap(), "normal ");
        assert_eq!(stream.next().await.unwrap().unwrap(), "tok");
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn match_and_source_error_are_terminal_without_pending_or_error_echo() {
        for tail in [
            Ok(Bytes::from_static(b"en")),
            Err(io::Error::other("token")),
        ] {
            let input = futures::stream::iter([
                Ok(Bytes::from_static(b"safe tok")),
                tail,
                Ok(Bytes::from_static(b"ignored")),
            ]);
            let stream = KnownSecretGuard::new(&[b"token"])
                .unwrap()
                .wrap_stream(input);
            futures::pin_mut!(stream);
            assert_eq!(stream.next().await.unwrap().unwrap(), "safe ");
            assert!(!stream
                .next()
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("token"));
            assert!(stream.next().await.is_none());
        }
    }
}
