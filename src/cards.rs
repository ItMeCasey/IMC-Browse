//! Saved payment cards. The card number is encrypted with Windows DPAPI (bound to this Windows
//! user account, separate entropy from passwords) before it touches disk. The CVV/CVC is never
//! stored, and neither is any other field that would make a stolen file directly usable: only the
//! label, name, brand, last four digits and expiry sit in the clear so the list can be shown.
//! The full number exists in memory only while adding or filling a form.

use crate::{config::root, passwords};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, time::SystemTime};

const ENTROPY: &[u8] = b"IMC-Browse-card-v1";

#[derive(Serialize, Deserialize, Clone)]
pub struct Card {
    pub label: String,
    pub name: String,
    pub brand: String,
    pub last4: String,
    pub exp_month: u8,
    pub exp_year: u16,
    /// hex(DPAPI(number))
    pub enc: String,
}

fn path() -> PathBuf {
    root().join("cards.json")
}

pub fn load() -> Vec<Card> {
    fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(c: &[Card]) {
    if let Ok(s) = serde_json::to_string(c) {
        let _ = fs::write(path(), s);
    }
}

pub fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Luhn checksum: catches typos before anything is stored.
pub fn luhn(n: &str) -> bool {
    let mut sum = 0;
    for (i, c) in n.chars().rev().enumerate() {
        let Some(mut d) = c.to_digit(10) else { return false };
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    !n.is_empty() && sum % 10 == 0
}

pub fn brand(n: &str) -> &'static str {
    let p = |k: usize| n.get(..k).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    if n.starts_with('4') {
        "Visa"
    } else if (51..=55).contains(&p(2)) || (2221..=2720).contains(&p(4)) {
        "Mastercard"
    } else if matches!(p(2), 34 | 37) {
        "American Express"
    } else if n.starts_with("6011") || n.starts_with("65") || (644..=649).contains(&p(3)) {
        "Discover"
    } else if matches!(p(2), 36 | 38 | 39) || (300..=305).contains(&p(3)) {
        "Diners Club"
    } else if (3528..=3589).contains(&p(4)) {
        "JCB"
    } else {
        "Card"
    }
}

/// Current (year, month) from the system clock (UTC).
fn now_ym() -> (u16, u8) {
    let days = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as u16, m as u8)
}

/// Validates and stores a card (replacing one with the same last four + expiry).
pub fn add(list: &mut Vec<Card>, label: &str, name: &str, number: &str, exp_month: u8, exp_year: u16) -> Result<String, String> {
    let n = digits(number);
    if !(13..=19).contains(&n.len()) || !luhn(&n) {
        return Err("That card number is not valid (check the digits).".into());
    }
    let year = if exp_year < 100 { 2000 + exp_year } else { exp_year };
    if !(1..=12).contains(&exp_month) {
        return Err("Expiry month must be 1 to 12.".into());
    }
    let (cy, cm) = now_ym();
    if year < cy || (year == cy && exp_month < cm) {
        return Err("That card has expired.".into());
    }
    let enc = passwords::protect(&n, ENTROPY).ok_or("Windows could not encrypt the card.")?;
    let (brand, last4) = (brand(&n).to_string(), n[n.len() - 4..].to_string());
    list.retain(|c| !(c.last4 == last4 && c.exp_month == exp_month && c.exp_year == year));
    let label = if label.trim().is_empty() { format!("{brand} ending {last4}") } else { label.trim().to_string() };
    list.push(Card { label, name: name.trim().to_string(), brand: brand.clone(), last4: last4.clone(), exp_month, exp_year: year, enc });
    Ok(format!("{brand} ending {last4}"))
}

pub fn number(c: &Card) -> Option<String> {
    passwords::unprotect(&c.enc, ENTROPY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luhn_and_brand() {
        assert!(luhn("4111111111111111") && luhn("5555555555554444") && luhn("378282246310005"));
        assert!(!luhn("4111111111111112") && !luhn(""));
        assert_eq!(brand("4111111111111111"), "Visa");
        assert_eq!(brand("5555555555554444"), "Mastercard");
        assert_eq!(brand("378282246310005"), "American Express");
    }

    #[test]
    fn store_encrypted_and_reject_bad() {
        let mut v = vec![];
        assert!(add(&mut v, "", "A Person", "4111 1111 1111 1112", 12, 2099).is_err());
        assert!(add(&mut v, "", "A Person", "4111 1111 1111 1111", 13, 2099).is_err());
        assert!(add(&mut v, "", "A Person", "4111 1111 1111 1111", 1, 2001).is_err());
        add(&mut v, "", "A Person", "4111 1111 1111 1111", 12, 2099).unwrap();
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("4111111111111111") && json.contains("1111"));
        assert_eq!(number(&v[0]).unwrap(), "4111111111111111");
        assert_eq!(v[0].label, "Visa ending 1111");
    }
}
