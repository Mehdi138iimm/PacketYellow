use serde::Serialize;

/// Result of a ping run. Jitter = mean absolute difference of consecutive RTTs (RFC 3550 style).
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PingStats {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub ip: String,
    pub sent: u32,
    pub received: u32,
    pub loss_pct: f64,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub median_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub samples: Vec<Option<f64>>,
    pub note: Option<String>,
    pub error: Option<String>,
}

impl PingStats {
    pub fn failed(name: String, host: String, port: u16, err: String) -> Self {
        PingStats { name, host, port, loss_pct: 100.0, error: Some(err), ..Default::default() }
    }
}

pub fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

pub fn summarize(host: &str, port: u16, ip: String, samples: Vec<Option<f64>>) -> PingStats {
    let ok: Vec<f64> = samples.iter().flatten().copied().collect();
    let sent = samples.len() as u32;
    let received = ok.len() as u32;
    let loss_pct = if sent == 0 { 0.0 } else { (sent - received) as f64 / sent as f64 * 100.0 };

    let (min_ms, avg_ms, max_ms) = if ok.is_empty() {
        (None, None, None)
    } else {
        let min = ok.iter().copied().fold(f64::INFINITY, f64::min);
        let max = ok.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (Some(min), Some(ok.iter().sum::<f64>() / ok.len() as f64), Some(max))
    };
    let mut sorted = ok.clone();
    let median_ms = median(&mut sorted);

    let jitter_ms = if ok.len() < 2 {
        None
    } else {
        Some(ok.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (ok.len() - 1) as f64)
    };

    PingStats {
        name: host.to_string(),
        host: host.to_string(),
        port,
        ip,
        sent,
        received,
        loss_pct,
        min_ms,
        avg_ms,
        median_ms,
        max_ms,
        jitter_ms,
        samples,
        note: None,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stats_math() {
        let s = summarize("x", 1, "1.1.1.1".into(), vec![Some(10.0), None, Some(20.0), Some(14.0)]);
        assert_eq!(s.received, 3);
        assert!((s.loss_pct - 25.0).abs() < 1e-9);
        assert_eq!(s.min_ms, Some(10.0));
        assert_eq!(s.max_ms, Some(20.0));
        assert_eq!(s.median_ms, Some(14.0));
        assert!((s.jitter_ms.unwrap() - 8.0).abs() < 1e-9); // (10 + 6) / 2
    }
    #[test]
    fn median_even() {
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(&mut []), None);
    }
    #[test]
    fn ip_classes() {
        use crate::probe::{is_fake_ip, is_filter_ip};
        assert!(is_filter_ip(&"10.10.34.35".parse().unwrap()));
        assert!(!is_filter_ip(&"10.10.35.1".parse().unwrap()));
        assert!(is_fake_ip(&"198.18.0.7".parse().unwrap()));
        assert!(!is_fake_ip(&"8.8.8.8".parse().unwrap()));
    }
}
