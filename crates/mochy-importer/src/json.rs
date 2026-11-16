//! Lenient readers for third-party JSON (Android `JsonHelpers.kt`): a missing
//! or differently-typed field is None, never a panic.

use serde_json::Value;

pub fn path<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for k in keys {
        cur = cur.as_object()?.get(*k)?;
    }
    Some(cur)
}

/// A non-blank string field.
pub fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().filter(|s| !s.trim().is_empty()).map(str::to_string)
}

pub fn long(v: &Value, key: &str) -> Option<i64> {
    // Kotlin `longOrNull`: an integer, or a string holding one.
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A number or numeric string, as ids sometimes come either way.
pub fn id_string(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) if !s.trim().is_empty() && s != "null" => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub fn array(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// Days from 1970-01-01 to the civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A timestamp as services send them (ISO with a Z, `+02:00`, or Tidal's
/// `+0000`), in epoch ms. None for anything else.
pub fn date_ms(text: Option<&str>) -> Option<i64> {
    let t = text?.trim();
    if t.len() < 19 {
        return None;
    }
    let b = t.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> { t.get(r)?.parse().ok() };
    if b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, s) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut rest = &t[19..];
    let mut ms = 0i64;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        let first3: String = digits.chars().chain("000".chars()).take(3).collect();
        ms = first3.parse().ok()?;
        rest = &frac[digits.len()..];
    }
    let offset_secs = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let o = rest[1..].replace(':', "");
            if o.len() != 4 || !o.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let oh: i64 = o[..2].parse().ok()?;
            let om: i64 = o[2..].parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s - offset_secs;
    Some(secs * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of `DateMsTest`.
    #[test]
    fn reads_each_services_timestamps() {
        // Spotify and Apple: ISO with a Z
        assert_eq!(date_ms(Some("2024-06-06T22:39:08Z")), Some(1_717_713_548_000));
        // Tidal: milliseconds and an offset without a colon
        assert_eq!(date_ms(Some("2020-09-14T17:29:43.587+0000")), Some(1_600_104_583_587));
        assert_eq!(date_ms(Some("2020-09-14T19:29:43+02:00")), Some(1_600_104_583_000));
    }

    #[test]
    fn missing_or_odd_is_null() {
        assert_eq!(date_ms(None), None);
        assert_eq!(date_ms(Some("")), None);
        assert_eq!(date_ms(Some("yesterday")), None);
        assert_eq!(date_ms(Some("2020-09-14T19:29:43")), None);
    }

    #[test]
    fn lenient_readers() {
        let v: Value = serde_json::json!({"a": {"b": " "}, "n": 5, "f": 2.0, "id": 12, "s": "x", "arr": [1]});
        assert_eq!(string(&v["a"], "b"), None);
        assert_eq!(string(&v, "s").as_deref(), Some("x"));
        assert_eq!(long(&v, "n"), Some(5));
        assert_eq!(long(&v, "f"), None);
        assert_eq!(long(&v, "s"), None);
        assert_eq!(long(&serde_json::json!({"d": "201000"}), "d"), Some(201_000));
        assert_eq!(id_string(&v, "id").as_deref(), Some("12"));
        assert_eq!(array(v.get("arr")).len(), 1);
        assert_eq!(array(v.get("nope")).len(), 0);
        assert!(path(&v, &["a", "b"]).is_some());
        assert!(path(&v, &["a", "c"]).is_none());
    }
}
