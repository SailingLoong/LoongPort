//! Conditional observation of an actual authenticated HTTPS response's Date.
//! RFC 9110 6.6.1 / 5.6.7. No local UTC, claimedAt or window-derived fallback.
use super::VerifiedContext;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub(crate) struct Observation {
    subject: String,
    generation: String,
    seconds: i64,
    received: Instant,
    request_elapsed: Duration,
}
impl Observation {
    pub(crate) fn for_context(&self, context: &VerifiedContext) -> Option<i64> {
        let elapsed = self.received.elapsed();
        (self.subject == context.identity.uid
            && self.generation == context.generation
            && elapsed.checked_add(self.request_elapsed)? <= Duration::from_secs(30))
        .then(|| self.seconds.checked_add(elapsed.as_secs() as i64))
        .flatten()
    }
    /// Date has whole-second precision. Include measured request duration and
    /// elapsed monotonic time as a conservative upper bound before dispatch.
    pub(crate) fn window_open(&self, context: &VerifiedContext, start: i64, end: i64) -> bool {
        let Some(lower) = self.for_context(context) else {
            return false;
        };
        let Some(total) = self.received.elapsed().checked_add(self.request_elapsed) else {
            return false;
        };
        let upper = self.seconds.checked_add(1).and_then(|n| {
            n.checked_add(total.as_secs() as i64 + i64::from(total.subsec_nanos() != 0))
        });
        lower >= start && upper.is_some_and(|upper| upper < end)
    }
}
/// Consume metadata from the retained HTTP response in the verified request context.
/// The adapter may read its bounded JSON body through chunks without discarding it.
/// The campaign transport must disable redirects/retries and use normal TLS validation.
/// This function does not prove that Qoder actually supplies Date on any endpoint.
pub(crate) fn from_response(
    response: &reqwest::Response,
    context: &VerifiedContext,
    request_started: Instant,
) -> Option<Observation> {
    let request_elapsed = request_started.elapsed();
    if request_elapsed > Duration::from_secs(30) {
        return None;
    }
    if !expected_origin(response, context) {
        return None;
    }
    let headers = response.headers();
    if headers.get_all(reqwest::header::DATE).iter().count() != 1
        || headers.get_all(reqwest::header::AGE).iter().count() > 1
        || headers.contains_key(reqwest::header::VIA)
    {
        return None;
    }
    let date = headers.get(reqwest::header::DATE)?.to_str().ok()?;
    let age = match headers.get(reqwest::header::AGE) {
        Some(value) => Some(value.to_str().ok()?),
        None => None,
    };
    let mut observed = parse_date(date, age, context)?;
    observed.request_elapsed = request_elapsed;
    Some(observed)
}
/// Response URL metadata is an origin check, not an independent TLS or identity proof.
/// The authenticated transport still owns normal TLS validation and disabled redirects.
pub(crate) fn expected_origin(response: &reqwest::Response, context: &VerifiedContext) -> bool {
    let url = response.url();
    // Production context can only originate in HttpAdapter's fixed HTTPS origin.
    // The loopback origin constructor is compiled exclusively for offline tests.
    url.origin() == context.origin.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && !response.status().is_redirection()
}

fn parse_date(date: &str, age: Option<&str>, context: &VerifiedContext) -> Option<Observation> {
    if age.is_some_and(|v| v.parse::<u64>().ok() != Some(0))
        || date.len() != 29
        || !date.ends_with(" GMT")
    {
        return None;
    }
    let parsed = chrono::DateTime::parse_from_rfc2822(date).ok()?;
    // Strict IMF-fixdate round trip also rejects contradictory weekday/zone data.
    if parsed.format("%a, %d %b %Y %H:%M:%S GMT").to_string() != date || parsed.timestamp() < 0 {
        return None;
    }
    Some(Observation {
        subject: context.identity.uid.clone(),
        generation: context.generation.clone(),
        seconds: parsed.timestamp(),
        received: Instant::now(),
        request_elapsed: Duration::ZERO,
    })
}
#[cfg(test)]
pub(crate) fn synthetic(
    date: &str,
    age: Option<&str>,
    context: &VerifiedContext,
) -> Option<Observation> {
    parse_date(date, age, context)
}

#[cfg(test)]
pub(crate) fn synthetic_elapsed(mut observation: Observation, elapsed: Duration) -> Observation {
    observation.received = Instant::now().checked_sub(elapsed).unwrap();
    observation
}
