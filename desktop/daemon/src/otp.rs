//! Spotting one-time codes in notifications, so a popup can offer "Copy 482913".
//!
//! Deliberately cautious: a code is only offered when the text talks about a code (a
//! keyword), and numbers that look like amounts, card or account digits, dates, times or
//! phone numbers are skipped. A miss only means no button; a wrong code would be worse.

const KEYWORDS: &[&str] = &[
    "otp",
    "one time",
    "one-time",
    "verification",
    "verify",
    "code",
    "passcode",
    "password",
    "pin",
    "2fa",
    "login",
    "authenticat",
    "security code",
];

/// Text right before a number that makes it money, an account or a reference.
const BAD_BEFORE: &[&str] = &[
    "rs", "rs.", "inr", "₹", "$", "usd", "ending", "a/c", "acct", "card", "xx", "no.", "no", "ref",
];

/// The most likely one-time code in a notification, if it clearly carries one.
pub fn find_code(title: &str, text: &str) -> Option<String> {
    let all = format!("{title}\n{text}");
    let lower: Vec<char> = all.to_lowercase().chars().collect();
    // Lower-casing can change the length of some characters; work on the lower-cased text
    // throughout (digits are unaffected).
    let keyword_at = keyword_positions(&lower);
    if keyword_at.is_empty() {
        return None;
    }
    let mut best: Option<(usize, String)> = None;
    for (start, code) in candidates(&lower) {
        let distance = keyword_at.iter().map(|&k| k.abs_diff(start)).min().unwrap_or(usize::MAX);
        if best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((distance, code));
        }
    }
    best.map(|(_, code)| code)
}

fn keyword_positions(lower: &[char]) -> Vec<usize> {
    let s: String = lower.iter().collect();
    let mut out = Vec::new();
    for kw in KEYWORDS {
        for (byte, _) in s.match_indices(kw) {
            // Whole word start ("pin" must not match "shipping").
            let at = s[..byte].chars().count();
            if at > 0 && lower[at - 1].is_alphanumeric() {
                continue;
            }
            out.push(at);
        }
    }
    out
}

/// Digit runs joined by single spaces or hyphens, e.g. "482 913" or "12-11-2026".
struct Cluster {
    start: usize,
    end: usize,
    runs: Vec<String>,
}

fn clusters(c: &[char]) -> Vec<Cluster> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if !c[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut runs = Vec::new();
        loop {
            let s = i;
            while i < c.len() && c[i].is_ascii_digit() {
                i += 1;
            }
            runs.push(c[s..i].iter().collect::<String>());
            if i + 1 < c.len() && (c[i] == ' ' || c[i] == '-') && c[i + 1].is_ascii_digit() {
                i += 1;
                continue;
            }
            break;
        }
        out.push(Cluster { start, end: i, runs });
    }
    out
}

fn candidates(c: &[char]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for cl in clusters(c) {
        let lens: Vec<usize> = cl.runs.iter().map(|r| r.len()).collect();
        let code = match lens.as_slice() {
            [n] if (4..=8).contains(n) => cl.runs[0].clone(),
            [3, 3] | [4, 4] => cl.runs.concat(),
            _ => continue, // dates, phone numbers, long ids, short numbers
        };
        // Word boundaries: "XX1234" or "1234abc" aren't codes.
        if cl.start > 0 && c[cl.start - 1].is_alphanumeric() {
            continue;
        }
        if cl.end < c.len() && c[cl.end].is_alphanumeric() {
            continue;
        }
        // Amounts and dates/times: "5000.00", "5,000", "12/11", "10:30".
        let next_digit = c.get(cl.end + 1).is_some_and(|x| x.is_ascii_digit());
        if next_digit && matches!(c[cl.end], '.' | ',' | ':' | '/') {
            continue;
        }
        if cl.start > 0 && matches!(c[cl.start - 1], ':' | '/' | ',' | '.')
            && cl.start > 1 && c[cl.start - 2].is_ascii_digit()
        {
            continue;
        }
        let before: String = c[cl.start.saturating_sub(8)..cl.start].iter().collect();
        let before = before.trim_end();
        if BAD_BEFORE.iter().any(|b| before.ends_with(b) && word_end(before, b)) {
            continue;
        }
        out.push((cl.start, code));
    }
    out
}

/// `prefix` is a whole word at the end of `before` (so "no" doesn't match "piano").
fn word_end(before: &str, prefix: &str) -> bool {
    let rest = &before[..before.len() - prefix.len()];
    !prefix.chars().next().is_some_and(|f| f.is_alphanumeric())
        || !rest.chars().last().is_some_and(|x| x.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::find_code;

    #[test]
    fn finds_real_codes() {
        let cases = [
            ("HDFC Bank", "123456 is your OTP for txn of INR 5,000.00 at AMAZON on card ending 4321. Valid 10 min", "123456"),
            ("Messages", "Your OTP is 4821", "4821"),
            ("Swiggy", "Use 482 913 to verify your login", "482913"),
            ("Google", "G-482913 is your Google verification code", "482913"),
            ("Your login code", "Enter 90817263 to sign in", "90817263"),
            ("Amazon", "Your one-time password is 7731-2290. Do not share it.", "77312290"),
            ("PhonePe", "Rs.250 sent. Use PIN 5521 only on the app. Ref 99887766", "5521"),
            ("Uber", "Your Uber code: 6402. Never share this code.", "6402"),
        ];
        for (title, text, want) in cases {
            assert_eq!(find_code(title, text).as_deref(), Some(want), "{text}");
        }
    }

    #[test]
    fn ignores_numbers_that_are_not_codes() {
        let cases = [
            ("SBI", "Rs 5000 debited from a/c XX1234 on 12-11-2026"),
            ("Support", "Call 9876543210 for your code"),
            ("Flipkart", "Order 12345678 shipped"),
            ("Calendar", "Meeting at 10:30"),
            ("Bank", "Your verification for card ending 4321 is pending"),
            ("Paytm", "Payment code sent. Amount ₹4500 paid"),
            ("Alerts", "Login from +91 98765 43210 detected"),
            ("Rahul", "see you at 2026"),
        ];
        for (title, text) in cases {
            assert_eq!(find_code(title, text), None, "{text}");
        }
    }
}
