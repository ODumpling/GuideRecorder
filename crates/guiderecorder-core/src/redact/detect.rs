//! Detectors for sensitive text: built-in formats, the user's own patterns
//! and word list. Text in, byte ranges out; see [`super`] for how the
//! ranges become blur regions.
//!
//! Each built-in format is a regular expression that finds candidates, most
//! followed by a check that keeps false positives down: Luhn for card
//! numbers, mod-97 for IBANs, a parse for IP addresses, digit counts and
//! grouping for phone numbers, the mix of characters for generic secrets. A
//! card or IBAN candidate that fails its check is tried again on its
//! separator-delimited parts, longest first ([`best_part`]), so a card
//! number followed by a year is still found.
//!
//! When findings overlap, the more specific kind wins (the [`Kind`] order).

use std::net::{Ipv4Addr, Ipv6Addr};
use std::ops::Range;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

/// Limits for the user's rules (settings).
pub const MAX_PATTERNS: usize = 50;
pub const MAX_PATTERN_LEN: usize = 500;
pub const MAX_WORDS: usize = 500;
pub const MAX_WORD_LEN: usize = 200;
/// Compiled size limit of one user pattern, so a pathological one cannot
/// take much memory or time to build.
const PATTERN_SIZE_LIMIT: usize = 1 << 20;
/// The same for the word list, which is one alternation of every word.
const WORDS_SIZE_LIMIT: usize = 16 << 20;

/// What a finding is. Most specific first: on overlap the earlier wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Matches one of the user's patterns.
    Pattern,
    /// On the user's word list.
    Word,
    PrivateKey,
    AwsKey,
    GithubToken,
    GitlabToken,
    SlackToken,
    StripeKey,
    GoogleApiKey,
    /// `sk-…` keys (Anthropic, OpenAI and others).
    ApiKey,
    Jwt,
    BearerToken,
    Email,
    Iban,
    CardNumber,
    IpAddress,
    /// A US social security number shape (`123-45-6789`).
    IdNumber,
    Phone,
    LongNumber,
    /// The value after `password:`, `token=` and the like.
    Credential,
    /// A long random-looking string: hex, or mixed-case letters and digits.
    Secret,
}

impl Kind {
    /// Why it was blurred, for the suggestion list (e.g. `email address`).
    pub fn reason(self) -> &'static str {
        match self {
            Kind::Pattern => "matches your pattern",
            Kind::Word => "on your word list",
            Kind::PrivateKey => "private key",
            Kind::AwsKey => "AWS access key",
            Kind::GithubToken => "GitHub token",
            Kind::GitlabToken => "GitLab token",
            Kind::SlackToken => "Slack token",
            Kind::StripeKey => "Stripe key",
            Kind::GoogleApiKey => "Google API key",
            Kind::ApiKey => "API key",
            Kind::Jwt => "access token (JWT)",
            Kind::BearerToken => "access token",
            Kind::Email => "email address",
            Kind::Iban => "IBAN",
            Kind::CardNumber => "card number",
            Kind::IpAddress => "IP address",
            Kind::IdNumber => "ID number",
            Kind::Phone => "phone number",
            Kind::LongNumber => "long number",
            Kind::Credential => "password or secret",
            Kind::Secret => "possible secret",
        }
    }
}

/// Sensitive text at `range` (bytes) of the text searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub range: Range<usize>,
    pub kind: Kind,
}

/// A user pattern or word that can't be used, with why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RulesError(pub String);

/// How a built-in candidate is checked.
#[derive(Debug, Clone, Copy)]
enum Check {
    Always,
    Card,
    Iban,
    Phone,
    Ipv4,
    Ipv6,
    Hex,
    Random,
}

struct Builtin {
    kind: Kind,
    /// A named group `v`, if any, is the part blurred.
    regex: Regex,
    check: Check,
}

