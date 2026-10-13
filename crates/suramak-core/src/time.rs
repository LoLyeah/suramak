/// Formats milliseconds as `HH:MM:SS.mmm`.
pub fn fmt_ms(ms: i64) -> String {
    let total_s = ms / 1000;
    let millis = ms % 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    format!("{h:02}:{m:02}:{s:02}.{millis:03}")
}

/// Formats milliseconds as SRT timestamp `HH:MM:SS,mmm`.
pub fn fmt_srt(ms: i64) -> String {
    let total_s = ms / 1000;
    let millis = ms % 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    format!("{h:02}:{m:02}:{s:02},{millis:03}")
}

/// Formats milliseconds as VTT timestamp `HH:MM:SS.mmm`.
pub fn fmt_vtt(ms: i64) -> String {
    fmt_ms(ms)
}

/// Formats milliseconds as plain `HH:MM:SS` (no sub-second).
pub fn fmt_plain(ms: i64) -> String {
    let total_s = ms / 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Parses `HH:MM:SS.mmm` → milliseconds.
pub fn parse_ms(s: &str) -> Option<i64> {
    let (hms, millis_str) = s.split_once('.')?;
    let millis: i64 = millis_str.parse().ok()?;
    let parts: Vec<&str> = hms.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: i64 = parts[0].parse().ok()?;
    let m: i64 = parts[1].parse().ok()?;
    let sec: i64 = parts[2].parse().ok()?;
    Some((h * 3600 + m * 60 + sec) * 1000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_zero() {
        assert_eq!(fmt_ms(0), "00:00:00.000");
    }

    #[test]
    fn roundtrip_sample() {
        assert_eq!(fmt_ms(3_723_456), "01:02:03.456");
        assert_eq!(parse_ms("01:02:03.456"), Some(3_723_456));
    }

    #[test]
    fn srt_comma() {
        assert_eq!(fmt_srt(2400), "00:00:02,400");
    }

    #[test]
    fn vtt_dot() {
        assert_eq!(fmt_vtt(2400), "00:00:02.400");
    }

    #[test]
    fn plain_no_millis() {
        assert_eq!(fmt_plain(3_723_999), "01:02:03");
    }
}
