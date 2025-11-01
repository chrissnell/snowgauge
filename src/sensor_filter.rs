/// MB7544-compatible exponential weighted average filter
///
/// This filter mimics the MB7544's internal filtering mechanism:
/// - Recent-biased exponential weighted average
/// - Rate limited to 1mm maximum change per reading
/// - 40-reading initialization period for stabilization
use log::debug;

/// Filter type selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterType {
    /// No filtering - pass through raw readings
    None,
    /// Exponential weighted average with rate limiting (MB7544-compatible)
    Exponential,
    /// Collect batch and use trimmed mean (discard outliers)
    TrimmedMean,
    /// Apply both exponential filtering per-reading AND trimmed mean on batch
    Both,
}

impl std::str::FromStr for FilterType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(FilterType::None),
            "exponential" | "exp" | "ema" => Ok(FilterType::Exponential),
            "trimmed" | "trimmed-mean" | "trimmedmean" => Ok(FilterType::TrimmedMean),
            "both" | "combined" => Ok(FilterType::Both),
            _ => Err(format!(
                "Invalid filter type '{}'. Valid options: none, exponential, trimmed-mean, both",
                s
            )),
        }
    }
}

impl std::fmt::Display for FilterType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterType::None => write!(f, "none"),
            FilterType::Exponential => write!(f, "exponential"),
            FilterType::TrimmedMean => write!(f, "trimmed-mean"),
            FilterType::Both => write!(f, "both"),
        }
    }
}

pub struct SensorFilter {
    /// Current filtered value (in mm)
    filtered_value: Option<f64>,

    /// Number of readings processed (for initialization period)
    reading_count: usize,

    /// Initialization period (default 40 readings as per MB7544 spec)
    init_period: usize,

    /// Maximum change per reading in mm (default 1.0mm as per MB7544 spec)
    max_rate_limit_mm: f64,

    /// Smoothing factor (alpha) for exponential weighted average
    /// Higher alpha = more weight to recent readings (typical range 0.1-0.3)
    alpha: f64,
}

impl SensorFilter {
    /// Create a new sensor filter with MB7544 defaults
    pub fn new() -> Self {
        Self::with_params(40, 1.0, 0.2)
    }

    /// Create a new sensor filter with custom parameters
    ///
    /// # Arguments
    /// * `init_period` - Number of readings before filter is considered stable
    /// * `max_rate_limit_mm` - Maximum change allowed per reading (mm)
    /// * `alpha` - Smoothing factor (0.0-1.0), higher = more responsive to changes
    pub fn with_params(init_period: usize, max_rate_limit_mm: f64, alpha: f64) -> Self {
        Self {
            filtered_value: None,
            reading_count: 0,
            init_period,
            max_rate_limit_mm,
            alpha: alpha.clamp(0.0, 1.0),
        }
    }

    /// Process a new sensor reading through the filter
    ///
    /// Returns the filtered value. During the initialization period,
    /// the filter builds up its state and may return less stable values.
    pub fn update(&mut self, raw_reading: f64) -> f64 {
        self.reading_count += 1;

        match self.filtered_value {
            None => {
                // First reading - initialize with raw value
                self.filtered_value = Some(raw_reading);
                debug!("Filter initialized with first reading: {:.2}mm", raw_reading);
                raw_reading
            }
            Some(current) => {
                // Apply exponential weighted average
                let ema_value = self.alpha * raw_reading + (1.0 - self.alpha) * current;

                // Apply rate limiting (1mm max change per reading)
                let delta = ema_value - current;
                let limited_delta = delta.clamp(-self.max_rate_limit_mm, self.max_rate_limit_mm);
                let new_value = current + limited_delta;

                if self.reading_count <= self.init_period {
                    debug!(
                        "Filter initializing ({}/{}): raw={:.2}mm, ema={:.2}mm, rate_limited={:.2}mm",
                        self.reading_count, self.init_period, raw_reading, ema_value, new_value
                    );
                } else if (delta - limited_delta).abs() > 0.001 {
                    debug!(
                        "Rate limit applied: raw={:.2}mm, ema={:.2}mm, delta={:.2}mm, limited={:.2}mm, final={:.2}mm",
                        raw_reading, ema_value, delta, limited_delta, new_value
                    );
                }

                self.filtered_value = Some(new_value);
                new_value
            }
        }
    }

