//! Colour-coded file age (F-05): a compact label plus a 0..1 "heat" value.
//! The UI maps heat onto a hue ramp (hot = recent, cold = old).

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Age {
    pub label: String,
    /// 1.0 = just modified, 0.0 = five years or older (log scale).
    pub heat: f32,
}

const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

pub fn age(modified_ms: i64, now_ms: i64) -> Age {
    let d = (now_ms - modified_ms).max(0);
    let label = if d < HOUR {
        format!("{}'", (d / MIN).max(1))
    } else if d < DAY {
        format!("{}h{}'", d / HOUR, (d % HOUR) / MIN)
    } else if d < MONTH {
        format!("{}d", d / DAY)
    } else if d < YEAR {
        format!("{}M", d / MONTH)
    } else {
        format!("{}Y", d / YEAR)
    };
    let max = (5 * YEAR) as f64;
    let heat = 1.0 - ((d as f64 / MIN as f64 + 1.0).ln() / (max / MIN as f64 + 1.0).ln()).min(1.0);
    Age {
        label,
        heat: heat as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        let now = 10 * YEAR;
        assert_eq!(age(now - 5 * MIN, now).label, "5'");
        assert_eq!(age(now - (HOUR + 10 * MIN), now).label, "1h10'");
        assert_eq!(age(now - 2 * DAY, now).label, "2d");
        assert_eq!(age(now - 6 * MONTH, now).label, "6M");
        assert_eq!(age(now - 5 * YEAR, now).label, "5Y");
        assert_eq!(age(now + DAY, now).label, "1'", "future clamps to now");
    }

    #[test]
    fn heat_monotonic() {
        let now = 10 * YEAR;
        let hs: Vec<f32> = [MIN, HOUR, DAY, MONTH, YEAR, 5 * YEAR]
            .iter()
            .map(|d| age(now - d, now).heat)
            .collect();
        assert!(hs.windows(2).all(|w| w[0] > w[1]));
        assert!(hs[5] <= 0.001);
    }
}