static BUILTINS: LazyLock<Vec<Builtin>> = LazyLock::new(|| {
    let b = |kind, pattern: &str, check| Builtin {
        kind,
        regex: Regex::new(pattern).expect("built-in pattern"),
        check,
    };
    use Check::*;
    vec![
        b(
            Kind::PrivateKey,
            r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----",
            Always,
        ),
        b(
            Kind::AwsKey,
            r"\b(?:AKIA|ASIA|ABIA|ACCA|AGPA|AIDA|AIPA|ANPA|ANVA|APKA|AROA|ASCA)[A-Z0-9]{16}\b",
            Always,
        ),
        b(
            Kind::GithubToken,
            r"\b(?:gh[pousr]_[A-Za-z0-9]{36,255}|github_pat_[A-Za-z0-9_]{22,255})",
            Always,
        ),
        b(Kind::GitlabToken, r"\bglpat-[A-Za-z0-9_\-]{20,}", Always),
        b(
            Kind::SlackToken,
            r"\bxox[abposre]-[A-Za-z0-9\-]{10,}",
            Always,
        ),
        b(
            Kind::SlackToken,
            r"https://hooks\.slack\.com/services/[A-Za-z0-9_/]+",
            Always,
        ),
        b(
            Kind::StripeKey,
            r"\b(?:(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{10,}|whsec_[A-Za-z0-9]{20,})",
            Always,
        ),
        b(Kind::GoogleApiKey, r"\bAIza[0-9A-Za-z_\-]{35}", Always),
        b(
            Kind::ApiKey,
            r"\bsk-(?:ant-|proj-|svcacct-)?[A-Za-z0-9_\-]{20,}",
            Always,
        ),
        b(
            Kind::Jwt,
            r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
            Always,
        ),
        b(
            Kind::BearerToken,
            r"(?i)\bbearer\s+(?P<v>[A-Za-z0-9._~+/\-]{16,}=*)",
            Always,
        ),
        b(
            Kind::Email,
            r"\b\w[\w.%+\-]*@[\p{L}\p{N}](?:[\p{L}\p{N}\-]*[\p{L}\p{N}])?(?:\.[\p{L}\p{N}](?:[\p{L}\p{N}\-]*[\p{L}\p{N}])?)*\.\p{L}{2,24}\b",
            Always,
        ),
        b(
            Kind::Iban,
            r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){2,7}(?: ?[A-Z0-9]{1,4})?\b",
            Iban,
        ),
        b(Kind::CardNumber, r"\b\d(?:[ \-]?\d){12,22}\b", Card),
        b(Kind::IpAddress, r"\b(?:\d{1,3}\.){3}\d{1,3}\b", Ipv4),
        b(
            Kind::IpAddress,
            r"(?i)[0-9a-f]{0,4}(?::[0-9a-f]{0,4}){2,7}(?:\.\d{1,3}){0,3}",
            Ipv6,
        ),
        b(Kind::IdNumber, r"\b\d{3}-\d{2}-\d{4}\b", Always),
        b(Kind::Phone, r"[+(]?\d[\d ().\-]{5,22}\d", Phone),
        b(
            Kind::LongNumber,
            r"\b\d{9,}\b|\b\d{4}(?:[ \-]\d{4}){2,}\b",
            Always,
        ),
        b(
            Kind::Credential,
            r#"(?i)\b(?:password|passwd|pwd|passcode|secret|client[_ \-]?secret|api[_ \-]?key|access[_ \-]?key|auth[_ \-]?token|token)\s*[:=]\s*(?P<v>[^\s"'`,;]{4,})"#,
            Always,
        ),
        b(Kind::Secret, r"(?i)\b[0-9a-f]{32,}\b", Hex),
        b(Kind::Secret, r"[A-Za-z0-9_\-]{32,}", Random),
    ]
});

/// Dates, which look like phone numbers: `2024-01-15`, `15.01.2024`.
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:\d{4}[\-./]\d{1,2}[\-./]\d{1,2}|\d{1,2}[\-./]\d{1,2}[\-./](?:\d{4}|\d{2}))\b")
        .expect("date pattern")
});

/// The built-in detectors plus the user's patterns and words.
#[derive(Debug, Clone, Default)]
pub struct Detector {
    patterns: Vec<Regex>,
    words: Option<Regex>,
}

impl Detector {
    /// The built-in detectors only.
    pub fn builtin() -> Self {
        Self::default()
    }