    #[cfg(test)]
    /// Reset the filter (equivalent to bringing RX pin low on MB7544)
    pub fn reset(&mut self) {
        debug!("Filter reset");
        self.filtered_value = None;
        self.reading_count = 0;
    }

    #[cfg(test)]
    /// Check if the filter has completed its initialization period
    pub fn is_initialized(&self) -> bool {
        self.reading_count >= self.init_period
    }

    #[cfg(test)]
    /// Get the current filtered value if available
    pub fn current_value(&self) -> Option<f64> {
        self.filtered_value
    }

    /// Get the number of readings processed
    pub fn reading_count(&self) -> usize {
        self.reading_count
    }
}

impl Default for SensorFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_initialization() {
        let mut filter = SensorFilter::new();
        assert_eq!(filter.is_initialized(), false);

        // Process first reading
        let result = filter.update(1000.0);
        assert_eq!(result, 1000.0);
        assert_eq!(filter.current_value(), Some(1000.0));
    }

    #[test]
    fn test_rate_limiting() {
        let mut filter = SensorFilter::with_params(1, 1.0, 1.0); // alpha=1.0 means no smoothing

        filter.update(1000.0);

        // Try to jump 10mm - should be limited to 1mm
        let result = filter.update(1010.0);
        assert_eq!(result, 1001.0);

        // Try to drop 10mm - should be limited to -1mm
        let result = filter.update(990.0);
        assert_eq!(result, 1000.0);
    }

    #[test]
    fn test_exponential_smoothing() {
        let mut filter = SensorFilter::with_params(1, 10.0, 0.2); // No rate limiting for this test

        filter.update(1000.0);

        // With alpha=0.2, new value should be: 0.2 * 1005 + 0.8 * 1000 = 1001
        let result = filter.update(1005.0);
        assert!((result - 1001.0).abs() < 0.01);
    }

    #[test]
    fn test_reset() {
        let mut filter = SensorFilter::new();

        filter.update(1000.0);
        filter.update(1001.0);
        assert!(filter.current_value().is_some());

        filter.reset();
        assert_eq!(filter.current_value(), None);
        assert_eq!(filter.reading_count(), 0);
    }

    #[test]
    fn test_initialization_period() {
        let mut filter = SensorFilter::with_params(5, 1.0, 0.2);

        for i in 0..4 {
            filter.update(1000.0);
            assert_eq!(filter.is_initialized(), false, "Should not be initialized at reading {}", i + 1);
        }

        filter.update(1000.0);
        assert_eq!(filter.is_initialized(), true, "Should be initialized at reading 5");

        filter.update(1000.0);
        assert_eq!(filter.is_initialized(), true, "Should remain initialized after reading 6");
    }

    #[test]
    fn test_realistic_scenario() {
        let mut filter = SensorFilter::new();

        // Simulate noisy readings around 1000mm
        let noisy_readings = vec![
            1000.0, 1002.0, 998.0, 1001.0, 999.0,
            1000.5, 1001.5, 999.5, 1000.2, 1000.8,
        ];

        for reading in noisy_readings {
            let filtered = filter.update(reading);
            // Filtered value should be smoother than raw readings
            println!("Raw: {:.2}, Filtered: {:.2}", reading, filtered);
        }

        // After filtering, the value should be close to 1000mm
        let final_value = filter.current_value().unwrap();
        assert!((final_value - 1000.0).abs() < 2.0, "Filtered value should be close to 1000mm");
    }
}
