//! Wi-Fi QR payloads (port of web/tests/unit/render/wifi.test.ts).

use baradar_core::doc::{WifiSecurity, WifiSettings};
use baradar_core::render::wifi::{escape_wifi_field, wifi_payload, wifi_warnings};

fn w(ssid: &str, password: &str, security: WifiSecurity, hidden: bool) -> WifiSettings {
    WifiSettings {
        ssid: ssid.into(),
        password: password.into(),
        security,
        hidden,
    }
}

use WifiSecurity::{Open, Wep, Wpa};

#[test]
fn payloads() {
    let cases = [
        (
            w("Home", "secret123", Wpa, false),
            "WIFI:T:WPA;S:Home;P:secret123;;",
        ),
        (w("Lab", "abcde", Wep, false), "WIFI:T:WEP;S:Lab;P:abcde;;"),
        (
            w("Café guest", "", Open, false),
            "WIFI:T:nopass;S:Café guest;;",
        ),
        (w("Open", "leftover", Open, false), "WIFI:T:nopass;S:Open;;"),
        (
            w("Hidden", "secret123", Wpa, true),
            "WIFI:T:WPA;S:Hidden;P:secret123;H:true;;",
        ),
        (
            w("Hidden open", "", Open, true),
            "WIFI:T:nopass;S:Hidden open;H:true;;",
        ),
    ];
    for (wifi, expected) in cases {
        assert_eq!(wifi_payload(&wifi).unwrap(), expected);
    }
}

#[test]
fn escaping() {
    assert_eq!(escape_wifi_field("plain"), "plain");
    assert_eq!(escape_wifi_field(r"a\b"), r"a\\b");
    assert_eq!(escape_wifi_field(r#"a;b,c:d"e"#), r#"a\;b\,c\:d\"e"#);
    assert_eq!(escape_wifi_field(r"\;"), r"\\\;");
    assert_eq!(
        wifi_payload(&w(r#"My;Net,"1":\"#, r#"p;a,s:s"\word"#, Wpa, false)).unwrap(),
        r#"WIFI:T:WPA;S:My\;Net\,\"1\"\:\\;P:p\;a\,s\:s\"\\word;;"#
    );
    assert_eq!(
        wifi_payload(&w("Bob's 📶 réseau", "", Open, false)).unwrap(),
        "WIFI:T:nopass;S:Bob's 📶 réseau;;"
    );
}

#[test]
fn required_fields() {
    let e = wifi_payload(&w("", "secret123", Wpa, false)).unwrap_err();
    assert_eq!(e, "Enter the network name (SSID).");
    let e = wifi_payload(&w("Home", "", Wpa, false)).unwrap_err();
    assert_eq!(
        e,
        "Enter the Wi-Fi password, or choose “None” for an open network."
    );
    assert!(
        wifi_payload(&w("Home", "", Wep, false))
            .unwrap_err()
            .contains("password")
    );
    assert_eq!(
        wifi_payload(&w("Home", "", Open, false)).unwrap(),
        "WIFI:T:nopass;S:Home;;"
    );
}

#[test]
fn wpa_warnings() {
    assert_eq!(
        wifi_warnings(&w("x", "short", Wpa, false)),
        vec!["A WPA password has 8 to 63 characters (or 64 hex digits); this one has 5. Phones may not join.".to_owned()]
    );
    assert!(wifi_warnings(&w("x", "12345678", Wpa, false)).is_empty());
    assert!(wifi_warnings(&w("x", &"a".repeat(63), Wpa, false)).is_empty());
    assert!(wifi_warnings(&w("x", &"ab".repeat(32), Wpa, false)).is_empty());
    assert!(wifi_warnings(&w("x", &"z".repeat(64), Wpa, false))[0].contains("8 to 63"));
    // Code points, not bytes.
    assert!(wifi_warnings(&w("x", "ééééééé", Wpa, false))[0].contains("this one has 7"));
}

#[test]
fn wep_and_open_warnings() {
    assert!(wifi_warnings(&w("x", "abcde", Wep, false)).is_empty());
    assert!(wifi_warnings(&w("x", "0123456789", Wep, false)).is_empty());
    assert_eq!(
        wifi_warnings(&w("x", "abcdef", Wep, false)),
        vec![
            "A WEP key has 5 or 13 characters (or 10 or 26 hex digits). Phones may not join."
                .to_owned()
        ]
    );
    assert!(wifi_warnings(&w("x", "x", Open, false)).is_empty());
    assert!(wifi_warnings(&w("x", "", Wpa, false)).is_empty());
}