    /// The built-in detectors plus `patterns` (regular expressions in the
    /// Rust `regex` syntax, matched as written; `(?i)` ignores case) and
    /// `words` (whole words or phrases, ignoring case and how much space
    /// is between words). Blank entries are skipped; see [`check_pattern`]
    /// and [`check_word`] for the rest.
    pub fn new(patterns: &[String], words: &[String]) -> Result<Self, RulesError> {
        if patterns.len() > MAX_PATTERNS {
            return Err(RulesError(format!("at most {MAX_PATTERNS} patterns")));
        }
        if words.len() > MAX_WORDS {
            return Err(RulesError(format!("at most {MAX_WORDS} words")));
        }
        let patterns = patterns
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| compile_pattern(p))
            .collect::<Result<_, _>>()?;
        for word in words {
            check_word(word)?;
        }
        Ok(Self {
            patterns,
            words: word_regex(words)?,
        })
    }

    /// Every sensitive span of `text`, in order, none overlapping.
    pub fn find(&self, text: &str) -> Vec<Finding> {
        let mut found = Vec::new();
        for regex in &self.patterns {
            push_matches(&mut found, Kind::Pattern, regex, text);
        }
        if let Some(regex) = &self.words {
            push_matches(&mut found, Kind::Word, regex, text);
        }
        for builtin in BUILTINS.iter() {
            for caps in builtin.regex.captures_iter(text) {
                let m = caps
                    .name("v")
                    .unwrap_or_else(|| caps.get(0).expect("group 0"));
                if let Some(range) = check(builtin.check, text, m.range()) {
                    if !range.is_empty() {
                        found.push(Finding {
                            range,
                            kind: builtin.kind,
                        });
                    }
                }
            }
        }
        resolve(found)
    }
}

/// Whether `pattern` can be one of the user's patterns.
pub fn check_pattern(pattern: &str) -> Result<(), RulesError> {
    compile_pattern(pattern).map(|_| ())
}

/// Whether `word` can be on the user's word list.
pub fn check_word(word: &str) -> Result<(), RulesError> {
    if word.trim().is_empty() {
        return Err(RulesError("a word to blur can't be empty".into()));
    }
    if word.chars().count() > MAX_WORD_LEN {
        return Err(RulesError(format!(
            "the word {word:?} is longer than {MAX_WORD_LEN} characters"
        )));
    }
    Ok(())
}

fn compile_pattern(pattern: &str) -> Result<Regex, RulesError> {
    if pattern.trim().is_empty() {
        return Err(RulesError("a pattern can't be empty".into()));
    }
    if pattern.chars().count() > MAX_PATTERN_LEN {
        return Err(RulesError(format!(
            "the pattern {pattern:?} is longer than {MAX_PATTERN_LEN} characters"
        )));
    }
    RegexBuilder::new(pattern)
        .size_limit(PATTERN_SIZE_LIMIT)
        .dfa_size_limit(PATTERN_SIZE_LIMIT)
        .build()
        .map_err(|e| {
            // Syntax errors draw the pattern with a caret over several
            // lines; the last says what is wrong.
            let message = e.to_string();
            let why = message.lines().last().unwrap_or("").trim();
            let why = why.strip_prefix("error: ").unwrap_or(why);
            RulesError(format!("the pattern {pattern:?} is not valid: {why}"))
        })
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// One case-insensitive alternation of `words`, longest first; `\b` only
/// at an end that is a word character, so `c++` and `.internal` work.
fn word_regex(words: &[String]) -> Result<Option<Regex>, RulesError> {
    let mut words: Vec<&str> = words
        .iter()
        .map(|w| w.trim())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return Ok(None);
    }
    words.sort_by_key(|w| std::cmp::Reverse(w.len()));
    let alternatives: Vec<String> = words
        .iter()
        .map(|w| {
            let body: Vec<String> = w.split_whitespace().map(regex::escape).collect();
            let start = if w.starts_with(is_word_char) {
                r"\b"
            } else {
                ""
            };
            let end = if w.ends_with(is_word_char) { r"\b" } else { "" };
            format!("{start}{}{end}", body.join(r"\s+"))
        })
        .collect();
    RegexBuilder::new(&alternatives.join("|"))
        .case_insensitive(true)
        .size_limit(WORDS_SIZE_LIMIT)
        .dfa_size_limit(WORDS_SIZE_LIMIT)
        .build()
        .map(Some)
        .map_err(|e| RulesError(format!("the word list can't be used: {e}")))
}

fn push_matches(found: &mut Vec<Finding>, kind: Kind, regex: &Regex, text: &str) {
    found.extend(
        regex
            .find_iter(text)
            .filter(|m| !m.is_empty())
            .map(|m| Finding {
                range: m.range(),
                kind,
            }),
    );
}

/// Keeps the most specific of overlapping findings (then the longest,
/// then the first), in text order.
fn resolve(mut found: Vec<Finding>) -> Vec<Finding> {
    found.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(b.range.len().cmp(&a.range.len()))
            .then(a.range.start.cmp(&b.range.start))
    });
    let mut kept: Vec<Finding> = Vec::new();
    for f in found {
        let overlaps = kept
            .iter()
            .any(|k| k.range.start < f.range.end && f.range.start < k.range.end);
        if !overlaps {
            kept.push(f);
        }
    }
    kept.sort_by_key(|f| f.range.start);
    kept
}

