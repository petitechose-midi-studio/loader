use std::collections::HashSet;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::halfkay;

#[derive(Error, Debug)]
pub enum WaitHalfKayError {
    #[error("HalfKay list failed: {0}")]
    ListFailed(#[from] halfkay::HalfKayError),

    #[error("multiple new HalfKay devices appeared ({count})")]
    Ambiguous { count: usize },

    #[error("HalfKay did not appear after soft reboot")]
    Timeout,
}

pub fn wait_for_new_halfkay(
    before: &HashSet<String>,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<String, WaitHalfKayError> {
    let start = Instant::now();
    loop {
        let now = halfkay::list_paths()?;
        if let Some(p) = diff_new_halfkay(before, &now)? {
            return Ok(p);
        }

        if start.elapsed() >= timeout {
            return Err(WaitHalfKayError::Timeout);
        }
        std::thread::sleep(poll_interval);
    }
}

pub fn diff_new_halfkay(
    before: &HashSet<String>,
    now: &[String],
) -> Result<Option<String>, WaitHalfKayError> {
    let mut new: Vec<String> = now
        .iter()
        .filter(|p| !before.contains(*p))
        .cloned()
        .collect();
    new.sort();

    if new.len() == 1 {
        return Ok(Some(new.remove(0)));
    }
    if new.len() > 1 {
        return Err(WaitHalfKayError::Ambiguous { count: new.len() });
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_new_halfkay() {
        let mut before = HashSet::new();
        before.insert("A".to_string());

        let now = vec!["A".to_string(), "B".to_string()];
        assert_eq!(
            diff_new_halfkay(&before, &now).unwrap(),
            Some("B".to_string())
        );

        let now2 = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let err = diff_new_halfkay(&before, &now2).unwrap_err();
        assert!(matches!(err, WaitHalfKayError::Ambiguous { count: 2 }));
    }
}
