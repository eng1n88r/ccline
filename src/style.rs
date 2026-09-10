pub(crate) fn dim(s: &str) -> String {
    format!("\x1b[2m{s}\x1b[0m")
}

/// Green under 50%, yellow under 80%, red above.
pub(crate) fn pct_color(pct: f64) -> &'static str {
    if pct < 50.0 {
        "\x1b[32m"
    } else if pct < 80.0 {
        "\x1b[33m"
    } else {
        "\x1b[31m"
    }
}

pub(crate) fn fmt_tokens(n: f64) -> String {
    if n >= 1000.0 {
        format!("{:.1}k", n / 1000.0)
    } else {
        format!("{n:.0}")
    }
}