/// The part of the candidate `text[range]` to report, if any.
fn check(check: Check, text: &str, range: Range<usize>) -> Option<Range<usize>> {
    let s = &text[range.clone()];
    match check {
        Check::Always => Some(range),
        Check::Card => best_part(text, range, &[' ', '-'], is_card),
        Check::Iban => best_part(text, range, &[' '], is_iban),
        Check::Phone => {
            let before = text[..range.start].chars().next_back();
            let after = text[range.end..].chars().next();
            let joined = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
            // Whole only: a part of a longer run of digits is not a phone.
            (!joined(before) && !joined(after) && is_phone(s)).then_some(range)
        }
        Check::Ipv4 => {
            // Not part of a longer dotted number (`1.2.3.4.5`).
            let before = text[..range.start].chars().next_back();
            let mut after = text[range.end..].chars();
            let dotted_on =
                after.next() == Some('.') && after.next().is_some_and(|c| c.is_ascii_digit());
            if before.is_some_and(|c| c == '.' || c.is_ascii_digit()) || dotted_on {
                return None;
            }
            let ip: Ipv4Addr = s.parse().ok()?;
            (!ip.is_loopback() && !ip.is_unspecified()).then_some(range)
        }
        Check::Ipv6 => {
            let before = text[..range.start].chars().next_back();
            let after = text[range.end..].chars().next();
            let joined = |c: Option<char>| c.is_some_and(|c| is_word_char(c) || c == ':');
            if joined(before) || joined(after) || !s.bytes().any(|b| b.is_ascii_digit()) {
                return None;
            }
            let ip: Ipv6Addr = s.parse().ok()?;
            (!ip.is_loopback() && !ip.is_unspecified()).then_some(range)
        }
        Check::Hex => {
            let letters = s.bytes().any(|b| b.is_ascii_alphabetic());
            let digits = s.bytes().filter(u8::is_ascii_digit).count();
            (letters && digits >= 2).then_some(range)
        }
        Check::Random => is_random(s).then_some(range),
    }
}

/// The longest run of whole parts of `text[range]` (split at
/// `separators`) that passes `valid`, the whole candidate first; the
/// leftmost of equally long runs.
fn best_part(
    text: &str,
    range: Range<usize>,
    separators: &[char],
    valid: fn(&str) -> bool,
) -> Option<Range<usize>> {
    let s = &text[range.clone()];
    let mut parts: Vec<Range<usize>> = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        if separators.contains(&c) {
            if let Some(start) = start.take() {
                parts.push(start..i);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(start) = start {
        parts.push(start..s.len());
    }
    let n = parts.len();
    for len in (1..=n).rev() {
        for first in 0..=n - len {
            let part = parts[first].start..parts[first + len - 1].end;
            if valid(&s[part.clone()]) {
                return Some(range.start + part.start..range.start + part.end);
            }
        }
    }
    None
}

/// Luhn checksum over decimal digits (the last is the check digit).
pub fn luhn(digits: &[u8]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let d = u32::from(d);
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                d
            }
        })
        .sum();
    !digits.is_empty() && sum.is_multiple_of(10)
}

/// 13–19 digits passing Luhn, not all the same digit, either in one run or
/// grouped as cards print them (fours, or 4-6-5 / 4-6-4).
fn is_card(s: &str) -> bool {
    if s.chars()
        .any(|c| !c.is_ascii_digit() && c != ' ' && c != '-')
    {
        return false;
    }
    let digits: Vec<u8> = s
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    if !(13..=19).contains(&digits.len()) || digits.iter().all(|&d| d == digits[0]) {
        return false;
    }
    let groups: Vec<usize> = s
        .split([' ', '-'])
        .filter(|g| !g.is_empty())
        .map(str::len)
        .collect();
    let (last, init) = groups.split_last().expect("has digits");
    let grouped = groups.len() == 1
        || matches!(groups.as_slice(), [4, 6, 5] | [4, 6, 4])
        || (init.iter().all(|&g| g == 4) && (1..=4).contains(last));
    grouped && luhn(&digits)
}

/// The IBAN check: the number with its first four characters moved to the
/// end and letters as 10–35, modulo 97. A valid IBAN gives 1.
pub fn iban_mod97(iban: &str) -> u32 {
    let (head, tail) = iban.split_at(4.min(iban.len()));
    let mut rem = 0u32;
    for c in tail.chars().chain(head.chars()) {
        rem = match c {
            '0'..='9' => (rem * 10 + (c as u32 - '0' as u32)) % 97,
            'A'..='Z' => (rem * 100 + (c as u32 - 'A' as u32 + 10)) % 97,
            _ => return 0,
        };
    }
    rem
}

