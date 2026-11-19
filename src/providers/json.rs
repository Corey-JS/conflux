//! Lenient JSON helpers. These web-app endpoints are undocumented and change
//! shape now and then, so parsers read through `Value` and tolerate missing
//! or renamed fields instead of failing the whole catalog.

use serde_json::Value;

/// Follow a path of object keys.
pub fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    Some(cur)
}

/// First non-empty string among several candidate keys.
pub fn str_of(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        match v.get(k) {
            Some(Value::String(s)) if !s.trim().is_empty() => return Some(s.trim().to_string()),
            Some(Value::Number(n)) => return Some(n.to_string()),
            _ => {}
        }
    }
    None
}

/// A number that may arrive as a number or a numeric string.
pub fn u32_of(v: &Value, keys: &[&str]) -> Option<u32> {
    for k in keys {
        match v.get(k) {
            Some(Value::Number(n)) => {
                if let Some(x) = n.as_u64() {
                    return u32::try_from(x).ok();
                }
            }
            Some(Value::String(s)) => {
                if let Ok(x) = s.trim().parse::<u32>() {
                    return Some(x);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn year_of(v: &Value, keys: &[&str]) -> Option<u16> {
    u32_of(v, keys)
        .and_then(|y| u16::try_from(y).ok())
        .filter(|y| (1880..=2100).contains(y))
}

/// The array at `path`, or an empty slice.
pub fn arr<'a>(v: &'a Value, path: &[&str]) -> &'a [Value] {
    at(v, path)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Strings from an array that may hold strings or objects with a name.
pub fn strings(v: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(a)) = v else {
        return match v {
            Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
            _ => Vec::new(),
        };
    };
    a.iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.trim().to_string()),
            Value::Object(_) => str_of(x, &["tag", "name", "title"]),
            _ => None,
        })
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lenient_numbers() {
        let v = json!({"a": "12", "b": 7, "c": "x"});
        assert_eq!(u32_of(&v, &["a"]), Some(12));
        assert_eq!(u32_of(&v, &["c", "b"]), Some(7));
        assert_eq!(year_of(&json!({"y": 3}), &["y"]), None);
    }

    #[test]
    fn strings_mixed() {
        let v = json!(["Drama", {"tag": "Comedy"}, "", 3]);
        assert_eq!(strings(Some(&v)), vec!["Drama", "Comedy"]);
        assert_eq!(strings(Some(&json!("News"))), vec!["News"]);
    }
}
