//! Percentile collectors for the latency and frame-time instrumentation.

/// A growable bag of millisecond samples that computes order statistics on
/// demand. Kept deliberately simple: the spike collects a few hundred to a few
/// thousand samples, so a sort-per-query is cheap and avoids a dependency.
#[derive(Default)]
pub struct Samples {
    values: Vec<f64>,
}

impl Samples {
    pub fn new() -> Self {
        Self { values: Vec::new() }
    }

    pub fn push(&mut self, value_ms: f64) {
        self.values.push(value_ms);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Nearest-rank percentile in `[0, 100]`. Returns `0.0` when empty so the
    /// JSON summary always has a numeric field.
    pub fn percentile(&self, p: f64) -> f64 {
        if self.values.is_empty() {
            return 0.0;
        }
        let mut sorted = self.values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let rank = (p / 100.0 * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[rank.min(sorted.len() - 1)]
    }

    pub fn max(&self) -> f64 {
        self.values
            .iter()
            .copied()
            .fold(0.0, |acc, v| if v > acc { v } else { acc })
    }
}
