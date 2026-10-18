//! Wi-Fi QR payloads (port of `web/src/render/wifi.ts`):
//! `WIFI:T:<WPA|WEP|nopass>;S:<ssid>;P:<pw>;H:true;;` with backslash escaping of `\ ; , : "` in
//! SSID and password (the ZXing format the iOS Camera app and Android join from).

use crate::doc::{WifiSecurity, WifiSettings};

fn auth(security: WifiSecurity) -> &'static str {
    match security {
        WifiSecurity::Wpa => "WPA",
        WifiSecurity::Wep => "WEP",
        WifiSecurity::Open => "nopass",
    }
}

/// Escapes a `WIFI:` field: backslash first, then `;` `,` `:` `"` (`escapeWifiField`).
pub fn escape_wifi_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The `WIFI:` string for `wifi` (`wifiPayload`).
///
/// # Errors
/// A user-facing message when it cannot be built (empty SSID, missing password for WPA/WEP).
pub fn wifi_payload(wifi: &WifiSettings) -> Result<String, String> {
    if wifi.ssid.is_empty() {
        return Err("Enter the network name (SSID).".into());
    }
    let open = wifi.security == WifiSecurity::Open;
    if !open && wifi.password.is_empty() {
        return Err(
            "Enter the Wi-Fi password, or choose \u{201c}None\u{201d} for an open network.".into(),
        );
    }
    let mut s = format!(
        "WIFI:T:{};S:{};",
        auth(wifi.security),
        escape_wifi_field(&wifi.ssid)
    );
    if !open {
        s.push_str(&format!("P:{};", escape_wifi_field(&wifi.password)));
    }
    if wifi.hidden {
        s.push_str("H:true;");
    }
    s.push(';');
    Ok(s)
}

/// Non-blocking problems a phone may have joining (`wifiWarnings`): a WPA password must be 8–63
/// characters or 64 hex digits, a WEP key 5/13 characters or 10/26 hex digits.
pub fn wifi_warnings(wifi: &WifiSettings) -> Vec<String> {
    let pw = &wifi.password;
    if pw.is_empty() || wifi.security == WifiSecurity::Open {
        return Vec::new();
    }
    let hex = pw.bytes().all(|b| b.is_ascii_hexdigit());
    if wifi.security == WifiSecurity::Wpa {
        // `[...pw].length`: code points.
        let len = pw.chars().count();
        if (8..=63).contains(&len) || (len == 64 && hex) {
            return Vec::new();
        }
        return vec![format!(
            "A WPA password has 8 to 63 characters (or 64 hex digits); this one has {len}. Phones may not join."
        )];
    }
    // `pw.length`: UTF-16 code units.
    let len = pw.encode_utf16().count();
    if matches!(len, 5 | 13) || (hex && matches!(len, 10 | 26)) {
        return Vec::new();
    }
    vec!["A WEP key has 5 or 13 characters (or 10 or 26 hex digits). Phones may not join.".into()]
}