/// Upper case, country letters then check digits, 15–34 characters,
/// written whole or in fours, and passing mod-97.
fn is_iban(s: &str) -> bool {
    let groups: Vec<&str> = s.split(' ').filter(|g| !g.is_empty()).collect();
    if groups.len() > 1 {
        let (last, init) = groups.split_last().expect("several");
        if !init.iter().all(|g| g.len() == 4) || !(1..=4).contains(&last.len()) {
            return false;
        }
    }
    let compact: String = groups.concat();
    let b = compact.as_bytes();
    (15..=34).contains(&b.len())
        && b[..2].iter().all(u8::is_ascii_uppercase)
        && b[2..4].iter().all(u8::is_ascii_digit)
        && b.iter()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && iban_mod97(&compact) == 1
}

/// 7–15 digits, not a date. With a leading `+` or an area code in
/// parentheses that is enough; otherwise two to five groups, not grouped
/// like a large number (`1 234 567`), an account number (`1234-5678-9012`),
/// a decimal or a dotted quad.
fn is_phone(s: &str) -> bool {
    let digits = s.bytes().filter(u8::is_ascii_digit).count();
    if !(7..=15).contains(&digits) || DATE.is_match(s) {
        return false;
    }
    let opens = s.matches('(').count();
    if opens != s.matches(')').count() || opens > 1 {
        return false;
    }
    let groups: Vec<&str> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .collect();
    if s.starts_with('+') || opens == 1 {
        return true;
    }
    if !(2..=5).contains(&groups.len()) || groups[1..].iter().any(|g| g.len() < 2 || g.len() > 5) {
        return false;
    }
    let dots_only = s.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    if dots_only && (groups.len() == 2 || groups.len() == 4) {
        return false;
    }
    if groups.len() >= 3 && groups.iter().all(|g| g.len() == 4) {
        return false;
    }
    !(groups[0].len() <= 3 && groups[1..].iter().all(|g| g.len() == 3))
}

/// Random-looking: at least a fifth upper case, a fifth lower case, two
/// digits, varied (Shannon entropy at least 3.5 bits a character) and with
/// short runs of lower case (3 on average at most). Camel-case names have
/// longer runs (`ClassName`); kebab and snake case no capitals.
fn is_random(s: &str) -> bool {
    let n = s.len() as f64;
    let count = |f: fn(&u8) -> bool| s.bytes().filter(f).count() as f64;
    let lower = count(u8::is_ascii_lowercase);
    let runs = s
        .split(|c: char| !c.is_ascii_lowercase())
        .filter(|r| !r.is_empty())
        .count() as f64;
    count(u8::is_ascii_uppercase) >= 0.2 * n
        && lower >= 0.2 * n
        && count(u8::is_ascii_digit) >= 2.0
        && lower / runs.max(1.0) <= 3.0
        && entropy(s) >= 3.5
}

fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[usize::from(b)] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / n;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The findings of the built-in detectors, as (text, kind).
    fn found(text: &str) -> Vec<(&str, Kind)> {
        found_with(&Detector::builtin(), text)
    }

    fn found_with<'a>(detector: &Detector, text: &'a str) -> Vec<(&'a str, Kind)> {
        detector
            .find(text)
            .into_iter()
            .map(|f| (&text[f.range], f.kind))
            .collect()
    }

    fn kinds(text: &str) -> Vec<Kind> {
        found(text).into_iter().map(|(_, k)| k).collect()
    }

    #[test]
    fn emails() {
        assert_eq!(
            found("Signed in as jane.doe+work@example.co.uk today"),
            [("jane.doe+work@example.co.uk", Kind::Email)]
        );
        assert_eq!(
            found("Mail OTIS_99@Mail-Server.example.COM."),
            [("OTIS_99@Mail-Server.example.COM", Kind::Email)]
        );
        // No top-level domain, or no name: not an address.
        assert!(found("user@localhost and @handle and a@b").is_empty());
    }

    #[test]
    fn phone_numbers() {
        for phone in [
            "+1 415 555 0123",
            "+14155550123",
            "(415) 555-0123",
            "415.555.0123",
            "415-555-0123",
            "+44 20 7946 0958",
            "020 7946 0958",
            "555-1234",
        ] {
            let text = format!("Call {phone} now");
            assert_eq!(found(&text), [(phone, Kind::Phone)], "{phone}");
        }
    }

    #[test]
    fn things_that_are_not_phone_numbers() {
        for text in [
            "Due 2024-01-15 10:30",
            "On 15.01.2024 at noon",
            "Total 1 234 567",
            "pi is 3.14159265",
            "Version 1.2.3",
            "3 4 5 6 7 8 9",
            "ID12345678",
            "12 apples, 7 pears",
            "Price 1234.56789",
        ] {
            assert!(
                !kinds(text).contains(&Kind::Phone),
                "{text}: {:?}",
                found(text)
            );
        }
    }

    #[test]
    fn luhn_checks() {
        let digits = |s: &str| s.bytes().map(|b| b - b'0').collect::<Vec<_>>();
        assert!(luhn(&digits("4111111111111111")));
        assert!(luhn(&digits("378282246310005")));
        assert!(luhn(&digits("79927398713")));
        assert!(!luhn(&digits("4111111111111112")));
        assert!(!luhn(&digits("79927398710")));
        assert!(!luhn(&[]));
    }

    #[test]
    fn card_numbers() {
        for card in [
            "4111 1111 1111 1111",
            "4111-1111-1111-1111",
            "4111111111111111",
            "5555 5555 5555 4444",
            "378282246310005",
            "3782 822463 10005",
        ] {
            let text = format!("Card: {card}.");
            assert_eq!(found(&text), [(card, Kind::CardNumber)], "{card}");
        }
        // Followed by a year: the card part is still found.
        assert_eq!(
            found("4111 1111 1111 1111 2030"),
            [("4111 1111 1111 1111", Kind::CardNumber)]
        );
    }

    #[test]
    fn card_lookalikes_are_not_cards() {
        // Fails Luhn: still a long number, not a card.
        assert_eq!(
            found("4111 1111 1111 1112"),
            [("4111 1111 1111 1112", Kind::LongNumber)]
        );
        // Passes Luhn but is all zeros, or oddly grouped.
        assert!(!kinds("0000 0000 0000 0000").contains(&Kind::CardNumber));
        assert!(!kinds("41 11111 111 11111 1").contains(&Kind::CardNumber));
    }

    #[test]
    fn iban_checks() {
        assert_eq!(iban_mod97("DE89370400440532013000"), 1);
        assert_eq!(iban_mod97("GB82WEST12345698765432"), 1);
        assert_ne!(iban_mod97("DE88370400440532013000"), 1);
        assert_eq!(iban_mod97("DE89 3704"), 0);
    }

    #[test]
    fn ibans() {
        for iban in [
            "DE89 3704 0044 0532 0130 00",
            "DE89370400440532013000",
            "GB82 WEST 1234 5698 7654 32",
            "NL91 ABNA 0417 1643 00",
        ] {
            let text = format!("IBAN {iban} (EUR)");
            assert_eq!(found(&text), [(iban, Kind::Iban)], "{iban}");
        }
        // Wrong check digits: not an IBAN.
        assert!(!kinds("DE88 3704 0044 0532 0130 00").contains(&Kind::Iban));
        // Followed by more digits: the IBAN part.
        assert_eq!(
            found("NL91 ABNA 0417 1643 00 1234")[0],
            ("NL91 ABNA 0417 1643 00", Kind::Iban)
        );
    }

    #[test]
    fn ip_addresses() {
        assert_eq!(
            found("Server 192.168.1.20 is up"),
            [("192.168.1.20", Kind::IpAddress)]
        );
        assert_eq!(
            found("http://10.0.0.1:8080/"),
            [("10.0.0.1", Kind::IpAddress)]
        );
        assert_eq!(
            found("Address 2001:db8::8a2e:370:7334."),
            [("2001:db8::8a2e:370:7334", Kind::IpAddress)]
        );
        assert_eq!(found("gw fe80::1"), [("fe80::1", Kind::IpAddress)]);
        assert_eq!(
            found("[2001:0db8:85a3:0000:0000:8a2e:0370:7334]:443"),
            [("2001:0db8:85a3:0000:0000:8a2e:0370:7334", Kind::IpAddress)]
        );
        for text in [
            "999.1.1.1",
            "127.0.0.1",
            "0.0.0.0",
            "release 1.2.3.4.5",
            "::1",
            "at 10:30:45",
            "use std::fs;",
            "mac 00:1a:2b:3c:4d:5e",
        ] {
            assert!(
                !kinds(text).contains(&Kind::IpAddress),
                "{text}: {:?}",
                found(text)
            );
        }
    }

    #[test]
    fn api_keys_and_tokens() {
        // Split so secret scanners do not take these for real keys.
        let samples = [
            (concat!("AKIA", "IOSFODNN7EXAMPLE"), Kind::AwsKey),
            (
                concat!("ghp_", "aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789"),
                Kind::GithubToken,
            ),
            (
                concat!("gho_", "16C7e42F292c6912E7710c838347Ae178B4a"),
                Kind::GithubToken,
            ),
            (
                concat!("github_", "pat_11ABCDEFG0123456789_abcdefghij"),
                Kind::GithubToken,
            ),
            (concat!("glpat-", "x1y2z3A4B5C6D7E8F9G0"), Kind::GitlabToken),
            (
                concat!("xoxb-", "123456789012-1234567890123-AbCdEfGhIjKl"),
                Kind::SlackToken,
            ),
            (
                concat!("https://hooks.slack.com/", "services/T000/B000/XXXX"),
                Kind::SlackToken,
            ),
            (
                concat!("sk_", "live_4eC39HqLyjWDarjtT1zdp7dc"),
                Kind::StripeKey,
            ),
            (concat!("rk_", "test_51Hx0000aaaaBBBB"), Kind::StripeKey),
            (
                concat!("AIza", "SyD-1234567890abcdefghijklmnopqrstu"),
                Kind::GoogleApiKey,
            ),
            (
                concat!("sk-", "ant-api03-AbCdEf0123456789_xyz-QRS"),
                Kind::ApiKey,
            ),
            (concat!("sk-", "proj-abcdefghijklmnopqrstuv"), Kind::ApiKey),
            (
                concat!(
                    "eyJhbGciOiJIUzI1NiJ9",
                    ".eyJzdWIiOiIxMjM0NTY3ODkwIn0",
                    ".dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
                ),
                Kind::Jwt,
            ),
            ("-----BEGIN RSA PRIVATE KEY-----", Kind::PrivateKey),
            ("-----BEGIN PRIVATE KEY-----", Kind::PrivateKey),
        ];
        for (sample, kind) in samples {
            let text = format!("key: {sample} ");
            let got = found(&text);
            assert_eq!(got, [(sample, kind)], "{sample}");
        }
    }

    #[test]
    fn bearer_tokens_and_credentials() {
        assert_eq!(
            found("Authorization: Bearer abc.DEF-123_456~xyz/7890"),
            [("abc.DEF-123_456~xyz/7890", Kind::BearerToken)]
        );
        assert_eq!(
            found("password: hunter22!"),
            [("hunter22!", Kind::Credential)]
        );
        assert_eq!(
            found("API_KEY=opensesame, then"),
            [("opensesame", Kind::Credential)]
        );
        // Too short to be worth it, or no value at all.
        assert!(found("Password: abc").is_empty());
        assert!(found("Forgot your password?").is_empty());
    }

    #[test]
    fn generic_secrets() {
        let hex = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b";
        assert_eq!(found(&format!("digest {hex}")), [(hex, Kind::Secret)]);
        let random = "Zx9QwErTy1UiOpAsDfGh6JkLmNbVcX3z";
        assert_eq!(found(&format!("secret {random}")), [(random, Kind::Secret)]);
        for text in [
            // Camel case, kebab case, words: not random.
            "MyVeryLongClassNameForTestingStuff12",
            "my-very-long-component-name-for-the-app-v2",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            // Hex without letters is a number; short hex is not a secret.
            "deadbeef1234",
        ] {
            assert!(
                !kinds(text).contains(&Kind::Secret),
                "{text}: {:?}",
                found(text)
            );
        }
    }

    #[test]
    fn long_numbers_and_id_numbers() {
        assert_eq!(
            found("Account 123456789"),
            [("123456789", Kind::LongNumber)]
        );
        assert_eq!(
            found("Ref 1234-5678-9012"),
            [("1234-5678-9012", Kind::LongNumber)]
        );
        assert_eq!(found("SSN 123-45-6789"), [("123-45-6789", Kind::IdNumber)]);
        assert!(found("Order 12345678").is_empty());
    }

    #[test]
    fn overlaps_keep_the_most_specific() {
        // Digits inside an address are part of the address.
        assert_eq!(
            found("123456789@example.com"),
            [("123456789@example.com", Kind::Email)]
        );
        // A card is not also a long number; a key is not also a secret.
        assert_eq!(kinds("4111111111111111"), [Kind::CardNumber]);
        assert_eq!(
            kinds(concat!("ghp_", "aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789")),
            [Kind::GithubToken]
        );
        // Several on one line, in order.
        assert_eq!(
            kinds("jane@example.com, +1 415 555 0123, 192.168.0.1"),
            [Kind::Email, Kind::Phone, Kind::IpAddress]
        );
    }

    #[test]
    fn plain_text_has_nothing() {
        for text in [
            "",
            "Click Save to keep your changes.",
            "Settings > Privacy & Security",
            "Step 3 of 12",
            "Monday, 12:45 PM",
            "Downloads (24 items)",
            "€1,299.00",
        ] {
            assert!(found(text).is_empty(), "{text}: {:?}", found(text));
        }
    }

    #[test]
    fn user_patterns() {
        let detector =
            Detector::new(&[r"INC-\d{6}".into(), r"(?i)project\s+falcon".into()], &[]).unwrap();
        assert_eq!(
            found_with(&detector, "See INC-004211 about Project  Falcon"),
            [
                ("INC-004211", Kind::Pattern),
                ("Project  Falcon", Kind::Pattern)
            ]
        );
        // A pattern wins over a built-in it overlaps.
        let detector = Detector::new(&[r"\S+@acme\.com".into()], &[]).unwrap();
        assert_eq!(
            found_with(&detector, "ops@acme.com"),
            [("ops@acme.com", Kind::Pattern)]
        );
        // Blank patterns are skipped, matches of nothing ignored.
        let detector = Detector::new(&["  ".into(), "x*".into()], &[]).unwrap();
        assert!(found_with(&detector, "abc").is_empty());
    }

    #[test]
    fn bad_patterns_are_refused_with_why() {
        let err = check_pattern("(unclosed").unwrap_err();
        assert!(err.0.contains("(unclosed"), "{err}");
        assert!(err.0.contains("unclosed group"), "{err}");
        assert!(check_pattern("").is_err());
        assert!(check_pattern(&"a".repeat(MAX_PATTERN_LEN + 1)).is_err());
        assert!(check_pattern(r"\d{3}").is_ok());
        assert!(Detector::new(&["[".into()], &[]).is_err());
        let many = vec![r"\d".to_string(); MAX_PATTERNS + 1];
        assert!(Detector::new(&many, &[]).is_err());
        // Too big once compiled.
        assert!(check_pattern(r"\w{1000}\w{1000}\w{1000}").is_err());
    }

    #[test]
    fn word_lists() {
        let words: Vec<String> = ["Acme Corp", "falcon", "intranet.acme.local", "c++", " "]
            .map(String::from)
            .to_vec();
        let detector = Detector::new(&[], &words[..4]).unwrap();
        assert_eq!(
            found_with(
                &detector,
                "ACME   corp hosts Falcon on intranet.acme.local in C++"
            ),
            [
                ("ACME   corp", Kind::Word),
                ("Falcon", Kind::Word),
                ("intranet.acme.local", Kind::Word),
                ("C++", Kind::Word),
            ]
        );
        // Whole words only.
        assert!(found_with(&detector, "falconry and acmes").is_empty());
        // A blank word is refused, as is one too long.
        assert!(Detector::new(&[], &words).is_err());
        assert!(check_word(&"w".repeat(MAX_WORD_LEN + 1)).is_err());
        assert!(check_word("Jane Doe").is_ok());
    }

    #[test]
    fn non_ascii_text_is_fine() {
        assert_eq!(
            found("Grüße an jürgen@example.de — Tel. +49 30 1234567"),
            [
                ("jürgen@example.de", Kind::Email),
                ("+49 30 1234567", Kind::Phone)
            ]
        );
        let detector = Detector::new(&[], &["Zoë".into()]).unwrap();
        assert_eq!(
            found_with(&detector, "zoë, ZOË"),
            [("zoë", Kind::Word), ("ZOË", Kind::Word)]
        );
    }

    #[test]
    fn every_kind_has_a_reason() {
        let all = [
            Kind::Pattern,
            Kind::Word,
            Kind::PrivateKey,
            Kind::AwsKey,
            Kind::GithubToken,
            Kind::GitlabToken,
            Kind::SlackToken,
            Kind::StripeKey,
            Kind::GoogleApiKey,
            Kind::ApiKey,
            Kind::Jwt,
            Kind::BearerToken,
            Kind::Email,
            Kind::Iban,
            Kind::CardNumber,
            Kind::IpAddress,
            Kind::IdNumber,
            Kind::Phone,
            Kind::LongNumber,
            Kind::Credential,
            Kind::Secret,
        ];
        let reasons: std::collections::HashSet<_> = all.iter().map(|k| k.reason()).collect();
        assert_eq!(reasons.len(), all.len());
        assert!(all.windows(2).all(|w| w[0] < w[1]));
    }
}
