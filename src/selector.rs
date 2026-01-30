use thiserror::Error;

use crate::targets::Target;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSelector {
    Index(usize),
    Id(String),
}

#[derive(Error, Debug)]
pub enum SelectorError {
    #[error("invalid selector: {0}")]
    InvalidSelector(String),

    #[error("index out of range: {index} (have {len})")]
    IndexOutOfRange { index: usize, len: usize },

    #[error("no target matched: {selector}")]
    NoMatch { selector: String },

    #[error("multiple targets matched: {selector}")]
    MultipleMatches { selector: String },
}

pub fn parse_selector(s: &str) -> Result<TargetSelector, SelectorError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(SelectorError::InvalidSelector("empty".to_string()));
    }

    if let Some(rest) = s.strip_prefix("index:") {
        let rest = rest.trim();
        let idx: usize = rest
            .parse()
            .map_err(|_| SelectorError::InvalidSelector(format!("invalid index: {rest}")))?;
        return Ok(TargetSelector::Index(idx));
    }

    if s.starts_with("serial:") || s.starts_with("halfkay:") {
        return Ok(TargetSelector::Id(s.to_string()));
    }

    // Bare digits => treat as list index.
    if s.chars().all(|c| c.is_ascii_digit()) {
        let idx: usize = s
            .parse()
            .map_err(|_| SelectorError::InvalidSelector(format!("invalid index: {s}")))?;
        return Ok(TargetSelector::Index(idx));
    }

    // Bare selector => treat as serial port name.
    Ok(TargetSelector::Id(format!("serial:{s}")))
}

pub fn resolve(selector: &TargetSelector, targets: &[Target]) -> Result<Vec<usize>, SelectorError> {
    match selector {
        TargetSelector::Index(i) => {
            if *i >= targets.len() {
                return Err(SelectorError::IndexOutOfRange {
                    index: *i,
                    len: targets.len(),
                });
            }
            Ok(vec![*i])
        }
        TargetSelector::Id(id) => Ok(targets
            .iter()
            .enumerate()
            .filter_map(|(i, t)| if t.id() == *id { Some(i) } else { None })
            .collect()),
    }
}

pub fn resolve_one(selector: &TargetSelector, targets: &[Target]) -> Result<usize, SelectorError> {
    let matches = resolve(selector, targets)?;
    if matches.is_empty() {
        return Err(SelectorError::NoMatch {
            selector: selector_string(selector),
        });
    }
    if matches.len() > 1 {
        return Err(SelectorError::MultipleMatches {
            selector: selector_string(selector),
        });
    }
    Ok(matches[0])
}

fn selector_string(s: &TargetSelector) -> String {
    match s {
        TargetSelector::Index(i) => format!("index:{i}"),
        TargetSelector::Id(id) => id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::targets::{HalfKayTarget, SerialTarget, Target};

    #[test]
    fn test_parse_selector() {
        assert_eq!(parse_selector("index:0").unwrap(), TargetSelector::Index(0));
        assert_eq!(
            parse_selector("serial:COM6").unwrap(),
            TargetSelector::Id("serial:COM6".to_string())
        );
        assert_eq!(
            parse_selector("COM6").unwrap(),
            TargetSelector::Id("serial:COM6".to_string())
        );
    }

    #[test]
    fn test_resolve_one_by_id() {
        let targets = vec![
            Target::HalfKay(HalfKayTarget {
                vid: 0x16C0,
                pid: 0x0478,
                path: "HK1".to_string(),
            }),
            Target::Serial(SerialTarget {
                port_name: "COM6".to_string(),
                vid: 0x16C0,
                pid: 0x0489,
                serial_number: None,
                manufacturer: None,
                product: None,
            }),
        ];

        let sel = parse_selector("serial:COM6").unwrap();
        let idx = resolve_one(&sel, &targets).unwrap();
        assert_eq!(idx, 1);
    }
}
